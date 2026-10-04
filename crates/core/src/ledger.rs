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
use brigadier_providers::{Artifact, Provider, cleanup};
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

pub struct CleanupLedger {
    core: Arc<Core>,
    platform: Arc<dyn Platform>,
    claude: Arc<Claude>,
    codex: Arc<Codex>,
    state: Mutex<State>,
    worktrees: Mutex<Option<WorktreeRemover>>,
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
                        }
                    }
                    DomainEvent::CleanupRequested { owner } => {
                        state.disposing.insert(owner);
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
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
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

    /// Removes everything `owner` created. What cannot be removed now stays recorded and is
    /// retried by the next sweep.
    pub async fn dispose(&self, owner: &str) -> Leftovers {
        let requested = self.state().disposing.contains(owner);
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
        // Processes first, so nothing is still writing the files removed next.
        let (processes, files): (Vec<Artifact>, Vec<Artifact>) =
            artifacts.into_iter().partition(is_process);
        let mut leftovers = self.remove(owner, processes).await;
        leftovers
            .failures
            .extend(self.remove(owner, files).await.failures);
        if self.artifacts(owner).is_empty() {
            self.state().disposing.remove(owner);
        }
        leftovers
    }

    /// After a restart, when nothing of Brigadier's runs: archives the Codex threads it still
    /// holds, so the Codex and ChatGPT apps don't list them among the user's own. A session
    /// closed normally archived its thread already; a thread is unarchived when resumed.
    pub async fn archive_codex_threads(&self) {
        let threads: Vec<String> = self
            .state()
            .artifacts
            .values()
            .flatten()
            .filter_map(|artifact| match artifact {
                Artifact::CodexThread { thread_id } => Some(thread_id.clone()),
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
                        matches!(artifact, Artifact::CodexThread { thread_id: held } if held == thread_id)
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
        let mut claude = Vec::new();
        let mut codex = Vec::new();
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
                Artifact::ClaudeSession { .. }
                | Artifact::ClaudeProjectDir { .. }
                | Artifact::ClaudeStagingDir { .. }
                | Artifact::ClaudeTempDir { .. } => claude.push(artifact),
                Artifact::CodexThread { .. }
                | Artifact::CodexGeneratedImages { .. }
                | Artifact::CodexProjectTrust { .. } => codex.push(artifact),
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
                Artifact::ScratchDir { path } => {
                    let data_dir = self.platform.paths().data_dir.clone();
                    let dir = PathBuf::from(path);
                    match tokio::task::spawn_blocking(move || remove_scratch(&data_dir, &dir)).await
                    {
                        Ok(Ok(())) => removed.push(artifact),
                        Ok(Err(err)) => leftovers.failures.push(format!("{path}: {err}")),
                        Err(err) => leftovers.failures.push(err.to_string()),
                    }
                }
            }
        }
        for (provider, artifacts) in [
            (&*self.claude as &dyn Provider, claude),
            (&*self.codex as &dyn Provider, codex),
        ] {
            if artifacts.is_empty() {
                continue;
            }
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

/// Removes a folder Brigadier created, only inside its data directory or a task's test data
/// folder in the temp directory.
fn remove_scratch(data_dir: &Path, dir: &Path) -> std::io::Result<()> {
    let inside = dir.starts_with(data_dir) && dir != data_dir;
    if !inside && !test_data_folder(dir) {
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
        use super::test_data_folder;
        use std::path::Path;
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
