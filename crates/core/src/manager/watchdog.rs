//! The stall watchdog (built in, always on): work that stopped moving is moved on, and the
//! user hears what only they can unblock.
//!
//! - **Silent workers.** Only a worker mid-turn is watched: its task starting or running, a
//!   turn under way in a CLI that is attached, and nothing it waits for (its question to the
//!   orchestrator, a permission card, quota). Reported, reviewing, ready-to-land, paused,
//!   blocked, hibernated and stopping workers are never touched; gate members (reviewers and
//!   verifiers) are workers like any other while they run. Silence is measured from the
//!   worker's last event, or the start of its turn.
//!   - Silent for [`Timing::silent`] (or [`Timing::command`] while one of its commands or
//!     tool calls runs): it is nudged, a steer asking it to carry on or report.
//!   - Still silent [`Timing::after_nudge`] later: it continues in a fresh session of the
//!     same model with its hand-off ([`super::worker_handoff`]), its stuck turn closed.
//!   - Stalled again in the same attempt: another model takes the task over
//!     (`ErrorKind::Stalled`, which counts toward the error hand-offs' cap).
//! - **Dead workers.** A running task whose CLI is gone or no longer runs (for
//!   [`Timing::grace`], and nothing is handing it on) is handed to another model at once.
//! - **Stuck gates.** A round whose members all have a result but that was never decided is
//!   decided; a member that ended without recording its result has it recorded.
//! - **Stuck cards.** A card the user left unanswered for [`Timing::card`] is listed under
//!   "Waiting on you" (over when the card is answered or expires), with one desktop
//!   notification.
//!
//! Each action on a worker is checked again right before it is taken ([`still_due`]): the
//! same CLI session, nothing heard from it since, its task still running and waiting for
//! nothing. So a worker that moved on meanwhile is never closed, and a restart is one with
//! the other hand-ons and hand-overs (under the task's `reroute` lock). Each runs in a task of
//! its own, so a CLI slow to take a nudge holds up only itself. Each is logged under
//! "Decided for you". A debug build scales every threshold with `BRIGADIER_STALL_SECS` (the
//! silence before a nudge, in seconds), so the watchdog can be tried live.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use brigadier_providers::{ErrorKind, NoticeLevel, ProviderEvent};

use super::SessionManager;
use super::worker_handoff::Handover;
use super::workers::TaskLive;
use crate::board::Board;
use crate::model::{ConversationId, Lifecycle};
use crate::now_ms;
use crate::work::{
    ApprovalSubject, AttemptEnd, CardId, CardState, PlanState, QuestionKind, Task, TaskId,
    TaskState, WaitingSource,
};

/// The silence before a nudge, in seconds; every other threshold follows from it.
const UNIT_SECS: u64 = 10 * 60;

/// How long things may stay still before the watchdog acts, in ms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Timing {
    /// A worker mid-turn silent this long is nudged.
    pub silent: i64,
    /// … this long while one of its commands or tool calls runs.
    pub command: i64,
    /// Still silent this long after the nudge: a fresh session.
    pub after_nudge: i64,
    /// A card unanswered this long waits on the user.
    pub card: i64,
    /// What a hand-off or a result being recorded may take before a task without a running
    /// CLI, or a member that ended without its result, counts as stuck.
    pub grace: i64,
    /// How often the watchdog looks.
    pub tick: Duration,
}

impl Timing {
    /// The thresholds in force: 10 minutes of silence (40 while a command runs), 10 more after
    /// the nudge, 30 for a card. A debug build takes `BRIGADIER_STALL_SECS` instead of the
    /// 10 minutes.
    pub(crate) fn current() -> Self {
        if cfg!(debug_assertions)
            && let Some(secs) = std::env::var("BRIGADIER_STALL_SECS")
                .ok()
                .and_then(|value| value.trim().parse::<u64>().ok())
                .filter(|secs| *secs > 0)
        {
            return Self::scaled(secs);
        }
        Self::scaled(UNIT_SECS)
    }

    fn scaled(unit_secs: u64) -> Self {
        let unit = i64::try_from(unit_secs.min(24 * 60 * 60)).unwrap_or(0) * 1_000;
        let tick = u64::try_from(unit / 4).unwrap_or(0).clamp(5_000, 60_000);
        Self {
            silent: unit,
            command: unit * 4,
            after_nudge: unit,
            card: unit * 3,
            grace: (unit / 5).max(1_000),
            tick: Duration::from_millis(tick),
        }
    }
}

/// What the watchdog knows of a task's live worker ([`TaskLive::watch`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WorkerWatch {
    /// The CLI session at work ([`TaskLive::generation`]).
    pub generation: u64,
    /// A CLI is attached and its process runs.
    pub alive: bool,
    /// A turn is under way.
    pub busy: bool,
    /// Its CLI is being closed on purpose (a stop, a hand-off).
    pub stopping: bool,
    /// It waits on its question to the orchestrator.
    pub question: bool,
    /// One of its commands or tool calls runs.
    pub command_running: bool,
    pub last_event_ms: i64,
    /// When the watchdog nudged this CLI session.
    pub nudged_at_ms: Option<i64>,
    /// Fresh sessions the watchdog started for stalls in the current attempt.
    pub stalls: u32,
    /// Since when the task is live without a running CLI.
    pub orphaned_at_ms: Option<i64>,
    /// Nothing is routing it again or handing it over right now.
    pub reroute_free: bool,
}

/// What the watchdog does about a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StallVerdict {
    /// Ask it whether it is stuck (a steer).
    Nudge,
    /// Continue in a fresh session of the same model.
    HandOver,
    /// Hand the task to another model.
    Replace(Replaced),
}

/// Why another model takes a task over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Replaced {
    /// Its CLI is gone, or no longer runs.
    Exited,
    /// It stalled again after a fresh session.
    Stalled,
}

/// What to do about the worker of a task in `state` (`quota_wait`: it waits for quota;
/// `card_open`: a permission card of it waits for the user), as `watch` saw it at `now`.
pub(crate) fn stall_verdict(
    state: TaskState,
    quota_wait: bool,
    card_open: bool,
    watch: &WorkerWatch,
    timing: &Timing,
    now: i64,
) -> Option<StallVerdict> {
    if !matches!(state, TaskState::Starting | TaskState::Running) || quota_wait || watch.stopping {
        return None;
    }
    if !watch.alive {
        // A starting task is still getting its CLI; a running one lost it.
        let lost = state == TaskState::Running
            && watch.reroute_free
            && watch
                .orphaned_at_ms
                .is_some_and(|since| now - since >= timing.grace);
        return lost.then_some(StallVerdict::Replace(Replaced::Exited));
    }
    if !watch.busy || watch.question || card_open {
        return None;
    }
    // A nudge counts until the worker shows it works again.
    if let Some(nudged) = watch
        .nudged_at_ms
        .filter(|nudged| *nudged >= watch.last_event_ms)
    {
        if now - nudged < timing.after_nudge {
            return None;
        }
        return Some(if watch.stalls > 0 {
            StallVerdict::Replace(Replaced::Stalled)
        } else {
            StallVerdict::HandOver
        });
    }
    let quiet = if watch.command_running {
        timing.command
    } else {
        timing.silent
    };
    (now - watch.last_event_ms >= quiet).then_some(StallVerdict::Nudge)
}

/// Whether `decided`, the action decided on the worker as `seen`, still holds for it as it
/// is now (`current`, with `verdict` what [`stall_verdict`] says of it now): the same CLI
/// session, nothing heard from it and no nudge or stall since, and the same verdict (its task
/// still starting or running, a turn under way, nothing it waits for). Rechecked right before
/// acting, since the worker may have moved on after the round looked.
pub(crate) fn still_due(
    decided: StallVerdict,
    seen: &WorkerWatch,
    current: &WorkerWatch,
    verdict: Option<StallVerdict>,
) -> bool {
    current.generation == seen.generation
        && current.last_event_ms == seen.last_event_ms
        && current.nudged_at_ms == seen.nudged_at_ms
        && current.stalls == seen.stalls
        && verdict == Some(decided)
}

/// Whether a permission card of `task` waits for the user.
fn card_open(board: &Board, task: &TaskId) -> bool {
    board.approvals.values().any(|approval| {
        approval.task_id.as_ref() == Some(task) && approval.state == CardState::Pending
    })
}

/// A card the user left unanswered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StuckCard {
    pub card_id: CardId,
    pub request_id: Option<String>,
    /// What the user is to do, in plain words.
    pub what: String,
}

/// The cards of `board` unanswered for `after` at `now` that were never listed for the user
/// (once they mark one done, it isn't listed again).
pub(crate) fn stuck_cards(board: &Board, now: i64, after: i64) -> Vec<StuckCard> {
    let listed = |card: &CardId| {
        let prefix = format!("card:{card}|");
        board
            .waits_listed
            .iter()
            .any(|key| key.starts_with(&prefix))
    };
    let old = |created: i64| now - created >= after;
    let task_name = |task: &Option<TaskId>| {
        task.as_ref()
            .and_then(|id| board.tasks.get(id))
            .map_or_else(|| "a worker".to_owned(), |t| format!("task-{}", t.number))
    };
    let mut stuck = Vec::new();
    for approval in board.approvals.values() {
        if approval.state != CardState::Pending
            || !old(approval.created_at_ms)
            || listed(&approval.id)
        {
            continue;
        }
        let what = match &approval.subject {
            ApprovalSubject::Cli { request } => format!(
                "Allow or decline what {} asked for: {}",
                if approval.task_id.is_some() {
                    task_name(&approval.task_id)
                } else {
                    "the session".to_owned()
                },
                request.command.as_deref().unwrap_or(&request.tool)
            ),
            ApprovalSubject::OutwardCommand { argv, .. } => {
                format!("Allow or decline the command: {}", argv.join(" "))
            }
            ApprovalSubject::Landing { task_id, .. } => format!(
                "Approve or decline landing {}",
                task_name(&Some(task_id.clone()))
            ),
            ApprovalSubject::FinishSession { branch, base, .. } => {
                format!("Approve or decline merging {branch} into {base}")
            }
            ApprovalSubject::Action { action, .. } => format!("Approve or decline: {action}"),
            ApprovalSubject::Outline { title, .. } => {
                format!("Start or decline the plan “{title}”")
            }
        };
        stuck.push(StuckCard {
            card_id: approval.id.clone(),
            request_id: approval.request_id.clone(),
            what,
        });
    }
    for question in board.questions.values() {
        if question.answer.is_some()
            || question.answered_at_ms.is_some()
            || !old(question.created_at_ms)
            || listed(&question.id)
        {
            continue;
        }
        let what = match &question.kind {
            QuestionKind::Orchestrator => format!("Answer the question: {}", question.text),
            QuestionKind::UncommittedChanges { .. } => {
                "Answer whether workers see your uncommitted changes".to_owned()
            }
            QuestionKind::Merge { base, .. } => format!("Answer whether to merge into {base}"),
        };
        stuck.push(StuckCard {
            card_id: question.id.clone(),
            request_id: question.request_id.clone(),
            what,
        });
    }
    // A plan in review waits for its reviewers, not the user.
    for plan in board.plans.values() {
        if plan.state != PlanState::Proposed || !old(plan.created_at_ms) || listed(&plan.id) {
            continue;
        }
        stuck.push(StuckCard {
            card_id: plan.id.clone(),
            request_id: plan.request_id.clone(),
            what: format!("Approve or decline the plan \u{201c}{}\u{201d}", plan.title),
        });
    }
    stuck.sort_by(|a, b| a.card_id.0.cmp(&b.card_id.0));
    stuck
}

/// A duration as people say it ("10 minutes", "45 seconds").
pub(crate) fn spoken(ms: i64) -> String {
    let secs = (ms / 1_000).max(0);
    let (n, unit) = if secs < 60 {
        (secs, "second")
    } else {
        ((secs + 30) / 60, "minute")
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

impl SessionManager {
    /// Starts the watchdog's rounds.
    pub(super) fn start_watchdog(&self) {
        let manager = self.me.clone();
        let timing = Timing::current();
        self.spawn(async move {
            let mut tick = tokio::time::interval(timing.tick);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await;
            loop {
                tick.tick().await;
                let Some(manager) = manager.upgrade() else {
                    return;
                };
                if manager.admit().is_err() {
                    return;
                }
                manager.watch(&timing).await;
            }
        });
    }

    /// One round: the live conversations' workers, gates and cards.
    async fn watch(&self, timing: &Timing) {
        let lives: Vec<Arc<TaskLive>> = self.tasks_lock().values().cloned().collect();
        let mut conversations: HashSet<ConversationId> =
            self.convs_lock().keys().cloned().collect();
        conversations.extend(lives.iter().map(|live| live.conversation_id.clone()));
        for conversation_id in conversations {
            if self
                .core
                .conversation(&conversation_id)
                .is_ok_and(|conversation| conversation.lifecycle == Lifecycle::Archived)
            {
                continue;
            }
            let Ok(board) = self.core.board(&conversation_id).await else {
                continue;
            };
            let now = now_ms();
            for live in lives
                .iter()
                .filter(|live| live.conversation_id == conversation_id)
            {
                if let Some(task) = board.tasks.get(&live.id) {
                    self.watch_worker(live, task, &board, timing, now).await;
                }
            }
            self.watch_cards(&conversation_id, &board, timing, now)
                .await;
        }
    }

    /// Acts on a worker that stopped moving: in a task of its own, so a CLI slow to take a
    /// nudge or to close holds up neither the round nor the other workers; one action per
    /// worker at a time.
    async fn watch_worker(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        board: &Board,
        timing: &Timing,
        now: i64,
    ) {
        if task.state.is_final() {
            return;
        }
        let watch = self.unheld(&task.id, &live.watch(now).await);
        let Some(verdict) = stall_verdict(
            task.state,
            task.quota_wait.is_some(),
            card_open(board, &task.id),
            &watch,
            timing,
            now,
        ) else {
            return;
        };
        if live.watchdog_busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let busy = Busy(live.clone());
        let manager = self.arc();
        let (task, timing) = (task.clone(), *timing);
        self.spawn(async move {
            manager
                .act_on_worker(&busy.0, &task, verdict, &watch, &timing, now)
                .await;
        });
    }

    /// Takes `verdict` on the worker of `task`, decided at `now` on the worker as `seen`, if
    /// it still holds ([`still_due`]).
    async fn act_on_worker(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        verdict: StallVerdict,
        seen: &WorkerWatch,
        timing: &Timing,
        now: i64,
    ) {
        let silent = now - seen.last_event_ms;
        let n = task.number;
        match verdict {
            StallVerdict::Nudge => {
                let text = format!(
                    "[Brigadier] Nothing has come from you for {}. If a command is hanging, stop it and run it again with a timeout, or in the background. If you are stuck, say why in submit_report. Otherwise carry on.",
                    spoken(silent)
                );
                let Some((state, quota_wait, card)) = self.task_now(live).await else {
                    return;
                };
                let due = |current: &WorkerWatch| {
                    let current = &self.unheld(&task.id, current);
                    let verdict_now =
                        stall_verdict(state, quota_wait, card, current, timing, now_ms());
                    still_due(verdict, seen, current, verdict_now)
                };
                match live.nudge_stall(due, text).await {
                    None => {}
                    Some(true) => {
                        tracing::info!(task = %task.id, silent, "nudged a silent worker");
                        self.watchdog_notice(
                            task,
                            format!(
                                "No activity for {}: asked the worker to carry on or report.",
                                spoken(silent)
                            ),
                        )
                        .await;
                        self.decided_for_task(
                            task,
                            format!("Nudged task-{n}: no activity for {}", spoken(silent)),
                            "A worker silent this long is usually stuck on a command; it was asked to carry on or report what stops it.".into(),
                        )
                        .await;
                    }
                    Some(false) => {
                        self.watchdog_notice(
                            task,
                            format!(
                                "No activity for {}, and its CLI didn't take the nudge: a fresh session takes over if it stays silent.",
                                spoken(silent)
                            ),
                        )
                        .await;
                    }
                }
            }
            StallVerdict::HandOver => {
                let _handing = live.reroute.lock().await;
                if self
                    .take_stalled(live, verdict, seen, timing)
                    .await
                    .is_none()
                {
                    return;
                }
                tracing::info!(task = %task.id, silent, "a silent worker continues in a fresh session");
                self.watchdog_notice(
                    task,
                    format!(
                        "Still no activity after the nudge ({} in all): a fresh session takes over.",
                        spoken(silent)
                    ),
                )
                .await;
                self.decided_for_task(
                    task,
                    format!("Restarted task-{n} in a fresh session: it stayed silent after a nudge"),
                    format!(
                        "It had shown no activity for {}. The same model continues from its hand-off, with its changes kept.",
                        spoken(silent)
                    ),
                )
                .await;
                if let Err(err) = self
                    .hand_over_held(live, task, seen.generation, Handover::Stalled { silent })
                    .await
                {
                    let reason = format!("The task could not continue after it stalled: {err}");
                    self.worker_failed(task, &reason).await;
                }
            }
            StallVerdict::Replace(why) => {
                let _handing = live.reroute.lock().await;
                let Some(cutoff) = self.take_stalled(live, verdict, seen, timing).await else {
                    return;
                };
                let (end, what, because) = match why {
                    Replaced::Stalled => (
                        AttemptEnd::Error {
                            kind: ErrorKind::Stalled,
                            message: format!(
                                "The worker went silent for {} again after a fresh session.",
                                spoken(silent)
                            ),
                        },
                        format!("Gave task-{n} to another model: it stalled twice"),
                        "It went silent again after a nudge and a fresh session of the same model.",
                    ),
                    Replaced::Exited => (
                        AttemptEnd::Error {
                            kind: ErrorKind::Process,
                            message: "The worker's CLI stopped running before it reported.".into(),
                        },
                        format!("Gave task-{n} to another model: its CLI had stopped"),
                        "Its CLI stopped running before it reported.",
                    ),
                };
                tracing::info!(task = %task.id, ?why, "handing a stuck task to another model");
                self.watchdog_notice(task, format!("{because} Another model takes over."))
                    .await;
                self.decided_for_task(task, what, because.into()).await;
                // A model cut off meanwhile hands on for that.
                self.hand_off_held(live, cutoff.unwrap_or(end)).await;
            }
        }
    }

    /// Closes the CLI session of a stalled worker for a hand-over or hand-off, if `verdict`
    /// (decided on the worker as `seen`) still holds now, with its task as the board has it:
    /// still starting or running, no card or quota it waits for, nothing reported meanwhile
    /// (the report lock is held for the check). The caller holds `reroute`, so no other
    /// hand-on or hand-over runs meanwhile. Answers a pending cut-off (for another model);
    /// `None` when the action is no longer due.
    async fn take_stalled(
        &self,
        live: &Arc<TaskLive>,
        verdict: StallVerdict,
        seen: &WorkerWatch,
        timing: &Timing,
    ) -> Option<Option<AttemptEnd>> {
        let settled = live.settle.lock().await;
        let (state, quota_wait, card) = self.task_now(live).await?;
        let due = |current: &WorkerWatch| {
            let current = &self.unheld(&live.id, current);
            let verdict_now = stall_verdict(state, quota_wait, card, current, timing, now_ms());
            still_due(verdict, seen, current, verdict_now)
        };
        let (cli, cutoff) = live
            .detach_stalled(due, verdict == StallVerdict::HandOver)
            .await?;
        drop(settled);
        TaskLive::end_cli(cli).await;
        Some(cutoff)
    }

    /// The task of `live` as the board has it now: its state, whether it waits for quota, and
    /// whether a permission card of it waits for the user.
    async fn task_now(&self, live: &TaskLive) -> Option<(TaskState, bool, bool)> {
        let board = self.core.board(&live.conversation_id).await.ok()?;
        let task = board.tasks.get(&live.id)?;
        Some((
            task.state,
            task.quota_wait.is_some(),
            card_open(&board, &task.id),
        ))
    }

    /// Records a watchdog action in the task's transcript.
    async fn watchdog_notice(&self, task: &Task, message: String) {
        self.record_worker_event(
            &task.id,
            ProviderEvent::Notice {
                level: NoticeLevel::Warning,
                message,
            },
        )
        .await;
    }

    /// Lists the cards the user left unanswered for long under "Waiting on you".
    async fn watch_cards(
        &self,
        conversation_id: &ConversationId,
        board: &Board,
        timing: &Timing,
        now: i64,
    ) {
        for card in stuck_cards(board, now, timing.card) {
            let added = self
                .wait_on_user(
                    conversation_id,
                    card.request_id.clone(),
                    WaitingSource::Card {
                        card_id: card.card_id.clone(),
                    },
                    &card.what,
                )
                .await;
            match added {
                Ok(true) => {
                    // The app shows it as a desktop notification too.
                    tracing::info!(conversation = %conversation_id, card = %card.card_id, "a card waits on the user");
                }
                Ok(false) => {}
                Err(err) => {
                    tracing::warn!(conversation = %conversation_id, error = %err, "could not list a card for the user");
                }
            }
        }
    }
}

/// Marks a watchdog action on a worker under way, until it is over.
struct Busy(Arc<TaskLive>);

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.watchdog_busy.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;

    fn timing() -> Timing {
        Timing::scaled(UNIT_SECS)
    }

    /// A worker mid-turn, last heard from at 0.
    fn working() -> WorkerWatch {
        WorkerWatch {
            generation: 1,
            alive: true,
            busy: true,
            reroute_free: true,
            ..WorkerWatch::default()
        }
    }

    fn verdict(state: TaskState, watch: &WorkerWatch, now: i64) -> Option<StallVerdict> {
        stall_verdict(state, false, false, watch, &timing(), now)
    }

    #[test]
    fn the_thresholds_scale_with_one_unit() {
        let t = timing();
        assert_eq!(
            (t.silent, t.command, t.after_nudge, t.card),
            (10 * MIN, 40 * MIN, 10 * MIN, 30 * MIN)
        );
        assert_eq!(t.grace, 2 * MIN);
        assert_eq!(t.tick, Duration::from_secs(60));
        let fast = Timing::scaled(60);
        assert_eq!(
            (fast.silent, fast.command, fast.card, fast.grace),
            (MIN, 4 * MIN, 3 * MIN, 12_000)
        );
        assert_eq!(fast.tick, Duration::from_secs(15));
        assert_eq!(Timing::scaled(1).tick, Duration::from_secs(5));
    }

    #[test]
    fn a_silent_worker_is_nudged_then_handed_over_then_replaced() {
        let mut watch = working();
        assert_eq!(verdict(TaskState::Running, &watch, 9 * MIN), None);
        assert_eq!(
            verdict(TaskState::Running, &watch, 10 * MIN),
            Some(StallVerdict::Nudge)
        );
        assert_eq!(
            verdict(TaskState::Starting, &watch, 10 * MIN),
            Some(StallVerdict::Nudge)
        );
        // Nudged at 10: nothing until 10 minutes later, then a fresh session.
        watch.nudged_at_ms = Some(10 * MIN);
        assert_eq!(verdict(TaskState::Running, &watch, 19 * MIN), None);
        assert_eq!(
            verdict(TaskState::Running, &watch, 20 * MIN),
            Some(StallVerdict::HandOver)
        );
        // A second stall in the same attempt: another model.
        watch.stalls = 1;
        assert_eq!(
            verdict(TaskState::Running, &watch, 20 * MIN),
            Some(StallVerdict::Replace(Replaced::Stalled))
        );
    }

    #[test]
    fn a_worker_that_answers_the_nudge_starts_over() {
        let mut watch = working();
        watch.nudged_at_ms = Some(10 * MIN);
        watch.last_event_ms = 12 * MIN;
        assert_eq!(verdict(TaskState::Running, &watch, 21 * MIN), None);
        assert_eq!(
            verdict(TaskState::Running, &watch, 22 * MIN),
            Some(StallVerdict::Nudge)
        );
    }

    #[test]
    fn a_running_command_gets_longer() {
        let mut watch = working();
        watch.command_running = true;
        assert_eq!(verdict(TaskState::Running, &watch, 39 * MIN), None);
        assert_eq!(
            verdict(TaskState::Running, &watch, 40 * MIN),
            Some(StallVerdict::Nudge)
        );
    }

    #[test]
    fn only_active_busy_turns_are_watched() {
        let late = 3 * 60 * MIN;
        let watch = working();
        for state in [
            TaskState::Queued,
            TaskState::Blocked,
            TaskState::Paused,
            TaskState::Reported,
            TaskState::Landing,
            TaskState::ReadyToLand,
            TaskState::Landed,
            TaskState::Done,
            TaskState::Stopped,
            TaskState::Failed,
        ] {
            assert_eq!(verdict(state, &watch, late), None, "{state:?}");
        }
        // Between turns (a reported worker waiting idle).
        let idle = WorkerWatch {
            busy: false,
            ..working()
        };
        assert_eq!(verdict(TaskState::Running, &idle, late), None);
        // Asked the orchestrator, or waiting on a permission card.
        let asking = WorkerWatch {
            question: true,
            ..working()
        };
        assert_eq!(verdict(TaskState::Running, &asking, late), None);
        let t = timing();
        assert_eq!(
            stall_verdict(TaskState::Running, false, true, &watch, &t, late),
            None
        );
        // Waiting for quota.
        assert_eq!(
            stall_verdict(TaskState::Running, true, false, &watch, &t, late),
            None
        );
        // Being stopped or handed on.
        let stopping = WorkerWatch {
            stopping: true,
            ..working()
        };
        assert_eq!(verdict(TaskState::Running, &stopping, late), None);
        // Hibernated: no CLI, and not running.
        let hibernated = WorkerWatch {
            alive: false,
            busy: false,
            orphaned_at_ms: Some(0),
            ..working()
        };
        assert_eq!(verdict(TaskState::Reported, &hibernated, late), None);
    }

    #[test]
    fn a_running_task_without_its_cli_is_replaced_after_the_grace() {
        let gone = WorkerWatch {
            alive: false,
            orphaned_at_ms: Some(0),
            ..working()
        };
        assert_eq!(verdict(TaskState::Running, &gone, MIN), None);
        assert_eq!(
            verdict(TaskState::Running, &gone, 2 * MIN),
            Some(StallVerdict::Replace(Replaced::Exited))
        );
        // Still starting, or being routed again meanwhile.
        assert_eq!(verdict(TaskState::Starting, &gone, 60 * MIN), None);
        let rerouting = WorkerWatch {
            reroute_free: false,
            ..gone.clone()
        };
        assert_eq!(verdict(TaskState::Running, &rerouting, 60 * MIN), None);
        // Not yet seen without it.
        let unseen = WorkerWatch {
            orphaned_at_ms: None,
            ..gone
        };
        assert_eq!(verdict(TaskState::Running, &unseen, 60 * MIN), None);
    }

    #[test]
    fn a_nudge_counts_whether_or_not_it_was_delivered() {
        // Recorded at the time the nudge began, even when its CLI never took it: the
        // hand-over follows the grace.
        let mut watch = working();
        watch.nudged_at_ms = Some(10 * MIN);
        assert_eq!(
            verdict(TaskState::Running, &watch, 20 * MIN),
            Some(StallVerdict::HandOver)
        );
        // Heard from while the nudge was on its way: it isn't stuck.
        watch.last_event_ms = 10 * MIN + 1;
        assert_eq!(verdict(TaskState::Running, &watch, 20 * MIN), None);
    }

    #[test]
    fn an_action_is_taken_only_while_it_still_holds() {
        let t = timing();
        let now = 20 * MIN;
        let seen = WorkerWatch {
            nudged_at_ms: Some(10 * MIN),
            ..working()
        };
        let due = |current: &WorkerWatch, state: TaskState, quota_wait: bool, card: bool| {
            let verdict_now = stall_verdict(state, quota_wait, card, current, &t, now);
            still_due(StallVerdict::HandOver, &seen, current, verdict_now)
        };
        assert!(due(&seen, TaskState::Running, false, false));
        let changed = |change: fn(&mut WorkerWatch)| {
            let mut current = seen.clone();
            change(&mut current);
            current
        };
        for (what, current) in [
            // Heard from since (still before the nudge, so the verdict alone wouldn't tell).
            ("activity", changed(|w| w.last_event_ms = MIN)),
            ("another nudge", changed(|w| w.nudged_at_ms = Some(5 * MIN))),
            ("a fresh session", changed(|w| w.generation = 2)),
            ("a stall counted", changed(|w| w.stalls = 1)),
            ("a question", changed(|w| w.question = true)),
            ("the turn ended", changed(|w| w.busy = false)),
            ("being closed", changed(|w| w.stopping = true)),
        ] {
            assert!(!due(&current, TaskState::Running, false, false), "{what}");
        }
        // Its task reported, waits for a card or for quota meanwhile.
        for state in [
            TaskState::Reported,
            TaskState::Landing,
            TaskState::Paused,
            TaskState::Stopped,
        ] {
            assert!(!due(&seen, state, false, false), "{state:?}");
        }
        assert!(!due(&seen, TaskState::Running, false, true));
        assert!(!due(&seen, TaskState::Running, true, false));
    }

    #[test]
    fn a_replacement_holds_only_while_the_cli_stays_gone() {
        let t = timing();
        let now = 5 * MIN;
        let seen = WorkerWatch {
            alive: false,
            orphaned_at_ms: Some(0),
            ..working()
        };
        let decided = StallVerdict::Replace(Replaced::Exited);
        let due = |current: &WorkerWatch| {
            let verdict_now = stall_verdict(TaskState::Running, false, false, current, &t, now);
            still_due(decided, &seen, current, verdict_now)
        };
        assert!(due(&seen));
        // Another hand-on took it meanwhile (its session closing), or started a new one.
        let closing = WorkerWatch {
            stopping: true,
            ..seen.clone()
        };
        assert!(!due(&closing));
        let fresh = WorkerWatch {
            alive: true,
            generation: 2,
            ..seen.clone()
        };
        assert!(!due(&fresh));
    }

    fn approval(id: &str, created: i64, state: CardState) -> crate::work::Approval {
        crate::work::Approval {
            id: CardId(id.into()),
            conversation_id: ConversationId("c1".into()),
            task_id: None,
            request_id: Some("r1".into()),
            position: 0,
            subject: ApprovalSubject::Action {
                action: "Deploy to staging".into(),
                details: String::new(),
                live: false,
            },
            state,
            created_at_ms: created,
            resolved_at_ms: None,
        }
    }

    #[test]
    fn a_card_waits_on_the_user_once_after_half_an_hour() {
        let mut board = Board::default();
        for card in [
            approval("a1", 0, CardState::Pending),
            approval("a2", 10 * MIN, CardState::Pending),
            approval(
                "a3",
                0,
                CardState::Expired {
                    reason: "The task ended".into(),
                },
            ),
        ] {
            board.approvals.insert(card.id.clone(), card);
        }
        let stuck = stuck_cards(&board, 30 * MIN, 30 * MIN);
        assert_eq!(
            stuck,
            [StuckCard {
                card_id: CardId("a1".into()),
                request_id: Some("r1".into()),
                what: "Approve or decline: Deploy to staging".into(),
            }]
        );
        // Listed once (and marked done by the user since): never again.
        let source = WaitingSource::Card {
            card_id: CardId("a1".into()),
        };
        board
            .waits_listed
            .insert(super::super::decisions::waiting_key(
                &source,
                &stuck[0].what,
            ));
        assert!(stuck_cards(&board, 31 * MIN, 30 * MIN).is_empty());
        // a2 is half an hour old by then.
        let later = stuck_cards(&board, 40 * MIN, 30 * MIN);
        assert_eq!(later.len(), 1);
        assert_eq!(later[0].card_id, CardId("a2".into()));
    }

    #[test]
    fn durations_read_as_people_say_them() {
        assert_eq!(spoken(45_000), "45 seconds");
        assert_eq!(spoken(60_000), "1 minute");
        assert_eq!(spoken(100_000), "2 minutes");
        assert_eq!(spoken(10 * MIN), "10 minutes");
        assert_eq!(spoken(2 * MIN), "2 minutes");
        assert_eq!(spoken(1_000), "1 second");
    }
}
