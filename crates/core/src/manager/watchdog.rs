//! Reports real worker silence to the thread, which decides whether to message, stop or
//! replace the worker. Commands and tool calls are never interrupted for taking time.
//! A CLI process that has gone away is recovered automatically after the plumbing grace.
//! Unanswered cards are still listed under "Waiting on you" for overnight runs.

pub(crate) mod progress;

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use brigadier_providers::{ErrorKind, NoticeLevel, ProviderEvent};

use super::SessionManager;
use super::decisions::waiting_run;
use super::workers::TaskLive;
use crate::board::Board;
use crate::model::{ConversationId, Lifecycle};
use crate::now_ms;
use crate::work::{
    ApprovalSubject, AttemptEnd, CardId, CardState, PlanState, QuestionKind, Task, TaskId,
    TaskState, WaitingSource,
};

/// The silence before a report, in seconds; every other threshold follows from it.
const UNIT_SECS: u64 = 3 * 60;

/// How long things may stay still before the watchdog acts, in ms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Timing {
    /// A worker mid-turn silent this long is reported to the thread.
    pub silent: i64,
    /// Command timing metadata, never a reason to act on a running call.
    pub command: i64,
    pub server: i64,
    pub build: i64,
    /// A card unanswered this long waits on the user.
    pub card: i64,
    /// What a hand-off or a result being recorded may take before a task without a running
    /// CLI, or a member that ended without its result, counts as stuck.
    pub grace: i64,
    /// How often the watchdog looks.
    pub tick: Duration,
}

impl Timing {
    /// Three minutes of silence before a report, with a separate process-exit grace.
    /// A debug build takes `BRIGADIER_STALL_SECS` instead of the three minutes.
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

    pub(crate) fn scaled(unit_secs: u64) -> Self {
        let unit = i64::try_from(unit_secs.min(24 * 60 * 60)).unwrap_or(0) * 1_000;
        let tick = u64::try_from(unit / 4).unwrap_or(0).clamp(5_000, 60_000);
        Self {
            silent: unit,
            command: unit * 5 / 3,
            server: unit * 2 / 3,
            build: unit * 10,
            card: unit * 10,
            grace: (unit * 2 / 3).max(1_000),
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
    pub command_kind: Option<progress::CommandKind>,
    pub command_due_ms: Option<i64>,
    pub waiting: bool,
    pub last_event_ms: i64,
    /// When silence was last reported for this CLI session.
    pub reported_at_ms: Option<i64>,
    pub last_command: Option<String>,
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
    Report,
    /// Its CLI is gone, or no longer runs.
    ReplaceExited,
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
        return lost.then_some(StallVerdict::ReplaceExited);
    }
    if !watch.busy || watch.question || watch.waiting || card_open || watch.command_running {
        return None;
    }
    // One fact per silent interval. New progress starts a new interval; silence alone
    // never escalates into a restart or a different model.
    if watch
        .reported_at_ms
        .is_some_and(|at| at >= watch.last_event_ms)
    {
        return None;
    }
    (now - watch.last_event_ms >= timing.silent).then_some(StallVerdict::Report)
}

/// Whether `decided`, the action decided on the worker as `seen`, still holds for it as it
/// is now (`current`, with `verdict` what [`stall_verdict`] says of it now): the same CLI
/// session, nothing heard from it and no report or stall since, and the same verdict (its task
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
        && current.reported_at_ms == seen.reported_at_ms
        && current.stalls == seen.stalls
        && current.command_kind == seen.command_kind
        && current.command_due_ms == seen.command_due_ms
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
            ApprovalSubject::PeerMessage { title, .. } => {
                format!("Send or don't send the message to “{title}”")
            }
            ApprovalSubject::Browser { question, .. } => format!("Allow or decline: {question}"),
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

    /// Reports silence or recovers an exited CLI in a task of its own, so a slow close
    /// holds up neither the round nor the other workers; one action per worker at a time.
    pub(super) async fn watch_worker(
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
        let watch = live.watch(now).await;
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
        match verdict {
            StallVerdict::Report => {
                let Some((state, quota_wait, card)) = self.task_now(live).await else {
                    return;
                };
                let due = |current: &WorkerWatch| {
                    still_due(
                        verdict,
                        seen,
                        current,
                        stall_verdict(state, quota_wait, card, current, timing, now),
                    )
                };
                if !live.report_stall(due, now).await {
                    return;
                }
                // Logged only: the thread reads its workers when it wants to (G1).
                tracing::info!(
                    task = %task.id,
                    silent_min = (now - seen.last_event_ms) / 60_000,
                    last_command = seen.last_command.as_deref().unwrap_or("none"),
                    "a worker is silent"
                );
            }
            StallVerdict::ReplaceExited => {
                let _handing = live.reroute.lock().await;
                let Some(cutoff) = self.take_exited(live, verdict, seen, timing, now).await else {
                    return;
                };
                let end = AttemptEnd::Error {
                    kind: ErrorKind::Process,
                    message: "The worker's CLI stopped running before it reported.".into(),
                };
                self.watchdog_notice(
                    task,
                    "Its CLI stopped running before it reported. Another model takes over.".into(),
                )
                .await;
                self.hand_off_held(live, cutoff.unwrap_or(end)).await;
            }
        }
    }

    /// Detaches an exited CLI for a hand-off, if `verdict`
    /// (decided at `now` on the worker as `seen`) still holds, with its task as the board has it:
    /// still starting or running, no card or quota it waits for, nothing reported meanwhile
    /// (the report lock is held for the check). The caller holds `reroute`, so no other
    /// hand-on or hand-over runs meanwhile. Answers a pending cut-off (for another model);
    /// `None` when the action is no longer due.
    async fn take_exited(
        &self,
        live: &Arc<TaskLive>,
        verdict: StallVerdict,
        seen: &WorkerWatch,
        timing: &Timing,
        now: i64,
    ) -> Option<Option<AttemptEnd>> {
        let settled = live.settle.lock().await;
        let (state, quota_wait, card) = self.task_now(live).await?;
        let due = |current: &WorkerWatch| {
            // Judged at the round's time, as a report is: a CLI seen gone first by an earlier
            // look is as long gone as the round found it (#443).
            let verdict_now = stall_verdict(state, quota_wait, card, current, timing, now);
            still_due(verdict, seen, current, verdict_now)
        };
        let (cli, cutoff) = live.detach_stalled(due, false).await?;
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

    /// Lists the cards of an overnight run the user left unanswered for long under its
    /// "Waiting on you" (a session's card is in front of the user already).
    async fn watch_cards(
        &self,
        conversation_id: &ConversationId,
        board: &Board,
        timing: &Timing,
        now: i64,
    ) {
        for card in stuck_cards(board, now, timing.card) {
            let source = WaitingSource::Card {
                card_id: card.card_id.clone(),
            };
            if waiting_run(&source, card.request_id.as_deref(), board).is_none() {
                continue;
            }
            let added = self
                .wait_on_user(conversation_id, card.request_id.clone(), source, &card.what)
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
            (t.silent, t.command, t.grace, t.card),
            (3 * MIN, 5 * MIN, 2 * MIN, 30 * MIN)
        );
        assert_eq!(t.grace, 2 * MIN);
        assert_eq!(t.tick, Duration::from_secs(45));
        let fast = Timing::scaled(60);
        assert_eq!(
            (fast.silent, fast.command, fast.card, fast.grace),
            (MIN, 100_000, 10 * MIN, 40_000)
        );
        assert_eq!(fast.tick, Duration::from_secs(15));
        assert_eq!(Timing::scaled(1).tick, Duration::from_secs(5));
    }

    #[test]
    fn a_silent_worker_is_reported_once_without_recovery() {
        let mut watch = working();
        assert_eq!(verdict(TaskState::Running, &watch, 2 * MIN), None);
        assert_eq!(
            verdict(TaskState::Running, &watch, 3 * MIN),
            Some(StallVerdict::Report)
        );
        assert_eq!(
            verdict(TaskState::Starting, &watch, 3 * MIN),
            Some(StallVerdict::Report)
        );
        watch.reported_at_ms = Some(3 * MIN);
        watch.stalls = 2;
        assert_eq!(verdict(TaskState::Running, &watch, 300 * MIN), None);
        watch.last_event_ms = 301 * MIN;
        assert_eq!(
            verdict(TaskState::Running, &watch, 304 * MIN),
            Some(StallVerdict::Report)
        );
    }

    #[test]
    fn a_running_call_is_never_reported_or_restarted() {
        let watch = WorkerWatch {
            command_running: true,
            ..working()
        };
        assert_eq!(verdict(TaskState::Running, &watch, 24 * 60 * MIN), None);
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
            Some(StallVerdict::ReplaceExited)
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
    fn an_action_is_taken_only_while_it_still_holds() {
        let t = timing();
        let now = 5 * MIN;
        let seen = WorkerWatch {
            reported_at_ms: None,
            ..working()
        };
        let due = |current: &WorkerWatch, state: TaskState, quota_wait: bool, card: bool| {
            let verdict_now = stall_verdict(state, quota_wait, card, current, &t, now);
            still_due(StallVerdict::Report, &seen, current, verdict_now)
        };
        assert!(due(&seen, TaskState::Running, false, false));
        let changed = |change: fn(&mut WorkerWatch)| {
            let mut current = seen.clone();
            change(&mut current);
            current
        };
        for (what, current) in [
            // New progress can leave the verdict unchanged; its timestamp must match too.
            ("activity", changed(|w| w.last_event_ms = MIN)),
            (
                "another report",
                changed(|w| w.reported_at_ms = Some(5 * MIN)),
            ),
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
        let decided = StallVerdict::ReplaceExited;
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
    fn watchdog_running_servers_builds_and_plain_commands_have_no_deadline() {
        use super::progress::{CommandKind, command_kind};
        for command in ["pnpm dev", "cargo build", "sleep 999999"] {
            let kind = command_kind(command);
            let watch = WorkerWatch {
                command_running: true,
                command_kind: Some(kind),
                command_due_ms: Some(kind.deadline(&timing())),
                ..working()
            };
            assert!(matches!(
                kind,
                CommandKind::Server | CommandKind::Build | CommandKind::Normal
            ));
            assert_eq!(verdict(TaskState::Running, &watch, 24 * 60 * MIN), None);
        }
    }

    #[test]
    fn watchdog_long_builds_printing_or_quiet_under_thirty_minutes_are_not_stalled() {
        use brigadier_providers::ItemStatus;
        let mut evidence = progress::Progress::default();
        let mut watch = working();
        let event = ProviderEvent::Command {
            item_id: "c".into(),
            command: "cargo test".into(),
            cwd: None,
            status: ItemStatus::InProgress,
            exit_code: None,
            output: None,
            duration_ms: None,
            kept: None,
        };
        assert!(evidence.observe(&event, 0));
        watch.command_running = true;
        watch.command_kind = evidence.command();
        watch.command_due_ms = evidence.command_deadline(&timing());
        assert_eq!(verdict(TaskState::Running, &watch, 30 * MIN - 1), None);
        for minute in 1..=120 {
            let event = ProviderEvent::CommandOutputDelta {
                item_id: "c".into(),
                text: format!("test {minute} passed"),
            };
            assert!(evidence.observe(&event, minute * MIN));
            watch.last_event_ms = minute * MIN;
            watch.command_due_ms = evidence.command_deadline(&timing());
            assert_eq!(verdict(TaskState::Running, &watch, minute * MIN), None);
        }
    }

    #[test]
    fn watchdog_notice_and_status_chatter_still_stalls_at_three_minutes() {
        let mut evidence = progress::Progress::default();
        let mut watch = working();
        for minute in 1..=3 {
            for event in [
                ProviderEvent::Notice {
                    level: NoticeLevel::Info,
                    message: format!("status {minute}"),
                },
                ProviderEvent::Progress {
                    item_id: "heartbeat".into(),
                },
                ProviderEvent::BackgroundJobs {
                    running: vec!["job".into()],
                },
            ] {
                if evidence.observe(&event, minute * MIN) {
                    watch.last_event_ms = minute * MIN;
                }
            }
        }
        assert_eq!(watch.last_event_ms, 0);
        assert_eq!(
            verdict(TaskState::Running, &watch, 3 * MIN),
            Some(StallVerdict::Report)
        );
    }
}
