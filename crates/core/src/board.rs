//! A conversation's board: its tasks, cards, queue and run state, folded from the
//! conversation's stream. Messages are not kept here; they are paged from the store.

use std::collections::{HashMap, HashSet};

use crate::knowledge::MemoryChange;
use crate::model::ConversationId;
use crate::model::{
    DomainEvent, MessageRole, Notice, OvernightRunId, PeerState, Rating, ReplyHold,
    StreamingMessage,
};
use crate::overnight::OvernightRun;
use crate::work::{
    Approval, BackgroundRunState, CardId, CardState, Compaction, ConversationActivity, Decision,
    IntegratedCheckState, MessageQueue, OrchestratorStep, OrchestratorStepKind, Plan, PlanState,
    Question, ReviewRun, ReviewState, RunState, SessionPhase, TabState, Task, TaskId, TaskKind,
    TaskState, UserRequest, WaitingItem, WorkerStep,
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
    "integrated_check.updated",
    "background_run.updated",
    "thread.seen",
    "thread.unreviewed",
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
    "memory.updated",
    "decision.made",
    "waiting.updated",
    "waiting.resolved",
    "overnight.updated",
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

impl PeerRecord {
    /// A message this session sent asking for a reply, still on its way or being worked on:
    /// its request waits for the reply (one waiting for the user's OK waits on its card).
    pub(crate) fn awaits_reply(&self) -> bool {
        self.outgoing
            && self.reply_to.is_none()
            && !self.settled
            && matches!(
                self.state,
                PeerState::Pending | PeerState::Delivered | PeerState::Seen
            )
    }
}

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

/// How many of the thread's check runs a board keeps.
pub(crate) const THREAD_CHECKS_KEPT: usize = 20;

/// One of the thread's `run_check` runs ([`Board::thread_checks`]).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ThreadCheck {
    pub(crate) command: String,
    pub(crate) workdir: String,
    /// The tree of the files it ran on, uncommitted changes included, when known.
    pub(crate) tree: Option<String>,
    pub(crate) status: String,
    pub(crate) position: i64,
}

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
    /// The thread's own `run_check` runs, the latest of each command and folder (at most
    /// [`THREAD_CHECKS_KEPT`]): what a merge record reads.
    pub(crate) thread_checks: Vec<ThreadCheck>,
    /// Its runs of the full checks on the integrated tree, by id.
    pub(crate) integrated_checks: HashMap<String, crate::work::IntegratedCheck>,
    /// The thread's commands that went to the background, by id.
    pub(crate) background_runs: HashMap<String, crate::work::BackgroundRun>,
    /// The thread's headline for the session now (`set_headline`).
    pub(crate) headline: Option<crate::work::SessionHeadline>,
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
    /// The thread's changes for its newest request that no review covered yet (BUILD-PLAN B2).
    /// Boxed: a board is cloned on deep stacks, and every byte of it counts there.
    pub(crate) unreviewed: Option<Box<crate::work::UnreviewedRange>>,
    /// The thread's command outputs stored whole, by alias (`out-<id>`).
    pub(crate) outputs: HashMap<String, crate::work::StoredOutput>,
    /// What the thread read and searched, bounded ([`crate::manager::reads::ReadLog`]);
    /// shared, so copying a board doesn't copy it.
    pub(crate) thread_reads: std::sync::Arc<crate::manager::reads::ReadLog>,
    /// Its previews, by id.
    pub(crate) previews: HashMap<String, crate::work::Preview>,
    /// Highest preview number ever recorded, including cleared rows.
    pub(crate) preview_number: u64,
    /// The ended previews and background runs that left the Background rows (Q94); a preview
    /// started again under its id is back.
    pub(crate) background_cleared: HashSet<String>,
    /// Its messages to and from other sessions, by message id.
    pub(crate) peer_messages: HashMap<String, PeerRecord>,
}

impl Board {
    /// The sessions request `request` waits for a reply from, oldest message first.
    pub(crate) fn awaited_peers(&self, request: &str) -> Vec<ConversationId> {
        let mut awaited: Vec<&PeerRecord> = self
            .peer_messages
            .values()
            .filter(|record| record.request_id.as_deref() == Some(request) && record.awaits_reply())
            .collect();
        awaited.sort_by_key(|record| record.position);
        let mut peers: Vec<ConversationId> = Vec::new();
        for record in awaited {
            if !peers.contains(&record.peer) {
                peers.push(record.peer.clone());
            }
        }
        peers
    }

    /// The conversation's activity, or `None` when it neither runs nor waits for the user. A
    /// failed run or latest request counts as waiting for the user until the next run starts
    /// (a turn's error fails its request; the CLI's failure, its run).
    pub(crate) fn activity(&self, id: &ConversationId) -> Option<ConversationActivity> {
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
        let phases = self.phases(running);
        let failed = !running
            && (self.run == RunState::Failed
                || self.latest_request().is_some_and(|request| {
                    matches!(request.state, crate::work::RequestState::Failed { .. })
                }));
        let stopped = !running
            && self
                .latest_request()
                .is_some_and(|request| matches!(request.state, crate::work::RequestState::Stopped));
        let mut requests: Vec<_> = self
            .requests
            .values()
            .filter(|request| request.state == crate::work::RequestState::Working)
            .map(|request| request.id.clone())
            .collect();
        requests.sort();
        (running
            || stopped
            || failed
            || !requests.is_empty()
            || !tasks.is_empty()
            || !approvals.is_empty()
            || !questions.is_empty()
            || !phases.is_empty())
        .then(|| ConversationActivity {
            conversation_id: id.clone(),
            run: self.run,
            tasks,
            titles,
            approvals,
            questions,
            phases,
            failed,
            stopped,
            requests,
        })
    }

    /// Where the session stands (WORKSPACES-PLAN.md "States"): needs you (a card or question
    /// open, or its last run failed), merging (a merge of its work runs), working (its turn, a
    /// worker or a phase, or `quota_wait`: its messages wait for a limit to reset), ready
    /// (idle with commits its base lacks), merged (a merge of its work, recorded or seen in
    /// git, and nothing newer unmerged), reverted (that merge reverted on the base since,
    /// `reverted`), or none (untouched, or idle with nothing to merge).
    ///
    /// `ahead` is its branch's commits not in base now, from git; without it the records
    /// decide: a request that settled ready after the last merge is ready.
    pub(crate) fn tab_state(
        &self,
        id: &ConversationId,
        quota_wait: bool,
        ahead: Option<u32>,
        reverted: bool,
    ) -> Option<TabState> {
        state_of(self.activity(id).as_ref(), quota_wait, || {
            self.settled_state(ahead, reverted)
        })
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

    /// The requests whose work a merge now takes: those that settled ready since the last
    /// merge, and `current`, the one whose turn merged it.
    pub(crate) fn merging_requests(&self, current: Option<&str>) -> Vec<String> {
        let mut requests: Vec<String> = self
            .unmerged_ready()
            .into_iter()
            .map(|(request, _)| request)
            .collect();
        if let Some(current) = current
            && !requests.iter().any(|request| request == current)
        {
            requests.push(current.to_owned());
        }
        requests
    }

    /// What the conversation is busy with that its sidebar row names, keyed as
    /// [`ConversationActivity::phases`] says; its thread's calls count only while `running`, in
    /// the request its turn serves.
    fn phases(&self, running: bool) -> Vec<(String, SessionPhase)> {
        let tasks = self.tasks.values().filter_map(|task| {
            let works = matches!(
                task.state,
                TaskState::Queued
                    | TaskState::Starting
                    | TaskState::Running
                    | TaskState::Blocked
                    | TaskState::Landing
            );
            let phase = match task.kind {
                TaskKind::Merge => SessionPhase::Merging,
                TaskKind::Review => SessionPhase::Reviewing,
                TaskKind::Verify => SessionPhase::Checking,
                _ => return None,
            };
            works.then(|| (format!("task:{}", task.id), phase))
        });
        let reviews = self
            .reviews
            .values()
            .filter(|review| review.state == ReviewState::Running)
            .map(|review| (format!("review:{}", review.id), SessionPhase::Reviewing));
        let checks = self
            .integrated_checks
            .values()
            .filter(|check| matches!(check.state, IntegratedCheckState::Running { .. }))
            .map(|check| (format!("check:{}", check.id), SessionPhase::Checking));
        let runs = self
            .background_runs
            .values()
            .filter(|run| run.tool == "run_check" && run.state == BackgroundRunState::Running)
            .map(|run| (format!("run:{}", run.id), SessionPhase::Checking));
        let steps = self
            .orchestrator_steps
            .iter()
            .filter_map(|step| match &step.kind {
                OrchestratorStepKind::CiWatch {
                    repo,
                    branch,
                    sha,
                    result: None,
                    ..
                } => Some((
                    format!("ci:{repo} {branch} {sha}"),
                    SessionPhase::WatchingCi,
                )),
                OrchestratorStepKind::Tool {
                    item_id,
                    name,
                    status: brigadier_providers::ItemStatus::InProgress,
                    ..
                    // The active turn's own call only: an earlier turn of the same request
                    // may have left one marked running (#253).
                } if running
                    && step.request_id == self.run_request
                    && step.position >= self.run_since =>
                {
                    let phase = match name.rsplit("__").next().unwrap_or(name) {
                        "run_check" => SessionPhase::Checking,
                        "finish_session" => SessionPhase::Merging,
                        _ => return None,
                    };
                    Some((format!("tool:{item_id}"), phase))
                }
                _ => None,
            });
        let merge = crate::manager::held_merge(self)
            .map(|asked_in| (format!("merge:{asked_in}"), SessionPhase::Merging));
        let mut phases: Vec<_> = tasks
            .chain(reviews)
            .chain(checks)
            .chain(runs)
            .chain(steps)
            .chain(merge)
            .collect();
        phases.sort_by(|a, b| a.0.cmp(&b.0));
        phases
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
                // A merge's record counts the thread's later word on the review's findings too:
                // one it settles after the merge no longer reads open there.
                for step in &mut self.orchestrator_steps {
                    if let crate::work::OrchestratorStepKind::Merged {
                        record: Some(record),
                        ..
                    } = &mut step.kind
                    {
                        for merged in &mut record.reviews {
                            if merged.counts(review) {
                                merged.recount(review);
                            }
                        }
                    }
                }
                self.reviews.insert(review.id.clone(), review.clone());
            }
            DomainEvent::IntegratedCheckUpdated { check } => {
                self.integrated_checks
                    .insert(check.id.clone(), check.clone());
            }
            DomainEvent::BackgroundRunUpdated { run } => {
                self.background_runs.insert(run.id.clone(), run.clone());
            }
            DomainEvent::HeadlineSet { headline, .. } => {
                self.headline = headline.clone();
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
            DomainEvent::UnreviewedChanged { range, .. } => {
                if let Some(range) = range
                    && range.local
                    && let Some(start) = &range.start
                {
                    let same = |known: &Authored| {
                        matches!(known, Authored::Files { request_id, start: from, .. }
                            if *request_id == range.request_id && from == start)
                    };
                    match self
                        .thread_authored
                        .iter_mut()
                        .rev()
                        .find(|known| same(known))
                    {
                        Some(Authored::Files { tip, .. }) => tip.clone_from(&range.tip),
                        _ => self.thread_authored.push(Authored::Files {
                            request_id: range.request_id.clone(),
                            branch: range.branch.clone(),
                            start: start.clone(),
                            tip: range.tip.clone(),
                        }),
                    }
                }
                self.unreviewed = range.clone().map(Box::new);
            }
            DomainEvent::OutputStored { output, .. } => {
                self.outputs.insert(output.alias.clone(), output.clone());
            }
            DomainEvent::CheckRan {
                task_id: None,
                tree,
                command,
                workdir,
                status,
                ..
            } => {
                self.thread_checks
                    .retain(|check| check.command != *command || check.workdir != *workdir);
                self.thread_checks.push(ThreadCheck {
                    command: command.clone(),
                    workdir: workdir.clone(),
                    tree: tree.clone(),
                    status: status.clone(),
                    position: stream_seq,
                });
                if self.thread_checks.len() > THREAD_CHECKS_KEPT {
                    self.thread_checks.remove(0);
                }
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
            // An item from a source Brigadier no longer has stays out of the list.
            DomainEvent::WaitingOnYou { item }
                if item.source == crate::work::WaitingSource::Retired => {}
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

    /// Its runs of the full checks on the integrated tree, oldest first (by number).
    pub(crate) fn sorted_integrated_checks(&self) -> Vec<crate::work::IntegratedCheck> {
        let mut checks: Vec<_> = self.integrated_checks.values().cloned().collect();
        checks.sort_by_key(|check| crate::manager::integrated_check_number(&check.id));
        checks
    }

    /// The thread's background runs, oldest first (by number).
    pub(crate) fn sorted_background_runs(&self) -> Vec<crate::work::BackgroundRun> {
        let mut runs: Vec<_> = self.background_runs.values().cloned().collect();
        runs.sort_by_key(|run| crate::manager::background_run_number(&run.id));
        runs
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

    /// The ended previews and runs that left the Background rows, by id.
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

    /// An idle session watching CI still shows in the sidebar, as "Watching CI" (#234).
    #[test]
    fn a_running_ci_watch_is_the_sessions_phase_until_it_ends() {
        let id = ConversationId("c1".into());
        let mut board = Board::default();
        board.apply(&watch("o/n", None), 1);
        let activity = board
            .activity(&id)
            .expect("a watch keeps the session active");
        assert_eq!(
            activity.phases,
            [(
                "ci:o/n main 97c298d5".to_owned(),
                crate::work::SessionPhase::WatchingCi
            )]
        );
        board.apply(&watch("o/n", Some("passed")), 2);
        assert_eq!(board.activity(&id), None);
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

/// [`Board::tab_state`] from its activity: the first that holds wins, as the app's `tabState`
/// decides it too (both read `session_states.json`'s cases in their tests).
pub(crate) fn state_of(
    activity: Option<&ConversationActivity>,
    quota_wait: bool,
    settled: impl FnOnce() -> Option<TabState>,
) -> Option<TabState> {
    if let Some(activity) = activity {
        // A run a usage limit stopped waits for the reset rather than for the user.
        if !activity.approvals.is_empty()
            || !activity.questions.is_empty()
            || (activity.failed && !quota_wait)
        {
            return Some(TabState::NeedsYou);
        }
        if activity
            .phases
            .iter()
            .any(|(_, phase)| *phase == SessionPhase::Merging)
        {
            return Some(TabState::Merging);
        }
        if matches!(activity.run, RunState::Starting | RunState::Running)
            || !activity.tasks.is_empty()
            || !activity.phases.is_empty()
            || !activity.requests.is_empty()
        {
            return Some(TabState::Working);
        }
    }
    if quota_wait {
        return Some(TabState::Working);
    }
    settled()
}

/// The states table both the engine and the app follow, case by case.
#[cfg(test)]
mod shared_states_tests {
    use super::*;

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Case {
        name: String,
        activity: Option<ConversationActivity>,
        quota_wait: bool,
        settled: Option<TabState>,
        expect: Option<TabState>,
    }

    #[test]
    fn the_engine_reads_each_shared_case_as_the_app_does() {
        let cases: Vec<Case> = serde_json::from_str(include_str!("session_states.json")).unwrap();
        assert!(cases.len() >= 10);
        for case in cases {
            assert_eq!(
                state_of(case.activity.as_ref(), case.quota_wait, || case.settled),
                case.expect,
                "{}",
                case.name
            );
        }
    }
}

#[cfg(test)]
mod tab_state_tests {
    use super::*;
    use crate::work::{Question, QuestionKind};

    fn id() -> ConversationId {
        ConversationId("tab".into())
    }

    fn event(json: serde_json::Value) -> DomainEvent {
        serde_json::from_value(json).unwrap()
    }

    fn run(state: &str) -> DomainEvent {
        event(serde_json::json!({
            "type": "runStateChanged", "conversationId": "tab", "state": state,
            "error": null, "requestId": "r1",
        }))
    }

    fn run_check(status: &str) -> DomainEvent {
        event(serde_json::json!({
            "type": "orchestratorStepped",
            "step": {
                "requestId": "r1", "atMs": 0, "position": 0,
                "kind": {
                    "type": "tool", "itemId": "check-1", "name": "mcp__brigadier__run_check",
                    "detail": null, "status": status, "throughPosition": 0,
                },
            },
        }))
    }

    fn ready(request: &str) -> DomainEvent {
        DomainEvent::RequestReady {
            conversation_id: id(),
            request_id: request.into(),
            head: "abc123".into(),
        }
    }

    fn merged() -> DomainEvent {
        event(serde_json::json!({
            "type": "orchestratorStepped",
            "step": {
                "requestId": "r2", "atMs": 0, "position": 0,
                "kind": { "type": "merged", "branch": "brigadier/tab/session", "base": "main", "commits": 1 },
            },
        }))
    }

    #[test]
    fn a_stopped_request_keeps_its_sidebar_mark_until_the_next_turn() {
        let mut board = Board::default();
        let request = |state: &str| {
            event(serde_json::json!({
                "type": "requestUpdated", "request": {
                    "id": "r1", "conversationId": "tab", "preview": "Do it",
                    "state": { "type": state }, "startedAtMs": 1, "endedAtMs": 2,
                    "steeredInto": null, "steeredAfter": null, "undo": null,
                    "worked": [], "quotaWait": false,
                },
            }))
        };
        board.apply(&request("stopped"), 1);
        assert!(
            board.activity(&id()).unwrap().stopped,
            "a reconnect reads it"
        );
        let mut reloaded = Board::default();
        reloaded.apply(&request("stopped"), 1);
        assert!(
            reloaded.activity(&id()).unwrap().stopped,
            "replaying the stored request keeps it"
        );
        board.apply(&run("running"), 2);
        assert!(!board.activity(&id()).unwrap().stopped);
        board.apply(&request("done"), 3);
        board.apply(&run("idle"), 4);
        assert!(board.activity(&id()).is_none());
    }

    /// Issue 485: a request that works with no turn, worker or phase going (its message waits
    /// for a turn, or a turn is owed) reads Working, as its composer shows it, not Ready.
    #[test]
    fn a_working_request_between_turns_reads_working() {
        let mut board = Board::default();
        let request = |state: &str| {
            event(serde_json::json!({
                "type": "requestUpdated", "request": {
                    "id": "r1", "conversationId": "tab", "preview": "Do it",
                    "state": { "type": state }, "startedAtMs": 1, "endedAtMs": null,
                    "steeredInto": null, "steeredAfter": null, "undo": null,
                    "worked": [], "quotaWait": false,
                },
            }))
        };
        board.apply(&ready("r0"), 1);
        board.apply(&request("working"), 2);
        board.apply(&run("idle"), 3);
        assert_eq!(
            board.activity(&id()).unwrap().requests,
            vec!["r1".to_owned()]
        );
        assert_eq!(
            board.tab_state(&id(), false, None, false),
            Some(TabState::Working)
        );
        board.apply(&request("done"), 4);
        assert!(board.activity(&id()).is_none());
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
        let activity = board.activity(&tab).expect("a worker runs");
        assert_eq!(
            activity.tasks,
            vec![(TaskId("t1".into()), TaskState::Running)]
        );
        assert_eq!(
            activity.titles,
            vec![(TaskId("t1".into()), "Fix the header".to_owned())]
        );
        board.apply(&task("t1", "Fix the header", "done"), 3);
        assert!(board.activity(&tab).is_none());
    }

    /// Each row of the states table, first match winning: a new tab has none (not Merged); an
    /// open question needs the user, over a running turn; a quota wait, a running turn or
    /// a phase works; a request that settled ahead of base is ready; a merge after it is
    /// merged, until a later request settles ahead again; idle with nothing to merge is none.
    #[test]
    fn a_tab_states_from_needs_you_down_to_none() {
        let tab = id();
        let mut board = Board::default();
        assert_eq!(
            board.tab_state(&tab, false, None, false),
            None,
            "an untouched tab"
        );
        assert_eq!(
            board.tab_state(&tab, true, None, false),
            Some(TabState::Working),
            "its messages wait for a limit to reset"
        );

        board.apply(&run("running"), 1);
        assert_eq!(
            board.tab_state(&tab, false, None, false),
            Some(TabState::Working)
        );
        board.questions.insert(
            CardId("q1".into()),
            Question {
                id: CardId("q1".into()),
                conversation_id: tab.clone(),
                task_id: None,
                request_id: Some("r1".into()),
                position: 2,
                kind: QuestionKind::Orchestrator,
                text: "Which colour?".into(),
                options: Vec::new(),
                recommended: None,
                items: Vec::new(),
                answer: None,
                answers: Vec::new(),
                title: None,
                created_at_ms: 0,
                answered_at_ms: None,
                words_id: None,
                answered_by: None,
            },
        );
        assert_eq!(
            board.tab_state(&tab, false, None, false),
            Some(TabState::NeedsYou)
        );
        board.questions.clear();
        board.apply(&run("idle"), 3);
        assert_eq!(
            board.tab_state(&tab, false, None, false),
            None,
            "idle, nothing ahead"
        );

        // A phase alone (a CI watch) is work.
        board.apply(
            &event(serde_json::json!({
                "type": "orchestratorStepped",
                "step": {
                    "requestId": "r1", "atMs": 0, "position": 0,
                    "kind": {
                        "type": "ciWatch", "repo": "o/n", "branch": "main", "sha": "97c298d5",
                        "waiting": false, "runs": [], "result": null, "consent": null,
                    },
                },
            })),
            4,
        );
        assert_eq!(
            board.tab_state(&tab, false, None, false),
            Some(TabState::Working)
        );
        board.orchestrator_steps.clear();

        board.apply(&ready("r1"), 5);
        assert_eq!(
            board.tab_state(&tab, false, None, false),
            Some(TabState::Ready)
        );
        board.apply(&merged(), 6);
        assert_eq!(
            board.tab_state(&tab, false, None, false),
            Some(TabState::Merged)
        );
        board.apply(&ready("r3"), 7);
        assert_eq!(
            board.tab_state(&tab, false, None, false),
            Some(TabState::Ready),
            "newer work"
        );
        board.apply(&merged(), 8);
        assert_eq!(
            board.tab_state(&tab, false, None, false),
            Some(TabState::Merged)
        );
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

    /// A failed run needs the user, in the activity snapshot too (so a reconnect keeps it),
    /// until the next run starts; a limit's wait over a failed run is work, not the user's.
    #[test]
    fn a_failed_run_needs_the_user_until_the_next_one_starts() {
        let tab = id();
        let mut board = Board::default();
        board.apply(&run("failed"), 1);
        let activity = board
            .activity(&tab)
            .expect("a failed run is in the snapshot");
        assert!(activity.failed);
        assert_eq!(
            board.tab_state(&tab, false, None, false),
            Some(TabState::NeedsYou)
        );
        assert_eq!(
            board.tab_state(&tab, true, None, false),
            Some(TabState::Working),
            "the limit's wait wins"
        );
        board.apply(&run("starting"), 2);
        assert_eq!(
            board.tab_state(&tab, false, None, false),
            Some(TabState::Working)
        );
        board.apply(&run("idle"), 3);
        assert!(board.activity(&tab).is_none(), "it cleared");
    }

    /// A merge running reads merging, over the work around it; a reverted latest merge reads
    /// reverted, and newer work ready again.
    #[test]
    fn merging_and_a_reverted_merge() {
        let tab = id();
        let mut board = Board::default();
        board.apply(&run("running"), 1);
        board.apply(
            &event(serde_json::json!({
                "type": "orchestratorStepped",
                "step": {
                    "requestId": "r1", "atMs": 0, "position": 1,
                    "kind": {
                        "type": "tool", "itemId": "fin-1", "name": "mcp__brigadier__finish_session",
                        "detail": null, "status": "inProgress", "throughPosition": 1,
                    },
                },
            })),
            2,
        );
        assert_eq!(
            board.tab_state(&tab, false, None, false),
            Some(TabState::Merging)
        );
        board.orchestrator_steps.clear();
        board.apply(&run("idle"), 3);
        board.apply(&merged_at(4, "m1"), 4);
        assert_eq!(board.latest_merge(), Some(("m1".to_owned(), 2)));
        assert_eq!(
            board.tab_state(&tab, false, Some(0), false),
            Some(TabState::Merged)
        );
        assert_eq!(
            board.tab_state(&tab, false, Some(0), true),
            Some(TabState::Reverted)
        );
        assert_eq!(
            board.tab_state(&tab, false, Some(1), true),
            Some(TabState::Ready),
            "newer work"
        );
        board.apply(&merged_at(9, "m2"), 9);
        assert_eq!(
            board.latest_merge(),
            Some(("m2".to_owned(), 2)),
            "the latest merge"
        );
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

    /// A merge with no recorded ready request (merged right away) still reads merged.
    #[test]
    fn a_merge_alone_reads_merged() {
        let mut board = Board::default();
        board.apply(&merged(), 1);
        assert_eq!(
            board.tab_state(&id(), false, None, false),
            Some(TabState::Merged)
        );
    }

    /// With git's count: commits ahead read ready whatever the records say (a late request
    /// after a merge), none ahead read merged once the work was merged or was ever ready (a
    /// merge from the Source panel or a terminal records no step), and an untouched tab none.
    #[test]
    fn the_branchs_count_now_decides_ready_and_merged() {
        let tab = id();
        let mut board = Board::default();
        assert_eq!(
            board.tab_state(&tab, false, Some(0), false),
            None,
            "untouched"
        );
        assert_eq!(
            board.tab_state(&tab, false, Some(2), false),
            Some(TabState::Ready)
        );
        board.apply(&ready("r1"), 1);
        assert_eq!(
            board.tab_state(&tab, false, Some(0), false),
            Some(TabState::Merged),
            "merged elsewhere"
        );
        board.apply(&merged(), 2);
        assert_eq!(
            board.tab_state(&tab, false, Some(1), false),
            Some(TabState::Ready),
            "newer commits"
        );
        assert_eq!(
            board.tab_state(&tab, false, Some(0), false),
            Some(TabState::Merged)
        );
        board.apply(&run("running"), 3);
        assert_eq!(
            board.tab_state(&tab, false, Some(1), false),
            Some(TabState::Working)
        );
    }

    /// #253: a run_check an earlier turn left marked running is no phase of a later turn of
    /// the same request (a reconnect reads the board); the live turn's own call is.
    #[test]
    fn a_phase_comes_from_the_active_turn_only() {
        let mut board = Board::default();
        board.apply(&run("running"), 1);
        board.apply(&run_check("inProgress"), 2);
        assert_eq!(
            board.activity(&id()).unwrap().phases,
            [("tool:check-1".to_owned(), SessionPhase::Checking)]
        );
        board.apply(&run("idle"), 3);
        // A report starts another turn for the same request.
        board.apply(&run("starting"), 4);
        board.apply(&run("running"), 5);
        assert!(board.activity(&id()).unwrap().phases.is_empty());
        assert_eq!(
            board.tab_state(&id(), false, None, false),
            Some(TabState::Working)
        );
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
