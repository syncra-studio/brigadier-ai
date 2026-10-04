//! User requests: what one user message set in motion, and when it is over.
//!
//! A request works while its turn runs, while an envelope or message for it waits for a turn,
//! or while a task it started runs. It waits while something it opened needs the user (a
//! card, a paused worker, a landing on hold, something only the user can do), while its last
//! turn ended asking the user something about a reported change still undecided, and, unless
//! it was stopped or failed, while a plan of it waits for a revision the orchestrator did not
//! make (it was asked for it again once). A waiting request holds back none of the user's
//! queued messages. Otherwise it is done, or stopped or failed when its last turn ended that
//! way. A done request works again when new work for it arrives (a late report, a worker's
//! question), so its block in the thread stays one block.

use std::collections::HashSet;

use super::SessionManager;
use super::conversation::Envelope;
use super::prompts;
use super::workers::relanding_pending;
use crate::board::Board;
use crate::model::{ConversationId, ConversationKind};
use crate::overnight::{OvernightRun, OvernightState};
use crate::work::{CardState, PlanState, RequestState, Task, TaskId, TaskState};

impl SessionManager {
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
        if self.settle_card_waits(conversation_id, &board).await {
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
                            | TaskState::Reviewing
                    )
                })
                || relanding_in(&board, id)
            {
                // A fix Brigadier checks and lands once its worker's turn is over still works.
                RequestState::Working
            } else if needs_user(&board, id)
                || (activity.asked_user.contains(id)
                    && !self.undecided(&board, id).await.is_empty())
            {
                // The orchestrator's question about a change it has not decided on is the
                // user's to answer (an offer at the end of a finished answer is not).
                RequestState::Waiting
            } else if tasks_in(&board, id, |state| state == TaskState::Blocked) {
                // Blocked on the orchestrator's answer (a gate's card makes it Waiting).
                RequestState::Working
            } else if let Some(outcome) = outcome {
                outcome.clone()
            } else if matches!(
                request.state,
                RequestState::Stopped | RequestState::Failed { .. }
            ) {
                request.state.clone()
            } else if awaits_revision(&board, id) {
                // Its plan waits for a revision its turns ended without (they were asked for
                // it again once): a decision is due, and nothing runs meanwhile.
                RequestState::Waiting
            } else {
                RequestState::Done
            };
            let over = !matches!(state, RequestState::Working | RequestState::Waiting);
            let done = state == RequestState::Done;
            if let Err(err) = self.core.update_request(conversation_id, id, state).await {
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
            if !self.changed_nothing(task).await {
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
        "[held, not landed: {}. Nothing of it landed; once what holds it is fixed, call accept_task for it again.]",
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

/// Whether a task of the request reported a fix Brigadier checks and lands on its own, once
/// its worker's turn is over.
fn relanding_in(board: &Board, request: &str) -> bool {
    board
        .tasks
        .values()
        .any(|task| task.request_id.as_deref() == Some(request) && relanding_pending(task))
}

/// Whether a plan of the request waits for the orchestrator's revision after its review.
/// The overnight run a request belongs to: its phases', its Phase 0's and its report's
/// requests are named `run-<short run id>-…`.
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

fn awaits_revision(board: &Board, request: &str) -> bool {
    board
        .plans
        .values()
        .any(|p| p.request_id.as_deref() == Some(request) && p.state == PlanState::Revising)
}

/// Whether something the request opened waits for the user: a card, a task, or something
/// only the user can do ("Waiting on you").
pub(super) fn needs_user(board: &Board, request: &str) -> bool {
    let of = |id: &Option<String>| id.as_deref() == Some(request);
    board.waiting.values().any(|item| of(&item.request_id))
        || board
            .approvals
            .values()
            .any(|a| of(&a.request_id) && a.state == CardState::Pending)
        || board
            .questions
            .values()
            .any(|q| of(&q.request_id) && q.answer.is_none() && q.answered_at_ms.is_none())
        || board.plans.values().any(|p| {
            of(&p.request_id) && matches!(p.state, PlanState::Proposed | PlanState::InReview { .. })
        })
        || tasks_in(board, request, |state| {
            matches!(
                state,
                TaskState::Paused | TaskState::AwaitingApproval | TaskState::ReadyToLand
            )
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
        }
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
    use crate::work::{CardId, Plan};

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

    #[test]
    fn a_plan_waiting_for_its_revision_is_found() {
        let plan = |request: &str, state| Plan {
            id: CardId(format!("{request}-plan")),
            conversation_id: ConversationId("c".into()),
            request_id: Some(request.into()),
            position: 0,
            title: "Plan".into(),
            steps: Vec::new(),
            risky: false,
            state,
            gate: None,
            revises: None,
            responses: Vec::new(),
            review_notes: Vec::new(),
            created_at_ms: 0,
            decided_at_ms: None,
        };
        let mut board = Board::default();
        for plan in [
            plan("r1", PlanState::Revising),
            plan("r2", PlanState::Superseded),
        ] {
            board.plans.insert(plan.id.clone(), plan);
        }
        assert!(awaits_revision(&board, "r1"));
        assert!(!awaits_revision(&board, "r2"));
        assert!(!awaits_revision(&board, "r3"));
    }
}
