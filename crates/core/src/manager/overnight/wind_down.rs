//! The clean ending (PLAN.md §10.9): Stop, the deadline, a dependent block or the plan done.
//!
//! The fence comes first and is durable (the run is `WindingDown`): no new phase, task, fix or
//! retry starts. Live workers are asked to finish their current step and hand off cleanly in
//! their report; checks already running may finish. Whatever still runs at the cutoff is
//! stopped, its work kept with the task. Phases that were not checked settle as what they are
//! (never verified for missing evidence), the report is written, and the run's leases go.
//!
//! The deadline is a time the report is ready by: a run with one gets a third of its length
//! (at most 20 minutes) for this, and the last part of that (at most 2 minutes) stays for the
//! report alone. A Stop without a deadline gets a bounded allowance, ending sooner whenever
//! everything settles sooner. The clock is the wall clock, checked every half minute, so a
//! machine that slept past the deadline winds down as soon as it runs again.

use std::time::Duration;

use super::super::SessionManager;
use crate::model::{ConversationId, OvernightRunId};
use crate::overnight::{
    CriterionResult, CriterionStatus, Deadline, OvernightRun, OvernightState, PhaseState,
    StopReason,
};
use crate::work::TaskState;
use crate::{Result, now_ms};

/// The longest a Stop without a deadline waits for live work to hand off.
const STOP_ALLOWANCE_MS: i64 = 10 * 60_000;
/// The share kept for the report at the end of a deadline run, at most.
const REPORT_RESERVE_MAX_MS: i64 = 2 * 60_000;
/// How often a winding-down run looks whether its work settled.
const POLL: Duration = Duration::from_secs(5);
/// How often the deadline clock looks at the wall clock.
pub(crate) const CLOCK: Duration = Duration::from_secs(30);

/// What a live worker is told when its run ends.
const HAND_OFF: &str = "[Brigadier] The overnight run is ending now ({reason}). Finish only the step you are in the middle of, leave the worktree coherent, and call submit_report at once with a clean handoff: what you changed, what is left unfinished, decisions you made, traps you found, and the exact commands that verify your work. Start nothing new.";

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
                    let manager = self.arc();
                    let run = run.clone();
                    self.spawn(async move { manager.end_run(run).await });
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
                OvernightState::Preparing
                    | OvernightState::Planning
                    | OvernightState::Running
                    | OvernightState::PhaseGate
                    | OvernightState::WaitingQuota
            ) {
                run.state = OvernightState::WindingDown;
                run.stop = Some(StopReason::Deadline);
            }
            Ok(())
        })
        .await?;
        self.advance_soon(conversation_id, run_id);
        Ok(())
    }

    /// The run's clean ending. Runs once per run, whoever asks.
    pub(crate) async fn end_run(&self, run: OvernightRun) {
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
        // Until everything of the run settled, or the cutoff.
        loop {
            let Ok(board) = self.core.board(&run.conversation_id).await else {
                break;
            };
            let live = board
                .tasks
                .values()
                .filter(|task| owned(task, &run))
                .any(|task| {
                    matches!(
                        task.state,
                        TaskState::Queued
                            | TaskState::Starting
                            | TaskState::Running
                            | TaskState::Reviewing
                            | TaskState::AwaitingApproval
                    )
                });
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
        // Phases that weren't checked settle as what they are. A run already settled (its report
        // was cut off by a restart or failed once) goes straight on to the report.
        let reason = stop_words(run.stop.as_ref());
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
                    for phase in &mut now.phases {
                        if !matches!(phase.state, PhaseState::Running | PhaseState::Checking) {
                            continue;
                        }
                        let candidate = phase.gate.as_ref().and_then(|gate| gate.commit.clone());
                        if phase.criteria.is_empty() {
                            phase.criteria = phase
                                .done_when
                                .iter()
                                .map(|criterion| CriterionResult {
                                    id: criterion.id.clone(),
                                    status: CriterionStatus::NotRun,
                                    evidence: format!("Not checked: {reason}."),
                                    candidate: candidate.clone(),
                                    by: None,
                                })
                                .collect();
                        }
                        // Cut off by the run's end, it is unfinished, not blocked: it needs nothing.
                        phase.state = PhaseState::Partial;
                        phase
                            .gaps
                            .push(format!("Its whole-phase checks never passed: {reason}."));
                        phase.settled_at_ms = Some(now_ms());
                    }
                    if let Some(planning) = now.planning.as_mut()
                        && matches!(planning.state, PhaseState::Running | PhaseState::Checking)
                    {
                        planning.state = PhaseState::Blocked;
                        planning
                            .gaps
                            .push(format!("The plan wasn't finished: {reason}."));
                        planning.settled_at_ms = Some(now_ms());
                    }
                    now.state = OvernightState::Reporting;
                    Some(())
                })
                .await
            }
        };
        if let Some(reporting) = settled {
            self.write_run_report(&reporting).await;
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

/// A task that works for this run's current generation.
fn owned(task: &crate::work::Task, run: &OvernightRun) -> bool {
    task.run
        .as_ref()
        .is_some_and(|context| context.run_id == run.id)
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

/// Why the run ended, as a phase's gap says it.
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
