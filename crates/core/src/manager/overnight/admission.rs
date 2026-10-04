//! Worker admission for overnight runs (PLAN.md §10.7).
//!
//! "max N workers" caps how many of a run's tasks execute at once: a run task takes a slot
//! when a turn of its worker starts (a new session, a resumed or restarted one, a fresh
//! session after a hand-off or fallback, a message or Continue to an idle worker) and gives
//! it back when the turn ends without another following, or the session ends. A worker that
//! reported and waits for its checks holds none, so its checks can run under max 1. Queued
//! tasks, the orchestrator and a worker's own sub-agents hold none. Sessions without a run
//! are never capped.
//!
//! Waiting tasks are admitted checks first: while a run's verifier waits for a slot, no
//! other task of that run takes one, and while a reviewer waits, no worker does. A change's
//! checks so never wait behind new work, and the verifier, which proves it, goes first.
//!
//! Checks build and test: a run's verifier also takes the daemon-wide build lease, so one
//! such check builds at a time, and every run worker runs at low OS priority (the spawn's
//! `low_priority`), so its builds yield to the user's own work.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

use super::super::SessionManager;
use crate::model::{OvernightRunId, TaskId};
use crate::work::{Task, TaskKind};
use crate::{Error, Result};

/// How long a waiting task sleeps between looks when no slot is freed meanwhile (a lowered
/// cap or an ended run is noticed this way too).
const RECHECK: Duration = Duration::from_secs(20);

pub(crate) struct Admission {
    /// The tasks of each run executing now.
    held: std::sync::Mutex<HashMap<OvernightRunId, HashSet<TaskId>>>,
    /// The tasks of each run waiting for a slot, with their [`rank`].
    waiting: std::sync::Mutex<HashMap<OvernightRunId, HashMap<TaskId, u8>>>,
    /// Signalled whenever a slot or the build lease is given back.
    freed: Notify,
    build: Arc<Semaphore>,
    /// The task holding the build lease, with it.
    building: std::sync::Mutex<Option<(TaskId, OwnedSemaphorePermit)>>,
}

impl Default for Admission {
    fn default() -> Self {
        Self {
            held: Default::default(),
            waiting: Default::default(),
            freed: Notify::new(),
            build: Arc::new(Semaphore::new(1)),
            building: Default::default(),
        }
    }
}

/// Whether a run task may execute now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    /// Not a run task, or it holds a slot now.
    Admitted,
    /// Its run has this many executing already.
    Full { cap: u32 },
}

impl Admission {
    fn held(&self) -> std::sync::MutexGuard<'_, HashMap<OvernightRunId, HashSet<TaskId>>> {
        self.held.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn waiting(&self) -> std::sync::MutexGuard<'_, HashMap<OvernightRunId, HashMap<TaskId, u8>>> {
        self.waiting.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Which waiting task of a run takes a freed slot first: a verifier (0), then a reviewer (1),
/// then any other work (2).
fn rank(task: &Task) -> u8 {
    match task.kind {
        TaskKind::Verify => 0,
        TaskKind::Review => 1,
        _ => 2,
    }
}

/// Whether a task of `rank` gives way to another task waiting for a slot of its run.
fn outranked(waiting: Option<&HashMap<TaskId, u8>>, task_id: &TaskId, rank: u8) -> bool {
    waiting.is_some_and(|waiting| {
        waiting
            .iter()
            .any(|(other, other_rank)| other != task_id && *other_rank < rank)
    })
}

/// A task's place among its run's waiting tasks, given up when its wait ends however it ends.
struct Queued<'a> {
    admission: &'a Admission,
    run: OvernightRunId,
    task: TaskId,
}

impl Drop for Queued<'_> {
    fn drop(&mut self) {
        let mut waiting = self.admission.waiting();
        if let Some(tasks) = waiting.get_mut(&self.run) {
            tasks.remove(&self.task);
            if tasks.is_empty() {
                waiting.remove(&self.run);
            }
        }
        drop(waiting);
        // A task that gave way to this one may take the slot now.
        self.admission.freed.notify_waiters();
    }
}

impl SessionManager {
    /// Takes a slot for `task` if its run has one free (or it holds one already). A task of
    /// a run that is over may not start.
    pub(crate) fn try_admit_run_task(&self, task: &Task) -> Result<Slot> {
        let Some(context) = &task.run else {
            return Ok(Slot::Admitted);
        };
        let active = self
            .overnight
            .active
            .get(&task.conversation_id)
            .filter(|active| active.id == context.run_id)
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "task-{} worked for an overnight run that has ended; nothing more of it starts.",
                    task.number
                ))
            })?;
        let mut held = self.overnight.admission.held();
        let tasks = held.entry(context.run_id.clone()).or_default();
        if tasks.contains(&task.id) {
            return Ok(Slot::Admitted);
        }
        let gives_way = || {
            outranked(
                self.overnight.admission.waiting().get(&context.run_id),
                &task.id,
                rank(task),
            )
        };
        match active.max_workers {
            Some(cap) if tasks.len() >= cap as usize || gives_way() => Ok(Slot::Full { cap }),
            _ => {
                tasks.insert(task.id.clone());
                Ok(Slot::Admitted)
            }
        }
    }

    /// Waits until `task` may execute (a slot of its run, and the build lease for a run's
    /// verifier), showing why it waits meanwhile. Fails when the task ends or its run is over
    /// while it waits.
    pub(crate) async fn admit_run_task(&self, task: &Task) -> Result<()> {
        self.admit_to_run(task, false).await
    }

    /// [`Self::admit_run_task`] for a worker session that starts anew (a task's first
    /// session, a restart, a fallback successor): once its run winds down, none starts, even
    /// one that was already waiting for a slot. A worker that is already going may still take
    /// a turn to hand off.
    pub(crate) async fn admit_new_run_task(&self, task: &Task) -> Result<()> {
        self.admit_to_run(task, true).await
    }

    async fn admit_to_run(&self, task: &Task, fresh: bool) -> Result<()> {
        if task.run.is_none() {
            return Ok(());
        }
        let mut waited = false;
        let mut queued = None;
        loop {
            if fresh && self.run_winding_down(task) {
                if waited {
                    self.set_task_blocked(&task.id, None).await;
                }
                return Err(Error::Invalid(format!(
                    "task-{} doesn't start: its overnight run is ending.",
                    task.number
                )));
            }
            let freed = self.overnight.admission.freed.notified();
            match self.try_admit_run_task(task)? {
                Slot::Admitted => break,
                Slot::Full { cap } => {
                    if queued.is_none()
                        && let Some(context) = &task.run
                    {
                        self.overnight
                            .admission
                            .waiting()
                            .entry(context.run_id.clone())
                            .or_default()
                            .insert(task.id.clone(), rank(task));
                        queued = Some(Queued {
                            admission: &self.overnight.admission,
                            run: context.run_id.clone(),
                            task: task.id.clone(),
                        });
                    }
                    if !waited {
                        waited = true;
                        self.set_task_blocked(
                            &task.id,
                            Some(format!(
                                "Waiting for a free worker: the run works with at most {cap} at once."
                            )),
                        )
                        .await;
                    }
                    let _ = tokio::time::timeout(RECHECK, freed).await;
                    self.still_wanted(task).await?;
                }
            }
        }
        drop(queued);
        if task.kind == TaskKind::Verify && !self.holds_build_lease(&task.id) {
            let lease = self.overnight.admission.build.clone();
            let permit = match lease.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    waited = true;
                    self.set_task_blocked(
                        &task.id,
                        Some("Waiting for another check's build and tests to finish.".into()),
                    )
                    .await;
                    loop {
                        if let Ok(Ok(permit)) =
                            tokio::time::timeout(RECHECK, lease.clone().acquire_owned()).await
                        {
                            break permit;
                        }
                        if let Err(err) = self.still_wanted(task).await {
                            self.release_run_task(&task.id);
                            return Err(err);
                        }
                    }
                }
            };
            *self
                .overnight
                .admission
                .building
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = Some((task.id.clone(), permit));
        }
        if waited {
            self.set_task_blocked(&task.id, None).await;
        }
        Ok(())
    }

    /// Gives back what `task_id` held: its run's slot and the build lease.
    pub(crate) fn release_run_task(&self, task_id: &TaskId) {
        let admission = &self.overnight.admission;
        let mut released = false;
        for tasks in admission.held().values_mut() {
            released |= tasks.remove(task_id);
        }
        {
            let mut building = admission.building.lock().unwrap_or_else(|p| p.into_inner());
            if building.as_ref().is_some_and(|(id, _)| id == task_id) {
                *building = None;
                released = true;
            }
        }
        if released {
            admission.freed.notify_waiters();
        }
    }

    /// Gives back a run task's slot when no turn of its worker runs (a call that took it
    /// started none).
    pub(crate) async fn release_if_idle(&self, task: &Task) {
        if task.run.is_none() {
            return;
        }
        let busy = match self.existing_task_live(&task.id) {
            Some(live) => live.busy().await,
            None => false,
        };
        if !busy {
            self.release_run_task(&task.id);
        }
    }

    /// Whether `task`'s run is ending (Stop, the deadline, a block): nothing new starts.
    fn run_winding_down(&self, task: &Task) -> bool {
        task.run.as_ref().is_some_and(|context| {
            self.overnight
                .active
                .get(&task.conversation_id)
                .is_some_and(|active| active.id == context.run_id && active.winding_down)
        })
    }

    fn holds_build_lease(&self, task_id: &TaskId) -> bool {
        self.overnight
            .admission
            .building
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|(id, _)| id == task_id)
    }

    /// Fails when the task waiting for a slot ended meanwhile (stopped), or its run is over.
    async fn still_wanted(&self, task: &Task) -> Result<()> {
        let now = self.task_by_id(&task.conversation_id, &task.id).await?;
        if now.state.is_final() {
            return Err(Error::Invalid(format!("task-{} has ended", task.number)));
        }
        let over = self
            .overnight
            .active
            .get(&task.conversation_id)
            .is_none_or(|active| task.run.as_ref().is_none_or(|run| run.run_id != active.id));
        if over {
            return Err(Error::Invalid(format!(
                "task-{} worked for an overnight run that has ended; nothing more of it starts.",
                task.number
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_waiting_check_goes_before_new_work_and_the_verifier_first() {
        let (verifier, reviewer, worker) =
            (TaskId::generate(), TaskId::generate(), TaskId::generate());
        let mut waiting = HashMap::new();
        assert!(!outranked(None, &worker, 2));
        waiting.insert(reviewer.clone(), 1);
        assert!(outranked(Some(&waiting), &worker, 2));
        assert!(!outranked(Some(&waiting), &reviewer, 1));
        waiting.insert(verifier.clone(), 0);
        assert!(outranked(Some(&waiting), &reviewer, 1));
        assert!(!outranked(Some(&waiting), &verifier, 0));
        // Two of a rank never block each other.
        let other = TaskId::generate();
        waiting.insert(other.clone(), 0);
        assert!(!outranked(Some(&waiting), &verifier, 0));
    }
}
