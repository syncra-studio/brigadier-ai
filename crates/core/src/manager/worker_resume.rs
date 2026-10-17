//! A worker whose CLI ends before it reports resumes itself (SESSION-FIXES-PLAN.md Q4): the
//! same CLI session, model, account and worktree, told it was interrupted. Another model takes
//! a task over only for a limit, a logout or an outage (`fallback`), never for a crash. A task
//! that dies again before it reports stops, and the thread hears both exits. An overnight
//! run's state comes first: a run ending inside its allowance resumes the worker only to hand
//! off, and past its cutoff nothing resumes.

use std::sync::Arc;

use super::SessionManager;
use super::conversation::Envelope;
use super::overnight::{Interrupted, interrupted, interrupted_hand_off};
use super::worker_handoff::Handover;
use super::workers::{ASKED_THE_ORCHESTRATOR, ASKED_THE_THREAD, TaskLive};
use crate::now_ms;
use crate::overnight::OvernightRun;
use crate::work::{InjectionKind, Task, TaskState};
use crate::{Error, Result};
use brigadier_providers::{Origin, TurnInput};

/// What a worker resumed after its process ended hears first.
pub(crate) const INTERRUPTED: &str = "[Brigadier] You were interrupted (your process ended). Check what was running (its output may be lost), then continue the task.";

/// How a CLI ended, in the words the thread and the task keep: "signal 9", "exit code 1",
/// with the last line it wrote to stderr.
pub(crate) fn exit_words(code: Option<i32>, signal: Option<i32>, stderr: Option<&str>) -> String {
    let how = match (signal, code) {
        (Some(signal), _) => format!("signal {signal}"),
        (None, Some(code)) => format!("exit code {code}"),
        (None, None) => "ended".to_owned(),
    };
    match stderr
        .and_then(|tail| tail.lines().rev().find(|line| !line.trim().is_empty()))
        .map(str::trim)
    {
        Some(line) => format!("{how}: {line}"),
        None => how,
    }
}

impl SessionManager {
    /// The overnight run `task` belongs to, as it is now; `None` for a task of no run (and
    /// for one whose run is gone, which [`interrupted`] reads as ended).
    async fn run_of_task(&self, task: &Task) -> Option<Option<OvernightRun>> {
        let context = task.run.as_ref()?;
        let board = self.core.board(&task.conversation_id).await.ok();
        Some(board.and_then(|board| board.runs.get(&context.run_id).cloned()))
    }

    /// What may become of `task`, interrupted now.
    pub(crate) async fn interrupted_verdict(&self, task: &Task) -> Interrupted {
        match self.run_of_task(task).await {
            None => Interrupted::Resume,
            Some(run) => interrupted(run.as_ref(), now_ms()),
        }
    }

    /// The worker's CLI ended with no report and no cut-off (`exit`: how it ended). `from` is
    /// the CLI session it ended in ([`TaskLive::generation`]). Runs outside the worker's event
    /// pump. Boxed: the worker it starts can come back here, and a recursive future must name
    /// its `Send` bound.
    pub(crate) fn worker_died<'a>(
        &'a self,
        live: &'a Arc<TaskLive>,
        exit: String,
        from: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(self.worker_died_now(live, exit, from))
    }

    async fn worker_died_now(&self, live: &Arc<TaskLive>, exit: String, from: u64) {
        let _handing = live.reroute.lock().await;
        if live.generation().await != from {
            return;
        }
        let Ok(task) = self.task_by_id(&live.conversation_id, &live.id).await else {
            return;
        };
        if task.state.is_final() || task.state == TaskState::TakenOver || task.report.is_some() {
            return;
        }
        tracing::warn!(task = %task.id, exit = %exit, "a worker's CLI ended before it reported");
        if let Some(first) = task.deaths.last().cloned() {
            self.died_twice(&task, &first, &exit).await;
            return;
        }
        let Ok(task) = self
            .update_task(&task.conversation_id, &task.id, |t| {
                t.deaths.push(exit.clone())
            })
            .await
        else {
            return;
        };
        let note = match self.interrupted_verdict(&task).await {
            Interrupted::Resume => INTERRUPTED.to_owned(),
            Interrupted::HandOff { reason } => interrupted_hand_off(&reason),
            Interrupted::Keep { reason } => {
                self.keep_interrupted(&task, &reason).await;
                return;
            }
        };
        if let Err(err) = self.resume_interrupted(live, &task, note).await {
            // Its session couldn't be resumed: a fresh session of the same model carries on
            // from the handoff files, which still beats another vendor.
            tracing::warn!(task = %task.id, error = %err, "could not resume an interrupted worker");
            let Ok(task) = self.task_by_id(&task.conversation_id, &task.id).await else {
                return;
            };
            if task.state.is_final() {
                return;
            }
            if let Err(err) = self
                .hand_over_held(live, &task, from, Handover::Interrupted)
                .await
            {
                let reason = format!("The task could not continue after its CLI ended: {err}");
                self.worker_failed(&task, &reason).await;
            }
        }
    }

    /// Starts `task`'s CLI session again after it ended without a report, with `note` first:
    /// the same session, model, account and worktree. Its row reads "Interrupted — resuming"
    /// until the session runs.
    pub(crate) async fn resume_interrupted(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        note: String,
    ) -> Result<()> {
        let Some(native_id) = self.last_worker_native_id(task).await else {
            return Err(Error::Invalid(format!(
                "task-{} has no CLI session to resume",
                task.number
            )));
        };
        let subject = match &task.subject {
            Some(id) => self.task_by_id(&task.conversation_id, id).await.ok(),
            None => None,
        };
        let task = self
            .update_task(&task.conversation_id, &task.id, |t| {
                t.resuming = true;
                t.state = TaskState::Running;
                t.blocked_reason = None;
            })
            .await?;
        live.close_cli().await;
        let started = self
            .launch_worker(
                live,
                &task,
                subject.as_ref(),
                Origin::Resume { native_id },
                TurnInput::text(note),
            )
            .await;
        let _ = self
            .update_task(&task.conversation_id, &task.id, |t| t.resuming = false)
            .await;
        started
    }

    /// The second death before a report: the task stops where it is (its worktree and session
    /// stay, for `message_worker` once the cause is fixed) and the thread hears both exits.
    async fn died_twice(&self, task: &Task, first: &str, second: &str) {
        let reason = format!("Its CLI ended twice before it reported: {first}; then {second}.");
        let Ok(task) = self
            .update_task(&task.conversation_id, &task.id, |t| {
                t.deaths.push(second.to_owned());
                t.state = TaskState::Blocked;
                t.blocked_reason = Some(reason.clone());
            })
            .await
        else {
            return;
        };
        self.release_run_task(&task.id);
        let n = task.number;
        self.deliver(
            &task.conversation_id,
            Envelope {
                kind: InjectionKind::TaskFailed,
                label: format!("task-{n} stopped"),
                task_id: Some(task.id.clone()),
                text: format!(
                    "[task-{n} stopped: died twice] {reason} Its worktree and session are kept. Tell the user what kills it; message_worker resumes it once the cause is fixed, stop_task ends it."
                ),
            },
        )
        .await;
    }

    /// After a restart, a task whose worker ran: by its run's state, it resumes (in the
    /// background), resumes only to hand off, or stops with its work kept.
    pub(crate) async fn resume_after_restart(&self, task: &Task) {
        let note = match self.interrupted_verdict(task).await {
            Interrupted::Resume => INTERRUPTED.to_owned(),
            Interrupted::HandOff { reason } => interrupted_hand_off(&reason),
            Interrupted::Keep { reason } => {
                self.keep_interrupted(task, &reason).await;
                return;
            }
        };
        let live = self.task_live(task);
        let manager = self.arc();
        let task = task.clone();
        self.spawn(async move {
            let resumed = {
                let _handing = live.reroute.lock().await;
                // A message (the run's hand-off request) revived it first.
                if live.cli().await.is_some() {
                    return;
                }
                manager.resume_interrupted(&live, &task, note).await
            };
            if let Err(err) = resumed {
                // Its CLI's files are gone, say: a fresh session of the same model carries on
                // from the whole hand-off rather than the task stopping (BUILD-PLAN D2.4).
                tracing::warn!(task = %task.id, error = %err, "could not resume a worker after a restart; handing it over");
                let handed = async {
                    let _handing = live.reroute.lock().await;
                    let task = manager.task_by_id(&task.conversation_id, &task.id).await?;
                    if task.state.is_final() || live.cli().await.is_some() {
                        return Ok(());
                    }
                    let from = live.generation().await;
                    manager
                        .hand_over_held(&live, &task, from, Handover::Restarted)
                        .await
                }
                .await;
                if let Err(handed) = handed {
                    let reason = format!("Brigadier quit while this task was running, and its session couldn't be resumed: {err}; nor could it continue in a fresh session: {handed}");
                    let _ = manager
                        .update_task(&task.conversation_id, &task.id, |t| t.error = Some(reason))
                        .await;
                    manager.dispose_task(&task, TaskState::Stopped).await;
                }
            }
        });
    }

    /// After a restart, a task whose worker waited, or that the user paused, when Brigadier
    /// stopped (BUILD-PLAN D1.4): no CLI runs, but its worktree and session stay, and it
    /// carries on from its wait. `active`: its conversation runs (not hibernated), so a
    /// worker whose wait ended with Brigadier resumes now; otherwise it waits for a message.
    pub(crate) async fn recover_waiting(&self, task: &Task, active: bool) {
        let live = self.task_live(task);
        match task.state {
            // The user's resume starts its session again (`resume_task`).
            TaskState::Paused => {}
            // The go-ahead resumes its session (`go_ahead` sends it as a message).
            TaskState::Blocked if Self::waits_for_go_ahead(task) => {}
            TaskState::Blocked => {
                let asked = task.blocked_reason.as_deref().and_then(|why| {
                    why.strip_prefix(ASKED_THE_THREAD)
                        .or_else(|| why.strip_prefix(ASKED_THE_ORCHESTRATOR))
                });
                match asked {
                    // Its call ended with its CLI: the thread hears the question again, and
                    // its answer resumes the session.
                    Some(question) => self.ask_again(&live, task, question).await,
                    // What it waited on (its review, an approval card, a free worker) ended
                    // with Brigadier: it resumes and looks again.
                    None if active => self.resume_after_restart(task).await,
                    None => {}
                }
            }
            _ => {}
        }
    }

    /// An interrupted run task that may not resume (its run is past its cutoff, stopped or
    /// over): it stops with the run's reason, its work kept with it for the report.
    pub(crate) async fn keep_interrupted(&self, task: &Task, reason: &str) {
        let _ = self
            .update_task(&task.conversation_id, &task.id, |t| {
                t.error = Some(format!(
                    "Interrupted, and not resumed: {reason}. Its work is kept."
                ));
            })
            .await;
        self.dispose_task(task, TaskState::Stopped).await;
        self.release_run_task(&task.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exit_is_named_by_its_signal_or_code_and_last_stderr_line() {
        assert_eq!(exit_words(None, Some(9), None), "signal 9");
        assert_eq!(
            exit_words(Some(1), None, Some("first\nBroken pipe\n\n")),
            "exit code 1: Broken pipe"
        );
        assert_eq!(exit_words(None, None, Some("  ")), "ended");
    }
}
