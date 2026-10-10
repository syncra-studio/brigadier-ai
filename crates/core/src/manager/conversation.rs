//! The conversation driver: the orchestrator of a session, or the model of a Chat.
//!
//! One CLI session per conversation, started on the first turn and replaced whenever it is
//! gone. Turns are non-blocking for everyone else:
//!
//! - A message sent while no turn runs starts one. While a turn runs it waits in the queue,
//!   or is steered into the turn when the user asks (a queued message's Steer).
//! - In a session, a message sent while the newest answer still works (its turn, a worker or
//!   a card) is a follow-up: it waits in the queue, marked deciding, while the orchestrator
//!   judges it with `route_follow_up`. One that belongs to that answer joins it (steered in,
//!   shown inside its block); one of its own waits until no answer works, then goes as its
//!   own request. A follow-up the orchestrator didn't judge waits too.
//! - Worker results arrive as [`Envelope`]s in the inbox. Only final reports, blocking
//!   questions, card outcomes and task failures ever enter the orchestrator's context;
//!   worker progress never does. An envelope arriving while the orchestrator is idle starts
//!   a turn.
//! - Each turn serves one user request (see [`crate::work::UserRequest`]). When a turn ends,
//!   the next one carries, in this order: user messages that could not be steered; else
//!   every envelope of one request (a worker's question first); else the next queued
//!   message (unless the queue is paused). So results reach the orchestrator before the
//!   user's next queued message, and never mixed into another request's turn unlabelled.
//!
//! Every byte sent to the orchestrator is logged as a [`ContextInjection`] on `orch:<id>`,
//! next to the CLI's own usage and context-size events, so the Inspector can show that its
//! context grows only by messages and reports.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use brigadier_providers::{
    Access, ApprovalDecision, Artifact, Decider, ErrorKind, InputFile, InputPart, ItemStatus,
    LimitHit, McpServer, Origin, OutputHook, ProviderEvent, ProviderKind, ProviderSession,
    Role as ProviderRole, SessionSpec, Started, ToolSet, TurnInput, TurnStatus,
};
use brigadier_store::StreamPage;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::cold::CacheMark;
use super::prompts;
use super::reads;
use super::rebirth::{self, BriefingPlan, RebirthPrep};
use super::usage::TokenOwner;
use super::{EventSource, SessionManager};
use crate::knowledge::RebirthTrigger;
use crate::model::{
    ConversationId, ConversationKind, ConversationStatus, DomainEvent, Lifecycle, Mention, Message,
    MessageRole, ModelChoice, ModelFallback, Notice, Setup, streams,
};
use crate::routing::TokenMeter;
use crate::runtime::{is_delta, merge_delta};
use crate::sessions::{inline_image_tokens, push_block};
use crate::tools::{Role, RunTools};
use crate::work::{
    AttachmentRef, Compaction, CompactionState, ContextInjection, InjectionKind, OrchestratorEntry,
    OrchestratorStepKind, QuestionKind, QueuedMessage, QuotaWait, RequestState, RunState, Task,
    TaskId,
};
use crate::{Error, Result, now_ms};

/// Text deltas arriving within this window are stored as one event.
const DELTA_WINDOW: Duration = Duration::from_millis(30);
/// How long a blocking MCP call may take for the thread: its `run_check` (and a Codex
/// thread's `run`) waits for its command (THREAD-PLAN.md Q4, Q8 lever 3), so the longest
/// command timeout plus a minute, so a command that runs out its time is still reaped and its
/// output stored before the call itself expires (the call's clock starts first).
const RUNNER_TOOL_TIMEOUT_SECS: u64 = super::run::RUN_TIMEOUT_MAX.as_secs() + 60;
const _: () = assert!(RUNNER_TOOL_TIMEOUT_SECS > super::run::RUN_TIMEOUT_MAX.as_secs());
/// A Chat's Brigadier tools (saving a memory) answer within this.
const CHAT_TOOL_TIMEOUT_SECS: u64 = 60;
/// Messages carried verbatim when a conversation's CLI session is started over.
const RESEED_MESSAGES: usize = 40;
/// Bytes of transcript carried when a conversation's CLI session is started over.
const RESEED_BYTES: usize = 48_000;
/// Messages of an @-mentioned conversation that go along as context.
const MENTIONED_CHAT_MESSAGES: usize = 30;
/// Bytes of an @-mentioned conversation that go along as context.
const MENTIONED_CHAT_BYTES: usize = 24_000;
/// A Chat's text attachments, and text the user pasted anywhere, up to this size go into the
/// message itself (PASTE_INLINE_BYTES in the app).
const INLINE_TEXT_MAX_BYTES: usize = 200_000;
/// Of longer pasted text, the start and the end that go into the message.
const PASTE_HEAD_BYTES: usize = 150_000;
const PASTE_TAIL_BYTES: usize = 50_000;
const ENDED_UNEXPECTEDLY: &str = "The CLI session ended unexpectedly.";
/// Sent with every turn while the session is in plan mode.
const PLAN_MODE_NOTE: &str = "[plan mode] The user turned plan mode on: they want a plan to decide on before anything changes. Change nothing. Look around yourself (read, search, project_map) or with scouts, ask_user only what only they can decide, then write the plan and propose it with propose_plan, in its shape: a short title, one summary sentence, then Changes, Checks and Assumptions. Reply [quiet] after it: the plan card is your answer. Their yes turns plan mode off and comes back as a message; build only then. Merging, landing and finish_session are refused until then.";

/// Where a sent message went.
#[derive(Debug, Clone)]
pub enum SendOutcome {
    /// In the transcript: a new turn, or steered into the running one.
    Sent(Message),
    /// Waiting in the queue.
    Queued(QueuedMessage),
}

/// Something for the orchestrator's next turn.
#[derive(Debug, Clone)]
pub(crate) struct Envelope {
    pub kind: InjectionKind,
    /// Short label for the Inspector ("report task-3").
    pub label: String,
    pub task_id: Option<TaskId>,
    /// The text the orchestrator reads.
    pub text: String,
}

/// A live CLI session of a conversation or task.
pub(crate) struct Cli {
    pub provider: ProviderKind,
    /// The login it runs on.
    pub account: crate::accounts::AccountRef,
    pub model: ModelChoice,
    /// The conversation's own model choice it was started for (none for a task, or a Chat on
    /// the default model): a different one in the setup means the user changed it since.
    pub chosen: Option<ModelChoice>,
    pub session: Arc<dyn ProviderSession>,
    /// The cleanup-ledger owner (`orch:…`, `chat:…`, `task:…`).
    pub owner: String,
    /// Cancelled once the session's pump has stored its last event.
    pub ended: CancellationToken,
    /// What each of its turns used, from the CLI's running totals.
    pub meter: TokenMeter,
    /// A session thread's: the workspace, level and access it was started for.
    pub launch: Option<super::thread::ThreadLaunch>,
}

#[derive(Default)]
struct ConvState {
    cli: Option<Arc<Cli>>,
    /// What the thread read and searched in the running turn, recorded when it ends
    /// ([`super::reads`]).
    looked: Vec<ProviderEvent>,
    /// What the CLI session was last told of the parts of its instructions that can change
    /// (its role instructions and later notes); unknown until a turn reads it from the log.
    told: Option<crate::work::Told>,
    /// A turn is starting or running.
    busy: bool,
    /// The turn was sent and its CLI hasn't begun it yet: the run shows as starting until it
    /// has (a fresh CLI takes seconds to start; the live line says so rather than "Thinking").
    awaiting_start: bool,
    /// The CLI began the running turn: it holds the turn's messages in its session.
    landed: bool,
    /// The messages of a turn cut short by an account's limit, which the CLI had already begun:
    /// they are in the session the next turn resumes on another account, so that turn says to
    /// carry on instead of sending them again (any others still go).
    continuing: HashSet<String>,
    /// The thread's commands (Bash, `run`, a Codex shell item) running in this turn.
    commands: HashSet<String>,
    /// The user stopped the turn while a command ran: its CLI is closed when the turn ends,
    /// which ends that command's process tree (a CLI's interrupt can leave it running); the
    /// next turn resumes the session.
    end_commands: bool,
    /// The CLI is being closed on purpose (hibernate, archive, fallback).
    closing: bool,
    /// Envelopes for coming turns, each with the request it belongs to.
    inbox: Vec<(Envelope, Option<String>)>,
    /// Tasks already recorded as ended or sent back whose envelope saying so is still being
    /// put together (a landing's cleanup runs first), each with its request: until the
    /// envelope is in the inbox, the request works and a turn counts the task as running.
    announcing: HashMap<TaskId, Option<String>>,
    /// User messages already in the transcript that the next turn carries.
    pending: Vec<Message>,
    /// A turn cut short by a limit, from its end until its messages are back in `pending` (on
    /// another account, a stand-in, or waiting for quota) or it has failed. No turn starts
    /// meanwhile: one would run on the CLI at its limit, as it closes.
    hand_over: Option<HandOver>,
    /// The request the running turn serves.
    request: Option<String>,
    /// How the last turn for a request ended, when it was stopped or failed.
    outcomes: HashMap<String, RequestState>,
    /// The last error the running turn reported.
    turn_error: Option<String>,
    /// Brigadier's notes for the next turn that carries user messages (an edit, a redo).
    notes: Vec<String>,
    /// No new turn starts while the user's edit or redo takes the thread apart.
    held: bool,
    /// Tasks stopped by an edit or redo: what they still send is dropped.
    withdrawn: HashSet<TaskId>,
    /// The user messages the running turn carries (resent after a Chat fallback).
    in_turn: Vec<Message>,
    /// Queued follow-ups the orchestrator is asked about in an inbox envelope, each with the
    /// request that envelope belongs to.
    asking: Vec<(String, Option<String>)>,
    /// Queued follow-ups the running turn was asked about: the ones it leaves undecided wait
    /// in the queue when it ends.
    asked: Vec<String>,
    /// Replies that were streaming when a message was steered in, and the request they
    /// answer (the one before the steer).
    replying_for: HashMap<String, String>,
    /// Per request: the last reply the narration filter kept out of the thread, which the
    /// thread gets after all if the request ends with nothing else said (see
    /// [`SessionManager::release_narration`]).
    narration: HashMap<String, Narration>,
    /// Requests the user saw a reply for.
    spoke: HashSet<String>,
    /// Requests in which the orchestrator sent a worker back and the user has seen nothing
    /// since: such a request must not end with nothing said.
    sent_back: HashMap<String, Unanswered>,
    /// Reported write tasks the orchestrator was reminded to decide on (see
    /// [`SessionManager::remind_undecided`]): once each.
    reminded: HashSet<TaskId>,
    /// The running turn's last reply.
    last_reply: Option<String>,
    /// Requests whose last turn ended asking the user something in its reply: until the user
    /// writes again, no reminder pushes the orchestrator past its question.
    asked_user: HashSet<String>,
    /// Requests whose thread was told once to ask its text question on a card instead (see
    /// [`ASK_ON_A_CARD`]).
    told_to_ask_on_card: HashSet<String>,
    /// The next CLI session starts fresh and must be given the transcript so far.
    reseed: bool,
    /// The running turn failed on a usage limit (which window, and its reset, when the CLI
    /// said).
    limit_hit: Option<LimitHit>,
    /// The running turn is one of its own that compacts the context: messages sent meanwhile
    /// wait for the next turn.
    compacting: bool,
    /// The compaction running now (a Chat's).
    compaction: Option<Compaction>,
    last_activity_ms: i64,
    /// The context the CLI last reported: tokens used, and its model's window.
    context: Option<(i64, Option<i64>)>,
    /// An orchestrator's rebirth being prepared (its handoff note is being written).
    rebirth: Option<Arc<RebirthPrep>>,
    /// The next CLI session starts fresh, not resuming the last one.
    fresh: bool,
    /// The next orchestrator CLI session starts from this briefing.
    briefing: Option<BriefingPlan>,
    /// Its model is at a limit and no model it may use can stand in: `pending` waits for one
    /// (see [`SessionManager::wait_for_model`]).
    waiting: bool,
    /// Counts the quota-wait timers set; only the newest one retries.
    wait_timer: u64,
    /// The model the waiting messages were for (it hit its limit).
    waited_model: Option<ModelChoice>,
    /// The orchestrator's last model request, for knowing when its prompt cache expires
    /// (recovered from the orchestrator log when absent, as after a restart).
    cache: Option<CacheMark>,
    /// A handoff note written while the cache was warm, for a rebirth once it has expired.
    checkpoint: Option<Arc<RebirthPrep>>,
}

/// Where a request the orchestrator sent a worker back in stands, while the user has seen no
/// reply since.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unanswered {
    Armed,
    /// It ended with nothing said; checked again after [`UNANSWERED_GRACE`].
    Checking,
    /// The orchestrator was asked for its answer.
    Reminded,
}

/// How long a request may look over with nothing said before the orchestrator is asked for its
/// answer: a task's end and the envelope that tells of it are not one step (a landing removes
/// a worktree in between).
const UNANSWERED_GRACE: Duration = Duration::from_secs(10);

/// A conversation's live state.
pub(crate) struct ConvLive {
    pub id: ConversationId,
    pub kind: ConversationKind,
    state: tokio::sync::Mutex<ConvState>,
    /// Held while waiting messages are routed again, so a timer and a routing change never
    /// both start a model for them.
    retry: tokio::sync::Mutex<()>,
    /// How many times the user wrote: a message or a queue item stored (sent, edited, moved
    /// from the queue). Held while each is stored, and by a merge from its last look at the
    /// user's consent until it lands, so a "wait" sent meanwhile either stops it or comes after.
    pub(super) user_wrote: tokio::sync::Mutex<u64>,
    /// Tests: what a merge waits for once prepared, before its last look at consent.
    #[cfg(test)]
    pub(super) merge_pause:
        std::sync::Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
    /// Tests: what a limited turn's end waits for before its hand-over is chosen.
    #[cfg(test)]
    pub(super) hand_over_pause:
        std::sync::Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
}

impl ConvLive {
    /// The conversation's CLI session, while one runs.
    #[cfg(debug_assertions)]
    pub(crate) async fn cli(&self) -> Option<Arc<Cli>> {
        self.state.lock().await.cli.clone()
    }

    pub fn new(id: ConversationId, kind: ConversationKind) -> Self {
        Self {
            id,
            kind,
            state: tokio::sync::Mutex::new(ConvState {
                last_activity_ms: now_ms(),
                ..ConvState::default()
            }),
            retry: tokio::sync::Mutex::new(()),
            user_wrote: tokio::sync::Mutex::new(0),
            #[cfg(test)]
            merge_pause: std::sync::Mutex::new(None),
            #[cfg(test)]
            hand_over_pause: std::sync::Mutex::new(None),
        }
    }

    /// Stores what the user wrote (`write`: a message or a queue item) and counts it, as one
    /// step for a merge's look at their consent: a merge sees either the write or the count.
    /// Waits while a merge lands.
    pub(super) async fn user_write<T>(
        &self,
        write: impl std::future::Future<Output = Result<T>>,
    ) -> Result<T> {
        let mut wrote = self.user_wrote.lock().await;
        let stored = write.await;
        *wrote += 1;
        stored
    }

    /// Holds what a tool call of the thread read or searched for the turn; the batch to record
    /// now once [`reads::BATCH_MAX`] are held.
    pub(super) async fn hold_looked(&self, event: ProviderEvent) -> Option<Vec<ProviderEvent>> {
        let mut state = self.state.lock().await;
        state.looked.push(event);
        (state.looked.len() >= reads::BATCH_MAX).then(|| std::mem::take(&mut state.looked))
    }

    /// What the thread read and searched since it was last recorded.
    pub(super) async fn take_looked(&self) -> Vec<ProviderEvent> {
        std::mem::take(&mut self.state.lock().await.looked)
    }

    /// What the thread read and searched since it was last recorded, left held.
    pub(super) async fn peek_looked(&self) -> Vec<ProviderEvent> {
        self.state.lock().await.looked.clone()
    }

    /// The conversation's CLI session, while one runs.
    pub(crate) async fn live_cli(&self) -> Option<Arc<Cli>> {
        self.state.lock().await.cli.clone()
    }

    /// The CLI session while it runs no turn and isn't being closed.
    pub(crate) async fn idle_cli(&self) -> Option<Arc<Cli>> {
        let state = self.state.lock().await;
        if state.busy || state.closing {
            return None;
        }
        state.cli.clone()
    }

    /// Tracks the thread's command `item` by its `status` (see `ConvState::commands`).
    async fn running_command(&self, item: &str, status: ItemStatus) {
        let mut state = self.state.lock().await;
        if status == ItemStatus::InProgress {
            state.commands.insert(item.to_owned());
        } else {
            state.commands.remove(item);
        }
    }

    /// Closes `cli` between turns, if it is still the conversation's and nothing runs: the
    /// next turn starts it again (resuming it). Returns once it has ended.
    pub(crate) async fn retire_cli(&self, cli: &Arc<Cli>) {
        let taken = {
            let mut state = self.state.lock().await;
            let current = state.cli.as_ref().is_some_and(|now| Arc::ptr_eq(now, cli));
            if !current || state.busy || state.closing {
                return;
            }
            state.closing = true;
            state.cli.take()
        };
        if let Some(cli) = taken {
            cli.session.close().await;
            cli.ended.cancelled().await;
        }
        self.state.lock().await.closing = false;
    }

    /// Ends the CLI session, if any. Its files stay (it can be resumed).
    pub async fn close_cli(&self) {
        let cli = {
            let mut state = self.state.lock().await;
            state.closing = true;
            state.cli.take()
        };
        if let Some(cli) = cli {
            cli.session.close().await;
            cli.ended.cancelled().await;
        }
        let mut state = self.state.lock().await;
        state.closing = false;
        state.busy = false;
        state.compacting = false;
    }

    /// Whether a turn is starting or running, or work waits for one.
    pub async fn is_busy(&self) -> bool {
        let state = self.state.lock().await;
        state.busy
            || state.hand_over.is_some()
            || !state.inbox.is_empty()
            || !state.pending.is_empty()
    }

    pub async fn idle_since_ms(&self) -> Option<i64> {
        let state = self.state.lock().await;
        (!state.busy && state.cli.is_some()).then_some(state.last_activity_ms)
    }

    /// The orchestrator's last model request as last seen live.
    pub(super) async fn cache_mark(&self) -> Option<CacheMark> {
        self.state.lock().await.cache.clone()
    }

    pub(super) async fn set_cache_mark(&self, mark: CacheMark) {
        self.state.lock().await.cache = Some(mark);
    }

    /// The checkpoint written for a rebirth once the cache has expired, if any.
    pub(super) async fn checkpoint(&self) -> Option<Arc<RebirthPrep>> {
        self.state.lock().await.checkpoint.clone()
    }

    pub(super) async fn set_checkpoint(&self, checkpoint: Arc<RebirthPrep>) {
        self.state.lock().await.checkpoint = Some(checkpoint);
    }

    /// A rebirth is on its way already (a handoff for the size threshold, or a briefing).
    pub(super) async fn rebirth_pending(&self) -> bool {
        let state = self.state.lock().await;
        state.rebirth.is_some() || state.briefing.is_some() || state.fresh
    }

    /// The next CLI session must start from the transcript (its files are gone).
    pub async fn mark_reseed(&self) {
        self.state.lock().await.reseed = true;
    }

    /// Whether a turn is starting or running.
    pub(super) async fn turn_running(&self) -> bool {
        self.state.lock().await.busy
    }

    /// Stops the running turn, if any; what it carried stays in the transcript.
    pub(super) async fn interrupt_turn(&self) {
        let cli = {
            let state = self.state.lock().await;
            state.busy.then(|| state.cli.clone()).flatten()
        };
        if let Some(cli) = cli
            && let Err(err) = cli.session.interrupt().await
        {
            tracing::warn!(conversation = %self.id, error = %err, "could not stop the turn");
        }
    }

    /// Holds new turns until [`Self::carry`].
    pub(super) async fn hold(&self) {
        self.state.lock().await.held = true;
    }

    /// Holds new turns and drops what waits for `request`, and anything `tasks` still send:
    /// the user edits or redoes it.
    pub(super) async fn withdraw(&self, request: &str, tasks: impl IntoIterator<Item = TaskId>) {
        let mut state = self.state.lock().await;
        state.held = true;
        state.inbox.retain(|(_, of)| of.as_deref() != Some(request));
        state
            .announcing
            .retain(|_, of| of.as_deref() != Some(request));
        state
            .pending
            .retain(|message| message.request_id.as_deref() != Some(request));
        state.withdrawn.extend(tasks);
    }

    /// Waits until no turn runs (a stopped turn has stored what it said), for at most `limit`.
    pub(super) async fn wait_idle(&self, limit: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + limit;
        while self.turn_running().await {
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        true
    }

    /// Queues a user message (already in the transcript) for the next turn, with a note, and
    /// lets turns start again.
    pub(super) async fn carry(&self, message: Option<Message>, note: Option<String>) {
        let mut state = self.state.lock().await;
        state.pending.extend(message);
        state.notes.extend(note);
        state.held = false;
    }

    /// A note for the next turn that carries user messages, starting no turn of its own.
    pub(super) async fn note(&self, note: String) {
        self.state.lock().await.notes.push(note);
    }

    /// The provider of the running turn's CLI, while a turn runs (a Brain job yields to it).
    pub(super) async fn busy_provider(&self) -> Option<ProviderKind> {
        let state = self.state.lock().await;
        state
            .cli
            .as_ref()
            .filter(|_| state.busy)
            .map(|cli| cli.provider)
    }

    /// The context the CLI last said it holds.
    pub(super) async fn context_used(&self) -> Option<i64> {
        self.state.lock().await.context.map(|(used, _)| used)
    }

    /// The request the running turn serves.
    pub(super) async fn running_request(&self) -> Option<String> {
        let state = self.state.lock().await;
        state.busy.then(|| state.request.clone()).flatten()
    }

    /// A turn runs, or a message of `request` waits for one.
    pub(super) async fn works_for(&self, request: &str) -> bool {
        let state = self.state.lock().await;
        state.busy
            || state
                .inbox
                .iter()
                .any(|(_, of)| of.as_deref() == Some(request))
    }

    /// The orchestrator sent a worker of `request` back to work: what the user saw of the
    /// request so far is not its answer.
    pub async fn sent_back(&self, request: &str) {
        let mut state = self.state.lock().await;
        state.spoke.remove(request);
        state
            .sent_back
            .entry(request.to_owned())
            .or_insert(Unanswered::Armed);
    }

    /// An overnight run ended: what was owed to its requests (`run-<short id>-…`) is over.
    /// Nothing the run left in the inbox or armed for them wakes the orchestrator later.
    pub async fn forget_run_requests(&self, prefix: &str) {
        let mut state = self.state.lock().await;
        state
            .sent_back
            .retain(|request, _| !request.starts_with(prefix));
        state
            .inbox
            .retain(|(_, request)| !request.as_deref().is_some_and(|r| r.starts_with(prefix)));
    }

    /// What the driver holds for each request right now.
    pub(super) async fn request_activity(&self) -> RequestActivity {
        let state = self.state.lock().await;
        RequestActivity {
            running: state.busy.then(|| state.request.clone()).flatten(),
            carried: state
                .inbox
                .iter()
                .filter_map(|(_, request)| request.clone())
                .chain(
                    state
                        .pending
                        .iter()
                        .filter_map(|message| message.request_id.clone()),
                )
                .chain(
                    state
                        .hand_over
                        .as_ref()
                        .and_then(|over| over.request.clone()),
                )
                .chain(state.announcing.values().flatten().cloned())
                .collect(),
            outcomes: state.outcomes.clone(),
        }
    }
}

/// A limited turn's hand-over under way (see `ConvState::hand_over`).
struct HandOver {
    /// The turn's request: it still works (closing the CLI on the way settles the requests).
    request: Option<String>,
    /// The user stopped it: its request ended Stopped, and its messages don't go again.
    stopped: bool,
}

/// A conversation driver's part in its requests.
pub(super) struct RequestActivity {
    /// The request of the running turn.
    pub running: Option<String>,
    /// Requests with envelopes or user messages waiting for a turn.
    pub carried: HashSet<String>,
    /// How requests' last turns ended, when they were stopped or failed.
    pub outcomes: HashMap<String, RequestState>,
}

impl SessionManager {
    /// Sends a user message: starts a turn, or queues it, or steers it into the running turn.
    /// With `queue_index` (and no `steer`) it queues at that slot whenever it would wait: while
    /// a turn runs, whatever the queueing setting, or while the queue is paused.
    pub async fn send_message(
        &self,
        id: ConversationId,
        text: String,
        attachments: Vec<AttachmentRef>,
        mentions: Vec<Mention>,
        steer: bool,
        queue_index: Option<u32>,
    ) -> Result<SendOutcome> {
        self.admit()?;
        let conversation = self.core.conversation(&id)?;
        match conversation.lifecycle {
            Lifecycle::Archived => {
                return Err(Error::Invalid(
                    "this conversation is archived; restore it first".into(),
                ));
            }
            Lifecycle::Hibernated => {
                self.core
                    .set_lifecycle(id.clone(), Lifecycle::Active)
                    .await?;
            }
            Lifecycle::Active => {}
        }
        if conversation.kind == ConversationKind::Session && conversation.setup.is_none() {
            return Err(Error::Invalid(
                "choose the session's repository and environment first".into(),
            ));
        }
        // Attachments uploaded a while ago must survive blob collection from now on.
        for attachment in &attachments {
            let Ok(hash) = attachment.id.parse::<brigadier_store::BlobHash>() else {
                return Err(Error::Invalid(format!(
                    "attachment {} is unknown",
                    attachment.name
                )));
            };
            if !self.core.store().blobs().touch(hash).await? {
                return Err(Error::Invalid(format!(
                    "{} is no longer stored; attach it again",
                    attachment.name
                )));
            }
        }
        if conversation.kind == ConversationKind::Session
            && queue_index.is_none()
            && let Some(outcome) = self
                .overnight_message(&id, &text, &attachments, &mentions)
                .await?
        {
            return Ok(outcome);
        }
        let queue_index = queue_index.filter(|_| !steer);
        let paused = queue_index.is_some() && self.core.board(&id).await?.queue.paused;
        let conv = self.conv(&id)?;
        if conversation.kind == ConversationKind::Session {
            // The next worker's worktree is made while the thread reads the message.
            self.prewarm(&id);
            return self
                .send_to_session(
                    &conv,
                    text,
                    attachments,
                    mentions,
                    steer,
                    queue_index,
                    paused,
                )
                .await;
        }
        let mut state = conv.state.lock().await;
        state.last_activity_ms = now_ms();
        if state.busy && !state.compacting {
            if steer {
                let message = self
                    .core
                    .append_user_message(id.clone(), text, attachments, mentions)
                    .await?;
                let steered = match &state.cli {
                    Some(cli) => {
                        let input = self
                            .turn_input(&conv, std::slice::from_ref(&message), &[])
                            .await;
                        cli.session.steer(input).await.is_ok()
                    }
                    None => false,
                };
                let mut into = None;
                if steered {
                    self.log_user_injection(&conv, &message).await;
                    // The user wrote again: whatever was asked of them is answered or moot.
                    state.asked_user.clear();
                    // The rest of the turn answers the new message: its own request.
                    into = std::mem::replace(&mut state.request, message.request_id.clone());
                    state.in_turn.push(message.clone());
                } else {
                    // The turn is still starting (or just ended): the next turn carries it.
                    state.pending.push(message.clone());
                }
                drop(state);
                self.note_steered(&conv, &message, into).await;
                self.settle_requests(&id).await;
                return Ok(SendOutcome::Sent(message));
            }
            let item = self
                .core
                .enqueue(&id, text, attachments, mentions, queue_index, false)
                .await?;
            return Ok(SendOutcome::Queued(item));
        }
        if paused {
            // Back into the paused queue it came from; nothing sends until the user resumes.
            let item = self
                .core
                .enqueue(&id, text, attachments, mentions, queue_index, false)
                .await?;
            return Ok(SendOutcome::Queued(item));
        }
        let message = self
            .core
            .append_user_message(id.clone(), text, attachments, mentions)
            .await?;
        state.pending.push(message.clone());
        drop(state);
        self.kick(&conv);
        Ok(SendOutcome::Sent(message))
    }

    pub(super) async fn prepare_proposal_turn(&self, conv: &Arc<ConvLive>, message: Message) {
        conv.state.lock().await.pending.push(message);
        self.kick(conv);
    }

    /// Changes a queued message, as the user wrote it again: counted like a new message, so a
    /// merge being prepared sees it.
    pub async fn edit_queued(
        &self,
        id: &ConversationId,
        item_id: &str,
        text: String,
        attachments: Vec<AttachmentRef>,
        mentions: Vec<Mention>,
    ) -> Result<crate::work::MessageQueue> {
        self.conv(id)?
            .user_write(
                self.core
                    .edit_queued(id, item_id, text, attachments, mentions),
            )
            .await
    }

    /// Sends a queued message now, into the running turn or a session's working answer (or
    /// as a new turn when nothing works).
    pub async fn steer_queued(&self, id: ConversationId, item_id: String) -> Result<()> {
        self.admit()?;
        let conv = self.conv(&id)?;
        let working = self.working_request(&id).await;
        let message = conv
            .user_write(async {
                let item = self.core.take_queued(&id, &item_id).await?;
                self.core
                    .append_user_message(id.clone(), item.text, item.attachments, item.mentions)
                    .await
            })
            .await?;
        self.join_working(&conv, message, working).await;
        Ok(())
    }

    /// A session's message: a follow-up while the newest answer works (see the module docs),
    /// else a new turn, or a place in the queue while an older request's turn runs.
    #[allow(clippy::too_many_arguments)]
    async fn send_to_session(
        &self,
        conv: &Arc<ConvLive>,
        text: String,
        attachments: Vec<AttachmentRef>,
        mentions: Vec<Mention>,
        steer: bool,
        queue_index: Option<u32>,
        paused: bool,
    ) -> Result<SendOutcome> {
        let id = &conv.id;
        let working = self.working_request(id).await;
        let busy = {
            let mut state = conv.state.lock().await;
            state.last_activity_ms = now_ms();
            state.busy
        };
        if steer && (busy || working.is_some()) {
            let message = conv
                .user_write(
                    self.core
                        .append_user_message(id.clone(), text, attachments, mentions),
                )
                .await?;
            self.join_working(conv, message.clone(), working).await;
            return Ok(SendOutcome::Sent(message));
        }
        if let Some(working) = working.filter(|_| !paused) {
            let item = conv
                .user_write(
                    self.core
                        .enqueue(id, text, attachments, mentions, queue_index, true),
                )
                .await?;
            self.ask_follow_up(conv, &item, working).await;
            return Ok(SendOutcome::Queued(item));
        }
        if busy || paused {
            // An older request's turn runs (the newest answer is done), or the queue is paused:
            // it waits for its own turn.
            let item = conv
                .user_write(
                    self.core
                        .enqueue(id, text, attachments, mentions, queue_index, false),
                )
                .await?;
            return Ok(SendOutcome::Queued(item));
        }
        let message = conv
            .user_write(
                self.core
                    .append_user_message(id.clone(), text, attachments, mentions),
            )
            .await?;
        conv.state.lock().await.pending.push(message.clone());
        self.kick(conv);
        Ok(SendOutcome::Sent(message))
    }

    /// Sends `message` into the answer that works now: steered into the running turn, or (the
    /// orchestrator idle while workers run) carried by the next turn. Either way the thread
    /// shows it inside that answer's block (`working`, the newest request, when idle).
    pub(super) async fn join_working(
        &self,
        conv: &Arc<ConvLive>,
        message: Message,
        working: Option<String>,
    ) {
        let mut state = conv.state.lock().await;
        let steered = match (&state.cli, state.busy && !state.compacting) {
            (Some(cli), true) => {
                let input = self
                    .turn_input(conv, std::slice::from_ref(&message), &[])
                    .await;
                cli.session.steer(input).await.is_ok()
            }
            _ => false,
        };
        let into = if steered {
            self.log_user_injection(conv, &message).await;
            // The user wrote again: whatever was asked of them is answered or moot.
            state.asked_user.clear();
            state.in_turn.push(message.clone());
            // The rest of the turn answers the new message: its own request.
            std::mem::replace(&mut state.request, message.request_id.clone())
        } else {
            // The turn is still starting (or none runs): the next turn carries it.
            state.pending.push(message.clone());
            working
        };
        drop(state);
        self.note_steered(conv, &message, into).await;
        self.settle_requests(&conv.id).await;
        if !steered {
            self.kick(conv);
        }
    }

    /// Asks the orchestrator whether follow-up `item` belongs to the working answer of request
    /// `working`: in its running turn, or in a turn of that request's own.
    async fn ask_follow_up(&self, conv: &Arc<ConvLive>, item: &QueuedMessage, working: String) {
        let preview = match self.core.board(&conv.id).await {
            Ok(board) => {
                // The block's own question: the first request of its steer chain.
                let mut request = board.requests.get(&working);
                for _ in 0..board.requests.len() {
                    match request
                        .and_then(|of| of.steered_into.as_deref())
                        .and_then(|into| board.requests.get(into))
                    {
                        Some(into) => request = Some(into),
                        None => break,
                    }
                }
                request.map(|of| of.preview.clone()).unwrap_or_default()
            }
            Err(_) => String::new(),
        };
        let words = self.core.brief_words(&item.text, &item.attachments).await;
        let files: Vec<AttachmentRef> = item
            .attachments
            .iter()
            .filter(|attachment| !attachment.pasted)
            .cloned()
            .collect();
        let text = prompts::follow_up(&item.id, &preview, &words, &files);
        let mut state = conv.state.lock().await;
        let steered = match (&state.cli, state.busy) {
            (Some(cli), true) => cli
                .session
                .steer(TurnInput::text(text.clone()))
                .await
                .is_ok(),
            _ => false,
        };
        if steered {
            state.asked.push(item.id.clone());
            drop(state);
            self.log_injection(
                &conv.id,
                InjectionKind::FollowUp,
                "follow-up".into(),
                None,
                text.len(),
            )
            .await;
            return;
        }
        state.asking.push((item.id.clone(), Some(working.clone())));
        drop(state);
        let envelope = Envelope {
            kind: InjectionKind::FollowUp,
            label: "follow-up".into(),
            task_id: None,
            text,
        };
        self.deliver_for(&conv.id, envelope, Some(working)).await;
    }

    /// `route_follow_up`: the orchestrator's judgment of a queued follow-up. One that joins
    /// the working answer is sent into it now; one of its own waits in the queue.
    pub(crate) async fn route_follow_up(
        &self,
        id: &ConversationId,
        item_id: &str,
        joins: bool,
    ) -> Result<String> {
        let conv = self.conv(id)?;
        {
            let mut state = conv.state.lock().await;
            state.asked.retain(|asked| asked != item_id);
            state.asking.retain(|(asking, _)| asking != item_id);
        }
        let queued = self
            .core
            .board(id)
            .await?
            .queue
            .items
            .iter()
            .any(|item| item.id == item_id);
        if !queued {
            return Ok(
                "The user already sent, changed or removed that follow-up; nothing to do.".into(),
            );
        }
        if !joins {
            self.core.settle_queued(id, &[item_id.to_owned()]).await?;
            return Ok(
                "It waits in the queue and reaches you as its own request once this work is \
                 done. Don't act on it now."
                    .into(),
            );
        }
        let working = self.working_request(id).await;
        let message = conv
            .user_write(async {
                let item = self.core.take_queued(id, item_id).await?;
                self.core
                    .append_user_message(id.clone(), item.text, item.attachments, item.mentions)
                    .await
            })
            .await?;
        self.join_working(&conv, message, working).await;
        Ok(
            "It joins this work and reaches you now as the user's message. Act on it (a running \
             worker gets it with message_worker) and answer it in the final answer with the rest."
                .into(),
        )
    }

    /// Sends a session's next queued message once no answer works (one that only waits on the
    /// user doesn't hold it): returns whether one is now pending. Holds the driver's lock, so
    /// two callers never send two at once.
    async fn send_queued(&self, conv: &Arc<ConvLive>) -> bool {
        if self.answer_working(&conv.id).await {
            return false;
        }
        let mut state = conv.state.lock().await;
        if state.busy || state.held || !state.pending.is_empty() || !state.inbox.is_empty() {
            return false;
        }
        let ready = match self.core.board(&conv.id).await {
            Ok(board) => board.queue.items.first().is_some_and(|item| !item.deciding),
            Err(_) => false,
        };
        if !ready {
            return false;
        }
        // The queue's next item becomes the user's message in one step for a merge's look.
        let popped = conv
            .user_write(async {
                let Some(item) = self.core.pop_queued(&conv.id).await? else {
                    return Ok(None);
                };
                Ok(Some(
                    self.core
                        .append_user_message(
                            conv.id.clone(),
                            item.text,
                            item.attachments,
                            item.mentions,
                        )
                        .await,
                ))
            })
            .await;
        match popped {
            Ok(Some(Ok(message))) => {
                state.pending.push(message);
                true
            }
            Ok(Some(Err(err))) => {
                tracing::warn!(conversation = %conv.id, error = %err, "could not send a queued message");
                false
            }
            Ok(None) => false,
            Err(err) => {
                tracing::warn!(conversation = %conv.id, error = %err, "could not read the queue");
                false
            }
        }
    }

    /// A message steered into the turn of request `into`: its request says so, and after
    /// which reply, so the thread shows it inside that request's block where it was sent. The
    /// reply streaming right now still answers `into`.
    async fn note_steered(&self, conv: &ConvLive, message: &Message, into: Option<String>) {
        let (Some(request), Some(into)) = (&message.request_id, into) else {
            return;
        };
        let streaming = match self.core.board(&conv.id).await {
            Ok(board) => board.streaming.map(|streaming| streaming.message_id),
            Err(_) => None,
        };
        if let Some(reply) = &streaming {
            conv.state
                .lock()
                .await
                .replying_for
                .insert(reply.clone(), into.clone());
        }
        if let Err(err) = self
            .core
            .mark_steered(&conv.id, request, &into, streaming)
            .await
        {
            tracing::warn!(conversation = %conv.id, error = %err, "could not mark a steered request");
        }
    }

    /// Unpauses the queue and sends its next message if nothing runs.
    pub async fn resume_queue(&self, id: ConversationId) -> Result<crate::work::MessageQueue> {
        let queue = self.core.set_queue_paused(&id, false).await?;
        let conv = self.conv(&id)?;
        self.kick(&conv);
        Ok(queue)
    }

    /// Stops the running turn. The queue pauses so nothing is sent until the user resumes.
    pub async fn interrupt(&self, id: ConversationId) -> Result<()> {
        let conv = self.conv(&id)?;
        // Messages waiting for quota: the user no longer wants them sent. After a retry that
        // is looking now (it could record the wait again once they are gone).
        let retrying = conv.retry.lock().await;
        let (stopped, handed_over) = {
            let mut state = conv.state.lock().await;
            // A limited turn being handed over: it ends Stopped, and its messages don't go again.
            let mut handed_over = false;
            if let Some(over) = &mut state.hand_over
                && !over.stopped
            {
                over.stopped = true;
                handed_over = true;
                if let Some(request) = over.request.take() {
                    state.outcomes.insert(request, RequestState::Stopped);
                }
            }
            let stopped = if state.waiting {
                let requests: HashSet<String> = state
                    .pending
                    .drain(..)
                    .filter_map(|message| message.request_id)
                    .collect();
                for request in &requests {
                    state
                        .outcomes
                        .insert(request.clone(), RequestState::Stopped);
                }
                true
            } else {
                false
            };
            (stopped, handed_over)
        };
        if stopped {
            self.stop_waiting(&conv).await;
        }
        if stopped || handed_over {
            self.settle_requests(&id).await;
        }
        drop(retrying);
        // A turn handed over isn't running: its CLI, at its limit, has nothing to interrupt.
        let cli = if handed_over {
            None
        } else {
            conv.state.lock().await.cli.clone()
        };
        let waiting = !self
            .core
            .conversation_view(id.clone(), 1)
            .await?
            .queue
            .items
            .is_empty();
        if waiting {
            self.core.set_queue_paused(&id, true).await?;
        }
        // The thread first, so it starts nothing more; then what it runs: its commands (when
        // the turn ends, see `end_commands`), its previews, a start of one still under way
        // included, even if the CLI didn't take the interrupt.
        {
            let mut state = conv.state.lock().await;
            state.end_commands = !state.commands.is_empty();
        }
        let interrupted = match cli {
            Some(cli) => cli
                .session
                .interrupt()
                .await
                .map_err(|err| Error::Provider(err.to_string())),
            None => Ok(()),
        };
        self.stop_previews(&id, super::preview::USER_STOP).await;
        self.drop_prewarm(&id, "stopped by the user");
        interrupted
    }

    /// Continues the latest request after the user stopped it: a turn for that request, in its
    /// block, telling the model to carry on. Workers are untouched (a stop never ends them);
    /// the queue unpauses and runs after this turn.
    pub async fn resume(&self, id: ConversationId) -> Result<()> {
        let conv = self.conv(&id)?;
        {
            let state = conv.state.lock().await;
            if state.busy || !state.pending.is_empty() || !state.inbox.is_empty() {
                return Err(Error::Invalid(
                    "a turn is already running or about to start".into(),
                ));
            }
        }
        let board = self.core.board(&id).await?;
        let request = match board.latest_request() {
            Some(request) if request.state == RequestState::Stopped => request.id.clone(),
            _ => {
                return Err(Error::Invalid(
                    "only a stopped request can be resumed".into(),
                ));
            }
        };
        self.core.set_queue_paused(&id, false).await?;
        let envelope = Envelope {
            kind: InjectionKind::Resume,
            label: "resume".into(),
            task_id: None,
            text: "[The user stopped you, then asked you to resume. Continue their request from \
                   where you left off.]"
                .into(),
        };
        self.deliver_for(&id, envelope, Some(request)).await;
        Ok(())
    }

    /// Compacts a Chat's context now, the same as a `/compact` command: its CLI summarizes the
    /// conversation in a turn of its own that answers nothing, which the thread shows as one
    /// row. A session's orchestrator never compacts: Brigadier starts it afresh instead.
    pub async fn compact(&self, id: ConversationId) -> Result<()> {
        self.admit()?;
        let conversation = self.core.conversation(&id)?;
        if conversation.kind != ConversationKind::Chat {
            return Err(Error::Invalid("only a chat compacts its context".into()));
        }
        match conversation.lifecycle {
            Lifecycle::Archived => {
                return Err(Error::Invalid(
                    "this conversation is archived; restore it first".into(),
                ));
            }
            Lifecycle::Hibernated => {
                self.core
                    .set_lifecycle(id.clone(), Lifecycle::Active)
                    .await?;
            }
            Lifecycle::Active => {}
        }
        if !self.has_history(&id).await {
            return Err(Error::Invalid("there is nothing to compact yet".into()));
        }
        let conv = self.conv(&id)?;
        {
            let mut state = conv.state.lock().await;
            if state.busy
                || state.held
                || state.hand_over.is_some()
                || !state.pending.is_empty()
                || !state.inbox.is_empty()
            {
                return Err(Error::Invalid(
                    "wait until the reply is done, then compact".into(),
                ));
            }
            // A turn of its own, for no request.
            state.busy = true;
            state.compacting = true;
            state.request = None;
            state.turn_error = None;
            state.limit_hit = None;
            state.in_turn.clear();
        }
        self.set_run(&id, RunState::Starting, None).await;
        let started = async {
            let cli = self.ensure_cli(&conv).await?;
            if conv.state.lock().await.reseed {
                return Err(Error::Invalid(
                    "the model starts afresh with the next message, so there is nothing to \
                     compact"
                        .into(),
                ));
            }
            if !cli.session.can_compact() {
                return Err(Error::Invalid(format!(
                    "this version of {} cannot compact its context",
                    cli.provider
                )));
            }
            self.set_run(&id, RunState::Running, None).await;
            cli.session
                .compact()
                .await
                .map_err(|err| Error::Provider(err.to_string()))
        }
        .await;
        if let Err(err) = started {
            {
                let mut state = conv.state.lock().await;
                state.busy = false;
                state.compacting = false;
            }
            self.set_run(&id, RunState::Idle, None).await;
            self.kick(&conv);
            return Err(err);
        }
        Ok(())
    }

    /// What `/status` shows: the CLI session of the conversation's model (running, or the one
    /// its next turn resumes) and its provider's usage left.
    pub async fn conversation_status(&self, id: ConversationId) -> Result<ConversationStatus> {
        let conversation = self.core.conversation(&id)?;
        let conv = self.conv(&id)?;
        let cli = conv.state.lock().await.cli.clone();
        let fallback = conversation
            .fallback
            .clone()
            .map(|fallback| fallback.choice);
        let (provider, native_id) = match cli {
            Some(cli) => (cli.provider, Some(cli.session.native_id())),
            None => {
                let provider = match (&conversation.setup, fallback) {
                    (_, Some(fallback)) => fallback.provider,
                    (Some(Setup::Chat { model }), None) => model.provider,
                    (Some(Setup::Session { orchestrator, .. }), None) => orchestrator.provider,
                    (None, None) => {
                        return Err(Error::Invalid(
                            "choose the conversation's model first".into(),
                        ));
                    }
                };
                (provider, self.last_native_id(&id, provider).await)
            }
        };
        Ok(ConversationStatus {
            provider,
            native_id,
            quota: self
                .runtime
                .overview(provider)
                .and_then(|overview| overview.quota),
        })
    }

    /// Queues an envelope for the orchestrator, starting a turn if it is idle. It belongs to
    /// its task's request (or the running turn's, or the newest).
    pub(crate) async fn deliver(&self, id: &ConversationId, envelope: Envelope) {
        let request = self.request_for(id, envelope.task_id.as_ref()).await;
        self.deliver_for(id, envelope, request).await;
    }

    /// Queues an envelope that belongs to `request`.
    pub(crate) async fn deliver_for(
        &self,
        id: &ConversationId,
        envelope: Envelope,
        request: Option<String>,
    ) {
        let Some(conv) = self.queue_envelope(id, envelope, request).await else {
            return;
        };
        // The request works again until the orchestrator has read it.
        self.settle_requests(id).await;
        self.kick(&conv);
    }

    /// Puts an envelope in the inbox without starting a turn (see [`Self::deliver_for`]): the
    /// conversation, unless it is gone, closing, archived or no longer takes the envelope's
    /// task.
    pub(crate) async fn queue_envelope(
        &self,
        id: &ConversationId,
        envelope: Envelope,
        request: Option<String>,
    ) -> Option<Arc<ConvLive>> {
        // A closing conversation takes nothing more (and gets no live state back).
        if self.is_closing(id) {
            return None;
        }
        let conv = self.conv(id).ok()?;
        let archived = matches!(
            self.core.conversation(id).map(|c| c.lifecycle),
            Ok(Lifecycle::Archived)
        );
        // Nothing wakes the orchestrator for an overnight run that has ended: its report
        // was the last word, and Continue starts a new segment with requests of its own.
        if let Some(request) = &request
            && self
                .core
                .board(id)
                .await
                .is_ok_and(|board| super::requests::ended_run_request(&board, request))
        {
            tracing::info!(conversation = %id, request, label = %envelope.label, "dropped a message for an overnight run that has ended");
            return None;
        }
        let mut state = conv.state.lock().await;
        if let Some(task) = &envelope.task_id {
            state.announcing.remove(task);
        }
        if archived
            || envelope
                .task_id
                .as_ref()
                .is_some_and(|task| state.withdrawn.contains(task))
        {
            return None;
        }
        state.inbox.push((envelope, request));
        drop(state);
        Some(conv)
    }

    /// Marks `task` as having news for the orchestrator on its way: call it before the task's
    /// state changes, when an envelope about that change follows ([`Self::deliver`]). A turn
    /// that starts in between then does not tell the orchestrator that nothing else runs.
    pub(crate) async fn announcing(&self, task: &Task) {
        let Ok(conv) = self.conv(&task.conversation_id) else {
            return;
        };
        let request = self
            .request_for(&task.conversation_id, Some(&task.id))
            .await;
        conv.state
            .lock()
            .await
            .announcing
            .insert(task.id.clone(), request);
    }

    /// The tasks of `request` with an envelope still to come: queued for a later turn, or
    /// being put together.
    pub(super) async fn announced(&self, conv: &ConvLive, request: &str) -> HashSet<TaskId> {
        let state = conv.state.lock().await;
        state
            .inbox
            .iter()
            .filter(|(_, of)| of.as_deref() == Some(request))
            .filter_map(|(envelope, _)| envelope.task_id.clone())
            .chain(
                state
                    .announcing
                    .iter()
                    .filter(|(_, of)| of.as_deref() == Some(request))
                    .map(|(task, _)| task.clone()),
            )
            .collect()
    }

    /// Starts the next turn if none runs and there is something to say.
    pub(crate) fn kick(&self, conv: &Arc<ConvLive>) {
        let manager = self.arc();
        let conv = conv.clone();
        self.spawn(async move { manager.next_turn(conv).await });
    }

    /// [`Self::kick`], returning once the next turn has started or found it may not (tests).
    #[cfg(all(test, unix))]
    pub(super) async fn next_turn_now(&self, conv: &Arc<ConvLive>) {
        self.arc().next_turn(conv.clone()).await;
    }

    async fn next_turn(self: Arc<Self>, conv: Arc<ConvLive>) {
        let waiting = {
            let state = conv.state.lock().await;
            // A hand-over under way starts the next turn itself, once it has moved its turn.
            if state.hand_over.is_some() {
                return;
            }
            state.waiting
        };
        // Messages waiting for quota go once a model can take them (this looks again now: the
        // user may have picked another model).
        if waiting {
            self.retry_conversation(&conv).await;
            return;
        }
        self.retire_changed_cli(&conv).await;
        self.retire_moved_thread(&conv).await;
        self.stop_moved_previews(&conv.id).await;
        self.end_stand_in(&conv).await;
        if conv.kind == ConversationKind::Session {
            self.rebirth_if_ready(&conv).await;
            self.rebirth_if_cache_expired(&conv).await;
        }
        let session = conv.kind == ConversationKind::Session;
        // A session's queued message goes once the requests have settled (no answer works).
        let mut sent_queued = false;
        let (users, envelopes, request, user_notes) = loop {
            let mut state = conv.state.lock().await;
            if state.busy
                || state.closing
                || state.held
                || state.hand_over.is_some()
                || self.admit().is_err()
            {
                return;
            }
            let mut users = std::mem::take(&mut state.pending);
            let notes = if users.is_empty() {
                Vec::new()
            } else {
                std::mem::take(&mut state.notes)
            };
            let envelopes = if users.is_empty() {
                take_one_request(&mut state.inbox)
            } else {
                Vec::new()
            };
            if let Some((_, request)) = envelopes
                .iter()
                .find(|(envelope, _)| envelope.kind == InjectionKind::FollowUp)
            {
                // The follow-ups asked about in this request's envelopes: this turn judges them.
                let request = request.clone();
                let (asked, asking) = std::mem::take(&mut state.asking)
                    .into_iter()
                    .partition::<Vec<_>, _>(|(_, of)| *of == request);
                state.asking = asking;
                state.asked.extend(asked.into_iter().map(|(item, _)| item));
            }
            if users.is_empty() && envelopes.is_empty() && !session {
                match self.core.pop_queued(&conv.id).await {
                    Ok(Some(item)) => match self
                        .core
                        .append_user_message(
                            conv.id.clone(),
                            item.text,
                            item.attachments,
                            item.mentions,
                        )
                        .await
                    {
                        Ok(message) => users.push(message),
                        Err(err) => {
                            tracing::warn!(conversation = %conv.id, error = %err, "could not send a queued message");
                        }
                    },
                    Ok(None) => {}
                    Err(err) => {
                        tracing::warn!(conversation = %conv.id, error = %err, "could not read the queue");
                    }
                }
            }
            if users.is_empty() && envelopes.is_empty() {
                drop(state);
                // Nothing left to say: the requests settle.
                self.settle_requests(&conv.id).await;
                if session && !sent_queued && self.send_queued(&conv).await {
                    sent_queued = true;
                    continue;
                }
                return;
            }
            let request = match users.last() {
                Some(message) => message.request_id.clone(),
                None => envelopes[0].1.clone(),
            };
            state.busy = true;
            state.landed = false;
            state.limit_hit = None;
            state.turn_error = None;
            state.last_reply = None;
            state.request.clone_from(&request);
            if let Some(request) = &request {
                state.outcomes.remove(request);
                // This turn says anew whether the request waits on the user.
                state.asked_user.remove(request);
            }
            if !users.is_empty() {
                // The user wrote again: whatever was asked of them is answered or moot.
                state.asked_user.clear();
            }
            state.in_turn = users.clone();
            break (users, envelopes, request, notes);
        };
        self.settle_requests(&conv.id).await;
        self.set_run_for(&conv.id, RunState::Starting, None, request.clone())
            .await;
        if session && !users.is_empty() {
            // The user's message brings back the session's worktree a merge removed
            // (THREAD-PLAN.md Q9), from the base's tip now.
            if let Err(err) = self.effective_workspace(&conv.id).await {
                tracing::warn!(conversation = %conv.id, error = %err, "could not make the session's worktree");
            }
        }
        let cli = match self.ensure_cli(&conv).await {
            Ok(cli) => cli,
            // Archived or deleted meanwhile: its cleanup took this turn's work along (a
            // restored one starts over from what is in its log).
            Err(_) if !self.is_attached(&conv) => return,
            Err(err) => {
                let message = err.to_string();
                self.fail_turn(&conv, users, envelopes, &message).await;
                return;
            }
        };
        self.brains.jobs.user_work(cli.provider);
        if session {
            // Where the branch stands before the turn: what came since the last turn is the
            // thread's only if marked so.
            self.scan_thread_commits(&conv.id, false).await;
        }
        let (reseed, briefing) = {
            let mut state = conv.state.lock().await;
            let reseed = std::mem::take(&mut state.reseed);
            let briefing = state.briefing.take();
            // A session that cannot resume its CLI starts from a Recovery briefing.
            let briefing = briefing.or_else(|| {
                (reseed && session).then(|| BriefingPlan {
                    swap_started_at_ms: now_ms(),
                    trigger: RebirthTrigger::Recovery,
                    prep: None,
                    at_tokens: state.context.map_or(0, |(used, _)| used),
                    window: state.context.and_then(|(_, window)| window),
                })
            });
            (reseed, briefing)
        };
        let notes = self
            .request_notes(&conv.id, &envelopes, request.as_deref())
            .await;
        let landed = std::mem::take(&mut conv.state.lock().await.continuing);
        let unsent: Vec<Message> = users
            .iter()
            .filter(|message| !landed.contains(&message.id))
            .cloned()
            .collect();
        let mut input = self
            .turn_input(&conv, &unsent, &[user_notes, notes.clone()].concat())
            .await;
        if unsent.len() < users.len() {
            input.prepend_text(prompts::CONTINUE_ON_ACCOUNT);
        }
        let mut reborn = None;
        if let Some(plan) = briefing {
            let (text, mut record) = self
                .briefing(
                    &conv.id,
                    cli.provider,
                    cli.model.model.clone(),
                    &plan,
                    &users,
                )
                .await;
            record.new_native_id = Some(cli.session.native_id());
            self.log_injection(
                &conv.id,
                InjectionKind::Briefing,
                format!("briefing (rebirth {})", record.generation),
                None,
                text.len(),
            )
            .await;
            input.prepend_text(text.as_ref());
            reborn = Some((plan, record));
        } else if reseed {
            let transcript = self.reseed_text(&conv.id, &users).await;
            if !transcript.is_empty() {
                self.log_injection(
                    &conv.id,
                    InjectionKind::Reseed,
                    "transcript so far".into(),
                    None,
                    transcript.len(),
                )
                .await;
                input.prepend_text(&transcript);
            }
        }
        // What changed in its instructions since the session last heard (the date, a setting,
        // the overnight run): a resumed CLI keeps its first instructions, so the change goes
        // with this turn, logged once the CLI took it.
        let instruction_notes = self.instruction_notes(&conv).await;
        if !instruction_notes.is_empty() {
            let text = instruction_notes
                .iter()
                .map(|note| note.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            input.prepend_text(&text);
        }
        for message in &unsent {
            self.log_user_injection(&conv, message).await;
        }
        for ((envelope, _), note) in envelopes.iter().zip(&notes) {
            self.log_injection(
                &conv.id,
                envelope.kind,
                envelope.label.clone(),
                envelope.task_id.clone(),
                note.len(),
            )
            .await;
        }
        conv.state.lock().await.awaiting_start = true;
        if let Err(err) = cli.session.send(input).await {
            conv.state.lock().await.awaiting_start = false;
            // The briefing goes with the next try.
            if let Some((plan, _)) = reborn {
                conv.state.lock().await.briefing = Some(plan);
            }
            let message = format!("The CLI did not take the turn: {err}");
            self.fail_turn(&conv, users, envelopes, &message).await;
            return;
        }
        self.told_notes(&conv, instruction_notes).await;
        if let Some((_, record)) = reborn {
            tracing::info!(conversation = %conv.id, generation = record.generation, tokens = record.briefing_tokens, trigger = ?record.trigger, "orchestrator reborn");
            self.log_orchestrator(
                &conv.id,
                OrchestratorEntry::Rebirth {
                    record: Box::new(record),
                },
            )
            .await;
        }
    }

    /// A turn could not start: keep what it carried for the next try and say why.
    async fn fail_turn(
        &self,
        conv: &Arc<ConvLive>,
        users: Vec<Message>,
        envelopes: Vec<(Envelope, Option<String>)>,
        message: &str,
    ) {
        let asked = {
            let mut state = conv.state.lock().await;
            state.busy = false;
            state.compacting = false;
            if let Some(request) = state.request.take() {
                state.outcomes.insert(
                    request,
                    RequestState::Failed {
                        error: message.to_owned(),
                    },
                );
            }
            let mut pending = users;
            pending.append(&mut state.pending);
            state.pending = pending;
            let mut inbox = envelopes;
            inbox.append(&mut state.inbox);
            state.inbox = inbox;
            std::mem::take(&mut state.asked)
        };
        // Its follow-ups wait in the queue; the carried envelopes ask again.
        if let Err(err) = self.core.settle_queued(&conv.id, &asked).await {
            tracing::warn!(conversation = %conv.id, error = %err, "could not settle follow-ups");
        }
        self.notice(&conv.id, brigadier_providers::NoticeLevel::Warning, message)
            .await;
        self.set_run(&conv.id, RunState::Failed, Some(message.to_owned()))
            .await;
        self.settle_requests(&conv.id).await;
    }

    /// The user changed the model, effort, Fast or account since the CLI started: close it
    /// while nothing runs, so the next turn resumes the conversation on the new choice, taking
    /// effect on the next message.
    async fn retire_changed_cli(&self, conv: &Arc<ConvLive>) {
        let Ok(conversation) = self.core.conversation(&conv.id) else {
            return;
        };
        let wanted = setup_choice(&conversation);
        if let Some(cli) = conv.idle_cli().await
            && cli.chosen != wanted
        {
            conv.retire_cli(&cli).await;
        }
    }

    /// Whether `conv` is still the conversation's live state: an archive or delete lets go of
    /// it.
    fn is_attached(&self, conv: &Arc<ConvLive>) -> bool {
        self.convs_lock()
            .get(&conv.id)
            .is_some_and(|live| Arc::ptr_eq(live, conv))
    }

    /// The conversation's live CLI session, started (or resumed) when there is none.
    async fn ensure_cli(&self, conv: &Arc<ConvLive>) -> Result<Arc<Cli>> {
        if let Some(cli) = conv.state.lock().await.cli.clone() {
            return Ok(cli);
        }
        let fence = self.enter(&conv.id)?;
        let conversation = self.core.conversation(&conv.id)?;
        let (owner, area) = match conv.kind {
            ConversationKind::Session => (format!("orch:{}", conv.id), "orch"),
            ConversationKind::Chat => (format!("chat:{}", conv.id), "chat"),
        };
        let dir = self.owned_dir(area, &conv.id.0);
        self.prepare_owned_dir(&owner, &dir).await?;

        // A stand-in while the chosen model is at its limit (the saved choice is untouched).
        let fallback = conversation
            .fallback
            .clone()
            .map(|fallback| fallback.choice);
        let mut grant_values = Vec::new();
        let short = self.core.settings().short_replies;
        let mut launch = None;
        let mut auto_review = false;
        let mut output_hook = None;
        let (choice, prompt, mcp, current) = match (&conversation.setup, conv.kind) {
            (Some(Setup::Session { orchestrator, .. }), _) => {
                let choice = fallback.unwrap_or_else(|| orchestrator.clone());
                // Its workspace exists before its CLI starts.
                let (started_for, reviews) = self
                    .thread_launch(&conv.id, &dir, choice.provider, true)
                    .await?;
                auto_review = reviews;
                let workspace = started_for.workspace.clone();
                let permission = started_for.permission;
                launch = Some(started_for);
                let preferences = self.memory_lines(super::brain_jobs::MEMORY_BYTES).await;
                let run = self.run_setting(&conv.id).await;
                let prompt = self
                    .thread_prompt(
                        &conversation,
                        &preferences,
                        workspace.as_ref(),
                        choice.provider,
                    )
                    .await;
                let current = prompts::Current::session(
                    &conversation,
                    run.as_ref()
                        .map(|(workspace, restrictions)| (workspace, restrictions.clone())),
                    short,
                    preferences,
                    workspace.as_ref().map(super::thread::ThreadWorkspace::told),
                );
                // A Codex thread runs long commands through `run`; a Claude thread's own Bash
                // output is trimmed by its hook (THREAD-PLAN.md Q4).
                let commands = super::run::run_tools(choice.provider, permission);
                let grant = self.grants.issue(
                    &owner,
                    Role::Orchestrator {
                        conversation_id: conv.id.clone(),
                        run: commands,
                    },
                );
                grant_values.push(grant.clone());
                let mut server = self.brigadier_server(grant, RUNNER_TOOL_TIMEOUT_SECS, false);
                // Under Ask for approval the user approves each command that leaves the sandbox;
                // a Claude thread's at Approve for me goes to Brigadier's reviewer instead.
                if commands == RunTools::WithEscalation
                    && permission == crate::model::PermissionLevel::AskForApproval
                {
                    server.prompt_tools = vec![super::run::RUN_UNSANDBOXED.into()];
                }
                if choice.provider == ProviderKind::Claude {
                    let hook_grant = self.grants.issue(
                        &owner,
                        Role::OutputHook {
                            conversation_id: conv.id.clone(),
                        },
                    );
                    grant_values.push(hook_grant.clone());
                    output_hook = Some(self.output_hook(hook_grant));
                }
                (choice, prompt, vec![server], current)
            }
            (Some(Setup::Chat { .. }) | None, ConversationKind::Chat) => {
                let model = match &conversation.setup {
                    Some(Setup::Chat { model }) => model.clone(),
                    _ => self.default_chat_choice(),
                };
                // A Chat may save what it learns about the user to the Personal Brain.
                let memories = self.memory_lines(super::brain_jobs::MEMORY_BYTES).await;
                let grant = self.grants.issue(
                    &owner,
                    Role::Chat {
                        conversation_id: conv.id.clone(),
                    },
                );
                grant_values.push(grant.clone());
                (
                    fallback.unwrap_or(model),
                    prompts::chat(&memories),
                    vec![self.brigadier_server(grant, CHAT_TOOL_TIMEOUT_SECS, false)],
                    prompts::Current::chat(memories),
                )
            }
            (Some(Setup::Chat { .. }), ConversationKind::Session) => {
                return Err(Error::Invalid(
                    "a session cannot have a Chat's setup".into(),
                ));
            }
            (None, ConversationKind::Session) => {
                return Err(Error::Invalid("the session has no setup".into()));
            }
        };

        let grant_redactor = super::secrets::redactor(grant_values.clone());
        let fresh = std::mem::take(&mut conv.state.lock().await.fresh);
        let mut resume = if fresh {
            None
        } else {
            self.last_native_id(&conv.id, choice.provider).await
        };
        // A thread whose CLI started on the instructions from before the thread's keeps them
        // when resumed, and no note can replace a whole role: it starts over from the
        // transcript with the thread's.
        if resume.is_some()
            && conv.kind == ConversationKind::Session
            && prompts::role_outdated(&self.told_from_log(&conv.id).await)
        {
            tracing::info!(conversation = %conv.id, "the thread's CLI started on older instructions; starting over from the transcript");
            resume = None;
        }
        let reseed_needed = !fresh && resume.is_none() && self.has_history(&conv.id).await;
        let mut spec = SessionSpec {
            cwd: dir.clone(),
            model: choice.model.clone(),
            effort: choice.effort.clone(),
            fast: choice.fast == Some(true),
            origin: match &resume {
                Some(native_id) => Origin::Resume {
                    native_id: native_id.clone(),
                },
                None => Origin::New,
            },
            // A Chat only searches the web.
            access: launch
                .as_ref()
                .map_or(Access::ReadOnly, |launch| launch.access.clone()),
            append_system_prompt: Some(prompt.clone()),
            mcp_servers: mcp,
            tools: match conv.kind {
                ConversationKind::Session => ToolSet::Thread,
                ConversationKind::Chat => ToolSet::Web,
            },
            // The workspace is the thread's to work in, never its own to clean up: it is not
            // recorded under the thread's owner.
            add_dirs: launch
                .as_ref()
                .and_then(|launch| launch.workspace.as_ref())
                .map(|workspace| vec![workspace.path.clone()])
                .unwrap_or_default(),
            env: match conv.kind {
                ConversationKind::Session => SessionManager::thread_env(&dir, choice.provider),
                ConversationKind::Chat => Vec::new(),
            },
            unset_env: Vec::new(),
            low_priority: false,
            record_to: None,
            redactor: grant_redactor.clone(),
            owned_cwd: true,
            // An orchestrator is reborn, never compacted.
            auto_compact: conv.kind == ConversationKind::Chat,
            allowed_models: None,
            auto_review,
            omit_ai_coauthors: self.core.settings().omit_ai_coauthors,
            output_hook,
        };
        let mut resumed = resume.is_some();
        // Every account shares the CLI's session history: a conversation resumes on whichever
        // account it runs on now.
        let account = self.runtime.account_for(&choice);
        let started = match self
            .runtime
            .start_hosted(&owner, &account, spec.clone())
            .await
        {
            Ok(started) => started,
            Err(err) if resume.is_some() => {
                tracing::info!(conversation = %conv.id, error = %err, "resume failed; starting over from the transcript");
                spec.origin = Origin::New;
                resumed = false;
                conv.state.lock().await.reseed = true;
                match self.runtime.start_hosted(&owner, &account, spec).await {
                    Ok(started) => started,
                    Err(err) => {
                        // Its own grants only: a restored conversation's live CLI keeps its.
                        self.grants.revoke(&grant_values);
                        return Err(err);
                    }
                }
            }
            Err(err) => {
                self.grants.revoke(&grant_values);
                // A reborn CLI that could not start is still owed its fresh start: the next
                // try must not resume the old, nearly full session.
                if fresh {
                    conv.state.lock().await.fresh = true;
                }
                return Err(err);
            }
        };
        // A cleanup that stopped waiting for this start has already passed this conversation,
        // and it may have been restored since, with a CLI of its own: this one ends unused,
        // before anything of it is logged, and only what it made goes.
        if fence.cut_off() {
            let native_id = started.session.native_id();
            started.session.close().await;
            self.grants.revoke(&grant_values);
            self.release_session_files(&owner, &native_id).await;
            return Err(super::closing::closing_error());
        }
        if reseed_needed {
            conv.state.lock().await.reseed = true;
        }
        if resumed {
            // A resumed CLI keeps the instructions it started with: the next turn reads from
            // the log what they and later notes said.
            conv.state.lock().await.told = None;
        } else {
            // A new one has them as they are now.
            let label = if conv.kind == ConversationKind::Session {
                prompts::instructions_label(short)
            } else {
                prompts::ROLE_INSTRUCTIONS
            };
            let told = current.told();
            conv.state.lock().await.told = Some(told.clone());
            self.log_told(&conv.id, label, prompt.len(), told).await;
        }
        let Started { session, events } = started;
        let cli = Arc::new(Cli {
            provider: choice.provider,
            meter: TokenMeter::new(resumed && choice.provider == ProviderKind::Codex)
                .on_account(account.account.clone()),
            account,
            model: choice,
            chosen: setup_choice(&conversation),
            session,
            owner,
            ended: CancellationToken::new(),
            launch,
        });
        conv.state.lock().await.cli = Some(cli.clone());
        let manager = self.arc();
        let pumped = cli.clone();
        let pumping = conv.clone();
        self.spawn(async move { manager.pump_conversation(pumping, pumped, events).await });
        Ok(cli)
    }

    /// Removes the files of `owner`'s CLI session `native_id` (one that started after its
    /// cleanup passed), leaving the rest of what it has.
    pub(super) async fn release_session_files(&self, owner: &str, native_id: &str) {
        let made: Vec<Artifact> = self
            .runtime
            .ledger()
            .artifacts(owner)
            .into_iter()
            .filter(|artifact| match artifact {
                Artifact::ClaudeSession { session_id, .. } => session_id == native_id,
                Artifact::CodexThread { thread_id, .. } => thread_id == native_id,
                _ => false,
            })
            .collect();
        let leftovers = self.runtime.ledger().release(owner, made).await;
        if !leftovers.is_clean() {
            tracing::warn!(
                owner,
                ?leftovers,
                "some leftovers of a cut-off start will be retried at the next launch"
            );
        }
    }

    /// The Brigadier MCP server entry a CLI session gets; `always_load` for a session that
    /// should use its tools without looking them up first (see [`McpServer::always_load`]).
    pub(crate) fn brigadier_server(
        &self,
        grant: String,
        timeout_secs: u64,
        always_load: bool,
    ) -> McpServer {
        McpServer {
            name: "brigadier".into(),
            command: self.config.daemon_exe.clone(),
            args: vec![
                "mcp".into(),
                "--data-dir".into(),
                self.data_dir.to_string_lossy().into_owned(),
            ],
            env: vec![("BRIGADIER_MCP_GRANT".into(), grant)],
            tool_timeout_secs: Some(timeout_secs),
            trusted: true,
            always_load,
            prompt_tools: Vec::new(),
        }
    }

    /// The `computer` MCP server a worker gets (COMPUTER-USE-PLAN.md §4.6): behind the CLI's
    /// tool search until a worker needs it, so one that never touches the desktop pays
    /// almost nothing; `always_load` for an operate worker, which needs it from the start.
    pub(crate) fn computer_server(
        &self,
        grant: String,
        timeout_secs: u64,
        always_load: bool,
    ) -> McpServer {
        let mut server = self.brigadier_server(grant.clone(), timeout_secs, always_load);
        server.name = "computer".into();
        // Claude puts every server's environment into its own, so each grant needs its own
        // variable; the bridge is told which one to read.
        server
            .args
            .extend(["--grant-env".into(), super::computer::GRANT_ENV.into()]);
        server.env = vec![(super::computer::GRANT_ENV.into(), grant)];
        server
    }

    /// A Claude thread's output hook (`brigadierd hook post-tool-use`), with its grant.
    fn output_hook(&self, grant: String) -> OutputHook {
        OutputHook {
            command: self.config.daemon_exe.clone(),
            args: vec![
                "hook".into(),
                "post-tool-use".into(),
                "--data-dir".into(),
                self.data_dir.to_string_lossy().into_owned(),
            ],
            env: vec![(super::tool_output::HOOK_GRANT_ENV.into(), grant)],
            timeout_secs: super::tool_output::HOOK_TIMEOUT_SECS,
        }
    }

    /// Creates a Brigadier-owned folder and records it (and anything running inside it) in
    /// the cleanup ledger first.
    pub(crate) async fn prepare_owned_dir(&self, owner: &str, dir: &std::path::Path) -> Result<()> {
        let ledger = self.runtime.ledger();
        let path = dir.to_string_lossy().into_owned();
        ledger
            .record(owner, Artifact::ScratchDir { path: path.clone() })
            .await?;
        ledger
            .record(owner, Artifact::ProcessesIn { dir: path })
            .await?;
        let dir = dir.to_owned();
        super::blocking(move || {
            std::fs::create_dir_all(&dir).map_err(|err| Error::Invalid(err.to_string()))
        })
        .await
    }

    /// The turn's input: user messages verbatim, then Brigadier's notes (the envelopes)
    /// and, in plan mode, a reminder of it.
    /// Text the user pasted is part of their message. Images go along as images. Other
    /// attachments are named so the orchestrator can hand them to workers; a Chat cannot open
    /// files, so it gets text files inline.
    async fn turn_input(
        &self,
        conv: &Arc<ConvLive>,
        users: &[Message],
        notes: &[String],
    ) -> TurnInput {
        let mut parts = Vec::new();
        let mut files = Vec::new();
        let mut copied = HashMap::new();
        let mut ordered = Vec::new();
        for message in users {
            let user_text = self.full_text(message).await;
            let plan = image_plan(&user_text, &message.attachments, &copied);
            for attachment in &plan.copies {
                copied.insert(
                    attachment.id.clone(),
                    self.attachment_file(conv, attachment).await,
                );
            }
            let text = name_inline_images(
                &user_text,
                &message.attachments,
                &copied,
                conv.kind,
                &mut files,
            );
            let mut inline = vec![InputPart::Text(text)];
            for attachment in plan.rows.iter().filter(|a| a.inline.is_none()) {
                if attachment.pasted
                    && let Ok(pasted) = self.core.read_blob_text(attachment.id.clone()).await
                {
                    push_input_block(&mut inline, &pasted_inline(&pasted, attachment, conv.kind));
                    continue;
                }
                if conv.kind == ConversationKind::Chat && !is_image(&attachment.mime) {
                    append_input_text(&mut inline, &self.inline_attachment(attachment).await);
                    continue;
                }
                if conv.kind == ConversationKind::Session {
                    append_input_text(
                        &mut inline,
                        &format!(
                            "\n[attachment {} \"{}\" ({}, {} bytes){}]",
                            attachment.id,
                            attachment.name,
                            attachment.mime,
                            attachment.bytes,
                            if is_image(&attachment.mime) {
                                ""
                            } else {
                                "; pass its id to delegate_task so a worker can read it"
                            }
                        ),
                    );
                }
            }
            for mention in &message.mentions {
                match mention {
                    Mention::Task { id } => {
                        if let Ok(tasks) = self.core.tasks(&conv.id).await
                            && let Some(task) = tasks.iter().find(|t| &t.id == id)
                        {
                            append_input_text(
                                &mut inline,
                                &format!("\n[mentions task-{}: {}]", task.number, task.title),
                            );
                        }
                    }
                    Mention::File { path } => {
                        append_input_text(&mut inline, &format!("\n[mentions the file {path}]"));
                    }
                    Mention::Chat { id, title } => {
                        append_input_text(&mut inline, &self.mentioned_chat(id, title).await);
                    }
                }
            }
            if !ordered.is_empty() {
                ordered.push(InputPart::Text("\n\n".into()));
            }
            ordered.append(&mut inline);
        }
        if let Ok(board) = self.core.board(&conv.id).await
            && let Some(run) = board
                .runs
                .values()
                .find(|run| run.state == crate::overnight::OvernightState::Proposed)
        {
            if run.predecessor.is_some() {
                parts.push(format!("[overnight proposal {} revision {}] This continues an earlier run on its branch: its phases, and those already verified, come from that run, so don't call propose_overnight. Only the user's Start begins it; until then you may only read.", run.id, run.revision));
            } else {
                parts.push(format!("[overnight proposal {} revision {}] This is preparation only. Read the user's brief and referenced plan with read-only scouts if necessary. Use propose_overnight to put its actual phases/criteria/Rules on this same proposal, keeping original phase numbers. Include every phase the user's words select (\"phases 1-3\"), also those after a \"stop after\" or a skip: Brigadier enforces those itself and keeps the rest for Continue. Do not invent scope or start implementation. For a bare goal keep phases empty (Phase 0 plans after Start). Only the user's Start begins the run. Reply with exactly [quiet] once the proposal is ready. The user's full agreement and latest corrections, verbatim:\n{}", run.id, run.revision, run.words));
            }
        }
        // A side chat answers about the conversation beside it, as it stands now.
        if let Ok(record) = self.core.conversation(&conv.id)
            && let Some(parent) = &record.side_of
            && let Ok(beside) = self.core.conversation(parent)
        {
            parts.push(self.side_chat_context(parent, &beside.title).await);
        }
        if conv.kind == ConversationKind::Session && self.plan_mode(&conv.id) {
            parts.push(PLAN_MODE_NOTE.into());
        }
        parts.extend(notes.iter().cloned());
        if !parts.is_empty() {
            if !ordered.is_empty() {
                ordered.push(InputPart::Text("\n\n".into()));
            }
            ordered.push(InputPart::Text(parts.join("\n\n")));
        }
        // Row images keep today's image-first order. Inline images stay in the user text.
        let mut merged: Vec<_> = files.into_iter().map(InputPart::Image).collect();
        // Joining adjacent text preserves today's single text block for ordinary turns.
        for part in ordered {
            match (merged.last_mut(), part) {
                (Some(InputPart::Text(previous)), InputPart::Text(text)) => {
                    previous.push_str(&text)
                }
                (_, part) => merged.push(part),
            }
        }
        TurnInput {
            parts: merged,
            files: Vec::new(),
        }
    }

    /// Another conversation the user @-mentioned, as context: its latest messages on the
    /// branch it shows (bounded).
    async fn mentioned_chat(&self, id: &ConversationId, title: &str) -> String {
        let lines = self.latest_messages(id).await;
        if lines.is_empty() {
            return format!("\n[mentions the conversation \"{title}\", which has no messages]");
        }
        format!(
            "\n[mentions the conversation \"{title}\"; its latest messages follow]\n{}\n[/conversation]",
            lines.join("\n\n")
        )
    }

    /// What a side chat's turn carries: the conversation it sits beside, as it stands.
    async fn side_chat_context(&self, id: &ConversationId, title: &str) -> String {
        let lines = self.latest_messages(id).await;
        let intro = format!(
            "[This is a side chat beside the conversation \"{title}\": the user asks about it \
             here without adding to it, and nothing here changes it."
        );
        if lines.is_empty() {
            return format!("{intro} It has no messages yet.]");
        }
        format!(
            "{intro} Its latest messages follow.]\n{}\n[/conversation]",
            lines.join("\n\n")
        )
    }

    /// A conversation's latest messages on the branch it shows, oldest first, as
    /// "User: …" / "Assistant: …" (bounded).
    async fn latest_messages(&self, id: &ConversationId) -> Vec<String> {
        let branch = match self.core.head(id).await {
            Ok(Some(head)) => self.core.branch(id, &head).await.unwrap_or_default(),
            _ => Vec::new(),
        };
        let mut lines = Vec::new();
        let mut bytes = 0;
        for message in branch.iter().rev().take(MENTIONED_CHAT_MESSAGES) {
            let who = match message.role {
                MessageRole::User => "User",
                MessageRole::Assistant => "Assistant",
                MessageRole::System => continue,
            };
            let words = self
                .core
                .brief_words(&message.text, &message.attachments)
                .await;
            let line = format!("{who}: {words}");
            bytes += line.len();
            if bytes > MENTIONED_CHAT_BYTES {
                break;
            }
            lines.push(line);
        }
        lines.reverse();
        lines
    }

    pub(super) async fn full_text(&self, message: &Message) -> String {
        match &message.blob {
            Some(hash) => self
                .core
                .read_blob_text(hash.clone())
                .await
                .unwrap_or_else(|_| message.text.clone()),
            None => message.text.clone(),
        }
    }

    /// Everything the user wrote in a message: what they typed, then what they pasted.
    pub(super) async fn full_words(&self, message: &Message) -> String {
        let mut words = self.full_text(message).await;
        for pasted in self.core.pasted_texts(&message.attachments).await {
            push_block(&mut words, &pasted);
        }
        words
    }

    /// A Chat's non-image attachment as text in the message: the file itself when it is text
    /// of a sensible size, otherwise a note saying it could not be read.
    async fn inline_attachment(&self, attachment: &AttachmentRef) -> String {
        let bytes = match attachment.id.parse::<brigadier_store::BlobHash>() {
            Ok(hash) => self.core.store().blobs().get(hash).await.ok().flatten(),
            Err(_) => None,
        };
        match bytes.map(String::from_utf8) {
            Some(Ok(content)) if content.len() <= INLINE_TEXT_MAX_BYTES => format!(
                "\n\n[attached file \"{}\" ({})]\n{content}\n[end of \"{}\"]",
                attachment.name, attachment.mime, attachment.name
            ),
            Some(Ok(_)) => format!(
                "\n\n[attached file \"{}\" is larger than {} kB, too large to include here]",
                attachment.name,
                INLINE_TEXT_MAX_BYTES / 1_000
            ),
            _ => format!(
                "\n\n[attached file \"{}\" ({}) is not text and cannot be read in a Chat]",
                attachment.name, attachment.mime
            ),
        }
    }

    /// Writes an attachment into the conversation's own folder so the CLI can read it.
    async fn attachment_file(
        &self,
        conv: &Arc<ConvLive>,
        attachment: &AttachmentRef,
    ) -> Option<InputFile> {
        let area = match conv.kind {
            ConversationKind::Session => "orch",
            ConversationKind::Chat => "chat",
        };
        let dir = self.owned_dir(area, &conv.id.0).join("attachments");
        let path = dir.join(format!(
            "{}-{}",
            &attachment.id[..attachment.id.len().min(12)],
            safe_file_name(&attachment.name)
        ));
        let bytes = self
            .core
            .store()
            .blobs()
            .get(attachment.id.parse::<brigadier_store::BlobHash>().ok()?)
            .await
            .ok()??;
        let target = path.clone();
        super::blocking(move || {
            std::fs::create_dir_all(&dir).map_err(|err| Error::Invalid(err.to_string()))?;
            std::fs::write(&target, bytes).map_err(|err| Error::Invalid(err.to_string()))
        })
        .await
        .ok()?;
        Some(InputFile {
            path,
            name: attachment.name.clone(),
            mime: attachment.mime.clone(),
        })
    }

    /// Stores the conversation CLI's text, reasoning and tool calls in the thread,
    /// and keeps the complete provider events in `orch:<id>` for the Inspector.
    async fn pump_conversation(
        self: Arc<Self>,
        conv: Arc<ConvLive>,
        cli: Arc<Cli>,
        mut events: mpsc::Receiver<ProviderEvent>,
    ) {
        let mut deltas: Vec<ProviderEvent> = Vec::new();
        let mut deadline: Option<tokio::time::Instant> = None;
        let mut quiet = Quiet::new(conv.kind == ConversationKind::Session);
        // A short orchestrator reply, held until what follows it shows whether it is narration.
        let mut held: Option<ProviderEvent> = None;
        loop {
            let flush_at = async {
                match deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                event = events.recv() => match event {
                    // Never stored.
                    Some(ProviderEvent::Progress { .. }) => {}
                    // Only a session's thread keeps what it read (a Chat only searches the web).
                    Some(event @ ProviderEvent::Looked { .. }) => {
                        if conv.kind == ConversationKind::Session {
                            self.hold_looked(&conv, event).await;
                        }
                    }
                    Some(event) if is_delta(&event) => {
                        merge_delta(&mut deltas, event);
                        deadline.get_or_insert_with(|| tokio::time::Instant::now() + DELTA_WINDOW);
                    }
                    Some(event) => {
                        deadline = None;
                        self.store_deltas(&conv, quiet.pass(std::mem::take(&mut deltas))).await;
                        // A worker it starts gets what the thread read so far in its context
                        // pack: recorded when the call starts, before it reaches Brigadier.
                        if matches!(
                            &event,
                            ProviderEvent::TurnCompleted { .. } | ProviderEvent::Exited { .. }
                        ) || matches!(
                            &event,
                            ProviderEvent::ToolCall { name, status: ItemStatus::InProgress, .. }
                                if name.ends_with("delegate_task")
                        ) {
                            self.record_held_looked(&conv).await;
                        }
                        let mut exited = false;
                        for event in
                            self.session_events(EventSource::Conversation(&conv.id), &cli, event)
                        {
                            exited |= matches!(event, ProviderEvent::Exited { .. });
                            match narration(&event) {
                                // The held reply only announced the work this call does: the user
                                // never sees it (the orchestrator log keeps it).
                                Some(true) => {
                                    if let Some(reply) = held.take() {
                                        if self.opening_line(&conv, &reply).await {
                                            self.on_conversation_event(&conv, &cli, reply).await;
                                        } else {
                                            self.hide_narration(&conv, &cli, &reply).await;
                                            self.log_provider(&conv.id, cli.provider, reply).await;
                                        }
                                    }
                                }
                                Some(false) => {
                                    if let Some(reply) = held.take() {
                                        self.on_conversation_event(&conv, &cli, reply).await;
                                    }
                                }
                                None => {}
                            }
                            if quiet.holds_whole(&event) {
                                if let Some(reply) = held.replace(event) {
                                    self.on_conversation_event(&conv, &cli, reply).await;
                                }
                            } else {
                                quiet.forget(&event);
                                self.on_conversation_event(&conv, &cli, event).await;
                            }
                        }
                        if exited {
                            break;
                        }
                    }
                    None => break,
                },
                () = flush_at => {
                    deadline = None;
                    self.store_deltas(&conv, quiet.pass(std::mem::take(&mut deltas))).await;
                }
            }
        }
        self.store_deltas(&conv, quiet.pass(deltas)).await;
        self.record_held_looked(&conv).await;
        if let Some(reply) = held.take() {
            self.on_conversation_event(&conv, &cli, reply).await;
        }
        self.grants.revoke_owner(&cli.owner);
        let (was_busy, closing) = {
            let mut state = conv.state.lock().await;
            if state
                .cli
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &cli))
            {
                state.cli = None;
            }
            let was_busy = state.busy;
            state.busy = false;
            state.compacting = false;
            if let Some(request) = state.request.take()
                && was_busy
                && !state.closing
            {
                state.outcomes.insert(
                    request,
                    RequestState::Failed {
                        error: ENDED_UNEXPECTEDLY.into(),
                    },
                );
            }
            (was_busy, state.closing)
        };
        self.end_compaction(&conv, ENDED_UNEXPECTEDLY.into()).await;
        cli.ended.cancel();
        if was_busy && !closing {
            self.set_run(&conv.id, RunState::Failed, Some(ENDED_UNEXPECTEDLY.into()))
                .await;
        } else if !closing {
            self.set_run(&conv.id, RunState::Idle, None).await;
        }
        self.settle_requests(&conv.id).await;
    }

    async fn store_deltas(&self, conv: &ConvLive, deltas: Vec<ProviderEvent>) {
        let id = &conv.id;
        let request_id = conv.state.lock().await.request.clone();
        let events: Vec<DomainEvent> = deltas
            .into_iter()
            .filter_map(|event| match event {
                ProviderEvent::MessageDelta { item_id, text } => Some(DomainEvent::MessageDelta {
                    conversation_id: id.clone(),
                    message_id: item_id,
                    text,
                }),
                ProviderEvent::ReasoningDelta { item_id, text } => {
                    Some(DomainEvent::ThinkingDelta {
                        conversation_id: id.clone(),
                        item_id,
                        text,
                        request_id: request_id.clone(),
                        at_ms: now_ms(),
                        complete: false,
                    })
                }
                _ => None,
            })
            .collect();
        if events.is_empty() {
            return;
        }
        if let Err(err) = self.core.record_conversation(id, events).await {
            tracing::debug!(conversation = %id, error = %err, "could not store streamed text");
        }
    }

    /// Whether a reply that announces work is the request's one short opening line, which the
    /// user sees: the first reply of the turn that carries their message, with nothing said or
    /// hidden for the request before it.
    async fn opening_line(&self, conv: &ConvLive, reply: &ProviderEvent) -> bool {
        let ProviderEvent::Message { item_id, .. } = reply else {
            return false;
        };
        let state = conv.state.lock().await;
        let Some(request) = state.replying_for.get(item_id).or(state.request.as_ref()) else {
            return false;
        };
        !state.in_turn.is_empty()
            && !state.spoke.contains(request)
            && !state.narration.contains_key(request)
    }

    /// Keeps a reply the narration filter hid, under its request, in case the request ends
    /// with nothing else said.
    async fn hide_narration(&self, conv: &ConvLive, cli: &Cli, reply: &ProviderEvent) {
        let ProviderEvent::Message { item_id, text, .. } = reply else {
            return;
        };
        let mut state = conv.state.lock().await;
        let Some(request) = state
            .replying_for
            .get(item_id)
            .cloned()
            .or_else(|| state.request.clone())
        else {
            return;
        };
        if !state.spoke.contains(&request) {
            state.narration.insert(
                request,
                Narration {
                    item_id: item_id.clone(),
                    text: text.clone(),
                    model: cli.model.clone(),
                },
            );
        }
    }

    /// A request is over: if the user saw no reply for it, the last one the narration filter
    /// hid goes into the thread after all, so an answer is never left empty. Its bookkeeping
    /// goes with it. A request done with nothing said at all after a worker was sent back
    /// (the orchestrator waited for a report, then answered it with [`prompts::QUIET`]) asks
    /// the orchestrator for its answer once, then says there was none.
    pub(super) async fn release_narration(&self, conv: &Arc<ConvLive>, request: &str, done: bool) {
        let (narration, unanswered) = {
            let mut state = conv.state.lock().await;
            let narration = state.narration.remove(request);
            let spoke = state.spoke.remove(request);
            if spoke || narration.is_some() || !done {
                state.sent_back.remove(request);
            }
            if spoke {
                return;
            }
            let unanswered = match state.sent_back.get_mut(request) {
                Some(stage @ Unanswered::Armed) => {
                    *stage = Unanswered::Checking;
                    Some(Unanswered::Checking)
                }
                Some(Unanswered::Reminded) => state.sent_back.remove(request),
                _ => None,
            };
            (narration, unanswered)
        };
        match unanswered {
            Some(Unanswered::Checking) => {
                let (manager, conv, request) = (self.arc(), conv.clone(), request.to_owned());
                self.spawn(async move {
                    tokio::time::sleep(UNANSWERED_GRACE).await;
                    manager.remind_unanswered(&conv, &request).await;
                });
            }
            Some(_) => {
                self.notice(
                    &conv.id,
                    brigadier_providers::NoticeLevel::Warning,
                    "The orchestrator ended this request without an answer. Ask it again for one.",
                )
                .await;
            }
            None => {}
        }
        let Some(narration) = narration else {
            return;
        };
        if let Err(err) = self
            .core
            .append_assistant_message(
                conv.id.clone(),
                narration.item_id,
                narration.text,
                Some(narration.model),
                Some(request.to_owned()),
            )
            .await
        {
            tracing::warn!(conversation = %conv.id, error = %err, "could not store a reply");
        }
    }

    /// A request still over with nothing said a while after it ended (see
    /// [`Self::release_narration`]): the orchestrator is asked for its answer. One that works
    /// again waits for its next end.
    async fn remind_unanswered(&self, conv: &Arc<ConvLive>, request: &str) {
        let board = self.core.board(&conv.id).await.ok();
        // An overnight run's request is answered by the run's report.
        if board
            .as_ref()
            .is_some_and(|board| super::requests::run_of_request(board, request).is_some())
        {
            conv.state.lock().await.sent_back.remove(request);
            return;
        }
        let over = board.is_some_and(|board| {
            board
                .requests
                .get(request)
                .is_some_and(|of| of.state == RequestState::Done)
        });
        let archived = matches!(
            self.core.conversation(&conv.id).map(|c| c.lifecycle),
            Ok(Lifecycle::Archived)
        );
        {
            let mut state = conv.state.lock().await;
            if state.sent_back.get(request) != Some(&Unanswered::Checking) {
                return;
            }
            if archived {
                state.sent_back.remove(request);
                return;
            }
            let carried = state
                .inbox
                .iter()
                .any(|(_, of)| of.as_deref() == Some(request));
            let running = state.busy && state.request.as_deref() == Some(request);
            let stage = if !over || carried || running {
                Unanswered::Armed
            } else {
                Unanswered::Reminded
            };
            state.sent_back.insert(request.to_owned(), stage);
            if stage == Unanswered::Armed {
                return;
            }
            state.inbox.push((
                Envelope {
                    kind: InjectionKind::Reminder,
                    label: "no answer".into(),
                    task_id: None,
                    text: format!(
                        "[Nothing runs for this request any more, and the user has seen no answer \
                         to it. Answer them now: what the workers found or changed, and what is \
                         left. Don't reply {}.]",
                        prompts::QUIET
                    ),
                },
                Some(request.to_owned()),
            ));
        }
        // The turn it starts settles the request.
        self.kick(conv);
    }

    async fn on_conversation_event(
        &self,
        conv: &Arc<ConvLive>,
        cli: &Arc<Cli>,
        event: ProviderEvent,
    ) {
        match &event {
            ProviderEvent::Reasoning { item_id, text } => {
                let request_id = conv.state.lock().await.request.clone();
                if let Err(err) = self
                    .core
                    .record_conversation(
                        &conv.id,
                        vec![DomainEvent::ThinkingDelta {
                            conversation_id: conv.id.clone(),
                            item_id: item_id.clone(),
                            text: text.clone(),
                            request_id,
                            at_ms: now_ms(),
                            complete: true,
                        }],
                    )
                    .await
                {
                    tracing::debug!(conversation = %conv.id, error = %err, "could not store reasoning summary");
                }
            }
            ProviderEvent::Message {
                item_id,
                role: ProviderRole::Assistant,
                text,
            } => {
                let request = {
                    let mut state = conv.state.lock().await;
                    state.last_reply = Some(text.clone());
                    state
                        .replying_for
                        .remove(item_id)
                        .or_else(|| state.request.clone())
                };
                let shown = if conv.kind == ConversationKind::Session {
                    without_quiet(text)
                } else {
                    Some(text.as_str())
                };
                if let Some(text) = shown {
                    if let Some(request) = &request {
                        let mut state = conv.state.lock().await;
                        state.spoke.insert(request.clone());
                        state.narration.remove(request);
                    }
                    if let Err(err) = self
                        .core
                        .append_assistant_message(
                            conv.id.clone(),
                            item_id.clone(),
                            text.to_owned(),
                            Some(cli.model.clone()),
                            request,
                        )
                        .await
                    {
                        tracing::warn!(conversation = %conv.id, error = %err, "could not store a reply");
                    }
                }
            }
            ProviderEvent::ApprovalRequested { request }
                if conv.kind == ConversationKind::Session =>
            {
                // The thread's requests follow the permission level, as a worker's do.
                self.route_thread_approval(conv, cli, request.clone()).await;
            }
            ProviderEvent::ApprovalRequested { request } => {
                // A Chat only searches the web.
                let decision = if matches!(request.tool.as_str(), "WebSearch" | "WebFetch") {
                    ApprovalDecision::Allow
                } else {
                    ApprovalDecision::Deny {
                        message: "Declined by Brigadier: a Chat only searches the web.".into(),
                    }
                };
                if let Err(err) = cli
                    .session
                    .answer(request.id.clone(), decision.clone())
                    .await
                {
                    tracing::warn!(conversation = %conv.id, error = %err, "could not answer an approval");
                }
                self.log_provider(
                    &conv.id,
                    cli.provider,
                    ProviderEvent::ApprovalResolved {
                        id: request.id.clone(),
                        decision,
                        decided_by: Decider::Policy,
                    },
                )
                .await;
            }
            ProviderEvent::Error { error } => {
                let limited = error.kind == ErrorKind::UsageLimit && !error.will_retry;
                if limited {
                    conv.state.lock().await.limit_hit =
                        Some(error.limit.clone().unwrap_or(LimitHit {
                            window: None,
                            resets_at_ms: None,
                            kind: brigadier_providers::LimitKind::UsageWindow,
                        }));
                }
                // A limit another account of the provider takes over from: the chat goes on
                // there, and only the note saying so is shown (see `switch_account`).
                let switching = limited
                    && self
                        .runtime
                        .switch_target(&cli.account, cli.model.model.as_deref())
                        .is_some();
                if !error.will_retry {
                    conv.state.lock().await.turn_error = Some(error.message.clone());
                }
                if !error.will_retry && !switching {
                    self.notice(
                        &conv.id,
                        brigadier_providers::NoticeLevel::Warning,
                        &error.message,
                    )
                    .await;
                }
            }
            ProviderEvent::Notice { level, message } => {
                self.notice(&conv.id, *level, message).await;
            }
            ProviderEvent::ToolCall {
                item_id,
                name,
                input,
                status,
                output,
            } => {
                let name = name.rsplit("__").next().unwrap_or(name);
                let name = name.rsplit('.').next().unwrap_or(name);
                let shell = crate::digest::is_shell_tool(name);
                if shell {
                    conv.running_command(item_id, *status).await;
                }
                let ended_at_ms = (*status != ItemStatus::InProgress).then(now_ms);
                // A shell tool's result names its exit status on its first line.
                let exit = output
                    .as_deref()
                    .filter(|_| shell && ended_at_ms.is_some())
                    .and_then(crate::digest::exit_of);
                let args: serde_json::Value = input
                    .as_deref()
                    .and_then(|input| serde_json::from_str(input).ok())
                    .unwrap_or_default();
                // A preview by its name, a command (Bash, run) by its own words.
                let detail = [
                    "query",
                    "pattern",
                    "file_path",
                    "path",
                    "url",
                    "title",
                    "task",
                    "name",
                    "command",
                ]
                .into_iter()
                .find_map(|key| args.get(key).and_then(serde_json::Value::as_str))
                .map(|text| text.chars().take(240).collect());
                self.orchestrator_step(
                    &conv.id,
                    OrchestratorStepKind::Tool {
                        item_id: item_id.clone(),
                        name: name.to_owned(),
                        detail,
                        status: *status,
                        through_position: 0,
                        ended_at_ms,
                        exit,
                    },
                )
                .await;
                if conv.kind == ConversationKind::Chat
                    && *status == ItemStatus::Completed
                    && let Some(kind) = web_step(name, input.as_deref())
                {
                    self.orchestrator_step(&conv.id, kind).await;
                }
            }
            // A Codex thread's own shell commands and edits come as items of their own, not as
            // tool calls: the live line names them the same way.
            ProviderEvent::Command {
                item_id,
                command,
                status,
                exit_code,
                ..
            } => {
                conv.running_command(item_id, *status).await;
                let command = brigadier_providers::policy::unwrapped_command(command);
                self.orchestrator_step(
                    &conv.id,
                    OrchestratorStepKind::Tool {
                        item_id: item_id.clone(),
                        name: "shell".into(),
                        detail: Some(command.chars().take(240).collect()),
                        status: *status,
                        through_position: 0,
                        ended_at_ms: (*status != ItemStatus::InProgress).then(now_ms),
                        exit: *exit_code,
                    },
                )
                .await;
            }
            ProviderEvent::FileChanges {
                item_id,
                changes,
                status,
            } => {
                let detail = changes.first().map(|first| match changes.len() {
                    1 => first.path.clone(),
                    n => format!("{} and {} more", first.path, n - 1),
                });
                self.orchestrator_step(
                    &conv.id,
                    OrchestratorStepKind::Tool {
                        item_id: item_id.clone(),
                        name: "apply_patch".into(),
                        detail,
                        status: *status,
                        through_position: 0,
                        ended_at_ms: (*status != ItemStatus::InProgress).then(now_ms),
                        exit: None,
                    },
                )
                .await;
            }
            ProviderEvent::RateLimits { quota } => {
                self.runtime
                    .note_quota_snapshot(&cli.account, quota.clone())
                    .await;
            }
            ProviderEvent::TurnStarted { .. } => {
                cli.meter.turn_started(now_ms());
                let started = {
                    let mut state = conv.state.lock().await;
                    state.landed = true;
                    std::mem::take(&mut state.awaiting_start).then(|| state.request.clone())
                };
                if let Some(request) = started {
                    self.set_run_for(&conv.id, RunState::Running, None, request)
                        .await;
                }
            }
            ProviderEvent::Usage { total, last } => {
                // Claude sums a turn's calls; the context its last call read came apart.
                let context = conv.context_used().await;
                self.note_use(
                    &cli.meter,
                    cli.provider,
                    cli.model.model.as_deref(),
                    TokenOwner::Conversation(&conv.id),
                    total,
                    last.as_ref(),
                    context,
                )
                .await;
            }
            ProviderEvent::ContextSize {
                used_tokens,
                window_tokens,
            } => {
                let mut state = conv.state.lock().await;
                let window = window_tokens.or(state.context.and_then(|(_, window)| window));
                state.context = Some((*used_tokens, window));
                if conv.kind == ConversationKind::Session {
                    state.cache = Some(CacheMark {
                        native_id: cli.session.native_id(),
                        provider: cli.provider,
                        at_ms: now_ms(),
                        context: *used_tokens,
                        window,
                    });
                }
            }
            ProviderEvent::CompactionStarted { automatic }
                if conv.kind == ConversationKind::Session =>
            {
                // PLAN.md §2: an orchestrator is reborn, never compacted.
                self.log_orchestrator(
                    &conv.id,
                    OrchestratorEntry::ContractBreach {
                        message: format!(
                            "The orchestrator's {} CLI compacted its context{}.",
                            cli.provider,
                            if *automatic { " on its own" } else { "" }
                        ),
                    },
                )
                .await;
            }
            ProviderEvent::CompactionStarted { automatic }
                if conv.kind == ConversationKind::Chat =>
            {
                let after = self
                    .core
                    .board(&conv.id)
                    .await
                    .ok()
                    .and_then(|board| board.head.map(|(head, _)| head));
                let compaction = {
                    let mut state = conv.state.lock().await;
                    let compaction = Compaction {
                        id: uuid::Uuid::now_v7().to_string(),
                        // Compacting on its own, the model is in the middle of a request.
                        request_id: automatic.then(|| state.request.clone()).flatten(),
                        after,
                        automatic: *automatic,
                        state: CompactionState::Running,
                        tokens_before: None,
                        tokens_after: None,
                        started_at_ms: now_ms(),
                        ended_at_ms: None,
                        position: 0,
                    };
                    state.compaction = Some(compaction.clone());
                    compaction
                };
                self.record_compaction(&conv.id, compaction).await;
            }
            ProviderEvent::CompactionEnded {
                automatic,
                tokens_before,
                tokens_after,
                error,
            } if conv.kind == ConversationKind::Chat => {
                let running = conv.state.lock().await.compaction.take();
                let now = now_ms();
                let mut compaction = running.unwrap_or_else(|| Compaction {
                    id: uuid::Uuid::now_v7().to_string(),
                    request_id: None,
                    after: None,
                    automatic: *automatic,
                    state: CompactionState::Running,
                    tokens_before: None,
                    tokens_after: None,
                    started_at_ms: now,
                    ended_at_ms: None,
                    position: 0,
                });
                compaction.state = match error {
                    Some(error) => CompactionState::Failed {
                        error: error.clone(),
                    },
                    None => CompactionState::Done,
                };
                compaction.tokens_before = *tokens_before;
                compaction.tokens_after = *tokens_after;
                compaction.ended_at_ms = Some(now);
                self.record_compaction(&conv.id, compaction).await;
            }
            _ => {}
        }
        let completed = match &event {
            ProviderEvent::TurnCompleted { status, .. } => Some(*status),
            _ => None,
        };
        self.log_provider(&conv.id, cli.provider, event).await;
        if let Some(status) = completed {
            self.turn_completed(conv, cli, status).await;
        }
    }

    /// Stores a compaction's snapshot.
    async fn record_compaction(&self, id: &ConversationId, compaction: Compaction) {
        if let Err(err) = self
            .core
            .record_conversation(id, vec![DomainEvent::CompactionUpdated { compaction }])
            .await
        {
            tracing::warn!(conversation = %id, error = %err, "could not store a compaction");
        }
    }

    /// A compaction the turn left unfinished failed (it was stopped, or the CLI ended).
    async fn end_compaction(&self, conv: &Arc<ConvLive>, error: String) {
        let Some(mut compaction) = conv.state.lock().await.compaction.take() else {
            return;
        };
        compaction.state = CompactionState::Failed { error };
        compaction.ended_at_ms = Some(now_ms());
        self.record_compaction(&conv.id, compaction).await;
    }

    async fn turn_completed(&self, conv: &Arc<ConvLive>, cli: &Arc<Cli>, status: TurnStatus) {
        // A turn that ended before its CLI said it began starts nothing more.
        conv.state.lock().await.awaiting_start = false;
        let unfinished = match status {
            TurnStatus::Interrupted => "You stopped it".to_owned(),
            _ => conv
                .state
                .lock()
                .await
                .turn_error
                .clone()
                .unwrap_or_else(|| "The model did not finish compacting".into()),
        };
        self.end_compaction(conv, unfinished).await;
        if conv.kind == ConversationKind::Session {
            // What the thread committed in this turn gets its review, before a next turn can
            // take its commits for the user's.
            self.scan_thread_commits(&conv.id, true).await;
            // What its Codex auto-reviews used, which its own totals leave out.
            self.meter_child_threads(cli, TokenOwner::Conversation(&conv.id))
                .await;
            // Before the next turn may start: a turn admitted in between would still run on
            // this CLI, past the swap threshold.
            self.consider_rebirth(conv, cli).await;
        }
        // A card the thread opened for a request is what the user answers; a question in its
        // text alongside one asks nothing more.
        let carded: HashSet<String> = match conv.kind {
            ConversationKind::Session => self
                .core
                .board(&conv.id)
                .await
                .map(|board| {
                    board
                        .questions
                        .values()
                        .filter(|question| {
                            question.is_open()
                                && matches!(
                                    question.kind,
                                    QuestionKind::Orchestrator | QuestionKind::Merge { .. }
                                )
                        })
                        .filter_map(|question| question.request_id.clone())
                        .collect()
                })
                .unwrap_or_default(),
            _ => HashSet::new(),
        };
        let in_run = self.overnight.active.get(&conv.id).is_some();
        let (limit_hit, ended, landed, carried, asked, served, end_commands) = {
            let mut state = conv.state.lock().await;
            state.commands.clear();
            // Taken now, so no turn starts on it in between (a stand-in closes it anyway).
            let end_commands = (std::mem::take(&mut state.end_commands)
                && state.limit_hit.is_none()
                && state.cli.as_ref().is_some_and(|now| Arc::ptr_eq(now, cli)))
            .then(|| {
                state.closing = true;
                state.cli.take()
            })
            .flatten();
            state.busy = false;
            state.compacting = false;
            state.last_activity_ms = now_ms();
            state.replying_for.clear();
            let ended = match status {
                TurnStatus::Interrupted => Some(RequestState::Stopped),
                TurnStatus::Failed => Some(RequestState::Failed {
                    error: state
                        .turn_error
                        .take()
                        .unwrap_or_else(|| "The reply failed.".into()),
                }),
                _ => None,
            };
            let served = state.request.take();
            let limit_hit = state.limit_hit.take();
            // A CLI at its limit fails the turn; an injected limit (development builds)
            // interrupts it first. Its request isn't over until a hand-over is chosen: it goes
            // on elsewhere, or waits for quota, or fails then. No turn starts meanwhile.
            let limited = limit_hit.filter(|_| status != TurnStatus::Completed);
            if limited.is_some() {
                state.hand_over = Some(HandOver {
                    request: served.clone(),
                    stopped: false,
                });
            }
            if let Some(request) = &served
                && let Some(ended) = ended.clone()
                && limited.is_none()
            {
                state.outcomes.insert(request.clone(), ended);
            }
            // A session's turn that ended on a question to the user in its text: the thread is
            // told once to ask it on a card, and the request goes on. After that, nothing
            // pushes the orchestrator past it before the user answers (see `asked_user`).
            let reply = state.last_reply.take();
            if conv.kind == ConversationKind::Session
                && status == TurnStatus::Completed
                && let Some(request) = &served
                && reply.as_deref().is_some_and(asks_user)
            {
                if carded.contains(request) || in_run || state.told_to_ask_on_card.contains(request)
                {
                    state.asked_user.insert(request.clone());
                } else {
                    state.told_to_ask_on_card.insert(request.clone());
                    state.inbox.push((
                        Envelope {
                            kind: InjectionKind::Reminder,
                            label: "ask on a card".into(),
                            task_id: None,
                            text: ASK_ON_A_CARD.to_owned(),
                        },
                        Some(request.clone()),
                    ));
                }
            }
            (
                limited,
                ended,
                state.landed,
                std::mem::take(&mut state.in_turn),
                std::mem::take(&mut state.asked),
                served,
                end_commands,
            )
        };
        // Follow-ups the turn left undecided wait in the queue for their own turn.
        if let Err(err) = self.core.settle_queued(&conv.id, &asked).await {
            tracing::warn!(conversation = %conv.id, error = %err, "could not settle follow-ups");
        }
        if let Some(limit) = limit_hit {
            self.runtime.note_limit(&cli.account, limit.clone()).await;
            #[cfg(test)]
            {
                let pause = conv.hand_over_pause.lock().unwrap().clone();
                if let Some((reached, release)) = pause {
                    reached.notify_one();
                    release.notified().await;
                }
            }
            // Another account of the provider first, with switching on.
            if let Some(next) = self
                .runtime
                .switch_target(&cli.account, cli.model.model.as_deref())
            {
                // Its turn goes on there. Not from inside the CLI's own event pump: closing the
                // CLI waits for it.
                let (manager, conv, cli) = (self.arc(), conv.clone(), cli.clone());
                self.spawn(async move {
                    manager
                        .switch_account(&conv, &cli, next, carried, landed)
                        .await;
                });
                return;
            }
            match self.stand_in_choice(conv, &cli.model).await {
                Ok(next) => {
                    // Its turn goes on there. Not from inside the CLI's own event pump: closing
                    // the CLI waits for it.
                    let (manager, conv, from) = (self.arc(), conv.clone(), cli.model.clone());
                    let until = limit.resets_at_ms;
                    self.spawn(async move {
                        manager.stand_in(&conv, &from, next, until, carried).await;
                    });
                    return;
                }
                Err(waiting) => {
                    // Its own model's reset frees the messages too.
                    if let Some(waiting) = with_own_reset(waiting, &cli.model, &limit) {
                        let waits = self
                            .wait_for_model(conv, &cli.model, waiting, carried, served)
                            .await;
                        self.set_run(&conv.id, RunState::Idle, None).await;
                        self.settle_requests(&conv.id).await;
                        // Stopped meanwhile: what the user sent since (a Resume, a message)
                        // goes now.
                        if !waits {
                            self.kick(conv);
                        }
                        return;
                    }
                }
            }
            // Nowhere to go: the turn fails (unless the user stopped it meanwhile).
            let mut state = conv.state.lock().await;
            let stopped = state.hand_over.take().is_some_and(|over| over.stopped);
            if let Some(request) = &served
                && let Some(ended) = ended
                && !stopped
            {
                state.outcomes.insert(request.clone(), ended);
            }
        }
        self.set_run(&conv.id, RunState::Idle, None).await;
        self.settle_requests(&conv.id).await;
        if status == TurnStatus::Completed
            && let Some(request) = &served
        {
            self.remind_undecided(conv, request).await;
        }
        if let Some(cli) = end_commands {
            // Not from inside the CLI's own event pump: closing the CLI waits for it. The next
            // turn waits for the close.
            let (manager, conv) = (self.arc(), conv.clone());
            self.spawn(async move {
                cli.session.close().await;
                cli.ended.cancelled().await;
                conv.state.lock().await.closing = false;
                manager.kick(&conv);
            });
            return;
        }
        self.kick(conv);
    }

    /// A request over while reported changes still wait for the orchestrator's decision: it
    /// is asked once per task to accept, send back or stop them, so none stays open for good.
    async fn remind_undecided(&self, conv: &Arc<ConvLive>, request: &str) {
        let Ok(board) = self.core.board(&conv.id).await else {
            return;
        };
        if board
            .requests
            .get(request)
            .is_none_or(|of| of.state != RequestState::Done)
            || super::requests::ended_run_request(&board, request)
        {
            return;
        }
        let undecided = self.undecided(&board, request).await;
        let mut state = conv.state.lock().await;
        // Its last turn asked the user something: nothing pushes it past that before they
        // answer.
        if state.asked_user.contains(request) {
            return;
        }
        let new: Vec<TaskId> = undecided
            .into_iter()
            .map(|task| task.id)
            .filter(|id| !state.reminded.contains(id))
            .collect();
        if new.is_empty() {
            return;
        }
        state.reminded.extend(new);
        state.inbox.push((
            Envelope {
                kind: InjectionKind::Reminder,
                label: "undecided reports".into(),
                task_id: None,
                // The turn's notes list them (see `request_notes`).
                text: format!(
                    "[Your answer is out, but reported work still waits for your decision (listed below). Decide now; then reply {} unless what you told the user changes.]",
                    prompts::QUIET
                ),
            },
            Some(request.to_owned()),
        ));
    }

    /// After an orchestrator's turn: past the prepare threshold its handoff note is started;
    /// past the swap threshold the next turn is reborn.
    async fn consider_rebirth(&self, conv: &Arc<ConvLive>, cli: &Arc<Cli>) {
        let Some((used, window)) = conv.state.lock().await.context else {
            return;
        };
        let (prepare, due) = rebirth::rebirth_needed(cli.provider, used, window);
        if !prepare {
            return;
        }
        let prep = conv.state.lock().await.rebirth.clone();
        let prep = match prep {
            Some(prep) => prep,
            None => {
                let prep = self.prepare_rebirth(
                    &conv.id,
                    cli.provider,
                    cli.model.clone(),
                    Some(cli.session.native_id()),
                    (used, window),
                    rebirth::HandoffPurpose::Threshold,
                );
                tracing::info!(conversation = %conv.id, used, "preparing the orchestrator's rebirth");
                conv.state.lock().await.rebirth = Some(prep.clone());
                prep
            }
        };
        if due {
            prep.set_due();
        }
    }

    /// Between turns: an orchestrator whose handoff note is ready, or whose rebirth is due and
    /// cannot wait for it ([`rebirth::swap_now`]), is reborn. Its CLI closes; the next one
    /// starts fresh from a briefing.
    async fn rebirth_if_ready(&self, conv: &Arc<ConvLive>) {
        let (prep, cli) = {
            let mut state = conv.state.lock().await;
            let Some(prep) = state.rebirth.clone() else {
                return;
            };
            // A note written only after the old CLI closed can't be waited for before.
            let now = prep.ready()
                || (prep.is_due()
                    && (prep.waits_for_close()
                        || rebirth::swap_now(
                            state.cli.as_ref().map(|cli| cli.provider),
                            state.context,
                        )));
            if state.busy || state.closing || !now {
                return;
            }
            // Nothing else starts a turn meanwhile.
            state.closing = true;
            (prep, state.cli.take())
        };
        let swap_started_at_ms = now_ms();
        if prep.waits_for_close() {
            if let Some(cli) = cli {
                cli.session.close().await;
                cli.ended.cancelled().await;
            }
            prep.closed();
            prep.note(rebirth::HANDOFF_AFTER_CLOSE).await;
        } else {
            if !prep.ready() {
                prep.note(rebirth::HANDOFF_WAIT).await;
            }
            if let Some(cli) = cli {
                cli.session.close().await;
                cli.ended.cancelled().await;
            }
        }
        let mut state = conv.state.lock().await;
        state.closing = false;
        state.rebirth = None;
        state.checkpoint = None;
        state.context = None;
        state.fresh = true;
        state.reseed = false;
        state.briefing = Some(BriefingPlan {
            trigger: RebirthTrigger::Threshold,
            at_tokens: prep.at_tokens,
            window: prep.window,
            prep: Some(prep),
            swap_started_at_ms,
        });
    }

    /// Between turns: an orchestrator whose prompt cache has expired, with a checkpoint that
    /// covers everything since its last request, is reborn from it instead of resuming
    /// (PLAN.md §7). A live CLI closes; the next turn starts fresh from a briefing.
    async fn rebirth_if_cache_expired(&self, conv: &Arc<ConvLive>) {
        if conv.turn_running().await || conv.rebirth_pending().await {
            return;
        }
        let Some(plan) = self.cold_rebirth_plan(conv).await else {
            return;
        };
        let cli = {
            let mut state = conv.state.lock().await;
            if state.busy || state.closing {
                return;
            }
            // Nothing else starts a turn meanwhile.
            state.closing = true;
            state.cli.take()
        };
        if let Some(cli) = cli {
            cli.session.close().await;
            cli.ended.cancelled().await;
        }
        let mut state = conv.state.lock().await;
        state.closing = false;
        state.checkpoint = None;
        state.context = None;
        state.fresh = true;
        state.reseed = false;
        state.briefing = Some(plan);
    }

    /// The model a conversation continues on after `cli`'s model hit a usage limit: the
    /// router's choice for its kind of conversation, less that model (its provider too when
    /// the limit is provider-wide), if one is usable.
    ///
    /// Without one: the reason to wait, when a reset or the user's rules and rankings could
    /// change that (else nothing: the turn fails as before).
    async fn stand_in_choice(
        &self,
        conv: &ConvLive,
        from: &ModelChoice,
    ) -> std::result::Result<brigadier_router::Routed, Option<brigadier_router::Waiting>> {
        let category = match conv.kind {
            ConversationKind::Session => brigadier_router::TaskCategory::Orchestrate,
            ConversationKind::Chat => brigadier_router::TaskCategory::Chat,
        };
        let project = self
            .core
            .conversation(&conv.id)
            .ok()
            .and_then(|conversation| conversation.project_id);
        let exclude = [brigadier_router::Exclusion {
            provider: from.provider,
            model: from.model.clone(),
        }];
        let decision = self
            .decide(&super::routing::Ask {
                category,
                areas: &[],
                floor: brigadier_router::default_floor(category),
                needs: brigadier_router::Needs::default(),
                pin: None,
                hold_pin: false,
                avoid: None,
                distinct_from: Vec::new(),
                exclude: &exclude,
                project_id: project.as_ref(),
                trial: super::routing::Trial::Never,
            })
            .await;
        let next = match decision {
            brigadier_router::Decision::Run(next) => next,
            brigadier_router::Decision::Wait(waiting) => {
                let waits = waiting.resets_at_ms.is_some()
                    || waiting.rule.is_some()
                    || waiting.ranking.is_some();
                return Err(waits.then_some(waiting));
            }
        };
        Ok(next)
    }

    /// A conversation's model hit its usage limit: it continues on `next`, which starts over
    /// from the transcript (an orchestrator from a recovery briefing), and the turn is sent
    /// again. Only the conversation's stand-in is recorded: its setup, the project's and the
    /// app's saved choices are left alone (Q17). The chosen model takes over again after its
    /// reset ([`Self::end_stand_in`]).
    async fn stand_in(
        &self,
        conv: &Arc<ConvLive>,
        from: &ModelChoice,
        next: brigadier_router::Routed,
        until_ms: Option<i64>,
        carried: Vec<Message>,
    ) {
        let choice = ModelChoice {
            provider: next.provider,
            model: Some(next.model.clone()),
            effort: next.effort.clone(),
            fast: None,
            account: None,
        };
        let replaces = match self.core.conversation(&conv.id) {
            Ok(conversation) => conversation
                .fallback
                .clone()
                .map(|fallback| fallback.replaces)
                .or_else(|| setup_choice(&conversation))
                .unwrap_or_else(|| from.clone()),
            Err(_) => from.clone(),
        };
        let stand_in = format!("{} {}", next.provider.label(), next.model);
        let reason = format!("{} hit its usage limit", from.provider.label());
        self.notice(
            &conv.id,
            brigadier_providers::NoticeLevel::Info,
            &format!("{reason}; continuing with {stand_in}."),
        )
        .await;
        conv.close_cli().await;
        let fallback = ModelFallback {
            choice,
            replaces,
            reason,
            since_ms: now_ms(),
            until_ms,
        };
        if let Err(err) = self.core.set_fallback(&conv.id, Some(fallback)).await {
            tracing::warn!(conversation = %conv.id, error = %err, "could not record the stand-in model");
        }
        let stopped = {
            let mut state = conv.state.lock().await;
            // A new session, not an old one of that vendor, briefed from the transcript.
            state.fresh = true;
            state.reseed = true;
            // Stopped meanwhile: its messages stay in the transcript, as a stopped turn's do.
            let stopped = state.hand_over.take().is_some_and(|over| over.stopped);
            if !stopped {
                let mut pending = carried;
                pending.append(&mut state.pending);
                state.pending = pending;
            }
            stopped
        };
        // Stopped meanwhile: its turn is over, as a stopped turn's is (else the next one runs).
        if stopped {
            self.set_run(&conv.id, RunState::Idle, None).await;
        }
        self.kick(conv);
    }

    /// `cli`'s account hit its limit and another account of its provider can take the work:
    /// the same CLI session resumes there (every account shares the CLI's session history).
    /// The failed turn's messages (`carried`) go again, or, once the CLI had begun that turn
    /// (`landed`), a note to carry on goes in their place, so nothing it did is done twice.
    async fn switch_account(
        &self,
        conv: &Arc<ConvLive>,
        cli: &Arc<Cli>,
        next: crate::accounts::AccountRef,
        carried: Vec<Message>,
        landed: bool,
    ) {
        let (from, to) = (
            self.runtime.account_label(&cli.account),
            self.runtime.account_label(&next),
        );
        self.notice(
            &conv.id,
            brigadier_providers::NoticeLevel::Info,
            &format!(
                "{} hit its usage limit on {from}; continuing on {to}.",
                cli.provider.label()
            ),
        )
        .await;
        conv.close_cli().await;
        if let Err(err) = self.run_on_account(&conv.id, &cli.model, &next).await {
            tracing::warn!(conversation = %conv.id, error = %err, "could not record the account it moved to");
        }
        let stopped = {
            let mut state = conv.state.lock().await;
            // Stopped meanwhile: its messages stay in the transcript, as a stopped turn's do.
            let stopped = state.hand_over.take().is_some_and(|over| over.stopped);
            if !stopped {
                if landed {
                    state.continuing = carried.iter().map(|message| message.id.clone()).collect();
                }
                let mut pending = carried;
                pending.append(&mut state.pending);
                state.pending = pending;
            }
            stopped
        };
        // Stopped meanwhile: its turn is over, as a stopped turn's is (else the next one runs).
        if stopped {
            self.set_run(&conv.id, RunState::Idle, None).await;
        }
        self.kick(conv);
    }

    /// Records that the conversation now runs on `account`: in its stand-in while one stands
    /// in, else in its own choice (where the app's account switch shows it).
    async fn run_on_account(
        &self,
        id: &ConversationId,
        running: &ModelChoice,
        account: &crate::accounts::AccountRef,
    ) -> Result<()> {
        let conversation = self.core.conversation(id)?;
        let named = Some(account.choice_id());
        if let Some(mut fallback) = conversation.fallback.clone() {
            fallback.choice.account = named;
            return self.core.set_fallback(id, Some(fallback)).await.map(drop);
        }
        let mut setup = conversation.setup.clone().unwrap_or_else(|| Setup::Chat {
            model: running.clone(),
        });
        match &mut setup {
            Setup::Session { orchestrator, .. } => orchestrator.account = named,
            Setup::Chat { model } => model.account = named,
        }
        self.core.set_setup(id.clone(), setup).await.map(drop)
    }

    /// A conversation's model hit its limit and no model it may use can stand in: its messages
    /// (`carried`, the failed turn's own) wait in `pending` and go on their own once a model
    /// can take them: at the reset, when the user changes their routing, or when a provider's
    /// state changes ([`Self::retry_conversation`]). Whether they wait (not once the user
    /// stopped the turn).
    async fn wait_for_model(
        &self,
        conv: &Arc<ConvLive>,
        from: &ModelChoice,
        waiting: brigadier_router::Waiting,
        carried: Vec<Message>,
        served: Option<String>,
    ) -> bool {
        let first = {
            let mut state = conv.state.lock().await;
            // The user stopped the limited turn while its hand-over was chosen: nothing waits.
            if state.hand_over.take().is_some_and(|over| over.stopped) {
                return false;
            }
            state.waited_model = Some(from.clone());
            let mut pending = carried;
            pending.append(&mut state.pending);
            state.pending = pending;
            // The request isn't over: its messages wait.
            if let Some(request) = &served {
                state.outcomes.remove(request);
            }
            !std::mem::replace(&mut state.waiting, true)
        };
        if first {
            tracing::info!(conversation = %conv.id, reason = %waiting.reason, "messages wait for quota");
            self.notice(
                &conv.id,
                brigadier_providers::NoticeLevel::Info,
                &format!(
                    "{}. Your message waits and goes on its own once a model you allow can take it.",
                    waiting.reason.trim_end_matches('.')
                ),
            )
            .await;
        }
        self.keep_conversation_waiting(conv, waiting).await;
        true
    }

    /// Records what a waiting conversation waits for (when that changed) and sets the timer
    /// that looks again.
    async fn keep_conversation_waiting(
        &self,
        conv: &Arc<ConvLive>,
        waiting: brigadier_router::Waiting,
    ) {
        let known = self
            .core
            .conversation(&conv.id)
            .ok()
            .and_then(|conversation| conversation.quota_wait);
        let messages = conv
            .state
            .lock()
            .await
            .pending
            .iter()
            .map(|message| message.id.clone())
            .collect();
        let wait = QuotaWait {
            reason: waiting.reason,
            resets_at_ms: waiting.resets_at_ms,
            rule: waiting.rule,
            ranking: waiting.ranking,
            since_ms: known.as_ref().map_or_else(now_ms, |known| known.since_ms),
            messages,
        };
        let unchanged = known.as_ref() == Some(&wait);
        if !unchanged
            && let Err(err) = self
                .core
                .set_conversation_wait(&conv.id, Some(wait.clone()))
                .await
        {
            tracing::warn!(conversation = %conv.id, error = %err, "could not record the wait");
        }
        let timer = {
            let mut state = conv.state.lock().await;
            state.wait_timer += 1;
            state.wait_timer
        };
        let delay = super::fallback::retry_delay(wait.resets_at_ms);
        let (manager, conv) = (self.arc(), conv.clone());
        self.spawn(async move {
            tokio::time::sleep(delay).await;
            if conv.state.lock().await.wait_timer == timer {
                manager.retry_conversation(&conv).await;
            }
        });
    }

    /// Looks again whether a model can take a waiting conversation's messages: the model it
    /// runs on once that one can run again (the user may have picked another meanwhile), else
    /// a stand-in; otherwise it keeps waiting.
    /// Boxed: its timer comes back here, and a recursive future must name its `Send` bound.
    fn retry_conversation<'a>(
        &'a self,
        conv: &'a Arc<ConvLive>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            // One retry at a time: a second one would close the model the first just started.
            let _retrying = conv.retry.lock().await;
            let waited = {
                let state = conv.state.lock().await;
                if !state.waiting {
                    return;
                }
                state.waited_model.clone()
            };
            // An archived (or deleted) conversation's messages never go.
            let attached = self.is_attached(conv);
            let Ok(conversation) = self.core.conversation(&conv.id) else {
                return;
            };
            if !attached || conversation.lifecycle == Lifecycle::Archived {
                return;
            }
            // A Chat without a model of its own waits for the one it ran on (the default).
            let from = conversation
                .fallback
                .as_ref()
                .map(|fallback| fallback.choice.clone())
                .or_else(|| setup_choice(&conversation))
                .or(waited)
                .unwrap_or_else(|| self.default_chat_choice());
            if self.choice_available(&from).await {
                self.stop_waiting(conv).await;
                self.kick(conv);
                return;
            }
            let known_reset = conversation
                .quota_wait
                .as_ref()
                .and_then(|wait| wait.resets_at_ms)
                .filter(|at| *at > now_ms());
            match self.stand_in_choice(conv, &from).await {
                Ok(next) => {
                    self.stop_waiting(conv).await;
                    // Its messages are in `pending` already.
                    self.stand_in(conv, &from, next, None, Vec::new()).await;
                }
                Err(Some(mut waiting)) => {
                    // A reset still ahead (its own model's, from its limit) frees it too.
                    waiting.resets_at_ms = match (waiting.resets_at_ms, known_reset) {
                        (Some(named), Some(known)) => Some(named.min(known)),
                        (named, known) => named.or(known),
                    };
                    self.keep_conversation_waiting(conv, waiting).await;
                }
                // No reset or routing change would help now; it still waits for its own model.
                Err(None) => {
                    let waiting = match conversation.quota_wait {
                        Some(wait) => brigadier_router::Waiting {
                            reason: wait.reason,
                            resets_at_ms: known_reset,
                            rule: wait.rule,
                            ranking: wait.ranking,
                        },
                        None => brigadier_router::Waiting {
                            reason: format!(
                                "{} can't run now",
                                super::fallback::model_label(&from)
                            ),
                            resets_at_ms: known_reset,
                            rule: None,
                            ranking: None,
                        },
                    };
                    self.keep_conversation_waiting(conv, waiting).await;
                }
            }
        })
    }

    /// The model a Chat without one of its own runs on: the app's default for Chats.
    fn default_chat_choice(&self) -> ModelChoice {
        self.core
            .settings()
            .default_chat_model
            .unwrap_or(ModelChoice {
                provider: ProviderKind::Claude,
                model: None,
                effort: None,
                fast: None,
                account: None,
            })
    }

    /// The conversation no longer waits for quota.
    async fn stop_waiting(&self, conv: &Arc<ConvLive>) {
        {
            let mut state = conv.state.lock().await;
            state.waiting = false;
            state.waited_model = None;
            // Any timer still set has nothing to do.
            state.wait_timer += 1;
        }
        let recorded = self
            .core
            .conversation(&conv.id)
            .is_ok_and(|conversation| conversation.quota_wait.is_some());
        if recorded && let Err(err) = self.core.set_conversation_wait(&conv.id, None).await {
            tracing::warn!(conversation = %conv.id, error = %err, "could not end the wait");
        }
    }

    /// The conversation's waiting messages are dropped: it is archived.
    pub(super) async fn drop_waiting(&self, conv: &Arc<ConvLive>) {
        let _retrying = conv.retry.lock().await;
        conv.state.lock().await.pending.clear();
        self.stop_waiting(conv).await;
    }

    /// Looks again at every conversation whose messages wait for quota.
    pub(super) async fn retry_waiting_conversations(&self) {
        let convs: Vec<Arc<ConvLive>> = self.convs_lock().values().cloned().collect();
        for conv in convs {
            if conv.state.lock().await.waiting {
                self.retry_conversation(&conv).await;
            }
        }
    }

    /// After a restart: a conversation whose messages waited for quota keeps waiting with
    /// them (those it recorded; from before it recorded them, the user messages after its
    /// last reply), its timer set again.
    pub(super) async fn keep_conversation_wait(&self, conversation: &crate::model::Conversation) {
        let Some(wait) = conversation.quota_wait.clone() else {
            return;
        };
        let Ok(conv) = self.conv(&conversation.id) else {
            return;
        };
        let branch = match self.core.head(&conversation.id).await {
            Ok(Some(head)) => self
                .core
                .branch(&conversation.id, &head)
                .await
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let unanswered: Vec<Message> = if wait.messages.is_empty() {
            let mut trailing: Vec<Message> = branch
                .into_iter()
                .rev()
                .take_while(|message| message.role != MessageRole::Assistant)
                .filter(|message| message.role == MessageRole::User)
                .collect();
            trailing.reverse();
            trailing
        } else {
            branch
                .into_iter()
                .filter(|message| wait.messages.contains(&message.id))
                .collect()
        };
        if unanswered.is_empty() {
            self.stop_waiting(&conv).await;
            return;
        }
        {
            let mut state = conv.state.lock().await;
            state.pending = unanswered;
            state.waiting = true;
        }
        let waiting = brigadier_router::Waiting {
            reason: wait.reason,
            resets_at_ms: wait.resets_at_ms,
            rule: wait.rule,
            ranking: wait.ranking,
        };
        self.keep_conversation_waiting(&conv, waiting).await;
    }

    /// Between turns: once the chosen model can run again (its provider usable and its own
    /// windows reset), or the user chose another model, the stand-in steps back and the next
    /// turn starts over on the chosen model from the transcript.
    async fn end_stand_in(&self, conv: &Arc<ConvLive>) {
        let Ok(conversation) = self.core.conversation(&conv.id) else {
            return;
        };
        let Some(fallback) = &conversation.fallback else {
            return;
        };
        let chosen = setup_choice(&conversation);
        let changed = chosen.is_some() && chosen.as_ref() != Some(&fallback.replaces);
        if !changed && !self.choice_available(&fallback.replaces).await {
            return;
        }
        let cli = {
            let mut state = conv.state.lock().await;
            if state.busy || state.closing {
                return;
            }
            state.closing = true;
            state.cli.take()
        };
        if let Some(cli) = cli {
            cli.session.close().await;
            cli.ended.cancelled().await;
        }
        if let Err(err) = self.core.set_fallback(&conv.id, None).await {
            tracing::warn!(conversation = %conv.id, error = %err, "could not end the stand-in model");
        }
        {
            let mut state = conv.state.lock().await;
            state.closing = false;
            state.fresh = true;
            state.reseed = true;
        }
        if !changed {
            self.notice(
                &conv.id,
                brigadier_providers::NoticeLevel::Info,
                &format!(
                    "{} is available again; continuing with it.",
                    fallback.replaces.provider.label()
                ),
            )
            .await;
        }
    }

    /// The native id of the conversation's last CLI session with `provider`, to resume it.
    pub(super) async fn last_native_id(
        &self,
        id: &ConversationId,
        provider: ProviderKind,
    ) -> Option<String> {
        let page = self
            .core
            .store()
            .read_stream(
                streams::orchestrator(id),
                StreamPage {
                    before: None,
                    kinds: vec!["orchestrator.logged".into()],
                    limit: 200,
                },
            )
            .await
            .ok()?;
        for stored in page {
            let Ok(DomainEvent::OrchestratorLogged {
                entry:
                    OrchestratorEntry::Provider {
                        provider: kind,
                        event,
                    },
                ..
            }) = serde_json::from_str::<DomainEvent>(stored.payload.get())
            else {
                continue;
            };
            match event {
                // A reset marker: the CLI files are gone (archive, cleanup).
                ProviderEvent::Notice { message, .. } if message == prompts::SESSION_RESET => {
                    return None;
                }
                ProviderEvent::SessionStarted { native_id, .. } if kind == provider => {
                    return Some(native_id);
                }
                _ => {}
            }
        }
        None
    }

    /// The notes the conversation's CLI session needs before its next turn: what changed in
    /// its instructions since it was last told (see [`prompts::notes`]).
    async fn instruction_notes(&self, conv: &Arc<ConvLive>) -> Vec<prompts::Note> {
        let current = match conv.kind {
            ConversationKind::Session => {
                let Ok(conversation) = self.core.conversation(&conv.id) else {
                    return Vec::new();
                };
                let run = self.run_setting(&conv.id).await;
                // The workspace its CLI was started for.
                let workspace = conv.live_cli().await.and_then(|cli| {
                    cli.launch
                        .as_ref()
                        .and_then(|launch| launch.workspace.as_ref())
                        .map(super::thread::ThreadWorkspace::told)
                });
                prompts::Current::session(
                    &conversation,
                    run.as_ref()
                        .map(|(workspace, restrictions)| (workspace, restrictions.clone())),
                    self.core.settings().short_replies,
                    self.memory_lines(super::brain_jobs::MEMORY_BYTES).await,
                    workspace,
                )
            }
            ConversationKind::Chat => {
                prompts::Current::chat(self.memory_lines(super::brain_jobs::MEMORY_BYTES).await)
            }
        };
        let known = conv.state.lock().await.told.clone();
        let told = match known {
            Some(told) => told,
            None => {
                let told = self.told_from_log(&conv.id).await;
                conv.state.lock().await.told = Some(told.clone());
                told
            }
        };
        prompts::notes(&told, &current)
    }

    /// The CLI took a turn carrying `notes`: they are logged, and what they told is now the
    /// session's. Never called for a turn that failed, so a restart sends them again.
    async fn told_notes(&self, conv: &Arc<ConvLive>, notes: Vec<prompts::Note>) {
        for note in notes {
            if let Some(told) = conv.state.lock().await.told.as_mut() {
                let mut now = note.told.clone();
                prompts::fill_told(&mut now, told);
                *told = now;
            }
            self.log_told(&conv.id, note.label, note.text.len(), note.told)
                .await;
        }
    }

    /// The conversation's own session saved a preference (`before`: the user's preferences
    /// until then): it knows, so its next turn needs no note about it, unless they had
    /// changed otherwise since it was last told. Kept in memory only: after a restart the
    /// note goes once.
    pub(crate) async fn told_own_preference(&self, id: &ConversationId, before: &[String]) {
        let Ok(conv) = self.conv(id) else {
            return;
        };
        let after = self.memory_lines(super::brain_jobs::MEMORY_BYTES).await;
        let mut state = conv.state.lock().await;
        if let Some(told) = state.told.as_mut()
            && told.preferences.as_deref()
                == Some(prompts::preferences_fingerprint(before).as_str())
        {
            told.preferences = Some(prompts::preferences_fingerprint(&after));
        }
    }

    /// What the conversation's current CLI session was last told, from the log: its role
    /// instructions and the notes since.
    async fn told_from_log(&self, id: &ConversationId) -> crate::work::Told {
        let mut entries = Vec::new();
        let mut before = None;
        loop {
            let Ok(page) = self
                .core
                .store()
                .read_stream(
                    streams::orchestrator(id),
                    StreamPage {
                        before,
                        kinds: vec!["orchestrator.logged".into()],
                        limit: 500,
                    },
                )
                .await
            else {
                break;
            };
            let Some(last) = page.last() else {
                break;
            };
            before = Some(last.stream_seq);
            let mut reached = false;
            for stored in &page {
                if let Ok(DomainEvent::OrchestratorLogged {
                    entry: OrchestratorEntry::Injection { injection },
                    ..
                }) = serde_json::from_str::<DomainEvent>(stored.payload.get())
                    && injection.kind == InjectionKind::Instructions
                {
                    reached |= injection.label.starts_with(prompts::ROLE_INSTRUCTIONS);
                    entries.push((stored.at_ms, injection));
                }
            }
            if reached {
                break;
            }
        }
        prompts::told_from_log(entries.iter().map(|(at, entry)| (*at, entry))).0
    }

    /// Marks the conversation's CLI session as gone for good: the next one starts over.
    pub(crate) async fn forget_native_session(&self, id: &ConversationId) {
        self.log_provider(
            id,
            ProviderKind::Claude,
            ProviderEvent::Notice {
                level: brigadier_providers::NoticeLevel::Info,
                message: prompts::SESSION_RESET.into(),
            },
        )
        .await;
    }

    async fn has_history(&self, id: &ConversationId) -> bool {
        self.core
            .list_messages(id.clone(), None, 2)
            .await
            .is_ok_and(|page| page.messages.len() > 1)
    }

    /// The transcript so far, for a CLI session that starts over: the last messages of the
    /// branch shown, verbatim (bounded), and the tasks.
    async fn reseed_text(&self, id: &ConversationId, carried: &[Message]) -> String {
        let branch = match self.core.head(id).await {
            Ok(Some(head)) => self.core.branch(id, &head).await.unwrap_or_default(),
            _ => Vec::new(),
        };
        let carried: Vec<&str> = carried.iter().map(|m| m.id.as_str()).collect();
        let mut lines = Vec::new();
        let mut bytes = 0;
        for message in branch.iter().rev().take(RESEED_MESSAGES) {
            if carried.contains(&message.id.as_str()) {
                continue;
            }
            let who = match message.role {
                MessageRole::User => "User",
                MessageRole::Assistant => "You",
                MessageRole::System => "Brigadier",
            };
            let words = self
                .core
                .brief_words(&message.text, &message.attachments)
                .await;
            let line = format!("{who}: {words}");
            bytes += line.len();
            if bytes > RESEED_BYTES {
                break;
            }
            lines.push(line);
        }
        lines.reverse();
        let mut text = String::new();
        if !lines.is_empty() {
            text.push_str(
                "[Brigadier: this conversation continues from an earlier session. The transcript so far]\n",
            );
            text.push_str(&lines.join("\n\n"));
        }
        if let Ok(tasks) = self.core.tasks(id).await
            && !tasks.is_empty()
        {
            text.push_str("\n\n[Tasks so far]\n");
            for task in tasks {
                text.push_str(&format!(
                    "task-{} ({:?}, {:?}): {}\n",
                    task.number, task.kind, task.state, task.title
                ));
            }
        }
        text
    }

    pub(crate) async fn set_run(
        &self,
        id: &ConversationId,
        state: RunState,
        error: Option<String>,
    ) {
        self.set_run_for(id, state, error, None).await;
    }

    /// Records the run state of a turn that serves `request`.
    async fn set_run_for(
        &self,
        id: &ConversationId,
        state: RunState,
        error: Option<String>,
        request: Option<String>,
    ) {
        if let Err(err) = self
            .core
            .record_conversation(
                id,
                vec![DomainEvent::RunStateChanged {
                    conversation_id: id.clone(),
                    state,
                    error,
                    request_id: request,
                }],
            )
            .await
        {
            tracing::debug!(conversation = %id, error = %err, "could not store the run state");
        }
    }

    pub(crate) async fn notice(
        &self,
        id: &ConversationId,
        level: brigadier_providers::NoticeLevel,
        text: &str,
    ) {
        let event = DomainEvent::ConversationNotice {
            conversation_id: id.clone(),
            notice: Notice {
                level,
                text: text.to_owned(),
                at_ms: now_ms(),
            },
        };
        if let Err(err) = self.core.record_conversation(id, vec![event]).await {
            tracing::debug!(conversation = %id, error = %err, "could not store a notice");
        }
    }

    async fn log_user_injection(&self, conv: &Arc<ConvLive>, message: &Message) {
        if conv.kind != ConversationKind::Session {
            return;
        }
        // Count all of what was sent: a long message keeps only its first part inline, and
        // pasted text is an attachment.
        let bytes = self.full_words(message).await.len();
        self.log_injection(
            &conv.id,
            InjectionKind::UserMessage,
            "user message".into(),
            None,
            bytes,
        )
        .await;
    }

    /// Logs what entered the orchestrator's context.
    pub(crate) async fn log_injection(
        &self,
        id: &ConversationId,
        kind: InjectionKind,
        label: String,
        task_id: Option<TaskId>,
        bytes: usize,
    ) {
        let entry = OrchestratorEntry::Injection {
            injection: ContextInjection {
                kind,
                bytes: bytes as u64,
                tokens_estimate: (bytes as u64).div_ceil(4),
                label,
                task_id,
                told: None,
            },
        };
        self.log_orchestrator(id, entry).await;
    }

    /// Logs instructions given to the conversation's CLI session, with what they told it.
    async fn log_told(
        &self,
        id: &ConversationId,
        label: &str,
        bytes: usize,
        told: crate::work::Told,
    ) {
        let entry = OrchestratorEntry::Injection {
            injection: ContextInjection {
                kind: InjectionKind::Instructions,
                bytes: bytes as u64,
                tokens_estimate: (bytes as u64).div_ceil(4),
                label: label.to_owned(),
                task_id: None,
                told: Some(told),
            },
        };
        self.log_orchestrator(id, entry).await;
    }

    pub(super) async fn log_provider(
        &self,
        id: &ConversationId,
        provider: ProviderKind,
        event: ProviderEvent,
    ) {
        self.log_orchestrator(id, OrchestratorEntry::Provider { provider, event })
            .await;
    }

    async fn log_orchestrator(&self, id: &ConversationId, entry: OrchestratorEntry) {
        let event = DomainEvent::OrchestratorLogged {
            conversation_id: id.clone(),
            entry,
        };
        if let Err(err) = self
            .core
            .record(vec![(streams::orchestrator(id), event)])
            .await
        {
            tracing::debug!(conversation = %id, error = %err, "could not log the orchestrator");
        }
    }
}

/// Holds back an orchestrator reply's streamed text while it may still be [`prompts::QUIET`],
/// so the user never sees it appear.
struct Quiet {
    enabled: bool,
    /// Text streamed so far per reply still held back; released replies are absent.
    held: HashMap<String, String>,
    released: HashSet<String>,
}

impl Quiet {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            held: HashMap::new(),
            released: HashSet::new(),
        }
    }

    /// The deltas the user may see: a reply's text is released, whole, once it can no longer
    /// be [`prompts::QUIET`] nor a short line of narration (see [`narration`]).
    fn pass(&mut self, deltas: Vec<ProviderEvent>) -> Vec<ProviderEvent> {
        if !self.enabled {
            return deltas;
        }
        deltas
            .into_iter()
            .filter_map(|event| match event {
                ProviderEvent::MessageDelta { item_id, text } => {
                    if self.released.contains(&item_id) {
                        return Some(ProviderEvent::MessageDelta { item_id, text });
                    }
                    let so_far = self.held.entry(item_id.clone()).or_default();
                    so_far.push_str(&text);
                    if prompts::QUIET.starts_with(so_far.trim()) || so_far.len() < NARRATION_BYTES {
                        return None;
                    }
                    let text = self.held.remove(&item_id).unwrap_or_default();
                    self.released.insert(item_id.clone());
                    Some(ProviderEvent::MessageDelta { item_id, text })
                }
                event => Some(event),
            })
            .collect()
    }

    /// Whether `event` is a complete reply none of which was shown yet that reads as narration
    /// ([`announces`]): it is held until the next event says whether it was. A question is
    /// never held.
    fn holds_whole(&mut self, event: &ProviderEvent) -> bool {
        let ProviderEvent::Message {
            item_id,
            role: ProviderRole::Assistant,
            text,
        } = event
        else {
            return false;
        };
        let whole = self.enabled
            && !self.released.contains(item_id)
            && text.len() < NARRATION_BYTES
            && !text.contains('?')
            && announces(text);
        if whole {
            self.forget(event);
        }
        whole
    }

    /// Forgets a reply once it is complete.
    fn forget(&mut self, event: &ProviderEvent) {
        if let ProviderEvent::Message { item_id, .. } = event {
            self.held.remove(item_id);
            self.released.remove(item_id);
        }
    }
}

/// What the thread is told when its reply asked the user something in text, with no card open.
const ASK_ON_A_CARD: &str = "[Your reply asked the user a question in text. Ask it on a card instead: propose_merge for the merge, ask_user for anything else (with options and the one you recommend). Then reply [quiet], and don't repeat the question in text.]";

/// Replies shorter than this are held back until the turn shows whether they were narration.
const NARRATION_BYTES: usize = 400;

/// How narration starts: the orchestrator saying it looks something up or hands work out
/// next. An opening needs one of [`LOOKUPS`] right after it ("I'll check …", "Let me ask a
/// scout …"); a bare "I'll use SQLite." is a decision, not narration.
const OPENINGS: &[&str] = &[
    "let me ",
    "let's ",
    "i'll ",
    "i will ",
    "i'm going to ",
    "i am going to ",
];

/// What narration announces.
const LOOKUPS: &[&str] = &[
    "check",
    "look",
    "ask",
    "hand",
    "delegate",
    "search",
    "read",
    "pull up",
    "dig",
    "find out",
    "see ",
    "verify",
    "confirm",
    "query",
    "get a scout",
    "have a scout",
    "send a scout",
    "spin up",
    "kick off",
];

/// Openings that are narration on their own ("Checking the report.", "On it.").
const PROGRESS: &[&str] = &[
    "checking ",
    "looking ",
    "asking ",
    "handing ",
    "delegating ",
    "searching ",
    "reading ",
    "pulling up ",
    "digging ",
    "querying ",
    "on it",
    "one moment",
];

/// Lead-ins before an announcement ("Now let me …").
const LEAD_INS: &[&str] = &["now ", "next, ", "first, ", "ok, ", "okay, ", "alright, "];

/// Words that give a reply more than an announcement: a reason, a choice, a limit.
const SUBSTANTIVE: &[&str] = &[
    "because", "since ", "instead", "decid", "chose", "choos", "won't", "will not", "can't",
    "cannot", "must", "should", "don't", "didn't", "not ",
];

/// How a sentence saying the Project Brain has no answer starts ("The Brain doesn't have
/// that."): the lookup's outcome, which the hand-off after it shows anyway.
const BRAIN_MISS: &[&str] = &[
    "the brain ",
    "the project brain ",
    "nothing in the brain",
    "nothing in the project brain",
];

/// Words that give a Brain-miss sentence more than the miss.
const REASONING: &[&str] = &[
    "because", "instead", "decid", "chose", "choos", "must", "should",
];

/// What a sentence opening with [`BRAIN_MISS`] must say for it to be a miss ("doesn't have
/// that", "has nothing on it"); "The Brain says we use SQLite." states a fact.
const MISSING: &[&str] = &[
    "doesn't", "does not", "didn't", "did not", "has no", "had no", "nothing", "no ", "not ",
    "isn't", "is not", "without",
];

/// What joins a second clause onto a sentence ("I'll check the Brain, then go with B."): an
/// announcement or a miss is one clause, so a sentence with one of these may carry more and is
/// shown.
const JOINS: &[&str] = &[
    ",", " then ", " and ", " but ", " so ", " or ", " also ", "—", "–", " - ", "(",
];

/// Whether a short reply only announces the next step ("Let me check the Brain.", "The Brain
/// doesn't have that. I'll ask a scout."): at most two sentences of one clause each, one
/// announcing a lookup or a hand-off, the other at most saying the Brain had no answer, with no
/// reason, choice or limit in them. Anything else (a decision, its reason, a fact, a second
/// clause, more sentences) is shown.
fn announces(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    let sentences: Vec<&str> = lower
        .split(['.', '!', ';', ':', '\n'])
        .map(str::trim)
        .filter(|sentence| !sentence.is_empty())
        .collect();
    let announcement = |sentence: &&str| {
        let mut rest: &str = sentence;
        for lead in LEAD_INS {
            rest = rest.strip_prefix(lead).unwrap_or(rest);
        }
        let opens = PROGRESS.iter().any(|opening| rest.starts_with(opening))
            || OPENINGS.iter().any(|opening| {
                rest.strip_prefix(opening)
                    .is_some_and(|after| LOOKUPS.iter().any(|verb| after.starts_with(verb)))
            });
        opens
            && !SUBSTANTIVE.iter().any(|word| sentence.contains(word))
            && !JOINS.iter().any(|join| sentence.contains(join))
    };
    let brain_miss = |sentence: &&str| {
        BRAIN_MISS
            .iter()
            .any(|opening| sentence.starts_with(opening))
            && (sentence.starts_with("nothing ")
                || MISSING.iter().any(|word| sentence.contains(word)))
            && !REASONING.iter().any(|word| sentence.contains(word))
            && !JOINS.iter().any(|join| sentence.contains(join))
    };
    sentences.len() <= 2
        && sentences.iter().any(announcement)
        && sentences
            .iter()
            .all(|sentence| announcement(sentence) || brain_miss(sentence))
}

/// Tools whose call a short reply before it may only have announced ("I'll ask a scout.",
/// "Let me read the report."): the user sees the call's result anyway.
const ANNOUNCED_TOOLS: &[&str] = &[
    "delegate_task",
    "read_report",
    "read_artifact",
    "query_brain",
    "search_transcript",
    "list_tasks",
    "code_search",
    "code_refs",
    "project_map",
];

/// A held reply, kept out of the thread for its request.
struct Narration {
    item_id: String,
    text: String,
    model: ModelChoice,
}

/// What `event`, following a held short reply in the same turn, makes of it: `Some(true)` when
/// it is a call that only looks something up or hands work out ([`ANNOUNCED_TOOLS`]), which
/// the reply announced; `Some(false)` when the reply stands: the turn ended, another reply
/// followed, or the call acts on the user's behalf (accepting or rejecting work, a plan, a
/// card, an answer to a worker), so the reply may carry a decision or its reason. `None` when
/// it cannot tell yet.
fn narration(event: &ProviderEvent) -> Option<bool> {
    match event {
        ProviderEvent::ToolCall { name, .. } => {
            let tool = name.rsplit("__").next().unwrap_or(name);
            let tool = tool.rsplit('.').next().unwrap_or(tool);
            Some(ANNOUNCED_TOOLS.contains(&tool))
        }
        ProviderEvent::TurnCompleted { .. }
        | ProviderEvent::Exited { .. }
        | ProviderEvent::Message { .. }
        | ProviderEvent::Error { .. } => Some(false),
        _ => None,
    }
}

/// An orchestrator reply as the user sees it: without a trailing [`prompts::QUIET`], and
/// nothing when that was all of it.
fn without_quiet(text: &str) -> Option<&str> {
    let text = text.trim_end();
    let text = text.strip_suffix(prompts::QUIET).unwrap_or(text).trim_end();
    (!text.trim_start().is_empty()).then_some(text)
}

/// Whether an orchestrator's reply ends by asking the user something: its last sentence (or
/// the one right before a closing list of options) is a question. A question followed by more
/// statements ("Why did it fail? A missing export.") asks nothing, and neither does a question
/// mark in code, in a quotation or inside a word (a URL's query). A question asked with
/// ask_user is a card, which the request waits on by itself; this reads the reply's text.
fn asks_user(reply: &str) -> bool {
    let Some(reply) = without_quiet(reply) else {
        return false;
    };
    let option = |line: &str| {
        let line = line.trim_start();
        line.strip_prefix(['-', '*', '\u{2022}'])
            .is_some_and(|rest| rest.starts_with(char::is_whitespace))
            || line.split_once(['.', ')']).is_some_and(|(number, _)| {
                !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
            })
    };
    let prose = prose_of(reply);
    let all: Vec<&str> = prose
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    // A closing list of options: the question is the sentence before it (a reply that is
    // only a list ends on its last item).
    let mut lines: Vec<&str> = prose.lines().collect();
    while lines
        .last()
        .is_some_and(|line| line.trim().is_empty() || option(line))
    {
        lines.pop();
    }
    if lines.is_empty() {
        return all.last().is_some_and(|line| ends_on_question(line));
    }
    let paragraph = lines
        .rsplit(|line| line.trim().is_empty())
        .next()
        .unwrap_or_default()
        .join("\n");
    ends_on_question(paragraph.trim_end())
}

/// A reply's prose: without code blocks and quoted lines ("> …"), each code span read as a
/// word.
fn prose_of(reply: &str) -> String {
    let mut prose = String::new();
    let mut fenced = false;
    for line in reply.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced || trimmed.starts_with('>') {
            continue;
        }
        let mut spans = line.split('`');
        prose.push_str(spans.next().unwrap_or_default());
        // Odd pieces are code; an unclosed span runs to the line's end.
        for (n, piece) in spans.enumerate() {
            if n % 2 == 1 {
                prose.push_str(piece);
            } else {
                prose.push_str("code");
            }
        }
        prose.push('\n');
    }
    prose
}

/// Whether `text` ends on a question of its own: a `?` after its last word, past closing
/// marks and a closing aside ("Land it? (It fails `npm test`.)"), outside quotation marks.
fn ends_on_question(text: &str) -> bool {
    const CLOSING: [char; 7] = [')', '"', '\'', '*', '_', '\u{201d}', '\u{2019}'];
    let text = text.trim_end();
    let bare = text.trim_end_matches(|c: char| CLOSING.contains(&c) || c.is_whitespace());
    if let Some(before) = bare.strip_suffix('?') {
        // Inside a quotation that closes after it: someone else's question.
        let straight = before.matches('"').count();
        let curly = before.matches('\u{201c}').count() > before.matches('\u{201d}').count();
        return straight % 2 == 0 && !curly;
    }
    // A closing aside after the question.
    if text.ends_with(')')
        && let Some(open) = text.rfind('(')
    {
        return ends_on_question(&text[..open]);
    }
    false
}

/// Takes every envelope of one request from the inbox, in arrival order: the request of the
/// first worker question (it blocks a worker), else of the oldest envelope.
fn take_one_request(
    inbox: &mut Vec<(Envelope, Option<String>)>,
) -> Vec<(Envelope, Option<String>)> {
    let Some(request) = inbox
        .iter()
        .find(|(envelope, _)| envelope.kind == InjectionKind::WorkerQuestion)
        .or_else(|| inbox.first())
        .map(|(_, request)| request.clone())
    else {
        return Vec::new();
    };
    let (taken, kept) = std::mem::take(inbox)
        .into_iter()
        .partition(|(_, of)| *of == request);
    *inbox = kept;
    taken
}

/// Text the user pasted, as it goes into their message: whole up to
/// [`INLINE_TEXT_MAX_BYTES`], else its start and end around a note of what is left out. In a
/// session the note (or one after the whole text) names the attachment, so the orchestrator
/// can give a worker all of it.
fn pasted_inline(text: &str, attachment: &AttachmentRef, kind: ConversationKind) -> String {
    let session = kind == ConversationKind::Session;
    if text.len() <= INLINE_TEXT_MAX_BYTES {
        if !session {
            return text.to_owned();
        }
        return format!(
            "{text}\n[the pasted text above is also attachment {}: pass its id to delegate_task \
             to give a worker the exact text]",
            attachment.id
        );
    }
    let head = &text[..text.floor_char_boundary(PASTE_HEAD_BYTES)];
    let tail = &text[text.ceil_char_boundary(text.len() - PASTE_TAIL_BYTES)..];
    let left_out = text.len() - head.len() - tail.len();
    let whole = if session {
        format!(
            "; all of it is attachment {}: pass its id to delegate_task so a worker can read it",
            attachment.id
        )
    } else {
        String::new()
    };
    format!(
        "{head}\n[… {left_out} bytes of the pasted text are left out here, since it is longer \
         than {} kB{whole}]\n{tail}",
        INLINE_TEXT_MAX_BYTES / 1_000
    )
}

/// Image placement and attachment work for one message. Copy candidates exclude cached ids.
struct ImagePlan<'a> {
    rows: Vec<&'a AttachmentRef>,
    copies: Vec<&'a AttachmentRef>,
}

fn image_plan<'a>(
    text: &str,
    attachments: &'a [AttachmentRef],
    copied: &HashMap<String, Option<InputFile>>,
) -> ImagePlan<'a> {
    let tokens = inline_image_tokens(text, attachments);
    let used: HashSet<_> = tokens
        .iter()
        .map(|(_, attachment)| attachment.id.as_str())
        .collect();
    let rows = attachments
        .iter()
        .filter(|a| !(a.inline.is_some() && is_image(&a.mime) && used.contains(a.id.as_str())))
        .collect();
    let mut seen: HashSet<_> = copied.keys().map(String::as_str).collect();
    let copies = attachments
        .iter()
        .filter(|a| is_image(&a.mime) && seen.insert(a.id.as_str()))
        .collect();
    ImagePlan { rows, copies }
}

/// The AB input: one image file per inline reference, named at every pasted position.
fn name_inline_images(
    user_text: &str,
    attachments: &[AttachmentRef],
    copied: &HashMap<String, Option<InputFile>>,
    kind: ConversationKind,
    files: &mut Vec<InputFile>,
) -> String {
    let mut text = user_text.to_owned();
    let mut image_numbers = HashMap::new();
    for attachment in attachments {
        let sent = copied
            .get(&attachment.id)
            .and_then(Option::as_ref)
            .map(|file| {
                files.push(InputFile {
                    path: file.path.clone(),
                    name: attachment.name.clone(),
                    mime: attachment.mime.clone(),
                });
                files.len()
            });
        let Some(number) = attachment.inline else {
            continue;
        };
        let n = if number > 0 {
            number
        } else {
            let next = image_numbers.len() as u32 + 1;
            *image_numbers.entry(attachment.id.clone()).or_insert(next)
        };
        let marker = if attachment.inline == Some(0) {
            format!("[image:{}]", attachment.id)
        } else {
            format!("[Image #{n}]")
        };
        place_inline_image(
            &mut text,
            &marker,
            n,
            &inline_image_note(sent, attachment, kind),
        );
    }
    text
}

/// What stands for an image pasted into the text: which of the turn's images it is (`sent`,
/// counting from 1), or that it could not be sent. In a session it names the attachment, so
/// the orchestrator can give it to a worker.
fn inline_image_note(
    sent: Option<usize>,
    attachment: &AttachmentRef,
    kind: ConversationKind,
) -> String {
    let what = match sent {
        Some(index) => format!("image {index} of the images sent with this message"),
        None => format!(
            "\"{}\", which could not be sent as an image",
            attachment.name
        ),
    };
    if kind == ConversationKind::Session {
        format!(
            "{what}; attachment {} ({}, {} bytes): pass its id to delegate_task so a worker \
             can see it",
            attachment.id, attachment.mime, attachment.bytes
        )
    } else {
        what
    }
}

/// Puts image `n` where the user pasted it: its first `[Image #n]` in `text` says it was
/// pasted there and which image it is (`note`); any later one (the same image pasted again)
/// says it is that image again, not another. Without one (edited out), the note goes at the
/// end.
fn place_inline_image(text: &mut String, marker: &str, n: u32, note: &str) {
    if text.contains(marker) {
        *text = text
            .replacen(marker, &format!("[Image #{n}, pasted here: {note}]"), 1)
            .replace(
                marker,
                &format!("[Image #{n} again: the same image as above, not another one]"),
            );
    } else {
        push_block(
            text,
            &format!("[Image #{n}, pasted with this message: {note}]"),
        );
    }
}

fn append_input_text(parts: &mut Vec<InputPart>, text: &str) {
    if let Some(InputPart::Text(last)) = parts.last_mut() {
        last.push_str(text);
    } else {
        parts.push(InputPart::Text(text.into()));
    }
}

/// Preserve push_block's replacement of whitespace-only user text, without byte slicing.
fn push_input_block(parts: &mut Vec<InputPart>, block: &str) {
    if parts
        .iter()
        .all(|part| matches!(part, InputPart::Text(text) if text.trim().is_empty()))
    {
        parts.clear();
        parts.push(InputPart::Text(block.into()));
    } else {
        append_input_text(parts, &format!("\n\n{block}"));
    }
}

fn is_image(mime: &str) -> bool {
    brigadier_providers::model::is_image_mime(mime)
}

/// A file name that is safe inside a folder.
pub(crate) fn safe_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_start_matches('.');
    if trimmed.is_empty() {
        "file".into()
    } else {
        trimmed.chars().take(80).collect()
    }
}

/// A Chat's web search or page read, as a row for its answer.
fn web_step(tool: &str, input: Option<&str>) -> Option<OrchestratorStepKind> {
    let field = |key: &str| {
        input
            .and_then(|input| serde_json::from_str::<serde_json::Value>(input).ok())
            .and_then(|args| {
                args.get(key)
                    .and_then(|value| value.as_str().map(str::to_owned))
            })
    };
    match tool {
        "WebSearch" => field("query").map(|query| OrchestratorStepKind::SearchedWeb { query }),
        // Codex gives the query itself.
        "web_search" => input.map(|query| OrchestratorStepKind::SearchedWeb {
            query: query.to_owned(),
        }),
        "WebFetch" => field("url").map(|url| OrchestratorStepKind::ReadPage { url }),
        _ => None,
    }
}

/// What a conversation whose model hit `limit` waits for, when no model can stand in: what
/// the router named, freed by that model's own reset as well when that comes sooner; or that
/// reset alone. Nothing when neither is known (the turn fails as before).
fn with_own_reset(
    waiting: Option<brigadier_router::Waiting>,
    from: &ModelChoice,
    limit: &LimitHit,
) -> Option<brigadier_router::Waiting> {
    match waiting {
        Some(mut waiting) => {
            waiting.resets_at_ms = match (waiting.resets_at_ms, limit.resets_at_ms) {
                (Some(named), Some(own)) => Some(named.min(own)),
                (named, own) => named.or(own),
            };
            Some(waiting)
        }
        None => limit
            .resets_at_ms
            .map(|resets_at_ms| brigadier_router::Waiting {
                reason: format!("{} hit its usage limit", from.provider.label()),
                resets_at_ms: Some(resets_at_ms),
                rule: None,
                ranking: None,
            }),
    }
}

/// The model a conversation's setup picks: a session's orchestrator, a Chat's model.
fn setup_choice(conversation: &crate::model::Conversation) -> Option<ModelChoice> {
    match &conversation.setup {
        Some(Setup::Session { orchestrator, .. }) => Some(orchestrator.clone()),
        Some(Setup::Chat { model }) => Some(model.clone()),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_ref(id: &str, inline: bool, mime: &str) -> AttachmentRef {
        AttachmentRef {
            id: id.into(),
            name: format!("{id}.png"),
            mime: mime.into(),
            bytes: 3,
            pasted: false,
            inline: inline.then_some(0),
        }
    }

    fn image_files(attachments: &[AttachmentRef]) -> HashMap<String, Option<InputFile>> {
        attachments
            .iter()
            .map(|a| {
                (
                    a.id.clone(),
                    Some(InputFile {
                        path: std::path::PathBuf::from(format!("/attachments/{}", a.id)),
                        name: a.name.clone(),
                        mime: a.mime.clone(),
                    }),
                )
            })
            .collect()
    }

    #[test]
    fn ab_and_legacy_messages_send_one_image_for_many_places() {
        for (text, number) in [
            ("before [Image #1] again [Image #1]", 1),
            ("before [image:a] again [image:a]", 0),
        ] {
            let mut attachment = image_ref("a", true, "image/png");
            attachment.inline = Some(number);
            let attachments = vec![attachment];
            let copied = image_files(&attachments);
            let mut sent = Vec::new();
            let words = name_inline_images(
                text,
                &attachments,
                &copied,
                ConversationKind::Chat,
                &mut sent,
            );
            assert_eq!(sent.len(), 1);
            assert_eq!(
                words,
                "before [Image #1, pasted here: image 1 of the images sent with this message] again [Image #1 again: the same image as above, not another one]"
            );
        }
    }

    #[test]
    fn inline_notes_count_row_images_in_attachment_order() {
        let row = image_ref("row", false, "image/png");
        let mut inline = image_ref("pasted", true, "image/png");
        inline.inline = Some(1);
        let attachments = vec![row, inline];
        let mut files = Vec::new();
        let text = name_inline_images(
            "[Image #1]",
            &attachments,
            &image_files(&attachments),
            ConversationKind::Chat,
            &mut files,
        );
        assert_eq!(
            files
                .iter()
                .map(|file| file.name.as_str())
                .collect::<Vec<_>>(),
            vec!["row.png", "pasted.png"]
        );
        assert_eq!(
            text,
            "[Image #1, pasted here: image 2 of the images sent with this message]"
        );
    }

    #[test]
    fn images_are_named_at_each_place_but_sent_once() {
        let attachment = image_ref("a", true, "image/png");
        let mut text = "before [Image #1] between [Image #1] after".to_owned();
        place_inline_image(
            &mut text,
            "[Image #1]",
            1,
            &inline_image_note(Some(1), &attachment, ConversationKind::Chat),
        );
        assert_eq!(
            text,
            "before [Image #1, pasted here: image 1 of the images sent with this message] between [Image #1 again: the same image as above, not another one] after"
        );
        assert!(
            inline_image_note(Some(2), &attachment, ConversationKind::Session)
                .contains("attachment a")
        );
        assert!(
            inline_image_note(Some(2), &attachment, ConversationKind::Session)
                .contains("delegate_task")
        );
    }

    #[test]
    fn old_tokens_and_missing_images_keep_their_positions_and_notes() {
        let attachment = image_ref("a", true, "image/png");
        let mut text = "é [image:a] [image:unknown] [image:a]".to_owned();
        place_inline_image(
            &mut text,
            "[image:a]",
            1,
            &inline_image_note(None, &attachment, ConversationKind::Chat),
        );
        assert!(text.starts_with(
            "é [Image #1, pasted here: \"a.png\", which could not be sent as an image]"
        ));
        assert!(text.contains("[image:unknown]"));
        assert!(text.ends_with("[Image #1 again: the same image as above, not another one]"));
        let mut text = "Look at this".to_owned();
        place_inline_image(
            &mut text,
            "[Image #3]",
            3,
            "image 1 of the images sent with this message",
        );
        assert_eq!(
            text,
            "Look at this\n\n[Image #3, pasted with this message: image 1 of the images sent with this message]"
        );
    }

    #[test]
    fn missing_tokens_fall_back_and_rows_with_the_same_id_remain_attachments() {
        let attachments = vec![
            image_ref("a", false, "image/png"),
            image_ref("a", true, "image/png"),
            image_ref("missing", true, "image/png"),
        ];
        let files = image_files(&attachments);
        let plan = image_plan("[image:a] again [image:a]", &attachments, &HashMap::new());
        assert_eq!(plan.rows, vec![&attachments[0], &attachments[2]]);
        assert_eq!(plan.copies, vec![&attachments[0], &attachments[2]]);
        assert!(
            image_plan("no token", &attachments, &files)
                .copies
                .is_empty()
        );
    }

    #[test]
    fn whitespace_only_text_with_a_pasted_attachment_preserves_the_whole_paste() {
        let attachment = image_ref("text", false, "text/plain");
        for text in [" \n", "                    ", "\u{2003}"] {
            let mut parts = vec![InputPart::Text(text.into())];
            push_input_block(
                &mut parts,
                &pasted_inline("é", &attachment, ConversationKind::Chat),
            );
            assert_eq!(parts, vec![InputPart::Text("é".into())]);
        }
    }

    #[test]
    fn a_reply_that_ends_on_a_question_asks_the_user() {
        for reply in [
            "There's a conflicting test.\n\nHow do you want to resolve it: keep isPrime(1) true, or change the convention?",
            "Stuck on a conflict.\n\nHow do you want to resolve it:\n1. Keep isPrime(1) true, or\n2. Change the convention.\n\nWhich one, or something else?",
            "Two ways on. Which do you prefer?\n\n- Keep it\n- Drop it",
            "Should I land it anyway? (It fails `npm test`.)",
            "Land it as it is?\n\n[quiet]",
            "Do you want \"strict\" mode?\"",
            "Two ways on:\n- keep it\n- drop it\n\nWhich one?",
            "Which one do you want?\n1. Keep it\n2. Drop it",
            "Should I land it (as it is?)",
            "**Land it as it is?**",
            "Should I run this?\n\n```sh\nnpm test\n```",
        ] {
            assert!(asks_user(reply), "{reply}");
        }
        for reply in [
            "[quiet]",
            "Landed as commit afd589834a on main.",
            "Why did it fail? A missing export.\n\nI sent the worker back to fix it.",
            "Why did verification fail? The worker omitted an export.",
            "See https://example.com/docs?page=2 for the details.",
            "The worker asked \"should this be public?\"",
            "The worker asked \u{201c}should this be public?\u{201d}",
            "It runs `git status --porcelain?`",
            "Its test reads:\n\n```js\nexpect(isPrime(1)).toBe(false) // why?\n```",
            "The worker wrote:\n> Should I keep the old flag?",
            "Done.\n\n- tests pass\n- lint passes",
            "",
        ] {
            assert!(!asks_user(reply), "{reply}");
        }
    }
}
