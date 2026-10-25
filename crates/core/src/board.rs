//! A conversation's board: its tasks, cards, queue and run state, folded from the
//! conversation's stream. Messages are not kept here; they are paged from the store.

use std::collections::{HashMap, HashSet};

use brigadier_providers::{McpServerState, McpStatus};

use crate::model::ConversationId;
use crate::model::{
    DomainEvent, MessageRole, Notice, PeerState, Rating, ReplyHold, StreamingMessage,
};
use crate::thread_view::{CliTurn, TurnEnd};
use crate::work::{
    Approval, CardId, CardState, Compaction, ConversationActivity, Decision, MessageQueue,
    OrchestratorStep, OrchestratorStepKind, Question, RunState, TabState, Task, TaskId,
    UserRequest, WorkerStep,
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
    "integrated_check.updated",
    "thread.seen",
    "thread.files",
    "output.stored",
    "thread.looked",
    "preview.updated",
    "previews.cleared",
    "background.cleared",
    "queue.changed",
    "request.updated",
    "request.ready",
    "pull_request.seen",
    "worker.step",
    "orchestrator.step",
    "compaction.updated",
    "message.rated",
    "rating.cleared",
    "conversation.branch",
    "decision.made",
    "peer.sent",
    "peer.received",
    "peer.state",
    "peer.settled",
    "peer.held",
    "peer.answered",
];

/// A message between this session and another, as its side recorded it
/// (SESSION-MESSAGING-PLAN.md §4).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PeerRecord {
    pub(crate) message_id: String,
    /// This session sent it (else it received it).
    pub(crate) outgoing: bool,
    /// The other session: where it went, or where it came from.
    pub(crate) peer: ConversationId,
    pub(crate) reply_to: Option<String>,
    pub(crate) chain_id: String,
    pub(crate) hop: u32,
    /// Sent: the request whose turn sent it. Received: the request it is filed under.
    pub(crate) request_id: Option<String>,
    pub(crate) state: PeerState,
    /// Sent: its text (the start of it, when `blob` holds the rest).
    pub(crate) text: String,
    pub(crate) blob: Option<String>,
    /// Sent: when it was allowed to go.
    pub(crate) sent_at_ms: Option<i64>,
    /// Sent: the thread no longer waits for its reply (`settle_wait`).
    pub(crate) settled: bool,
    /// Its position in the stream.
    pub(crate) position: i64,
    /// Sent: Brigadier sent it for this session, as its answer to `reply_to`.
    pub(crate) automatic: bool,
    /// An automatic reply: the answer message it carries.
    pub(crate) answer_id: Option<String>,
    /// Received reply: where it stands with a wait of this session's thread.
    pub(crate) hold: Option<ReplyHold>,
}

impl PeerRecord {}

/// What the thread is known to have written, without its commits' trailer saying so.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Authored {
    /// Its commits on `branch` after `from` up to `tip` (`ThreadCommitsSeen.authored_from`).
    Commits {
        branch: String,
        from: String,
        tip: String,
    },
    /// A local checkout's files over one request: snapshots of them when it started and as
    /// last looked at, its commits included.
    Files {
        request_id: String,
        branch: String,
        start: String,
        tip: String,
    },
}

/// A turn a restart cut off (see [`Board::cut_off_request`]).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CutOff {
    pub(crate) request: String,
    /// When the conversation last recorded anything before the stop.
    pub(crate) at_ms: i64,
    /// A turn of it had begun in the CLI, this one or an earlier (else it was still starting,
    /// and the thread never had the message).
    pub(crate) began: bool,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct Board {
    pub(crate) tasks: HashMap<TaskId, Task>,
    pub(crate) approvals: HashMap<CardId, Approval>,
    pub(crate) questions: HashMap<CardId, Question>,
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
    /// Where the active (or last) turn started in the stream: its own steps come after it.
    pub(crate) run_since: i64,
    /// The requests that settled with their branch ahead of base: the head then, and where
    /// it was recorded.
    pub(crate) ready: HashMap<String, (String, i64)>,
    /// Its branch's pull request as last seen on GitHub (`PullRequestSeen`).
    pub(crate) pull_request: Option<Box<crate::work::PullRequest>>,
    /// The request whose turn was still starting or running when the daemon last stopped (a
    /// quit or a crash cut it off), and when the conversation last recorded anything, which is
    /// as far as it is known to have worked: found when the board is first read after a start,
    /// and taken once by the start-up recovery, which marks it interrupted then.
    pub(crate) cut_off_request: Option<CutOff>,
    /// The last message of the branch shown, and the stream sequence that set it.
    pub(crate) head: Option<(String, i64)>,
    pub(crate) streaming: Option<StreamingMessage>,
    pub(crate) thinking: Vec<crate::model::ThinkingSegment>,
    pub(crate) notices: Vec<Notice>,
    /// What was decided on the user's behalf, in stream order.
    pub(crate) decisions: Vec<Decision>,
    /// The MCP servers of each of its agents whose CLI runs (by task id; `None`: its own
    /// model's), with the daemon process that heard them ([`crate::boot_id`]).
    pub(crate) mcp_servers: HashMap<Option<String>, (String, crate::model::AgentMcpServers)>,
    /// Per branch: the tip up to which the thread's commits were looked at.
    pub(crate) thread_tips: HashMap<String, String>,
    /// Per branch: where the thread's commits were first looked for, which its self-edit
    /// metric counts from (THREAD-PLAN.md Q13).
    pub(crate) thread_starts: HashMap<String, String>,
    /// What the thread is known to have written without its trailer: its commit ranges, and
    /// its local checkout's files per request, oldest first (its self-edit metric, UI audit
    /// M6).
    pub(crate) thread_authored: Vec<Authored>,
    /// The thread's command outputs stored whole, by alias (`out-<id>`).
    pub(crate) outputs: HashMap<String, crate::work::StoredOutput>,
    /// What the thread read and searched, bounded ([`crate::manager::reads::ReadLog`]);
    /// shared, so copying a board doesn't copy it.
    pub(crate) thread_reads: std::sync::Arc<crate::manager::reads::ReadLog>,
    /// Its previews, by id.
    pub(crate) previews: HashMap<String, crate::work::Preview>,
    /// Highest preview number ever recorded, including cleared rows.
    pub(crate) preview_number: u64,
    /// The ended previews that left the Background rows (Q94); a preview started again under
    /// its id is back.
    pub(crate) background_cleared: HashSet<String>,
    /// Its messages to and from other sessions, by message id.
    pub(crate) peer_messages: HashMap<String, PeerRecord>,
}

impl Board {
    /// The conversation's activity, or `None` when nothing of it runs or waits for the user,
    /// with `turn`, where its CLI's turn stands ([`activity_of`]).
    pub(crate) fn activity(
        &self,
        id: &ConversationId,
        turn: CliTurn,
    ) -> Option<ConversationActivity> {
        let open: Vec<_> = self
            .tasks
            .values()
            .filter(|task| !task.state.is_final())
            .collect();
        let tasks: Vec<_> = open
            .iter()
            .map(|task| (task.id.clone(), task.state))
            .collect();
        let titles = open
            .iter()
            .map(|task| (task.id.clone(), task.title.clone()))
            .collect();
        let approvals: Vec<_> = self
            .approvals
            .values()
            .filter(|approval| approval.state == CardState::Pending)
            .map(|approval| approval.id.clone())
            .collect();
        let questions: Vec<_> = self
            .questions
            .values()
            .filter(|question| question.answer.is_none())
            .map(|question| question.id.clone())
            .collect();
        let sign_ins = sign_ins(
            self.mcp_servers_of(None)
                .map_or(&[][..], |agent| &agent.servers),
        );
        activity_of(
            ConversationActivity {
                conversation_id: id.clone(),
                run: self.run,
                turn_open: turn.open,
                workers_working: false,
                commands_working: false,
                turn_ended_at_ms: turn.turn_ended_at_ms,
                tasks,
                titles,
                approvals,
                questions,
                sign_ins,
                failed: false,
                stopped: false,
            },
            turn.last_end,
        )
    }

    /// The session's state with nothing running or waiting: ready, merged, reverted or none,
    /// as [`Self::tab_state`] reads `ahead`, `reverted` and the records.
    pub(crate) fn settled_state(&self, ahead: Option<u32>, reverted: bool) -> Option<TabState> {
        let merged = self.merged_state(ahead);
        if merged == Some(TabState::Merged) && reverted {
            return Some(TabState::Reverted);
        }
        merged
    }

    /// The commit on the base its latest merge made, and how many commits it brought, when
    /// recorded: what a revert on the base would name.
    pub(crate) fn latest_merge(&self) -> Option<(String, u32)> {
        self.orchestrator_steps
            .iter()
            .filter_map(|step| match &step.kind {
                OrchestratorStepKind::Merged {
                    merge_commit: Some(commit),
                    commits,
                    ..
                } => Some((step.position, commit.clone(), *commits)),
                _ => None,
            })
            .max_by_key(|(position, ..)| *position)
            .map(|(_, commit, commits)| (commit, commits))
    }

    /// Whether the thread attached proof (`attach_proof`) since its last merge.
    pub(crate) fn proof_since_merge(&self) -> bool {
        let merged = self
            .orchestrator_steps
            .iter()
            .filter(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
            .map(|step| step.position)
            .max();
        self.orchestrator_steps.iter().any(|step| {
            matches!(step.kind, OrchestratorStepKind::Proof { .. })
                && merged.is_none_or(|merged| step.position > merged)
        })
    }

    /// Ready, merged or none, from `ahead` and the records.
    fn merged_state(&self, ahead: Option<u32>) -> Option<TabState> {
        let merged = self
            .orchestrator_steps
            .iter()
            .filter(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
            .map(|step| step.position)
            .max();
        let ready = self.ready.values().map(|(_, at)| *at).max();
        match ahead {
            Some(0) => (merged.is_some() || ready.is_some()).then_some(TabState::Merged),
            Some(_) => Some(TabState::Ready),
            None => match (ready, merged) {
                (Some(ready), merged) if merged.is_none_or(|merged| ready > merged) => {
                    Some(TabState::Ready)
                }
                (_, Some(_)) => Some(TabState::Merged),
                _ => None,
            },
        }
    }

    /// The requests that settled ready since the last merge, oldest first, with the head each
    /// left.
    pub(crate) fn unmerged_ready(&self) -> Vec<(String, String)> {
        let merged = self
            .orchestrator_steps
            .iter()
            .filter(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
            .map(|step| step.position)
            .max();
        let mut requests: Vec<(i64, String, String)> = self
            .ready
            .iter()
            .filter(|(_, (_, at))| merged.is_none_or(|merged| *at > merged))
            .map(|(request, (head, at))| (*at, request.clone(), head.clone()))
            .collect();
        requests.sort();
        requests
            .into_iter()
            .map(|(_, request, head)| (request, head))
            .collect()
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
            DomainEvent::McpServersUpdated {
                task_id,
                provider,
                servers,
                at_start,
                boot,
                ..
            } => {
                if servers.is_empty() {
                    self.mcp_servers.remove(task_id);
                } else {
                    self.mcp_servers.insert(
                        task_id.clone(),
                        (
                            boot.clone(),
                            crate::model::AgentMcpServers {
                                task_id: task_id.clone(),
                                provider: *provider,
                                servers: servers.clone(),
                                at_start: *at_start,
                            },
                        ),
                    );
                }
            }
            DomainEvent::ThreadCommitsSeen {
                branch,
                tip,
                authored_from,
                ..
            } => {
                self.thread_starts
                    .entry(branch.clone())
                    .or_insert_with(|| tip.clone());
                self.thread_tips.insert(branch.clone(), tip.clone());
                if let Some(from) = authored_from {
                    self.thread_authored.push(Authored::Commits {
                        branch: branch.clone(),
                        from: from.clone(),
                        tip: tip.clone(),
                    });
                }
            }
            DomainEvent::ThreadFilesSeen {
                request_id,
                branch,
                start,
                tip,
                ..
            } => {
                let same = |known: &Authored| {
                    matches!(known, Authored::Files { request_id: request, start: from, .. }
                        if request == request_id && from == start)
                };
                match self
                    .thread_authored
                    .iter_mut()
                    .rev()
                    .find(|known| same(known))
                {
                    Some(Authored::Files { tip: last, .. }) => last.clone_from(tip),
                    _ => self.thread_authored.push(Authored::Files {
                        request_id: request_id.clone(),
                        branch: branch.clone(),
                        start: start.clone(),
                        tip: tip.clone(),
                    }),
                }
            }
            DomainEvent::OutputStored { output, .. } => {
                self.outputs.insert(output.alias.clone(), output.clone());
            }
            DomainEvent::ThreadLooked {
                reads, searches, ..
            } => std::sync::Arc::make_mut(&mut self.thread_reads).apply(reads, searches),
            DomainEvent::PreviewUpdated { preview } => {
                if let Some(number) = preview
                    .id
                    .strip_prefix("preview-")
                    .and_then(|n| n.parse().ok())
                {
                    self.preview_number = self.preview_number.max(number);
                }
                if preview.state.is_live() {
                    self.background_cleared.remove(&preview.id);
                }
                self.previews.insert(preview.id.clone(), preview.clone());
            }
            DomainEvent::BackgroundCleared { ids, .. } => {
                self.background_cleared.extend(ids.iter().cloned());
            }
            DomainEvent::PreviewsCleared { ids, .. } => {
                self.previews
                    .retain(|id, preview| preview.state.is_live() || !ids.contains(id));
            }
            DomainEvent::QueueChanged { queue, .. } => self.queue = queue.clone(),
            DomainEvent::RunStateChanged {
                state, request_id, ..
            } => {
                let live =
                    |state: RunState| matches!(state, RunState::Starting | RunState::Running);
                if live(*state) && !live(self.run) {
                    self.run_since = stream_seq;
                }
                self.run = *state;
                self.run_request.clone_from(request_id);
                if *state != RunState::Running {
                    self.streaming = None;
                }
            }
            DomainEvent::RequestUpdated { request } => {
                self.requests.insert(request.id.clone(), request.clone());
            }
            DomainEvent::RequestReady {
                request_id, head, ..
            } => {
                self.ready
                    .insert(request_id.clone(), (head.clone(), stream_seq));
            }
            DomainEvent::PullRequestSeen { pull_request, .. } => {
                self.pull_request.clone_from(pull_request);
            }
            DomainEvent::PeerMessageSent {
                message_id,
                to,
                text,
                blob,
                reply_to,
                chain_id,
                hop,
                request_id,
                state,
                sent_at_ms,
                automatic,
                answer_id,
                ..
            } => {
                self.peer_messages.insert(
                    message_id.clone(),
                    PeerRecord {
                        message_id: message_id.clone(),
                        outgoing: true,
                        peer: to.clone(),
                        reply_to: reply_to.clone(),
                        chain_id: chain_id.clone(),
                        hop: *hop,
                        request_id: request_id.clone(),
                        state: state.clone(),
                        text: text.clone(),
                        blob: blob.clone(),
                        sent_at_ms: *sent_at_ms,
                        settled: false,
                        position: stream_seq,
                        automatic: *automatic,
                        answer_id: answer_id.clone(),
                        hold: None,
                    },
                );
            }
            DomainEvent::PeerMessageReceived {
                message_id,
                from,
                reply_to,
                chain_id,
                hop,
                request_id,
                automatic,
                answer_id,
            } => {
                self.peer_messages.insert(
                    message_id.clone(),
                    PeerRecord {
                        message_id: message_id.clone(),
                        outgoing: false,
                        peer: from.clone(),
                        reply_to: reply_to.clone(),
                        chain_id: chain_id.clone(),
                        hop: *hop,
                        request_id: Some(request_id.clone()),
                        state: PeerState::Pending,
                        text: String::new(),
                        blob: None,
                        sent_at_ms: None,
                        settled: false,
                        position: stream_seq,
                        automatic: *automatic,
                        answer_id: answer_id.clone(),
                        hold: None,
                    },
                );
            }
            DomainEvent::PeerMessageState {
                message_id,
                state,
                sent_at_ms,
            } => {
                if let Some(record) = self.peer_messages.get_mut(message_id) {
                    // Delivery never goes back: a late `delivered` or `seen` written after the
                    // receiver already took or answered it changes nothing.
                    let behind = match state {
                        PeerState::Delivered => {
                            matches!(record.state, PeerState::Seen | PeerState::Replied)
                        }
                        PeerState::Seen => record.state == PeerState::Replied,
                        _ => false,
                    };
                    if !behind {
                        record.state = state.clone();
                    }
                    if sent_at_ms.is_some() {
                        record.sent_at_ms = *sent_at_ms;
                    }
                }
            }
            DomainEvent::PeerWaitSettled { message_id } => {
                if let Some(record) = self.peer_messages.get_mut(message_id) {
                    record.settled = true;
                }
            }
            DomainEvent::PeerReplyHeld { message_id, hold } => {
                if let Some(record) = self.peer_messages.get_mut(message_id) {
                    record.hold = Some(hold.clone());
                }
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
                        // Input and result updates stay where the call first appeared, in
                        // the turn it started in.
                        if let (
                            crate::work::OrchestratorStepKind::Tool { turn: started, .. },
                            crate::work::OrchestratorStepKind::Tool { turn, .. },
                        ) = (&known.kind, &mut step.kind)
                            && started.is_some()
                        {
                            *turn = *started;
                        }
                        known.kind = step.kind;
                        return;
                    }
                }
                // A held merge's later tips stay where it was held. Held again on the same
                // consent after it ended (a stop), it is a new hold, in its own place.
                if let crate::work::OrchestratorStepKind::MergeHeld { asked_in, .. } = &step.kind
                    && let Some(at) = self.orchestrator_steps.iter().rposition(|known| {
                        matches!(&known.kind, crate::work::OrchestratorStepKind::MergeHeld { asked_in: a, .. } if a == asked_in)
                    })
                    && !self.orchestrator_steps[at + 1..]
                        .iter()
                        .any(|later| later.kind.ends_merge(asked_in))
                {
                    self.orchestrator_steps[at].kind = step.kind;
                    return;
                }
                // A CI watch's updates stay where it started: one row per repository, branch
                // and sha, since the same push may go to two repositories.
                if let crate::work::OrchestratorStepKind::CiWatch {
                    repo, sha, branch, ..
                } = &step.kind
                    && let Some(known) = self.orchestrator_steps.iter_mut().find(|known| {
                        matches!(&known.kind, crate::work::OrchestratorStepKind::CiWatch { repo: r, sha: s, branch: b, .. } if r == repo && s == sha && b == branch)
                    })
                {
                    // Watched again from a later request: the row is that request's now, so it
                    // works while the watch runs. An earlier watch's update doesn't take it back.
                    let started = |request: &Option<String>| {
                        request
                            .as_ref()
                            .and_then(|id| self.requests.get(id))
                            .map(|request| (request.started_at_ms, request.id.clone()))
                    };
                    if step.request_id.is_some()
                        && started(&step.request_id) > started(&known.request_id)
                    {
                        known.request_id = step.request_id;
                    }
                    // Its result row keeps when it ended: the app shows the result for a
                    // moment after that, a reopened conversation too.
                    if matches!(&step.kind, crate::work::OrchestratorStepKind::CiWatch { result: Some(_), .. }) {
                        known.at_ms = step.at_ms;
                    }
                    known.kind = step.kind;
                    return;
                }
                // A feedback offer answered again (a Try again filed the rest) keeps one answer,
                // with what was filed by now.
                if let crate::work::OrchestratorStepKind::FeedbackAnswered { id, .. } = &step.kind
                    && let Some(known) = self.orchestrator_steps.iter_mut().find(|known| {
                        matches!(&known.kind, crate::work::OrchestratorStepKind::FeedbackAnswered { id: answered, .. } if answered == id)
                    })
                {
                    known.kind = step.kind;
                    return;
                }
                step.position = stream_seq;
                self.orchestrator_steps.push(step);
            }
            DomainEvent::MachineStepped { step, .. } => {
                let mut step = step.clone();
                step.position = stream_seq;
                self.machine_steps.push(step);
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
                self.decisions.push(decision);
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
            DomainEvent::RatingCleared { subject } => {
                self.ratings.remove(subject);
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
                message_id,
                text,
                phase,
                ..
            } => match &mut self.streaming {
                Some(streaming) if streaming.message_id == *message_id => {
                    streaming.text.push_str(text);
                    streaming.phase = streaming.phase.or(*phase);
                }
                _ => {
                    self.streaming = Some(StreamingMessage {
                        message_id: message_id.clone(),
                        text: text.clone(),
                        request_id: self.run_request.clone(),
                        phase: *phase,
                        started_at_ms: crate::now_ms(),
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

    pub(crate) fn sorted_compactions(&self) -> Vec<Compaction> {
        let mut compactions: Vec<Compaction> = self.compactions.values().cloned().collect();
        compactions.sort_by_key(|compaction| compaction.position);
        compactions
    }

    /// The MCP servers of `task_id`'s agent (`None`: the conversation's own model), as this
    /// daemon process heard them.
    pub(crate) fn mcp_servers_of(
        &self,
        task_id: Option<&str>,
    ) -> Option<&crate::model::AgentMcpServers> {
        self.mcp_servers
            .get(&task_id.map(str::to_owned))
            .filter(|(boot, _)| boot == crate::boot_id())
            .map(|(_, servers)| servers)
    }

    /// The MCP servers of its agents as this daemon process heard them: its own model's
    /// first, then each worker's by task id. What an earlier process heard is gone with its
    /// CLIs.
    pub(crate) fn current_mcp_servers(&self) -> Vec<crate::model::AgentMcpServers> {
        let mut agents: Vec<_> = self
            .mcp_servers
            .values()
            .filter(|(boot, _)| boot == crate::boot_id())
            .map(|(_, servers)| servers.clone())
            .collect();
        agents.sort_by(|a, b| a.task_id.cmp(&b.task_id));
        agents
    }

    /// Its previews, oldest first.
    pub(crate) fn sorted_previews(&self) -> Vec<crate::work::Preview> {
        let mut previews: Vec<_> = self.previews.values().cloned().collect();
        previews.sort_by(|a, b| a.started_at_ms.cmp(&b.started_at_ms).then(a.id.cmp(&b.id)));
        previews
    }

    /// The ended previews that left the Background rows, by id.
    pub(crate) fn sorted_background_cleared(&self) -> Vec<String> {
        let mut ids: Vec<_> = self.background_cleared.iter().cloned().collect();
        ids.sort();
        ids
    }

    /// Its messages to and from other sessions, in stream order.
    pub(crate) fn sorted_peer_messages(&self) -> Vec<crate::model::PeerMessage> {
        let mut records: Vec<&PeerRecord> = self.peer_messages.values().collect();
        records.sort_by_key(|record| record.position);
        records
            .into_iter()
            .map(|record| crate::model::PeerMessage {
                message_id: record.message_id.clone(),
                outgoing: record.outgoing,
                peer: record.peer.clone(),
                reply_to: record.reply_to.clone(),
                request_id: record.request_id.clone(),
                state: record.state.clone(),
                text: record.text.clone(),
                blob: record.blob.clone(),
                sent_at_ms: record.sent_at_ms,
                settled: record.settled,
                position: record.position,
                automatic: record.automatic,
                answer_id: record.answer_id.clone(),
                hold: record.hold.clone(),
            })
            .collect()
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

#[cfg(test)]
mod ci_tests {
    use super::*;
    use crate::work::{OrchestratorStep, OrchestratorStepKind};

    fn watch(repo: &str, result: Option<&str>) -> DomainEvent {
        DomainEvent::OrchestratorStepped {
            step: OrchestratorStep {
                request_id: None,
                kind: OrchestratorStepKind::CiWatch {
                    repo: repo.into(),
                    branch: "main".into(),
                    sha: "97c298d5".into(),
                    waiting: false,
                    runs: Vec::new(),
                    result: result.map(str::to_owned),
                },
                at_ms: 0,
                position: 0,
            },
        }
    }

    #[test]
    fn a_ci_watch_is_one_row_per_repository_branch_and_sha() {
        let mut board = Board::default();
        board.apply(&watch("o/fork", None), 1);
        board.apply(&watch("o/upstream", None), 2);
        board.apply(&watch("o/fork", Some("failed")), 3);
        board.apply(&watch("o/upstream", Some("passed")), 4);
        let rows: Vec<(String, Option<String>, i64)> = board
            .orchestrator_steps
            .iter()
            .filter_map(|step| match &step.kind {
                OrchestratorStepKind::CiWatch { repo, result, .. } => {
                    Some((repo.clone(), result.clone(), step.position))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("o/fork".into(), Some("failed".into()), 1),
                ("o/upstream".into(), Some("passed".into()), 2),
            ]
        );
    }

    /// A CI watch alone is no work of the session: its CLI's turn decides.
    #[test]
    fn a_ci_watch_alone_is_no_activity() {
        let id = ConversationId("c1".into());
        let mut board = Board::default();
        board.apply(&watch("o/n", None), 1);
        assert_eq!(board.activity(&id, CliTurn::default()), None);
    }

    /// A reopened conversation's board shows a watch's result for a moment after it ended, as
    /// the live one does: its row takes the time of the result, not of the watch's start.
    #[test]
    fn a_ci_watchs_row_keeps_when_its_result_came() {
        let at = |event: DomainEvent, at_ms: i64| match event {
            DomainEvent::OrchestratorStepped { mut step } => {
                step.at_ms = at_ms;
                DomainEvent::OrchestratorStepped { step }
            }
            _ => unreachable!(),
        };
        let mut board = Board::default();
        board.apply(&at(watch("o/fork", None), 1_000), 1);
        board.apply(&at(watch("o/fork", None), 5_000), 2);
        assert_eq!(board.orchestrator_steps[0].at_ms, 1_000);
        board.apply(&at(watch("o/fork", Some("passed")), 90_000), 3);
        assert_eq!(board.orchestrator_steps[0].at_ms, 90_000);
    }
}

#[cfg(test)]
mod feedback_tests {
    use super::*;
    use crate::feedback::FeedbackSent;
    use crate::work::{FeedbackKind, OrchestratorStep, OrchestratorStepKind};

    fn answered(id: &str, results: Vec<FeedbackSent>) -> DomainEvent {
        DomainEvent::OrchestratorStepped {
            step: OrchestratorStep {
                request_id: Some("request".into()),
                kind: OrchestratorStepKind::FeedbackAnswered {
                    id: id.into(),
                    sent: true,
                    results,
                },
                at_ms: 0,
                position: 0,
            },
        }
    }

    fn filed(number: u64) -> FeedbackSent {
        FeedbackSent::Filed {
            kind: FeedbackKind::Bug,
            title: format!("Report {number}"),
            number,
            url: format!("https://github.com/syncra-studio/brigadier-ai/issues/{number}"),
        }
    }

    /// A sent feedback card's issues are stored with its answer, so a board rebuilt from the
    /// stored events after a restart still has them (#476); a Try again that filed the rest
    /// answers again, in the first answer's place.
    #[test]
    fn a_sent_feedback_answer_keeps_its_issues_through_a_restart() {
        let stored: Vec<String> = [
            answered("offer", vec![filed(482)]),
            answered("offer", vec![filed(482), filed(483)]),
        ]
        .iter()
        .map(|event| serde_json::to_string(event).unwrap())
        .collect();
        assert!(stored[0].contains(
            r#""results":[{"outcome":"filed","kind":"bug","title":"Report 482","number":482,"url":"#
        ));
        let mut board = Board::default();
        for (seq, event) in stored.iter().enumerate() {
            board.apply(&serde_json::from_str(event).unwrap(), seq as i64 + 1);
        }
        let answers: Vec<_> = board
            .orchestrator_steps
            .iter()
            .filter_map(|step| match &step.kind {
                OrchestratorStepKind::FeedbackAnswered { results, .. } => Some(results.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(answers, vec![vec![filed(482), filed(483)]]);
        assert_eq!(board.orchestrator_steps[0].position, 1);
        // Answered before answers kept their issues: none.
        let old: DomainEvent = serde_json::from_str(
            r#"{"type":"orchestratorStepped","step":{"requestId":null,"kind":{"type":"feedbackAnswered","id":"old","sent":true},"atMs":0,"position":0}}"#,
        )
        .unwrap();
        board.apply(&old, 3);
        assert!(matches!(
            &board.orchestrator_steps[1].kind,
            OrchestratorStepKind::FeedbackAnswered { id, results, .. } if id == "old" && results.is_empty()
        ));
    }
}

#[cfg(test)]
mod rating_tests {
    use super::*;

    /// A plan is rated like an answer, and either rating can be taken back (#127).
    #[test]
    fn a_plan_or_answer_rating_is_kept_until_cleared() {
        let mut board = Board::default();
        let rated = |subject: &str, rating| DomainEvent::MessageRated {
            subject: subject.into(),
            rating,
        };
        board.apply(&rated("plan:p1", Rating::Good), 1);
        board.apply(&rated("m1", Rating::Bad), 2);
        assert_eq!(board.ratings.get("plan:p1"), Some(&Rating::Good));
        board.apply(
            &DomainEvent::RatingCleared {
                subject: "plan:p1".into(),
            },
            3,
        );
        assert_eq!(board.ratings.get("plan:p1"), None);
        assert_eq!(board.ratings.get("m1"), Some(&Rating::Bad));
        // Rated again after a clear, it counts again.
        board.apply(&rated("plan:p1", Rating::Bad), 4);
        assert_eq!(board.ratings.get("plan:p1"), Some(&Rating::Bad));
    }

    /// A board rebuilt from the stored events replays a clear too, so a cleared rating stays
    /// cleared after a restart.
    #[test]
    fn a_rebuilt_board_replays_ratings_and_their_clears() {
        let rated = DomainEvent::MessageRated {
            subject: "m1".into(),
            rating: Rating::Good,
        };
        let cleared = DomainEvent::RatingCleared {
            subject: "m1".into(),
        };
        assert!(KINDS.contains(&rated.kind()));
        assert!(KINDS.contains(&cleared.kind()));
    }
}

/// `activity` with its failed and stopped marks from its run and `last_end`, how its CLI's last
/// turn ended, or `None` when nothing of it runs or waits. Its CLI works while its turn is open
/// or Brigadier hands it a message (starting); otherwise it failed when the CLI did not take
/// the turn (a failed run) or its last turn failed or was cut off, and stopped when the user
/// stopped its last turn. Its workers don't decide it.
pub(crate) fn activity_of(
    mut activity: ConversationActivity,
    last_end: Option<TurnEnd>,
) -> Option<ConversationActivity> {
    let working = activity.turn_open || activity.run == RunState::Starting;
    activity.failed = !working
        && (activity.run == RunState::Failed
            || matches!(last_end, Some(TurnEnd::Failed | TurnEnd::Interrupted)));
    activity.stopped = !working && last_end == Some(TurnEnd::Stopped);
    (working
        || activity.run == RunState::Running
        || activity.turn_ended_at_ms.is_some()
        || activity.failed
        || activity.stopped
        || !activity.tasks.is_empty()
        || !activity.approvals.is_empty()
        || !activity.questions.is_empty()
        || !activity.sign_ins.is_empty())
    .then_some(activity)
}

/// The names of the CLI's MCP servers in `servers` that want the user to sign in. A claude.ai
/// connector is left out: the user manages those on claude.ai, and many stay unsigned on
/// purpose.
pub(crate) fn sign_ins(servers: &[McpServerState]) -> Vec<String> {
    servers
        .iter()
        .filter(|server| {
            server.status == McpStatus::NeedsAuth && !server.name.starts_with("claude.ai ")
        })
        .map(|server| server.name.clone())
        .collect()
}

/// The sidebar's states table the app follows: the engine marks its failures and stops.
#[cfg(test)]
mod shared_states_tests {
    use super::*;
    use crate::work::TaskState;

    #[derive(serde::Deserialize)]
    struct Server {
        name: String,
        status: McpStatus,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Case {
        name: String,
        #[serde(default)]
        turn: CliTurn,
        #[serde(default)]
        run: RunState,
        #[serde(default)]
        tasks: Vec<(TaskId, TaskState)>,
        #[serde(default)]
        approvals: Vec<CardId>,
        #[serde(default)]
        questions: Vec<CardId>,
        #[serde(default)]
        servers: Vec<Server>,
        /// The app's words for it: a tab state, or "failed" or "stopped".
        status: Option<String>,
    }

    /// The app names a failure or a stop where the engine marks it.
    #[test]
    fn the_engine_marks_each_shared_case_as_the_app_does() {
        let cases: Vec<Case> = serde_json::from_str(include_str!("sidebar_states.json")).unwrap();
        assert!(cases.len() >= 20);
        for case in cases {
            let servers: Vec<_> = case
                .servers
                .into_iter()
                .map(|server| McpServerState {
                    name: server.name,
                    status: server.status,
                    tools: Vec::new(),
                    transport: None,
                })
                .collect();
            let activity = activity_of(
                ConversationActivity {
                    conversation_id: ConversationId("s".into()),
                    run: case.run,
                    turn_open: case.turn.open,
                    workers_working: false,
                    commands_working: false,
                    turn_ended_at_ms: case.turn.turn_ended_at_ms,
                    tasks: case.tasks,
                    titles: Vec::new(),
                    approvals: case.approvals.into_iter().collect(),
                    questions: case.questions,
                    sign_ins: sign_ins(&servers),
                    failed: false,
                    stopped: false,
                },
                case.turn.last_end,
            );
            let marked = |word: &str| case.status.as_deref() == Some(word);
            if marked("failed") {
                assert!(activity.as_ref().is_some_and(|a| a.failed), "{}", case.name);
            }
            if marked("stopped") {
                assert!(
                    activity.as_ref().is_some_and(|a| a.stopped),
                    "{}",
                    case.name
                );
            }
        }
    }
}

#[cfg(test)]
mod tab_state_tests {
    use super::*;
    use crate::work::TaskState;

    /// No turn of the CLI open, and none ended.
    const IDLE: CliTurn = CliTurn {
        open: false,
        last_end: None,
        turn_ended_at_ms: None,
    };

    fn id() -> ConversationId {
        ConversationId("tab".into())
    }

    fn event(json: serde_json::Value) -> DomainEvent {
        serde_json::from_value(json).unwrap()
    }

    fn ready(request: &str) -> DomainEvent {
        DomainEvent::RequestReady {
            conversation_id: id(),
            request_id: request.into(),
            head: "abc123".into(),
        }
    }

    /// A last turn the user stopped marks the session stopped, through a reconnect (the board
    /// and the record are read again), until the next turn opens.
    #[test]
    fn a_stopped_turn_keeps_its_sidebar_mark_until_the_next_turn() {
        let board = Board::default();
        let stopped = CliTurn {
            open: false,
            last_end: Some(TurnEnd::Stopped),
            turn_ended_at_ms: Some(100),
        };
        assert!(board.activity(&id(), stopped).unwrap().stopped);
        let next = CliTurn {
            open: true,
            ..stopped
        };
        let activity = board.activity(&id(), next).unwrap();
        assert!(!activity.stopped && activity.turn_open);
        let done = CliTurn {
            open: false,
            last_end: Some(TurnEnd::Done),
            turn_ended_at_ms: Some(200),
        };
        assert_eq!(
            board.activity(&id(), done).unwrap().turn_ended_at_ms,
            Some(200)
        );
    }

    /// A session's open workers come with their titles, for their rows under its sidebar row;
    /// one that ended leaves both.
    #[test]
    fn the_activity_names_its_open_workers() {
        let tab = id();
        let mut board = Board::default();
        let task = |id: &str, title: &str, state: &str| {
            DomainEvent::TaskUpdated {
            task: Box::new(
                serde_json::from_value(serde_json::json!({
                    "id": id, "conversationId": tab.0, "number": 1, "position": 0,
                    "title": title, "kind": "implement", "spec": "Do it.",
                    "access": { "repo": "write", "network": false, "unsandboxed": false },
                    "route": { "choice": { "provider": "claude", "model": null, "effort": null }, "reason": "" },
                    "state": state, "attachments": [], "createdAtMs": 0, "updatedAtMs": 0
                }))
                .expect("a task"),
            ),
        }
        };
        board.apply(&task("t1", "Fix the header", "running"), 1);
        board.apply(&task("t2", "Old work", "landed"), 2);
        let activity = board.activity(&tab, IDLE).expect("a worker runs");
        assert_eq!(
            activity.tasks,
            vec![(TaskId("t1".into()), TaskState::Running)]
        );
        assert_eq!(
            activity.titles,
            vec![(TaskId("t1".into()), "Fix the header".to_owned())]
        );
        board.apply(&task("t1", "Fix the header", "done"), 3);
        assert!(board.activity(&tab, IDLE).is_none());
    }

    fn merged_at(position: i64, commit: &str) -> DomainEvent {
        event(serde_json::json!({
            "type": "orchestratorStepped",
            "step": {
                "requestId": "r2", "atMs": 0, "position": position,
                "kind": {
                    "type": "merged", "branch": "brigadier/tab/session", "base": "main",
                    "commits": 2, "mergeCommit": commit,
                },
            },
        }))
    }

    /// Proof counts from the last merge on; a pull request seen is kept until seen otherwise.
    #[test]
    fn proof_since_the_merge_and_the_pull_request_seen() {
        let proof = |position: i64| {
            event(serde_json::json!({
                "type": "orchestratorStepped",
                "step": {
                    "requestId": "r1", "atMs": 0, "position": position,
                    "kind": {
                        "type": "proof", "id": "p1", "name": "shot.png", "mime": "image/png",
                        "file": "proof/p1.png",
                    },
                },
            }))
        };
        let mut board = Board::default();
        assert!(!board.proof_since_merge());
        board.apply(&proof(1), 1);
        assert!(board.proof_since_merge());
        board.apply(&merged_at(2, "m1"), 2);
        assert!(!board.proof_since_merge(), "proof of merged work");
        board.apply(&proof(3), 3);
        assert!(board.proof_since_merge());

        let seen = |state: Option<crate::work::PullRequestState>| DomainEvent::PullRequestSeen {
            conversation_id: id(),
            pull_request: state.map(|state| {
                Box::new(crate::work::PullRequest {
                    number: 7,
                    title: "Header".into(),
                    url: "https://github.com/o/n/pull/7".into(),
                    state,
                    base: None,
                    merge_commit: None,
                    ci: None,
                    approved: false,
                    mergeable: false,
                    conflicts: false,
                    changes_requested: false,
                    unresolved: 0,
                })
            }),
        };
        board.apply(&seen(Some(crate::work::PullRequestState::Open)), 4);
        assert_eq!(
            board.pull_request.as_ref().map(|pr| pr.state),
            Some(crate::work::PullRequestState::Open)
        );
        board.apply(&seen(None), 5);
        assert_eq!(board.pull_request, None);
        assert!(KINDS.contains(&seen(None).kind()));
    }

    /// One ready record per request: the board keeps each request's head.
    #[test]
    fn a_ready_record_is_kept_per_request() {
        let mut board = Board::default();
        board.apply(&ready("r1"), 1);
        board.apply(&ready("r2"), 2);
        assert_eq!(board.ready.len(), 2);
        assert_eq!(board.ready["r1"], ("abc123".to_owned(), 1));
        assert!(KINDS.contains(&ready("r1").kind()));
    }
}
