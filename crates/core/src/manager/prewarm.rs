//! The pre-warmed worker (THREAD-PLAN.md Q8 lever 6): when the user writes in a session, the
//! daemon makes the next writing task's worktree in the background (checked out at the base a
//! worker would get, with its dependency installs and build caches copied in), so the
//! `delegate_task` that usually follows starts its worker at once instead of after the copy.
//!
//! The pre-warm is made for a task id reserved up front and recorded under that task's own
//! cleanup owner (`task:<id>`) from the first artifact, so taking it over moves nothing: the
//! task created with that id simply finds its worktree. A writing task with no subject and no
//! overnight run takes it when the session's permission level hasn't changed; its worker uses
//! it when the base it would get now holds the same files, after putting the checkout on the
//! task's branch. Otherwise it is removed and the worker starts as usual.
//!
//! One pre-warm per session at most. It is never made while the machine is strained, during an
//! overnight run, or when making it would ask the user about uncommitted changes, and it is
//! removed after [`PREWARM_TTL`], when the user stops the session, when the session hibernates,
//! is archived or deleted, and when another one replaces it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use brigadier_git::{Oid, WorktreeSpec};
use tokio_util::sync::CancellationToken;

use super::workers::Workspace;
use super::{SessionManager, blocking, git_error};
use crate::model::{ConversationId, PermissionLevel, Setup, TaskId};
use crate::work::{Task, TaskKind};
use crate::{Error, Result};

/// How long an unused pre-warm is kept.
#[cfg(not(test))]
pub(crate) const PREWARM_TTL: Duration = Duration::from_secs(10 * 60);
#[cfg(test)]
pub(crate) const PREWARM_TTL: Duration = Duration::from_secs(4);

/// The sessions' pre-warms: the one each session may still use, and those a new task took and
/// its worker hasn't started from yet.
#[derive(Default)]
pub(crate) struct Prewarms {
    inner: std::sync::Mutex<PrewarmState>,
}

#[derive(Default)]
struct PrewarmState {
    open: HashMap<ConversationId, Arc<Slot>>,
    claimed: HashMap<TaskId, Arc<Slot>>,
}

/// One pre-warm. Its preparer holds `ready` until it is done, so whoever takes or removes it
/// waits for that and never races the making of its files.
pub(crate) struct Slot {
    pub task_id: TaskId,
    permission: PermissionLevel,
    made_at: Instant,
    cancel: CancellationToken,
    ready: Arc<tokio::sync::Mutex<Option<Warm>>>,
}

/// What a finished pre-warm made.
#[derive(Debug, Clone)]
pub(crate) struct Warm {
    repo: PathBuf,
    target: String,
    base: Oid,
    base_tree: Oid,
    on_snapshot: bool,
    worktree: PathBuf,
    scratch: PathBuf,
    warmed: Vec<String>,
}

impl Prewarms {
    fn lock(&self) -> std::sync::MutexGuard<'_, PrewarmState> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Whether `task` is a pre-warm's reserved id: its owner is alive though no task has it yet.
    pub(crate) fn owns(&self, task: &str) -> bool {
        let state = self.lock();
        state.open.values().any(|slot| slot.task_id.0 == task)
            || state.claimed.keys().any(|id| id.0 == task)
    }
}

impl SessionManager {
    /// The user wrote in session `id`: makes its pre-warm unless one it can still use exists.
    pub(crate) fn prewarm(&self, id: &ConversationId) {
        let Ok(conversation) = self.core.conversation(id) else {
            return;
        };
        if !matches!(conversation.setup, Some(Setup::Session { .. })) {
            return;
        }
        if self.overnight.active.get(id).is_some() || self.machine_strained() {
            return;
        }
        let permission = self.permission(id);
        let stale = {
            let mut state = self.prewarms.lock();
            match state.open.get(id) {
                Some(slot)
                    if slot.permission == permission
                        && !slot.cancel.is_cancelled()
                        && slot.made_at.elapsed() < PREWARM_TTL / 2 =>
                {
                    return;
                }
                _ => state.open.remove(id),
            }
        };
        if let Some(stale) = stale {
            self.drop_slot(stale, "replaced");
        }
        let slot = Arc::new(Slot {
            task_id: TaskId::generate(),
            permission,
            made_at: Instant::now(),
            cancel: CancellationToken::new(),
            ready: Arc::default(),
        });
        // Held by the preparer from before anyone can see the slot.
        let Ok(guard) = slot.ready.clone().try_lock_owned() else {
            return;
        };
        self.prewarms.lock().open.insert(id.clone(), slot.clone());
        let manager = self.arc();
        let id = id.clone();
        self.spawn(async move {
            let mut guard = guard;
            // Never cut short mid-step (a worktree being copied would outlive its removal):
            // a cancelled pre-warm stops between steps, and its remover then removes it all.
            let made = manager
                .make_prewarm(&id, &slot.task_id, &slot.cancel)
                .await;
            match made {
                Ok(warm) => {
                    tracing::info!(conversation = %id, task = %slot.task_id, ms = slot.made_at.elapsed().as_millis() as u64, "pre-warmed the next task's worktree");
                    *guard = Some(warm);
                }
                Err(err) => {
                    tracing::info!(conversation = %id, error = %err, "no pre-warmed worktree");
                }
            }
            drop(guard);
            if slot.cancel.is_cancelled() {
                // Its remover waits for the guard and removes what was made.
                return;
            }
            let failed = slot.ready.lock().await.is_none();
            if failed {
                manager.drop_prewarm_if(&id, &slot, "it could not be made");
                return;
            }
            tokio::time::sleep(PREWARM_TTL).await;
            manager.drop_prewarm_if(&id, &slot, "unused for 10 minutes");
        });
    }

    /// The pre-warm a new task of `kind` in session `id` takes, as the task's id: only a
    /// writing task with no subject and no run, made at the session's permission level now.
    pub(crate) fn claim_prewarm(&self, id: &ConversationId, kind: TaskKind) -> Option<TaskId> {
        if !kind.writes() {
            return None;
        }
        let permission = self.permission(id);
        let mut state = self.prewarms.lock();
        let slot = state.open.remove(id)?;
        if slot.permission != permission || slot.cancel.is_cancelled() {
            drop(state);
            self.drop_slot(slot, "the permission level changed");
            return None;
        }
        let task_id = slot.task_id.clone();
        state.claimed.insert(task_id.clone(), slot);
        Some(task_id)
    }

    /// Removes the pre-warm task `id` took when the task itself wasn't made.
    pub(crate) fn release_prewarm(&self, id: &TaskId) {
        let slot = self.prewarms.lock().claimed.remove(id);
        if let Some(slot) = slot {
            self.drop_slot(slot, "its task wasn't made");
        }
    }

    /// The pre-warmed workspace `task` took, when its worker may start from it: the base it
    /// would get now holds the same files. Otherwise what the pre-warm made is removed first,
    /// and `None` sends the worker down the usual path.
    pub(crate) async fn adopt_prewarm(&self, task: &Task) -> Option<Workspace> {
        let slot = self.prewarms.lock().claimed.remove(&task.id)?;
        let warm = slot.ready.lock().await.clone();
        let owner = format!("task:{}", task.id);
        let adopted = match warm {
            Some(warm) => match self.take_over(task, &warm).await {
                Ok(true) => {
                    tracing::info!(task = %task.id, "the worker starts in the pre-warmed worktree");
                    return Some(Workspace {
                        repo: warm.repo,
                        worktree: Some(warm.worktree),
                        branch: Some(super::workers::task_branch(
                            &task.conversation_id,
                            task.number,
                            &task.title,
                        )),
                        base: Some(warm.base),
                        on_snapshot: warm.on_snapshot,
                        target: Some(warm.target),
                        scratch: warm.scratch,
                        warmed: warm.warmed,
                    });
                }
                Ok(false) => "its base moved",
                Err(err) => {
                    tracing::warn!(task = %task.id, error = %err, "could not take the pre-warmed worktree over");
                    "it could not be taken over"
                }
            },
            None => "it wasn't made",
        };
        tracing::info!(task = %task.id, why = adopted, "the worker starts without the pre-warm");
        let leftovers = self.runtime.ledger().dispose(&owner).await;
        if !leftovers.failures.is_empty() {
            tracing::warn!(task = %task.id, failures = ?leftovers.failures, "the unused pre-warm left files");
        }
        None
    }

    /// Removes session `id`'s pre-warm (the user stopped the session, it hibernates or
    /// closes). One a task already took stays the task's.
    pub(crate) fn drop_prewarm(&self, id: &ConversationId, why: &'static str) {
        let slot = self.prewarms.lock().open.remove(id);
        if let Some(slot) = slot {
            self.drop_slot(slot, why);
        }
    }

    /// [`Self::drop_prewarm`] when `slot` is still the session's.
    fn drop_prewarm_if(&self, id: &ConversationId, slot: &Arc<Slot>, why: &'static str) {
        let slot = {
            let mut state = self.prewarms.lock();
            match state.open.get(id) {
                Some(open) if Arc::ptr_eq(open, slot) => state.open.remove(id),
                _ => None,
            }
        };
        if let Some(slot) = slot {
            self.drop_slot(slot, why);
        }
    }

    /// Cancels `slot`'s making, waits for its preparer and removes what it made.
    fn drop_slot(&self, slot: Arc<Slot>, why: &'static str) {
        slot.cancel.cancel();
        let manager = self.arc();
        self.spawn(async move {
            let _made = slot.ready.lock().await;
            let owner = format!("task:{}", slot.task_id);
            let leftovers = manager.runtime.ledger().dispose(&owner).await;
            tracing::info!(task = %slot.task_id, why, failures = leftovers.failures.len(), "removed a pre-warm");
        });
    }

    /// Makes the worktree a new writing task in session `id` would get, for `task_id`.
    async fn make_prewarm(
        &self,
        id: &ConversationId,
        task_id: &TaskId,
        cancel: &CancellationToken,
    ) -> Result<Warm> {
        let go_on = || {
            if cancel.is_cancelled() {
                Err(Error::Invalid("removed while it was made".into()))
            } else {
                Ok(())
            }
        };
        let conversation = self.core.conversation(id)?;
        let Some(Setup::Session {
            repo, environment, ..
        }) = &conversation.setup
        else {
            return Err(Error::Invalid("not a session".into()));
        };
        let owner = format!("task:{task_id}");
        let repo = PathBuf::from(repo);
        let target = self.ensure_target(id, &repo, environment).await?;
        let (base, on_snapshot) = self.worker_base(id, &repo, &target, false).await?;
        go_on()?;
        let scratch = self.owned_dir("scratch", &task_id.0);
        self.prepare_owned_dir(&owner, &scratch).await?;
        let project = conversation
            .project_id
            .as_ref()
            .map(|id| id.0.clone())
            .unwrap_or_else(|| "none".into());
        let worktree = self
            .owned_dir("worktrees", &project)
            .join(format!("task-next-{}", &task_id.0[task_id.0.len() - 8..]));
        go_on()?;
        let warmed = self
            .make_worktree(
                &owner,
                &repo,
                &worktree,
                WorktreeSpec::Detached { at: base.clone() },
                true,
                task_id,
            )
            .await?;
        let (git, repo_path, commit) = (self.git.clone(), repo.clone(), base.0.clone());
        let base_tree = blocking(move || {
            git.open(&repo_path)
                .and_then(|repo| repo.tree_of(&commit))
                .map_err(git_error)
        })
        .await?;
        Ok(Warm {
            repo,
            target,
            base,
            base_tree,
            on_snapshot,
            worktree,
            scratch,
            warmed,
        })
    }

    /// Whether `task`'s worker may start from `warm` (and then puts it on the task's branch):
    /// the base it would get now holds the same files.
    async fn take_over(&self, task: &Task, warm: &Warm) -> Result<bool> {
        let conversation = self.core.conversation(&task.conversation_id)?;
        let Some(Setup::Session { environment, .. }) = &conversation.setup else {
            return Ok(false);
        };
        let target = self
            .ensure_target(&task.conversation_id, &warm.repo, environment)
            .await?;
        if target != warm.target {
            return Ok(false);
        }
        let (base, on_snapshot) = self
            .worker_base(&task.conversation_id, &warm.repo, &target, true)
            .await?;
        if on_snapshot != warm.on_snapshot || (!on_snapshot && base != warm.base) {
            return Ok(false);
        }
        let branch = super::workers::task_branch(&task.conversation_id, task.number, &task.title);
        let (git, repo_path, worktree, at, expected) = (
            self.git.clone(),
            warm.repo.clone(),
            warm.worktree.clone(),
            warm.base.clone(),
            warm.base_tree.clone(),
        );
        blocking(move || {
            // A snapshot of the user's uncommitted changes is a new commit each time: the same
            // files are what counts.
            if on_snapshot
                && git
                    .open(&repo_path)
                    .and_then(|repo| repo.tree_of(&base.0))
                    .map_err(git_error)?
                    != expected
            {
                return Ok(false);
            }
            git.open_worktree(&worktree)
                .and_then(|worktree| worktree.start_branch(&branch, &at))
                .map_err(git_error)?;
            Ok(true)
        })
        .await
    }

    /// Session `id`'s open pre-warm once made: its reserved task id and its worktree.
    #[cfg(test)]
    pub(crate) async fn prewarm_made(&self, id: &ConversationId) -> Option<(TaskId, PathBuf)> {
        let slot = self.prewarms.lock().open.get(id).cloned()?;
        let warm = slot.ready.lock().await.clone()?;
        Some((slot.task_id.clone(), warm.worktree))
    }

    /// Session `id`'s open pre-warm's reserved task id, made or not.
    #[cfg(test)]
    pub(crate) fn prewarm_id(&self, id: &ConversationId) -> Option<TaskId> {
        let state = self.prewarms.lock();
        state.open.get(id).map(|slot| slot.task_id.clone())
    }
}
