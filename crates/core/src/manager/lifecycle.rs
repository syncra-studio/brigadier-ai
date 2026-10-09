//! The lifecycle of sessions and Chats (PLAN §5): hibernate, archive, restore, delete, and
//! the recovery after Brigadier was quit or crashed.
//!
//! - **Hibernate** (automatic when idle, or on request): the conversation's CLI sessions
//!   stop and their temp files go. The CLI session files stay, so the next message resumes
//!   it; if they are gone, it starts over from Brigadier's transcript.
//! - **Archive**: workers stop (unfinished work kept as a WIP commit on its task branch),
//!   every CLI session ends, and everything recorded in the cleanup ledger for the
//!   conversation is removed: worktrees, scratch folders, CLI session files, processes.
//!   Transcript, tasks, artifacts and branches stay; restoring starts a new CLI session from
//!   the transcript.
//! - **Delete**: answered at once like an archive; in the background it is wound down like on
//!   archive, the branches Brigadier created for it go, then its streams and the blobs nothing
//!   else references, and last its catalog entry.
//! - **Recovery**: after a restart, work that was running has lost its CLI: tasks end (their
//!   work kept), cards nobody waits for expire, and the ledger finishes every cleanup.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use brigadier_providers::Artifact;

use super::conversation::Envelope;
use super::prompts;
use super::workers::{route_label, test_data_dir, test_data_root};
use super::{SessionManager, blocking, git_error};
use crate::model::{
    Conversation, ConversationId, ConversationKind, Environment, Lifecycle, Setup, streams,
};
use crate::overnight::OvernightRun;
use crate::work::{InjectionKind, Task, TaskState};
use crate::{Error, Result, now_ms};

/// How often idle conversations are checked for hibernation.
const HIBERNATE_CHECK: Duration = Duration::from_secs(60);

/// How long nothing in a test data folder no task of this data folder claims has changed
/// before the launch sweep removes it. Another data folder's task that sat that long loses
/// only its test data: its worker's next start makes the folder again.
const UNCLAIMED_TEST_DATA_AGE: Duration = Duration::from_secs(24 * 60 * 60);

impl SessionManager {
    /// Lets the cleanup ledger remove the worktrees it records.
    pub(super) fn install_worktree_remover(&self) {
        let git = self.git.clone();
        self.runtime.ledger().set_worktree_remover(Arc::new(
            move |repo: PathBuf, path: PathBuf| {
                let git = git.clone();
                Box::pin(async move {
                    tokio::task::spawn_blocking(move || {
                        match git.open(&repo) {
                            Ok(repo) => {
                                let same = |a: &std::path::Path| {
                                    a == path || a.canonicalize().ok() == path.canonicalize().ok()
                                };
                                let registered = repo
                                    .worktrees()
                                    .map_err(|err| err.to_string())?
                                    .iter()
                                    .any(|w| same(&w.path));
                                if registered || !path.exists() || same(repo.root()) {
                                    repo.remove_worktree(&path, true)
                                        .map_err(|err| err.to_string())
                                } else {
                                    // Git already let go of it (a removal that failed halfway);
                                    // what is left is the folder Brigadier created and recorded.
                                    std::fs::remove_dir_all(&path).map_err(|err| err.to_string())
                                }
                            }
                            // The repository itself is gone: only the folder is left to remove.
                            Err(_) if !path.exists() => Ok(()),
                            Err(_) => std::fs::remove_dir_all(&path).map_err(|err| err.to_string()),
                        }
                    })
                    .await
                    .map_err(|err| err.to_string())?
                })
            },
        ));
    }

    /// After a restart: nothing runs any more, so running tasks end, stale cards expire, and
    /// cleanups that could not finish before (worktrees need the git engine) finish now.
    pub(super) async fn recover(&self) {
        self.runtime.ledger().sweep().await;
        self.reconcile_trust().await;
        for conversation in self.core.catalog().conversations {
            // One still being deleted (a delete that failed at launch) gets nothing new.
            if conversation.deleting {
                continue;
            }
            // Messages that waited for quota keep waiting (an archived conversation's never go).
            if conversation.quota_wait.is_some() && conversation.lifecycle != Lifecycle::Archived {
                self.keep_conversation_wait(&conversation).await;
            }
            let Ok(tasks) = self.core.tasks(&conversation.id).await else {
                continue;
            };
            for task in tasks.into_iter().filter(|task| !task.state.is_final()) {
                match task.state {
                    // A self-check after a rebase, or its report Brigadier was to land on its
                    // own: the restart ended that, so the orchestrator decides.
                    _ if interrupted_fix(&task) => {
                        self.recover_fix(&task).await;
                        continue;
                    }
                    // Its worktree and session stay: the terminal is gone, and the session
                    // goes back to its worker (or the end under way finishes).
                    TaskState::TakenOver => {
                        self.recover_takeover(&task).await;
                        continue;
                    }
                    // A write task that reported changing nothing has nothing to land.
                    TaskState::Reported if self.changed_nothing(&task).await => {
                        self.dispose_task(&task, TaskState::Done).await;
                        continue;
                    }
                    // Its worktree is intact; the worker resumes when sent back to work.
                    TaskState::Reported | TaskState::ReadyToLand if task.kind.writes() => continue,
                    // No CLI ran; it waits for quota as before.
                    TaskState::Paused if task.quota_wait.is_some() => {
                        self.keep_waiting(&task).await;
                        continue;
                    }
                    // A read task that reported is done.
                    TaskState::Reported => {
                        self.dispose_task(&task, TaskState::Done).await;
                        continue;
                    }
                    // The landing was interrupted (older stores: its checks, or its card):
                    // nothing landed, and the orchestrator lands it again.
                    TaskState::Landing if task.kind.writes() => {
                        let _ = self
                            .update_task(&conversation.id, &task.id, |t| {
                                t.state = TaskState::Reported;
                                t.candidate = None;
                                t.landing = None;
                            })
                            .await;
                        // The orchestrator was promised the landing's outcome as a message.
                        self.deliver(
                            &conversation.id,
                            Envelope {
                                kind: InjectionKind::Decision,
                                label: format!("landing task-{}", task.number),
                                task_id: Some(task.id.clone()),
                                text: format!(
                                    "[not landed task-{} \"{}\"] Brigadier restarted before the landing finished; nothing landed. Call land_phase for task-{} again.",
                                    task.number, task.title, task.number,
                                ),
                            },
                        )
                        .await;
                        continue;
                    }
                    _ => {}
                }
                let _ = self
                    .update_task(&conversation.id, &task.id, |t| {
                        t.error = Some("Brigadier quit while this task was running.".into());
                    })
                    .await;
                self.dispose_task(&task, TaskState::Stopped).await;
            }
            self.expire_stale_cards(&conversation.id).await;
            // A one-shot review the restart cut off is over; the orchestrator hears it.
            self.recover_reviews(&conversation.id).await;
            // So are its previews (the sweep above ended any a crash left running).
            self.recover_previews(&conversation.id).await;
            // What waits for the user matches the tasks and reports as they are now.
            self.reconcile_waiting(&conversation.id).await;
            // Nothing runs any more: what was working is over or waits for the user.
            self.settle_requests(&conversation.id).await;
        }
        self.sweep_test_data().await;
    }

    /// Removes the test data folders no task needs any more (PLAN.md §10.13): a task's own
    /// goes when it ends, but a crash, an older build or a deleted data folder leaves them
    /// behind. Several data folders (dev builds, smoke runs) share the temp directory, so a
    /// folder goes only if this data folder's task for it is over, or if no task here claims
    /// it and nothing in it changed for `UNCLAIMED_TEST_DATA_AGE`.
    async fn sweep_test_data(&self) {
        let mut tasks = HashMap::new();
        for conversation in self.core.catalog().conversations {
            let Ok(list) = self.core.tasks(&conversation.id).await else {
                continue;
            };
            for task in list {
                let over = task.state.is_final();
                // Two tasks whose ids end alike share a folder: it stays while either runs.
                tasks
                    .entry(test_data_dir(&task.id))
                    .and_modify(|both: &mut bool| *both &= over)
                    .or_insert(over);
            }
        }
        let claimed: HashSet<PathBuf> = self
            .runtime
            .ledger()
            .owners()
            .into_iter()
            .filter(|(_, _, disposing)| !disposing)
            .flat_map(|(_, artifacts, _)| artifacts)
            .filter_map(|artifact| match artifact {
                Artifact::ScratchDir { path } => Some(PathBuf::from(path)),
                _ => None,
            })
            .collect();
        let cutoff = SystemTime::now() - UNCLAIMED_TEST_DATA_AGE;
        let swept = blocking(move || {
            let folders = test_folders(&test_data_root());
            Ok(sweep_test_folders(folders, &tasks, &claimed, cutoff))
        })
        .await;
        match swept {
            Ok(0) => {}
            Ok(removed) => tracing::info!(removed, "removed test data folders no task needs"),
            Err(err) => tracing::warn!(error = %err, "could not sweep the test data folders"),
        }
    }

    /// A write task whose commits were rebased during its landing, cut off by the restart
    /// while its worker ran the quick self-check (or before its report landed): its worktree
    /// stays and it waits as reported; the orchestrator gets its report, and decides.
    async fn recover_fix(&self, task: &Task) {
        let checking = task.state != TaskState::Reported;
        let Ok(task) = self
            .update_task(&task.conversation_id, &task.id, |t| {
                t.landing = None;
                t.state = TaskState::Reported;
                t.blocked_reason = None;
            })
            .await
        else {
            return;
        };
        let mut text = match &task.report {
            Some(report) => prompts::report_envelope(&task, report, &route_label(&task)),
            None => String::new(),
        };
        let n = task.number;
        text.push_str(&if checking {
            format!(
                "\n[not landed task-{n}] Its commits were rebased onto the session's branch, which had moved, and Brigadier restarted while the worker ran its quick self-check; nothing landed. Send it back to finish the self-check (message_worker), then call land_phase for task-{n}."
            )
        } else {
            format!(
                "\n[not landed task-{n}] This is the report after its self-check; Brigadier restarted before landing it, so nothing landed. Call land_phase for task-{n}."
            )
        });
        self.deliver(
            &task.conversation_id,
            Envelope {
                kind: InjectionKind::Report,
                label: format!("report task-{}", task.number),
                task_id: Some(task.id.clone()),
                text: text.trim_start().to_owned(),
            },
        )
        .await;
    }

    /// Hibernates conversations idle for longer than the setting.
    pub(super) fn start_hibernation_timer(&self) {
        let manager = self.me.clone();
        self.spawn(async move {
            let mut tick = tokio::time::interval(HIBERNATE_CHECK);
            tick.tick().await;
            loop {
                tick.tick().await;
                let Some(manager) = manager.upgrade() else {
                    return;
                };
                if manager.admit().is_err() {
                    return;
                }
                manager.hibernate_idle().await;
                manager.brain_upkeep().await;
            }
        });
    }

    async fn hibernate_idle(&self) {
        let minutes = self.core.settings().hibernate_after_minutes;
        if minutes == 0 {
            return;
        }
        let cutoff = now_ms() - i64::from(minutes) * 60_000;
        let convs: Vec<_> = self.convs_lock().values().cloned().collect();
        for conv in convs {
            let Some(since) = conv.idle_since_ms().await else {
                continue;
            };
            // An overnight run's session stays up while the run lasts (PLAN.md §10.10).
            if since > cutoff
                || conv.is_busy().await
                || self.has_running_workers(&conv.id).await
                || self.overnight.active.get(&conv.id).is_some()
            {
                continue;
            }
            if let Err(err) = self.hibernate(conv.id.clone()).await {
                tracing::debug!(conversation = %conv.id, error = %err, "could not hibernate");
            }
        }
    }

    /// Whether any conversation has a turn in progress, work waiting for one, or a worker
    /// that hasn't finished: what keeping the computer awake "while agents work" means.
    pub async fn agents_working(&self) -> bool {
        if self.overnight_active() {
            return true;
        }
        let convs: Vec<_> = self.convs_lock().values().cloned().collect();
        for conv in convs {
            if conv.is_busy().await || self.has_running_workers(&conv.id).await {
                return true;
            }
        }
        false
    }

    /// Whether an overnight run is under way: it is work (the daemon stays up, the computer
    /// awake) even between its phases, while it waits for quota or writes its report.
    pub fn overnight_active(&self) -> bool {
        !self.overnight.active.all().is_empty()
    }

    pub(super) async fn has_running_workers(&self, id: &ConversationId) -> bool {
        self.core.tasks(id).await.is_ok_and(|tasks| {
            tasks.iter().any(|task| {
                matches!(
                    task.state,
                    TaskState::Queued
                        | TaskState::Starting
                        | TaskState::Running
                        | TaskState::Blocked
                        | TaskState::Landing
                        | TaskState::TakenOver
                )
            })
        })
    }

    /// Stops the conversation's CLI sessions and removes their temp files; it continues on
    /// the next message.
    pub async fn hibernate(&self, id: ConversationId) -> Result<Conversation> {
        let conversation = self.core.conversation(&id)?;
        if conversation.lifecycle == Lifecycle::Archived {
            return Err(Error::Invalid(
                "an archived conversation cannot hibernate".into(),
            ));
        }
        if self.has_running_workers(&id).await {
            return Err(Error::Invalid(
                "workers are still running; stop them or let them finish first".into(),
            ));
        }
        if let Ok(conv) = self.conv(&id) {
            if conv.is_busy().await {
                return Err(Error::Invalid(
                    "a turn is running; interrupt it first".into(),
                ));
            }
            conv.close_cli().await;
        }
        // Reported workers wait idle; they stop too and resume when sent back to work.
        for task in self.core.tasks(&id).await? {
            if !task.state.is_final()
                && let Some(live) = self.existing_task_live(&task.id)
            {
                live.close_cli().await;
                live.allow_revival().await;
            }
        }
        self.drop_prewarm(&id, "the session hibernated");
        let (owner, area) = conversation_owner(&conversation);
        self.runtime.ledger().end_processes(&owner).await;
        let attachments = self.owned_dir(area, &id.0).join("attachments");
        let _ = blocking(move || match std::fs::remove_dir_all(&attachments) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(Error::Invalid(err.to_string())),
        })
        .await;
        self.set_run(&id, crate::work::RunState::Hibernated, None)
            .await;
        self.core.set_lifecycle(id, Lifecycle::Hibernated).await
    }

    /// Archives conversations, each as [`Self::archive`] does: one outcome per id.
    pub async fn archive_all(&self, ids: &[ConversationId]) -> Vec<Result<Conversation>> {
        let mut outcomes = Vec::with_capacity(ids.len());
        for id in ids {
            outcomes.push(self.archive(id.clone()).await);
        }
        outcomes
    }

    /// Archives a conversation: everything it created goes, except its transcript, tasks,
    /// artifacts and branches. Answers once nothing new of it can start and it is stored as
    /// archived; the rest of the cleanup goes on in the background ([`Self::finish_archive`]).
    pub async fn archive(&self, id: ConversationId) -> Result<Conversation> {
        self.admit()?;
        let conversation = self.core.conversation(&id)?;
        if conversation.deleting {
            return Err(being_deleted());
        }
        if conversation.lifecycle == Lifecycle::Archived {
            return Ok(conversation);
        }
        let sides: Vec<ConversationId> = self
            .side_chats(&id)
            .into_iter()
            .map(|side| side.id)
            .collect();
        for fenced in sides.iter().chain([&id]) {
            self.close_fence(fenced);
        }
        let archived = match self.fence_runs(&id).await {
            Ok(runs) => self
                .core
                .mark_archived(id.clone())
                .await
                .map(|archived| (archived, runs)),
            Err(err) => Err(err),
        };
        let (archived, runs) = match archived {
            Ok(archived) => archived,
            Err(err) => {
                for fenced in sides.iter().chain([&id]) {
                    self.open_fence(fenced);
                }
                return Err(err);
            }
        };
        let manager = self.arc();
        self.start_cleanup(id.clone(), async move {
            manager.finish_archive(&id, runs).await;
        });
        Ok(archived)
    }

    /// The cleanup of an archive: once the work that passed the fence has finished, its side
    /// chats go, everything it runs stops, and what it created is removed. The cleanup mark
    /// goes last, so a restart finishes a cleanup cut off at any step.
    pub(super) async fn finish_archive(&self, id: &ConversationId, runs: Vec<OvernightRun>) {
        if !self.drain(id).await {
            tracing::warn!(conversation = %id, "work of an archived session is still going; cleaning up anyway");
        }
        self.delete_side_chats(id).await;
        let Ok(conversation) = self.core.conversation(id) else {
            return;
        };
        self.wind_down(&conversation).await;
        self.close_runs(runs, true).await;
        self.release_conversation_runs(id).await;
        if let Err(err) = self.core.finish_cleanup(id.clone()).await {
            tracing::warn!(conversation = %id, error = %err, "could not record the end of an archive's cleanup; the next launch looks again");
        }
    }

    /// At launch, before any run resumes: finishes the deletes and the archives' cleanups a
    /// quit or crash cut off (their processes are gone; worktrees, session files, runs and
    /// streams are not).
    pub(super) async fn finish_cut_off_cleanups(&self) {
        for conversation in self.core.catalog().conversations {
            let id = conversation.id;
            if conversation.deleting {
                tracing::info!(conversation = %id, "finishing a delete cut off by a quit");
                self.close_fence(&id);
                self.finish_delete(&id).await;
                continue;
            }
            if !conversation.cleanup_pending {
                continue;
            }
            if conversation.lifecycle != Lifecycle::Archived {
                let _ = self.core.finish_cleanup(id).await;
                continue;
            }
            tracing::info!(conversation = %id, "finishing an archive's cleanup cut off by a quit");
            self.close_fence(&id);
            let runs = match self.fence_runs(&id).await {
                Ok(runs) => runs,
                Err(err) => {
                    tracing::warn!(conversation = %id, error = %err, "could not fence the archived session's runs");
                    Vec::new()
                }
            };
            self.finish_archive(&id, runs).await;
        }
    }

    /// Stops everything a conversation runs and removes what it created.
    pub(super) async fn wind_down(&self, conversation: &Conversation) {
        let id = &conversation.id;
        // Its previews run in the workspace that goes next: they stop first.
        self.stop_previews(id, "the session closed").await;
        self.drop_prewarm(id, "the session closed");
        let conv = self.convs_lock().remove(id);
        if let Some(conv) = conv {
            // Messages waiting for quota never go.
            self.drop_waiting(&conv).await;
            conv.close_cli().await;
        }
        // The workers' CLI sessions end side by side; then their worktrees go, one cleanup's
        // git work at a time.
        if let Ok(tasks) = self.core.tasks(id).await {
            let mut closing = tokio::task::JoinSet::new();
            for live in tasks
                .iter()
                .filter(|task| !task.state.is_final())
                .filter_map(|task| self.existing_task_live(&task.id))
            {
                closing.spawn(async move { live.close_cli().await });
            }
            closing.join_all().await;
        }
        let _lane = self.closing.lane.lock().await;
        if let Ok(tasks) = self.core.tasks(id).await {
            for task in tasks.into_iter().filter(|task| !task.state.is_final()) {
                if let Some(live) = self.existing_task_live(&task.id) {
                    live.close_cli().await;
                }
                self.dispose_task(&task, TaskState::Stopped).await;
            }
        }
        let (owner, _) = conversation_owner(conversation);
        self.grants.revoke_owner(&owner);
        let mut owners = vec![owner, super::workers::worker_home_owner(id)];
        // Its previews' log folder.
        let previews = super::preview::preview_owner(id);
        if !self.runtime.ledger().artifacts(&previews).is_empty() {
            owners.push(previews);
        }
        self.stop_reviews(id);
        let session_worktree_goes = self.keep_session_changes(conversation).await;
        if session_worktree_goes {
            owners.push(format!("session:{id}"));
        }
        for owner in owners {
            let leftovers = self.runtime.ledger().dispose(&owner).await;
            if !leftovers.is_clean() {
                tracing::warn!(
                    owner,
                    ?leftovers,
                    "some leftovers will be retried at the next launch"
                );
            }
        }
        // Its CLI session files are gone: the next CLI session starts from the transcript.
        self.forget_native_session(id).await;
        self.expire_stale_cards(id).await;
        self.set_run(id, crate::work::RunState::Idle, None).await;
        if session_worktree_goes {
            self.forget_session_worktree(conversation).await;
        }
    }

    /// After the user merged the session (THREAD-PLAN.md Q9): its worktree and merged branch
    /// go, and the next user message makes a fresh worktree from the base's tip, at the same
    /// path and under the same branch name, so the thread's CLI goes on as it is. Uncommitted
    /// changes in the worktree keep both. Returns what the thread is told about it.
    pub(crate) async fn release_merged_session(&self, id: &ConversationId) -> String {
        let Ok(conversation) = self.core.conversation(id) else {
            return String::new();
        };
        let Some(Setup::Session {
            environment:
                Environment::NewWorktree {
                    base,
                    path: Some(path),
                    ..
                },
            ..
        }) = &conversation.setup
        else {
            return String::new();
        };
        let (git, worktree) = (self.git.clone(), PathBuf::from(path));
        if worktree_locked(&worktree) {
            // Removing it would fail; the session would then make a new one where it still is.
            return " The session's worktree is locked (`git worktree lock`), so it and its branch stay; unlock it, and the next merge removes both.".into();
        }
        let dirty = blocking(move || {
            let repo = git.open(&worktree).map_err(git_error)?;
            Ok(repo.state().map_err(git_error)?.dirty_files)
        })
        .await;
        match dirty {
            Ok(dirty) if dirty.is_empty() => {}
            Ok(dirty) => {
                return format!(
                    " The session's worktree has uncommitted changes ({}), so it and its branch stay; commit or drop them, and the next merge removes both.",
                    dirty.join(", ")
                );
            }
            Err(err) => {
                tracing::warn!(conversation = %id, error = %err, "could not look at the merged session's worktree; it stays");
                return String::new();
            }
        }
        // A pre-warm was made from the branch that just went.
        self.drop_prewarm(id, "the session was merged");
        let owner = format!("session:{id}");
        let leftovers = self.runtime.ledger().dispose(&owner).await;
        let removed = leftovers.is_clean();
        if !removed {
            tracing::warn!(
                owner,
                ?leftovers,
                "the merged session's worktree couldn't be removed; the next message retries"
            );
        }
        let deleted = self.forget_session_worktree(&conversation).await;
        if !removed {
            " Its worktree couldn't be removed yet; the user's next message tries again, then starts a fresh branch from the base in the same folder.".into()
        } else if deleted {
            format!(
                " Its worktree and branch are removed; the user's next message starts a fresh branch from `{base}` in the same folder."
            )
        } else {
            " Its worktree is removed; the user's next message makes it again in the same folder."
                .into()
        }
    }

    /// A new-worktree session's worktree is gone: its branch goes too while the base has all
    /// of its work, and the session records no worktree, so the next one is made from the base
    /// (at the same path, under the same branch name). Returns whether the branch went.
    pub(super) async fn forget_session_worktree(&self, conversation: &Conversation) -> bool {
        let id = &conversation.id;
        let Some(Setup::Session {
            repo,
            environment:
                Environment::NewWorktree {
                    base,
                    branch,
                    path: Some(_),
                    mut start,
                },
            permission,
            orchestrator,
            workers_see_uncommitted,
            plan_mode,
        }) = conversation.setup.clone()
        else {
            return false;
        };
        // The session worktree is gone. The branch stays while it holds work the base does
        // not have; a restored session creates it again from the base otherwise.
        let mut deleted_branch = false;
        if branch.starts_with("brigadier/") {
            let (git, repo, branch, base) = (
                self.git.clone(),
                PathBuf::from(&repo),
                branch.clone(),
                base.clone(),
            );
            let deleted = blocking(move || {
                let repo = git.open(&repo).map_err(git_error)?;
                if let Some(tip) = repo.branch_tip(&branch).map_err(git_error)?
                    && repo.is_merged(&branch, &base).map_err(git_error)?
                {
                    repo.delete_branch_at(&branch, &tip).map_err(git_error)?;
                    return Ok(true);
                }
                Ok(false)
            })
            .await;
            match deleted {
                // Its work is in the base now: a fork's branch starts there again too.
                Ok(true) => {
                    start = None;
                    deleted_branch = true;
                }
                Ok(false) => {}
                Err(err) => {
                    tracing::warn!(conversation = %id, error = %err, "could not delete the merged session branch");
                }
            }
        }
        let _ = self
            .core
            .set_setup(
                id.clone(),
                Setup::Session {
                    repo,
                    environment: Environment::NewWorktree {
                        base,
                        branch,
                        path: None,
                        start,
                    },
                    permission,
                    orchestrator,
                    workers_see_uncommitted,
                    plan_mode,
                },
            )
            .await;
        deleted_branch
    }

    /// Changes made in a new-worktree session's own worktree (by the user; landings there are
    /// commits) are kept as a WIP commit on the session branch before the worktree goes. If
    /// they cannot be, the worktree stays, and so does everything in it (`false`).
    async fn keep_session_changes(&self, conversation: &Conversation) -> bool {
        let Some(Setup::Session {
            environment:
                Environment::NewWorktree {
                    path: Some(path), ..
                },
            ..
        }) = &conversation.setup
        else {
            return true;
        };
        let (git, path) = (self.git.clone(), PathBuf::from(path));
        if !path.exists() {
            return true;
        }
        let kept = blocking(move || {
            git.open_worktree(&path)
                .map_err(git_error)?
                .commit_wip("WIP: uncommitted changes in the session worktree (kept by Brigadier)")
                .map_err(git_error)
        })
        .await;
        match kept {
            Ok(Some(commit)) => {
                tracing::info!(conversation = %conversation.id, commit = %commit.0, "kept the session worktree's changes as a WIP commit");
                true
            }
            Ok(None) => true,
            Err(err) => {
                tracing::error!(conversation = %conversation.id, error = %err, "could not keep the session worktree's changes; the worktree stays");
                false
            }
        }
    }

    /// Brings an archived conversation back; its next turn starts from the transcript.
    pub async fn restore(&self, id: ConversationId) -> Result<Conversation> {
        // What the archive stopped and removed is gone first.
        self.cleanup_finished(&id).await;
        let conversation = self.core.conversation(&id)?;
        if conversation.deleting {
            return Err(being_deleted());
        }
        if conversation.lifecycle != Lifecycle::Archived {
            return Err(Error::Invalid(
                "only an archived conversation can be restored".into(),
            ));
        }
        if let Ok(conv) = self.conv(&id) {
            conv.mark_reseed().await;
        }
        let restored = self
            .core
            .set_lifecycle(id.clone(), Lifecycle::Active)
            .await?;
        self.open_fence(&id);
        Ok(restored)
    }

    /// Deletes conversations for good, each as [`Self::delete`] does: one outcome per id.
    pub async fn delete_all(&self, ids: &[ConversationId]) -> Vec<Result<()>> {
        let mut outcomes = Vec::with_capacity(ids.len());
        for id in ids {
            outcomes.push(self.delete(id.clone()).await);
        }
        outcomes
    }

    /// Deletes a conversation for good: its transcript, everything it created, and the branches
    /// Brigadier created for it (its session, task and overnight-run branches, unmerged ones
    /// too; the user's own branches never). What the Brain learned in it stays; its transcript
    /// index goes. Answers once nothing new of it can start and it is durably marked as being
    /// deleted, which hides it from the app; the rest goes on in the background
    /// ([`Self::finish_delete`]), after an archive's cleanup still under way.
    pub async fn delete(&self, id: ConversationId) -> Result<()> {
        self.admit()?;
        let conversation = self.core.conversation(&id)?;
        let sides: Vec<ConversationId> = self
            .side_chats(&id)
            .into_iter()
            .map(|side| side.id)
            .collect();
        for fenced in sides.iter().chain([&id]) {
            self.close_fence(fenced);
        }
        let marked = match self.fence_runs(&id).await {
            Ok(_) => self.core.mark_deleting(id.clone()).await,
            Err(err) => Err(err),
        };
        if let Err(err) = marked {
            // Still there: an archived one stays closed; one that works goes on working.
            if conversation.lifecycle != Lifecycle::Archived {
                for fenced in sides.iter().chain([&id]) {
                    self.open_fence(fenced);
                }
            }
            return Err(err);
        }
        let manager = self.arc();
        self.start_cleanup(id.clone(), async move {
            manager.finish_delete(&id).await;
        });
        Ok(())
    }

    /// The background part of a delete. The `deleting` mark stays until every step has
    /// succeeded, so a restart tries again where this one failed or was cut off.
    async fn finish_delete(&self, id: &ConversationId) {
        let Ok(conversation) = self.core.conversation(id) else {
            // Gone already (deleted twice, or with its project).
            return;
        };
        match self.delete_closed(conversation, true, false).await {
            Ok(()) => {
                self.open_fence(id);
                self.note_space_freed();
            }
            Err(err) => {
                tracing::warn!(conversation = %id, error = %err, "could not finish deleting a conversation; the next launch tries again")
            }
        }
    }

    /// Deletes a conversation now, after its cleanup under way (a project removal, a side
    /// chat going with its parent). Branches Brigadier created for it go only with
    /// `delete_branches`; `record_kept` records those it leaves (a project removal records them
    /// itself, once the user's choices are through).
    pub(super) async fn delete_conversation(
        &self,
        id: ConversationId,
        delete_branches: bool,
        record_kept: bool,
    ) -> Result<()> {
        self.core.conversation(&id)?;
        // An archive's or a delete's cleanup under way finishes first.
        self.cleanup_finished(&id).await;
        let Ok(conversation) = self.core.conversation(&id) else {
            // That cleanup was a delete.
            return Ok(());
        };
        self.close_fence(&id);
        let deleted = self
            .delete_closed(conversation, delete_branches, record_kept)
            .await;
        // Deleted, or still there and working: either way its fence has no more to hold.
        if deleted.is_ok()
            || self
                .core
                .conversation(&id)
                .is_ok_and(|now| now.lifecycle != Lifecycle::Archived && !now.deleting)
        {
            self.open_fence(&id);
        }
        deleted
    }

    /// Deletes a conversation once nothing new of it starts. It leaves the catalog last, after
    /// its streams are purged, so one cut off at any step is still there to finish.
    async fn delete_closed(
        &self,
        conversation: Conversation,
        delete_branches: bool,
        record_kept: bool,
    ) -> Result<()> {
        let id = conversation.id.clone();
        if !self.drain(&id).await {
            tracing::warn!(conversation = %id, "work of a deleted conversation is still going; deleting anyway");
        }
        self.delete_side_chats(&id).await;
        let runs = self.fence_runs(&id).await?;
        self.wind_down(&conversation).await;
        self.close_runs(runs, false).await;
        let runs = self.release_conversation_runs(&id).await;
        let tasks = self.core.tasks(&id).await.unwrap_or_default();
        // A repository that is gone (moved or removed by the user) has no branches left to
        // delete.
        if delete_branches
            && let Some(Setup::Session {
                repo, environment, ..
            }) = &conversation.setup
            && std::path::Path::new(repo).exists()
        {
            let mut branches: Vec<String> = tasks
                .iter()
                .filter_map(|task| task.workspace.as_ref().and_then(|w| w.branch.clone()))
                .collect();
            if let Environment::NewWorktree { branch, .. } = environment
                && branch.starts_with("brigadier/")
            {
                branches.push(branch.clone());
            }
            // Its runs' branches, once their worktrees are gone (one per run; Continue keeps it).
            for run in &runs {
                if let Some(workspace) = &run.workspace
                    && !branches.contains(&workspace.branch)
                {
                    branches.push(workspace.branch.clone());
                }
            }
            let (git, repo) = (self.git.clone(), PathBuf::from(repo));
            let _lane = self.closing.lane.lock().await;
            blocking(move || {
                let repo = git.open(&repo).map_err(git_error)?;
                for branch in branches {
                    if repo.branch_tip(&branch).map_err(git_error)?.is_some()
                        && let Err(err) = repo.delete_branch(&branch, true)
                    {
                        tracing::warn!(branch, error = %err, "could not delete a branch");
                    }
                }
                Ok(())
            })
            .await?;
        }
        if !delete_branches && record_kept {
            self.record_left_branches(super::project_removal::branch_records(
                &conversation,
                &tasks,
                &runs,
            ))
            .await;
        }
        let mut purge = vec![
            streams::conversation(&id),
            streams::orchestrator(&id),
            streams::draft(&id.to_string()),
        ];
        purge.extend(tasks.iter().map(|task| streams::task(&task.id)));
        self.forget_brain_conversation(&id, conversation.project_id.clone())
            .await;
        self.forget_routing(&id, &tasks).await;
        let store = self.core.store().clone();
        // Its own blobs go now, not a day later; then the usual collection of older leftovers.
        let (removed, own) = store.delete_streams_and_blobs(purge).await?;
        self.core.forget_conversation(id.clone()).await?;
        self.convs_lock().remove(&id);
        match store.gc_blobs().await {
            Ok(stats) => {
                tracing::info!(conversation = %id, removed, ?own, ?stats, "conversation deleted")
            }
            Err(err) => tracing::warn!(conversation = %id, error = %err, "could not collect blobs"),
        }
        Ok(())
    }
}

impl SessionManager {
    /// A closing conversation's run worktrees go (see [`Self::release_run_worktrees`]). Its
    /// runs, as they stand now.
    async fn release_conversation_runs(
        &self,
        id: &ConversationId,
    ) -> Vec<crate::overnight::OvernightRun> {
        let runs: Vec<_> = match self.core.board(id).await {
            Ok(board) => board.runs.values().cloned().collect(),
            Err(_) => return Vec::new(),
        };
        self.release_run_worktrees(&runs).await;
        runs
    }

    /// The routing store lets go of a deleted conversation: its turns and its tasks' outcomes.
    async fn forget_routing(&self, id: &ConversationId, tasks: &[Task]) {
        let Some(store) = self.runtime.routing_store().cloned() else {
            return;
        };
        let task_ids = tasks.iter().map(|task| task.id.0.clone()).collect();
        match store.forget_conversation(id.0.clone(), task_ids).await {
            Ok((turns, outcomes)) => {
                tracing::info!(conversation = %id, turns, outcomes, "routing forgot a deleted conversation")
            }
            Err(err) => {
                tracing::warn!(conversation = %id, error = %err, "could not forget a deleted conversation's routing records")
            }
        }
    }
}

fn being_deleted() -> Error {
    Error::Invalid("It is being deleted.".into())
}

/// The cleanup-ledger owner and data-dir area of a conversation's CLI session.
fn conversation_owner(conversation: &Conversation) -> (String, &'static str) {
    match conversation.kind {
        ConversationKind::Session => (format!("orch:{}", conversation.id), "orch"),
        ConversationKind::Chat => (format!("chat:{}", conversation.id), "chat"),
    }
}

/// Whether a restart cut off a fix Brigadier was landing on its own: the worker's fix report
/// waits for the checks Brigadier had not started yet, or the worker was still making it (a
/// task waiting for quota keeps waiting, and lands its fix when it reports).
fn interrupted_fix(task: &Task) -> bool {
    task.kind.writes()
        && task.landing.is_some()
        && task.quota_wait.is_none()
        && matches!(
            task.state,
            TaskState::Reported
                | TaskState::Queued
                | TaskState::Starting
                | TaskState::Running
                | TaskState::Blocked
                | TaskState::Paused
        )
}

/// Whether `worktree` is a linked worktree the user locked (`git worktree lock`): its admin
/// folder, which its `.git` file names, holds a `locked` file.
fn worktree_locked(worktree: &std::path::Path) -> bool {
    let Ok(link) = std::fs::read_to_string(worktree.join(".git")) else {
        return false;
    };
    let Some(admin) = link.trim().strip_prefix("gitdir:") else {
        return false;
    };
    let admin = PathBuf::from(admin.trim());
    let admin = if admin.is_absolute() {
        admin
    } else {
        worktree.join(admin)
    };
    admin.join("locked").exists()
}

/// The task test data folders in `root`.
fn test_folders(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("brigadier-test-"))
                && entry.file_type().is_ok_and(|kind| kind.is_dir())
        })
        .map(|entry| entry.path())
        .collect()
}

/// Removes those of `folders` no task needs (see `sweep_test_data`), through the ledger's
/// guard; `tasks` says, for the folders this data folder's tasks have, whether the task is
/// over, and `claimed` holds those its ledger still records for an owner it isn't disposing
/// of. A folder that can't be removed stays for the next launch. Returns how many went.
fn sweep_test_folders(
    folders: Vec<PathBuf>,
    tasks: &HashMap<PathBuf, bool>,
    claimed: &HashSet<PathBuf>,
    cutoff: SystemTime,
) -> usize {
    let mut removed = 0;
    for folder in folders {
        // What the ledger still holds goes with its owner.
        let stale = !claimed.contains(&folder)
            && match tasks.get(&folder) {
                Some(over) => *over,
                None => !changed_since(&folder, cutoff),
            };
        if !stale {
            continue;
        }
        match crate::ledger::remove_test_data_folder(&folder) {
            Ok(()) => removed += 1,
            Err(err) => {
                tracing::warn!(folder = %folder.display(), error = %err, "could not remove a test data folder");
            }
        }
    }
    removed
}

/// Whether `path`, or anything in it, changed at or after `cutoff` (or can't be read).
fn changed_since(path: &Path, cutoff: SystemTime) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return true;
    };
    if meta.modified().map_or(true, |modified| modified >= cutoff) {
        return true;
    }
    if !meta.is_dir() {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return true;
    };
    entries
        .into_iter()
        .any(|entry| entry.map_or(true, |entry| changed_since(&entry.path(), cutoff)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test data folder in the temp directory, named for a new task, with a file in it,
    /// all last changed `age` ago.
    fn test_folder(age: Duration) -> PathBuf {
        let folder = test_data_dir(&crate::work::TaskId::generate());
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("data.txt");
        std::fs::write(&file, "test data").unwrap();
        let then = SystemTime::now() - age;
        for path in [&file, &folder] {
            set_changed(path, then);
        }
        folder
    }

    fn set_changed(path: &Path, at: SystemTime) {
        #[cfg(windows)]
        let file = {
            use std::os::windows::fs::OpenOptionsExt;
            // FILE_FLAG_BACKUP_SEMANTICS, which opens a folder too.
            std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(0x0200_0000)
                .open(path)
        };
        #[cfg(not(windows))]
        let file = std::fs::File::open(path);
        file.unwrap().set_modified(at).unwrap();
    }

    #[test]
    fn the_launch_sweep_removes_test_data_no_task_needs_and_keeps_the_rest() {
        let day = Duration::from_secs(24 * 60 * 60);
        let ended = test_folder(Duration::ZERO);
        let live = test_folder(2 * day);
        let recent = test_folder(Duration::from_secs(60));
        let stale = test_folder(2 * day);
        let claimed = test_folder(2 * day);
        let held_after_end = test_folder(Duration::ZERO);
        // Another data folder's task wrote into its folder a minute ago.
        let in_use = test_folder(2 * day);
        set_changed(
            &in_use.join("data.txt"),
            SystemTime::now() - Duration::from_secs(60),
        );
        let tasks = HashMap::from([
            (ended.clone(), true),
            (live.clone(), false),
            (held_after_end.clone(), true),
        ]);
        let held = HashSet::from([claimed.clone(), held_after_end.clone()]);
        let folders = vec![
            ended.clone(),
            live.clone(),
            recent.clone(),
            stale.clone(),
            claimed.clone(),
            held_after_end.clone(),
            in_use.clone(),
        ];
        let removed = sweep_test_folders(folders, &tasks, &held, SystemTime::now() - day);
        assert_eq!(removed, 2);
        assert!(!ended.exists());
        assert!(!stale.exists());
        for kept in [&live, &recent, &claimed, &held_after_end, &in_use] {
            assert!(kept.exists(), "{}", kept.display());
            std::fs::remove_dir_all(kept).unwrap();
        }
        // Only test data folders are listed.
        let folders = test_folders(&test_data_root());
        assert!(folders.iter().all(|folder| {
            folder
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("brigadier-test-"))
        }));
    }

    fn task(state: TaskState, landing: bool) -> Task {
        let mut task: Task = serde_json::from_value(serde_json::json!({
            "id": "t1",
            "conversationId": "c1",
            "number": 1,
            "position": 0,
            "title": "Add the flag",
            "kind": "implement",
            "spec": "Add the flag.",
            "access": { "repo": "write", "network": false, "unsandboxed": false },
            "route": { "choice": { "provider": "claude", "model": null, "effort": null }, "reason": "" },
            "state": "reported",
            "attachments": [],
            "createdAtMs": 0,
            "updatedAtMs": 0
        }))
        .expect("a task");
        task.state = state;
        task.landing = landing.then(|| "Add the flag".to_owned());
        task
    }

    #[test]
    fn a_fix_a_restart_cut_off_goes_to_the_orchestrator() {
        // Its fix report, not yet checked, and a worker still fixing.
        assert!(interrupted_fix(&task(TaskState::Reported, true)));
        assert!(interrupted_fix(&task(TaskState::Running, true)));
        // A report the orchestrator already has, and a landing under way (handled apart).
        assert!(!interrupted_fix(&task(TaskState::Reported, false)));
        assert!(!interrupted_fix(&task(TaskState::Landing, true)));
        assert!(!interrupted_fix(&task(TaskState::ReadyToLand, true)));
        let mut review = task(TaskState::Reported, true);
        review.kind = crate::work::TaskKind::Review;
        assert!(!interrupted_fix(&review));
    }
}
