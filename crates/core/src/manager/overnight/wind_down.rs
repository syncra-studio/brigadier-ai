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
use crate::model::{ConversationId, DomainEvent, OvernightRunId};
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
                    settle_cut_off(now, &reason);
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

impl SessionManager {
    /// The session is being archived or deleted: its runs end before anything of it stops or
    /// goes. The fence is durable (an active run is `WindingDown`, a proposal is dropped), so
    /// no new phase, task, fix or retry starts and nothing new is admitted, and the deadline
    /// clock leaves the ending to [`Self::close_runs`]. The runs it fenced.
    pub(crate) async fn fence_runs(&self, conversation_id: &ConversationId) -> Vec<OvernightRun> {
        let _held = self.overnight.changes.lock().await;
        let Ok(board) = self.core.board(conversation_id).await else {
            return Vec::new();
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
        if !events.is_empty()
            && let Err(err) = self.record_runs(conversation_id, events).await
        {
            tracing::warn!(conversation = %conversation_id, error = %err, "could not fence the session's runs");
        }
        let mut winding = self
            .overnight
            .winding
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        for run in &fenced {
            winding.insert(run.id.clone());
        }
        fenced
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
            let reason = if report {
                "the session was archived"
            } else {
                "the session was deleted"
            };
            let settled = self
                .change_run_if(&run, |now| {
                    if now.state == OvernightState::WindingDown {
                        settle_cut_off(now, reason);
                    }
                    if !report {
                        now.state = OvernightState::Finished;
                        now.finished_at_ms = Some(now_ms());
                    }
                    Some(())
                })
                .await;
            if report && let Some(reporting) = settled {
                self.write_run_report(&reporting).await;
                self.change_run_if(&reporting, |now| {
                    now.state = OvernightState::Finished;
                    now.finished_at_ms = Some(now_ms());
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
        OvernightState::Preparing
        | OvernightState::Planning
        | OvernightState::Running
        | OvernightState::PhaseGate
        | OvernightState::WaitingQuota => {
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

/// Phases (and Phase 0) that were running or checked when the run ended settle as what they
/// are, `reason` saying why; the run goes on to its report.
fn settle_cut_off(run: &mut OvernightRun, reason: &str) {
    for phase in &mut run.phases {
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
        phase.gaps.push(format!("{CUT_OFF}{reason}."));
        phase.settled_at_ms = Some(now_ms());
    }
    if let Some(planning) = run.planning.as_mut()
        && matches!(planning.state, PhaseState::Running | PhaseState::Checking)
    {
        planning.state = PhaseState::Blocked;
        planning
            .gaps
            .push(format!("The plan wasn't finished: {reason}."));
        planning.settled_at_ms = Some(now_ms());
    }
    run.state = OvernightState::Reporting;
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

/// How the gap of a phase the run's end cut off begins; why the run ended follows.
pub(super) const CUT_OFF: &str = "Its whole-phase checks never passed: ";

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The night of 2026-10-03 (the app's fixture of it), as it was mid-run.
    fn running() -> OvernightRun {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../apps/desktop/src/fixtures/boards/overnight-2026-10-03.json"
        ))
        .expect("the fixture");
        let mut run: OvernightRun = serde_json::from_value(
            fixture["overnight"]
                .as_object()
                .and_then(|runs| runs.values().next())
                .cloned()
                .expect("the run"),
        )
        .expect("a run");
        run.state = OvernightState::Running;
        run.stop = None;
        run.phases[0].state = PhaseState::Running;
        run.phases[0].criteria.clear();
        run
    }

    #[test]
    fn a_closing_session_fences_its_run_and_settles_what_it_cut_off() {
        let mut run = running();
        assert!(fence(&mut run));
        assert_eq!(run.state, OvernightState::WindingDown);
        assert_eq!(run.stop, Some(StopReason::Stopped));
        // Fencing again, or a run already ending, changes nothing.
        assert!(!fence(&mut run));
        settle_cut_off(&mut run, "the session was deleted");
        assert_eq!(run.state, OvernightState::Reporting);
        assert_eq!(run.phases[0].state, PhaseState::Partial);
        assert!(
            run.phases[0]
                .criteria
                .iter()
                .all(|c| c.status == CriterionStatus::NotRun)
        );
        assert!(
            run.phases[0]
                .gaps
                .last()
                .is_some_and(|gap| gap.ends_with("the session was deleted."))
        );
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
