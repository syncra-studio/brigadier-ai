//! The lifecycle of sessions and Chats (PLAN §5): hibernate, archive, restore, delete, and
//! the recovery after Brigadier was quit or crashed.
//!
//! - **Hibernate** (automatic when idle, or on request): the conversation's CLI sessions
//!   stop and their temp files go. The CLI session files stay, so the next message resumes
//!   it; if they are gone, it starts over from Brigadier's transcript.
//! - **Archive**: stops the session and its workers, removes transient and build output and
//!   clean merged worktrees, and keeps records, lineage transcripts, Brain entries and refs.
//! - **Delete**: removes those records too, including every worker, its branch (after the
//!   existing unmerged confirmation), transcript files, Brain entries and snapshot refs.
//! - **Recovery**: after a restart, work that was running has lost its CLI: tasks end (their
//!   work kept), cards nobody waits for expire, and the ledger finishes every cleanup.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use brigadier_providers::{Artifact, BoxFuture, ItemStatus};

use super::conversation::{ConvLive, Envelope};
use super::workers::{test_data_dir, test_data_root};
use super::{SessionManager, blocking, git_error};
use crate::model::{
    Conversation, ConversationId, ConversationKind, Environment, Lifecycle, Setup, streams,
};
use crate::work::{InjectionKind, OrchestratorStepKind, RequestState, Task, TaskState};
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
        let refs_git = self.git.clone();
        self.runtime
            .ledger()
            .set_ref_remover(Arc::new(move |repo, namespace| {
                let git = refs_git.clone();
                Box::pin(async move {
                    let session = namespace
                        .strip_prefix("refs/brigadier/s/")
                        .and_then(|s| s.strip_suffix('/'))
                        .ok_or_else(|| "invalid session refs namespace".to_owned())?
                        .to_owned();
                    tokio::task::spawn_blocking(move || {
                        git.open(&repo)
                            .and_then(|repo| repo.delete_session_snapshots(&session))
                            .map_err(|err| err.to_string())
                    })
                    .await
                    .map_err(|err| err.to_string())?
                })
            }));
        let space_git = self.git.clone();
        self.runtime
            .ledger()
            .set_git_remover(Arc::new(move |artifact: Artifact| {
                let git = space_git.clone();
                Box::pin(async move {
                    tokio::task::spawn_blocking(move || remove_git_leftover(&git, artifact))
                        .await
                        .map_err(|err| err.to_string())?
                })
            }));
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
        // Before the sweep finishes old disposals: a merge's removal of a session workspace is
        // taken back (BUILD-PLAN A3).
        self.keep_merged_workspaces().await;
        self.runtime.ledger().sweep().await;
        // Spare worktrees an earlier version made ahead of a task that never came (BUILD-PLAN A8).
        self.sweep_spare_worktrees().await;
        self.reconcile_trust().await;
        // Tab chats whose session went go now; those whose tab closed, once the app says.
        self.sweep_tab_chats().await;
        // Each big await below is polled from the heap ([`crate::heap`]): recovery runs on a
        // runtime thread's stack, and their futures in this frame came to over 700 KB.
        for conversation in self.core.catalog().conversations {
            // One still being deleted (a delete that failed at launch) gets nothing new.
            if conversation.deleting {
                continue;
            }
            // No turn starts until what the stop left is back and the reviews' recovery has
            // seen it (#210): a turn now could take or settle what it has yet to see.
            let live = self.conv(&conversation.id).ok();
            if let Some(live) = &live {
                live.recovering(true).await;
            }
            // A queued message the stop caught Deciding waits for its own turn (#449).
            self.settle_unjudged(&conversation.id).await;
            // The thread's turn the restart cut off ran its calls no further, and its request
            // reads interrupted, not answered.
            self.end_cut_off_calls(&conversation.id).await;
            // A seam the restart came in goes on from its record (S22); the turn it cut off
            // reads interrupted. What the stop left in the thread's inbox goes back once a
            // cut-off request reads so, and before reviews are told again (so a review in it
            // already isn't told twice).
            crate::heap(|| self.recover_seam(&conversation.id)).await;
            self.mark_cut_off_turn(&conversation.id).await;
            self.recover_unheard_envelopes(&conversation.id).await;
            // What its workers did that it hadn't heard yet (S29).
            if conversation.lifecycle != Lifecycle::Archived {
                self.recover_worker_events(&conversation.id).await;
                // Other sessions' messages its CLI never took (S30).
                self.recover_session_messages(&conversation.id).await;
            }
            // Merge cards are retired: open ones close, and no card answer merges.
            if conversation.lifecycle != Lifecycle::Archived {
                self.retire_merge_cards(&conversation.id).await;
            }
            // Messages that waited for quota keep waiting (an archived conversation's never go).
            if conversation.quota_wait.is_some() && conversation.lifecycle != Lifecycle::Archived {
                self.keep_conversation_wait(&conversation).await;
            }
            let Ok(tasks) = self.core.tasks(&conversation.id).await else {
                if let Some(live) = &live
                    && live.recovering(false).await
                {
                    self.kick(live);
                }
                continue;
            };
            for task in tasks.into_iter().filter(|task| !task.state.is_final()) {
                match task.state {
                    // Its worktree is intact; the worker resumes when its session sends it more.
                    TaskState::Reported | TaskState::ReadyToLand if task.kind.writes() => continue,
                    // Its CLI stopped with Brigadier: its worktree and session stay, and it goes
                    // on, resumed, once its session sends it more or the user resumes it.
                    TaskState::Running
                    | TaskState::Starting
                    | TaskState::Blocked
                    | TaskState::Paused
                        if conversation.lifecycle != Lifecycle::Archived
                            && task.native_session.is_some() =>
                    {
                        // A paused one stays paused, as it was.
                        if task.state == TaskState::Paused {
                            continue;
                        }
                        // A plain worker's cut-off turn ends as an interrupted one does: it
                        // waits for its session's next message, and its parent hears it
                        // stopped (S29), once, with how it goes on (#559).
                        let plain = task.plain.is_some();
                        let _ = self
                            .update_task(&conversation.id, &task.id, |t| {
                                if plain {
                                    t.state = TaskState::Reported;
                                    t.blocked_reason = None;
                                } else {
                                    t.state = TaskState::Paused;
                                    t.blocked_reason =
                                        Some("Brigadier restarted while it worked.".into());
                                }
                            })
                            .await;
                        if plain {
                            self.plain_cut_off(&task);
                        }
                        continue;
                    }
                    // A read task that reported is done.
                    TaskState::Reported => {
                        crate::heap(|| self.dispose_task(&task, TaskState::Done)).await;
                        continue;
                    }
                    _ => {}
                }
                let _ = self
                    .update_task(&conversation.id, &task.id, |t| {
                        t.error = Some("Brigadier quit while this task was running.".into());
                    })
                    .await;
                crate::heap(|| self.dispose_task(&task, TaskState::Stopped)).await;
            }
            crate::heap(|| self.expire_stale_cards(&conversation.id)).await;
            if let Some(live) = &live
                && live.recovering(false).await
            {
                self.kick(live);
            }
            // So are its previews (the sweep above ended any a crash left running).
            crate::heap(|| self.recover_previews(&conversation.id)).await;
            // Not its pull request's: the one its ready work waits on is watched again.
            if conversation.lifecycle != Lifecycle::Archived {
                self.rewatch_pr(&conversation.id).await;
            }
            // Nothing runs any more: what was working is over or waits for the user.
            self.settle_requests(&conversation.id).await;
        }
        self.sweep_test_data().await;
    }

    /// Ends the thread's calls a restart cut off: no turn of the thread runs at startup, so
    /// a call still running (a command, a wait for replies) failed with its turn, as a Stop
    /// ends it, rather than showing as running in a turn that is over.
    async fn end_cut_off_calls(&self, id: &ConversationId) {
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        for step in &board.orchestrator_steps {
            let mut kind = step.kind.clone();
            if let OrchestratorStepKind::Tool {
                status: status @ ItemStatus::InProgress,
                stopped,
                through_position,
                ended_at_ms,
                exit,
                ..
            } = &mut kind
            {
                *status = ItemStatus::Failed;
                // A call the user's Stop ended still reads stopped (#75, #184).
                *stopped = step
                    .request_id
                    .as_ref()
                    .and_then(|request| board.requests.get(request))
                    .is_some_and(|request| request.state == RequestState::Stopped);
                *through_position = 0;
                // When it ended isn't known: it ran no further than it is recorded.
                *ended_at_ms = Some(step.at_ms);
                *exit = None;
                self.orchestrator_step_in(id, step.request_id.clone(), kind)
                    .await;
            }
        }
    }

    /// Removes the test data folders no task needs any more (PLAN.md §10.13): a task's own
    /// goes when it ends, but a crash, an older build or a deleted data folder leaves them
    /// behind. Several data folders (dev builds, smoke runs) share the temp directory, so a
    /// folder goes only if this data folder's task for it is over, or if no task here claims
    /// it and nothing in it changed for `UNCLAIMED_TEST_DATA_AGE`. The folders are read and
    /// removed in the background: a big one must not hold up the launch.
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
        self.spawn(async move {
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
        });
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
            if since > cutoff || conv.is_busy().await || self.has_running_workers(&conv.id).await {
                continue;
            }
            if let Err(err) = self.hibernate(conv.id.clone()).await {
                crate::logging::private_trace!(conv.private_logs, debug, "could not hibernate"; conversation = %conv.id, error = %err);
            }
        }
    }

    /// Whether any conversation has a turn in progress, work waiting for one, or a worker
    /// that hasn't finished: what keeping the computer awake "while agents work" means.
    pub async fn agents_working(&self) -> bool {
        let convs: Vec<_> = self.convs_lock().values().cloned().collect();
        for conv in convs {
            if conv.is_busy().await || self.has_running_workers(&conv.id).await {
                return true;
            }
        }
        false
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
            // Closing the CLI would end them with it, and their results with them.
            if conv.has_cli_jobs().await {
                return Err(Error::Invalid(
                    "a command the thread left running in its CLI hasn't ended; it can rest once that ends".into(),
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

    /// Archives a conversation: stops its processes and workers, removes transient output
    /// and clean merged worktrees, and keeps the record, transcripts, Brain and snapshot refs. Answers once nothing new of it can start and it is stored as
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
        let archived = match self.core.mark_archived(id.clone()).await {
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
            manager.finish_archive(&id).await;
        });
        Ok(archived)
    }

    /// The cleanup of an archive: once the work that passed the fence has finished, its side
    /// chats go, everything it runs stops, and its disposable output goes. The cleanup mark
    /// goes last, so a restart finishes a cleanup cut off at any step.
    pub(super) async fn finish_archive(&self, id: &ConversationId) {
        let drained = self.drain(id).await;
        self.delete_side_chats(id).await;
        let Ok(conversation) = self.core.conversation(id) else {
            return;
        };
        self.wind_down(&conversation).await;
        self.settle_requests(id).await;
        if !drained {
            // What that work makes after this is removed once it has finished: the mark stays
            // until then.
            crate::logging::private_trace!(self.tab_chat_logs(id), warn, "work of an archived session is still going; cleaning up again once it ends"; conversation = %id);
            self.again_after_late_work(id, |manager| async move {
                manager.finish_cleanup_of(&conversation.id).await;
            });
            return;
        }
        if let Err(err) = self.core.finish_cleanup(id.clone()).await {
            crate::logging::private_trace!(self.tab_chat_logs(id), warn, "could not record the end of an archive's cleanup; the next launch looks again"; conversation = %id, error = %err);
        }
    }

    /// At launch, before any run resumes: finishes the deletes and the archives' cleanups a
    /// quit or crash cut off (their processes are gone; worktrees, session files, runs and
    /// streams are not).
    pub(super) async fn finish_cut_off_cleanups(&self) {
        for conversation in self.core.catalog().conversations {
            self.finish_cleanup_of(&conversation.id).await;
        }
    }

    /// Finishes conversation `id`'s delete or archive cleanup, if one is marked as not done (a
    /// quit cut it off, or work it stopped waiting for went on).
    fn finish_cleanup_of<'a>(&'a self, id: &'a ConversationId) -> BoxFuture<'a, ()> {
        // Boxed: an archive's cleanup comes back here once late work has ended.
        Box::pin(async move {
            let Ok(conversation) = self.core.conversation(id) else {
                return;
            };
            if conversation.deleting {
                crate::logging::private_trace!(self.tab_chat_logs(id), info, "finishing a delete"; conversation = %id);
                self.close_fence(id);
                self.finish_delete(id).await;
                return;
            }
            if !conversation.cleanup_pending {
                return;
            }
            // Restored meanwhile: what it has is its own again.
            if conversation.lifecycle != Lifecycle::Archived {
                let _ = self.core.finish_cleanup(id.clone()).await;
                return;
            }
            crate::logging::private_trace!(self.tab_chat_logs(id), info, "finishing an archive's cleanup"; conversation = %id);
            self.close_fence(id);
            self.finish_archive(id).await;
        })
    }

    /// Removes the spare `task-next-*` worktrees earlier versions made ahead of a session's next
    /// task: an owner `task:<id>` that holds one and whose task was never made. The ledger's own
    /// sweep finishes only disposals already started, so these stayed on disk.
    async fn sweep_spare_worktrees(&self) {
        let mut tasks = std::collections::HashSet::new();
        for conversation in self.core.catalog().conversations {
            if let Ok(found) = self.core.tasks(&conversation.id).await {
                tasks.extend(found.into_iter().map(|task| task.id.0));
            }
        }
        let ledger = self.runtime.ledger();
        for (owner, artifacts, _) in ledger.owners() {
            let Some(id) = owner.strip_prefix("task:") else {
                continue;
            };
            let spare = artifacts.iter().any(|artifact| {
                matches!(artifact, Artifact::Worktree { path, .. }
                    if Path::new(path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("task-next-")))
            });
            if !spare || tasks.contains(id) {
                continue;
            }
            let leftovers = ledger.dispose(&owner).await;
            tracing::info!(
                owner,
                failures = leftovers.failures.len(),
                "removed a spare worktree"
            );
        }
    }

    /// Takes back the removal of a session's workspace that an earlier version started on a
    /// merge (THREAD-PLAN.md Q9, retired by BUILD-PLAN A3) and couldn't finish: a `session:`
    /// owner still disposing whose session is neither archived nor being deleted. Its worktree
    /// stays, and the session works in it again if it is still there. Archive, delete and
    /// project removal dispose of the owner after the session is archived or marked deleting,
    /// so theirs go on.
    async fn keep_merged_workspaces(&self) {
        let ledger = self.runtime.ledger();
        let disposing = ledger.disposing();
        for conversation in self.core.catalog().conversations {
            let owner = format!("session:{}", conversation.id);
            if !disposing.contains(&owner)
                || conversation.deleting
                || conversation.lifecycle == Lifecycle::Archived
            {
                continue;
            }
            // Read before the disposal is taken back, which ends it.
            let closing = ledger.closing(&owner);
            if let Err(err) = ledger.cancel_disposal(&owner).await {
                tracing::warn!(conversation = %conversation.id, error = %err, "could not keep a merged session's workspace");
                continue;
            }
            tracing::info!(conversation = %conversation.id, "kept a merged session's workspace");
            // The session still records its folder: it stays. Only a Close workspace that
            // couldn't remove it says so (#182); the user hears it stays rather than finding
            // it there unannounced.
            if let Some(Setup::Session {
                environment:
                    Environment::NewWorktree {
                        path: Some(path), ..
                    },
                ..
            }) = &conversation.setup
            {
                if !closing {
                    continue;
                }
                self.notice(
                    &conversation.id,
                    brigadier_providers::NoticeLevel::Warning,
                    &format!(
                        "Removing this session's worktree didn't finish, so its folder {path} stays and the session keeps working there. Remove the worktree again to finish."
                    ),
                )
                .await;
                continue;
            }
            let Some(Setup::Session {
                repo,
                environment:
                    Environment::NewWorktree {
                        base,
                        branch,
                        path: None,
                        start,
                    },
                permission,
                orchestrator,
                workers_see_uncommitted,
                plan_mode,
            }) = conversation.setup.clone()
            else {
                continue;
            };
            let Some(path) =
                ledger
                    .artifacts(&owner)
                    .into_iter()
                    .find_map(|artifact| match artifact {
                        Artifact::Worktree { path, .. }
                            if Path::new(&path).join(".git").exists() =>
                        {
                            Some(path)
                        }
                        _ => None,
                    })
            else {
                continue;
            };
            let _ = self
                .core
                .set_setup(
                    conversation.id.clone(),
                    Setup::Session {
                        repo,
                        environment: Environment::NewWorktree {
                            base,
                            branch,
                            path: Some(path),
                            start,
                        },
                        permission,
                        orchestrator,
                        workers_see_uncommitted,
                        plan_mode,
                    },
                )
                .await;
        }
    }

    /// "Close workspace" (BUILD-PLAN A3): the user removes the session's own worktree, after a
    /// confirm. Its uncommitted changes are kept as a commit on its branch first; the branch
    /// stays unless all of its work is on the base. The next change makes a fresh worktree in
    /// the same folder. Refused while anything still works in it.
    pub async fn close_workspace(&self, id: &ConversationId) -> Result<String> {
        let conversation = self.core.conversation(id)?;
        if conversation.deleting || conversation.lifecycle == Lifecycle::Archived {
            return Err(Error::Invalid(
                "This session is closed already: its worktree went with it.".into(),
            ));
        }
        let Some(Setup::Session {
            environment:
                Environment::NewWorktree {
                    path: Some(path),
                    branch,
                    base,
                    ..
                },
            ..
        }) = conversation.setup.clone()
        else {
            return Err(Error::Invalid(
                "This session has no worktree of its own to remove.".into(),
            ));
        };
        self.quiet_for_close(id).await?;
        // No turn starts from here until the folder is gone: a message or a worker's report
        // that comes meanwhile waits for the turn after, in a fresh worktree.
        let conv = self.conv(id)?;
        if !conv
            .reserve_workspace_within(std::time::Duration::from_secs(10))
            .await
        {
            return Err(Error::Invalid(
                "The session is working: stop it first, then remove its worktree.".into(),
            ));
        }
        let closed = self
            .close_reserved(id, &conversation, &path, &branch, &base, &conv)
            .await;
        conv.release_workspace().await;
        self.kick(&conv);
        closed
    }

    /// The end of [`Self::close_workspace`], while no turn of the session can start.
    async fn close_reserved(
        &self,
        id: &ConversationId,
        conversation: &Conversation,
        path: &str,
        branch: &str,
        base: &str,
        conv: &ConvLive,
    ) -> Result<String> {
        // Asked again just before the folder goes: a turn or worker that started meanwhile
        // keeps it (Codex review, phase A verify).
        self.quiet_for_close(id).await?;
        // What runs in the folder stops before it goes.
        let _ = self.stop_previews(id, "its worktree was removed").await;
        self.stop_session_work(id);
        // A request that just ended takes its last snapshot of the folder right after: it
        // runs before the folder goes.
        if !self
            .requests_snapshotted_within(id, std::time::Duration::from_secs(10))
            .await
        {
            return Err(Error::Invalid(
                "The session is still finishing its last request: try again in a moment.".into(),
            ));
        }
        // Its changes are kept only now, with no turn able to start: a turn the wait for the
        // sweep let through can't leave changes behind unkept.
        if !self.keep_session_changes(conversation).await {
            return Err(Error::Invalid(
                "Its uncommitted changes couldn't be kept, so the worktree stays.".into(),
            ));
        }
        let leftovers = self
            .runtime
            .ledger()
            .dispose_closing(&format!("session:{id}"))
            .await;
        if !leftovers.is_clean() {
            return Err(Error::Invalid(format!(
                "The worktree couldn't be removed: {}",
                leftovers.failures.join("; ")
            )));
        }
        let deleted = self.forget_session_worktree(conversation).await;
        let branch_line = if deleted {
            format!("Branch `{branch}` went too: all of its work is on `{base}`.")
        } else {
            format!("Branch `{branch}` stays.")
        };
        conv.workspace_closed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        conv.note(format!(
            "[worktree removed] The user removed this session's worktree: its folder {path} is gone. {branch_line} The next change makes a fresh worktree in the same folder."
        ))
        .await;
        Ok(format!("Removed the worktree. {branch_line}"))
    }

    /// Whether every request of the session that isn't waiting ended and took its last
    /// snapshot of the folder (its changes settled), waiting up to `limit` for them.
    async fn requests_snapshotted_within(
        &self,
        id: &ConversationId,
        limit: std::time::Duration,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let Ok(board) = self.core.board(id).await else {
                return true;
            };
            // A request still shown working with its turn over is about to end and settle.
            let pending = board.requests.values().any(|request| match request.state {
                crate::work::RequestState::Working => true,
                crate::work::RequestState::Waiting => false,
                _ => request
                    .changes
                    .as_ref()
                    .is_some_and(|changes| !changes.settled),
            });
            if !pending {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// Why the session's workspace can't close now: its thread works, or a worker of it runs
    /// (a read task's report would start a turn in the folder that goes).
    async fn quiet_for_close(&self, id: &ConversationId) -> Result<()> {
        let board = self.core.board(id).await?;
        if !matches!(
            board.run,
            crate::work::RunState::Idle
                | crate::work::RunState::Hibernated
                | crate::work::RunState::Failed
        ) {
            return Err(Error::Invalid(
                "The session is working: stop it first, then remove its worktree.".into(),
            ));
        }
        let running: Vec<String> = self
            .core
            .tasks(id)
            .await?
            .iter()
            .filter(|task| !task.state.is_final())
            .map(|task| format!("task-{}", task.number))
            .collect();
        if !running.is_empty() {
            return Err(Error::Invalid(format!(
                "Workers of this session still run ({}): stop them or let them finish first.",
                running.join(", ")
            )));
        }
        Ok(())
    }

    /// Stops everything a conversation runs and removes what it created.
    pub(super) async fn wind_down(&self, conversation: &Conversation) {
        self.wind_down_for(conversation, false).await;
    }

    async fn wind_down_for(&self, conversation: &Conversation, deleting: bool) {
        let id = &conversation.id;
        // Its previews run in the workspace that goes next: they stop first.
        let _ = self.stop_previews(id, "the session closed").await;
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
        // A tab chat's folder, and what runs in it, is its own owner's and goes with it: the
        // shared records never name it.
        let tab_chat = conversation.tab_chat.is_some();
        if !tab_chat {
            // Also stop detached commands in the session folder, including retained worktrees.
            let folder_owner = super::session_dir::folder_owner(id);
            let process = Artifact::ProcessesIn {
                dir: self.session_dir(id).root().to_string_lossy().into_owned(),
            };
            let ledger = self.runtime.ledger();
            if let Err(err) = ledger.record(&folder_owner, process).await {
                tracing::warn!(conversation = %id, error = %err, "could not record the session's process cleanup");
            }
            let _ = ledger.stop_processes(&folder_owner).await;
        }
        let _lane = self.closing.lane.lock().await;
        // What an older Brigadier kept of landed workers for their reviews goes too.
        let mut held = Vec::new();
        if let Ok(tasks) = self.core.tasks(id).await {
            for task in tasks {
                let owner = format!("review-hold:{}", task.id);
                if !self.runtime.ledger().artifacts(&owner).is_empty() {
                    // The held session's folder goes now (BUILD-PLAN D2.2).
                    self.keep_saved_files(&task.id).await;
                    held.push(owner);
                }
                if deleting {
                    self.dispose_task(&task, TaskState::Stopped).await;
                } else {
                    self.archive_worker(&task).await;
                }
            }
        }
        // Its thread's setup script stops.
        self.stop_session_work(id);
        let (owner, _) = conversation_owner(conversation);
        self.grants.revoke_owner(&owner);
        let mut owners = vec![
            owner,
            super::workers::worker_home_owner(id),
            super::workers::thread_temp_owner(id),
            super::setup_script::setup_owner(id),
            super::preview::preview_data_owner(id),
            // The checkout an older Brigadier ran the session's full checks in.
            format!("checks:{id}"),
        ];
        owners.extend(held);
        // Its previews' log folder.
        let previews = super::preview::preview_owner(id);
        if !self.runtime.ledger().artifacts(&previews).is_empty() {
            owners.push(previews);
        }
        let session_worktree_goes = if deleting {
            self.keep_session_changes(conversation).await
        } else {
            match &conversation.setup {
                Some(Setup::Session {
                    environment:
                        Environment::NewWorktree {
                            path: Some(path),
                            branch,
                            base,
                            ..
                        },
                    ..
                }) => {
                    self.archivable_worktree(Path::new(path), branch, base)
                        .await
                }
                _ => true,
            }
        };
        if session_worktree_goes {
            owners.push(format!("session:{id}"));
        }
        for owner in owners {
            let leftovers = self.runtime.ledger().dispose(&owner).await;
            if !leftovers.is_clean() {
                crate::logging::private_trace!(owner.starts_with(crate::ledger::TAB_CHAT_OWNER), warn, "some leftovers will be retried at the next launch"; owner,
                    ?leftovers);
            }
        }
        if !tab_chat {
            self.remove_transient_parts(id).await;
        }
        // Its CLI session files stay (S8): Restore resumes the same CLI session.
        self.expire_stale_cards(id).await;
        self.set_run(id, crate::work::RunState::Idle, None).await;
        if session_worktree_goes {
            self.forget_session_worktree(conversation).await;
        }
    }

    /// Archive retains unmerged or dirty worktrees, without making a WIP commit.
    async fn archivable_worktree(&self, path: &Path, branch: &str, base: &str) -> bool {
        if !path.exists() {
            return true;
        }
        let sessions = self.runtime.platform().paths().sessions_dir.clone();
        let root = if path.starts_with(&sessions) {
            sessions
        } else {
            self.data_dir.clone()
        };
        let (git, path, branch, base) = (
            self.git.clone(),
            path.to_owned(),
            branch.to_owned(),
            base.to_owned(),
        );
        blocking(move || {
            let folder = brigadier_sandbox::removal::bind(&root, &path)
                .map_err(|err| Error::Invalid(err.to_string()))?;
            let repo = git.open(&path).map_err(git_error)?;
            let state = repo.state().map_err(git_error)?;
            let clean =
                state.dirty_files.is_empty() && state.current_branch.as_deref() == Some(&branch);
            let merged = super::disk::worktree_state::merged_into(&repo, &branch, &base);
            super::disk::archive_output(&git, &folder)?;
            Ok(clean && merged)
        })
        .await
        .unwrap_or(false)
    }

    /// G19: a worker keeps its record and transcript just like its parent.
    async fn archive_worker(&self, task: &Task) {
        if let Some(live) = self.existing_task_live(&task.id) {
            live.close_cli().await;
        }
        self.stop_task_previews(&task.conversation_id, &task.id)
            .await;
        let owner = format!("task:{}", task.id);
        let ledger = self.runtime.ledger();
        let _ = ledger.stop_processes(&owner).await;
        let remove_worktree = match &task.workspace {
            Some(workspace) => match (&workspace.worktree, &workspace.branch, &workspace.target) {
                (Some(path), Some(branch), Some(base)) => {
                    self.archivable_worktree(Path::new(path), branch, base)
                        .await
                }
                _ => false,
            },
            None => false,
        };
        let lineage = crate::ledger::lineage_owner(&task.conversation_id.0);
        if let Err(err) = ledger.hand_over_session_files(&owner, &lineage).await {
            tracing::warn!(task = %task.id, error = %err, "could not keep the worker's lineage");
            return;
        }
        let outputs = self.final_outputs(task).await;
        self.keep_saved_files(&task.id).await;
        // What goes now: everything when its worktree goes; else all but the worktree, a folder
        // of its own (a worker with no repository) with the rest.
        let released: Vec<Artifact> = if remove_worktree {
            Vec::new()
        } else {
            ledger
                .artifacts(&owner)
                .into_iter()
                .filter(|artifact| !matches!(artifact, Artifact::Worktree { .. }))
                .collect()
        };
        let folder_goes = remove_worktree
            || task
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.worktree.as_deref())
                .is_some_and(|folder| {
                    released.iter().any(|artifact| {
                        matches!(artifact, Artifact::ScratchDir { path } if path == folder)
                    })
                });
        let _ = self
            .update_task(&task.conversation_id, &task.id, |current| {
                // A worker that had reported reads so after a restore. Its session can send it
                // more while its folder is kept; one whose folder went with the archive is closed.
                let reported = current.state == TaskState::Reported && current.plain.is_some();
                if !current.state.is_final() && !reported {
                    current.state = TaskState::Stopped;
                }
                if let Some(plain) = &mut current.plain
                    && !(reported && !folder_goes)
                {
                    plain.closed = true;
                }
                current.quota_wait = None;
                if let Some(outputs) = outputs {
                    current.outputs = outputs;
                }
            })
            .await;
        if remove_worktree {
            // Disposal records a durable retry if a merged worktree cannot go yet.
            let _ = ledger.dispose(&owner).await;
        } else {
            let _ = ledger.release(&owner, released).await;
        }
        self.grants.revoke_owner(&owner);
        self.tasks_lock().remove(&task.id);
    }

    async fn remove_transient_parts(&self, id: &ConversationId) {
        let dir = self.session_dir(id);
        let mut folders = vec![dir.root().to_owned()];
        if let Ok(tasks) = self.core.tasks(id).await {
            folders.extend(tasks.iter().map(|task| dir.worker(task)));
        }
        let owner = format!("archive-parts:{id}");
        let ledger = self.runtime.ledger();
        for folder in folders {
            for part in ["tmp", "dev", "logs"] {
                let path = folder.join(part);
                if path.exists() {
                    let _ = ledger
                        .record(
                            &owner,
                            Artifact::ScratchDir {
                                path: path.to_string_lossy().into_owned(),
                            },
                        )
                        .await;
                }
            }
        }
        let _ = ledger.dispose(&owner).await;
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
        if self
            .runtime
            .ledger()
            .branch_ownership()
            .owns(Path::new(&repo), &branch)
        {
            self.forget_branch(Path::new(&repo), &branch);
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
                    crate::logging::private_trace!(self.tab_chat_logs(id), warn, "could not delete the merged session branch"; conversation = %id, error = %err);
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
                crate::logging::private_trace!(self.tab_chat_logs(&conversation.id), info, "kept the session worktree's changes as a WIP commit"; conversation = %conversation.id, commit = %commit.0);
                true
            }
            Ok(None) => true,
            Err(err) => {
                crate::logging::private_trace!(self.tab_chat_logs(&conversation.id), error, "could not keep the session worktree's changes; the worktree stays"; conversation = %conversation.id, error = %err);
                false
            }
        }
    }

    /// Brings an archived conversation back; its next turn resumes its CLI session, kept
    /// through the archive (S8), or starts from the transcript when that can't resume.
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
        let admitting = self.local_checkouts.lock().await;
        if let Some(Setup::Session {
            repo,
            environment: Environment::LocalCheckout { .. },
            ..
        }) = &conversation.setup
            && let Some(holder) = self.checkout_holder(repo, Some(&id))
        {
            return Err(Error::Invalid(format!(
                "“{}” already works in this checkout. Archive it before restoring this session.",
                holder.title
            )));
        }
        let restored = self
            .core
            .set_lifecycle(id.clone(), Lifecycle::Active)
            .await?;
        drop(admitting);
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
    /// too; the user's own branches never). Its Brain entries and transcript index go. Answers once nothing new of it can start and it is durably marked as being
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
        let marked = self.core.mark_deleting(id.clone()).await;
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
            Ok(true) => {
                self.open_fence(id);
                self.note_space_freed();
            }
            // The mark stays, and the fence closed, until it is done.
            Ok(false) => {
                let id = id.clone();
                self.again_after_late_work(&id.clone(), |manager| async move {
                    manager.finish_cleanup_of(&id).await;
                });
            }
            Err(err) => {
                crate::logging::private_trace!(self.tab_chat_logs(id), warn, "could not finish deleting a conversation; the next launch tries again"; conversation = %id, error = %err)
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
        self.delete_now(id, delete_branches, record_kept).await
    }

    /// [`Self::delete_conversation`] once no cleanup of it is under way.
    fn delete_now(
        &self,
        id: ConversationId,
        delete_branches: bool,
        record_kept: bool,
    ) -> BoxFuture<'_, Result<()>> {
        // Boxed: it comes back here once late work has ended.
        Box::pin(async move {
            let Ok(conversation) = self.core.conversation(&id) else {
                // That cleanup was a delete.
                return Ok(());
            };
            self.close_fence(&id);
            // Only a session has branches of its own: a chat's delete is the same whatever was
            // asked of them.
            let has_branches = matches!(conversation.setup, Some(Setup::Session { .. }));
            let deleted = self
                .delete_closed(conversation, delete_branches, record_kept)
                .await;
            if let Ok(false) = deleted {
                // A chat (a side chat going with its parent) is marked, so a quit before that
                // work ends leaves its delete to the next launch. A session isn't: that delete
                // would take its branches whatever was asked (a project removal stops instead,
                // and can be done again).
                if !has_branches && let Err(err) = self.core.mark_deleting(id.clone()).await {
                    crate::logging::private_trace!(self.tab_chat_logs(&id), warn, "could not mark a delete that waits for late work"; conversation = %id, error = %err);
                }
                // Its fence stays closed until it is done.
                let again = id.clone();
                self.again_after_late_work(&id, move |manager| async move {
                    if let Err(err) = manager.delete_now(again.clone(), delete_branches, record_kept).await {
                        crate::logging::private_trace!(manager.tab_chat_logs(&again), warn, "could not finish deleting a conversation"; conversation = %again, error = %err);
                    }
                });
                return Err(Error::Invalid(
                    "Its work is still going: it is deleted once that ends.".into(),
                ));
            }
            // Deleted, or still there and working: either way its fence has no more to hold.
            if deleted.is_ok()
                || self
                    .core
                    .conversation(&id)
                    .is_ok_and(|now| now.lifecycle != Lifecycle::Archived && !now.deleting)
            {
                self.open_fence(&id);
            }
            deleted.map(|_| ())
        })
    }

    /// Deletes a conversation once nothing new of it starts. It leaves the catalog last, after
    /// its streams are purged, so one cut off at any step is still there to finish. `false`
    /// when work that passed its fence is still going: everything it ran has stopped, and the
    /// rest waits until that work has ended, so nothing it makes then is left behind.
    async fn delete_closed(
        &self,
        conversation: Conversation,
        delete_branches: bool,
        record_kept: bool,
    ) -> Result<bool> {
        let id = conversation.id.clone();
        let private_logs = conversation.tab_chat.is_some();
        // What its browser tab sent goes now, however it is deleted (its tab closed, New chat).
        self.tab_chats.forget_page(&id);
        let drained = self.drain(&id).await;
        self.delete_side_chats(&id).await;
        self.wind_down_for(&conversation, true).await;
        if !drained {
            crate::logging::private_trace!(self.tab_chat_logs(&id), warn, "work of a deleted conversation is still going; deleting it once that ends"; conversation = %id);
            return Ok(false);
        }
        let tasks = self.core.tasks(&id).await.unwrap_or_default();
        self.forget_worker_rows(&id, &tasks);
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
                && self
                    .runtime
                    .ledger()
                    .branch_ownership()
                    .owns(Path::new(repo), branch)
            {
                branches.push(branch.clone());
            }
            let (git, repo) = (self.git.clone(), PathBuf::from(repo));
            for branch in &branches {
                self.forget_branch(&repo, branch);
            }
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
        // A Chat's workers branched in the repositories they were started in (S28).
        if delete_branches && matches!(conversation.setup, Some(Setup::Chat { .. })) {
            for task in &tasks {
                let (Some(repo), Some(branch)) = (
                    task.plain.as_ref().and_then(|plain| plain.repo.clone()),
                    task.workspace.as_ref().and_then(|w| w.branch.clone()),
                ) else {
                    continue;
                };
                if !Path::new(&repo).exists() {
                    continue;
                }
                let (git, repo) = (self.git.clone(), PathBuf::from(repo));
                self.forget_branch(&repo, &branch);
                let _lane = self.closing.lane.lock().await;
                blocking(move || {
                    let repo = git.open(&repo).map_err(git_error)?;
                    if repo.branch_tip(&branch).map_err(git_error)?.is_some()
                        && let Err(err) = repo.delete_branch(&branch, true)
                    {
                        tracing::warn!(branch, error = %err, "could not delete a branch");
                    }
                    Ok(())
                })
                .await?;
            }
        }
        if !delete_branches && record_kept {
            self.record_left_branches(super::project_removal::branch_records(
                &self.runtime.ledger().branch_ownership(),
                &conversation,
                &tasks,
            ))
            .await;
        }
        let mut purge = vec![
            streams::conversation(&id),
            streams::orchestrator(&id),
            streams::inbox(&id),
            streams::draft(&id.to_string()),
        ];
        purge.extend(tasks.iter().map(|task| streams::task(&task.id)));
        self.forget_brain_conversation(&id, conversation.project_id.clone())
            .await;
        self.forget_routing(&id, &tasks).await;
        // Its CLI session files go, then its folder with its records (S7, S8): Archive keeps
        // both.
        let lineage = crate::ledger::lineage_owner(&id.0);
        let leftovers = self.runtime.ledger().dispose(&lineage).await;
        if !leftovers.is_clean() {
            crate::logging::private_trace!(private_logs, warn, "some of its CLI session files will be retried at the next launch"; conversation = %id, ?leftovers);
        }
        for task in &tasks {
            self.forget_brain_conversation(
                &ConversationId(task.id.0.clone()),
                conversation.project_id.clone(),
            )
            .await;
            self.delete_turn_snapshots(&ConversationId(task.id.0.clone()))
                .await?;
        }
        self.remove_session_dir(&id).await;
        let store = self.core.store().clone();
        // Its own blobs go now, not a day later; then the usual collection of older leftovers.
        self.delete_turn_snapshots(&id).await?;
        self.delete_thread_images(&id).await?;
        // What background runs of an older Brigadier printed.
        store
            .delete_stream_prefixes(vec![format!("background-output:{id}:")])
            .await?;
        let (removed, own) = store.delete_streams_and_blobs(purge).await?;
        self.core.forget_conversation(id.clone()).await?;
        self.convs_lock().remove(&id);
        match store.gc_blobs().await {
            Ok(stats) => {
                crate::logging::private_trace!(private_logs, info, "conversation deleted"; conversation = %id, removed, ?own, ?stats)
            }
            Err(err) => {
                crate::logging::private_trace!(private_logs, warn, "could not collect blobs"; conversation = %id, error = %err)
            }
        }
        if conversation.tab_chat.is_some() {
            self.scrub_databases().await;
        }
        Ok(true)
    }
}

impl SessionManager {
    /// The routing store lets go of a deleted conversation: its turns and its tasks' outcomes.
    async fn forget_routing(&self, id: &ConversationId, tasks: &[Task]) {
        let Some(store) = self.runtime.routing_store().cloned() else {
            return;
        };
        let task_ids = tasks.iter().map(|task| task.id.0.clone()).collect();
        match store.forget_conversation(id.0.clone(), task_ids).await {
            Ok((turns, outcomes)) => {
                crate::logging::private_trace!(self.tab_chat_logs(id), info, "routing forgot a deleted conversation"; conversation = %id, turns, outcomes)
            }
            Err(err) => {
                crate::logging::private_trace!(self.tab_chat_logs(id), warn, "could not forget a deleted conversation's routing records"; conversation = %id, error = %err)
            }
        }
    }
}

impl SessionManager {
    /// A tab's chat leaves nothing, not even in the database files: their WAL is emptied, and
    /// with `secure_delete` on, the deleted rows' pages hold no copy either.
    pub(crate) async fn scrub_databases(&self) {
        if self.core.store().scrub().await.is_err() {
            tracing::warn!("could not empty the store's write-ahead log after a tab chat");
        }
        if let Some(routing) = self.runtime.routing_store()
            && routing.scrub().await.is_err()
        {
            tracing::warn!("could not empty the routing write-ahead log after a tab chat");
        }
    }
}

/// Removes a deleted session's branch namespace or a Local checkout file it made, for the
/// cleanup ledger ([`crate::ledger::GitRemover`]): `Ok(Some(why))` when it stays on purpose.
fn remove_git_leftover(
    git: &brigadier_git::Git,
    artifact: Artifact,
) -> std::result::Result<Option<String>, String> {
    let (repo, what) = match &artifact {
        Artifact::GitBranches { repo, .. } | Artifact::CheckoutFile { repo, .. } => {
            (PathBuf::from(repo), &artifact)
        }
        _ => return Err("not a git leftover".into()),
    };
    let repo = match git.open(&repo) {
        Ok(repo) => repo,
        // The repository itself is gone (moved or removed by the user): nothing is left in it.
        Err(_) if !repo.exists() => return Ok(None),
        Err(err) => return Err(err.to_string()),
    };
    match what {
        Artifact::GitBranches { namespace, .. } => {
            let mut removal = repo
                .remove_namespace(namespace)
                .map_err(|err| err.to_string())?;
            if !removal.checked_out.is_empty() {
                // A worktree whose folder went with the session's (one its CLI made there)
                // still holds its branch in git's records: those records go, then the branch.
                let worktrees = repo.worktrees().map_err(|err| err.to_string())?;
                let gone = worktrees.iter().any(|worktree| {
                    worktree
                        .branch
                        .as_ref()
                        .is_some_and(|branch| removal.checked_out.contains(branch))
                        && !worktree.path.exists()
                });
                if gone {
                    repo.prune_worktrees().map_err(|err| err.to_string())?;
                    removal = repo
                        .remove_namespace(namespace)
                        .map_err(|err| err.to_string())?;
                }
            }
            if removal.checked_out.is_empty() {
                Ok(None)
            } else {
                Ok(Some(format!(
                    "{} stays: a worktree has it out",
                    removal.checked_out.join(", ")
                )))
            }
        }
        Artifact::CheckoutFile { path, blob, .. } => {
            let removed = repo
                .remove_untracked_if(path, &brigadier_git::Oid(blob.clone()))
                .map_err(|err| err.to_string())?;
            if removed || !repo.root().join(path).exists() {
                Ok(None)
            } else {
                Ok(Some(format!("{path} stays: it changed since the session made it")))
            }
        }
        _ => Err("not a git leftover".into()),
    }
}

fn being_deleted() -> Error {
    Error::Invalid("It is being deleted.".into())
}

/// The cleanup-ledger owner and data-dir area of a conversation's CLI session.
pub(super) fn conversation_owner(conversation: &Conversation) -> (String, &'static str) {
    if conversation.tab_chat.is_some() {
        return (
            format!("{}{}", crate::ledger::TAB_CHAT_OWNER, conversation.id),
            "tabchat",
        );
    }
    match conversation.kind {
        ConversationKind::Session => (format!("orch:{}", conversation.id), "orch"),
        ConversationKind::Chat => (format!("chat:{}", conversation.id), "chat"),
    }
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

impl SessionManager {
    /// Merge cards are retired (BUILD-PLAN A4). After an upgrade, a session's open merge card
    /// is withdrawn and the thread hears of it in one line.
    async fn retire_merge_cards(&self, id: &ConversationId) {
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        let open: Vec<_> = board
            .questions
            .values()
            .filter(|question| {
                matches!(question.kind, crate::work::QuestionKind::Merge { .. })
                    && question.is_open()
            })
            .cloned()
            .collect();
        if open.is_empty() {
            return;
        }
        for question in &open {
            self.withdraw_question(question).await;
        }
        self.queue_envelope(
            id,
            Envelope {
                kind: InjectionKind::Decision,
                label: "merge cards retired".into(),
                task_id: None,
                text: "[merge cards retired] Brigadier no longer asks about merges on cards, so the open merge card is closed. Merge when the user's words ask for it.".into(),
            },
            None,
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder in the temp directory, removed when dropped however the test ends.
    struct Temp(PathBuf);

    impl std::ops::Deref for Temp {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A test data folder in the temp directory, named for a new task, with a file in it,
    /// all last changed `then`.
    fn test_folder(then: SystemTime) -> Temp {
        let folder = test_data_dir(&crate::work::TaskId::generate());
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("data.txt");
        std::fs::write(&file, "test data").unwrap();
        for path in [&file, &folder] {
            set_changed(path, then);
        }
        Temp(folder)
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
        // The sweep runs two days from now, so the folders made now are old by then while
        // the sweeps of tests running alongside, today, still see them all as recent.
        let day = Duration::from_secs(24 * 60 * 60);
        let now = SystemTime::now();
        let later = now + 2 * day;
        let minute_before = later - Duration::from_secs(60);
        let ended = test_folder(later);
        let live = test_folder(now);
        let recent = test_folder(minute_before);
        let stale = test_folder(now);
        let claimed = test_folder(now);
        let held_after_end = test_folder(later);
        // Another data folder's task wrote into its folder a minute before.
        let in_use = test_folder(now);
        set_changed(&in_use.join("data.txt"), minute_before);
        let tasks = HashMap::from([
            (ended.to_path_buf(), true),
            (live.to_path_buf(), false),
            (held_after_end.to_path_buf(), true),
        ]);
        let held = HashSet::from([claimed.to_path_buf(), held_after_end.to_path_buf()]);
        let folders = vec![
            ended.to_path_buf(),
            live.to_path_buf(),
            recent.to_path_buf(),
            stale.to_path_buf(),
            claimed.to_path_buf(),
            held_after_end.to_path_buf(),
            in_use.to_path_buf(),
        ];
        let removed = sweep_test_folders(folders, &tasks, &held, later - day);
        assert_eq!(removed, 2);
        assert!(!ended.exists());
        assert!(!stale.exists());
        for kept in [&live, &recent, &claimed, &held_after_end, &in_use] {
            assert!(kept.exists(), "{}", kept.display());
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
}
