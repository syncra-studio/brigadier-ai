//! The cleanup ledger: everything each CLI session, worker and conversation created, recorded
//! before it is relied on, and removed exactly when its owner is disposed of.
//!
//! - Artifacts are acknowledged one by one (`cleanup.removed`). A removal that fails keeps its
//!   artifact in the ledger, and the owner stays marked for disposal (`cleanup.requested`), so
//!   the next launch's sweep tries again.
//! - Processes are ended by their recorded identity (pid and start time, with their whole
//!   tree) and by where they work: anything still running inside a folder Brigadier created
//!   for the owner (a worktree, a scratch folder) is ended with it, including processes that
//!   detached from the CLI's tree. A process that detaches *and* leaves those folders is out of
//!   reach.
//! - Only recorded artifacts are ever touched.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};

use brigadier_providers::claude::Claude;
use brigadier_providers::codex::Codex;
use brigadier_providers::{Artifact, Provider, ProviderKind, cleanup};
use brigadier_sandbox::Platform;
use brigadier_store::NewEvent;

use crate::model::{DomainEvent, streams};
use crate::{Core, Error, Result, now_ms};

const PAGE: u32 = 1_000;
/// Rounds of "find the processes in a folder, kill their trees" before giving up: a process
/// may start another while it is being killed.
const KILL_ROUNDS: usize = 3;

/// Removes a git worktree Brigadier created. Provided by whoever owns the git engine.
pub type WorktreeRemover = Arc<
    dyn Fn(
            PathBuf,
            PathBuf,
        ) -> Pin<Box<dyn Future<Output = std::result::Result<(), String>> + Send>>
        + Send
        + Sync,
>;

#[derive(Default)]
struct State {
    artifacts: HashMap<String, Vec<Artifact>>,
    /// Owners whose artifacts are all to be removed.
    disposing: HashSet<String>,
}

impl State {
    /// Whether an owner other than `owner`, and not being disposed of, records the worktree at
    /// `path` too (through symbolic links).
    fn shares_worktree(&self, owner: &str, path: &str) -> bool {
        let real = |path: &str| std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
        let place = real(path);
        self.artifacts.iter().any(|(other, artifacts)| {
            other != owner
                && !self.disposing.contains(other)
                && artifacts.iter().any(|artifact| {
                    matches!(artifact, Artifact::Worktree { path, .. } if real(path) == place)
                })
        })
    }
}

/// What a disposal left behind.
#[derive(Debug, Default)]
pub struct Leftovers {
    pub failures: Vec<String>,
}

impl Leftovers {
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

/// The adapter of the extra account whose CLI home is the given folder, while it is set up.
pub type AccountResolver = Arc<dyn Fn(&str) -> Option<Arc<dyn Provider>> + Send + Sync>;

pub struct CleanupLedger {
    core: Arc<Core>,
    platform: Arc<dyn Platform>,
    claude: Arc<Claude>,
    codex: Arc<Codex>,
    state: Mutex<State>,
    worktrees: Mutex<Option<WorktreeRemover>>,
    accounts: Mutex<Option<AccountResolver>>,
    #[cfg(test)]
    test_providers: Mutex<Option<[Arc<dyn Provider>; 2]>>,
    #[cfg(test)]
    pub(crate) finish_pause: Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
}

impl CleanupLedger {
    pub(crate) async fn load(
        core: Arc<Core>,
        platform: Arc<dyn Platform>,
        claude: Arc<Claude>,
        codex: Arc<Codex>,
    ) -> Result<Self> {
        let mut state = State::default();
        let mut after = 0;
        loop {
            let page = core
                .store()
                .read_stream_since(streams::CLEANUP.into(), after, PAGE)
                .await?;
            for event in &page {
                after = event.stream_seq;
                match crate::sessions::decode(event)? {
                    DomainEvent::CleanupRecorded { owner, artifact } => {
                        let artifacts = state.artifacts.entry(owner).or_default();
                        if !artifacts.contains(&artifact) {
                            artifacts.push(artifact);
                        }
                    }
                    DomainEvent::CleanupRemoved { owner, artifacts } => {
                        if let Some(known) = state.artifacts.get_mut(&owner) {
                            known.retain(|artifact| !artifacts.contains(artifact));
                            if known.is_empty() {
                                state.disposing.remove(&owner);
                            }
                        }
                    }
                    DomainEvent::CleanupRequested { owner } => {
                        state.disposing.insert(owner);
                    }
                    DomainEvent::CleanupFinished { owner } => {
                        state.disposing.remove(&owner);
                    }
                    DomainEvent::CleanupCompleted { owner, .. } => {
                        state.artifacts.remove(&owner);
                        state.disposing.remove(&owner);
                    }
                    _ => {}
                }
            }
            if page.len() < PAGE as usize {
                break;
            }
        }
        state.artifacts.retain(|_, artifacts| !artifacts.is_empty());
        Ok(Self {
            core,
            platform,
            claude,
            codex,
            state: Mutex::new(state),
            worktrees: Mutex::new(None),
            accounts: Mutex::new(None),
            #[cfg(test)]
            test_providers: Mutex::new(None),
            #[cfg(test)]
            finish_pause: Mutex::new(None),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Wires in the extra accounts: a session's files go with the adapter of the home it ran
    /// in while that account is set up (the user's own adapter otherwise).
    pub fn set_account_resolver(&self, resolver: AccountResolver) {
        *self.accounts.lock().unwrap_or_else(|p| p.into_inner()) = Some(resolver);
    }

    /// Keeps flow tests' cleanup on their scripted CLIs, including the own login.
    #[cfg(test)]
    pub(crate) fn set_test_providers(&self, providers: [Arc<dyn Provider>; 2]) {
        *self.test_providers.lock().unwrap() = Some(providers);
    }

    /// The adapter that removes `kind`'s files of a session that ran in `home`.
    fn remover(&self, kind: ProviderKind, home: Option<&str>) -> Arc<dyn Provider> {
        let resolver = self
            .accounts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if let (Some(home), Some(resolver)) = (home, resolver)
            && let Some(provider) = resolver(home).filter(|provider| provider.kind() == kind)
        {
            return provider;
        }
        #[cfg(test)]
        if let Some(providers) = &*self.test_providers.lock().unwrap() {
            return providers
                .iter()
                .find(|provider| provider.kind() == kind)
                .unwrap()
                .clone();
        }
        match kind {
            ProviderKind::Claude => self.claude.clone(),
            ProviderKind::Codex => self.codex.clone(),
        }
    }

    /// Wires in the git engine for removing worktrees.
    pub fn set_worktree_remover(&self, remover: WorktreeRemover) {
        *self.worktrees.lock().unwrap_or_else(|p| p.into_inner()) = Some(remover);
    }

    /// Records `artifact` for `owner`, durably, before it is created or relied on.
    pub async fn record(&self, owner: &str, artifact: Artifact) -> Result<()> {
        if self
            .state()
            .artifacts
            .get(owner)
            .is_some_and(|artifacts| artifacts.contains(&artifact))
        {
            return Ok(());
        }
        self.append(DomainEvent::CleanupRecorded {
            owner: owner.to_owned(),
            artifact: artifact.clone(),
        })
        .await?;
        self.state()
            .artifacts
            .entry(owner.to_owned())
            .or_default()
            .push(artifact);
        Ok(())
    }

    /// Whether any owner still holds `artifact`.
    pub fn holds(&self, artifact: &Artifact) -> bool {
        self.state()
            .artifacts
            .values()
            .any(|artifacts| artifacts.contains(artifact))
    }

    pub fn artifacts(&self, owner: &str) -> Vec<Artifact> {
        self.state()
            .artifacts
            .get(owner)
            .cloned()
            .unwrap_or_default()
    }

    /// A handle a provider records its session's artifacts through.
    pub fn handle(self: &Arc<Self>, owner: String) -> Arc<dyn brigadier_providers::Ledger> {
        Arc::new(OwnerLedger {
            ledger: self.clone(),
            owner,
        })
    }

    /// Ends every process `owner` started (by identity and by folder), keeping its files. Used
    /// when a session stops or hibernates.
    pub async fn end_processes(&self, owner: &str) -> Leftovers {
        let processes: Vec<Artifact> = self
            .artifacts(owner)
            .into_iter()
            .filter(is_process)
            .collect();
        self.remove(owner, processes).await
    }

    /// A process `owner` recorded ended on its own: it is forgotten (whatever still answers to
    /// its recorded identity is ended first).
    pub async fn forget(&self, owner: &str, process: Artifact) {
        if is_process(&process) {
            self.remove(owner, vec![process]).await;
        }
    }

    /// Removes everything `owner` created. What cannot be removed now stays recorded and is
    /// retried by the next sweep.
    pub async fn dispose(&self, owner: &str) -> Leftovers {
        let requested = self.state().disposing.contains(owner);
        if !requested && self.artifacts(owner).is_empty() {
            return Leftovers::default();
        }
        if !requested {
            if let Err(err) = self
                .append(DomainEvent::CleanupRequested {
                    owner: owner.to_owned(),
                })
                .await
            {
                tracing::warn!(owner, error = %err, "could not record a cleanup request");
            }
            self.state().disposing.insert(owner.to_owned());
        }
        let artifacts = self.artifacts(owner);
        let was_empty = artifacts.is_empty();
        // Processes first, so nothing is still writing the files removed next.
        let (processes, files): (Vec<Artifact>, Vec<Artifact>) =
            artifacts.into_iter().partition(is_process);
        let mut leftovers = self.remove(owner, processes).await;
        leftovers
            .failures
            .extend(self.remove(owner, files).await.failures);
        if was_empty {
            // No removal can finish this request. Keep any artifacts recorded while the
            // finish is being stored, both here and on replay.
            if let Err(err) = self
                .append(DomainEvent::CleanupFinished {
                    owner: owner.to_owned(),
                })
                .await
            {
                tracing::warn!(owner, error = %err, "could not record a finished cleanup");
            }
            self.state().disposing.remove(owner);
        }
        leftovers
    }

    /// After a restart, when nothing of Brigadier's runs: archives the Codex threads it still
    /// holds, so the Codex and ChatGPT apps don't list them among the user's own. A session
    /// closed normally archived its thread already; a thread is unarchived when resumed.
    pub async fn archive_codex_threads(&self) {
        #[cfg(test)]
        if self.test_providers.lock().unwrap().is_some() {
            return;
        }
        let threads: Vec<String> = self
            .state()
            .artifacts
            .values()
            .flatten()
            .filter_map(|artifact| match artifact {
                Artifact::CodexThread { thread_id, .. } => Some(thread_id.clone()),
                _ => None,
            })
            .collect();
        if threads.is_empty() {
            return;
        }
        // A conversation resumed meanwhile has a CLI process again: its thread stays open.
        let open = |thread_id: &str| {
            self.state().artifacts.values().any(|artifacts| {
                artifacts.iter().any(is_process)
                    && artifacts.iter().any(|artifact| {
                        matches!(artifact, Artifact::CodexThread { thread_id: held, .. } if held == thread_id)
                    })
            })
        };
        if let Err(err) = self.codex.archive_threads(threads, open).await {
            tracing::debug!(error = %err, "could not archive codex threads");
        }
    }

    /// Crash sweep: ends every process a previous daemon left running, then finishes the
    /// disposals it had started (or that failed before).
    pub async fn sweep(&self) {
        let owners: Vec<String> = self.state().artifacts.keys().cloned().collect();
        let mut ended = 0;
        for owner in &owners {
            let processes: Vec<Artifact> = self
                .artifacts(owner)
                .into_iter()
                .filter(is_process)
                .collect();
            ended += processes.len();
            let leftovers = self.remove(owner, processes).await;
            if !leftovers.is_clean() {
                tracing::warn!(owner, failures = ?leftovers.failures, "processes survived the sweep");
            }
        }
        if ended > 0 {
            tracing::info!(ended, "ended CLI processes left by a previous daemon");
        }
        let disposing: Vec<String> = self.state().disposing.iter().cloned().collect();
        let git_engine = self
            .worktrees
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some();
        for owner in disposing {
            if !git_engine
                && self
                    .artifacts(&owner)
                    .iter()
                    .any(|artifact| matches!(artifact, Artifact::Worktree { .. }))
            {
                // Worktrees need the git engine; the session manager sweeps again once it is in.
                continue;
            }
            let leftovers = self.dispose(&owner).await;
            if !leftovers.is_clean() {
                tracing::warn!(owner, failures = ?leftovers.failures, "cleanup still incomplete");
            }
        }
    }

    /// Every CLI process recorded, with its owner and start time: the processes Brigadier itself
    /// started, which may still run.
    pub fn processes(&self) -> Vec<(String, u32, Option<f64>)> {
        let state = self.state();
        state
            .artifacts
            .iter()
            .flat_map(|(owner, artifacts)| {
                artifacts.iter().filter_map(move |artifact| match artifact {
                    Artifact::Process { pid, started_at_ms } => {
                        Some((owner.clone(), *pid, *started_at_ms))
                    }
                    _ => None,
                })
            })
            .collect()
    }

    /// Every owner with what it still holds, and whether it is being disposed of.
    pub fn owners(&self) -> Vec<(String, Vec<Artifact>, bool)> {
        let state = self.state();
        state
            .artifacts
            .iter()
            .map(|(owner, artifacts)| {
                (
                    owner.clone(),
                    artifacts.clone(),
                    state.disposing.contains(owner),
                )
            })
            .collect()
    }

    /// Records Brigadier branches left in `repo` (unmerged work the user kept), so Storage can
    /// offer them later while they still point where they did.
    pub async fn record_kept_branches(
        &self,
        repo: String,
        branches: Vec<crate::model::KeptBranch>,
    ) -> Result<()> {
        if branches.is_empty() {
            return Ok(());
        }
        self.append(DomainEvent::BranchesKept { repo, branches })
            .await
    }

    /// Removes these of `owner`'s artifacts now (the rest stay recorded); what can't be removed
    /// stays recorded too.
    pub async fn release(&self, owner: &str, artifacts: Vec<Artifact>) -> Leftovers {
        self.remove(owner, artifacts).await
    }

    /// Revoke a failed claim immediately, before another start can reuse it. Its durable
    /// cleanup continues even when the start future was cancelled.
    pub(crate) fn rollback(self: &Arc<Self>, owner: String, artifacts: Vec<Artifact>) {
        if let Some(known) = self.state().artifacts.get_mut(&owner) {
            known.retain(|artifact| !artifacts.contains(artifact));
        }
        let ledger = self.clone();
        tokio::spawn(async move {
            for artifact in artifacts {
                let leftovers = ledger.release(&owner, vec![artifact.clone()]).await;
                if !leftovers.is_clean() {
                    // Keep failed cleanup visible for a later archive or delete.
                    let mut state = ledger.state();
                    let known = state.artifacts.entry(owner.clone()).or_default();
                    if !known.contains(&artifact) {
                        known.push(artifact);
                    }
                }
            }
        });
    }

    /// Forgets a recorded artifact that was never created after all: nothing is touched.
    pub async fn unrecord(&self, owner: &str, artifact: Artifact) -> Result<()> {
        self.append(DomainEvent::CleanupRemoved {
            owner: owner.to_owned(),
            artifacts: vec![artifact.clone()],
        })
        .await?;
        let mut state = self.state();
        if let Some(known) = state.artifacts.get_mut(owner) {
            known.retain(|held| *held != artifact);
            if known.is_empty() {
                state.artifacts.remove(owner);
            }
        }
        Ok(())
    }

    /// Owners marked for disposal.
    pub fn disposing(&self) -> Vec<String> {
        self.state().disposing.iter().cloned().collect()
    }

    /// Removes `artifacts` of `owner`, acknowledging each one that is gone.
    async fn remove(&self, owner: &str, artifacts: Vec<Artifact>) -> Leftovers {
        let mut leftovers = Leftovers::default();
        if artifacts.is_empty() {
            return leftovers;
        }
        let mut removed = Vec::new();
        // The CLIs' own files, by the home of the session that made them.
        let mut cli_files: Vec<(ProviderKind, Option<String>, Vec<Artifact>)> = Vec::new();
        let mut add = |kind: ProviderKind, home: Option<String>, artifact: Artifact| match cli_files
            .iter_mut()
            .find(|(k, h, _)| *k == kind && *h == home)
        {
            Some((_, _, artifacts)) => artifacts.push(artifact),
            None => cli_files.push((kind, home, vec![artifact])),
        };
        for artifact in artifacts {
            match &artifact {
                Artifact::Process { pid, started_at_ms } => {
                    let (platform, pid, started) = (self.platform.clone(), *pid, *started_at_ms);
                    let _ = tokio::task::spawn_blocking(move || {
                        cleanup::end_process(&*platform, pid, started)
                    })
                    .await;
                    removed.push(artifact);
                }
                Artifact::ProcessesIn { dir } => {
                    let platform = self.platform.clone();
                    let dir = PathBuf::from(dir);
                    match tokio::task::spawn_blocking(move || end_in_dir(&*platform, &dir)).await {
                        Ok(Ok(())) => removed.push(artifact),
                        Ok(Err(err)) => leftovers.failures.push(err),
                        Err(err) => leftovers.failures.push(err.to_string()),
                    }
                }
                Artifact::ClaudeSession { home, .. } => {
                    add(ProviderKind::Claude, home.clone(), artifact)
                }
                Artifact::ClaudeProjectDir { .. }
                | Artifact::ClaudeStagingDir { .. }
                | Artifact::ClaudeTempDir { .. } => add(ProviderKind::Claude, None, artifact),
                Artifact::CodexThread { home, .. } => {
                    add(ProviderKind::Codex, home.clone(), artifact)
                }
                Artifact::CodexGeneratedImages { .. } | Artifact::CodexProjectTrust { .. } => {
                    add(ProviderKind::Codex, None, artifact)
                }
                // Run segments share one worktree: the last owner using it removes it.
                Artifact::Worktree { path, .. } if self.state().shares_worktree(owner, path) => {
                    tracing::info!(owner, path, "left a worktree another owner still uses");
                    removed.push(artifact);
                }
                Artifact::Worktree { repo, path } => {
                    let remover = self
                        .worktrees
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .clone();
                    let result = match remover {
                        Some(remove) => remove(PathBuf::from(repo), PathBuf::from(path)).await,
                        None => Err("the git engine is not running".into()),
                    };
                    match result {
                        Ok(()) => removed.push(artifact),
                        Err(err) => leftovers.failures.push(format!("worktree {path}: {err}")),
                    }
                }
                Artifact::CliTrust {
                    cli,
                    file,
                    folder,
                    before,
                } => {
                    let (cli, file, folder, before) =
                        (*cli, PathBuf::from(file), folder.clone(), before.clone());
                    match tokio::task::spawn_blocking(move || {
                        brigadier_providers::trust::undo(cli, &file, &folder, &before)
                    })
                    .await
                    {
                        Ok(Ok(())) => removed.push(artifact),
                        Ok(Err(err)) => leftovers
                            .failures
                            .push(format!("{} folder trust: {err}", cli.label())),
                        Err(err) => leftovers.failures.push(err.to_string()),
                    }
                }
                Artifact::PreviewDataDir { path, identity } => {
                    let data_dir = self.platform.paths().data_dir.clone();
                    let dir = PathBuf::from(path);
                    let identity = *identity;
                    let held = owner.to_owned();
                    match tokio::task::spawn_blocking(move || match identity {
                        Some(identity) => remove_preview_data(&data_dir, &dir, identity, &held),
                        None => Ok(()),
                    })
                    .await
                    {
                        Ok(Ok(())) => removed.push(artifact),
                        Ok(Err(err)) => leftovers.failures.push(format!("{path}: {err}")),
                        Err(err) => leftovers.failures.push(err.to_string()),
                    }
                }
                Artifact::ScratchDir { path } => {
                    let data_dir = self.platform.paths().data_dir.clone();
                    let dir = PathBuf::from(path);
                    // Older previews used path-only session records. They cannot prove
                    // ownership and must not delete a replacement or block the worktree.
                    if owner.starts_with("session:")
                        && !dir.starts_with(&data_dir)
                        && !test_data_folder(&dir)
                    {
                        removed.push(artifact);
                        continue;
                    }
                    match tokio::task::spawn_blocking(move || remove_scratch(&data_dir, &dir)).await
                    {
                        Ok(Ok(())) => removed.push(artifact),
                        Ok(Err(err)) => leftovers.failures.push(format!("{path}: {err}")),
                        Err(err) => leftovers.failures.push(err.to_string()),
                    }
                }
            }
        }
        for (kind, home, artifacts) in cli_files {
            let provider = self.remover(kind, home.as_deref());
            match provider.remove(artifacts.clone()).await {
                Ok(()) => removed.extend(artifacts),
                Err(err) => leftovers
                    .failures
                    .push(format!("{} files: {err}", provider.kind().label())),
            }
        }
        if !removed.is_empty() {
            match self
                .append(DomainEvent::CleanupRemoved {
                    owner: owner.to_owned(),
                    artifacts: removed.clone(),
                })
                .await
            {
                Ok(()) => {
                    let mut state = self.state();
                    if let Some(known) = state.artifacts.get_mut(owner) {
                        known.retain(|artifact| !removed.contains(artifact));
                        if known.is_empty() {
                            state.artifacts.remove(owner);
                            state.disposing.remove(owner);
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!(owner, error = %err, "could not record removed artifacts");
                    leftovers.failures.push(err.to_string());
                }
            }
        }
        if !leftovers.is_clean() {
            tracing::warn!(owner, failures = ?leftovers.failures, "some artifacts were not removed");
        }
        leftovers
    }

    async fn append(&self, event: DomainEvent) -> Result<()> {
        #[cfg(test)]
        if matches!(event, DomainEvent::CleanupFinished { .. }) {
            let pause = self.finish_pause.lock().unwrap().clone();
            if let Some((reached, release)) = pause {
                reached.notify_one();
                release.notified().await;
            }
        }
        let new = NewEvent::new(streams::CLEANUP, event.kind(), now_ms(), &event)?;
        self.core.store().append(vec![new]).await?;
        Ok(())
    }
}

fn is_process(artifact: &Artifact) -> bool {
    matches!(
        artifact,
        Artifact::Process { .. } | Artifact::ProcessesIn { .. }
    )
}

/// Kills every process working inside `dir`, with its tree, until none is left.
fn end_in_dir(platform: &dyn Platform, dir: &Path) -> std::result::Result<(), String> {
    if !dir.exists() {
        return Ok(());
    }
    let processes = platform.processes();
    let find = || {
        processes
            .in_dir(dir)
            .map_err(|err| format!("finding processes in {}: {err}", dir.display()))
    };
    for _ in 0..KILL_ROUNDS {
        let found = find()?;
        if found.is_empty() {
            return Ok(());
        }
        for pid in found {
            if let Err(err) = processes.kill_tree(pid) {
                tracing::debug!(pid, error = %err, "could not kill a process in a Brigadier folder");
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let left = find()?;
    if left.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "processes still running in {}: {left:?}",
            dir.display()
        ))
    }
}

/// Removes a folder Brigadier created: inside its data directory, a task's test data
/// folder, or a recorded preview's short temp folder. Preview removal never follows links.
fn remove_scratch(data_dir: &Path, dir: &Path) -> std::io::Result<()> {
    let inside = dir.starts_with(data_dir) && dir != data_dir;
    if !inside {
        if preview_temp_folder(dir) {
            if !dir.try_exists()? {
                return Ok(());
            }
            let root = dir
                .parent()
                .ok_or_else(|| std::io::Error::other("no temp parent"))?;
            let bound =
                brigadier_sandbox::removal::bind(root, dir).map_err(std::io::Error::other)?;
            return brigadier_sandbox::removal::delete(&bound).map_err(std::io::Error::other);
        }
        return remove_test_data_folder(dir);
    }
    match std::fs::remove_dir_all(dir) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Separately owned preview data can have a caller-chosen name, but must remain strictly
/// inside /tmp and outside both app data directories. The ledger is the ownership proof.
fn remove_preview_data(
    data_dir: &Path,
    dir: &Path,
    identity: (u64, u64),
    owner: &str,
) -> std::io::Result<()> {
    let root = Path::new("/tmp").canonicalize()?;
    if !dir.starts_with(&root) || dir == root {
        return Err(std::io::Error::other("preview data is outside /tmp"));
    }
    for own in [
        Some(data_dir.to_owned()),
        brigadier_sandbox::default_data_dir().ok(),
    ]
    .into_iter()
    .flatten()
    {
        let own = own.canonicalize().unwrap_or(own);
        if dir.starts_with(&own) || own.starts_with(dir) {
            return Err(std::io::Error::other(
                "preview data overlaps an app data directory",
            ));
        }
    }
    match brigadier_sandbox::removal::bind(&root, dir) {
        Ok(bound) => {
            #[cfg(unix)]
            {
                if !bound.is_dir() || bound.unix_identity() != identity {
                    // The recorded folder is gone; its replacement is not ours.
                    tracing::warn!(
                        path = %dir.display(),
                        owner,
                        "preview data folder was replaced; dropping its claim"
                    );
                    return Ok(());
                }
                brigadier_sandbox::removal::delete(&bound).map_err(std::io::Error::other)
            }
            #[cfg(not(unix))]
            {
                let _ = (bound, identity, owner);
                Err(std::io::Error::other("preview data identity requires Unix"))
            }
        }
        Err(brigadier_sandbox::removal::RemovalError::Gone(_)) => Ok(()),
        Err(err) => Err(std::io::Error::other(err)),
    }
}

/// Only this preview-owned name at the temp root is eligible, and only when ledger-recorded.
fn preview_temp_folder(dir: &Path) -> bool {
    let root = Path::new("/tmp");
    #[cfg(test)]
    let test_root = std::env::temp_dir();
    #[cfg(test)]
    let root = if dir.parent() == Some(test_root.as_path()) {
        test_root.as_path()
    } else {
        root
    };
    dir.parent() == Some(root)
        && dir
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("brigadier-pv-"))
            .is_some_and(|id| id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Removes a task's test data folder, and nothing else.
pub(crate) fn remove_test_data_folder(dir: &Path) -> std::io::Result<()> {
    if !test_data_folder(dir) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "not a folder in Brigadier's data directory",
        ));
    }
    match std::fs::remove_dir_all(dir) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

// For the flow tests, which run on Unix only.
#[cfg(all(test, unix))]
impl CleanupLedger {
    /// What a test's daemon still holds, for a test that ends without disposing of it: ends
    /// at once every process recorded and anything running in `dir` or a recorded test data
    /// folder, and returns those folders for the caller to remove.
    pub(crate) fn abandon(&self, dir: &Path) -> Vec<PathBuf> {
        let mut folders = Vec::new();
        for (_, artifacts, _) in self.owners() {
            for artifact in artifacts {
                match artifact {
                    Artifact::Process { pid, started_at_ms } => {
                        cleanup::end_process(&*self.platform, pid, started_at_ms);
                    }
                    Artifact::ScratchDir { path } if test_data_folder(Path::new(&path)) => {
                        folders.push(PathBuf::from(path));
                    }
                    _ => {}
                }
            }
        }
        for place in std::iter::once(dir).chain(folders.iter().map(PathBuf::as_path)) {
            if let Err(err) = end_in_dir(&*self.platform, place) {
                tracing::debug!(error = %err, "could not end a test's processes");
            }
        }
        folders
    }
}

/// A task's test data folder (`/tmp/brigadier-test-<last 8 of its id>`, PLAN.md §10.13):
/// directly in the temp directory, named only that way.
fn test_data_folder(dir: &Path) -> bool {
    let in_temp = dir.parent().is_some_and(|parent| {
        parent == Path::new("/tmp") || parent == std::env::temp_dir().as_path()
    });
    let named = dir
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("brigadier-test-"))
        .is_some_and(|id| id.len() == 8 && id.chars().all(|c| c.is_ascii_alphanumeric()));
    in_temp && named
}

struct OwnerLedger {
    ledger: Arc<CleanupLedger>,
    owner: String,
}

impl brigadier_providers::Ledger for OwnerLedger {
    fn record(
        &self,
        artifact: Artifact,
    ) -> brigadier_providers::BoxFuture<'_, brigadier_providers::Result<()>> {
        Box::pin(async move {
            self.ledger
                .record(&self.owner, artifact)
                .await
                .map_err(|err: Error| brigadier_providers::Error::Ledger(err.to_string()))
        })
    }

    fn holds(&self, artifact: &Artifact) -> bool {
        self.ledger.holds(artifact)
    }
}

#[cfg(test)]
mod test_folder_tests {
    #[test]
    fn a_tasks_test_data_folder_may_be_removed_and_nothing_else_outside_the_data_dir() {
        use super::{preview_temp_folder, test_data_folder};
        use std::path::Path;
        assert!(preview_temp_folder(Path::new(
            "/tmp/brigadier-pv-0123456789abcdef"
        )));
        for path in [
            "/tmp",
            "/tmp/brigadier-pv-0123456789abcdef/sub",
            "/tmp/brigadier-pv-other",
            "/Users/x/brigadier-pv-0123456789abcdef",
        ] {
            assert!(!preview_temp_folder(Path::new(path)), "{path}");
        }
        assert!(test_data_folder(Path::new("/tmp/brigadier-test-40cf6d11")));
        assert!(!test_data_folder(Path::new(
            "/tmp/brigadier-test-40cf6d11/sub"
        )));
        assert!(!test_data_folder(Path::new("/tmp/brigadier-test-../x")));
        assert!(!test_data_folder(Path::new("/tmp/other")));
        assert!(!test_data_folder(Path::new(
            "/Users/x/brigadier-test-40cf6d11"
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worktree(path: &str) -> Artifact {
        Artifact::Worktree {
            repo: "/repo".into(),
            path: path.into(),
        }
    }

    #[test]
    fn a_worktree_stays_while_an_owner_not_being_disposed_of_shares_it() {
        let mut state = State::default();
        state
            .artifacts
            .insert("overnight:r1".into(), vec![worktree("/wt/overnight-1")]);
        state
            .artifacts
            .insert("overnight:r2".into(), vec![worktree("/wt/overnight-1")]);
        state
            .artifacts
            .insert("task:t1".into(), vec![worktree("/wt/task-1")]);
        state.disposing.insert("overnight:r1".into());
        // The earlier segment's cleanup (say, retried at launch) leaves the continuation's.
        assert!(state.shares_worktree("overnight:r1", "/wt/overnight-1"));
        assert!(!state.shares_worktree("task:t1", "/wt/task-1"));
        // Once the continuation is being disposed of too, the worktree can go.
        state.disposing.insert("overnight:r2".into());
        assert!(!state.shares_worktree("overnight:r1", "/wt/overnight-1"));
    }
}
