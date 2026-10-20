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
/// The owner of a tab's chat ([`crate::model::TabChat`]) starts so. Its records are kept in a
/// stream of their own, which goes once everything it recorded is gone: nothing of it stays.
pub const TAB_CHAT_OWNER: &str = "tabchat:";
/// Where tab chats' owners keep their records: this, then the owner.
const TAB_CHAT_STREAMS: &str = "cleanup:tabchat:";

/// The stream `owner`'s records go to.
fn owner_stream(owner: &str) -> String {
    if owner.starts_with(TAB_CHAT_OWNER) {
        format!("cleanup:{owner}")
    } else {
        streams::CLEANUP.to_owned()
    }
}
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
    /// Owners whose latest disposal request was a Close workspace (counts only while they
    /// are disposing).
    closing: HashSet<String>,
    /// Every CLI session Brigadier ever started, by its CLI's own id, kept after its files
    /// went: what proves a session's files found later are Brigadier's.
    native: HashMap<String, NativeSession>,
    /// The branches Brigadier made (or kept for the user), by repository: what proves a branch
    /// Brigadier's to delete.
    made: HashSet<(String, String)>,
    /// Tab chats whose records went this run: one recorded again must still be removed.
    forgotten: HashSet<String>,
}

/// The branches Brigadier's records prove its own: those it made or kept for the user, and,
/// from before it recorded them, those named like its own (`brigadier/…`) or an overnight
/// run's (`overnight/…`). What it may delete.
#[derive(Debug, Clone, Default)]
pub struct BranchOwnership {
    made: HashSet<(String, String)>,
}

impl BranchOwnership {
    /// Whether `name` in the repository at `repo` is Brigadier's.
    pub fn owns(&self, repo: &std::path::Path, name: &str) -> bool {
        let made = |repo: &std::path::Path| {
            self.made
                .contains(&(repo.to_string_lossy().into_owned(), name.to_owned()))
        };
        name.starts_with("brigadier/")
            || name.starts_with("overnight/")
            || made(repo)
            || made(&repo_key(repo))
    }

    /// The branches its records show Brigadier made (or kept) in the repository at `repo`,
    /// whatever their prefix.
    pub fn made_in(&self, repo: &std::path::Path) -> Vec<String> {
        let keys = [
            repo.to_string_lossy().into_owned(),
            repo_key(repo).to_string_lossy().into_owned(),
        ];
        let mut names: Vec<String> = self
            .made
            .iter()
            .filter(|(made, _)| keys.contains(made))
            .map(|(_, name)| name.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

/// The path a branch record keeps its repository by: the real one where it resolves, so a
/// link or a trailing `/` names the same repository.
fn repo_key(repo: &std::path::Path) -> PathBuf {
    std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_owned())
}

/// A CLI session Brigadier started, as its ledger recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeSession {
    pub kind: ProviderKind,
    /// The folder it ran in (unknown for sessions recorded before Brigadier noted it).
    pub cwd: Option<String>,
}

impl State {
    fn note_native(&mut self, artifact: &Artifact) {
        let (id, kind, cwd) = match artifact {
            Artifact::ClaudeSession {
                session_id, cwd, ..
            } => (session_id, ProviderKind::Claude, cwd),
            Artifact::CodexThread { thread_id, cwd, .. } => (thread_id, ProviderKind::Codex, cwd),
            _ => return,
        };
        let known = self
            .native
            .entry(id.clone())
            .or_insert(NativeSession { kind, cwd: None });
        if cwd.is_some() {
            known.cwd.clone_from(cwd);
        }
    }

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
    /// Adopted entries left in place on purpose, and why: they are no longer the entry that
    /// was shown (changed or replaced), or a folder others still use. Forgotten, not retried.
    pub kept: Vec<String>,
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
    /// A tab chat's records go, or one is added, one at a time: none slips in between.
    tab_chats: tokio::sync::Mutex<()>,
    worktrees: Mutex<Option<WorktreeRemover>>,
    accounts: Mutex<Option<AccountResolver>>,
    #[cfg(test)]
    test_providers: Mutex<Option<[Arc<dyn Provider>; 2]>>,
    /// Holds the next append of an event of this kind: signals the first `Notify` when it is
    /// reached, then waits for the second.
    #[cfg(test)]
    append_pause: Mutex<Option<AppendPause>>,
    /// Holds the next forgetting of an owner, as `append_pause` holds an append.
    #[cfg(test)]
    forget_pause: Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
}

#[cfg(test)]
type AppendPause = (
    &'static str,
    Arc<tokio::sync::Notify>,
    Arc<tokio::sync::Notify>,
);

impl CleanupLedger {
    pub(crate) async fn load(
        core: Arc<Core>,
        platform: Arc<dyn Platform>,
        claude: Arc<Claude>,
        codex: Arc<Codex>,
    ) -> Result<Self> {
        let mut state = State::default();
        let mut events = read_stream(&core, streams::CLEANUP.into()).await?;
        for head in core.store().stream_heads(TAB_CHAT_STREAMS.into()).await? {
            events.extend(read_stream(&core, head.stream).await?);
        }
        events.sort_by_key(|event| event.seq);
        {
            for event in &events {
                match crate::sessions::decode(event)? {
                    DomainEvent::CleanupRecorded { owner, artifact } => {
                        state.note_native(&artifact);
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
                    DomainEvent::CleanupRequested { owner, close } => {
                        if close {
                            state.closing.insert(owner.clone());
                        } else {
                            state.closing.remove(&owner);
                        }
                        state.disposing.insert(owner);
                    }
                    DomainEvent::CleanupFinished { owner } => {
                        state.disposing.remove(&owner);
                    }
                    DomainEvent::CleanupCompleted { owner, .. } => {
                        state.artifacts.remove(&owner);
                        state.disposing.remove(&owner);
                    }
                    DomainEvent::BranchMade { repo, branch } => {
                        state.made.insert((repo, branch));
                    }
                    DomainEvent::BranchesKept { repo, branches } => {
                        for branch in branches {
                            state.made.insert((repo.clone(), branch.name));
                        }
                    }
                    _ => {}
                }
            }
        }
        state.artifacts.retain(|_, artifacts| !artifacts.is_empty());
        Ok(Self {
            core,
            platform,
            claude,
            codex,
            state: Mutex::new(state),
            tab_chats: tokio::sync::Mutex::new(()),
            worktrees: Mutex::new(None),
            accounts: Mutex::new(None),
            #[cfg(test)]
            test_providers: Mutex::new(None),
            #[cfg(test)]
            append_pause: Mutex::new(None),
            #[cfg(test)]
            forget_pause: Mutex::new(None),
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
        let tab_chat = owner.starts_with(TAB_CHAT_OWNER);
        let _one = if tab_chat {
            Some(self.tab_chats.lock().await)
        } else {
            None
        };
        if tab_chat && self.state().forgotten.contains(owner) {
            // Its chat's records went already: this one is to go too, at the latest when the
            // next launch sweeps.
            self.append(DomainEvent::CleanupRequested {
                owner: owner.to_owned(),
                close: false,
            })
            .await?;
            let mut state = self.state();
            state.forgotten.remove(owner);
            state.disposing.insert(owner.to_owned());
        }
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
        let mut state = self.state();
        state.note_native(&artifact);
        state
            .artifacts
            .entry(owner.to_owned())
            .or_default()
            .push(artifact);
        Ok(())
    }

    /// Every CLI session this data directory's Brigadier ever started, by its CLI's own id,
    /// including the ones whose files are gone.
    pub fn native_sessions(&self) -> HashMap<String, NativeSession> {
        self.state().native.clone()
    }

    /// Takes on `artifacts` for `owner`: the entries Free up space showed and the user approved,
    /// each recorded (durably, before anything is removed) with the identity it had when shown,
    /// so that removing them now, or the launch sweep's retry after a crash, removes exactly
    /// those entries. Then disposes of `owner`.
    pub async fn adopt(&self, owner: &str, artifacts: Vec<Artifact>) -> Result<Leftovers> {
        // Codex threads go by id through Codex's own `thread/delete`, which also takes them off
        // the Codex app's lists; the caller checked their files are still the ones shown.
        if artifacts.iter().any(|artifact| {
            !matches!(
                artifact,
                Artifact::Adopted { .. } | Artifact::CodexThread { .. }
            )
        }) {
            return Err(Error::Invalid(
                "only adopted entries and Codex threads can be taken on this way".into(),
            ));
        }
        for artifact in artifacts {
            self.record(owner, artifact).await?;
        }
        Ok(self.dispose(owner).await)
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
        self.dispose_as(owner, false).await
    }

    /// [`Self::dispose`] for the user's Close workspace (`session:` owners): a restart that
    /// takes the disposal back knows it was one ([`Self::closing`]).
    pub async fn dispose_closing(&self, owner: &str) -> Leftovers {
        self.dispose_as(owner, true).await
    }

    /// Whether `owner` is disposing because of the user's Close workspace.
    pub fn closing(&self, owner: &str) -> bool {
        let state = self.state();
        state.disposing.contains(owner) && state.closing.contains(owner)
    }

    async fn dispose_as(&self, owner: &str, close: bool) -> Leftovers {
        let (requested, marked) = {
            let state = self.state();
            (
                state.disposing.contains(owner),
                state.closing.contains(owner),
            )
        };
        if !requested && self.artifacts(owner).is_empty() {
            return Leftovers::default();
        }
        if !requested || (close && !marked) {
            if let Err(err) = self
                .append(DomainEvent::CleanupRequested {
                    owner: owner.to_owned(),
                    close,
                })
                .await
            {
                crate::logging::private_trace!(owner.starts_with(TAB_CHAT_OWNER), warn, "could not record a cleanup request"; owner, error = %err);
            }
            let mut state = self.state();
            state.disposing.insert(owner.to_owned());
            if close {
                state.closing.insert(owner.to_owned());
            } else {
                state.closing.remove(owner);
            }
        }
        let artifacts = self.artifacts(owner);
        // Processes first, so nothing is still writing the files removed next.
        let (processes, files): (Vec<Artifact>, Vec<Artifact>) =
            artifacts.into_iter().partition(is_process);
        let mut leftovers = self.remove(owner, processes).await;
        let removed = self.remove(owner, files).await;
        leftovers.failures.extend(removed.failures);
        leftovers.kept.extend(removed.kept);
        if leftovers.is_clean()
            && owner.starts_with(TAB_CHAT_OWNER)
            && self.artifacts(owner).is_empty()
        {
            // A tab's chat leaves no record: with all of it gone, its records go too.
            self.forget_owner(owner).await;
        } else if leftovers.is_clean() {
            // Everything in the snapshot is gone. Artifacts recorded meanwhile stay, both here
            // and on replay. Always end the request durably: a record's event can be stored
            // before it shows in memory, so memory can't tell whether one raced this disposal.
            if let Err(err) = self
                .append(DomainEvent::CleanupFinished {
                    owner: owner.to_owned(),
                })
                .await
            {
                crate::logging::private_trace!(owner.starts_with(TAB_CHAT_OWNER), warn, "could not record a finished cleanup"; owner, error = %err);
            }
            self.state().disposing.remove(owner);
        }
        leftovers
    }

    /// Forgets `owner` (a tab chat's) altogether once nothing of it is left: its records go,
    /// with the CLI sessions they named. If they can't go now, its finished cleanup stays
    /// recorded and the next launch's sweep tries again.
    async fn forget_owner(&self, owner: &str) {
        #[cfg(test)]
        {
            let pause = self.forget_pause.lock().unwrap().take();
            if let Some((reached, release)) = pause {
                reached.notify_one();
                release.notified().await;
            }
        }
        let _one = self.tab_chats.lock().await;
        if !self.artifacts(owner).is_empty() {
            // Recorded meanwhile: it is to go too, at the latest when the next launch sweeps.
            if self
                .append(DomainEvent::CleanupRequested {
                    owner: owner.to_owned(),
                    close: false,
                })
                .await
                .is_err()
            {
                tracing::warn!("could not keep a tab chat's cleanup request");
            }
            self.state().disposing.insert(owner.to_owned());
            return;
        }
        let stream = owner_stream(owner);
        let natives: Vec<String> = match read_stream(&self.core, stream.clone()).await {
            Ok(events) => events
                .iter()
                .filter_map(|event| match crate::sessions::decode(event) {
                    Ok(DomainEvent::CleanupRecorded {
                        artifact:
                            Artifact::ClaudeSession { session_id: id, .. }
                            | Artifact::CodexThread { thread_id: id, .. },
                        ..
                    }) => Some(id),
                    _ => None,
                })
                .collect(),
            Err(err) => {
                crate::logging::private_trace!(owner.starts_with(TAB_CHAT_OWNER), warn, "could not read a tab chat's records"; owner, error = %err);
                return;
            }
        };
        if let Err(err) = self.core.store().delete_streams(vec![stream]).await {
            crate::logging::private_trace!(owner.starts_with(TAB_CHAT_OWNER), warn, "could not forget a tab chat's records"; owner, error = %err);
            return;
        }
        // Nor a copy in the store's write-ahead log.
        if self.core.store().scrub().await.is_err() {
            tracing::warn!("could not empty the store's write-ahead log after a tab chat");
        }
        let mut state = self.state();
        state.disposing.remove(owner);
        state.artifacts.remove(owner);
        state.forgotten.insert(owner.to_owned());
        for id in natives {
            state.native.remove(&id);
        }
    }

    /// After a restart, when nothing of Brigadier's runs: archives the Codex threads it still
    /// holds, so the user's own Codex apps don't list them among their threads. A session
    /// closed normally archived its thread already; a thread is unarchived when resumed.
    pub async fn archive_codex_threads(&self) {
        #[cfg(test)]
        if self.test_providers.lock().unwrap().is_some() {
            return;
        }
        let (private, threads): (Vec<_>, Vec<_>) = self
            .state()
            .artifacts
            .iter()
            .flat_map(|(owner, artifacts)| {
                artifacts.iter().filter_map(move |artifact| match artifact {
                    Artifact::CodexThread { thread_id, .. } => {
                        Some((owner.starts_with(TAB_CHAT_OWNER), thread_id.clone()))
                    }
                    _ => None,
                })
            })
            .partition(|(private, _)| *private);
        let private: Vec<String> = private.into_iter().map(|(_, id)| id).collect();
        let threads: Vec<String> = threads.into_iter().map(|(_, id)| id).collect();
        if threads.is_empty() && private.is_empty() {
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
        if !threads.is_empty()
            && let Err(err) = self.codex.archive_threads(threads, false, &open).await
        {
            tracing::debug!(error = %err, "could not archive codex threads");
        }
        if !private.is_empty()
            && self
                .codex
                .archive_threads(private, true, open)
                .await
                .is_err()
        {
            tracing::debug!(conversation = "tab chat", "could not archive codex threads");
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
                crate::logging::private_trace!(owner.starts_with(TAB_CHAT_OWNER), warn, "processes survived the sweep"; owner, failures = ?leftovers.failures);
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
                crate::logging::private_trace!(owner.starts_with(TAB_CHAT_OWNER), warn, "cleanup still incomplete"; owner, failures = ?leftovers.failures);
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
        {
            let mut state = self.state();
            for branch in &branches {
                state.made.insert((repo.clone(), branch.name.clone()));
            }
        }
        self.append(DomainEvent::BranchesKept { repo, branches })
            .await
    }

    /// Records that Brigadier created `branch` in `repo`, once.
    pub async fn record_branch_made(&self, repo: &std::path::Path, branch: &str) -> Result<()> {
        let key = (
            repo_key(repo).to_string_lossy().into_owned(),
            branch.to_owned(),
        );
        if !self.state().made.insert(key.clone()) {
            return Ok(());
        }
        self.append(DomainEvent::BranchMade {
            repo: key.0,
            branch: key.1,
        })
        .await
    }

    /// The branches Brigadier's records prove its own, as they are now.
    pub fn branch_ownership(&self) -> BranchOwnership {
        BranchOwnership {
            made: self.state().made.clone(),
        }
    }

    /// Removes these of `owner`'s artifacts now (the rest stay recorded); what can't be removed
    /// stays recorded too.
    pub async fn release(&self, owner: &str, artifacts: Vec<Artifact>) -> Leftovers {
        self.remove(owner, artifacts).await
    }

    /// Revoke a failed claim immediately, before another start can reuse it. Its durable
    /// cleanup continues even when the start future was cancelled.
    #[cfg(any(target_os = "macos", all(test, unix)))]
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

    /// Takes back `owner`'s disposal: what it still holds stays recorded and is kept, and no
    /// sweep removes it.
    pub async fn cancel_disposal(&self, owner: &str) -> Result<()> {
        if !self.state().disposing.contains(owner) {
            return Ok(());
        }
        self.append(DomainEvent::CleanupFinished {
            owner: owner.to_owned(),
        })
        .await?;
        self.state().disposing.remove(owner);
        Ok(())
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
                    let private_logs = owner.starts_with(TAB_CHAT_OWNER);
                    match tokio::task::spawn_blocking(move || {
                        end_in_dir(&*platform, &dir, private_logs)
                    })
                    .await
                    {
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
                Artifact::Adopted {
                    root,
                    path,
                    identity,
                    empty_only,
                    ..
                } => {
                    let platform = self.platform.clone();
                    let (root, path, identity, empty_only) = (
                        PathBuf::from(root),
                        PathBuf::from(path),
                        *identity,
                        *empty_only,
                    );
                    match tokio::task::spawn_blocking(move || {
                        remove_adopted(&*platform, &root, &path, identity, empty_only)
                    })
                    .await
                    {
                        Ok(Ok(Adopted::Removed)) => removed.push(artifact),
                        Ok(Ok(Adopted::Kept(why))) => {
                            crate::logging::private_trace!(owner.starts_with(TAB_CHAT_OWNER), info, "left an adopted entry in place"; owner, why);
                            leftovers.kept.push(why);
                            removed.push(artifact);
                        }
                        Ok(Err(err)) => leftovers.failures.push(err),
                        Err(err) => leftovers.failures.push(err.to_string()),
                    }
                }
                // Run segments share one worktree: the last owner using it removes it.
                Artifact::Worktree { path, .. } if self.state().shares_worktree(owner, path) => {
                    crate::logging::private_trace!(owner.starts_with(TAB_CHAT_OWNER), info, "left a worktree another owner still uses"; owner, path);
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
            match provider
                .remove_with_privacy(artifacts.clone(), owner.starts_with(TAB_CHAT_OWNER))
                .await
            {
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
                    crate::logging::private_trace!(owner.starts_with(TAB_CHAT_OWNER), warn, "could not record removed artifacts"; owner, error = %err);
                    leftovers.failures.push(err.to_string());
                }
            }
        }
        if !leftovers.is_clean() {
            crate::logging::private_trace!(owner.starts_with(TAB_CHAT_OWNER), warn, "some artifacts were not removed"; owner, failures = ?leftovers.failures);
        }
        leftovers
    }

    async fn append(&self, event: DomainEvent) -> Result<()> {
        #[cfg(test)]
        {
            let pause = self.append_pause.lock().unwrap().clone();
            if let Some((kind, reached, release)) = pause
                && kind == event.kind()
            {
                reached.notify_one();
                release.notified().await;
            }
        }
        let stream = match &event {
            DomainEvent::CleanupRecorded { owner, .. }
            | DomainEvent::CleanupRemoved { owner, .. }
            | DomainEvent::CleanupRequested { owner, .. }
            | DomainEvent::CleanupFinished { owner }
            | DomainEvent::CleanupCompleted { owner, .. } => owner_stream(owner),
            _ => streams::CLEANUP.to_owned(),
        };
        let new = NewEvent::new(stream, event.kind(), now_ms(), &event)?;
        self.core.store().append(vec![new]).await?;
        Ok(())
    }
}

/// Every event of `stream`, oldest first.
async fn read_stream(core: &Core, stream: String) -> Result<Vec<brigadier_store::StoredEvent>> {
    let mut events = Vec::new();
    let mut after = 0;
    loop {
        let page = core
            .store()
            .read_stream_since(stream.clone(), after, PAGE)
            .await?;
        let full = page.len() == PAGE as usize;
        if let Some(last) = page.last() {
            after = last.stream_seq;
        }
        events.extend(page);
        if !full {
            return Ok(events);
        }
    }
}

/// What became of an adopted entry.
enum Adopted {
    /// Gone (removed now, or already).
    Removed,
    /// Left in place on purpose, and why.
    Kept(String),
}

/// Removes an adopted entry, but only while it is the very entry that was shown: bound under
/// its root again with its recorded identity (no link below the root, no `..`), nothing running
/// in it, and, for a folder others may share, only while it is empty. Anything else stays.
fn remove_adopted(
    platform: &dyn Platform,
    root: &Path,
    path: &Path,
    identity: (u64, u64),
    empty_only: bool,
) -> std::result::Result<Adopted, String> {
    use brigadier_sandbox::removal::{self, Identity, RemovalError};
    if removal::is_gone(path) {
        return Ok(Adopted::Removed);
    }
    let bound = match removal::bind_as(root, path, Identity::from_parts(identity)) {
        Ok(bound) => bound,
        Err(RemovalError::Gone(_)) => return Ok(Adopted::Removed),
        Err(err @ (RemovalError::Changed(_) | RemovalError::Symlink(_))) => {
            return Ok(Adopted::Kept(format!("{err}; it is not what was shown")));
        }
        Err(err) => return Err(err.to_string()),
    };
    if bound.is_dir() {
        match platform.processes().in_dir(bound.path()) {
            Ok(found) if found.is_empty() => {}
            Ok(_) => return Err(format!("{}: something runs in it now", path.display())),
            // Windows can't name the processes working in a folder yet, but it refuses to
            // remove a folder one works in, so an empty one may still go.
            Err(brigadier_sandbox::Error::Unsupported { .. }) if cfg!(windows) && empty_only => {}
            Err(err) => return Err(format!("{}: {err}", path.display())),
        }
    }
    if empty_only {
        let empty = std::fs::read_dir(bound.path()).is_ok_and(|mut e| e.next().is_none());
        if !empty {
            return Ok(Adopted::Kept(format!(
                "{} still holds other files",
                path.display()
            )));
        }
        return removal::remove_empty_dir(&bound)
            .map(|()| Adopted::Removed)
            .map_err(|err| err.to_string());
    }
    removal::delete(&bound)
        .map(|()| Adopted::Removed)
        .map_err(|err| err.to_string())
}

fn is_process(artifact: &Artifact) -> bool {
    matches!(
        artifact,
        Artifact::Process { .. } | Artifact::ProcessesIn { .. }
    )
}

/// Kills every process working inside `dir`, with its tree, until none is left.
fn end_in_dir(
    platform: &dyn Platform,
    dir: &Path,
    private_logs: bool,
) -> std::result::Result<(), String> {
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
                crate::logging::private_trace!(private_logs, debug, "could not kill a process in a Brigadier folder"; pid, error = %err);
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
            if let Err(err) = end_in_dir(&*self.platform, place, false) {
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
    fn private_logs(&self) -> bool {
        self.owner.starts_with(TAB_CHAT_OWNER)
    }

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
    use brigadier_providers::cli::CliEnv;
    use brigadier_store::{Store, StoreConfig};
    use tokio::sync::Notify;

    /// A session's branch under Settings › Git's prefix is listed as the engine's in its own
    /// repository only, so other sessions can be kept off it.
    #[test]
    fn made_in_lists_the_branches_made_in_that_repository_whatever_their_prefix() {
        let owned = BranchOwnership {
            made: HashSet::from([
                ("/repos/a".to_owned(), "sg/aa/session".to_owned()),
                ("/repos/a".to_owned(), "brigadier/bb/task-1".to_owned()),
                ("/repos/b".to_owned(), "sg/cc/session".to_owned()),
            ]),
        };
        assert_eq!(
            owned.made_in(Path::new("/repos/a")),
            ["brigadier/bb/task-1", "sg/aa/session"]
        );
        assert!(owned.made_in(Path::new("/repos/c")).is_empty());
    }

    /// A store and a platform of their own, to load ledgers from again and again.
    struct Fixture {
        dir: PathBuf,
        core: Arc<Core>,
        platform: Arc<dyn Platform>,
    }

    impl Fixture {
        async fn new() -> Self {
            let dir =
                std::env::temp_dir().join(format!("brigadier-ledger-{}", uuid::Uuid::now_v7()));
            let store = Store::open(StoreConfig {
                db_path: dir.join("test.db"),
                blobs_dir: dir.join("blobs"),
                readers: 1,
            })
            .unwrap();
            let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
                data_dir: Some(dir.join("data")),
            })
            .unwrap();
            Self {
                core: Core::load(store).await.unwrap(),
                platform,
                dir,
            }
        }

        async fn load(&self) -> Arc<CleanupLedger> {
            let env = Arc::new(CliEnv::from_vars([]));
            Arc::new(
                CleanupLedger::load(
                    self.core.clone(),
                    self.platform.clone(),
                    Arc::new(Claude::new(self.platform.clone(), env.clone())),
                    Arc::new(Codex::new(self.platform.clone(), env)),
                )
                .await
                .unwrap(),
            )
        }

        /// A scratch folder in the data folder, and its artifact.
        fn folder(&self, name: &str) -> (PathBuf, Artifact) {
            let path = self.platform.paths().data_dir.join(name);
            std::fs::create_dir_all(&path).unwrap();
            let artifact = Artifact::ScratchDir {
                path: path.to_string_lossy().into_owned(),
            };
            (path, artifact)
        }

        async fn append(&self, event: DomainEvent) {
            let new = NewEvent::new(streams::CLEANUP, event.kind(), 0, &event).unwrap();
            self.core.store().append(vec![new]).await.unwrap();
        }

        /// The kinds of `owner`'s cleanup events, in order, and the last event's position.
        async fn kinds(&self, owner: &str) -> (Vec<&'static str>, i64) {
            let events = self
                .core
                .store()
                .read_stream_since(streams::CLEANUP.into(), 0, 1000)
                .await
                .unwrap();
            let kinds = events
                .iter()
                .filter_map(|event| {
                    let decoded = crate::sessions::decode(event).unwrap();
                    match &decoded {
                        DomainEvent::CleanupRequested { owner: held, .. }
                        | DomainEvent::CleanupRecorded { owner: held, .. }
                        | DomainEvent::CleanupRemoved { owner: held, .. }
                        | DomainEvent::CleanupCompleted { owner: held, .. }
                        | DomainEvent::CleanupFinished { owner: held }
                            if held == owner =>
                        {
                            Some(decoded.kind())
                        }
                        _ => None,
                    }
                })
                .collect();
            (kinds, events.last().map_or(0, |event| event.stream_seq))
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Starts disposing of `owner` and holds its next append of a `kind` event until the
    /// returned `Notify` is notified.
    async fn pause_at(
        ledger: &Arc<CleanupLedger>,
        kind: &'static str,
        owner: &'static str,
    ) -> (tokio::task::JoinHandle<Leftovers>, Arc<Notify>) {
        let (reached, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        *ledger.append_pause.lock().unwrap() = Some((kind, reached.clone(), release.clone()));
        let disposing = {
            let ledger = ledger.clone();
            tokio::spawn(async move { ledger.dispose(owner).await })
        };
        reached.notified().await;
        *ledger.append_pause.lock().unwrap() = None;
        (disposing, release)
    }

    #[tokio::test]
    async fn empty_disposal_preserves_a_record_during_finish_and_does_not_repeat_events() {
        let fixture = Fixture::new().await;
        let owner = "preview-data:empty-disposal-race";
        // A request left by an older daemon, with no artifacts to acknowledge.
        let request = DomainEvent::CleanupRequested {
            owner: owner.into(),
            close: false,
        };
        fixture.append(request.clone()).await;
        let ledger = fixture.load().await;
        let (disposing, release) = pause_at(&ledger, "cleanup.finished", owner).await;
        let (path, artifact) = fixture.folder("race-artifact");
        ledger.record(owner, artifact.clone()).await.unwrap();
        release.notify_one();
        assert!(disposing.await.unwrap().is_clean());
        assert_eq!(ledger.artifacts(owner), vec![artifact.clone()]);
        assert!(!ledger.disposing().contains(&owner.to_owned()));
        let reloaded = fixture.load().await;
        assert_eq!(reloaded.artifacts(owner), ledger.artifacts(owner));
        assert!(!reloaded.disposing().contains(&owner.to_owned()));
        reloaded.sweep().await;
        assert!(path.is_dir());
        assert_eq!(reloaded.artifacts(owner), vec![artifact]);
        assert!(reloaded.dispose(owner).await.is_clean());
        assert!(!path.exists());
        let (kinds, last) = fixture.kinds(owner).await;
        assert_eq!(
            kinds,
            vec![
                "cleanup.requested",
                "cleanup.recorded",
                "cleanup.finished",
                "cleanup.requested",
                "cleanup.removed",
                "cleanup.finished"
            ]
        );
        for ledger in [&reloaded, &fixture.load().await] {
            for _ in 0..3 {
                assert!(ledger.dispose(owner).await.is_clean());
                assert!(ledger.dispose("never-recorded").await.is_clean());
            }
        }
        let after = fixture
            .core
            .store()
            .read_stream_since(streams::CLEANUP.into(), last, 1000)
            .await
            .unwrap();
        assert!(after.is_empty());

        // A pending empty request also finishes when no record races with it.
        fixture.append(request).await;
        let pending = fixture.load().await;
        assert!(pending.disposing().contains(&owner.to_owned()));
        assert!(pending.dispose(owner).await.is_clean());
        let finished = fixture.load().await;
        assert!(!finished.disposing().contains(&owner.to_owned()));
        assert!(finished.artifacts(owner).is_empty());
    }

    #[tokio::test]
    async fn a_record_made_while_a_disposal_removes_others_survives_a_restart() {
        let fixture = Fixture::new().await;
        let owner = "preview-data:removal-race";
        let ledger = fixture.load().await;
        let (old, held) = fixture.folder("old");
        ledger.record(owner, held).await.unwrap();
        // Say a preview re-records its data folder while the old one is being acknowledged.
        let (disposing, release) = pause_at(&ledger, "cleanup.removed", owner).await;
        let (path, artifact) = fixture.folder("new");
        ledger.record(owner, artifact.clone()).await.unwrap();
        release.notify_one();
        assert!(disposing.await.unwrap().is_clean());
        assert!(!old.exists());
        assert_eq!(ledger.artifacts(owner), vec![artifact.clone()]);
        assert!(!ledger.disposing().contains(&owner.to_owned()));
        let reloaded = fixture.load().await;
        assert!(!reloaded.disposing().contains(&owner.to_owned()));
        reloaded.sweep().await;
        assert!(path.is_dir());
        assert_eq!(reloaded.artifacts(owner), vec![artifact]);

        // When nothing is recorded meanwhile, the request still ends with a finish.
        assert!(reloaded.dispose(owner).await.is_clean());
        assert!(!path.exists());
        let (kinds, _) = fixture.kinds(owner).await;
        assert_eq!(
            kinds,
            vec![
                "cleanup.recorded",
                "cleanup.requested",
                "cleanup.recorded",
                "cleanup.removed",
                "cleanup.finished",
                "cleanup.requested",
                "cleanup.removed",
                "cleanup.finished"
            ]
        );
        assert!(fixture.load().await.disposing().is_empty());
    }

    /// Disposes of tab chat `owner` (it holds nothing, so its records are to go) and holds the
    /// forgetting until the returned `Notify` is notified.
    async fn pause_forgetting(
        ledger: &Arc<CleanupLedger>,
        owner: &'static str,
    ) -> (tokio::task::JoinHandle<Leftovers>, Arc<Notify>) {
        let (reached, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        *ledger.forget_pause.lock().unwrap() = Some((reached.clone(), release.clone()));
        let disposing = {
            let ledger = ledger.clone();
            tokio::spawn(async move { ledger.dispose(owner).await })
        };
        reached.notified().await;
        (disposing, release)
    }

    #[tokio::test]
    async fn a_record_made_while_a_tab_chat_is_forgotten_stays_for_the_next_sweep() {
        let fixture = Fixture::new().await;
        let owner = "tabchat:forget-race";
        let ledger = fixture.load().await;
        let (old, held) = fixture.folder("tab-old");
        ledger.record(owner, held).await.unwrap();
        let (disposing, release) = pause_forgetting(&ledger, owner).await;
        assert!(!old.exists());
        // Say a process tree of the chat is recorded just as its records go.
        let (path, artifact) = fixture.folder("tab-new");
        let recording = {
            let (ledger, artifact) = (ledger.clone(), artifact.clone());
            tokio::spawn(async move { ledger.record(owner, artifact).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        release.notify_one();
        assert!(disposing.await.unwrap().is_clean());
        recording.await.unwrap().unwrap();
        assert_eq!(ledger.artifacts(owner), vec![artifact.clone()]);
        let reloaded = fixture.load().await;
        assert_eq!(reloaded.artifacts(owner), vec![artifact]);
        assert!(reloaded.disposing().contains(&owner.to_owned()));
        reloaded.sweep().await;
        assert!(!path.exists(), "the launch sweep removes it");
        assert!(reloaded.artifacts(owner).is_empty());
        assert!(fixture.load().await.artifacts(owner).is_empty());
    }

    #[tokio::test]
    async fn a_record_made_after_a_tab_chat_was_forgotten_goes_at_the_next_sweep() {
        let fixture = Fixture::new().await;
        let owner = "tabchat:late-record";
        let ledger = fixture.load().await;
        let (_, held) = fixture.folder("late-old");
        ledger.record(owner, held).await.unwrap();
        assert!(ledger.dispose(owner).await.is_clean());
        assert!(fixture.load().await.artifacts(owner).is_empty());
        let (path, artifact) = fixture.folder("late-new");
        ledger.record(owner, artifact).await.unwrap();
        let reloaded = fixture.load().await;
        assert!(reloaded.disposing().contains(&owner.to_owned()));
        reloaded.sweep().await;
        assert!(!path.exists(), "the launch sweep removes it");
        assert!(fixture.load().await.artifacts(owner).is_empty());
    }

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

    /// An entry Free up space showed, bound with its identity.
    fn adopted(root: &Path, path: &Path, empty_only: bool) -> Artifact {
        let bound = brigadier_sandbox::removal::bind(root, path).unwrap();
        Artifact::Adopted {
            root: root.display().to_string(),
            path: path.display().to_string(),
            identity: bound.identity().parts(),
            evidence: "they used its tools".into(),
            empty_only,
        }
    }

    /// Adoption survives a crash: once the approved entries are recorded and their removal was
    /// asked for, the next launch's sweep removes exactly those entries, keeps one replaced
    /// since (and forgets it), and never touches what is beside them.
    #[tokio::test]
    async fn adopted_entries_go_only_as_they_were_shown_even_after_a_crash() {
        let fixture = Fixture::new().await;
        let root = fixture.dir.join("home");
        let project = root.join("projects").join("-data-orch-a");
        std::fs::create_dir_all(&project).unwrap();
        let (shown, replaced, beside) = (
            project.join("a.jsonl"),
            project.join("b.jsonl"),
            project.join("mine.jsonl"),
        );
        for path in [&shown, &replaced, &beside] {
            std::fs::write(path, "{}\n").unwrap();
        }
        let owner = "sweep:crashed";
        let ledger = fixture.load().await;
        for artifact in [
            adopted(&root, &shown, false),
            adopted(&root, &replaced, false),
            adopted(&root, &project, true),
        ] {
            ledger.record(owner, artifact).await.unwrap();
        }
        // The daemon stopped after asking for the removal, before anything went.
        fixture
            .append(DomainEvent::CleanupRequested {
                owner: owner.into(),
                close: false,
            })
            .await;
        drop(ledger);
        // Meanwhile, b.jsonl became another file at the same path.
        // (Written beside it first, so the new file can't reuse the old one's inode.)
        let next = project.join("b.next");
        std::fs::write(&next, "the user's own\n").unwrap();
        std::fs::rename(&next, &replaced).unwrap();

        let ledger = fixture.load().await;
        ledger.sweep().await;
        assert!(!shown.exists());
        assert_eq!(
            std::fs::read_to_string(&replaced).unwrap(),
            "the user's own\n"
        );
        assert!(beside.exists());
        // Not empty: the folder others still use stays.
        assert!(project.is_dir());
        // Kept entries are forgotten, not retried.
        assert!(ledger.artifacts(owner).is_empty());
        assert!(!ledger.disposing().contains(&owner.to_owned()));

        // Taking entries on directly removes them at once, and only adopted entries are taken.
        let more = project.join("c.jsonl");
        std::fs::write(&more, "{}\n").unwrap();
        let leftovers = ledger
            .adopt("sweep:now", vec![adopted(&root, &more, false)])
            .await
            .unwrap();
        assert!(leftovers.is_clean() && leftovers.kept.is_empty());
        assert!(!more.exists());
        // One replaced between the preview and the removal stays, and the caller hears why.
        let swapped = project.join("d.jsonl");
        std::fs::write(&swapped, "{}\n").unwrap();
        let shown = adopted(&root, &swapped, false);
        std::fs::write(&next, "the user's own\n").unwrap();
        std::fs::rename(&next, &swapped).unwrap();
        let leftovers = ledger.adopt("sweep:swapped", vec![shown]).await.unwrap();
        assert_eq!(leftovers.kept.len(), 1, "{leftovers:?}");
        assert!(swapped.exists());
        let (folder, scratch) = fixture.folder("not-adoptable");
        assert!(ledger.adopt("sweep:bad", vec![scratch]).await.is_err());
        assert!(folder.is_dir());
    }
}
