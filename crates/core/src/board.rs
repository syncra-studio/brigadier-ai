//! A conversation's board: its tasks, cards, queue and run state, folded from the
//! conversation's stream. Messages are not kept here; they are paged from the store.

use std::collections::{HashMap, HashSet};

use crate::knowledge::MemoryChange;
use crate::model::ConversationId;
use crate::model::{DomainEvent, MessageRole, Notice, OvernightRunId, Rating, StreamingMessage};
use crate::overnight::OvernightRun;
use crate::work::{
    Approval, CardId, CardState, Compaction, ConversationActivity, Decision, MessageQueue,
    OrchestratorStep, Plan, PlanState, Question, ReviewRun, RunState, Task, TaskId, UserRequest,
    WaitingItem, WorkerStep,
};

/// Notices kept per conversation.
const NOTICES_KEPT: usize = 20;

/// Event kinds the board is folded from (everything on a conversation stream but messages).
pub(crate) const KINDS: &[&str] = &[
    "conversation.run",
    "thinking.delta",
    "conversation.notice",
    "task.updated",
    "approval.updated",
    "question.updated",
    "plan.updated",
    "review.updated",
    "thread.seen",
    "queue.changed",
    "request.updated",
    "worker.step",
    "orchestrator.step",
    "compaction.updated",
    "message.rated",
    "conversation.branch",
    "memory.updated",
    "decision.made",
    "waiting.updated",
    "waiting.resolved",
    "overnight.updated",
];

#[derive(Debug, Default, Clone)]
pub(crate) struct Board {
    pub(crate) tasks: HashMap<TaskId, Task>,
    pub(crate) approvals: HashMap<CardId, Approval>,
    pub(crate) questions: HashMap<CardId, Question>,
    pub(crate) plans: HashMap<CardId, Plan>,
    pub(crate) requests: HashMap<String, UserRequest>,
    /// Every worker step, in stream order.
    pub(crate) worker_steps: Vec<WorkerStep>,
    /// Every orchestrator step, in stream order.
    pub(crate) orchestrator_steps: Vec<OrchestratorStep>,
    /// Every row about the machine, in stream order.
    pub(crate) machine_steps: Vec<crate::model::MachineStep>,
    pub(crate) compactions: HashMap<String, Compaction>,
    pub(crate) ratings: HashMap<String, Rating>,
    pub(crate) queue: MessageQueue,
    pub(crate) run: RunState,
    /// The request the running turn serves.
    pub(crate) run_request: Option<String>,
    /// The last message of the branch shown, and the stream sequence that set it.
    pub(crate) head: Option<(String, i64)>,
    pub(crate) streaming: Option<StreamingMessage>,
    pub(crate) thinking: Vec<crate::model::ThinkingSegment>,
    pub(crate) notices: Vec<Notice>,
    /// A Chat's saved memories, the latest change per node, in the order first saved.
    pub(crate) memories: Vec<MemoryChange>,
    /// What was decided on the user's behalf, in stream order.
    pub(crate) decisions: Vec<Decision>,
    /// What only the user can do and is not done yet, by id.
    pub(crate) waiting: HashMap<String, WaitingItem>,
    /// The key of every item ever listed, open or over: a restart lists only what a report
    /// could not, never again what the user marked done.
    pub(crate) waits_listed: HashSet<String>,
    /// Its overnight runs, a segment each.
    pub(crate) runs: HashMap<OvernightRunId, OvernightRun>,
    /// Its one-shot reviews, by id.
    pub(crate) reviews: HashMap<String, ReviewRun>,
    /// Per branch: the tip up to which the thread's commits were looked at.
    pub(crate) thread_tips: HashMap<String, String>,
}

impl Board {
    /// The conversation's activity, or `None` when it neither runs nor waits for the user.
    pub(crate) fn activity(&self, id: &ConversationId) -> Option<ConversationActivity> {
        let tasks: Vec<_> = self
            .tasks
            .values()
            .filter(|task| !task.state.is_final())
            .map(|task| (task.id.clone(), task.state))
            .collect();
        let approvals: Vec<_> = self
            .approvals
            .values()
            .filter(|approval| approval.state == CardState::Pending)
            .map(|approval| approval.id.clone())
            .chain(
                self.plans
                    .values()
                    .filter(|plan| plan.state == PlanState::Proposed)
                    .map(|plan| plan.id.clone()),
            )
            .collect();
        let questions: Vec<_> = self
            .questions
            .values()
            .filter(|question| question.answer.is_none())
            .map(|question| question.id.clone())
            .collect();
        let running = matches!(self.run, RunState::Starting | RunState::Running);
        (running || !tasks.is_empty() || !approvals.is_empty() || !questions.is_empty()).then(
            || ConversationActivity {
                conversation_id: id.clone(),
                run: self.run,
                tasks,
                approvals,
                questions,
            },
        )
    }

    /// Applies an event stored at `stream_seq` in the conversation's stream. An object's
    /// position is the stream sequence of the event that first recorded it.
    pub(crate) fn apply(&mut self, event: &DomainEvent, stream_seq: i64) {
        match event {
            DomainEvent::TaskUpdated { task } => {
                let position = self
                    .tasks
                    .get(&task.id)
                    .map_or(stream_seq, |known| known.position);
                let mut task = (**task).clone();
                task.position = position;
                self.tasks.insert(task.id.clone(), task);
            }
            DomainEvent::ApprovalUpdated { approval } => {
                let position = self
                    .approvals
                    .get(&approval.id)
                    .map_or(stream_seq, |known| known.position);
                let mut approval = approval.clone();
                approval.position = position;
                self.approvals.insert(approval.id.clone(), approval);
            }
            DomainEvent::QuestionUpdated { question } => {
                let position = self
                    .questions
                    .get(&question.id)
                    .map_or(stream_seq, |known| known.position);
                let mut question = question.clone();
                question.position = position;
                self.questions.insert(question.id.clone(), question);
            }
            DomainEvent::PlanUpdated { plan } => {
                let position = self
                    .plans
                    .get(&plan.id)
                    .map_or(stream_seq, |known| known.position);
                let mut plan = plan.clone();
                plan.position = position;
                self.plans.insert(plan.id.clone(), plan);
            }
            DomainEvent::ReviewUpdated { review } => {
                self.reviews.insert(review.id.clone(), review.clone());
            }
            DomainEvent::ThreadCommitsSeen { branch, tip, .. } => {
                self.thread_tips.insert(branch.clone(), tip.clone());
            }
            DomainEvent::QueueChanged { queue, .. } => self.queue = queue.clone(),
            DomainEvent::RunStateChanged {
                state, request_id, ..
            } => {
                self.run = *state;
                self.run_request.clone_from(request_id);
                if *state != RunState::Running {
                    self.streaming = None;
                }
            }
            DomainEvent::RequestUpdated { request } => {
                self.requests.insert(request.id.clone(), request.clone());
            }
            DomainEvent::WorkerStepped { step } => {
                let mut step = step.clone();
                step.position = stream_seq;
                self.worker_steps.push(step);
            }
            DomainEvent::OrchestratorStepped { step } => {
                let mut step = step.clone();
                if let crate::work::OrchestratorStepKind::Tool {
                    item_id,
                    through_position,
                    ..
                } = &mut step.kind
                {
                    *through_position = stream_seq;
                    if let Some(known) = self.orchestrator_steps.iter_mut().find(|known| {
                        matches!(&known.kind, crate::work::OrchestratorStepKind::Tool { item_id: id, .. } if id == item_id)
                    }) {
                        // Input and result updates stay where the call first appeared.
                        known.kind = step.kind;
                        return;
                    }
                }
                step.position = stream_seq;
                self.orchestrator_steps.push(step);
            }
            DomainEvent::MachineStepped { step, .. } => {
                let mut step = step.clone();
                step.position = stream_seq;
                self.machine_steps.push(step);
            }
            DomainEvent::MemoryUpdated { memory, .. } => {
                match self
                    .memories
                    .iter_mut()
                    .find(|known| known.node_id == memory.node_id)
                {
                    // A removal keeps the chip where the turn saved it.
                    Some(known) => {
                        known.forgotten = memory.forgotten;
                        known.text.clone_from(&memory.text);
                    }
                    None => self.memories.push(memory.clone()),
                }
            }
            DomainEvent::DecidedForYou { decision } => {
                let mut decision = decision.clone();
                decision.position = stream_seq;
                // Recorded before decisions had a kind: a run's phase outcomes by their words.
                if matches!(decision.source, crate::work::DecisionSource::Run { .. })
                    && (decision.what.starts_with("Verified phase ")
                        || decision.what.starts_with("Settled phase "))
                {
                    decision.kind = crate::work::DecisionKind::PhaseOutcome;
                }
                decision.short = Some(crate::manager::decisions::short_words(
                    &decision.what,
                    &decision.why,
                ));
                self.decisions.push(decision);
            }
            DomainEvent::WaitingOnYou { item } => {
                self.waits_listed.insert(item.key.clone());
                self.waiting.insert(item.id.clone(), item.clone());
            }
            DomainEvent::WaitingResolved { id, .. } => {
                self.waiting.remove(id);
            }
            DomainEvent::OvernightUpdated { run } => {
                self.runs.insert(run.id.clone(), (**run).clone());
            }
            DomainEvent::CompactionUpdated { compaction } => {
                let position = self
                    .compactions
                    .get(&compaction.id)
                    .map_or(stream_seq, |known| known.position);
                let mut compaction = compaction.clone();
                compaction.position = position;
                self.compactions.insert(compaction.id.clone(), compaction);
            }
            DomainEvent::MessageRated { subject, rating } => {
                self.ratings.insert(subject.clone(), *rating);
            }
            DomainEvent::BranchSwitched { head, .. } => {
                self.head = Some((head.clone(), stream_seq));
            }
            DomainEvent::ConversationNotice { notice, .. } => {
                self.notices.push(notice.clone());
                if self.notices.len() > NOTICES_KEPT {
                    self.notices.remove(0);
                }
            }
            DomainEvent::ThinkingDelta {
                item_id,
                request_id,
                text,
                at_ms,
                complete,
                ..
            } => {
                if let Some(segment) = self
                    .thinking
                    .iter_mut()
                    .find(|segment| segment.item_id == *item_id)
                {
                    if *complete {
                        segment.text.clone_from(text);
                    } else if !segment.complete {
                        segment.text.push_str(text);
                    }
                    segment.updated_at_ms = *at_ms;
                    segment.through_position = stream_seq;
                    segment.complete |= *complete;
                } else {
                    self.thinking.push(crate::model::ThinkingSegment {
                        item_id: item_id.clone(),
                        request_id: request_id.clone(),
                        text: text.clone(),
                        position: stream_seq,
                        started_at_ms: *at_ms,
                        updated_at_ms: *at_ms,
                        complete: *complete,
                        through_position: stream_seq,
                    });
                }
            }
            DomainEvent::MessageDelta {
                message_id, text, ..
            } => match &mut self.streaming {
                Some(streaming) if streaming.message_id == *message_id => {
                    streaming.text.push_str(text);
                }
                _ => {
                    self.streaming = Some(StreamingMessage {
                        message_id: message_id.clone(),
                        text: text.clone(),
                        request_id: self.run_request.clone(),
                    });
                }
            },
            DomainEvent::MessageAppended { message } => {
                // A new message continues the branch shown.
                self.head = Some((message.id.clone(), stream_seq));
                if message.role == MessageRole::Assistant
                    && self
                        .streaming
                        .as_ref()
                        .is_some_and(|streaming| streaming.message_id == message.id)
                {
                    self.streaming = None;
                }
            }
            _ => {}
        }
    }

    pub(crate) fn next_task_number(&self) -> u32 {
        self.tasks
            .values()
            .map(|task| task.number)
            .max()
            .unwrap_or(0)
            + 1
    }

    pub(crate) fn sorted_tasks(&self) -> Vec<Task> {
        let mut tasks: Vec<Task> = self.tasks.values().cloned().collect();
        tasks.sort_by_key(|task| task.position);
        tasks
    }

    pub(crate) fn sorted_approvals(&self) -> Vec<Approval> {
        let mut approvals: Vec<Approval> = self.approvals.values().cloned().collect();
        approvals.sort_by_key(|approval| approval.position);
        approvals
    }

    pub(crate) fn sorted_questions(&self) -> Vec<Question> {
        let mut questions: Vec<Question> = self.questions.values().cloned().collect();
        questions.sort_by_key(|question| question.position);
        questions
    }

    pub(crate) fn sorted_requests(&self) -> Vec<UserRequest> {
        let mut requests: Vec<UserRequest> = self.requests.values().cloned().collect();
        requests.sort_by(|a, b| a.started_at_ms.cmp(&b.started_at_ms).then(a.id.cmp(&b.id)));
        requests
    }

    /// The newest request, which work without a request of its own is filed under.
    pub(crate) fn latest_request(&self) -> Option<&UserRequest> {
        self.requests
            .values()
            .max_by(|a, b| a.started_at_ms.cmp(&b.started_at_ms).then(a.id.cmp(&b.id)))
    }

    pub(crate) fn sorted_waiting(&self) -> Vec<WaitingItem> {
        let mut waiting: Vec<WaitingItem> = self.waiting.values().cloned().collect();
        waiting.sort_by(|a, b| a.created_at_ms.cmp(&b.created_at_ms).then(a.id.cmp(&b.id)));
        waiting
    }

    pub(crate) fn sorted_compactions(&self) -> Vec<Compaction> {
        let mut compactions: Vec<Compaction> = self.compactions.values().cloned().collect();
        compactions.sort_by_key(|compaction| compaction.position);
        compactions
    }

    pub(crate) fn sorted_plans(&self) -> Vec<Plan> {
        let mut plans: Vec<Plan> = self.plans.values().cloned().collect();
        plans.sort_by_key(|plan| plan.position);
        plans
    }

    /// Its one-shot reviews, oldest first.
    pub(crate) fn sorted_reviews(&self) -> Vec<ReviewRun> {
        let mut reviews: Vec<ReviewRun> = self.reviews.values().cloned().collect();
        reviews.sort_by(|a, b| a.started_at_ms.cmp(&b.started_at_ms).then(a.id.cmp(&b.id)));
        reviews
    }

    /// Its overnight runs, oldest first.
    pub(crate) fn sorted_runs(&self) -> Vec<OvernightRun> {
        let mut runs: Vec<OvernightRun> = self.runs.values().cloned().collect();
        runs.sort_by(|a, b| a.created_at_ms.cmp(&b.created_at_ms).then(a.id.cmp(&b.id)));
        runs
    }

    /// The run that owns the session now (started and not finished), if any.
    pub(crate) fn active_run(&self) -> Option<&OvernightRun> {
        self.runs.values().find(|run| run.state.is_active())
    }
}

#[cfg(test)]
mod thinking_tests {
    use super::*;

    #[test]
    fn reasoning_keeps_its_position_and_final_summary_replaces_deltas() {
        let mut board = Board::default();
        let delta = |text: &str, at_ms, complete| DomainEvent::ThinkingDelta {
            conversation_id: ConversationId("chat".into()),
            item_id: "summary".into(),
            request_id: Some("request".into()),
            text: text.into(),
            at_ms,
            complete,
        };
        board.apply(&delta("I will read", 1000, false), 2);
        board.apply(&delta(" the file.", 2000, false), 3);
        assert_eq!(board.thinking[0].text, "I will read the file.");
        board.apply(&delta("Read the file first.", 5000, true), 8);
        let segment = &board.thinking[0];
        assert_eq!(segment.position, 2);
        assert_eq!(segment.through_position, 8);
        assert_eq!(segment.started_at_ms, 1000);
        assert_eq!(segment.updated_at_ms, 5000);
        assert_eq!(segment.request_id.as_deref(), Some("request"));
        assert_eq!(segment.text, "Read the file first.");
        assert!(segment.complete);
        // Legacy events still deserialize and leave reasoning absent.
        let old: DomainEvent = serde_json::from_str(
            r#"{"type":"messageDelta","conversationId":"chat","messageId":"old","text":"Hello"}"#,
        )
        .unwrap();
        board.apply(&old, 9);
        assert_eq!(board.streaming.unwrap().text, "Hello");
    }
}
