//! "Decided for you" and "Waiting on you": what the session's summary tells the user about
//! the work Brigadier runs on their behalf (built in, always on).
//!
//! - **Decided for you.** Under "Approve for me" and "Full access", each decision Brigadier
//!   takes in the user's place is logged with why: a plan approved, sent back for revision or
//!   turned down; a change landed after its checks, sent back with their findings, held
//!   because it couldn't be verified, or handed to the orchestrator; a permission a worker
//!   asked for declined. The orchestrator notes its own judgement calls (`note_for_user`), and
//!   the stall watchdog its actions ([`SessionManager::decided_for_task`]).
//! - **Waiting on you.** What only the user can do: a worker's `needs_user` lines, what a
//!   change's checks need from the user first, what the orchestrator notes, and cards left
//!   unanswered (the watchdog, with [`WaitingSource::Card`]). An item is listed once per
//!   source and text (in an overnight run, once per config key or criterion it names:
//!   `account_id`, `p2-c2`), under the request its task works for now, and keeps that request
//!   waiting while other work goes on. It is over when the user clicks Done (the orchestrator
//!   hears it), or without them: when its card settles, its task is stopped or reports again
//!   without it, the change it held lands or a later round of its checks no longer lists it,
//!   or the user edits its request or asks for a new answer. In an overnight run, only what a
//!   done-when criterion needs is listed, and marking an item done after its phase settled
//!   or its run ended wakes nobody.

use std::collections::HashSet;

use super::SessionManager;
use super::conversation::Envelope;
use crate::board::Board;
use crate::model::{ConversationId, DomainEvent, OvernightRunId, PermissionLevel};
use crate::sessions::one_line;
use crate::work::{
    CardId, CardState, Decision, DecisionKind, DecisionSource, DecisionWords, InjectionKind,
    PlanState, ResolvedBy, Task, TaskId, TaskState, WaitingItem, WaitingSource,
};
use crate::{Error, Result, now_ms};

/// The longest line a decision or a waiting item keeps.
const LINE_CHARS: usize = 300;
/// The longest reason a decision keeps.
const WHY_CHARS: usize = 600;

impl SessionManager {
    /// Logs a decision Brigadier took on the user's behalf. Under "Ask for approval" the user
    /// takes them, so nothing is logged.
    pub(crate) async fn decided_for_you(
        &self,
        conversation_id: &ConversationId,
        request_id: Option<String>,
        source: DecisionSource,
        what: String,
        why: String,
    ) {
        if self.permission(conversation_id) == PermissionLevel::AskForApproval {
            return;
        }
        self.record_decision(
            conversation_id,
            request_id,
            source,
            DecisionKind::Routine,
            what,
            why,
        )
        .await;
    }

    /// Logs a decision about a task (its landing, a fix round, a declined permission, a
    /// watchdog action), under the task's request.
    pub(crate) async fn decided_for_task(&self, task: &Task, what: String, why: String) {
        let request = self
            .request_for(&task.conversation_id, Some(&task.id))
            .await;
        self.decided_for_you(
            &task.conversation_id,
            request,
            DecisionSource::Task {
                task_id: task.id.clone(),
            },
            what,
            why,
        )
        .await;
    }

    /// Records a decision, whatever the permission level (the orchestrator's own notes).
    pub(crate) async fn record_decision(
        &self,
        conversation_id: &ConversationId,
        request_id: Option<String>,
        source: DecisionSource,
        kind: DecisionKind,
        what: String,
        why: String,
    ) {
        let what = self.core.display_quote(conversation_id, &what).await;
        let why = self.core.display_quote(conversation_id, &why).await;
        // What the user reads names a worker as the app shows it, never as "task-N".
        let (what, why) = match self.core.board(conversation_id).await {
            Ok(board) => (named(&what, &board), named(&why, &board)),
            Err(_) => (what, why),
        };
        let decision = Decision {
            id: uuid::Uuid::now_v7().to_string(),
            request_id,
            source,
            kind,
            what: one_line(&what, LINE_CHARS),
            why: one_line(&why, WHY_CHARS),
            at_ms: now_ms(),
            position: 0,
            short: None,
        };
        if let Err(err) = self
            .core
            .record_conversation(
                conversation_id,
                vec![DomainEvent::DecidedForYou { decision }],
            )
            .await
        {
            tracing::warn!(conversation = %conversation_id, error = %err, "could not record a decision");
        }
    }

    /// Lists something only the user can do, or rewords the open item with the same key (and
    /// files it under `request_id`, a later request that repeats it). Returns whether a new
    /// item was listed.
    pub(crate) async fn wait_on_user(
        &self,
        conversation_id: &ConversationId,
        request_id: Option<String>,
        source: WaitingSource,
        what: &str,
    ) -> Result<bool> {
        let what = self.core.display_quote(conversation_id, what).await;
        let what = one_line(&what, LINE_CHARS);
        if what.is_empty() {
            return Err(Error::Invalid("say what the user must do".into()));
        }
        let added = {
            let _held = self.waiting.lock().await;
            let board = self.core.board(conversation_id).await?;
            let what = named(&what, &board);
            let key = waiting_key(&source, &what);
            let open_same = board.waiting.values().find(|open| open.key == key);
            if open_same.is_none() && repeats_run_ask(&board, &source, request_id.as_deref(), &what)
            {
                return Ok(false);
            }
            let (item, added) = match open_same {
                Some(open) => {
                    let request_id = request_id.or_else(|| open.request_id.clone());
                    if open.what == what && open.request_id == request_id {
                        return Ok(false);
                    }
                    (
                        WaitingItem {
                            what,
                            request_id,
                            ..open.clone()
                        },
                        false,
                    )
                }
                None => (
                    WaitingItem {
                        id: uuid::Uuid::now_v7().to_string(),
                        request_id,
                        source,
                        key,
                        what,
                        created_at_ms: now_ms(),
                    },
                    true,
                ),
            };
            self.core
                .record_conversation(conversation_id, vec![DomainEvent::WaitingOnYou { item }])
                .await?;
            added
        };
        self.settle_requests(conversation_id).await;
        Ok(added)
    }

    /// A task reported what only the user can do (its `needs_user`, `source`
    /// [`WaitingSource::Task`]) or what its change's checks need from them first
    /// ([`WaitingSource::Landing`]): each line is listed once, and the task's open items of
    /// that source the new list no longer names are over. Nothing changes when the task moved
    /// on meanwhile (stopped, reported again, a newer round of checks). Returns how many lines
    /// are listed.
    pub(crate) async fn sync_waiting(
        &self,
        task: &Task,
        source: WaitingSource,
        lines: &[String],
    ) -> usize {
        let request = self
            .request_for(&task.conversation_id, Some(&task.id))
            .await;
        let mut shown = Vec::with_capacity(lines.len());
        for line in lines {
            shown.push(self.core.display_quote(&task.conversation_id, line).await);
        }
        let lines = &shown;
        let changed = {
            let _held = self.waiting.lock().await;
            let Ok(board) = self.core.board(&task.conversation_id).await else {
                return 0;
            };
            if !board
                .tasks
                .get(&task.id)
                .is_some_and(|now| lists_still(&source, task, now))
            {
                return 0;
            }
            let open: Vec<WaitingItem> = board
                .waiting
                .values()
                .filter(|item| item.source == source)
                .cloned()
                .collect();
            let lines = not_listed_by_run(&board, &source, request.as_deref(), lines);
            let (listed, gone) = report_waits(&source, &open, &lines, request, now_ms(), || {
                uuid::Uuid::now_v7().to_string()
            });
            let events: Vec<DomainEvent> = listed
                .into_iter()
                .map(|item| DomainEvent::WaitingOnYou { item })
                .chain(gone.into_iter().map(|id| DomainEvent::WaitingResolved {
                    id,
                    by: ResolvedBy::Brigadier,
                }))
                .collect();
            if events.is_empty() {
                false
            } else if let Err(err) = self
                .core
                .record_conversation(&task.conversation_id, events)
                .await
            {
                tracing::warn!(task = %task.id, error = %err, "could not list what waits for the user");
                false
            } else {
                true
            }
        };
        if changed {
            self.settle_requests(&task.conversation_id).await;
        }
        distinct_lines(lines).len()
    }

    /// A task ended: what its change's checks waited for is over, and what its reports listed
    /// is too when it was stopped.
    pub(crate) async fn task_ended_waiting(&self, task: &Task, state: TaskState) {
        if self
            .resolve_where(&task.conversation_id, |item| {
                ended_with(&item.source, &task.id, state)
            })
            .await
        {
            self.settle_requests(&task.conversation_id).await;
        }
    }

    /// After a restart: a task's end and the end of what it waited for are separate writes,
    /// as are a report and what it lists, so either may be missing. What ended tasks waited
    /// for is over, and what the current report of a live task lists is listed (never again
    /// what the user marked done).
    pub(crate) async fn reconcile_waiting(&self, conversation_id: &ConversationId) {
        let _held = self.waiting.lock().await;
        let Ok(mut board) = self.core.board(conversation_id).await else {
            return;
        };
        // Recovered worker reports also pass through the display path before line limits.
        for task in board.tasks.values_mut() {
            if let Some(report) = &mut task.report {
                for line in &mut report.needs_user {
                    *line = self.core.display_quote(conversation_id, line).await;
                }
            }
        }
        // A permission item from before the restart closes once the permissions are read
        // granted.
        if board
            .waiting
            .values()
            .any(|item| matches!(item.source, WaitingSource::Computer))
        {
            self.computer_wait_found(conversation_id);
        }
        let (listed, gone) =
            reconciled_waits(&board, now_ms(), || uuid::Uuid::now_v7().to_string());
        let events: Vec<DomainEvent> = listed
            .into_iter()
            .map(|item| DomainEvent::WaitingOnYou { item })
            .chain(gone.into_iter().map(|id| DomainEvent::WaitingResolved {
                id,
                by: ResolvedBy::Brigadier,
            }))
            .collect();
        if events.is_empty() {
            return;
        }
        if let Err(err) = self.core.record_conversation(conversation_id, events).await {
            tracing::warn!(conversation = %conversation_id, error = %err, "could not bring what waits for the user up to date");
        }
    }

    /// Phase `number` of an overnight run was verified: what this run, its earlier segments or
    /// their leads asked of the user for that phase is over (the answer reached the phase, for
    /// example in the words of a Continue).
    pub(crate) async fn phase_verified_waiting(
        &self,
        conversation_id: &ConversationId,
        run: &OvernightRunId,
        number: u32,
    ) {
        let Ok(board) = self.core.board(conversation_id).await else {
            return;
        };
        let mut lineage = HashSet::new();
        let mut at = Some(run.clone());
        while let Some(id) = at {
            at = board.runs.get(&id).and_then(|run| run.predecessor.clone());
            if !lineage.insert(id) {
                break;
            }
        }
        if self
            .resolve_where(conversation_id, |item| {
                asks_of_phase(&board, &lineage, item, number)
            })
            .await
        {
            self.settle_requests(conversation_id).await;
        }
    }

    /// The user edited a request or asked for a new answer: what its abandoned answer waited
    /// for is over. The caller settles the requests.
    pub(crate) async fn rework_waiting(&self, conversation_id: &ConversationId, request: &str) {
        self.resolve_where(conversation_id, |item| {
            item.request_id.as_deref() == Some(request)
        })
        .await;
    }

    /// Items whose card was answered or expired are over. Returns whether any was, before
    /// the conversation's requests are settled.
    pub(crate) async fn settle_card_waits(
        &self,
        conversation_id: &ConversationId,
        board: &Board,
    ) -> bool {
        let settled = |item: &WaitingItem| matches!(&item.source, WaitingSource::Card { card_id } if !card_open(board, card_id));
        if !board.waiting.values().any(settled) {
            return false;
        }
        self.resolve_where(conversation_id, settled).await
    }

    /// Marks the open items `over` picks done by Brigadier. Returns whether any was.
    async fn resolve_where(
        &self,
        conversation_id: &ConversationId,
        over: impl Fn(&WaitingItem) -> bool,
    ) -> bool {
        let _held = self.waiting.lock().await;
        let Ok(board) = self.core.board(conversation_id).await else {
            return false;
        };
        let events: Vec<DomainEvent> = board
            .waiting
            .values()
            .filter(|item| over(item))
            .map(|item| DomainEvent::WaitingResolved {
                id: item.id.clone(),
                by: ResolvedBy::Brigadier,
            })
            .collect();
        if events.is_empty() {
            return false;
        }
        match self.core.record_conversation(conversation_id, events).await {
            Ok(_) => true,
            Err(err) => {
                tracing::warn!(conversation = %conversation_id, error = %err, "could not resolve what waited for the user");
                false
            }
        }
    }

    /// Closes the conversation's open computer-permission items: the permissions are in.
    pub(crate) async fn resolve_computer_waits(&self, conversation_id: &ConversationId) {
        let _held = self.waiting.lock().await;
        let Ok(board) = self.core.board(conversation_id).await else {
            return;
        };
        let over: Vec<DomainEvent> = board
            .waiting
            .values()
            .filter(|item| matches!(item.source, WaitingSource::Computer))
            .map(|item| DomainEvent::WaitingResolved {
                id: item.id.clone(),
                by: ResolvedBy::Brigadier,
            })
            .collect();
        if over.is_empty() {
            return;
        }
        if let Err(err) = self.core.record_conversation(conversation_id, over).await {
            tracing::warn!(error = %err, "could not close the computer-permission item");
        }
    }

    /// The user marked an item done (Done on the summary card): the orchestrator hears it.
    pub async fn resolve_waiting(&self, conversation_id: ConversationId, id: String) -> Result<()> {
        let (item, board) = {
            let _held = self.waiting.lock().await;
            let board = self.core.board(&conversation_id).await?;
            let item = board
                .waiting
                .get(&id)
                .cloned()
                .ok_or_else(|| Error::Invalid("That item is already done.".into()))?;
            self.core
                .record_conversation(
                    &conversation_id,
                    vec![DomainEvent::WaitingResolved {
                        id,
                        by: ResolvedBy::User,
                    }],
                )
                .await?;
            (item, board)
        };
        let number = |task_id: &TaskId| board.tasks.get(task_id).map(|t| t.number);
        let next = match &item.source {
            WaitingSource::Task { task_id } => number(task_id)
                .map(|n| format!(" (task-{n} listed it as something only the user can do)"))
                .unwrap_or_default(),
            WaitingSource::Landing { task_id } => number(task_id)
                .map(|n| {
                    format!(" It waited for this to land: call land_phase for task-{n} again.")
                })
                .unwrap_or_default(),
            WaitingSource::Run {
                task_id: Some(task_id),
                ..
            } => number(task_id)
                .map(|n| format!(" (declined by the overnight rules for task-{n})"))
                .unwrap_or_default(),
            WaitingSource::Card { .. }
            | WaitingSource::Orchestrator
            | WaitingSource::Computer
            | WaitingSource::Run { task_id: None, .. } => String::new(),
        };
        // Clearing an item of a run that ended, or of a phase that settled, wakes nobody:
        // nothing is left to do with it (user decision, 2026-10-04).
        if !over_for_the_lead(&board, &item) {
            self.deliver_for(
                &conversation_id,
                Envelope {
                    kind: InjectionKind::Decision,
                    label: "user did".into(),
                    task_id: None,
                    text: format!("[decision] The user did: {}{next}", item.what),
                },
                item.request_id.clone(),
            )
            .await;
        }
        self.settle_requests(&conversation_id).await;
        Ok(())
    }
}

/// Whether the thread has nothing left to do with an item the user marked done: its overnight
/// run ended (or is writing its report).
fn over_for_the_lead(board: &Board, item: &WaitingItem) -> bool {
    let run = waiting_run(&item.source, item.request_id.as_deref(), board)
        .and_then(|run| board.runs.get(&run));
    let Some(run) = run else {
        return false;
    };
    run.state == crate::overnight::OvernightState::Reporting || !run.state.is_active()
}

/// Whether a card still waits for the user.
fn card_open(board: &Board, card: &CardId) -> bool {
    board
        .approvals
        .get(card)
        .is_some_and(|approval| approval.state == CardState::Pending)
        || board
            .questions
            .get(card)
            .is_some_and(|question| question.answer.is_none() && question.answered_at_ms.is_none())
        || board
            .plans
            .get(card)
            .is_some_and(|plan| plan.state == PlanState::Proposed)
}

/// What makes two items the same: their source, and their text in lower case, without
/// punctuation or bullets and with single spaces.
pub(crate) fn waiting_key(source: &WaitingSource, text: &str) -> String {
    let source = match source {
        WaitingSource::Card { card_id } => format!("card:{card_id}"),
        WaitingSource::Task { task_id } => format!("task:{task_id}"),
        WaitingSource::Landing { task_id } => format!("landing:{task_id}"),
        WaitingSource::Orchestrator => "orchestrator".to_owned(),
        WaitingSource::Computer => "computer".to_owned(),
        WaitingSource::Run { run_id, .. } => format!("run:{run_id}"),
    };
    let words: Vec<String> = text
        .split_whitespace()
        .map(|word| {
            word.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|word| !word.is_empty())
        .collect();
    format!("{source}|{}", words.join(" "))
}

/// Whether an item of `source` is over once `task` is in `state`: what a change's checks
/// waited for ends with the task, what its reports listed only when it was stopped.
fn ended_with(source: &WaitingSource, task: &TaskId, state: TaskState) -> bool {
    match source {
        WaitingSource::Landing { task_id } => task_id == task && state.is_final(),
        WaitingSource::Task { task_id } => task_id == task && state == TaskState::Stopped,
        // Kept for the run's report until the user marks it done.
        WaitingSource::Card { .. }
        | WaitingSource::Orchestrator
        | WaitingSource::Computer
        | WaitingSource::Run { .. } => false,
    }
}

/// Whether what `synced` (the task when it reported, or when its round of checks ended)
/// lists of `source` still speaks for the task as it is `now`: its items did not end with
/// it, and it has no newer report or round of checks.
fn lists_still(source: &WaitingSource, synced: &Task, now: &Task) -> bool {
    if ended_with(source, &now.id, now.state) {
        return false;
    }
    let submitted = |task: &Task| task.report.as_ref().map(|report| report.submitted_at_ms);
    match source {
        WaitingSource::Task { .. } => {
            submitted(synced).is_some() && submitted(synced) == submitted(now)
        }
        // Listed by the per-change checks, which are gone: nothing lists them again.
        WaitingSource::Landing { .. } => false,
        WaitingSource::Card { .. }
        | WaitingSource::Orchestrator
        | WaitingSource::Computer
        | WaitingSource::Run { .. } => true,
    }
}

/// What a restart makes of the open items: the items to list (what the current report of a
/// live task lists that was never listed) and the ids of items that are over (those its task
/// ended, and those its current report no longer names).
fn reconciled_waits(
    board: &Board,
    now: i64,
    mut new_id: impl FnMut() -> String,
) -> (Vec<WaitingItem>, Vec<String>) {
    let ended = |item: &WaitingItem| match &item.source {
        WaitingSource::Task { task_id } | WaitingSource::Landing { task_id } => board
            .tasks
            .get(task_id)
            .is_some_and(|task| ended_with(&item.source, task_id, task.state)),
        WaitingSource::Card { .. }
        | WaitingSource::Orchestrator
        | WaitingSource::Computer
        | WaitingSource::Run { .. } => false,
    };
    let mut gone: Vec<String> = board
        .waiting
        .values()
        .filter(|item| ended(item))
        .map(|item| item.id.clone())
        .collect();
    let mut listed = Vec::new();
    // A gate member's report goes to its gate, which lists what it needs (`Landing`).
    let live = board
        .tasks
        .values()
        .filter(|task| !task.state.is_final() && task.gate_link.is_none());
    for task in live {
        let Some(report) = &task.report else {
            continue;
        };
        let source = WaitingSource::Task {
            task_id: task.id.clone(),
        };
        let open: Vec<WaitingItem> = board
            .waiting
            .values()
            .filter(|item| item.source == source)
            .cloned()
            .collect();
        let lines: Vec<String> = distinct_lines(&report.needs_user)
            .into_iter()
            .filter(|line| {
                let key = waiting_key(&source, line);
                open.iter().any(|item| item.key == key) || !board.waits_listed.contains(&key)
            })
            .collect();
        let request = task
            .request_id
            .clone()
            .or_else(|| board.latest_request().map(|request| request.id.clone()));
        let lines = not_listed_by_run(board, &source, request.as_deref(), &lines);
        let (more, over) = report_waits(&source, &open, &lines, request, now, &mut new_id);
        listed.extend(more);
        gone.extend(over);
    }
    (listed, gone)
}

/// The longest worker name in a line the user reads, in characters.
const NAME_CHARS: usize = 40;

/// A worker as the user knows it: its title, as on its row, in quotes and cut short at a word
/// ("“Fix the A/B breakdown note after…”").
pub(crate) fn worker_name(task: &Task) -> String {
    let title = task.title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.chars().count() <= NAME_CHARS {
        return format!("\u{201c}{title}\u{201d}");
    }
    let cut: String = title.chars().take(NAME_CHARS).collect();
    let cut = match cut.rfind(' ') {
        Some(at) if at > NAME_CHARS / 2 => &cut[..at],
        _ => cut.as_str(),
    };
    format!(
        "\u{201c}{}\u{2026}\u{201d}",
        cut.trim_end_matches([' ', ',', ':', ';', '-', '·'])
    )
}

/// `text` with each "task-N" of the conversation's tasks as the worker's name. A title quoted
/// right after it ("task-3 “Add the flag”") becomes that name; "task-N" inside a branch, path,
/// file name or longer word stays as it is.
pub(crate) fn named(text: &str, board: &Board) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("task-") {
        let before = rest[..at].chars().next_back();
        let digits: String = rest[at + 5..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        let mut tail = rest[at + 5 + digits.len()..].chars();
        let after = tail.next();
        // A file name's extension ("task-3.diff") makes it a longer word too.
        let extension = after == Some('.') && tail.next().is_some_and(char::is_alphanumeric);
        let word = before.is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '/' | '-' | '_')))
            && !digits.is_empty()
            && !extension
            && after.is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '-' | '_' | '/')));
        let task = word
            .then(|| digits.parse::<u32>().ok())
            .flatten()
            .and_then(|number| board.tasks.values().find(|task| task.number == number));
        let Some(task) = task else {
            let end = at + 5;
            out.push_str(&rest[..end]);
            rest = &rest[end..];
            continue;
        };
        out.push_str(&rest[..at]);
        out.push_str(&worker_name(task));
        rest = &rest[at + 5 + digits.len()..];
        // Its title quoted after it is the name already.
        if let Some(quoted) = rest.strip_prefix(" \u{201c}")
            && let Some(close) = quoted.find('\u{201d}')
        {
            rest = &quoted[close + '\u{201d}'.len_utf8()..];
        }
    }
    out.push_str(rest);
    out
}

/// A decision as the app and the morning report show it. A decision recorded in an earlier
/// version's words (which carried the checks' findings) says the same in the current short
/// ones; the findings stay on the task's checks. One recorded now is returned as it is.
pub(crate) fn short_words(what: &str, why: &str) -> DecisionWords {
    let (what, why) = (what.trim(), why.trim());
    let words = |what: String, why: &str| DecisionWords {
        what,
        why: why.to_owned(),
    };
    const ON_ITS_CHECKS: &str = "The findings are on its checks.";
    if let Some(rest) = what.strip_prefix("Sent task-")
        && let Some((number, tail)) = rest.split_once(" back to fix what its checks found")
    {
        let from = match (
            why.contains("From the review"),
            why.contains("From the verification"),
        ) {
            (true, true) => "review and verification",
            (true, false) => "review",
            (false, true) => "verification",
            (false, false) => "its checks",
        };
        return words(
            format!("Sent task-{number} back after {from}{}", tail.trim_end()),
            ON_ITS_CHECKS,
        );
    }
    if let Some(rest) = what.strip_prefix("Did not land task-") {
        if let Some(number) = rest.strip_suffix(": the orchestrator decides what happens next") {
            let reason = if let Some(rounds) = why
                .strip_prefix("The problems were still there after ")
                .and_then(|rest| rest.split_once('.'))
                .map(|(rounds, _)| rounds)
            {
                format!("problems left after {rounds}")
            } else if why.starts_with("Its change is the same one") {
                "the fix changed nothing".into()
            } else {
                "its checks found problems".into()
            };
            return words(
                format!("Didn't land task-{number}: {reason}"),
                "The orchestrator decides what happens next; the findings are on its checks.",
            );
        }
        if let Some(number) = rest.strip_suffix(": its checks could not finish") {
            return words(
                format!("Didn't land task-{number}: its checks couldn't finish"),
                &reason_head(why),
            );
        }
    }
    if let Some(rest) = what.strip_prefix("Held task-")
        && let Some(held) = rest.strip_suffix(": its change could not be verified")
    {
        let number = held.split_once(' ').map_or(held, |(number, _)| number);
        return words(
            format!("Held task-{number}: its change couldn't be verified"),
            &reason_head(why),
        );
    }
    if what.starts_with("Landed ") {
        if let Some(rest) = why.strip_prefix("Its change passed independent checks: ") {
            let reviewed = if rest.starts_with("reviewed by another vendor") {
                "Reviewed by another vendor"
            } else if rest.starts_with("reviewed by another model of the same vendor") {
                "Reviewed by the same vendor (the only one available)"
            } else {
                "Reviewed"
            };
            let rounds = if rest.contains(", after one round of fixes") {
                ", after 1 fix round".to_owned()
            } else {
                rest.split_once(", after ")
                    .and_then(|(_, after)| after.split_once(" rounds of fixes"))
                    .map(|(rounds, _)| format!(", after {rounds} fix rounds"))
                    .unwrap_or_default()
            };
            return words(
                what.to_owned(),
                &format!("{reviewed} and verified{rounds}."),
            );
        }
        if why.starts_with("Landed on the user's word despite") {
            return words(what.to_owned(), "Landed despite its checks' findings.");
        }
    }
    if let Some(plan) = what
        .strip_prefix("Sent the plan ")
        .and_then(|rest| rest.strip_suffix(" back for revision"))
    {
        return words(
            format!("Sent the plan {plan} back after review"),
            "The findings are on the plan card.",
        );
    }
    if what.starts_with("Did not approve the plan ") {
        let what = what.replacen("Did not approve", "Didn't approve", 1);
        if let Some(rounds) = why
            .strip_prefix("It still had problems after ")
            .and_then(|rest| rest.split_once(';'))
            .map(|(rounds, _)| rounds)
        {
            return words(
                what,
                &format!(
                    "Problems left after {rounds}. The orchestrator asks you or makes it smaller."
                ),
            );
        }
        if why.starts_with("Its independent review could not run") {
            return words(what, "Its review couldn't run.");
        }
    }
    if let Some(number) = what
        .strip_prefix("Sent phase ")
        .and_then(|rest| rest.strip_suffix(" back to its lead to fix what its checks found"))
    {
        let round = why
            .strip_prefix("Fix round ")
            .map(|round| format!(" (fix {})", round.trim_end_matches('.')))
            .unwrap_or_default();
        return words(
            format!("Sent phase {number} back to its lead{round}"),
            "The findings are on the phase's checks.",
        );
    }
    if what.starts_with("Verified phase ")
        && let Some(count) = why
            .strip_prefix("A fresh verifier showed each of its ")
            .and_then(|rest| rest.split_once(' '))
            .map(|(count, _)| count)
    {
        return words(
            what.to_owned(),
            &format!(
                "All {count} criteria met. A fresh verifier, another vendor's reviewer and a judge agreed."
            ),
        );
    }
    if what.starts_with("Settled phase ")
        && let Some((met, _)) = why.split_once(" Still missing: ")
    {
        return words(
            what.to_owned(),
            &format!("{met} What's missing is in the phase's checks."),
        );
    }
    if what.starts_with("Approved the overnight plan ")
        && let Some(count) = why
            .strip_prefix("Another vendor reviewed its ")
            .and_then(|rest| rest.split_once(" phases and a fresh judge"))
            .map(|(count, _)| count)
    {
        return words(
            what.to_owned(),
            &format!(
                "Another vendor reviewed its {count} phases; a judge found they follow the goal."
            ),
        );
    }
    words(what.to_owned(), why)
}

/// Brigadier's own words at the head of a reason, without the check's text after them:
/// "Nothing lands unverified. The project's checks could not run." from "… could not run:
/// [not run] CI desktop: …".
fn reason_head(why: &str) -> String {
    let head = why.split_once(": ").map_or(why, |(head, _)| head).trim();
    let head = head.trim_start_matches("- ").trim_end_matches('.');
    if head.is_empty() {
        String::new()
    } else {
        format!("{head}.")
    }
}

/// The overnight run an item belongs to: its own, its task's, or its request's.
pub(crate) fn waiting_run(
    source: &WaitingSource,
    request_id: Option<&str>,
    board: &Board,
) -> Option<OvernightRunId> {
    match source {
        WaitingSource::Run { run_id, .. } => Some(run_id.clone()),
        WaitingSource::Task { task_id } | WaitingSource::Landing { task_id } => board
            .tasks
            .get(task_id)?
            .run
            .as_ref()
            .map(|context| context.run_id.clone()),
        WaitingSource::Computer => None,
        WaitingSource::Card { .. } | WaitingSource::Orchestrator => {
            let request = request_id?;
            board
                .runs
                .keys()
                .find(|run| request.starts_with(&format!("run-{}-", run.short())))
                .cloned()
        }
    }
}

/// What an ask is about: the phase criteria it names ("p2-c2") and the config keys, the
/// identifiers with an underscore (`account_id`, `STRIPE_KEY`), in lower case.
#[derive(Default)]
struct AskNames {
    criteria: HashSet<String>,
    keys: HashSet<String>,
}

impl AskNames {
    fn of(text: &str) -> Self {
        let mut names = Self::default();
        for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')) {
            let word = word.trim_matches(['-', '_']).to_ascii_lowercase();
            let criterion = word
                .strip_prefix('p')
                .and_then(|rest| rest.split_once("-c"))
                .is_some_and(|(phase, criterion)| {
                    [phase, criterion].iter().all(|number| {
                        !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
                    })
                });
            if criterion {
                names.criteria.insert(word);
            } else if word.contains('_')
                && !word.contains('-')
                && word.chars().any(|c| c.is_ascii_alphabetic())
            {
                names.keys.insert(word);
            }
        }
        names
    }

    fn extend(&mut self, other: Self) {
        self.criteria.extend(other.criteria);
        self.keys.extend(other.keys);
    }

    /// Whether `listed` already asks for all of this: every config key it names, or every
    /// criterion.
    fn within(&self, listed: &Self) -> bool {
        (!self.keys.is_empty() && self.keys.is_subset(&listed.keys))
            || (!self.criteria.is_empty() && self.criteria.is_subset(&listed.criteria))
    }
}

/// What the run's lasting open items ask for (the run's and its lead's: a task's are over
/// when it stops), from any source or only other ones.
fn run_asks(board: &Board, run: &OvernightRunId, except: Option<&WaitingSource>) -> AskNames {
    let mut names = AskNames::default();
    for open in board.waiting.values().filter(|open| {
        matches!(
            open.source,
            WaitingSource::Run { .. } | WaitingSource::Orchestrator
        ) && except != Some(&open.source)
            && waiting_run(&open.source, open.request_id.as_deref(), board).as_ref() == Some(run)
    }) {
        names.extend(AskNames::of(&open.what));
    }
    names
}

/// Whether `item` is a lasting ask of a run in `lineage` about phase `number` alone: listed
/// for that phase by its checks ("Phase 2: …"), or naming only that phase's criteria.
fn asks_of_phase(
    board: &Board,
    lineage: &HashSet<OvernightRunId>,
    item: &WaitingItem,
    number: u32,
) -> bool {
    if !matches!(
        item.source,
        WaitingSource::Run { .. } | WaitingSource::Orchestrator
    ) || !waiting_run(&item.source, item.request_id.as_deref(), board)
        .is_some_and(|run| lineage.contains(&run))
    {
        return false;
    }
    let criteria = AskNames::of(&item.what).criteria;
    let prefix = format!("p{number}-c");
    criteria
        .iter()
        .all(|criterion| criterion.starts_with(&prefix))
        && (!criteria.is_empty() || item.what.starts_with(&format!("Phase {number}: ")))
}

/// In an overnight run, asks for the same config keys or criteria are one ask, however the
/// lead, a worker, a verifier and the judge word it: whether the run already lists this one.
fn repeats_run_ask(
    board: &Board,
    source: &WaitingSource,
    request_id: Option<&str>,
    what: &str,
) -> bool {
    waiting_run(source, request_id, board)
        .is_some_and(|run| AskNames::of(what).within(&run_asks(board, &run, None)))
}

/// A source's lines without the asks its run already lists from another source, or an
/// earlier line makes.
fn not_listed_by_run(
    board: &Board,
    source: &WaitingSource,
    request_id: Option<&str>,
    lines: &[String],
) -> Vec<String> {
    let Some(run) = waiting_run(source, request_id, board) else {
        return lines.to_vec();
    };
    let mut listed = run_asks(board, &run, Some(source));
    lines
        .iter()
        .filter(|line| {
            let names = AskNames::of(line);
            let repeat = names.within(&listed);
            listed.extend(names);
            !repeat
        })
        .cloned()
        .collect()
}

/// A report's lines, trimmed and on one line each, without blanks or repeats of one item.
fn distinct_lines(lines: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    lines
        .iter()
        .map(|line| one_line(line.trim().trim_start_matches(['-', '*', ' ']), LINE_CHARS))
        .filter(|line| !line.is_empty())
        .filter(|line| seen.insert(waiting_key(&WaitingSource::Orchestrator, line)))
        .collect()
}

/// What a source's new list of lines makes of its open items: the items to list (new ones,
/// and open ones reworded or now filed under `request_id`, the request the task works for
/// now) and the ids of open items it no longer names.
fn report_waits(
    source: &WaitingSource,
    open: &[WaitingItem],
    lines: &[String],
    request_id: Option<String>,
    now: i64,
    mut new_id: impl FnMut() -> String,
) -> (Vec<WaitingItem>, Vec<String>) {
    let mut listed = Vec::new();
    let mut named = std::collections::HashSet::new();
    for what in distinct_lines(lines) {
        let key = waiting_key(source, &what);
        named.insert(key.clone());
        match open.iter().find(|item| item.key == key) {
            Some(item) => {
                let request_id = request_id.clone().or_else(|| item.request_id.clone());
                if item.what != what || item.request_id != request_id {
                    listed.push(WaitingItem {
                        what,
                        request_id,
                        ..item.clone()
                    });
                }
            }
            None => listed.push(WaitingItem {
                id: new_id(),
                request_id: request_id.clone(),
                source: source.clone(),
                key,
                what,
                created_at_ms: now,
            }),
        }
    }
    let gone = open
        .iter()
        .filter(|item| !named.contains(&item.key))
        .map(|item| item.id.clone())
        .collect();
    (listed, gone)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The app's fixture of the night of 2026-10-03 holds its decisions as the board serves
    /// them, with the short words. After extracting it again, add each decision's `short` from
    /// what this test prints.
    #[test]
    fn the_nights_fixture_carries_the_boards_short_words() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../apps/desktop/src/fixtures/boards/overnight-2026-10-03.json"
        ))
        .expect("the fixture");
        let decisions: Vec<Decision> =
            serde_json::from_value(fixture["decisions"].clone()).expect("decisions");
        let expected: Vec<(String, DecisionWords)> = decisions
            .iter()
            .map(|decision| {
                (
                    decision.id.clone(),
                    short_words(&decision.what, &decision.why),
                )
            })
            .collect();
        let found: Vec<(String, DecisionWords)> = decisions
            .iter()
            .filter_map(|decision| Some((decision.id.clone(), decision.short.clone()?)))
            .collect();
        assert!(
            found == expected,
            "{}",
            serde_json::to_string(&expected).expect("json")
        );
    }

    #[test]
    fn a_decision_in_old_words_reads_as_one_recorded_now() {
        for (what, why, short_what, short_why) in [
            (
                "Sent task-5 back to fix what its checks found (fix 1 of 2)",
                "From the review (task-7, Codex gpt-6.1-sol): F1: docs/x.md:132 must say …",
                "Sent task-5 back after review (fix 1 of 2)",
                "The findings are on its checks.",
            ),
            (
                "Did not land task-1: the orchestrator decides what happens next",
                "The problems were still there after 1 fix round. From the review (task-2): P2 …",
                "Didn't land task-1: problems left after 1 fix round",
                "The orchestrator decides what happens next; the findings are on its checks.",
            ),
            (
                "Did not land task-4: its checks could not finish",
                "The project's checks could not run: [not run] CI desktop: …",
                "Didn't land task-4: its checks couldn't finish",
                "The project's checks could not run.",
            ),
            (
                "Held task-13 “Make cause 1's fix obey §10.6”: its change could not be verified",
                "Nothing lands unverified. The project's checks could not run: [not run] CI: …",
                "Held task-13: its change couldn't be verified",
                "Nothing lands unverified. The project's checks could not run.",
            ),
            (
                "Landed task-32 “Landing: read entries” on `overnight/x`",
                "Its change passed independent checks: reviewed by another vendor, and verified against each \"done when\" criterion, after one round of fixes.",
                "Landed task-32 “Landing: read entries” on `overnight/x`",
                "Reviewed by another vendor and verified, after 1 fix round.",
            ),
            (
                "Landed task-9 “X” on `b`",
                "Landed on the user's word despite: F1: …",
                "Landed task-9 “X” on `b`",
                "Landed despite its checks' findings.",
            ),
            (
                "Sent the plan “Phase 2 · Fix the causes” back for revision",
                "Its independent review asked for changes. F1: Replace tools/full-checks.sh …",
                "Sent the plan “Phase 2 · Fix the causes” back after review",
                "The findings are on the plan card.",
            ),
            (
                "Did not approve the plan “Y”",
                "It still had problems after 3 review rounds; the orchestrator asks you or makes it smaller. F1: …",
                "Didn't approve the plan “Y”",
                "Problems left after 3 review rounds. The orchestrator asks you or makes it smaller.",
            ),
            (
                "Sent phase 2 back to its lead to fix what its checks found",
                "Fix round 1 of 2.",
                "Sent phase 2 back to its lead (fix 1 of 2)",
                "The findings are on the phase's checks.",
            ),
            (
                "Verified phase 1 “Measure”",
                "A fresh verifier showed each of its 1 criteria met, a reviewer from another vendor approved the whole diff, and a fresh judge agreed.",
                "Verified phase 1 “Measure”",
                "All 1 criteria met. A fresh verifier, another vendor's reviewer and a judge agreed.",
            ),
            (
                "Settled phase 3 “Ship” as partial",
                "2 of 3 criteria met. Still missing: p3-c2: the smoke …",
                "Settled phase 3 “Ship” as partial",
                "2 of 3 criteria met. What's missing is in the phase's checks.",
            ),
            (
                "Approved the overnight plan “Speed”",
                "Another vendor reviewed its 3 phases and a fresh judge found they follow the goal without invented scope.",
                "Approved the overnight plan “Speed”",
                "Another vendor reviewed its 3 phases; a judge found they follow the goal.",
            ),
            (
                "Kept the old API",
                "The orchestrator's own words stay as they are.",
                "Kept the old API",
                "The orchestrator's own words stay as they are.",
            ),
        ] {
            let short = short_words(what, why);
            assert_eq!(
                (short.what.as_str(), short.why.as_str()),
                (short_what, short_why)
            );
            // The current words read as they are.
            assert_eq!(short_words(&short.what, &short.why), short);
        }
    }

    #[test]
    fn a_worker_is_named_as_the_app_shows_it() {
        let mut board = Board::default();
        let mut short = task("t1", TaskState::Landed, None);
        short.number = 3;
        let mut long = task("t2", TaskState::Stopped, None);
        long.number = 37;
        long.title = "Gate members take run slots in a fixed priority order".into();
        board.tasks.insert(short.id.clone(), short);
        board.tasks.insert(long.id.clone(), long);
        assert_eq!(
            named(
                "Landed task-3 \u{201c}Add the flag\u{201d} on `main`",
                &board
            ),
            "Landed \u{201c}Add the flag\u{201d} on `main`"
        );
        assert_eq!(
            named("Landed task-3.", &board),
            "Landed \u{201c}Add the flag\u{201d}."
        );
        assert_eq!(
            named("Sent task-37 back (task-37's fix 1 of 2).", &board),
            "Sent \u{201c}Gate members take run slots in a fixed\u{2026}\u{201d} back (\u{201c}Gate members take run slots in a fixed\u{2026}\u{201d}'s fix 1 of 2)."
        );
        // Branches, paths and unknown numbers stay as they are.
        for kept in [
            "on `brigadier/4158464b/task-3-add-the-flag`",
            "see task-30 and subtask-3 and task-3x",
            "read task-3.diff and docs/task-3.md",
            "task-",
        ] {
            assert_eq!(named(kept, &board), kept);
        }
    }

    fn task_source() -> WaitingSource {
        WaitingSource::Task {
            task_id: TaskId("t1".into()),
        }
    }

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|line| (*line).to_owned()).collect()
    }

    fn ids() -> impl FnMut() -> String {
        let mut next = 0;
        move || {
            next += 1;
            format!("w{next}")
        }
    }

    #[test]
    fn the_key_ignores_case_punctuation_and_spacing_but_not_the_source() {
        let source = task_source();
        assert_eq!(
            waiting_key(&source, "Set STRIPE_KEY in `.env`."),
            waiting_key(&source, "- set  stripe_key in .env")
        );
        assert_ne!(
            waiting_key(&source, "Set STRIPE_KEY in .env"),
            waiting_key(&WaitingSource::Orchestrator, "Set STRIPE_KEY in .env")
        );
        assert_ne!(
            waiting_key(&source, "Sign in to npm"),
            waiting_key(&source, "Sign in to GitHub")
        );
    }

    #[test]
    fn a_report_lists_each_line_once() {
        let (listed, gone) = report_waits(
            &task_source(),
            &[],
            &lines(&[
                "Set STRIPE_KEY in .env",
                "- set stripe_key in .env.",
                "  ",
                "Push the branch",
            ]),
            Some("r1".into()),
            5,
            ids(),
        );
        assert!(gone.is_empty());
        let what: Vec<&str> = listed.iter().map(|item| item.what.as_str()).collect();
        assert_eq!(what, ["Set STRIPE_KEY in .env", "Push the branch"]);
        assert!(
            listed
                .iter()
                .all(|item| item.request_id.as_deref() == Some("r1"))
        );
        assert_eq!(listed[0].id, "w1");
        assert_eq!(listed[0].created_at_ms, 5);
    }

    #[test]
    fn a_repeat_report_keeps_the_item_and_one_that_drops_it_ends_it() {
        let source = task_source();
        let (open, _) = report_waits(
            &source,
            &[],
            &lines(&["Set STRIPE_KEY in .env", "Push the branch"]),
            None,
            1,
            ids(),
        );
        // The same lines again: nothing new.
        let (listed, gone) = report_waits(
            &source,
            &open,
            &lines(&["Set STRIPE_KEY in .env", "Push the branch"]),
            None,
            2,
            ids(),
        );
        assert!(listed.is_empty() && gone.is_empty());
        // Reworded: the open item is updated in place.
        let (listed, gone) = report_waits(
            &source,
            &open,
            &lines(&["set stripe_key in .env!", "Push the branch"]),
            None,
            2,
            ids(),
        );
        assert!(gone.is_empty());
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, open[0].id);
        assert_eq!(listed[0].created_at_ms, 1);
        assert_eq!(listed[0].what, "set stripe_key in .env!");
        // A later report without it: it is over.
        let (listed, gone) =
            report_waits(&source, &open, &lines(&["Push the branch"]), None, 3, ids());
        assert!(listed.is_empty());
        assert_eq!(gone, [open[0].id.clone()]);
        let (_, gone) = report_waits(&source, &open, &[], None, 3, ids());
        assert_eq!(gone.len(), 2);
    }

    #[test]
    fn a_repeat_under_a_later_request_moves_the_item_to_it() {
        let source = task_source();
        let (open, _) = report_waits(
            &source,
            &[],
            &lines(&["Set STRIPE_KEY in .env"]),
            Some("r1".into()),
            1,
            ids(),
        );
        // The same text, now that the task works for r2: the item is filed under r2.
        let (listed, gone) = report_waits(
            &source,
            &open,
            &lines(&["Set STRIPE_KEY in .env"]),
            Some("r2".into()),
            2,
            ids(),
        );
        assert!(gone.is_empty());
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, open[0].id);
        assert_eq!(listed[0].request_id.as_deref(), Some("r2"));
        // No request to file it under: it stays where it was.
        let (listed, _) = report_waits(
            &source,
            &open,
            &lines(&["Set STRIPE_KEY in .env"]),
            None,
            2,
            ids(),
        );
        assert!(listed.is_empty());
    }

    /// Task `id` at `state`, with a report submitted at `reported` listing `needs_user`.
    fn task(id: &str, state: TaskState, reported: Option<(i64, &[&str])>) -> Task {
        let mut task: Task = serde_json::from_value(serde_json::json!({
            "id": id,
            "conversationId": "c1",
            "number": 1,
            "position": 0,
            "title": "Add the flag",
            "kind": "implement",
            "spec": "Add the flag.",
            "access": { "repo": "write", "network": false, "unsandboxed": false },
            "route": { "choice": { "provider": "claude", "model": null, "effort": null }, "reason": "" },
            "state": "reported",
            "requestId": "r1",
            "attachments": [],
            "createdAtMs": 0,
            "updatedAtMs": 0
        }))
        .expect("a task");
        task.state = state;
        task.report = reported.map(|(at, needs_user)| {
            serde_json::from_value(serde_json::json!({
                "summary": "Done",
                "changes": [],
                "decisions": [],
                "verification": [],
                "doneWhen": [],
                "openQuestions": [],
                "risks": [],
                "needsUser": needs_user,
                "artifacts": [],
                "submittedAtMs": at,
            }))
            .expect("a report")
        });
        task
    }

    #[test]
    fn a_report_lists_its_waits_only_while_it_is_the_tasks_latest() {
        let source = task_source();
        let reported = task("t1", TaskState::Reported, Some((5, &["Sign in to npm"])));
        assert!(lists_still(&source, &reported, &reported));
        // A read task done after its report: what it listed still waits for the user.
        let done = task("t1", TaskState::Done, Some((5, &["Sign in to npm"])));
        assert!(lists_still(&source, &reported, &done));
        // Stopped meanwhile, or reported again: this report lists nothing any more.
        let stopped = task("t1", TaskState::Stopped, Some((5, &["Sign in to npm"])));
        assert!(!lists_still(&source, &reported, &stopped));
        let again = task("t1", TaskState::Reported, Some((9, &[])));
        assert!(!lists_still(&source, &reported, &again));
        assert!(!lists_still(
            &source,
            &task("t1", TaskState::Reported, None),
            &task("t1", TaskState::Reported, None)
        ));
    }

    fn item(id: &str, source: WaitingSource, what: &str) -> WaitingItem {
        WaitingItem {
            id: id.into(),
            request_id: Some("r1".into()),
            key: waiting_key(&source, what),
            source,
            what: what.into(),
            created_at_ms: 1,
        }
    }

    fn listed(board: &mut Board, item: WaitingItem) {
        board.apply(&DomainEvent::WaitingOnYou { item }, 1);
    }

    #[test]
    fn a_restart_ends_what_ended_tasks_waited_for() {
        let mut board = Board::default();
        for task in [
            task(
                "stopped",
                TaskState::Stopped,
                Some((1, &["Sign in to npm"])),
            ),
            task("landed", TaskState::Landed, Some((1, &[]))),
            task("read", TaskState::Done, Some((1, &["Push the branch"]))),
        ] {
            board.tasks.insert(task.id.clone(), task);
        }
        let of = |id: &str| TaskId(id.into());
        listed(
            &mut board,
            item(
                "w1",
                WaitingSource::Task {
                    task_id: of("stopped"),
                },
                "Sign in to npm",
            ),
        );
        listed(
            &mut board,
            item(
                "w2",
                WaitingSource::Landing {
                    task_id: of("landed"),
                },
                "Add the API key",
            ),
        );
        listed(
            &mut board,
            item(
                "w3",
                WaitingSource::Task {
                    task_id: of("read"),
                },
                "Push the branch",
            ),
        );
        listed(
            &mut board,
            item("w4", WaitingSource::Orchestrator, "Create an account"),
        );
        let (more, mut gone) = reconciled_waits(&board, 2, ids());
        gone.sort();
        assert!(more.is_empty());
        // A stopped task's report and a landed change's checks are over; what a finished
        // read task listed, and the orchestrator's note, still wait for the user.
        assert_eq!(gone, ["w1", "w2"]);
    }

    #[test]
    fn a_restart_lists_what_a_live_report_could_not() {
        let mut board = Board::default();
        let reported = task(
            "t1",
            TaskState::Reported,
            Some((1, &["Sign in to npm", "Push the branch", "Set STRIPE_KEY"])),
        );
        let mut member = task("t2", TaskState::Reported, Some((1, &["Add the API key"])));
        member.gate_link = Some(crate::work::GateLink {
            owner: crate::work::GateOwner::Task {
                task_id: TaskId("t1".into()),
            },
            round: 1,
            role: crate::work::GateRole::Verify,
        });
        for task in [reported, member] {
            board.tasks.insert(task.id.clone(), task);
        }
        let source = task_source();
        // Still open; listed once and marked done by the user; and one the current report
        // no longer names.
        listed(&mut board, item("w1", source.clone(), "Sign in to npm"));
        listed(&mut board, item("w2", source.clone(), "Push the branch"));
        board.apply(
            &DomainEvent::WaitingResolved {
                id: "w2".into(),
                by: ResolvedBy::User,
            },
            2,
        );
        listed(&mut board, item("w3", source.clone(), "Restart the server"));
        let (more, gone) = reconciled_waits(&board, 7, ids());
        assert_eq!(gone, ["w3"]);
        // Only the line never listed is new, under the task's request; nothing for the gate
        // member, whose report goes to its gate.
        assert_eq!(more.len(), 1);
        assert_eq!(more[0].what, "Set STRIPE_KEY");
        assert_eq!(more[0].source, source);
        assert_eq!(more[0].request_id.as_deref(), Some("r1"));
        assert_eq!(more[0].created_at_ms, 7);
        // Once listed, a second restart changes nothing.
        listed(&mut board, more[0].clone());
        board.apply(
            &DomainEvent::WaitingResolved {
                id: "w3".into(),
                by: ResolvedBy::Brigadier,
            },
            3,
        );
        assert_eq!(reconciled_waits(&board, 8, ids()), (Vec::new(), Vec::new()));
    }

    #[test]
    fn clearing_an_item_of_an_ended_run_wakes_nobody() {
        use crate::overnight::{OvernightRun, OvernightState};
        let mut run = OvernightRun::for_test(ConversationId("c".into()), "Speed", Vec::new());
        let request = format!("run-{}-g1", run.id.short());
        let mut board = Board::default();
        board.runs.insert(run.id.clone(), run.clone());
        let mut ask = item(
            "w1",
            WaitingSource::Run {
                run_id: run.id.clone(),
                task_id: None,
            },
            "Set account_id.",
        );
        ask.request_id = Some(request);
        // The run still works: the thread hears it.
        assert!(!over_for_the_lead(&board, &ask));
        // The run writes its report, or finished: nobody.
        for state in [OvernightState::Reporting, OvernightState::Finished] {
            run.state = state;
            board.runs.insert(run.id.clone(), run.clone());
            assert!(over_for_the_lead(&board, &ask), "{state:?}");
        }
        // An item of no run always reaches the lead.
        assert!(!over_for_the_lead(
            &board,
            &item("w2", WaitingSource::Orchestrator, "Sign in to npm.")
        ));
    }

    #[test]
    fn a_card_item_is_open_only_while_its_card_waits() {
        use crate::work::{Question, QuestionKind};
        let card = CardId("q1".into());
        let mut board = Board::default();
        assert!(!card_open(&board, &card));
        let mut question = Question {
            id: card.clone(),
            conversation_id: ConversationId("c1".into()),
            task_id: None,
            request_id: None,
            position: 0,
            kind: QuestionKind::Orchestrator,
            text: "Which region?".into(),
            options: Vec::new(),
            recommended: None,
            items: Vec::new(),
            answer: None,
            answers: Vec::new(),
            created_at_ms: 0,
            answered_at_ms: None,
        };
        board.questions.insert(card.clone(), question.clone());
        assert!(card_open(&board, &card));
        question.answer = Some("eu".into());
        board.questions.insert(card.clone(), question);
        assert!(!card_open(&board, &card));
    }

    #[test]
    fn a_run_lists_one_ask_per_key_or_criterion_however_it_is_worded() {
        // The live runs' wordings of one ask, from their leads, workers, verifiers and judges.
        let run = OvernightRunId("01a0fde6-run".into());
        let phase = |task: &str| WaitingSource::Run {
            run_id: run.clone(),
            task_id: Some(TaskId(task.into())),
        };
        let mut board = Board::default();
        let lead = item(
            "w1",
            phase("verifier"),
            "Set account_id in the [account] section of ledger.ini (left empty; only you have it).",
        );
        board.apply(&DomainEvent::WaitingOnYou { item: lead }, 1);
        for repeat in [
            "Phase 2: set `account_id` in `ledger.ini`. Only then can p2-c1 be met.",
            "The owner must fill in ledger.ini ACCOUNT_ID to get a real whoami.",
        ] {
            assert!(
                repeats_run_ask(&board, &phase("judge"), None, repeat),
                "{repeat}"
            );
        }
        // More than it lists, another run, or nothing named: a different item.
        assert!(!repeats_run_ask(
            &board,
            &phase("judge"),
            None,
            "Set STRIPE_KEY and account_id."
        ));
        let other = WaitingSource::Run {
            run_id: OvernightRunId("other".into()),
            task_id: None,
        };
        assert!(!repeats_run_ask(&board, &other, None, "Set account_id."));
        assert!(!repeats_run_ask(
            &board,
            &phase("judge"),
            None,
            "Sign in to npm."
        ));
        assert_eq!(
            not_listed_by_run(
                &board,
                &phase("judge"),
                None,
                &lines(&[
                    "Owner must set account_id (p2-c2).",
                    "Add p2-c3's key.",
                    "Again: p2-c3."
                ])
            ),
            lines(&["Add p2-c3's key."])
        );
    }

    #[test]
    fn a_verified_phase_ends_only_its_own_asks_from_its_run_and_earlier_segments() {
        // The short live run: segment 2's checks asked for the ID, the Continue's words gave it.
        let earlier = OvernightRunId("01a0fe2c-run".into());
        let lineage: HashSet<_> = [earlier.clone(), OvernightRunId("01a0fe3e-run".into())].into();
        let board = Board::default();
        let run = |run_id: &OvernightRunId, task: Option<&str>| WaitingSource::Run {
            run_id: run_id.clone(),
            task_id: task.map(|task| TaskId(task.into())),
        };
        let over = |source: WaitingSource, what: &str| {
            asks_of_phase(&board, &lineage, &item("w", source, what), 2)
        };
        assert!(over(
            run(&earlier, None),
            "Phase 2: The owner must supply the team's real ledger account ID for ledger.ini [account].account_id."
        ));
        assert!(over(
            run(&earlier, None),
            "Set account_id in ledger.ini (criterion p2-c2)."
        ));
        // Another phase's, a refused outward command, another run's or a task's: still open.
        assert!(!over(run(&earlier, None), "Phase 3: set account_id."));
        assert!(!over(
            run(&earlier, None),
            "Set the IDs for p2-c2 and p3-c1."
        ));
        assert!(!over(
            run(&earlier, Some("t9")),
            "Push the run branch to origin."
        ));
        let other = OvernightRunId("other".into());
        assert!(!over(run(&other, None), "Phase 2: set account_id."));
        assert!(!over(
            WaitingSource::Task {
                task_id: TaskId("t9".into())
            },
            "Phase 2: set account_id (p2-c2)."
        ));
    }

    #[test]
    fn the_board_keeps_open_items_and_every_decision() {
        let item = WaitingItem {
            id: "w1".into(),
            request_id: Some("r1".into()),
            source: WaitingSource::Orchestrator,
            key: waiting_key(&WaitingSource::Orchestrator, "Sign in to npm"),
            what: "Sign in to npm".into(),
            created_at_ms: 1,
        };
        let mut board = Board::default();
        board.apply(&DomainEvent::WaitingOnYou { item: item.clone() }, 3);
        assert_eq!(board.sorted_waiting(), [item]);
        board.apply(
            &DomainEvent::WaitingResolved {
                id: "w1".into(),
                by: ResolvedBy::User,
            },
            4,
        );
        assert!(board.waiting.is_empty());
        board.apply(
            &DomainEvent::DecidedForYou {
                decision: Decision {
                    id: "d1".into(),
                    request_id: None,
                    source: DecisionSource::Orchestrator,
                    kind: DecisionKind::Routine,
                    what: "Kept the old API".into(),
                    why: String::new(),
                    at_ms: 2,
                    position: 0,
                    short: None,
                },
            },
            5,
        );
        assert_eq!(board.decisions.len(), 1);
        assert_eq!(board.decisions[0].position, 5);
    }

    #[test]
    fn stored_events_read_back() {
        let event: DomainEvent = serde_json::from_value(serde_json::json!({
            "type": "decidedForYou",
            "decision": {
                "id": "d1",
                "source": { "type": "task", "taskId": "t1" },
                "what": "Landed task-1",
                "atMs": 1,
            },
        }))
        .expect("a decision");
        let DomainEvent::DecidedForYou { decision } = event else {
            panic!("not a decision");
        };
        assert!(decision.why.is_empty() && decision.request_id.is_none());
        let event: DomainEvent = serde_json::from_value(serde_json::json!({
            "type": "waitingResolved",
            "id": "w1",
            "by": "user",
        }))
        .expect("a resolution");
        assert_eq!(event.kind(), "waiting.resolved");
    }
}
