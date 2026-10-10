//! User requests: what one user message set in motion, and when it is over.
//!
//! A request works while its turn runs, while an envelope or message for it waits for a turn,
//! or while a task it started runs. It waits while something it opened needs the user (a
//! card, a paused worker, a landing on hold, in an overnight run something only the user can
//! do), and, unless
//! it was stopped or failed, while a plan of it waits for a revision the orchestrator did not
//! make (it was asked for it again once). A waiting request holds back none of the user's
//! queued messages. Otherwise it is done, or stopped or failed when its last turn ended that
//! way. A done request works again when new work for it arrives (a late report, a worker's
//! question), so its block in the thread stays one block.

use std::collections::HashSet;

use super::SessionManager;
use super::conversation::Envelope;
use super::decisions::waiting_run;
use super::prompts;
use super::workers::relanding_pending;
use crate::board::Board;
use crate::model::{ConversationId, ConversationKind};
use crate::overnight::{OvernightRun, OvernightState};
use crate::work::{CardState, PlanState, QuestionKind, RequestState, Task, TaskId, TaskState};

impl SessionManager {
    /// What a request waits for, if anything. A question in the thread's text makes nothing
    /// wait: the user is asked on cards (THREAD-PARITY-PLAN §5 Q6).
    fn waiting_on(board: &Board, request: &str) -> Option<Wait> {
        if waits_on_user(board, request) {
            return Some(Wait::User);
        }
        if open_question_cards(board, request) {
            return Some(Wait::Card);
        }
        tasks_waiting_for_quota(board, request).then_some(Wait::Quota)
    }

    /// The request new work is filed under: the task's, else the running turn's, else the
    /// conversation's newest.
    pub(crate) async fn request_for(
        &self,
        conversation_id: &ConversationId,
        task_id: Option<&TaskId>,
    ) -> Option<String> {
        if let Some(task_id) = task_id
            && let Ok(task) = self.task_by_id(conversation_id, task_id).await
            && task.request_id.is_some()
        {
            return task.request_id;
        }
        if let Ok(conv) = self.conv(conversation_id)
            && let Some(request) = conv.running_request().await
        {
            return Some(request);
        }
        let board = self.core.board(conversation_id).await.ok()?;
        board.latest_request().map(|request| request.id.clone())
    }

    /// The request a task moves to when the running turn acts on it for a later request than
    /// its own: what follows (its landing, the answer) then belongs to the request that asked.
    pub(crate) async fn later_request_for(
        &self,
        conversation_id: &ConversationId,
        task: &Task,
    ) -> Option<String> {
        let running = self.conv(conversation_id).ok()?.running_request().await?;
        let Some(own) = &task.request_id else {
            return Some(running);
        };
        if *own == running {
            return None;
        }
        let board = self.core.board(conversation_id).await.ok()?;
        let started = |id: &str| {
            board
                .requests
                .get(id)
                .map(|request| (request.started_at_ms, &request.id))
        };
        (started(&running)? > started(own)?).then_some(running)
    }

    /// The newest request, while the answer the thread shows for it still works or waits: it,
    /// or a request it was steered into, has a turn, a worker or a card going. A follow-up
    /// sent now may belong to that answer.
    pub(crate) async fn working_request(&self, conversation_id: &ConversationId) -> Option<String> {
        self.newest_answer(conversation_id, |state| {
            matches!(state, RequestState::Working | RequestState::Waiting)
        })
        .await
    }

    /// Whether the newest answer still works, not only waits on the user: queued messages
    /// wait for it. One that only waits on the user lets them through, since the user may be
    /// answering it, or moving on meanwhile.
    pub(crate) async fn answer_working(&self, conversation_id: &ConversationId) -> bool {
        self.newest_answer(conversation_id, |state| *state == RequestState::Working)
            .await
            .is_some()
    }

    /// The newest request, while it or a request it was steered into is in a state `holds`
    /// accepts.
    async fn newest_answer(
        &self,
        conversation_id: &ConversationId,
        holds: impl Fn(&RequestState) -> bool,
    ) -> Option<String> {
        let board = self.core.board(conversation_id).await.ok()?;
        let latest = board.latest_request()?;
        let mut request = Some(latest);
        // A steer chain is short; the bound only guards against a cycle in stored data.
        for _ in 0..board.requests.len() {
            let Some(of) = request else {
                break;
            };
            if holds(&of.state) {
                return Some(latest.id.clone());
            }
            request = of
                .steered_into
                .as_deref()
                .and_then(|into| board.requests.get(into));
        }
        None
    }

    /// Brings every request of the conversation up to date with what runs and waits.
    pub(crate) async fn settle_requests(&self, conversation_id: &ConversationId) {
        let Ok(mut board) = self.core.board(conversation_id).await else {
            return;
        };
        // What waited on a card that is answered now is over.
        if self.settle_waits(conversation_id, &board).await {
            let Ok(now) = self.core.board(conversation_id).await else {
                return;
            };
            board = now;
        }
        if board.requests.is_empty() {
            return;
        }
        let Ok(conv) = self.conv(conversation_id) else {
            return;
        };
        let activity = conv.request_activity().await;
        for request in board.requests.values() {
            let id = request.id.as_str();
            let outcome = activity.outcomes.get(id);
            let mut wait = None;
            let state = if let Some(state) = ended_run_state(&board, request) {
                // An overnight run's request is over with the run: its report is the answer,
                // and nothing that comes later works or waits for it again.
                state
            } else if activity.running.as_deref() == Some(id)
                || (activity.carried.contains(id) && outcome.is_none())
                || tasks_in(&board, id, |state| {
                    matches!(
                        state,
                        TaskState::Queued
                            | TaskState::Starting
                            | TaskState::Running
                            | TaskState::Landing
                    )
                })
                || relanding_in(&board, id)
            {
                // A fix Brigadier checks and lands once its worker's turn is over still works.
                RequestState::Working
            } else if let Some(on) = Self::waiting_on(&board, id) {
                wait = Some(on);
                RequestState::Waiting
            } else if blocked_on_the_thread(&board, id) {
                // Blocked on the orchestrator's answer (a gate's card makes it Waiting).
                RequestState::Working
            } else if let Some(outcome) = outcome {
                outcome.clone()
            } else if matches!(
                request.state,
                RequestState::Stopped | RequestState::Failed { .. }
            ) {
                request.state.clone()
            } else {
                RequestState::Done
            };
            let over = !matches!(state, RequestState::Working | RequestState::Waiting);
            let done = state == RequestState::Done;
            if let Err(err) = self
                .core
                .update_request(
                    conversation_id,
                    id,
                    state,
                    wait == Some(Wait::Quota),
                    wait == Some(Wait::Card),
                )
                .await
            {
                tracing::debug!(conversation = %conversation_id, error = %err, "could not update a request");
            }
            if over {
                self.release_narration(&conv, id, done).await;
            }
        }
        // A session's answer that ended with no turn after it (a worker the user stopped):
        // the follow-up waiting for it goes now.
        let waiting = conv.kind == ConversationKind::Session
            && activity.running.is_none()
            && activity.carried.is_empty()
            && !board.queue.paused
            && board.queue.items.first().is_some_and(|item| !item.deciding);
        if waiting && !self.answer_working(conversation_id).await {
            self.kick(&conv);
        }
    }

    /// What a turn tells the orchestrator besides user messages: each envelope, labelled when
    /// it belongs to an earlier request than the newest, then what else still runs for the
    /// request.
    pub(super) async fn request_notes(
        &self,
        conversation_id: &ConversationId,
        envelopes: &[(Envelope, Option<String>)],
        request: Option<&str>,
    ) -> Vec<String> {
        if envelopes.is_empty() {
            return Vec::new();
        }
        let Ok(board) = self.core.board(conversation_id).await else {
            return envelopes.iter().map(|(e, _)| e.text.clone()).collect();
        };
        let earlier = match (request, board.latest_request()) {
            (Some(request), Some(latest)) if latest.id != request => {
                board.requests.get(request).map(|of| of.preview.clone())
            }
            _ => None,
        };
        let mut notes: Vec<String> = envelopes
            .iter()
            .map(|(envelope, _)| match &earlier {
                Some(preview) => format!(
                    "[for the user's earlier request: \"{preview}\"]\n{}",
                    envelope.text
                ),
                None => envelope.text.clone(),
            })
            .collect();
        if let (Some(request), Some(last)) = (request, notes.last_mut()) {
            // A task that already ended or reported still counts while the envelope saying so
            // has not reached a turn: its landing's cleanup can outlast the turn's start.
            let announced = match self.conv(conversation_id) {
                Ok(conv) => self.announced(&conv, request).await,
                Err(_) => HashSet::new(),
            };
            let mut running: Vec<_> = board
                .tasks
                .values()
                .filter(|task| {
                    announced.contains(&task.id)
                        || (task.request_id.as_deref() == Some(request)
                            && !task.state.is_final()
                            && (task.state != TaskState::Reported || relanding_pending(task)))
                })
                .collect();
            running.sort_by_key(|task| task.number);
            // A change held back from landing runs no more: it waits for what holds it.
            let (held, running): (Vec<_>, Vec<_>) = running.into_iter().partition(|task| {
                task.state == TaskState::ReadyToLand && !announced.contains(&task.id)
            });
            let undecided = self.undecided(&board, request).await;
            last.push_str("\n\n");
            if running.is_empty() {
                last.push_str("[nothing else is running for this request]");
            } else {
                let list: Vec<String> = running
                    .iter()
                    .map(|task| {
                        let news = if announced.contains(&task.id) {
                            ", its message follows"
                        } else {
                            ""
                        };
                        format!(
                            "task-{} \"{}\" ({:?}{news})",
                            task.number, task.title, task.state
                        )
                    })
                    .collect();
                last.push_str(&format!(
                    "[still running for this request: {}. Their reports come as later messages. \
                     Act on this message with tools if it needs it. Then, unless the user must \
                     change plans, reply with exactly {} and nothing else: the user already \
                     sees the workers' progress.]",
                    list.join(", "),
                    prompts::QUIET
                ));
            }
            if !held.is_empty() {
                last.push('\n');
                last.push_str(&held_note(&held));
            }
            if !undecided.is_empty() {
                last.push('\n');
                last.push_str(&prompts::undecided_note(&undecided));
            }
        }
        notes
    }

    /// The request's write tasks whose report waits for the orchestrator's decision: reported
    /// with a change that could land, and not a fix Brigadier lands on its own. Until it
    /// accepts, sends back or stops them they stay open.
    pub(super) async fn undecided(&self, board: &Board, request: &str) -> Vec<Task> {
        let mut undecided = Vec::new();
        for task in board.tasks.values().filter(|task| {
            task.request_id.as_deref() == Some(request)
                && task.state == TaskState::Reported
                && task.kind.writes()
                && !relanding_pending(task)
        }) {
            // A task that changed nothing ends meanwhile, and its worktree goes after it is
            // recorded as ended: one whose diff is gone is undecided only if still reported.
            if !self.changed_nothing(task).await
                && self
                    .task_by_id(&task.conversation_id, &task.id)
                    .await
                    .is_ok_and(|now| now.state == TaskState::Reported)
            {
                undecided.push(task.clone());
            }
        }
        undecided.sort_by_key(|task| task.number);
        undecided
    }
}

/// The request's changes held back from landing (`ReadyToLand`): not running, and not landed.
fn held_note(held: &[&Task]) -> String {
    let list: Vec<String> = held
        .iter()
        .map(|task| format!("task-{} \"{}\"", task.number, task.title))
        .collect();
    format!(
        "[held, not landed: {}. Nothing of it landed; once what holds it is fixed, call land_phase for it again.]",
        list.join(", ")
    )
}

/// Whether a task of the request is in a state `matches` accepts.
fn tasks_in(board: &Board, request: &str, matches: impl Fn(TaskState) -> bool) -> bool {
    board
        .tasks
        .values()
        .any(|task| task.request_id.as_deref() == Some(request) && matches(task.state))
}

/// Whether a task of the request is blocked on the thread's answer. A lead whose outline is
/// on a proposed plan's card waits for the user's yes instead, and that card holds nothing up
/// (THREAD-PARITY-PLAN §8.5).
fn blocked_on_the_thread(board: &Board, request: &str) -> bool {
    let proposed = board.plans.values().any(|plan| {
        plan.request_id.as_deref() == Some(request)
            && plan.state == PlanState::Proposed
            && plan.body.is_some()
    });
    board.tasks.values().any(|task| {
        task.request_id.as_deref() == Some(request)
            && task.state == TaskState::Blocked
            && !(proposed && SessionManager::waits_for_go_ahead(task))
    })
}

/// Whether a task of the request reported a fix Brigadier checks and lands on its own, once
/// its worker's turn is over.
fn relanding_in(board: &Board, request: &str) -> bool {
    board
        .tasks
        .values()
        .any(|task| task.request_id.as_deref() == Some(request) && relanding_pending(task))
}

/// The overnight run a request belongs to: its requests are named `run-<short run id>-…`.
pub(crate) fn run_of_request<'a>(board: &'a Board, request: &str) -> Option<&'a OvernightRun> {
    board
        .runs
        .values()
        .find(|run| request.starts_with(&format!("run-{}-", run.id.short())))
}

/// Whether the request belongs to an overnight run that has ended (or is writing its report):
/// nothing new starts or wakes the orchestrator for it.
pub(crate) fn ended_run_request(board: &Board, request: &str) -> bool {
    run_of_request(board, request)
        .is_some_and(|run| run.state == OvernightState::Reporting || run.state.is_final())
}

/// The state a request of a finished run keeps for good: how it ended, or done.
fn ended_run_state(board: &Board, request: &crate::work::UserRequest) -> Option<RequestState> {
    if !run_of_request(board, &request.id).is_some_and(|run| run.state.is_final()) {
        return None;
    }
    Some(match &request.state {
        RequestState::Stopped | RequestState::Failed { .. } => request.state.clone(),
        _ => RequestState::Done,
    })
}

/// A worker of the request is paused until a model it may use is free.
fn tasks_waiting_for_quota(board: &Board, request: &str) -> bool {
    board.tasks.values().any(|task| {
        task.request_id.as_deref() == Some(request)
            && task.state == TaskState::Paused
            && task.quota_wait.is_some()
    })
}

/// What a waiting request waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wait {
    /// The user, with the request's work paused (an approval, a plan, a held worker).
    User,
    /// The user's answer to the request's own question card: the request goes on with it, so
    /// its time keeps counting.
    Card,
    /// Quota only: its workers paused until a model is free.
    Quota,
}

/// The request's thread asked the user a round of questions on a card that is open. Its merge
/// card holds nothing up: it comes after the request's answer, so the request is done, with
/// its work folded and its time stopped, while the card waits in the composer's place; the
/// user's answer makes it work again (THREAD-PARITY-PLAN §5 Q9).
fn open_question_cards(board: &Board, request: &str) -> bool {
    board.questions.values().any(|q| {
        q.request_id.as_deref() == Some(request)
            && q.is_open()
            && q.kind == QuestionKind::Orchestrator
    })
}

/// What only the user can do holds the request up (a card, a worker's question, a paused or
/// held worker, an overnight run's "Waiting on you" item), quota and the thread's own question
/// cards aside. A session lists no items, and one listed before it stopped makes nothing wait.
/// A proposed plan holds nothing up either: its card is the request's answer, and the user's
/// yes makes the request work again (THREAD-PARITY-PLAN §6).
fn waits_on_user(board: &Board, request: &str) -> bool {
    let of = |id: &Option<String>| id.as_deref() == Some(request);
    board.waiting.values().any(|item| {
        of(&item.request_id)
            && waiting_run(&item.source, item.request_id.as_deref(), board).is_some()
    }) || board
        .approvals
        .values()
        .any(|a| of(&a.request_id) && a.state == CardState::Pending)
        || board.questions.values().any(|q| {
            of(&q.request_id)
                && q.is_open()
                && matches!(q.kind, QuestionKind::UncommittedChanges { .. })
        })
        || board.tasks.values().any(|task| {
            of(&task.request_id)
                && (task.state == TaskState::ReadyToLand
                    // The user works with it in their terminal.
                    || task.state == TaskState::TakenOver
                    || (task.state == TaskState::Paused && task.quota_wait.is_none()))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(id: &str, state: RequestState) -> crate::work::UserRequest {
        crate::work::UserRequest {
            id: id.into(),
            conversation_id: ConversationId("c".into()),
            preview: String::new(),
            state,
            started_at_ms: 0,
            ended_at_ms: None,
            steered_into: None,
            steered_after: None,
            undo: None,
            worked: Vec::new(),
            quota_wait: false,
        }
    }

    fn spans(request: &crate::work::UserRequest) -> Vec<(i64, Option<i64>)> {
        request
            .worked
            .iter()
            .map(|span| (span.from_ms, span.to_ms))
            .collect()
    }

    #[test]
    fn a_request_works_in_spans_that_leave_out_waits_for_the_user_and_stops() {
        let mut stored = request("r", RequestState::Working);
        stored.moved_to(RequestState::Working, false, false, 0);
        stored.moved_to(RequestState::Waiting, false, false, 10);
        assert_eq!(stored.ended_at_ms, Some(10));
        stored.moved_to(RequestState::Working, false, false, 30);
        stored.moved_to(RequestState::Stopped, false, false, 40);
        stored.moved_to(RequestState::Working, false, false, 100);
        stored.moved_to(RequestState::Done, false, false, 110);
        assert_eq!(
            spans(&stored),
            [(0, Some(10)), (30, Some(40)), (100, Some(110))]
        );
        assert_eq!(stored.ended_at_ms, Some(110));
    }

    #[test]
    fn waiting_for_quota_is_work_until_the_wait_turns_to_the_user() {
        let mut stored = request("r", RequestState::Working);
        stored.moved_to(RequestState::Working, false, false, 0);
        stored.moved_to(RequestState::Waiting, true, false, 10);
        assert!(stored.quota_wait);
        assert_eq!(spans(&stored), [(0, None)]);
        // Still waiting, now for the user: the span closes there.
        stored.moved_to(RequestState::Waiting, false, false, 20);
        assert!(!stored.quota_wait);
        assert_eq!(stored.ended_at_ms, Some(10));
        stored.moved_to(RequestState::Done, false, false, 40);
        assert_eq!(spans(&stored), [(0, Some(20))]);
    }

    #[test]
    fn waiting_on_its_question_card_is_work_so_the_time_runs_on() {
        // 10 s of work, 30 s on the card, 5 s more: it worked for 45 s.
        let mut stored = request("r", RequestState::Working);
        stored.moved_to(RequestState::Working, false, false, 0);
        stored.moved_to(RequestState::Waiting, false, true, 10_000);
        assert_eq!(spans(&stored), [(0, None)]);
        assert!(!stored.quota_wait);
        stored.moved_to(RequestState::Working, false, false, 40_000);
        stored.moved_to(RequestState::Done, false, false, 45_000);
        assert_eq!(spans(&stored), [(0, Some(45_000))]);
        // A wait for anything else of the user's closes the span, as before.
        stored.moved_to(RequestState::Working, false, false, 50_000);
        stored.moved_to(RequestState::Waiting, false, true, 55_000);
        stored.moved_to(RequestState::Waiting, false, false, 60_000);
        stored.moved_to(RequestState::Done, false, false, 90_000);
        assert_eq!(spans(&stored), [(0, Some(45_000)), (50_000, Some(60_000))]);
    }

    #[test]
    fn a_request_stored_before_spans_counts_from_its_start() {
        let mut waiting = request("r", RequestState::Waiting);
        waiting.ended_at_ms = Some(10);
        waiting.moved_to(RequestState::Done, false, false, 20);
        assert_eq!(spans(&waiting), [(0, Some(10))]);
        let mut working = request("r", RequestState::Working);
        working.moved_to(RequestState::Done, false, false, 50);
        assert_eq!(spans(&working), [(0, Some(50))]);
    }

    /// A task of `request` in `state`.
    fn task_of(request: &str, state: TaskState) -> Task {
        let mut task: Task = serde_json::from_value(serde_json::json!({
            "id": format!("t-{request}"),
            "conversationId": "c",
            "number": 1,
            "position": 0,
            "title": "Add the flag",
            "kind": "implement",
            "spec": "Add the flag.",
            "access": { "repo": "write", "network": false, "unsandboxed": false },
            "route": { "choice": { "provider": "claude", "model": null, "effort": null }, "reason": "" },
            "state": "running",
            "requestId": request,
            "attachments": [],
            "createdAtMs": 0,
            "updatedAtMs": 0
        }))
        .expect("a task");
        task.state = state;
        task
    }

    /// A "Waiting on you" item of `request`, from a worker's report.
    fn item_of(request: &str, task: &str) -> crate::work::WaitingItem {
        crate::work::WaitingItem {
            id: format!("w-{request}"),
            request_id: Some(request.into()),
            source: crate::work::WaitingSource::Task {
                task_id: TaskId(task.into()),
            },
            key: String::new(),
            what: "Add STRIPE_KEY to .env".into(),
            created_at_ms: 0,
        }
    }

    /// A session's request listed for the user before sessions stopped listing anything is
    /// not waiting for it; an overnight run's still is. Quota, a takeover, a paused worker
    /// and an open card make either wait (THREAD-PARITY-PLAN §5 Q6).
    #[test]
    fn only_a_runs_list_and_what_truly_holds_it_make_a_request_wait() {
        let wait = |board: &Board, request: &str| SessionManager::waiting_on(board, request);
        let mut board = Board::default();
        let reported = task_of("r1", TaskState::Reported);
        board
            .waiting
            .insert("w-r1".into(), item_of("r1", &reported.id.0));
        board.tasks.insert(reported.id.clone(), reported);
        assert_eq!(wait(&board, "r1"), None);

        let run = OvernightRun::for_test(ConversationId("c".into()), "Speed", Vec::new());
        let phase = format!("run-{}-phase-1-g1", run.id.short());
        let mut lead = task_of(&phase, TaskState::Reported);
        lead.run = Some(crate::overnight::RunTaskContext {
            run_id: run.id.clone(),
            segment: 1,
            generation: 1,
            role: crate::overnight::RunRole::Worker,
            rules_hash: String::new(),
        });
        board.runs.insert(run.id.clone(), run);
        board
            .waiting
            .insert(format!("w-{phase}"), item_of(&phase, &lead.id.0));
        board.tasks.insert(lead.id.clone(), lead);
        assert_eq!(wait(&board, &phase), Some(Wait::User));

        let held = |state: TaskState, quota: bool| {
            let mut board = board.clone();
            let mut task = task_of("r1", state);
            task.id = TaskId("held".into());
            task.quota_wait = quota.then(|| crate::work::QuotaWait {
                reason: "The 5-hour window resets at 21:10".into(),
                resets_at_ms: None,
                rule: None,
                ranking: None,
                since_ms: 0,
                messages: Vec::new(),
            });
            board.tasks.insert(task.id.clone(), task);
            wait(&board, "r1")
        };
        assert_eq!(held(TaskState::Paused, true), Some(Wait::Quota));
        assert_eq!(held(TaskState::TakenOver, false), Some(Wait::User));
        assert_eq!(held(TaskState::Paused, false), Some(Wait::User));

        let mut asked = board.clone();
        let card: crate::work::Question = serde_json::from_value(serde_json::json!({
            "id": "q1",
            "conversationId": "c",
            "taskId": null,
            "requestId": "r1",
            "position": 0,
            "kind": { "type": "orchestrator" },
            "text": "Which format?",
            "options": ["CSV", "JSON"],
            "answer": null,
            "createdAtMs": 0,
            "answeredAtMs": null
        }))
        .expect("a question");
        let mut merge = card.clone();
        merge.id = crate::model::CardId("q2".into());
        merge.kind = QuestionKind::Merge {
            branch: "brigadier/c/session".into(),
            base: "main".into(),
        };
        asked.questions.insert(card.id.clone(), card);
        assert_eq!(wait(&asked, "r1"), Some(Wait::Card));
        // The merge card comes after the answer and holds nothing up.
        let mut merging = board.clone();
        merging.questions.insert(merge.id.clone(), merge);
        assert_eq!(wait(&merging, "r1"), None);
    }

    #[test]
    fn a_finished_runs_requests_stay_over_and_wake_nothing() {
        let mut run = OvernightRun::for_test(ConversationId("c".into()), "Speed", Vec::new());
        let phase = format!("run-{}-phase-2-g1", run.id.short());
        let mut board = Board::default();
        board.runs.insert(run.id.clone(), run.clone());
        // A running run's phase request lives as usual.
        assert!(run_of_request(&board, &phase).is_some());
        assert!(!ended_run_request(&board, &phase));
        assert_eq!(
            ended_run_state(&board, &request(&phase, RequestState::Working)),
            None
        );
        // Writing its report: nothing new starts for it any more.
        run.state = OvernightState::Reporting;
        board.runs.insert(run.id.clone(), run.clone());
        assert!(ended_run_request(&board, &phase));
        // Finished: its requests are done for good, whatever waits or comes later, and keep
        // a stop.
        run.state = OvernightState::Finished;
        board.runs.insert(run.id.clone(), run.clone());
        assert_eq!(
            ended_run_state(&board, &request(&phase, RequestState::Waiting)),
            Some(RequestState::Done)
        );
        assert_eq!(
            ended_run_state(&board, &request(&phase, RequestState::Stopped)),
            Some(RequestState::Stopped)
        );
        // The user's own requests in the same session are not the run's.
        assert!(run_of_request(&board, "01a0fefa-user-message").is_none());
        assert!(!ended_run_request(&board, "01a0fefa-user-message"));
    }
    use crate::model::ConversationId;

    #[test]
    fn a_held_change_is_named_held_not_running() {
        let task: Task = serde_json::from_value(serde_json::json!({
            "id": "t3",
            "conversationId": "c",
            "number": 3,
            "position": 0,
            "title": "credit bot",
            "kind": "implement",
            "spec": "Credit the bot.",
            "access": { "repo": "write", "network": false, "unsandboxed": false },
            "route": { "choice": { "provider": "claude", "model": null, "effort": null }, "reason": "" },
            "state": "readyToLand",
            "attachments": [],
            "createdAtMs": 0,
            "updatedAtMs": 0
        }))
        .expect("a task");
        let note = held_note(&[&task]);
        assert!(
            note.starts_with("[held, not landed: task-3 \"credit bot\""),
            "{note}"
        );
        assert!(!note.contains("running"), "{note}");
    }

    #[test]
    fn a_fix_brigadier_is_about_to_check_keeps_its_request_working() {
        let mut task: Task = serde_json::from_value(serde_json::json!({
            "id": "t4",
            "conversationId": "c",
            "number": 4,
            "position": 0,
            "title": "export avg",
            "kind": "implement",
            "spec": "Export avg.",
            "access": { "repo": "write", "network": false, "unsandboxed": false },
            "route": { "choice": { "provider": "claude", "model": null, "effort": null }, "reason": "" },
            "state": "reported",
            "attachments": [],
            "createdAtMs": 0,
            "updatedAtMs": 0
        }))
        .expect("a task");
        task.request_id = Some("r1".into());
        let mut board = Board::default();
        board.tasks.insert(task.id.clone(), task.clone());
        // A report the orchestrator decides about: nothing runs for it.
        assert!(!relanding_in(&board, "r1"));
        // A fix reported while Brigadier lands the change: checked once the turn is over.
        task.landing = Some("Export avg".into());
        board.tasks.insert(task.id.clone(), task.clone());
        assert!(relanding_in(&board, "r1"));
        assert!(!relanding_in(&board, "r2"));
        // Stopped meanwhile: it is over.
        task.state = TaskState::Stopped;
        board.tasks.insert(task.id.clone(), task);
        assert!(!relanding_in(&board, "r1"));
    }
}
