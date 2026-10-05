//! Settings → Storage: what Brigadier keeps on disk, and what it can clean up.
//!
//! A scan lists only what Brigadier can prove is its own, and nothing anything live uses:
//!
//! - ownership comes from a record: the cleanup ledger, a task's or session's workspace, a
//!   kept-branch record, the catalog, or a folder's owner marker, never from a name that
//!   merely looks like Brigadier's;
//! - whatever a live conversation, worker or CLI session holds is left out, as is anything a
//!   process still works in;
//! - each item is bound when it is listed (see [`brigadier_sandbox::removal`]) and checked
//!   again before it goes; git worktrees and branches go only through git.
//!
//! Safe items come checked; the ones that cost something to lose (unmerged work, a Brain,
//! models, recordings) are offered unchecked.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use brigadier_providers::Artifact;
use brigadier_sandbox::removal::{self, Bound};
use brigadier_store::{Compacted, DbSpace};

use super::{SessionManager, blocking, git_error};
use crate::model::{
    Conversation, ConversationId, Environment, KeptBranch, Lifecycle, Project, Setup, streams,
};
use crate::overnight::OvernightRun;
use crate::storage::{CleanBadge, CleanCategory, CleanItem, ProjectUsage, SharedPart, SharedUsage};
use crate::work::Task;
use crate::{Error, Result};

/// How long a folder in a shared temp place must have been left alone to count as left over.
#[cfg(unix)]
const TEMP_MIN_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// A working folder in the data directory no record claims is left alone this long first
/// (something may be about to record it).
const SCRATCH_MIN_AGE: Duration = Duration::from_secs(60 * 60);
/// Logs older than the daemon's week of logs.
const LOG_MAX_AGE: Duration = Duration::from_secs(8 * 24 * 60 * 60);
/// Recordings older than this are offered.
const RECORDING_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// Compacting the database is offered from this much free space on.
const COMPACT_MIN_BYTES: u64 = 1024 * 1024;
/// The database compacts on its own from this much free space on, or from
/// [`AUTO_COMPACT_SHARE`] of the file when that is at least [`AUTO_COMPACT_MIN_BYTES`].
const AUTO_COMPACT_BYTES: u64 = 32 * 1024 * 1024;
const AUTO_COMPACT_SHARE: f64 = 0.25;
const AUTO_COMPACT_MIN_BYTES: u64 = 4 * 1024 * 1024;
/// A larger database (the data that stays) is only compacted from Settings › Storage: the end
/// of a `VACUUM`, its copy back, can't be stopped, and holds up a write for about 2.5 ms per
/// MB (measured), so about 0.3 s here at most.
const AUTO_COMPACT_MAX_LIVE_BYTES: u64 = 128 * 1024 * 1024;

/// A removal the daemon runs for a picked item.
#[derive(Debug, Clone)]
pub enum Action {
    /// Rebuildable data, deleted for good.
    Delete(Vec<(Bound, u64)>),
    /// A folder removed only while it is still empty.
    RemoveEmptyDir(Bound),
    /// What the user may want back, moved to the Trash.
    Trash(Vec<(Bound, u64)>),
    /// A worktree removed through git, its uncommitted work kept as a WIP commit first.
    RemoveWorktree {
        repo: PathBuf,
        worktree: Bound,
        bytes: u64,
    },
    /// `git worktree prune` in `repo`, whose stale records are all Brigadier's.
    PruneWorktrees { repo: PathBuf },
    /// A branch deleted at the tip it had when it was listed.
    DeleteBranch {
        repo: PathBuf,
        branch: KeptBranch,
        merged: bool,
    },
    /// Everything the cleanup ledger records for `owner`.
    Dispose { owner: String, bytes: u64 },
    /// A downloaded model's folder, deleted only while nothing uses the model.
    DeleteModel {
        /// The dictation speech model; otherwise the Brain's embedding model.
        speech: bool,
        folder: Bound,
        bytes: u64,
    },
    /// Stored content no event references.
    CollectBlobs,
    /// The database rebuilt without its free pages.
    Compact,
    /// One of the daemon's own kinds, run by the daemon.
    External(String),
}

/// A listed item and what removing it does.
pub struct ScanItem {
    pub item: CleanItem,
    pub action: Action,
    /// Purely Brigadier's, rebuildable, and clearly left over: housekeeping removes it without
    /// asking.
    pub routine: bool,
}

/// What the daemon knows that the session manager doesn't.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScanContext {
    /// Dictation, or the speech model's download, is running.
    pub speech_busy: bool,
}

/// What one removal gave back.
#[derive(Debug, Clone, Default)]
pub struct Cleaned {
    pub reclaimed: u64,
    pub trashed: u64,
    /// Entries of the item that stayed, and why; the others are gone.
    pub failures: Vec<String>,
}

/// The records a scan works from, taken once.
struct Records {
    data_dir: PathBuf,
    projects: Vec<Project>,
    conversations: Vec<Conversation>,
    tasks: HashMap<ConversationId, Vec<Task>>,
    runs: HashMap<ConversationId, Vec<OvernightRun>>,
    owners: Vec<(String, Vec<Artifact>, bool)>,
    kept: Vec<(String, KeptBranch)>,
}

impl Records {
    fn conversation(&self, id: &str) -> Option<&Conversation> {
        self.conversations.iter().find(|c| c.id.0 == id)
    }

    fn task(&self, id: &str) -> Option<(&Conversation, &Task)> {
        self.tasks.iter().find_map(|(conversation, tasks)| {
            let task = tasks.iter().find(|task| task.id.0 == id)?;
            Some((self.conversation(&conversation.0)?, task))
        })
    }

    /// The overnight run `id` and its session, while the session exists.
    fn run(&self, id: &str) -> Option<(&Conversation, &OvernightRun)> {
        self.runs.iter().find_map(|(conversation, runs)| {
            let run = runs.iter().find(|run| run.id.0 == id)?;
            Some((self.conversation(&conversation.0)?, run))
        })
    }

    fn repo_of(conversation: &Conversation) -> Option<PathBuf> {
        match &conversation.setup {
            Some(Setup::Session { repo, .. }) => Some(PathBuf::from(repo)),
            _ => None,
        }
    }
}

/// How a ledger owner stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnerState {
    /// Something live holds it: never offered.
    Live,
    /// Its disposal started and did not finish.
    Unfinished,
    /// What it belonged to is gone.
    Orphaned,
}

impl SessionManager {
    /// Scans the data directory and Brigadier's places outside it.
    pub async fn scan_storage(&self, context: ScanContext) -> Result<(Vec<ScanItem>, ScanUsage)> {
        let records = self.storage_records().await?;
        let states = self.owner_states(&records);
        let embeddings_busy = self.embeddings_busy();
        let store = self.core.store().clone();
        let blobs = match store.collectable_blobs().await {
            Ok(found) => found,
            Err(err) => {
                tracing::warn!(error = %err, "could not count collectable blobs");
                (0, 0)
            }
        };
        let space = store.free_space().await.ok();
        let mut project_blobs = HashMap::new();
        for project in &records.projects {
            let hashes = store
                .blob_hashes_of(project_streams(&records, project))
                .await?;
            let files: Vec<PathBuf> = hashes
                .iter()
                .map(|hash| store.blobs().file_of(hash))
                .collect();
            project_blobs.insert(project.id.0.clone(), files);
        }
        let git = self.git.clone();
        let platform = self.runtime.platform().clone();
        let (mut items, usage) = blocking(move || {
            let mut scan = Scanner {
                records: &records,
                states: &states,
                git: &git,
                platform: &*platform,
                items: Vec::new(),
            };
            scan.worktrees();
            scan.stale_worktree_records();
            scan.branches();
            scan.ledger_leftovers();
            scan.working_folders();
            scan.session_temp_folders();
            scan.brains();
            scan.logs_and_recordings();
            scan.models(embeddings_busy, context.speech_busy);
            let usage = scan.usage(&project_blobs);
            Ok((scan.items, usage))
        })
        .await?;
        if blobs.0 > 0 {
            items.push(ScanItem {
                item: item(
                    CleanCategory::LogsAndData,
                    format!(
                        "{} nothing uses any more",
                        counted(
                            usize::try_from(blobs.0).unwrap_or(usize::MAX),
                            "stored file",
                            "stored files"
                        )
                    ),
                    Some(self.data_dir.join("blobs")),
                    blobs.1,
                    "Attachments, reports and artifacts of conversations that were deleted.",
                    true,
                ),
                action: Action::CollectBlobs,
                routine: false,
            });
        }
        if let Some(space) = space
            && space.free_bytes + space.wal_bytes >= COMPACT_MIN_BYTES
        {
            items.push(ScanItem {
                item: item(
                    CleanCategory::Database,
                    "Compact the database".into(),
                    Some(self.data_dir.join("brigadier.db")),
                    space.free_bytes + space.wal_bytes,
                    "Space left behind by deleted conversations, given back by rebuilding the \
                     database file. Runs only while nothing is working.",
                    true,
                ),
                action: Action::Compact,
                routine: false,
            });
        }
        Ok((items, usage))
    }

    async fn storage_records(&self) -> Result<Records> {
        let catalog = self.core.catalog();
        let mut tasks = HashMap::new();
        let mut runs = HashMap::new();
        for conversation in &catalog.conversations {
            if let Ok(board) = self.core.board(&conversation.id).await {
                tasks.insert(conversation.id.clone(), board.sorted_tasks());
                runs.insert(
                    conversation.id.clone(),
                    board.runs.values().cloned().collect(),
                );
            }
        }
        Ok(Records {
            data_dir: self.data_dir.clone(),
            projects: catalog.projects,
            conversations: catalog.conversations,
            tasks,
            runs,
            owners: self.runtime.ledger().owners(),
            kept: self.kept_branches().await?,
        })
    }

    /// Every kept-branch record, oldest first.
    pub(super) async fn kept_branches(&self) -> Result<Vec<(String, KeptBranch)>> {
        let mut kept = Vec::new();
        let mut after = 0;
        loop {
            let page = self
                .core
                .store()
                .read_stream_since(streams::CLEANUP.into(), after, 1_000)
                .await?;
            for event in &page {
                after = event.stream_seq;
                if event.kind != "branches.kept" {
                    continue;
                }
                if let crate::model::DomainEvent::BranchesKept { repo, branches } =
                    crate::sessions::decode(event)?
                {
                    kept.extend(branches.into_iter().map(|branch| (repo.clone(), branch)));
                }
            }
            if page.len() < 1_000 {
                return Ok(kept);
            }
        }
    }

    fn owner_states(&self, records: &Records) -> HashMap<String, OwnerState> {
        let processes = self.runtime.platform().processes();
        let mut states: HashMap<String, OwnerState> = records
            .owners
            .iter()
            .map(|(owner, artifacts, disposing)| {
                let state = if *disposing {
                    OwnerState::Unfinished
                } else {
                    let alive = match owner.split_once(':') {
                        Some(("orch" | "chat" | "session", id)) => {
                            records.conversation(id).is_some()
                        }
                        Some(("task", id)) => records.task(id).is_some(),
                        // A run's worktree serves Continue and Merge while its session exists.
                        Some(("overnight", id)) => records.run(id).is_some(),
                        // Commit-message writers, Brain jobs, raw sessions: alive while one of
                        // their processes still runs.
                        _ => artifacts.iter().any(|artifact| match artifact {
                            Artifact::Process { pid, started_at_ms } => {
                                processes.is_alive(*pid)
                                    && started_at_ms.is_none_or(|started| {
                                        processes
                                            .start_time_ms(*pid)
                                            .is_ok_and(|now| (now - started).abs() < 1_000.0)
                                    })
                            }
                            _ => false,
                        }),
                    };
                    if alive {
                        OwnerState::Live
                    } else {
                        OwnerState::Orphaned
                    }
                };
                (owner.clone(), state)
            })
            .collect();
        // Run segments share one worktree: it is never offered while a live owner holds it, not
        // even by an owner whose cleanup didn't finish.
        let live: HashSet<PathBuf> = records
            .owners
            .iter()
            .filter(|(owner, _, _)| states.get(owner) == Some(&OwnerState::Live))
            .flat_map(|(_, artifacts, _)| worktree_places(artifacts))
            .collect();
        for (owner, artifacts, _) in &records.owners {
            if states.get(owner) != Some(&OwnerState::Live)
                && worktree_places(artifacts).any(|place| live.contains(&place))
            {
                states.insert(owner.clone(), OwnerState::Live);
            }
        }
        states
    }

    /// The database's space, when enough of it is free to compact it on its own: a rebuild
    /// costs about the data that stays, so only a large share or a lot of space is worth it.
    pub async fn worth_compacting(&self) -> Option<DbSpace> {
        let space = self.core.store().free_space().await.ok()?;
        let free = space.free_bytes;
        let share = free as f64 / space.file_bytes.max(1) as f64;
        (free >= AUTO_COMPACT_BYTES
            || (free >= AUTO_COMPACT_MIN_BYTES && share >= AUTO_COMPACT_SHARE))
            .then_some(space)
    }

    /// The Brain's embedding model is loaded, or a Brain job embeds.
    fn embeddings_busy(&self) -> bool {
        self.brain_work()
            .iter()
            .any(|work| work.contains("embedding"))
            || self.brain_counters().embedder_loaded
    }

    /// Removes what `action` names, after checking it again (`context` as it is now). What it
    /// gave back.
    pub async fn clean_storage(
        &self,
        action: Action,
        context: ScanContext,
    ) -> std::result::Result<Cleaned, String> {
        let instance = self.runtime.platform().paths().instance.clone();
        let platform = self.runtime.platform().clone();
        match action {
            Action::Delete(entries) => {
                blocking(move || {
                    let mut cleaned = Cleaned::default();
                    for (bound, bytes) in &entries {
                        if let Err(why) = unused(&*platform, bound.path()) {
                            cleaned.failures.push(why);
                            continue;
                        }
                        match removal::delete(bound) {
                            Ok(()) => cleaned.reclaimed += bytes,
                            Err(err) => cleaned.failures.push(err.to_string()),
                        }
                    }
                    Ok(cleaned)
                })
                .await
            }
            Action::RemoveEmptyDir(bound) => {
                blocking(move || {
                    removal::remove_empty_dir(&bound)
                        .map_err(|err| Error::Invalid(err.to_string()))?;
                    Ok(Cleaned::default())
                })
                .await
            }
            Action::Trash(entries) => {
                blocking(move || {
                    let mut cleaned = Cleaned::default();
                    for (bound, bytes) in &entries {
                        if let Err(why) = unused(&*platform, bound.path()) {
                            cleaned.failures.push(why);
                            continue;
                        }
                        match removal::trash(bound, &instance) {
                            Ok(()) => cleaned.trashed += bytes,
                            Err(err) => cleaned.failures.push(err.to_string()),
                        }
                    }
                    Ok(cleaned)
                })
                .await
            }
            Action::RemoveWorktree {
                repo,
                worktree,
                bytes,
            } => {
                if self.holds_live(&worktree.path().to_string_lossy()) {
                    return Err("a conversation or worker uses it now".into());
                }
                unused(&*platform, worktree.path())?;
                let git = self.git.clone();
                blocking(move || {
                    removal::recheck(&worktree).map_err(|err| Error::Invalid(err.to_string()))?;
                    keep_changes(&git, worktree.path())?;
                    git.open(&repo)
                        .map_err(git_error)?
                        .remove_worktree(worktree.path(), false)
                        .map_err(git_error)?;
                    if removal::is_gone(worktree.path()) {
                        Ok(Cleaned {
                            reclaimed: bytes,
                            ..Cleaned::default()
                        })
                    } else {
                        Err(Error::Invalid(format!(
                            "{} is still there",
                            worktree.path().display()
                        )))
                    }
                })
                .await
            }
            Action::PruneWorktrees { repo } => {
                let git = self.git.clone();
                let ours = self.data_dir.join("worktrees");
                blocking(move || {
                    let repo = git.open(&repo).map_err(git_error)?;
                    let stale: Vec<PathBuf> = repo
                        .worktrees()
                        .map_err(git_error)?
                        .into_iter()
                        .filter(|worktree| worktree.prunable)
                        .map(|worktree| worktree.path)
                        .collect();
                    if stale.iter().any(|path| !path.starts_with(&ours)) {
                        return Err(Error::Invalid(
                            "the repository also has stale worktree records that aren't \
                             Brigadier's; prune them yourself with `git worktree prune`"
                                .into(),
                        ));
                    }
                    repo.prune_worktrees().map_err(git_error)?;
                    Ok(Cleaned::default())
                })
                .await
            }
            Action::DeleteBranch {
                repo,
                branch,
                merged,
            } => {
                let git = self.git.clone();
                blocking(move || {
                    delete_branch_checked(&git, &repo, &branch, merged)?;
                    Ok(Cleaned::default())
                })
                .await
            }
            Action::Dispose { owner, bytes } => {
                let records = self
                    .storage_records()
                    .await
                    .map_err(|err| err.to_string())?;
                if self.owner_states(&records).get(&owner) == Some(&OwnerState::Live) {
                    return Err("it is in use again".into());
                }
                let worktrees: Vec<PathBuf> = self
                    .runtime
                    .ledger()
                    .artifacts(&owner)
                    .into_iter()
                    .filter_map(|artifact| match artifact {
                        Artifact::Worktree { path, .. } => Some(PathBuf::from(path)),
                        _ => None,
                    })
                    .collect();
                let git = self.git.clone();
                let checked = worktrees.clone();
                blocking(move || {
                    for path in &checked {
                        if path.exists() {
                            keep_changes(&git, path)?;
                        }
                    }
                    Ok(())
                })
                .await
                .map_err(|err| err.to_string())?;
                let leftovers = self.runtime.ledger().dispose(&owner).await;
                if leftovers.is_clean() {
                    // A worktree another owner still uses stays: its space isn't given back.
                    let kept = blocking(move || {
                        Ok(worktrees
                            .iter()
                            .filter(|path| path.exists())
                            .map(|path| removal::allocated_size(path))
                            .sum::<u64>())
                    })
                    .await
                    .unwrap_or_default();
                    Ok(Cleaned {
                        reclaimed: bytes.saturating_sub(kept),
                        ..Cleaned::default()
                    })
                } else {
                    Err(Error::Invalid(leftovers.failures.join("; ")))
                }
            }
            Action::DeleteModel {
                speech,
                folder,
                bytes,
            } => {
                let busy = if speech {
                    context.speech_busy
                } else {
                    self.embeddings_busy()
                };
                if busy {
                    return Err("the model is in use now".into());
                }
                blocking(move || {
                    removal::delete(&folder).map_err(|err| Error::Invalid(err.to_string()))?;
                    Ok(Cleaned {
                        reclaimed: bytes,
                        ..Cleaned::default()
                    })
                })
                .await
            }
            Action::CollectBlobs => self
                .core
                .store()
                .gc_blobs()
                .await
                .map(|stats| Cleaned {
                    reclaimed: stats.removed_bytes,
                    ..Cleaned::default()
                })
                .map_err(Error::from),
            Action::Compact => self.compact_database().await,
            Action::External(kind) => Err(Error::Invalid(format!("{kind} is the daemon's"))),
        }
        .map_err(|err| err.to_string())
    }

    async fn compact_database(&self) -> Result<Cleaned> {
        if self.busy_for_maintenance().await.is_some() {
            return Err(Error::Invalid(
                "Brigadier is working; compact the database once nothing runs".into(),
            ));
        }
        let before = self.database_bytes();
        self.core.store().compact().await?;
        Ok(Cleaned {
            reclaimed: before.saturating_sub(self.database_bytes()),
            ..Cleaned::default()
        })
    }

    /// Compacts the database on its own when that gives back enough
    /// ([`SessionManager::worth_compacting`]) and Brigadier has been quiet since
    /// `quiet_generation` ([`SessionManager::maintenance_generation`]). It gives way to any
    /// write meanwhile. Logs what it gave back; shows nothing.
    pub async fn compact_when_quiet(&self, quiet_generation: u64) {
        let Some(space) = self.worth_compacting().await else {
            return;
        };
        let live = space.file_bytes.saturating_sub(space.free_bytes);
        if live > AUTO_COMPACT_MAX_LIVE_BYTES {
            if self.first_too_large() {
                tracing::info!(
                    live,
                    free = space.free_bytes,
                    "the database is too large to compact on its own without holding up work; \
                     Settings › Storage compacts it"
                );
            }
            return;
        }
        let store = self.core.store();
        let Ok(armed) = store.arm_compaction() else {
            return;
        };
        // Armed first: work that starts from now on writes, and so stops it.
        if let Some(busy) = self.busy_for_maintenance().await {
            tracing::debug!(busy, "compacting the database waits: something works");
            return;
        }
        if self.maintenance_generation() != quiet_generation {
            tracing::debug!("compacting the database waits: something ran since the last look");
            return;
        }
        let before = self.database_bytes();
        let started = std::time::Instant::now();
        let outcome = armed.run().await;
        let ms = started.elapsed().as_millis() as u64;
        match outcome {
            Ok(Compacted::Done) => {
                let after = self.database_bytes();
                tracing::info!(
                    before,
                    after,
                    given_back = before.saturating_sub(after),
                    free_pages_bytes = space.free_bytes,
                    ms,
                    "compacted the database on its own"
                );
            }
            Ok(Compacted::GaveWay) => {
                tracing::info!(
                    ms,
                    "compacting the database gave way to new work; it tries again once quiet"
                );
            }
            Err(brigadier_store::Error::ShuttingDown) => {
                tracing::info!(ms, "compacting the database stopped for the quit");
            }
            Err(err) => tracing::warn!(error = %err, "could not compact the database"),
        }
    }

    /// The database's files, with its WAL.
    fn database_bytes(&self) -> u64 {
        let db = self.data_dir.join("brigadier.db");
        ["", "-wal", "-shm"]
            .iter()
            .map(|suffix| {
                let mut path = db.clone().into_os_string();
                path.push(suffix);
                std::fs::metadata(path).map_or(0, |meta| meta.len())
            })
            .sum()
    }

    /// Whether a live owner of the cleanup ledger holds the worktree at `path`.
    fn holds_live(&self, path: &str) -> bool {
        let catalog = self.core.catalog();
        self.runtime
            .ledger()
            .owners()
            .into_iter()
            .filter(|(_, _, disposing)| !disposing)
            .any(|(owner, artifacts, _)| {
                let conversation_gone = match owner.split_once(':') {
                    Some(("orch" | "chat" | "session", id)) => {
                        !catalog.conversations.iter().any(|c| c.id.0 == id)
                    }
                    _ => false,
                };
                !conversation_gone
                    && artifacts.iter().any(|artifact| {
                        matches!(artifact, Artifact::Worktree { path: held, .. } if held == path)
                    })
            })
    }
}

/// Disk use, as a scan measured it.
pub struct ScanUsage {
    pub total_bytes: u64,
    pub projects: Vec<ProjectUsage>,
    pub shared: Vec<SharedUsage>,
}

/// Keeps a worktree's uncommitted changes as a WIP commit on its branch. Refuses when they
/// can't be kept (no branch), so they are never lost with the worktree.
pub(super) fn keep_changes(git: &brigadier_git::Git, path: &Path) -> Result<()> {
    let worktree = git.open_worktree(path).map_err(git_error)?;
    match worktree
        .commit_wip("WIP: uncommitted changes kept by Brigadier before removing its worktree")
    {
        Ok(Some(commit)) => {
            tracing::info!(worktree = %path.display(), commit = %commit.0, "kept a worktree's changes as a WIP commit");
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(err) => Err(Error::Invalid(format!(
            "its uncommitted changes could not be kept ({err}); it stays"
        ))),
    }
}

/// Whether Brigadier names branches like this: a session's or worker's `brigadier/…`, or an
/// overnight run's `overnight/…`.
pub(super) fn brigadier_branch(name: &str) -> bool {
    name.starts_with("brigadier/") || name.starts_with("overnight/")
}

/// Deletes a Brigadier branch at the tip it was listed with, if it is still Brigadier's to
/// delete: its name, its tip, not checked out, and still merged if it was listed as merged.
pub(super) fn delete_branch_checked(
    git: &brigadier_git::Git,
    repo: &Path,
    branch: &KeptBranch,
    merged: bool,
) -> Result<()> {
    if !brigadier_branch(&branch.name) {
        return Err(Error::Invalid(format!(
            "{} is not one of Brigadier's branches",
            branch.name
        )));
    }
    let repo = git.open(repo).map_err(git_error)?;
    let tip = repo.branch_tip(&branch.name).map_err(git_error)?;
    let Some(tip) = tip else {
        return Ok(());
    };
    if tip.0 != branch.tip {
        return Err(Error::Invalid(format!(
            "{} changed since it was listed",
            branch.name
        )));
    }
    if merged
        && !repo
            .is_merged(&branch.name, &branch.target)
            .map_err(git_error)?
    {
        return Err(Error::Invalid(format!(
            "{} is no longer merged into {}",
            branch.name, branch.target
        )));
    }
    repo.delete_branch_at(&branch.name, &tip).map_err(git_error)
}

/// How a Brigadier branch stands: `None` when it isn't there or moved on from `recorded`.
/// Worktrees in `leaving` are about to go, so a branch checked out only there is free.
pub(super) fn branch_standing(
    repo: &brigadier_git::Repo,
    name: &str,
    target: &str,
    recorded: Option<&str>,
    leaving: &[PathBuf],
) -> Option<crate::storage::RemovalBranch> {
    if !brigadier_branch(name) {
        return None;
    }
    let tip = repo.branch_tip(name).ok()??;
    if recorded.is_some_and(|recorded| recorded != tip.0) {
        return None;
    }
    let checked_out = repo.worktrees().ok()?.iter().any(|worktree| {
        worktree.branch.as_deref() == Some(name) && !same_path_in(&worktree.path, leaving)
    });
    // A worker's branch targets its session's branch; once that is gone, its work would land
    // on the repository's default branch.
    let (target, target_tip) = match repo.branch_tip(target).ok().flatten() {
        Some(tip) => (target.to_owned(), Some(tip)),
        None => match repo.default_branch().ok().flatten() {
            Some(fallback) => {
                let tip = repo.branch_tip(&fallback).ok().flatten();
                (fallback, tip)
            }
            None => (target.to_owned(), None),
        },
    };
    let (merged, ahead) = match &target_tip {
        Some(target_tip) => (
            repo.is_merged(name, &target).unwrap_or(false),
            repo.count_commits(target_tip, &tip).unwrap_or(0),
        ),
        None => (false, 0),
    };
    Some(crate::storage::RemovalBranch {
        repo: repo.root().display().to_string(),
        name: name.to_owned(),
        target,
        tip: tip.0,
        merged,
        ahead,
        checked_out,
        command: format!(
            "git -C {} branch -D {name}",
            shell_quote(&repo.root().display().to_string())
        ),
    })
}

/// Whether `path` is one of `paths`, also through a symbolic link above it (`/tmp` and
/// `/private/tmp` are the same place).
pub(super) fn same_path_in(path: &Path, paths: &[PathBuf]) -> bool {
    let real = path.canonicalize().ok();
    paths
        .iter()
        .any(|other| other == path || (real.is_some() && other.canonicalize().ok() == real))
}

pub(super) fn shell_quote(text: &str) -> String {
    if text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c))
    {
        text.to_owned()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

/// The streams a project's conversations and their tasks keep.
fn project_streams(records: &Records, project: &Project) -> Vec<String> {
    let mut list = Vec::new();
    for conversation in records
        .conversations
        .iter()
        .filter(|c| c.project_id.as_ref() == Some(&project.id))
    {
        list.push(streams::conversation(&conversation.id));
        list.push(streams::orchestrator(&conversation.id));
        list.push(streams::draft(&conversation.id.to_string()));
        for task in records.tasks.get(&conversation.id).into_iter().flatten() {
            list.push(streams::task(&task.id));
        }
    }
    list
}

/// Where the worktrees among `artifacts` are, through symbolic links.
fn worktree_places(artifacts: &[Artifact]) -> impl Iterator<Item = PathBuf> + '_ {
    artifacts.iter().filter_map(|artifact| match artifact {
        Artifact::Worktree { path, .. } => {
            Some(std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path)))
        }
        _ => None,
    })
}

/// "1 old log file", "3 old log files".
pub fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn item(
    category: CleanCategory,
    label: String,
    path: Option<PathBuf>,
    bytes: u64,
    reason: &str,
    checked: bool,
) -> CleanItem {
    CleanItem {
        id: String::new(),
        category,
        label,
        path: path.map(|path| path.display().to_string()),
        bytes,
        reason: reason.to_owned(),
        checked,
        selectable: true,
        to_trash: false,
        badges: Vec::new(),
    }
}

/// When anything in `path` last changed (the newest modification time in it, links not
/// followed).
fn last_change(path: &Path) -> Option<SystemTime> {
    let mut newest: Option<SystemTime> = None;
    let mut stack = vec![path.to_owned()];
    while let Some(next) = stack.pop() {
        let Ok(meta) = std::fs::symlink_metadata(&next) else {
            continue;
        };
        if let Ok(modified) = meta.modified() {
            newest = Some(newest.map_or(modified, |newest| newest.max(modified)));
        }
        if meta.is_dir()
            && let Ok(entries) = std::fs::read_dir(&next)
        {
            stack.extend(entries.flatten().map(|entry| entry.path()));
        }
    }
    newest
}

/// Nothing runs inside `path` now (a file never holds a process). Why not otherwise.
fn unused(
    platform: &dyn brigadier_sandbox::Platform,
    path: &Path,
) -> std::result::Result<(), String> {
    if !path.is_dir() {
        return Ok(());
    }
    match platform.processes().in_dir(path) {
        Ok(pids) if pids.is_empty() => Ok(()),
        Ok(_) => Err(format!("{}: something runs in it now", path.display())),
        Err(err) => Err(format!(
            "{}: couldn't check that nothing runs in it ({err})",
            path.display()
        )),
    }
}

fn older_than(path: &Path, age: Duration) -> bool {
    last_change(path)
        .and_then(|changed| changed.elapsed().ok())
        .is_some_and(|elapsed| elapsed >= age)
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    dirs.sort();
    dirs
}

struct Scanner<'a> {
    records: &'a Records,
    states: &'a HashMap<String, OwnerState>,
    git: &'a brigadier_git::Git,
    platform: &'a dyn brigadier_sandbox::Platform,
    items: Vec<ScanItem>,
}

impl Scanner<'_> {
    fn data(&self, part: &str) -> PathBuf {
        self.records.data_dir.join(part)
    }

    /// Paths the ledger holds, by the state of their owners.
    fn held(&self, wanted: impl Fn(OwnerState) -> bool) -> HashSet<String> {
        self.records
            .owners
            .iter()
            .filter(|(owner, _, _)| self.states.get(owner).is_some_and(|state| wanted(*state)))
            .flat_map(|(_, artifacts, _)| artifacts.iter().filter_map(artifact_path))
            .collect()
    }

    fn bind_data(&self, path: &Path) -> Option<Bound> {
        match removal::bind(&self.records.data_dir, path) {
            Ok(bound) => Some(bound),
            Err(err) => {
                tracing::debug!(path = %path.display(), error = %err, "left out of the scan");
                None
            }
        }
    }

    fn busy(&self, dir: &Path) -> bool {
        self.platform
            .processes()
            .in_dir(dir)
            .map_or(true, |found| !found.is_empty())
    }

    fn push(&mut self, item: CleanItem, action: Action) {
        self.items.push(ScanItem {
            item,
            action,
            routine: false,
        });
    }

    /// An item housekeeping may remove on its own too.
    fn push_routine(&mut self, item: CleanItem, action: Action) {
        self.items.push(ScanItem {
            item,
            action,
            routine: true,
        });
    }

    /// Worktree folders in the data directory that nothing live uses.
    fn worktrees(&mut self) {
        let held = self.held(|_| true);
        for project_dir in subdirs(&self.data("worktrees")) {
            let worktrees = subdirs(&project_dir);
            if worktrees.is_empty() {
                let empty = std::fs::read_dir(&project_dir).is_ok_and(|mut e| e.next().is_none());
                if empty && let Some(bound) = self.bind_data(&project_dir) {
                    self.push_routine(
                        item(
                            CleanCategory::Worktrees,
                            "Empty worktree folder".into(),
                            Some(project_dir.clone()),
                            0,
                            "A project's worktree folder with nothing left in it.",
                            true,
                        ),
                        Action::RemoveEmptyDir(bound),
                    );
                }
                continue;
            }
            for path in worktrees {
                let key = path.display().to_string();
                // The ledger's own items (live ones are never offered) cover what it holds.
                if held.contains(&key) {
                    continue;
                }
                let Some((label, repo)) = self.recorded_worktree(&path) else {
                    let mut report = item(
                        CleanCategory::Worktrees,
                        "Worktree folder with no record".into(),
                        Some(path.clone()),
                        removal::allocated_size(&path),
                        "Nothing Brigadier recorded names this folder, so it is left alone.",
                        false,
                    );
                    report.selectable = false;
                    self.items.push(ScanItem {
                        item: report,
                        action: Action::External("none".into()),
                        routine: false,
                    });
                    continue;
                };
                let Some(bound) = self.bind_data(&path) else {
                    continue;
                };
                let bytes = removal::allocated_size(&path);
                if !repo.exists() {
                    let mut entry = item(
                        CleanCategory::Worktrees,
                        label,
                        Some(path.clone()),
                        bytes,
                        "Its repository is gone, so git can't remove it; the folder goes to the \
                         Trash.",
                        false,
                    );
                    entry.to_trash = true;
                    self.push(entry, Action::Trash(vec![(bound, bytes)]));
                    continue;
                }
                let dirty = self
                    .git
                    .open(&path)
                    .and_then(|worktree| worktree.state())
                    .map_or(true, |state| !state.dirty_files.is_empty());
                let mut entry = item(
                    CleanCategory::Worktrees,
                    label,
                    Some(path.clone()),
                    bytes,
                    if dirty {
                        "Its conversation or task ended. Its uncommitted changes are kept as a \
                         WIP commit on its branch before git removes it."
                    } else {
                        "Its conversation or task ended; git removes it."
                    },
                    !dirty,
                );
                if dirty {
                    entry.badges.push(CleanBadge::HasChanges);
                }
                self.push(
                    entry,
                    Action::RemoveWorktree {
                        repo,
                        worktree: bound,
                        bytes,
                    },
                );
            }
        }
    }

    /// The task or session that recorded this worktree, and its repository.
    fn recorded_worktree(&self, path: &Path) -> Option<(String, PathBuf)> {
        let key = path.display().to_string();
        for conversation in &self.records.conversations {
            let repo = Records::repo_of(conversation);
            if let Some(Setup::Session {
                environment:
                    Environment::NewWorktree {
                        path: Some(worktree),
                        ..
                    },
                ..
            }) = &conversation.setup
                && *worktree == key
                && conversation.lifecycle == Lifecycle::Archived
            {
                return Some((
                    format!("Session worktree of “{}”", conversation.title),
                    repo?,
                ));
            }
            for task in self
                .records
                .tasks
                .get(&conversation.id)
                .into_iter()
                .flatten()
            {
                let recorded = task
                    .workspace
                    .as_ref()
                    .and_then(|workspace| workspace.worktree.as_deref());
                if recorded == Some(key.as_str())
                    && (task.state.is_final() || conversation.lifecycle == Lifecycle::Archived)
                {
                    return Some((
                        format!(
                            "Worktree of task-{} “{}” in “{}”",
                            task.number, task.title, conversation.title
                        ),
                        repo?,
                    ));
                }
            }
        }
        None
    }

    /// Git's records of worktrees in the data directory whose folders are gone.
    fn stale_worktree_records(&mut self) {
        let ours = self.data("worktrees");
        let mut repos: Vec<PathBuf> = self
            .records
            .projects
            .iter()
            .flat_map(|project| project.repos.iter().map(|repo| PathBuf::from(&repo.path)))
            .chain(
                self.records
                    .conversations
                    .iter()
                    .filter_map(Records::repo_of),
            )
            .collect();
        repos.sort();
        repos.dedup();
        for repo_path in repos {
            let Ok(repo) = self.git.open(&repo_path) else {
                continue;
            };
            let Ok(worktrees) = repo.worktrees() else {
                continue;
            };
            let stale: Vec<&brigadier_git::WorktreeInfo> =
                worktrees.iter().filter(|w| w.prunable).collect();
            let mine = stale.iter().filter(|w| w.path.starts_with(&ours)).count();
            if mine == 0 {
                continue;
            }
            let all_mine = mine == stale.len();
            let mut entry = item(
                CleanCategory::Worktrees,
                format!(
                    "Records of {mine} removed worktrees in {}",
                    repo_path.display()
                ),
                Some(repo_path.clone()),
                0,
                if all_mine {
                    "Git still lists worktrees whose folders are gone; `git worktree prune` \
                     forgets them."
                } else {
                    "Git still lists worktrees whose folders are gone, but some of those records \
                     aren't Brigadier's, so it leaves `git worktree prune` to you."
                },
                all_mine,
            );
            entry.selectable = all_mine;
            self.push(entry, Action::PruneWorktrees { repo: repo_path });
        }
    }

    /// Branches of archived sessions, and branches deleted conversations left behind.
    fn branches(&mut self) {
        let mut candidates: Vec<(PathBuf, String, String, Option<String>, &'static str)> =
            Vec::new();
        for conversation in &self.records.conversations {
            if conversation.lifecycle != Lifecycle::Archived {
                continue;
            }
            let Some(Setup::Session {
                repo, environment, ..
            }) = &conversation.setup
            else {
                continue;
            };
            let (session_target, session_branch) = match environment {
                Environment::NewWorktree { base, branch, .. } => (base.clone(), Some(branch)),
                Environment::LocalCheckout { branch } => (branch.clone(), None),
            };
            if let Some(branch) = session_branch {
                candidates.push((
                    PathBuf::from(repo),
                    branch.clone(),
                    session_target.clone(),
                    None,
                    "Its session is archived.",
                ));
            }
            for task in self
                .records
                .tasks
                .get(&conversation.id)
                .into_iter()
                .flatten()
            {
                let Some(workspace) = &task.workspace else {
                    continue;
                };
                let Some(branch) = &workspace.branch else {
                    continue;
                };
                let target = workspace
                    .target
                    .clone()
                    .unwrap_or_else(|| session_target.clone());
                candidates.push((
                    PathBuf::from(repo),
                    branch.clone(),
                    target,
                    None,
                    "Its session is archived.",
                ));
            }
            for run in self
                .records
                .runs
                .get(&conversation.id)
                .into_iter()
                .flatten()
            {
                if let Some(workspace) = &run.workspace {
                    candidates.push((
                        PathBuf::from(repo),
                        workspace.branch.clone(),
                        workspace.base.clone(),
                        None,
                        "Its session is archived.",
                    ));
                }
            }
        }
        for (repo, kept) in &self.records.kept {
            candidates.push((
                PathBuf::from(repo),
                kept.name.clone(),
                kept.target.clone(),
                Some(kept.tip.clone()),
                "Its conversation was deleted and the branch kept.",
            ));
        }
        let mut seen = HashSet::new();
        for (repo_path, name, target, recorded, why) in candidates {
            if !seen.insert((repo_path.clone(), name.clone())) {
                continue;
            }
            let Ok(repo) = self.git.open(&repo_path) else {
                continue;
            };
            let Some(standing) = branch_standing(&repo, &name, &target, recorded.as_deref(), &[])
            else {
                continue;
            };
            if standing.checked_out {
                continue;
            }
            let target = &standing.target;
            let reason = if standing.merged {
                format!("{why} Everything on it is in {target}.")
            } else {
                format!(
                    "{why} It has {} {target} doesn't have.",
                    counted(standing.ahead as usize, "commit", "commits")
                )
            };
            let mut entry = item(
                CleanCategory::Branches,
                format!("{name} in {}", repo_path.display()),
                Some(repo_path.clone()),
                0,
                &reason,
                standing.merged,
            );
            if !standing.merged {
                entry.badges.push(CleanBadge::NotMerged {
                    ahead: standing.ahead,
                });
            }
            self.push(
                entry,
                Action::DeleteBranch {
                    repo: repo_path,
                    branch: KeptBranch {
                        name,
                        target: standing.target,
                        tip: standing.tip,
                    },
                    merged: standing.merged,
                },
            );
        }
    }

    /// What the cleanup ledger still holds for owners that are gone or whose cleanup stopped.
    fn ledger_leftovers(&mut self) {
        for (owner, artifacts, _) in &self.records.owners {
            let state = self.states.get(owner).copied().unwrap_or(OwnerState::Live);
            if state == OwnerState::Live {
                continue;
            }
            let paths: Vec<PathBuf> = artifacts
                .iter()
                .filter_map(artifact_path)
                .map(PathBuf::from)
                .collect();
            let bytes: u64 = paths.iter().map(|path| removal::allocated_size(path)).sum();
            let files = artifacts
                .iter()
                .filter(|artifact| {
                    !matches!(
                        artifact,
                        Artifact::Process { .. } | Artifact::ProcessesIn { .. }
                    )
                })
                .count();
            let dirty = artifacts.iter().any(|artifact| match artifact {
                Artifact::Worktree { path, .. } => {
                    Path::new(path).exists()
                        && self
                            .git
                            .open(Path::new(path))
                            .and_then(|worktree| worktree.state())
                            .map_or(true, |state| !state.dirty_files.is_empty())
                }
                _ => false,
            });
            let what = match owner.split_once(':') {
                Some(("orch" | "session", _)) => "a session",
                Some(("chat", _)) => "a Chat",
                Some(("task", _)) => "a worker",
                Some(("brain", _)) => "a Brain job",
                Some(("gen", _)) => "a commit message writer",
                Some(("overnight", _)) => "an overnight run",
                _ => "an Inspector session",
            };
            let (label, reason) = match state {
                OwnerState::Unfinished => (
                    format!("Cleanup of {what} that didn't finish ({files} items)"),
                    "Removing these failed before; Brigadier tries again.",
                ),
                _ => (
                    format!("Files left by {what} that is gone ({files} items)"),
                    "Its worktrees, working folders and CLI session files, as Brigadier \
                     recorded them.",
                ),
            };
            let mut entry = item(
                CleanCategory::SessionFiles,
                label,
                paths.first().cloned(),
                bytes,
                reason,
                !dirty,
            );
            if dirty {
                entry.badges.push(CleanBadge::HasChanges);
            }
            self.push(
                entry,
                Action::Dispose {
                    owner: owner.clone(),
                    bytes,
                },
            );
        }
    }

    /// Conversations' and workers' working folders in the data directory with no owner left.
    fn working_folders(&mut self) {
        let held = self.held(|_| true);
        let live_tasks: HashSet<&str> = self
            .records
            .tasks
            .values()
            .flatten()
            .filter(|task| !task.state.is_final())
            .map(|task| task.id.0.as_str())
            .collect();
        let mut found = Vec::new();
        for area in ["orch", "chat"] {
            for path in subdirs(&self.data(area)) {
                let id = path.file_name().map(|n| n.to_string_lossy().into_owned());
                let live = id
                    .as_deref()
                    .and_then(|id| self.records.conversation(id))
                    .is_some_and(|conversation| conversation.lifecycle != Lifecycle::Archived);
                if !live {
                    found.push(path);
                }
            }
        }
        for path in subdirs(&self.data("scratch")) {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if !live_tasks.contains(name.as_str()) {
                found.push(path);
            }
        }
        let mut entries = Vec::new();
        for path in found {
            let key = path.display().to_string();
            if held.contains(&key)
                || held.iter().any(|held| Path::new(held).starts_with(&path))
                || !older_than(&path, SCRATCH_MIN_AGE)
                || self.busy(&path)
            {
                continue;
            }
            if let Some(bound) = self.bind_data(&path) {
                let bytes = removal::allocated_size(&path);
                entries.push((bound, bytes));
            }
        }
        if entries.is_empty() {
            return;
        }
        let bytes = entries.iter().map(|(_, bytes)| bytes).sum();
        self.push(
            item(
                CleanCategory::SessionFiles,
                counted(
                    entries.len(),
                    "working folder of an ended conversation or worker",
                    "working folders of ended conversations and workers",
                ),
                Some(self.data("scratch")),
                bytes,
                "Nothing live uses them and nothing Brigadier recorded still claims them.",
                true,
            ),
            Action::Delete(entries),
        );
    }

    /// Claude sessions' own temp folders under /tmp that this data directory made.
    #[cfg(unix)]
    fn session_temp_folders(&mut self) {
        let instance = self.platform.paths().instance.clone();
        let held = self.held(|_| true);
        let base = Path::new(brigadier_sandbox::footprint::SESSION_TEMP_BASE);
        let mut ours = Vec::new();
        let mut legacy = Vec::new();
        for folder in brigadier_sandbox::footprint::session_temp_folders() {
            if held.contains(&folder.path.display().to_string()) {
                continue;
            }
            if folder.made_by(&instance) {
                if older_than(&folder.path, TEMP_MIN_AGE) && !self.busy(&folder.path) {
                    ours.push(folder.path);
                }
            } else if folder.marker.is_none() {
                legacy.push(folder.path);
            }
        }
        let mut entries = Vec::new();
        for path in ours {
            if let Ok(bound) = removal::bind(base, &path) {
                let bytes = removal::allocated_size(&path);
                entries.push((bound, bytes));
            }
        }
        if !entries.is_empty() {
            let bytes = entries.iter().map(|(_, bytes)| bytes).sum();
            self.push_routine(
                item(
                    CleanCategory::SessionFiles,
                    counted(entries.len(), "session temp folder", "session temp folders"),
                    Some(base.to_owned()),
                    bytes,
                    "Temp folders of Claude sessions this Brigadier started, untouched for a \
                     day, with nothing working in them.",
                    true,
                ),
                Action::Delete(entries),
            );
        }
        if !legacy.is_empty() {
            let bytes = legacy
                .iter()
                .map(|path| removal::allocated_size(path))
                .sum();
            let mut entry = item(
                CleanCategory::SessionFiles,
                counted(
                    legacy.len(),
                    "older session temp folder",
                    "older session temp folders",
                ),
                Some(base.to_owned()),
                bytes,
                "Made by an older Brigadier, which didn't mark which data directory they belong \
                 to, so they are left alone. They are named /tmp/brigadier-… .",
                false,
            );
            entry.selectable = false;
            entry.badges.push(CleanBadge::Legacy);
            self.items.push(ScanItem {
                item: entry,
                action: Action::External("none".into()),
                routine: false,
            });
        }
    }

    #[cfg(not(unix))]
    fn session_temp_folders(&mut self) {}

    /// Brains of projects that are no longer in Brigadier.
    fn brains(&mut self) {
        let known: HashSet<&str> = self
            .records
            .projects
            .iter()
            .map(|project| project.id.0.as_str())
            .collect();
        for path in subdirs(&self.data("brains")) {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if known.contains(name.as_str()) {
                continue;
            }
            let Some(bound) = self.bind_data(&path) else {
                continue;
            };
            let bytes = removal::allocated_size(&path);
            let mut entry = item(
                CleanCategory::Brains,
                "Brain of a removed project".into(),
                Some(path.clone()),
                bytes,
                "What Brigadier learned about a project that is no longer in Brigadier. Adding \
                 it again would learn it again, which costs time and usage.",
                false,
            );
            entry.to_trash = true;
            self.push(entry, Action::Trash(vec![(bound, bytes)]));
        }
    }

    fn logs_and_recordings(&mut self) {
        let old_files = |dir: &Path, age: Duration| -> Vec<PathBuf> {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return Vec::new();
            };
            entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                .map(|entry| entry.path())
                .filter(|path| older_than(path, age))
                .collect()
        };
        let logs: Vec<(Bound, u64)> = old_files(&self.data("logs"), LOG_MAX_AGE)
            .into_iter()
            .filter_map(|path| {
                let bytes = removal::allocated_size(&path);
                Some((self.bind_data(&path)?, bytes))
            })
            .collect();
        if !logs.is_empty() {
            let bytes = logs.iter().map(|(_, bytes)| bytes).sum();
            self.push(
                item(
                    CleanCategory::LogsAndData,
                    counted(logs.len(), "old log file", "old log files"),
                    Some(self.data("logs")),
                    bytes,
                    "Logs older than the week Brigadier keeps.",
                    true,
                ),
                Action::Delete(logs),
            );
        }
        let recordings: Vec<(Bound, u64)> = old_files(&self.data("recordings"), RECORDING_MAX_AGE)
            .into_iter()
            .filter_map(|path| {
                let bytes = removal::allocated_size(&path);
                Some((self.bind_data(&path)?, bytes))
            })
            .collect();
        if !recordings.is_empty() {
            let bytes = recordings.iter().map(|(_, bytes)| bytes).sum();
            let mut entry = item(
                CleanCategory::LogsAndData,
                format!(
                    "{} older than 30 days",
                    counted(recordings.len(), "recording", "recordings")
                ),
                Some(self.data("recordings")),
                bytes,
                "Sessions recorded from the Inspector for replay.",
                false,
            );
            entry.to_trash = true;
            self.push(entry, Action::Trash(recordings));
        }
    }

    fn models(&mut self, embeddings_busy: bool, speech_busy: bool) {
        for (folder, label, busy) in [
            ("embeddings", "Brain embedding model", embeddings_busy),
            ("whisper", "Dictation speech model", speech_busy),
        ] {
            let path = self.data("models").join(folder);
            if !path.exists() {
                continue;
            }
            let bytes = removal::allocated_size(&path);
            let Some(bound) = self.bind_data(&path) else {
                continue;
            };
            let mut entry = item(
                CleanCategory::Models,
                label.into(),
                Some(path.clone()),
                bytes,
                if busy {
                    "In use now, so it stays."
                } else {
                    "Re-downloads when needed."
                },
                false,
            );
            entry.selectable = !busy;
            self.push(
                entry,
                Action::DeleteModel {
                    speech: folder == "whisper",
                    folder: bound,
                    bytes,
                },
            );
        }
    }

    fn usage(&self, project_blobs: &HashMap<String, Vec<PathBuf>>) -> ScanUsage {
        let size = |path: PathBuf| removal::allocated_size(&path);
        let total_bytes = size(self.records.data_dir.clone());
        let mut projects = Vec::new();
        for project in &self.records.projects {
            let mut scratch = 0;
            for conversation in self
                .records
                .conversations
                .iter()
                .filter(|c| c.project_id.as_ref() == Some(&project.id))
            {
                scratch += size(self.data("orch").join(&conversation.id.0));
                scratch += size(self.data("chat").join(&conversation.id.0));
                for task in self
                    .records
                    .tasks
                    .get(&conversation.id)
                    .into_iter()
                    .flatten()
                {
                    scratch += size(self.data("scratch").join(&task.id.0));
                }
            }
            let blobs_bytes = project_blobs
                .get(&project.id.0)
                .into_iter()
                .flatten()
                .map(|file| std::fs::metadata(file).map_or(0, |meta| meta.len()))
                .sum();
            projects.push(ProjectUsage {
                project_id: project.id.clone(),
                name: project.name.clone(),
                repo_found: project
                    .repos
                    .first()
                    .is_some_and(|repo| Path::new(&repo.path).is_dir()),
                worktrees_bytes: size(self.data("worktrees").join(&project.id.0)),
                brain_bytes: size(self.data("brains").join(&project.id.0)),
                blobs_bytes,
                scratch_bytes: scratch,
            });
        }
        // The event store and routing's history (quota samples, outcomes, research).
        let database = [
            "brigadier.db",
            "brigadier.db-wal",
            "brigadier.db-shm",
            "routing.sqlite",
            "routing.sqlite-wal",
            "routing.sqlite-shm",
        ]
        .iter()
        .map(|name| size(self.data(name)))
        .sum();
        let blobs = size(self.data("blobs"));
        let project_blob_total: u64 = projects.iter().map(|p| p.blobs_bytes).sum();
        let personal = [
            "personal.sqlite",
            "personal.sqlite-wal",
            "personal.sqlite-shm",
        ]
        .iter()
        .map(|name| size(self.data("brains").join(name)))
        .sum();
        let mut shared = vec![
            SharedUsage {
                part: SharedPart::Database,
                bytes: database,
            },
            SharedUsage {
                part: SharedPart::OtherBlobs,
                bytes: blobs.saturating_sub(project_blob_total),
            },
            SharedUsage {
                part: SharedPart::Models,
                bytes: size(self.data("models")),
            },
            SharedUsage {
                part: SharedPart::Logs,
                bytes: size(self.data("logs")),
            },
            SharedUsage {
                part: SharedPart::Recordings,
                bytes: size(self.data("recordings")),
            },
            SharedUsage {
                part: SharedPart::PersonalBrain,
                bytes: personal,
            },
        ];
        let counted: u64 = shared.iter().map(|part| part.bytes).sum::<u64>()
            + projects
                .iter()
                .map(|p| p.worktrees_bytes + p.brain_bytes + p.scratch_bytes)
                .sum::<u64>()
            + project_blob_total.min(blobs);
        shared.push(SharedUsage {
            part: SharedPart::Other,
            bytes: total_bytes.saturating_sub(counted),
        });
        ScanUsage {
            total_bytes,
            projects,
            shared,
        }
    }
}

/// The path an artifact is on disk, if it is one.
fn artifact_path(artifact: &Artifact) -> Option<String> {
    match artifact {
        Artifact::Worktree { path, .. }
        | Artifact::ScratchDir { path }
        | Artifact::ClaudeTempDir { path }
        | Artifact::ClaudeProjectDir { path }
        | Artifact::CodexGeneratedImages { path } => Some(path.clone()),
        _ => None,
    }
}
