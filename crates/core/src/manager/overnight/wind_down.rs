//! The clean ending (PLAN.md §10.9): Stop, the deadline, a dependent block or the plan done.
//!
//! The fence comes first and is durable (the run is `WindingDown`): no new phase, task, fix or
//! retry starts. Live workers are asked to finish their current step and hand off cleanly in
//! their report, with the sections a fresh worker carries on from; reviews already running may
//! finish. Whatever still runs at the cutoff is stopped, its work kept with the task. Phases
//! that weren't settled end as partial, the report is written, and the run's leases go.
//!
//! The deadline is a time the report is ready by: a run with one gets a third of its length
//! (at most 20 minutes) for this, and the last part of that (at most 2 minutes) stays for the
//! report alone. A Stop without a deadline gets a bounded allowance, ending sooner whenever
//! everything settles sooner. The clock is the wall clock, checked every half minute, so a
//! machine that slept past the deadline winds down as soon as it runs again.

use std::time::Duration;

use super::super::SessionManager;
use super::run::run_request;
use crate::model::{ConversationId, DomainEvent, OvernightRunId};
use crate::overnight::{Deadline, OvernightRun, OvernightState, StopReason};
use crate::work::TaskState;
use crate::{Result, now_ms};

/// The longest a Stop without a deadline waits for live work to hand off.
const STOP_ALLOWANCE_MS: i64 = 10 * 60_000;
/// The share kept for the report at the end of a deadline run, at most.
const REPORT_RESERVE_MAX_MS: i64 = 2 * 60_000;
/// How often a winding-down run looks whether its work settled.
const POLL: Duration = Duration::from_secs(5);
/// How long the thread's interrupted turn gets to stop at the run's end.
const QUIESCE_WAIT: Duration = Duration::from_secs(30);
/// How often the deadline clock looks at the wall clock.
pub(crate) const CLOCK: Duration = Duration::from_secs(30);

/// What the thread is told when its run ends at the deadline or a Stop.
pub(super) const ENDING: &str = "[run] The overnight run is ending now ({reason}). Start nothing new. Land the workers' finished work with land_phase and settle the phases you can judge with settle_step; Brigadier stops what still runs shortly. Then write the user's morning answer: what got done, what is left and what waits on them, briefly. Brigadier adds the run's report (commits, reviews, waiting items, usage) after it.";

/// What a live worker is told when its run ends.
const HAND_OFF: &str = "[Brigadier] The overnight run is ending now ({reason}). Start nothing new. Finish only the step you are in the middle of, leave the worktree coherent and commit the finished steps (not broken work). Then call submit_report at once with a handoff a fresh worker with no memory of this session can carry on from, under these headings in the summary: Goal and where it stands; Done (with commit hashes); In progress (exact files and state, and anything uncommitted); Next steps (ordered and concrete); Decisions and approvals already given; Gotchas learned; How to verify (the exact commands and what passing looks like).";

impl SessionManager {
    /// Starts the deadline clock: every half minute, a run past its wind-down instant (by the
    /// wall clock) starts its clean ending, and a heartbeat records that Brigadier was running.
    pub(crate) fn start_overnight_clock(&self) {
        let manager = self.arc();
        self.spawn(async move {
            loop {
                manager.check_deadlines().await;
                manager.beat().await;
                tokio::time::sleep(CLOCK).await;
            }
        });
    }

    async fn check_deadlines(&self) {
        let now = now_ms();
        for (conversation_id, active) in self.overnight.active.all() {
            if active.winding_down {
                if let Ok(board) = self.core.board(&conversation_id).await
                    && let Some(run) = board.runs.get(&active.id)
                    && run.state == OvernightState::Reporting
                {
                    self.wind_down_soon(run);
                }
                continue;
            }
            if active.wind_down_at_ms.is_none_or(|at| now < at) {
                continue;
            }
            if let Err(err) = self.deadline_reached(&conversation_id, &active.id).await {
                tracing::warn!(run = %active.id, error = %err, "could not wind down at the deadline");
            }
        }
    }

    /// The deadline's wind-down instant passed: the run ends cleanly, like a Stop.
    pub(crate) async fn deadline_reached(
        &self,
        conversation_id: &ConversationId,
        run_id: &OvernightRunId,
    ) -> Result<()> {
        let command = format!("deadline-{run_id}");
        self.change_run(conversation_id, run_id, command, |run, _| {
            if matches!(
                run.state,
                OvernightState::Preparing | OvernightState::Running | OvernightState::WaitingQuota
            ) {
                run.state = OvernightState::WindingDown;
                run.stop = Some(StopReason::Deadline);
            }
            Ok(())
        })
        .await
        .map(|run| {
            if run.state == OvernightState::WindingDown {
                self.wind_down_soon(&run);
            }
        })
    }

    /// Starts the run's clean ending in the background.
    pub(crate) fn wind_down_soon(&self, run: &OvernightRun) {
        let (manager, run) = (self.arc(), run.clone());
        self.spawn(async move { manager.wind_down_run(run).await });
    }

    /// The run's clean ending. Runs once per run, whoever asks.
    pub(crate) async fn wind_down_run(&self, run: OvernightRun) {
        {
            let mut winding = self
                .overnight
                .winding
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if !winding.insert(run.id.clone()) {
                return;
            }
        }
        let cutoff = cutoff_ms(&run, now_ms());
        tracing::info!(run = %run.id, stop = ?run.stop, cutoff, "overnight run winding down");
        // Live workers hand off; nothing new starts (the fence is the run's state).
        if let Ok(board) = self.core.board(&run.conversation_id).await {
            for task in board.tasks.values().filter(|task| owned(task, &run)) {
                if task.kind.writes()
                    && matches!(
                        task.state,
                        TaskState::Running | TaskState::Starting | TaskState::Blocked
                    )
                {
                    let words = HAND_OFF.replace("{reason}", &stop_words(run.stop.as_ref()));
                    let _ = self
                        .message_worker(&run.conversation_id, task, words, "Brigadier")
                        .await;
                }
            }
        }
        // The thread lands what passed and writes the morning answer. It heard of an ending it
        // caused itself (`end_run`, settling the step "stop after" names) in that tool's reply.
        if run.state == OvernightState::WindingDown
            && matches!(run.stop, Some(StopReason::Deadline | StopReason::Stopped))
        {
            self.tell_thread(
                &run,
                "run ending",
                ENDING.replace("{reason}", &stop_words(run.stop.as_ref())),
            )
            .await;
        }
        // Until everything of the run settled and the thread said its last, or the cutoff.
        loop {
            let Ok(board) = self.core.board(&run.conversation_id).await else {
                break;
            };
            let thread = self.thread_has_run_work(&run).await;
            let live = thread
                || board
                    .tasks
                    .values()
                    .any(|task| owned(task, &run) && still_works(task));
            if !live || now_ms() >= cutoff {
                break;
            }
            tokio::time::sleep(POLL).await;
        }
        // What still runs or waits stops here; its work stays with the task.
        if let Ok(board) = self.core.board(&run.conversation_id).await {
            let open: Vec<_> = board
                .tasks
                .values()
                .filter(|task| owned(task, &run) && !task.state.is_final())
                .cloned()
                .collect();
            for task in open {
                // A check that already gave its result (the last phase's judge, still ending
                // its turn) ends done.
                if let Err(err) = Box::pin(self.stop_task(task.id.clone())).await {
                    tracing::warn!(task = %task.id, error = %err, "could not stop a run task at wind-down");
                }
                self.release_run_task(&task.id);
            }
        }
        // The thread stops too: its turn and what it runs end, its own session stays to resume.
        // Then what it committed on the run's branch gets its review before the report.
        self.quiesce_thread(&run).await;
        self.scan_run_branch(&run).await;
        // Then the report. A run already reporting (its report was cut off by a restart or
        // failed once) goes straight on to it.
        let reporting = self
            .core
            .board(&run.conversation_id)
            .await
            .ok()
            .and_then(|board| board.runs.get(&run.id).cloned())
            .filter(|now| {
                now.generation == run.generation && now.state == OvernightState::Reporting
            });
        let settled = match reporting {
            Some(now) => Some(now),
            None => {
                self.change_run_if(&run, |now| {
                    if now.state != OvernightState::WindingDown {
                        return None;
                    }
                    now.state = OvernightState::Reporting;
                    Some(())
                })
                .await
            }
        };
        if let Some(reporting) = settled {
            self.write_run_report(&reporting, true).await;
            if let Some(finished) = self
                .change_run_if(&reporting, |now| {
                    if now.state != OvernightState::Reporting || now.report_message_id.is_none() {
                        return None;
                    }
                    now.state = OvernightState::Finished;
                    now.finished_at_ms = Some(now_ms());
                    Some(())
                })
                .await
            {
                self.run_finished(&finished).await;
            }
        }
        self.overnight
            .winding
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&run.id);
    }
}

impl SessionManager {
    /// Whether the thread still works for the run: a turn runs, or a message of the run's
    /// request waits for one.
    async fn thread_has_run_work(&self, run: &OvernightRun) -> bool {
        let Ok(conv) = self.conv(&run.conversation_id) else {
            return false;
        };
        conv.works_for(&run_request(run)).await
    }

    /// Ends the thread's work for the run: its running turn is interrupted, and once it has
    /// stopped its CLI closes, which ends what its shell and `run` still ran. The native
    /// session stays: the next turn resumes it, in the session's checkout.
    pub(crate) async fn quiesce_thread(&self, run: &OvernightRun) {
        let Ok(conv) = self.conv(&run.conversation_id) else {
            return;
        };
        if conv.turn_running().await {
            tracing::info!(run = %run.id, "the run's end interrupts the thread's turn");
            conv.interrupt_turn().await;
            conv.wait_idle(QUIESCE_WAIT).await;
        }
        if let Some(cli) = conv.idle_cli().await {
            conv.retire_cli(&cli).await;
        }
    }

    /// Looks for the thread's own commits on the run's branch once more, so each gets its
    /// review before the report (and before the worktree may go).
    pub(crate) async fn scan_run_branch(&self, run: &OvernightRun) {
        let (Some(workspace), Ok(conversation)) = (
            run.workspace.as_ref(),
            self.core.conversation(&run.conversation_id),
        ) else {
            return;
        };
        let Some(crate::model::Setup::Session { repo, .. }) = &conversation.setup else {
            return;
        };
        // Landings move the recorded tip past their own commits: what is new is the thread's.
        self.scan_thread_branch(
            &run.conversation_id,
            std::path::Path::new(repo),
            &workspace.branch,
            true,
        )
        .await;
    }
}

impl SessionManager {
    /// The session is being archived or deleted: its runs end before anything of it stops or
    /// goes. The fence is durable (an active run is `WindingDown`, a proposal is dropped), so
    /// no new phase, task, fix or retry starts and nothing new is admitted, and the deadline
    /// clock leaves the ending to [`Self::close_runs`]. The runs it fenced.
    pub(crate) async fn fence_runs(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<Vec<OvernightRun>> {
        let _held = self.overnight.changes.lock().await;
        let Ok(board) = self.core.board(conversation_id).await else {
            return Ok(Vec::new());
        };
        let mut fenced = Vec::new();
        let mut events = Vec::new();
        for run in board.runs.values() {
            let mut now = run.clone();
            if fence(&mut now) {
                events.push(DomainEvent::OvernightUpdated {
                    run: Box::new(now.clone()),
                });
            }
            if now.state.is_active() {
                fenced.push(now);
            }
        }
        if !events.is_empty() {
            self.record_runs(conversation_id, events).await?;
        }
        let mut winding = self
            .overnight
            .winding
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        for run in &fenced {
            winding.insert(run.id.clone());
        }
        Ok(fenced)
    }

    /// Ends the runs [`Self::fence_runs`] fenced, once the session's work stopped: what they
    /// held under admission goes, a report an earlier ending is writing lands first, and
    /// phases that weren't checked settle as cut off. An archived session gets the report a
    /// Stop gives (without a notification: the user is here); a deleted one doesn't, its
    /// transcript goes with it. Either way the run is finished, so Continue can follow a
    /// restore and nothing of it resumes after a restart.
    pub(crate) async fn close_runs(&self, runs: Vec<OvernightRun>, report: bool) {
        for run in runs {
            self.release_run(&run.id);
            drop(self.overnight.reporting.lock().await);
            let settled = self
                .change_run_if(&run, |now| {
                    if now.state == OvernightState::WindingDown {
                        now.state = OvernightState::Reporting;
                    }
                    if !report {
                        now.state = OvernightState::Finished;
                        now.finished_at_ms = Some(now_ms());
                    }
                    Some(())
                })
                .await;
            if report && let Some(reporting) = settled {
                // The thread's last commits on the run's branch get their review before the
                // report lists them and the worktree goes.
                self.scan_run_branch(&reporting).await;
                self.write_run_report(&reporting, false).await;
                self.change_run_if(&reporting, |now| {
                    now.state = OvernightState::Finished;
                    now.finished_at_ms = Some(now_ms());
                    // One an earlier ending queued, too.
                    if let Some(notification) = now.notification.as_mut() {
                        notification.delivered_at_ms.get_or_insert_with(now_ms);
                    }
                    Some(())
                })
                .await;
            }
            self.overnight
                .winding
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&run.id);
            tracing::info!(run = %run.id, report, "a closing session ended its overnight run");
        }
    }
}

/// Fences a run whose session closes: a proposal is dropped, a started run winds down as if
/// stopped. Whether it changed.
fn fence(run: &mut OvernightRun) -> bool {
    match run.state {
        OvernightState::Proposed => {
            run.state = OvernightState::Superseded;
            run.revision += 1;
            true
        }
        OvernightState::Preparing | OvernightState::Running | OvernightState::WaitingQuota => {
            run.state = OvernightState::WindingDown;
            run.stop.get_or_insert(StopReason::Stopped);
            true
        }
        OvernightState::WindingDown
        | OvernightState::Reporting
        | OvernightState::Finished
        | OvernightState::Superseded => false,
    }
}

/// A task that works for this run's current generation.
fn owned(task: &crate::work::Task, run: &OvernightRun) -> bool {
    task.run
        .as_ref()
        .is_some_and(|context| context.run_id == run.id)
}

/// Whether a run's task still works, so its ending waits for it: it runs or lands, or its
/// checked fix lands on its own once its worker's turn is over.
fn still_works(task: &crate::work::Task) -> bool {
    matches!(
        task.state,
        TaskState::Queued | TaskState::Starting | TaskState::Running | TaskState::Landing
    ) || super::super::workers::relanding_pending(task)
}

/// When live work stops at the latest: a deadline run keeps the last part of its reserve for
/// the report; a Stop without one waits a bounded time.
fn cutoff_ms(run: &OvernightRun, now: i64) -> i64 {
    let allowance = now + STOP_ALLOWANCE_MS;
    match (&run.directives.deadline, run.wind_down_at_ms) {
        (Deadline::At { time }, Some(wind_down)) => {
            let reserve = ((time.at_ms - wind_down) / 3).clamp(0, REPORT_RESERVE_MAX_MS);
            (time.at_ms - reserve).min(allowance).max(now)
        }
        _ => allowance,
    }
}

/// The words of [`stop_words`] that name the user, and how they read to the user. A phase's
/// gaps and evidence keep the stored words (a later lead reads them too); the report and the
/// app show these.
pub(super) const TO_YOU: [(&str, &str); 3] = [
    ("the user stopped the run", "you stopped the run"),
    (
        "the run stopped as the user asked",
        "the run stopped as you asked",
    ),
    (" needs the user", " needs you"),
];

/// `text` with the run's words about the user (see [`TO_YOU`]) said to the user.
pub(super) fn to_you(text: &str) -> String {
    TO_YOU
        .iter()
        .fold(text.to_owned(), |text, (stored, shown)| {
            text.replace(stored, shown)
        })
}

/// Why the run ended, as the thread and its workers are told.
fn stop_words(stop: Option<&StopReason>) -> String {
    match stop {
        Some(StopReason::Stopped) => "the user stopped the run".into(),
        Some(StopReason::Deadline) => "the run reached its deadline".into(),
        Some(StopReason::Blocked { phase_id }) => {
            format!("the run stopped early: {phase_id} needs the user")
        }
        Some(StopReason::StopDirective) => "the run stopped as the user asked".into(),
        Some(StopReason::Failed { message }) => format!("the run failed: {message}"),
        Some(StopReason::Done) | None => "the run ended".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> OvernightRun {
        OvernightRun::for_test(
            crate::model::ConversationId("c".into()),
            "Speed",
            Vec::new(),
        )
    }

    #[test]
    fn a_checked_fix_landing_on_its_own_holds_the_ending() {
        let mut task: crate::work::Task = serde_json::from_value(serde_json::json!({
            "id": "t1",
            "conversationId": "c1",
            "number": 1,
            "position": 0,
            "title": "Phase 4",
            "kind": "implement",
            "spec": "Write p4.txt.",
            "access": { "repo": "write", "network": false, "unsandboxed": false },
            "route": { "choice": { "provider": "claude", "model": null, "effort": null }, "reason": "" },
            "state": "reported",
            "attachments": [],
            "createdAtMs": 0,
            "updatedAtMs": 0
        }))
        .expect("a task");
        // A report the thread decides about doesn't hold the ending; one Brigadier lands
        // itself once its worker's turn is over does.
        assert!(!still_works(&task));
        task.landing = Some("Phase 4".into());
        assert!(still_works(&task));
        task.state = TaskState::Landing;
        assert!(still_works(&task));
        task.state = TaskState::Landed;
        assert!(!still_works(&task));
    }

    #[test]
    fn a_closing_session_fences_its_run() {
        let mut run = running();
        assert!(fence(&mut run));
        assert_eq!(run.state, OvernightState::WindingDown);
        assert_eq!(run.stop, Some(StopReason::Stopped));
        // Fencing again, or a run already ending, changes nothing.
        assert!(!fence(&mut run));
        // A proposal nobody started is dropped; a finished run stays as it is.
        let mut proposed = running();
        proposed.state = OvernightState::Proposed;
        assert!(fence(&mut proposed));
        assert_eq!(proposed.state, OvernightState::Superseded);
        let mut finished = running();
        finished.state = OvernightState::Finished;
        assert!(!fence(&mut finished));
        assert_eq!(finished.state, OvernightState::Finished);
    }
}
