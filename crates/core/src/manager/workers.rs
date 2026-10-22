//! The worker runtime: one CLI session per task.
//!
//! A task goes in; Brigadier routes it to a model (`crates/router`: by capability, quota and
//! outcomes, where the orchestrator's vendor is not an input), gives it a workspace, and starts
//! the worker:
//!
//! - **Workspace.** Write tasks get a worktree on a new task branch, read tasks a detached
//!   worktree, research tasks none. Worktrees start from the session branch's latest commit
//!   (new-worktree mode) or the picked branch's (local checkout). In a dirty local checkout
//!   the user is asked once whether workers see the uncommitted changes; if so, workers start
//!   from a snapshot commit of them, and only the worker's own changes ever land.
//! - **Scratch folder** outside the repository, which is also the worker's TMPDIR. Everything
//!   is recorded in the cleanup ledger under `task:<id>` before it is created, including any
//!   process running inside those folders.
//! - **Permissions**: the worker runs at its session's level, passed straight to its CLI as
//!   its own permission mode (G5). What its CLI asks reaches the user as a card, once per kind
//!   of command with "Allow … for this session".
//! - **Instructions**: the role and task prompt, plus the repository's `CLAUDE.md` and
//!   `AGENTS.md` whichever vendor runs it ([`super::instructions`]).
//! - **Secrets**: the project's gitignored env files are copied in, and their values are
//!   redacted from everything the worker produces.
//!
//! The worker streams its events to `task:<id>` (never to the orchestrator), may block on
//! `ask_orchestrator`, and ends with `submit_report`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use brigadier_git::{Oid, PatchOutcome, WorktreeSpec};
use brigadier_providers::policy;
use brigadier_providers::{
    AllowedModels, ApprovalDecision, ApprovalKind, ApprovalRequest, Artifact, Decider, InputFile,
    Origin, ProviderEvent, ProviderKind, SessionSpec, Started, ToolSet, TurnInput, TurnStatus,
};
use brigadier_router::QualityTier;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::brains::ReportLearning;
use super::conversation::{Cli, Envelope, safe_file_name};
use super::outputs::outputs_dir;
use super::refcheck::engine_rewrite;
use super::usage::TokenOwner;
use super::watchdog::WorkerWatch;
use super::worker_handoff::Handover;
use super::{
    EventSource, SessionManager, blocking, fallback, git_error, instructions, prompts, secrets,
    warm,
};
use crate::model::{
    Conversation, ConversationId, ConversationKind, DomainEvent, Environment, ModelChoice,
    PermissionLevel, Setup, streams,
};
use crate::routing::{TokenMeter, WriteCause};
use crate::runtime::{is_delta, merge_delta};
use crate::sessions::McpUpdate;
use crate::tools::Role;
use crate::work::{
    ArtifactKind, ArtifactRef, AttachmentRef, Attempt, AttemptEnd, GateLink, GateOwner,
    InjectionKind, OrchestratorStepKind, QuestionKind, QuotaWait, RepoAccess, Report, Route, Task,
    TaskId, TaskKind, TaskState, TaskWorkspace, WaitingSource, WorkerAccess, WorkerRole,
};
use crate::{Error, Result, now_ms};

/// Text deltas arriving within this window are stored as one event.
const DELTA_WINDOW: Duration = Duration::from_millis(30);
/// The CLIs' own limit on a worker's MCP calls: as long as `ask_orchestrator` waits for its
/// answer ([`QUESTION_TIMEOUT`]); no other worker tool waits that long.
pub(crate) const WORKER_TOOL_TIMEOUT_SECS: u64 = QUESTION_TIMEOUT.as_secs();
/// How long a fix Brigadier lands waits for the worker's turn that reported it to end.
const TURN_END_WAIT: Duration = Duration::from_secs(30);
/// How long `ask_orchestrator` waits for the orchestrator's answer.
pub(crate) const QUESTION_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// How a task waiting on its question to the thread says so; the question follows.
pub(crate) const ASKED_THE_THREAD: &str = "Asked the thread: ";
/// How it said so before #66, as a task recorded then still says.
pub(crate) const ASKED_THE_ORCHESTRATOR: &str = "Asked the orchestrator: ";
/// How long a worker waiting for its own background jobs may stay silent before its wait
/// counts as a turn without a report. A background sub-agent shows its steps; a background
/// command says nothing until it ends.
#[cfg(not(test))]
const BACKGROUND_SILENCE: Duration = Duration::from_secs(30 * 60);
#[cfg(test)]
const BACKGROUND_SILENCE: Duration = Duration::from_secs(2);
/// How often that wait is checked.
#[cfg(not(test))]
const BACKGROUND_CHECK: Duration = Duration::from_secs(60);
#[cfg(test)]
const BACKGROUND_CHECK: Duration = Duration::from_millis(200);
/// What of a report the thread reads inline, about 800 tokens: a longer one is taken whole
/// and reaches it shortened ([`prompts::report_envelope`]).
pub(crate) const REPORT_MAX_BYTES: usize = 3_600;
/// A worker's message at least this long, left out of its report, is kept as an artifact.
const KEEP_MESSAGE_MIN_BYTES: usize = 400;

/// What a task's workspace is made of, once prepared.
#[derive(Debug, Clone)]
pub(crate) struct Workspace {
    pub repo: PathBuf,
    /// The task's worktree, if it has one.
    pub worktree: Option<PathBuf>,
    pub branch: Option<String>,
    /// The commit the worker started from (a snapshot commit when it saw uncommitted changes).
    pub base: Option<Oid>,
    /// `base` is a snapshot of the user's uncommitted changes.
    pub on_snapshot: bool,
    /// The branch accepted work lands on.
    pub target: Option<String>,
    pub scratch: PathBuf,
    /// Folders copied in from the user's checkout (dependency installs, build caches).
    pub warmed: Vec<String>,
    /// A merge task: what it merges into its target.
    pub merge: Option<crate::work::MergeInputs>,
}

/// A worker's CLI session as [`SessionManager::worker_session`] makes it.
pub(crate) struct WorkerSession {
    pub spec: SessionSpec,
    /// Where its CLI runs.
    pub cwd: PathBuf,
    pub outputs: PathBuf,
    pub redactor: Option<Arc<brigadier_providers::redact::Redactor>>,
    pub allowed_models: Option<AllowedModels>,
    /// The task's worktree, if it has one.
    pub worktree: Option<PathBuf>,
    /// The grant its Brigadier tools run under.
    pub grant: String,
}

#[derive(Default)]
struct TaskLiveState {
    cli: Option<Arc<Cli>>,
    /// The worker waits on `ask_orchestrator`: what it asked, and where the answer goes.
    question: Option<(String, oneshot::Sender<String>)>,
    /// A turn is running.
    busy: bool,
    /// Asked once to submit its report after a turn ended without one.
    nudged: bool,
    /// The current turn's last assistant message: the orchestrator never sees it.
    last_message: Option<String>,
    /// A message the worker wrote instead of reporting it, kept for its report.
    unsent: Option<ArtifactRef>,
    /// Its outputs folder.
    outputs: Option<PathBuf>,
    stopping: bool,
    /// Where the worker's CLI runs (a command without its own cwd runs here).
    cwd: Option<PathBuf>,
    /// The task's worktree, whose file changes the user may allow for the session.
    worktree: Option<PathBuf>,
    redactor: Option<Arc<brigadier_providers::redact::Redactor>>,
    /// Why the running model must stop and hand the task on, once its turn is over (a usage
    /// limit, or an error another model may not have).
    cutoff: Option<AttemptEnd>,
    /// Transient provider errors (overloaded, server, network) in a row: the first is retried
    /// on the same model, the second hands the task on.
    transient: u32,
    /// The worker's latest context size, in tokens.
    context: Option<i64>,
    /// Its model's context window, in tokens, when the CLI said.
    window: Option<i64>,
    /// The CLI session's first context size, in tokens (a fresh session starts with its
    /// hand-off).
    session_start: Option<i64>,
    /// Counts the CLI sessions started for the task, so a hand-over decided for one session
    /// never closes the next.
    generation: u64,
    /// The worker was asked to end this turn with a handoff note (PLAN.md §7): a fresh session
    /// takes over when the turn ends.
    handoff_asked: bool,
    /// The context it was last asked at, this turn: the ask is repeated after each further
    /// stretch of growth (BUILD-PLAN D2.3).
    steered_at: Option<i64>,
    /// Its turn was stopped at the hard line, or its compaction refused, for a hand-off.
    forced: bool,
    /// A fresh session's first message, held while the task is paused: resuming starts it.
    held_handover: Option<TurnInput>,
    /// When the worker last showed it works (an event from its CLI, or a turn started); the
    /// stall watchdog measures silence from it.
    last_event_ms: i64,
    progress: super::watchdog::progress::Progress,
    /// The commands and tool calls the worker started that have not finished.
    running_items: std::collections::HashSet<String>,
    /// Its CLI's own background jobs still running (`ProviderEvent::BackgroundJobs`): each
    /// one's end starts a turn of its own.
    background_jobs: u32,
    /// It waited for its background jobs past [`BACKGROUND_SILENCE`]: until they are gone, a
    /// turn that ends without a report counts as one, jobs or not.
    background_overdue: bool,
    /// Its turn ended while it waits for its background jobs: their end starts its next turn,
    /// so it keeps its place in an overnight run meanwhile.
    background_waiting: bool,
    /// When the watchdog reported silence from this CLI session.
    stall_reported_at_ms: Option<i64>,
    /// Fresh sessions the watchdog started for stalls in the current attempt.
    stalls: u32,
    /// Since when the task is live without a running CLI, as the watchdog saw it.
    orphaned_at_ms: Option<i64>,
    /// The models the CLI session's sub-agents were held to when it started (PLAN.md §7).
    allowed_models: Option<AllowedModels>,
    /// Its session is (being) opened in the user's terminal: no headless CLI may register.
    taken_over: bool,
    /// The orchestrator's messages while it was open in the terminal, oldest first.
    held_messages: Vec<String>,
    /// The terminal its session is open in: what its tool calls run with.
    terminal: Option<ToolAccess>,
    /// Tool calls and commands it finished that its task's Brain counters don't hold yet.
    brain_calls: u32,
}

/// What a worker's tool calls that run commands (its previews) run with: its headless CLI's,
/// or while it is open in the user's terminal, the terminal's.
#[derive(Clone)]
pub(crate) struct ToolAccess {
    /// Cancelled when the CLI or the terminal ends: its commands end with it.
    pub ended: CancellationToken,
}

impl TaskLiveState {
    /// A turn starts: the worker works, and its silence is measured from now.
    fn begin_turn(&mut self) {
        self.busy = true;
        self.last_event_ms = now_ms();
        self.stall_reported_at_ms = None;
        self.progress = super::watchdog::progress::Progress::default();
    }

    /// Progress accounting takes explicit time so transport chatter can be fault-injected.
    fn note_progress(&mut self, event: &ProviderEvent, now: i64) {
        if matches!(event, ProviderEvent::TurnStarted { .. }) && !self.busy {
            self.progress = super::watchdog::progress::Progress::default();
        }
        if self.progress.observe(event, now) {
            self.last_event_ms = now;
            self.stall_reported_at_ms = None;
        }
    }

    /// Whether the CLI session started with looser model limits than `now` (a rule took a
    /// model away since): it must start again before its next turn, or its sub-agents keep
    /// the old ones. Limits that only widened leave the session as it is.
    fn model_rules_changed(&self, now: &Option<AllowedModels>) -> bool {
        match now {
            Some(now) => self.models_changed(now),
            None => self.allowed_models.is_some(),
        }
    }
    fn models_changed(&self, now: &AllowedModels) -> bool {
        let Some(then) = &self.allowed_models else {
            return true;
        };
        then.ids.iter().any(|id| !now.ids.contains(id))
            || now.outside.iter().any(|id| !then.outside.contains(id))
    }
}

/// A task's live worker.
pub(crate) struct TaskLive {
    pub id: TaskId,
    pub conversation_id: ConversationId,
    state: tokio::sync::Mutex<TaskLiveState>,
    /// Held while the worker's report is recorded and while it is stopped, so a stop and a
    /// report never both take effect.
    pub(crate) settle: tokio::sync::Mutex<()>,
    /// Held while the task is routed again and its next model started, so a quota timer and
    /// a Resume (or two hand-offs) never start two models on it.
    pub(crate) reroute: tokio::sync::Mutex<()>,
    /// Counts the quota-wait timers set for the task; only the newest one retries.
    wait_timer: std::sync::atomic::AtomicU64,
    /// A stall watchdog action on the worker is under way: the next rounds leave it be.
    pub(crate) watchdog_busy: std::sync::atomic::AtomicBool,
    /// Versions the task's reports before their Brain writes are spawned, so late findings
    /// survive an older report's write running afterward.
    pub(crate) learning: Arc<ReportLearning>,
    /// Held while the end of a worker's turn is handled (what it wrote after its report among
    /// it): see [`TaskLive::turn_over`].
    turn_end: tokio::sync::Mutex<()>,
    /// Held while the task is opened in a terminal, handed back or its terminal ended; the
    /// terminal it is open in ([`super::takeover`]).
    pub(crate) takeover: tokio::sync::Mutex<Option<super::takeover::LiveTakeover>>,
    /// Why its next CLI starts, when that isn't a plain start or resume (BUILD-PLAN B0).
    next_cause: std::sync::Mutex<Option<WriteCause>>,
    /// A plain worker of `brigadier start` ([`Task::plain`]): its CLI's events are passed on to
    /// its session's `brigadier wait`, and nothing of Brigadier's own runs it.
    pub(crate) plain: bool,
}

impl TaskLive {
    /// Records why its next CLI starts (a hand-off, a reroute).
    pub(crate) fn next_start(&self, cause: WriteCause) {
        *self
            .next_cause
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(cause);
    }

    fn take_next_cause(&self) -> Option<WriteCause> {
        self.next_cause
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    /// Records why the running model must hand the task on when its turn ends.
    pub(crate) async fn set_cutoff(&self, end: AttemptEnd) {
        self.state.lock().await.cutoff = Some(end);
    }

    /// Takes the pending hand-off, if any.
    pub(crate) async fn take_cutoff(&self) -> Option<AttemptEnd> {
        self.state.lock().await.cutoff.take()
    }

    /// What its CLI last said.
    pub(crate) async fn last_message(&self) -> Option<String> {
        self.state.lock().await.last_message.clone()
    }

    /// Counts a transient provider error; answers how many came in a row.
    pub(crate) async fn transient_error(&self) -> u32 {
        let mut state = self.state.lock().await;
        state.transient += 1;
        state.transient
    }

    /// Transient provider errors in a row so far.
    pub(crate) async fn transient(&self) -> u32 {
        self.state.lock().await.transient
    }

    /// A turn went through: transient errors are no longer in a row.
    pub(crate) async fn clear_transient(&self) {
        self.state.lock().await.transient = 0;
    }

    /// Notes the worker's context size; answers the CLI session's first one.
    pub(crate) async fn note_context(&self, tokens: i64, window: Option<i64>) -> i64 {
        let mut state = self.state.lock().await;
        state.context = Some(tokens);
        if window.is_some() {
            state.window = window;
        }
        *state.session_start.get_or_insert(tokens)
    }

    /// Its model's context window, when its CLI said.
    pub(crate) async fn window(&self) -> Option<i64> {
        self.state.lock().await.window
    }

    /// Counts a finished tool call or command, which its task's counters don't hold yet.
    pub(crate) async fn count_call(&self) {
        self.state.lock().await.brain_calls += 1;
    }

    /// The counted calls its task's counters don't hold yet; they are added now.
    pub(crate) async fn take_calls(&self) -> u32 {
        std::mem::take(&mut self.state.lock().await.brain_calls)
    }

    /// Whether to ask the worker, now, at `tokens`, to end its turn with a handoff note: once,
    /// then again after each further `stretch` of growth, only while a turn runs that no
    /// hand-on or question waits in.
    pub(crate) async fn ask_handoff(&self, tokens: i64, stretch: i64) -> bool {
        let mut state = self.state.lock().await;
        let ask = state.busy
            && state.steered_at.is_none_or(|at| tokens >= at + stretch)
            && !state.stopping
            && !state.forced
            && state.cutoff.is_none()
            && state.question.is_none();
        if ask {
            state.handoff_asked = true;
            state.steered_at = Some(tokens);
        }
        ask
    }

    /// Whether to stop the worker's turn now for a hand-off (the hard line, or a refused
    /// compaction): once per CLI session, while a turn runs. Answers the session's generation.
    pub(crate) async fn force_handoff(&self) -> Option<u64> {
        let mut state = self.state.lock().await;
        let force = state.busy && !state.forced && !state.stopping && state.cutoff.is_none();
        if force {
            state.forced = true;
        }
        force.then_some(state.generation)
    }

    /// The worker's latest context size and its CLI session's first one, while this daemon
    /// has seen them.
    pub(crate) async fn context(&self) -> (Option<i64>, Option<i64>) {
        let state = self.state.lock().await;
        (state.context, state.session_start)
    }

    /// The CLI session now running (or last run) for the task.
    pub(crate) async fn generation(&self) -> u64 {
        self.state.lock().await.generation
    }

    /// The session moves to the user's terminal: a hand-off or hand-over decided for the CLI
    /// session before it finds the task moved on.
    pub(crate) async fn supersede(&self) {
        self.state.lock().await.generation += 1;
    }

    /// Holds a fresh session's first message until the paused task is resumed.
    pub(crate) async fn hold_handover(&self, first: TurnInput) {
        self.state.lock().await.held_handover = Some(first);
    }

    /// The first message of a fresh session held while the task was paused, if any.
    pub(crate) async fn take_held_handover(&self) -> Option<TurnInput> {
        self.state.lock().await.held_handover.take()
    }

    /// Gives the running CLI session `input`: a steer mid-turn, else a new turn. False
    /// without a session.
    pub(crate) async fn deliver(&self, input: TurnInput) -> Result<bool> {
        let mut state = self.state.lock().await;
        let Some(cli) = state.cli.clone() else {
            return Ok(false);
        };
        if state.busy {
            cli.session.steer(input).await
        } else {
            state.begin_turn();
            state.nudged = false;
            cli.session.send(input).await
        }
        .map_err(|err| Error::Provider(err.to_string()))?;
        Ok(true)
    }

    /// An event came from `cli`: if it is the worker's session now, the worker shows it works,
    /// and a command or tool call it starts or ends is noted. The user's own input echoed back
    /// says nothing about the worker.
    async fn note_event(&self, cli: &Arc<Cli>, event: &ProviderEvent) {
        use brigadier_providers::ItemStatus;
        let mut state = self.state.lock().await;
        if !state.cli.as_ref().is_some_and(|c| Arc::ptr_eq(c, cli)) {
            return;
        }
        state.note_progress(event, now_ms());
        let (item_id, status) = match event {
            ProviderEvent::Message {
                role: brigadier_providers::Role::User,
                ..
            } => return,
            ProviderEvent::Command {
                item_id, status, ..
            }
            | ProviderEvent::ToolCall {
                item_id, status, ..
            } => (Some(item_id), Some(*status)),
            ProviderEvent::TurnCompleted { .. } => {
                state.running_items.clear();
                (None, None)
            }
            ProviderEvent::Exited { .. } => {
                state.running_items.clear();
                state.background_jobs = 0;
                (None, None)
            }
            ProviderEvent::BackgroundJobs { running } => {
                state.background_jobs = u32::try_from(running.len()).unwrap_or(u32::MAX);
                if running.is_empty() {
                    state.background_overdue = false;
                }
                (None, None)
            }
            // A turn its CLI started on its own (a background job ended) runs like one we
            // started.
            ProviderEvent::TurnStarted { .. } => {
                state.busy = true;
                state.background_waiting = false;
                (None, None)
            }
            _ => (None, None),
        };
        if let (Some(item_id), Some(status)) = (item_id, status) {
            if status == ItemStatus::InProgress {
                state.running_items.insert(item_id.clone());
            } else {
                state.running_items.remove(item_id);
            }
        }
    }

    /// What the stall watchdog needs to know of the worker at `now`. A task live without a
    /// running CLI is noted from the first time it is seen so.
    pub(crate) async fn watch(&self, now: i64) -> WorkerWatch {
        let reroute_free = self.reroute.try_lock().is_ok();
        Self::watched(&mut *self.state.lock().await, now, reroute_free)
    }

    fn watched(state: &mut TaskLiveState, now: i64, reroute_free: bool) -> WorkerWatch {
        let alive = state
            .cli
            .as_ref()
            .is_some_and(|cli| cli.session.is_running());
        if alive || state.stopping {
            state.orphaned_at_ms = None;
        } else {
            state.orphaned_at_ms.get_or_insert(now);
        }
        WorkerWatch {
            generation: state.generation,
            alive,
            busy: state.busy,
            stopping: state.stopping,
            question: state.question.is_some(),
            command_running: !state.running_items.is_empty(),
            command_kind: state.progress.command(),
            command_due_ms: state
                .progress
                .command_deadline(&super::watchdog::Timing::current()),
            waiting: state.progress.waiting(),
            last_event_ms: state.last_event_ms,
            reported_at_ms: state.stall_reported_at_ms,
            last_command: state.progress.last_command(now),
            stalls: state.stalls,
            orphaned_at_ms: state.orphaned_at_ms,
            reroute_free,
        }
    }

    /// Claims one silence report under the same lock as provider progress, without
    /// steering the worker or changing its turn.
    pub(crate) async fn report_stall(
        &self,
        due: impl FnOnce(&WorkerWatch) -> bool,
        at: i64,
    ) -> bool {
        let reroute_free = self.reroute.try_lock().is_ok();
        let mut state = self.state.lock().await;
        let watch = Self::watched(&mut state, at, reroute_free);
        if !watch.alive || !due(&watch) {
            return false;
        }
        state.stall_reported_at_ms = Some(at);
        true
    }

    /// Takes the CLI session of a stalled worker off its task, if `due` still holds for it as
    /// it is now ([`super::watchdog::still_due`]); the caller holds `reroute`, and `settle`
    /// so no report is recorded meanwhile. After this the session's end is this hand-on, not
    /// a reason for another. `stall`: the same model continues in a fresh session, which
    /// counts as a stall of the current attempt; else another model takes over, and a pending
    /// cut-off (which wins) is returned with the session to end ([`Self::end_cli`]). `None`
    /// when it was no longer due.
    pub(crate) async fn detach_stalled(
        &self,
        due: impl FnOnce(&WorkerWatch) -> bool,
        stall: bool,
    ) -> Option<(Option<Arc<Cli>>, Option<AttemptEnd>)> {
        let mut state = self.state.lock().await;
        if !due(&Self::watched(&mut state, now_ms(), true)) {
            return None;
        }
        let cutoff = if stall {
            state.stalls += 1;
            None
        } else {
            state.cutoff.take()
        };
        Some((Self::detach_cli(&mut state), cutoff))
    }

    /// A new attempt: its stalls are counted afresh.
    pub(crate) async fn reset_stalls(&self) {
        self.state.lock().await.stalls = 0;
    }

    /// What the worker's tool calls run with, while its CLI runs headless or in a terminal.
    pub(crate) async fn tool_access(&self) -> Option<ToolAccess> {
        let state = self.state.lock().await;
        if let Some(cli) = &state.cli {
            return Some(ToolAccess {
                ended: cli.ended.clone(),
            });
        }
        state.terminal.clone()
    }

    /// The terminal the session is open in, or none any more (its commands end then).
    pub(crate) async fn set_terminal_access(&self, terminal: Option<ToolAccess>) {
        let old = std::mem::replace(&mut self.state.lock().await.terminal, terminal);
        if let Some(old) = old {
            old.ended.cancel();
        }
    }

    /// The worker's CLI session, while one runs.
    pub(crate) async fn cli(&self) -> Option<Arc<Cli>> {
        self.state.lock().await.cli.clone()
    }

    /// Whether a CLI session of the worker runs now.
    pub(crate) async fn has_cli(&self) -> bool {
        self.state.lock().await.cli.is_some()
    }

    /// Whether the worker can move its branch now: a CLI session of it runs (mid-turn or
    /// between turns), or the session is open in the user's terminal. A worker waiting for
    /// quota, or stopped after its CLI died, runs nothing.
    pub(crate) async fn executing(&self) -> bool {
        let state = self.state.lock().await;
        state.cli.is_some() || state.taken_over
    }

    #[cfg(test)]
    fn new(id: TaskId, conversation_id: ConversationId) -> Self {
        Self::reported(id, conversation_id, None)
    }

    /// A live task whose reports went up to `report_version` before.
    fn reported(id: TaskId, conversation_id: ConversationId, report_version: Option<u64>) -> Self {
        Self {
            id,
            conversation_id,
            state: tokio::sync::Mutex::new(TaskLiveState::default()),
            settle: tokio::sync::Mutex::new(()),
            reroute: tokio::sync::Mutex::new(()),
            wait_timer: std::sync::atomic::AtomicU64::new(0),
            watchdog_busy: std::sync::atomic::AtomicBool::new(false),
            learning: Arc::new(ReportLearning::after(report_version)),
            turn_end: tokio::sync::Mutex::new(()),
            takeover: tokio::sync::Mutex::new(None),
            next_cause: std::sync::Mutex::new(None),
            plain: false,
        }
    }

    /// Marks the session as open in the user's terminal, or no longer.
    pub(crate) async fn set_taken_over(&self, taken: bool) {
        self.state.lock().await.taken_over = taken;
    }

    /// Keeps `message` for the report request while the session is open in a terminal;
    /// `false` when it isn't.
    pub(crate) async fn held_for_terminal(&self, message: &str) -> bool {
        let mut state = self.state.lock().await;
        if state.taken_over {
            state.held_messages.push(message.to_owned());
        }
        state.taken_over
    }

    /// The messages held while the session was open in a terminal.
    pub(crate) async fn take_held_messages(&self) -> Vec<String> {
        std::mem::take(&mut self.state.lock().await.held_messages)
    }

    /// Waits until the worker's running turn is over and its end handled, so what it wrote
    /// after its report is known; at most [`TURN_END_WAIT`] (a CLI that never ends its turn,
    /// or an end that never finishes being handled, holds nothing up for good).
    pub(crate) async fn turn_over(&self) {
        self.turn_over_within(TURN_END_WAIT).await;
    }

    /// [`Self::turn_over`], waiting at most `limit`, for the turn's end being handled too.
    async fn turn_over_within(&self, limit: Duration) {
        let over = async {
            loop {
                {
                    let _end = self.turn_end.lock().await;
                    if !self.state.lock().await.busy {
                        return;
                    }
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };
        if tokio::time::timeout(limit, over).await.is_err() {
            tracing::debug!(task = %self.id, "the worker's turn did not end in time; going on");
        }
    }

    /// A new quota-wait timer: the ones set before it no longer retry.
    pub(crate) fn next_wait_timer(&self) -> u64 {
        self.wait_timer
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            + 1
    }

    /// Whether `timer` is still the newest quota-wait timer.
    pub(crate) fn is_wait_timer(&self, timer: u64) -> bool {
        self.wait_timer.load(std::sync::atomic::Ordering::Acquire) == timer
    }

    /// The provider of the worker's CLI while it works (a Brain job yields to it).
    pub(super) async fn busy_provider(&self) -> Option<ProviderKind> {
        let state = self.state.lock().await;
        state
            .cli
            .as_ref()
            .filter(|_| state.busy || state.question.is_some())
            .map(|cli| cli.provider)
    }

    /// The account its CLI session runs on, while it has one.
    pub(crate) async fn cli_account(&self) -> Option<crate::accounts::AccountRef> {
        let state = self.state.lock().await;
        state.cli.as_ref().map(|cli| cli.account.clone())
    }

    /// Ends the worker's CLI session (interrupting first; Codex keeps running its current
    /// command after an interrupt, so the session is closed, which ends its process tree).
    pub async fn close_cli(&self) {
        let cli = Self::detach_cli(&mut *self.state.lock().await);
        Self::end_cli(cli).await;
    }

    /// Takes the worker's CLI session out of `state`, to be ended with [`Self::end_cli`].
    fn detach_cli(state: &mut TaskLiveState) -> Option<Arc<Cli>> {
        state.stopping = true;
        state.question = None;
        state.cli.take()
    }

    /// Ends a CLI session taken out of the worker's state.
    pub(crate) async fn end_cli(cli: Option<Arc<Cli>>) {
        if let Some(cli) = cli {
            let _ = tokio::time::timeout(Duration::from_secs(2), cli.session.interrupt()).await;
            cli.session.close().await;
            cli.ended.cancelled().await;
        }
    }

    /// Whether a turn of its worker runs now.
    pub async fn busy(&self) -> bool {
        self.state.lock().await.busy
    }

    /// Whether its worker still needs its place in an overnight run: a turn runs, or one is
    /// coming when its background jobs end.
    async fn holds_run_slot(&self) -> bool {
        let state = self.state.lock().await;
        state.busy || state.background_waiting
    }

    /// After a deliberate close (hibernation) the worker may be started again.
    pub async fn allow_revival(&self) {
        let mut state = self.state.lock().await;
        state.stopping = false;
        state.busy = false;
    }

    pub async fn redactor(&self) -> Option<Arc<brigadier_providers::redact::Redactor>> {
        self.state.lock().await.redactor.clone()
    }
}

impl SessionManager {
    pub(crate) fn task_live(&self, task: &Task) -> Arc<TaskLive> {
        self.tasks_lock()
            .entry(task.id.clone())
            .or_insert_with(|| {
                Arc::new(TaskLive {
                    plain: task.plain.is_some(),
                    ..TaskLive::reported(
                        task.id.clone(),
                        task.conversation_id.clone(),
                        task.report_version,
                    )
                })
            })
            .clone()
    }

    /// The session's permission level, which its thread and its workers run at. While an
    /// overnight run is active nobody is there to ask, so Ask counts as Accept edits until the
    /// run ends (PLAN.md §10.8). In a folder the user doesn't trust it is Ask, overnight too.
    pub(crate) fn permission(&self, id: &ConversationId) -> PermissionLevel {
        if self.repo_trust(id) == Some(false) {
            return PermissionLevel::Ask;
        }
        let saved = match self.core.conversation(id).map(|c| c.setup) {
            Ok(Some(Setup::Session { permission, .. })) => permission,
            _ => PermissionLevel::default(),
        };
        match saved {
            PermissionLevel::Ask if self.overnight.active.get(id).is_some() => {
                PermissionLevel::AcceptEdits
            }
            saved => saved,
        }
    }

    /// Whether the session is in plan mode (see [`Setup::Session`]). Never during an
    /// overnight run: the user's Start approved its reviewed plans.
    pub(crate) fn plan_mode(&self, id: &ConversationId) -> bool {
        self.overnight.active.get(id).is_none()
            && matches!(
                self.core.conversation(id).map(|c| c.setup),
                Ok(Some(Setup::Session {
                    plan_mode: true,
                    ..
                }))
            )
    }

    /// How many workers each provider runs now, so parallel work spreads across vendors.
    pub(crate) fn running_workers(&self) -> Vec<(ProviderKind, u32)> {
        let mut claude = 0;
        let mut codex = 0;
        for live in self.tasks_lock().values() {
            match live
                .state
                .try_lock()
                .ok()
                .and_then(|s| s.cli.as_ref().map(|c| c.provider))
            {
                Some(ProviderKind::Claude) => claude += 1,
                Some(ProviderKind::Codex) => codex += 1,
                None => {}
            }
        }
        vec![(ProviderKind::Claude, claude), (ProviderKind::Codex, codex)]
    }

    pub(crate) fn existing_task_live(&self, id: &TaskId) -> Option<Arc<TaskLive>> {
        self.tasks_lock().get(id).cloned()
    }

    /// A task of the conversation by its reference: `task-3`, `3`, or its id.
    pub(crate) async fn find_task(
        &self,
        conversation_id: &ConversationId,
        reference: &str,
    ) -> Result<Task> {
        let reference = reference.trim();
        let number = reference
            .trim_start_matches("task-")
            .trim_start_matches('#')
            .parse::<u32>()
            .ok();
        let tasks = self.core.tasks(conversation_id).await?;
        tasks
            .into_iter()
            .find(|task| Some(task.number) == number || task.id.0 == reference)
            .ok_or_else(|| Error::NotFound(format!("{reference} (use list_tasks)")))
    }

    pub(crate) async fn task_by_id(
        &self,
        conversation_id: &ConversationId,
        id: &TaskId,
    ) -> Result<Task> {
        let board = self.core.board(conversation_id).await?;
        board
            .tasks
            .get(id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("task {id}")))
    }

    /// Changes a task and records it. The read, the change and the record are one step for
    /// every writer, so concurrent changes of a task all stay.
    pub(crate) async fn update_task(
        &self,
        conversation_id: &ConversationId,
        id: &TaskId,
        change: impl FnOnce(&mut Task),
    ) -> Result<Task> {
        self.update_task_if(conversation_id, id, |_| true, change)
            .await?
            .ok_or_else(|| Error::Invalid("the task changed meanwhile".into()))
    }

    /// [`Self::update_task`] when `still` holds for the task as it is now, checked and changed
    /// in one step (nothing else changes the task in between); `None`, and nothing recorded,
    /// when it no longer holds.
    pub(crate) async fn update_task_if(
        &self,
        conversation_id: &ConversationId,
        id: &TaskId,
        still: impl FnOnce(&Task) -> bool,
        change: impl FnOnce(&mut Task),
    ) -> Result<Option<Task>> {
        let (task, was) = {
            // Nothing under it waits for anything but the board.
            let _held = self.task_writes.lock().await;
            let mut task = self.task_by_id(conversation_id, id).await?;
            if !still(&task) {
                return Ok(None);
            }
            let was = task.state;
            let reported = task.report.is_some();
            change(&mut task);
            task.updated_at_ms = now_ms();
            let mut events = vec![DomainEvent::TaskUpdated {
                task: Box::new(task.clone()),
            }];
            events.extend(worker_step(&task, Some(was), reported));
            self.core
                .record_conversation(conversation_id, events)
                .await?;
            (task, was)
        };
        if task.state != was {
            self.settle_requests(conversation_id).await;
        }
        self.workers_changed(conversation_id, false);
        Ok(Some(task))
    }

    pub(crate) async fn set_task_state(
        &self,
        conversation_id: &ConversationId,
        id: &TaskId,
        state: TaskState,
    ) -> Result<Task> {
        self.update_task(conversation_id, id, |task| {
            // Only the takeover moves a task out of the user's terminal, or its end.
            if task.state == TaskState::TakenOver && !state.is_final() {
                return;
            }
            task.state = state;
            if state != TaskState::Blocked {
                task.blocked_reason = None;
            }
        })
        .await
    }

    /// Marks a task blocked (with why) or running again.
    pub(crate) async fn set_task_blocked(&self, id: &TaskId, reason: Option<String>) {
        let Some(live) = self.existing_task_live(id) else {
            return;
        };
        let result = self
            .update_task(&live.conversation_id, id, |task| {
                if task.state.is_final() || task.state == TaskState::TakenOver {
                    return;
                }
                match &reason {
                    Some(reason) => {
                        task.state = TaskState::Blocked;
                        task.blocked_reason = Some(reason.clone());
                    }
                    None if task.state == TaskState::Blocked => {
                        task.state = TaskState::Running;
                        task.blocked_reason = None;
                    }
                    None => {}
                }
            })
            .await;
        if let Err(err) = result {
            tracing::debug!(task = %id, error = %err, "could not update a task");
        }
    }

    /// [`Self::create_task`], with what only Brigadier's own tasks set (an overnight run's
    /// whole-phase checks and judge).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn create_task_as(
        &self,
        conversation_id: &ConversationId,
        title: String,
        kind: TaskKind,
        spec: String,
        pin: Option<brigadier_router::Pin>,
        avoid: Option<brigadier_router::Author>,
        distinct_from: Vec<brigadier_router::Author>,
        gate_link: Option<GateLink>,
        subject: Option<Task>,
        attachments: Vec<AttachmentRef>,
        areas: Option<Vec<brigadier_router::Area>>,
        floor: Option<QualityTier>,
        needs: Vec<brigadier_router::Capability>,
        extra: TaskExtra,
    ) -> Result<Task> {
        self.admit()?;
        let fence = self.enter(conversation_id)?;
        let conversation = self.core.conversation(conversation_id)?;
        let permission = self.permission(conversation_id);
        match &conversation.setup {
            Some(Setup::Session { .. }) => {}
            // A Chat is a thread without a project (Q76): its workers look things up and
            // operate apps, and never touch a repository.
            Some(Setup::Chat { .. })
                if conversation.kind == ConversationKind::Chat
                    && conversation.tab_chat.is_none()
                    && repo_access_for(kind, permission).repo == RepoAccess::None => {}
            Some(Setup::Chat { .. }) => {
                return Err(Error::Invalid(
                    "A Chat has no project or repository: its workers only research or operate apps (kind research or operate).".into(),
                ));
            }
            None => return Err(Error::Invalid("tasks belong to a session".into())),
        }
        let active_run = self.overnight.active.get(conversation_id);
        // Work filed under an overnight run that has ended starts nothing: read from the board,
        // so this holds after a restart and once the run's in-memory state is gone.
        let filed_under = match (&extra.request, &subject) {
            (Some(request), _) => Some(request.clone()),
            (None, Some(subject)) => subject.request_id.clone(),
            (None, None) => self.request_for(conversation_id, None).await,
        };
        if let Some(request) = &filed_under
            && self
                .core
                .board(conversation_id)
                .await
                .is_ok_and(|board| super::requests::ended_run_request(&board, request))
        {
            return Err(Error::Invalid(
                "The overnight run has ended; nothing new starts for it. If more is wanted, tell the user that Continue on the run's card starts a new segment on the same branch.".into(),
            ));
        }
        if subject.is_none()
            && active_run
                .as_ref()
                .is_some_and(|active| active.workspace.is_none())
        {
            return Err(Error::Invalid(
                "The overnight run's branch and worktree are still being made; try again in a moment.".into(),
            ));
        }
        // An ending run starts no new work; checks of what it already did may still finish.
        if subject.is_none()
            && extra.run.is_none()
            && active_run.as_ref().is_some_and(|active| {
                active.winding_down
                    || active
                        .wind_down_at_ms
                        .is_some_and(|at| crate::now_ms() >= at)
            })
        {
            return Err(Error::Invalid(
                "The overnight run is ending: no new work starts now. Finish the current step; what is left goes into the morning report.".into(),
            ));
        }
        let run = match extra.run {
            Some(run) => Some(run),
            None => super::overnight::policy::ActiveRun::context_for(
                active_run.as_ref(),
                subject.as_ref(),
            ),
        };
        let design = makes_designs(extra.design, extra.mockup.as_ref(), &needs);
        let category = extra.category.unwrap_or_else(|| category(kind, design));
        let areas = areas.unwrap_or_else(|| brigadier_router::infer_areas(&spec));
        let floor = floor.unwrap_or_else(|| brigadier_router::default_floor(category));
        // The roles that write, check or merge the request's code run on a vendor's best,
        // whatever floor the orchestrator asked for.
        let floor = match extra.role {
            Some(
                WorkerRole::Lead
                | WorkerRole::Fix
                | WorkerRole::Verifier
                | WorkerRole::Merge
                | WorkerRole::Reviewer,
            ) => floor.max(QualityTier::Frontier),
            _ => floor,
        };
        let now = now_ms();
        // A plain worker runs on the CLI, model and effort `brigadier start` named, the CLI's
        // own default model when it named none: Brigadier routes nothing for it (G18).
        let plain_choice = extra.plain.as_ref().and(pin.as_ref()).and_then(|pin| {
            Some(ModelChoice {
                provider: pin.provider?,
                model: pin.model.clone(),
                effort: pin.effort.clone(),
                fast: None,
                account: None,
            })
        });
        let (route, wait, trial_slot) = if let Some(choice) = plain_choice {
            let route = Route {
                choice,
                reason: "Started by its session with `brigadier start`.".into(),
                explanation: None,
            };
            (route, None, false)
        } else {
            let (preview, trial_slot) = self
                .preview(&super::routing::Ask {
                    category,
                    areas: &areas,
                    floor,
                    floor_binds: extra.rerun.is_some(),
                    needs: needs_of(kind, &attachments, &needs),
                    pin: pin.clone(),
                    hold_pin: false,
                    avoid,
                    distinct_from,
                    exclude: &[],
                    project_id: conversation.project_id.as_ref(),
                    // Checks of another task's work never go to a model on trial, nor does a rerun
                    // one tier up.
                    trial: if gate_link.is_some() || extra.rerun.is_some() {
                        super::routing::Trial::Never
                    } else {
                        super::routing::Trial::Take
                    },
                })
                .await;
            let (route, wait) = match preview.decision {
                brigadier_router::Decision::Run(routed) => {
                    (super::routing::route_from(routed), None)
                }
                // Nothing it may use is available. When a reset or the user's rules and rankings
                // can change that, the task waits like one that ran into a limit, and starts on
                // its own; otherwise the orchestrator hears why and can wait or ask.
                brigadier_router::Decision::Wait(waiting) => {
                    let waits = waiting.resets_at_ms.is_some()
                        || waiting.rule.is_some()
                        || waiting.ranking.is_some();
                    // The model it waits for (the first a reset or a rule change would free).
                    let Some(first) = preview.candidates.first().filter(|_| waits) else {
                        return Err(Error::Invalid(format!(
                            "no model can take this task now: {}",
                            waiting.reason
                        )));
                    };
                    let route = Route {
                        choice: ModelChoice {
                            provider: first.provider,
                            model: Some(first.model.clone()),
                            effort: None,
                            fast: None,
                            account: None,
                        },
                        reason: format!("Waits: {}", waiting.reason),
                        explanation: None,
                    };
                    let wait = QuotaWait {
                        reason: waiting.reason,
                        resets_at_ms: waiting.resets_at_ms,
                        rule: waiting.rule,
                        ranking: waiting.ranking,
                        since_ms: now,
                        messages: Vec::new(),
                        model: None,
                        request: None,
                        first_reset_ms: None,
                        account: None,
                        limit_kind: None,
                    };
                    (route, Some(wait))
                }
            };
            (route, wait, trial_slot)
        };
        let waits = wait.is_some();
        let mut route = route;
        if let Some(rerun) = &extra.rerun {
            super::reruns::mark_route(&mut route, rerun);
        }
        let number = self.core.next_task_number(conversation_id).await?;
        // A plan's reviewer belongs to the plan's request, even when a restart reopens an
        // older plan's review after newer requests.
        let plan_request = match &gate_link {
            Some(GateLink {
                owner: GateOwner::Plan { plan_id },
                ..
            }) => self
                .core
                .board(conversation_id)
                .await
                .ok()
                .and_then(|board| board.plans.get(plan_id)?.request_id.clone()),
            _ => None,
        };
        let request_id = match (&subject, plan_request, extra.request) {
            // A plain worker belongs to its session, never to one of its requests: its turns
            // hold none of them open.
            _ if extra.plain.is_some() => None,
            (_, _, Some(request)) => Some(request),
            (Some(subject), _, None) => subject.request_id.clone(),
            (None, Some(request), None) => Some(request),
            (None, None, None) => self.request_for(conversation_id, None).await,
        };
        let names = self
            .core
            .board(conversation_id)
            .await?
            .tasks
            .values()
            .map(|task| task.title.clone())
            .collect::<Vec<_>>();
        // A rerun one tier up is the same worker retried, not a numbered second one (#368).
        let title = if extra.rerun.is_some() {
            format!("{} retried", title.trim())
        } else {
            job_name(
                &title,
                kind,
                subject.as_ref().map(|task| task.title.as_str()),
                &names,
            )
        };
        let task = Task {
            id: extra.id.clone().unwrap_or_else(TaskId::generate),
            conversation_id: conversation_id.clone(),
            number,
            position: 0,
            title: title.trim().to_owned(),
            kind,
            spec,
            access: match &extra.plain {
                Some(plain) => repo_access_for(kind, plain.permission),
                None => repo_access_for(kind, permission),
            },
            // A waiting task's first model is recorded when it starts.
            attempts: if waits {
                Vec::new()
            } else {
                vec![Attempt {
                    route: route.clone(),
                    started_at_ms: now,
                    ended_at_ms: None,
                    end: None,
                }]
            },
            route,
            floor,
            areas,
            pin,
            needs,
            state: if waits {
                TaskState::Paused
            } else {
                TaskState::Queued
            },
            blocked_reason: wait
                .as_ref()
                .map(|wait| format!("Waiting for quota: {}", wait.reason)),
            quota_wait: wait.map(Box::new),
            subject: subject.as_ref().map(|task| task.id.clone()),
            target: extra.target,
            end_state: extra.end_state,
            mockup: extra.mockup,
            design,
            plan: None,
            attachments,
            workspace: None,
            report: None,
            candidate: None,
            gate_link,
            role: extra.role,
            phase: extra.phase,
            landing: None,
            landing_from: None,
            landed: None,
            error: None,
            kept: None,
            outputs: Vec::new(),
            request_id,
            run,
            messages: Vec::new(),
            rework_rounds: 0,
            native_session: None,
            report_version: None,
            deaths: Vec::new(),
            live_look: extra.live_look,
            fresh_commit: extra.fresh_commit,
            integrations: extra.integrations,
            brain: Default::default(),
            resuming: false,
            takeover: None,
            trial_slot: waits && trial_slot,
            rerun: extra.rerun,
            plain: extra.plain.clone(),
            created_at_ms: now,
            updated_at_ms: now,
        };
        let mut events = vec![DomainEvent::TaskUpdated {
            task: Box::new(task.clone()),
        }];
        events.extend(worker_step(&task, None, false));
        // A cleanup that stopped waiting for this has already passed the session's tasks (it
        // may have been restored since): the task isn't made.
        if fence.cut_off() {
            return Err(super::closing::closing_error());
        }
        self.core
            .record_conversation(conversation_id, events)
            .await?;
        if waits {
            // Its timer is set as for any task waiting for quota.
            self.keep_waiting(&task).await;
            return Ok(task);
        }
        let live = self.task_live(&task);
        let manager = self.arc();
        let started = task.clone();
        self.spawn(async move {
            if let Err(err) = manager.start_worker(&live, started.clone(), subject).await {
                manager.worker_failed(&started, &err.to_string()).await;
            }
        });
        Ok(task)
    }

    pub(crate) async fn start_worker(
        &self,
        live: &Arc<TaskLive>,
        task: Task,
        subject: Option<Task>,
    ) -> Result<()> {
        let conversation_id = task.conversation_id.clone();
        // Held while its worktree is made and recorded; the launch below enters again.
        let fence = self.enter(&conversation_id)?;
        self.set_task_state(&conversation_id, &task.id, TaskState::Starting)
            .await?;
        let owner = format!("task:{}", task.id);
        let workspace = self
            .prepare_workspace(&owner, &task, subject.as_ref())
            .await?;
        let task = self
            .update_task(&conversation_id, &task.id, |t| {
                t.workspace = Some(TaskWorkspace {
                    worktree: workspace
                        .worktree
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned()),
                    branch: workspace.branch.clone(),
                    base: workspace.base.as_ref().map(|oid| oid.0.clone()),
                    on_snapshot: workspace.on_snapshot,
                    target: workspace.target.clone(),
                    scratch: workspace.scratch.to_string_lossy().into_owned(),
                    warmed: workspace.warmed.clone(),
                    merge: workspace.merge.clone(),
                });
            })
            .await?;
        let files = self.worker_files(&task, &workspace.scratch).await;
        drop(fence);
        // A plain worker's first message is its prompt, as `brigadier start` gave it.
        if task.plain.is_some() {
            self.record_plain_workspace(&task).await;
            return self
                .launch_worker(
                    live,
                    &task,
                    None,
                    Origin::New,
                    TurnInput::with_files(task.spec.clone(), files),
                )
                .await;
        }
        // What the Brain knows about the task, then what the thread already read, go with its
        // first message (SESSION-FIXES-PLAN.md Q6.6, THREAD-PLAN.md Q8 lever 1).
        // A fresh verifier gets none of the build's context: its spec holds all it reads.
        let fresh = task.fresh_commit.is_some();
        let brain = match fresh {
            true => None,
            false => {
                self.brain_section(&task, workspace.worktree.as_deref())
                    .await
            }
        };
        let pack = match &workspace.worktree {
            Some(worktree) if !fresh => self.context_pack(&task, worktree).await,
            _ => None,
        };
        let mut text = String::new();
        // The user's standing constraints first, word for word (BUILD-PLAN D1.1); a fresh
        // verifier's spec already holds them.
        if !fresh
            && let Some(section) =
                super::constraints::brief_text(&self.active_constraints(&conversation_id).await)
        {
            text.push_str(&section);
            text.push_str("\n\n");
        }
        if let Some((section, facts)) = &brain {
            text.push_str(section);
            text.push_str("\n\n");
            let facts = *facts;
            self.update_task(&conversation_id, &task.id, |task| {
                task.brain.facts_in = facts;
            })
            .await
            .ok();
        }
        if let Some(pack) = pack {
            text.push_str(&pack);
            text.push_str("\n\n");
        }
        text.push_str("Start the task.");
        // A fix of landed work whose session can't be resumed starts from its worker's report.
        if let Some(fixed) = subject
            .as_ref()
            .filter(|s| task.role == Some(WorkerRole::Fix) && s.landed.is_some())
            && let Some(report) = &fixed.report
        {
            text.push_str(&format!(
                "\n\nThe work you fix is task-{}'s, which landed. Its worker's report:\n{}",
                fixed.number, report.summary
            ));
        }
        // Instructions the orchestrator sent while the task waited to start go with it.
        if !task.messages.is_empty() {
            text.push_str("\n\nLater instructions from the thread, oldest first:\n");
            for message in &task.messages {
                text.push_str("\n- ");
                text.push_str(&message.replace('\n', "\n  "));
            }
        }
        self.launch_worker(
            live,
            &task,
            subject.as_ref(),
            Origin::New,
            TurnInput::with_files(text, files),
        )
        .await?;
        Ok(())
    }

    /// Starts (or resumes) the worker's CLI session in the task's prepared workspace and sends
    /// it `first`.
    pub(crate) async fn launch_worker(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        subject: Option<&Task>,
        origin: Origin,
        first: TurnInput,
    ) -> Result<()> {
        // A new worker waits for serious heat or critical memory pressure (PLAN.md §10.7).
        // An overnight run's worker starts once its run has a worker free (§10.7). A new
        // session doesn't start at all once its run winds down (§10.9); a resumed one may
        // still hand off.
        if matches!(origin, Origin::New) {
            self.hold_while_strained(task).await?;
            self.admit_new_run_task(task).await?;
            // The machine may have heated up while it waited for a run's slot.
            while task.run.is_some() && self.machine_strained() {
                self.release_run_task(&task.id);
                self.hold_while_strained(task).await?;
                self.admit_new_run_task(task).await?;
            }
        } else {
            self.admit_run_task(task).await?;
        }
        // Held until its CLI session is registered, where the session's cleanup finds it.
        let launched = match self.enter(&task.conversation_id) {
            Ok(fence) => {
                let launched = self
                    .launch_admitted(live, task, subject, origin, first, &fence)
                    .await;
                // A cleanup that stopped waiting for it has already passed this task.
                if launched.is_ok() && fence.cut_off() {
                    live.close_cli().await;
                    Err(super::closing::closing_error())
                } else {
                    launched
                }
            }
            Err(err) => Err(err),
        };
        if launched.is_err() {
            self.release_run_task(&task.id);
        }
        launched
    }

    /// The worker's CLI session for `task` as its headless process runs it (and a terminal
    /// continues it): its spec, with a fresh worker grant issued to `grant_owner`. A new
    /// session's `first` message gets the task's brief in front.
    pub(crate) async fn worker_session(
        &self,
        task: &Task,
        subject: Option<&Task>,
        origin: Origin,
        first: Option<&mut TurnInput>,
        grant_owner: &str,
    ) -> Result<WorkerSession> {
        let conversation_id = task.conversation_id.clone();
        let owner = format!("task:{}", task.id);
        let recorded = task
            .workspace
            .clone()
            .ok_or_else(|| Error::Invalid("the task has no workspace".into()))?;
        let conversation = self.core.conversation(&conversation_id)?;
        let workspace = Workspace {
            repo: self.tasks_repo(&conversation, task)?,
            worktree: recorded.worktree.map(PathBuf::from),
            branch: recorded.branch,
            base: recorded.base.map(Oid),
            on_snapshot: recorded.on_snapshot,
            target: recorded.target,
            scratch: PathBuf::from(recorded.scratch),
            warmed: recorded.warmed,
            merge: recorded.merge,
        };
        let project = conversation
            .project_id
            .as_ref()
            .and_then(|id| self.core.project(id).ok());
        let secret_files = project
            .as_ref()
            .map(|p| p.prefs.secret_files.clone())
            .unwrap_or_default();
        // A writer's new worktree with the repository's setup script has it run before the
        // worker starts (never a reader's, never in a folder the user doesn't trust).
        // A plain worker in its session's checkout (`--local`) works in a folder that is not
        // its own: nothing is set up, copied or swept there for it.
        let plain = task.plain.as_ref();
        let shared_checkout = plain.is_some_and(|plain| plain.local);
        let setup = match (&workspace.worktree, &origin) {
            (Some(worktree), Origin::New)
                if task.kind.writes() && task.attempts.len() <= 1 && !shared_checkout =>
            {
                self.repo_trust(&conversation_id) == Some(true) && {
                    let (git, worktree) = (self.git.clone(), worktree.clone());
                    blocking(move || Ok(has_setup(&git, &worktree)))
                        .await
                        .unwrap_or(false)
                }
            }
            _ => false,
        };
        // A successor taking the task over finds the worktree's secrets already in place.
        let mut secret_values = match (&workspace.worktree, &origin) {
            (Some(worktree), Origin::New) if task.attempts.len() <= 1 && !shared_checkout => {
                secrets::copy_secrets(self, &owner, &workspace.repo, worktree, &secret_files)
                    .await?
            }
            // A Chat's task has no repository, so no env files either.
            _ if conversation.kind == ConversationKind::Chat => Vec::new(),
            _ => secrets::values(&self.git, &workspace.repo, &secret_files).await,
        };

        let provider = task.route.choice.provider;
        let write = task.kind.writes();
        // A Claude worker starts in the conversation's worker folder, the same for all its
        // workers, with its worktree added: Claude's prompt names its working directory, so
        // one shared folder lets a second worker read the first one's prompt from the cache
        // (THREAD-PLAN.md Q8 lever 2). Codex cannot run with a read-only cwd: a read-only
        // Codex worker works from its scratch folder and reads the worktree by path.
        // A plain worker starts in its worktree, as a CLI started there would.
        let home = match (&workspace.worktree, provider) {
            (Some(_), ProviderKind::Claude) if plain.is_none() => {
                let home = self.worker_home(&conversation_id).await?;
                Some(home)
            }
            _ => None,
        };
        let cwd = match (&workspace.worktree, &home, write) {
            (Some(_), Some(home), _) => home.clone(),
            (Some(worktree), None, true) => worktree.clone(),
            _ => workspace.scratch.clone(),
        };
        let repo_note = match (&workspace.worktree, write) {
            (Some(worktree), true) if cwd != *worktree => format!(
                "Your worktree (a checkout of the repository on branch `{}`): {}\n{}: run every command in your worktree (`cd` there first, or `git -C <worktree>`), and edit its files by their full path.\nYour scratch folder: {}",
                workspace.branch.clone().unwrap_or_default(),
                worktree.display(),
                start_note(&cwd, &workspace.scratch),
                workspace.scratch.display()
            ),
            (Some(worktree), false) if home.is_some() => format!(
                "A read-only checkout of the repository: {}\n{}: `cd` into the checkout first, and read its files by their full path.\nYour scratch folder (writable): {}",
                worktree.display(),
                start_note(&cwd, &workspace.scratch),
                workspace.scratch.display()
            ),
            (Some(worktree), true) => format!(
                "Your worktree (a checkout of the repository on branch `{}`): {}\nYour scratch folder: {}",
                workspace.branch.clone().unwrap_or_default(),
                worktree.display(),
                workspace.scratch.display()
            ),
            (Some(worktree), false) => format!(
                "A read-only checkout of the repository: {}\nYour scratch folder (writable): {}",
                worktree.display(),
                workspace.scratch.display()
            ),
            (None, _) => format!(
                "There is no repository checkout for this task. Your scratch folder: {}",
                workspace.scratch.display()
            ),
        };
        let outputs = outputs_dir(&workspace.scratch);
        let mut repo_note = format!(
            "{repo_note}\nYour outputs folder (files for the thread and the user): {}\nLink a file there by its name as outputs/<name> (outputs/notes.md), never by that folder's path, in your report and messages too.",
            outputs.display()
        );
        if provider == ProviderKind::Codex {
            repo_note.push_str(
                "\nImages you generate are copied into your outputs folder as they are made.",
            );
        }
        {
            let outputs = outputs.clone();
            blocking(move || {
                std::fs::create_dir_all(&outputs).map_err(|err| Error::Invalid(err.to_string()))
            })
            .await?;
        }
        let test_dir = test_data_dir(&task.id);
        self.prepare_owned_dir(&owner, &test_dir).await?;
        // Its rights in the worktree are those of a worker started there; its start folder
        // and the worktree (added) are writable together or not at all.
        // A worker gets its session's level; a plain one the level it was started with (Q8),
        // Ask in a folder the user doesn't trust.
        let access = match plain {
            Some(_) if self.repo_trust(&task.conversation_id) == Some(false) => {
                PermissionLevel::Ask.access()
            }
            Some(plain) => plain.permission.access(),
            None => self.permission(&task.conversation_id).access(),
        };
        // Once its worktree has the checkout's env files.
        let setup = match (&workspace.worktree, setup) {
            (Some(worktree), true) => Some(
                self.run_setup_script(
                    &conversation_id,
                    &owner,
                    worktree,
                    &worker_env(&workspace.scratch),
                    // A stopped worker's processes end with its owner's.
                    tokio_util::sync::CancellationToken::new(),
                )
                .await,
            ),
            _ => None,
        };
        repo_note.push_str("\n\n");
        repo_note.push_str(&prompts::environment(&prompts::WorkerEnvironment {
            warmed: &workspace.warmed,
            test_dir: &test_dir,
            run_repo: task.run.as_ref().map(|_| workspace.repo.as_path()),
            branch: workspace.branch.as_deref(),
            low_priority: true,
            setup: setup.as_deref(),
        }));
        let native = match &workspace.worktree {
            Some(worktree) => instructions::for_worker(provider, worktree, &task.spec).await,
            None => String::new(),
        };
        let mut extra = match (&task.kind, subject) {
            (TaskKind::Review | TaskKind::Verify, Some(subject)) => {
                self.review_brief(subject, &workspace.scratch).await
            }
            (TaskKind::Merge, Some(subject)) => self.merge_brief(subject, &workspace).await,
            (TaskKind::Merge, None) => self.base_merge_brief(task, &workspace).await,
            _ => String::new(),
        };
        // Computer use (a GUI job, and the live look, have its tools loaded from the start).
        let computer = match plain {
            Some(_) => None,
            None => {
                self.computer_use_kit(
                    &task.id.to_string(),
                    task.kind == TaskKind::Operate || prompts::checks_live(task),
                )
                .await
            }
        };
        // The one live look at the integrated tree, and a change matched to its mockup, are
        // checked live with the computer tools; other UI work with the project's scripted UI
        // checks (SESSION-FIXES-PLAN.md "Computer use", §11).
        let scripted = cfg!(target_os = "macos") && prompts::scripted_ui(task);
        if computer.is_some() && prompts::checks_live(task) {
            let dev_app = workspace
                .worktree
                .as_deref()
                .and_then(super::context_pack::dev_app_command);
            extra.push_str(&prompts::worker_computer(dev_app.as_deref()));
        } else if scripted {
            extra.push_str(&prompts::scripted_ui_note(computer.is_some()));
        }
        let omit_ai_coauthors = self.core.settings().omit_ai_coauthors;
        if omit_ai_coauthors && provider == ProviderKind::Codex && task.kind.writes() {
            extra.push_str("\n\n");
            extra.push_str(prompts::NO_AI_COAUTHORS);
        }
        // Read once: what the brief says is what the worker counts as told.
        let writing = prompts::git_writing(&self.core.settings());
        if task.kind == TaskKind::Implement
            && let Some(text) = &writing
        {
            extra.push_str("\n\n");
            extra.push_str(text);
        }
        let browser = self.core.settings().browser_enabled;
        if !browser {
            extra.push_str("\n\n");
            extra.push_str(prompts::BROWSER_OFF);
        }
        if computer.is_some() {
            extra.push_str("\n\n");
            extra.push_str(prompts::COMPUTER_USE);
            if scripted {
                extra.push_str(prompts::SCRIPTED_UI_COMPUTER);
            }
        }
        if self.held_by_plan_mode(task).await {
            extra.push_str("\n\n");
            extra.push_str(super::phases::PLAN_MODE_HOLD);
        }
        let prompt = prompts::worker_system(&native);
        // A new CLI session hears its task first; a resumed or forked one has it already.
        match (&origin, first) {
            // A plain worker's first message is its prompt alone.
            (Origin::New, Some(_)) if plain.is_some() => {}
            (Origin::New, Some(first)) => {
                first.parts.insert(
                    0,
                    brigadier_providers::InputPart::Text(prompts::worker_brief(
                        task, &repo_note, &extra,
                    )),
                );
            }
            // A resumed or forked session keeps the instructions it started with.
            (_, Some(_)) => {}
            (_, None) => {}
        }

        let worker_grant = self.grants.issue(
            grant_owner,
            Role::Worker {
                conversation_id: conversation_id.clone(),
                task_id: task.id.clone(),
                checks: task.gate_link.is_some() || task.role == Some(WorkerRole::Reviewer),
            },
        );
        // B7: the grant is a secret too.
        secret_values.push(worker_grant.clone());
        // A plain worker reaches Brigadier through `brigadier` only.
        let mut mcp_servers = match plain {
            Some(_) => Vec::new(),
            None => {
                vec![self.brigadier_server(worker_grant.clone(), WORKER_TOOL_TIMEOUT_SECS, true)]
            }
        };
        let mut skill_plugins = Vec::new();
        if let Some(kit) = computer {
            secret_values.push(kit.token);
            mcp_servers.push(kit.server);
            skill_plugins.push(kit.plugin);
        }
        // The servers its task names that its agent lacks, the skills its agent doesn't find
        // by itself, and what the user switched off (`integrations`).
        let shared = match plain {
            // A plain worker has its CLI's own setup, as a thread does; a lean one none of its
            // servers and only the skills it kept (G23).
            Some(plain) => super::integrations::Shared {
                switched_off: match plain.lean {
                    true => {
                        self.lean_worker_switched_off(
                            &conversation_id,
                            provider,
                            &cwd,
                            &plain.skills,
                        )
                        .await
                    }
                    false => {
                        self.thread_switched_off(&conversation_id, provider, &cwd)
                            .await
                    }
                },
                ..Default::default()
            },
            None => {
                self.worker_shared(
                    &conversation_id,
                    &task.integrations,
                    provider,
                    &cwd,
                    workspace.worktree.as_deref(),
                    &worker_grant,
                )
                .await
            }
        };
        secret_values.extend(shared.secrets);
        mcp_servers.extend(shared.servers);
        skill_plugins.extend(shared.skills);
        // A Claude worker's turn ends only with its report, or rightly waiting (BUILD-PLAN E2).
        let stop_hook = (task.route.choice.provider == ProviderKind::Claude && plain.is_none())
            .then(|| {
                let grant = self.grants.issue(
                    grant_owner,
                    Role::StopHook {
                        conversation_id: conversation_id.clone(),
                        task_id: task.id.clone(),
                    },
                );
                secret_values.push(grant.clone());
                self.stop_hook_command(grant)
            });
        let mut env = worker_env(&workspace.scratch);
        env.extend(self.cli_env(&task.id.0, &worker_grant));
        let redactor = secrets::redactor(secret_values);
        let allowed_models = self.rules_models(task.route.choice.provider)?;
        // Its own branch is the only ref it moves; the branch its work lands on is the engine's.
        // Its branch and the session's, recorded for the ref check, beside its start.
        if let Some(worktree) = workspace.worktree.clone() {
            let manager = self.arc();
            let (branch, conversation, task) = (
                workspace.branch.clone(),
                conversation_id.clone(),
                task.id.clone(),
            );
            // Git runs off the async threads.
            self.spawn(async move {
                let _ = blocking(move || {
                    if let Some(branch) = &branch {
                        manager.record_branch(&worktree, branch, &conversation, Some(&task));
                    }
                    manager.record_session_refs(&conversation, &worktree);
                    Ok(())
                })
                .await;
            });
        }
        let spec = SessionSpec {
            cwd: cwd.clone(),
            model: task.route.choice.model.clone(),
            effort: task.route.choice.effort.clone(),
            fast: false,
            origin,
            // Resumed for follow-ups, fixes and retries.
            ephemeral: false,
            access,
            append_system_prompt: plain.is_none().then_some(prompt),
            mcp_servers,
            tools: ToolSet::Lean,
            skill_plugins,
            switched_off: shared.switched_off,
            add_dirs: match (&home, &workspace.worktree) {
                (Some(_), Some(worktree)) => vec![worktree.clone()],
                _ => Vec::new(),
            },
            // The project's own setup (its hooks, MCP servers, skills), in a folder the user
            // trusts.
            project: workspace
                .worktree
                .clone()
                .filter(|_| self.repo_trust(&conversation_id) == Some(true)),
            env,
            unset_env: Vec::new(),
            low_priority: true,
            record_to: None,
            redactor: redactor.clone(),
            owned_cwd: !shared_checkout,
            // The shared worker folder is never one worker's to sweep: ending what runs there
            // when one CLI goes would kill every other Claude worker of the session.
            sweep_dir: Some(if home.is_some() || shared_checkout {
                workspace.scratch.clone()
            } else {
                cwd.clone()
            }),
            // Workers are handed off, never compacted by their CLI (BUILD-PLAN D2.3); a plain
            // one compacts as its CLI does.
            auto_compact: plain.is_some(),
            // A plain worker's sub-agents run on what its CLI allows (G18).
            allowed_models: allowed_models.clone(),
            omit_ai_coauthors,
            output_hook: None,
            stop_hook,
            deny_tools: None,
            lean_worker: plain.is_some_and(|plain| plain.lean),
        };
        let worktree = workspace.worktree.clone();
        Ok(WorkerSession {
            spec,
            cwd,
            outputs,
            redactor,
            allowed_models,
            worktree,
            grant: worker_grant,
        })
    }

    /// [`Self::launch_worker`] once admitted.
    async fn launch_admitted(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        subject: Option<&Task>,
        origin: Origin,
        first: TurnInput,
        fence: &super::closing::WorkGuard,
    ) -> Result<()> {
        let conversation_id = task.conversation_id.clone();
        let owner = format!("task:{}", task.id);
        // A resumed Codex thread's token totals include the turns counted before.
        let resumed = matches!(origin, Origin::Resume { .. });
        let continues = task.route.choice.provider == ProviderKind::Codex && resumed;
        // A session resumed after a restart: where it started is in its recorded events.
        let seeded_start = if resumed && live.context().await.1.is_none() {
            self.last_worker_context(&task.id).await.1
        } else {
            None
        };
        let mut first = first;
        let WorkerSession {
            spec,
            cwd,
            outputs,
            redactor,
            allowed_models,
            worktree,
            grant,
            ..
        } = self
            .worker_session(task, subject, origin, Some(&mut first), &owner)
            .await?;
        // Stopped while its new worktree's setup script ran (#457): its CLI never starts.
        if self.stopped_meanwhile(task).await {
            self.grants.revoke_owner(&owner);
            return Err(stopped_as_it_started(task));
        }
        let provider = task.route.choice.provider;
        let account = self.runtime.account_for(&task.route.choice);
        let tools_key = crate::routing::meter::tools_key(&spec);
        let Started { session, events } =
            match self.runtime.start_hosted(&owner, &account, spec).await {
                Ok(started) => started,
                Err(err) => {
                    self.grants.revoke_owner(&owner);
                    return Err(err);
                }
            };
        // A cleanup that stopped waiting for this start has already ended the task (its
        // session may have been restored since, and the task launched again): this session
        // ends unused, and only what it made goes.
        if fence.cut_off() {
            let native_id = session.native_id();
            session.close().await;
            self.grants.revoke(&[grant]);
            self.release_session_files(&owner, &native_id).await;
            return Err(super::closing::closing_error());
        }
        // Why this worker's CLI starts (BUILD-PLAN B0): a hand-off or a reroute said so.
        let cause = live.take_next_cause().unwrap_or(if resumed {
            WriteCause::Resume
        } else {
            WriteCause::FreshStart
        });
        let cli = Arc::new(Cli {
            provider,
            meter: TokenMeter::new(continues)
                .on_account(account.account.clone())
                .starting(cause)
                .with_tools(tools_key),
            account,
            model: task.route.choice.clone(),
            chosen: None,
            session,
            owner,
            ended: CancellationToken::new(),
            launch: None,
        });
        {
            let mut state = live.state.lock().await;
            // Opened in the user's terminal meanwhile: the terminal is the session's one
            // writer.
            if state.taken_over {
                drop(state);
                cli.session.close().await;
                self.grants.revoke_owner(&cli.owner);
                return Err(Error::Invalid(format!(
                    "task-{} is open in the user's terminal",
                    task.number
                )));
            }
            state.cli = Some(cli.clone());
            state.cwd = Some(cwd);
            state.worktree = worktree;
            state.outputs = Some(outputs);
            state.redactor = redactor;
            state.begin_turn();
            state.nudged = false;
            state.stopping = false;
            state.handoff_asked = false;
            state.steered_at = None;
            state.forced = false;
            // Any start supersedes a fresh session held for a paused task.
            state.held_handover = None;
            state.generation += 1;
            state.running_items.clear();
            state.background_jobs = 0;
            state.background_overdue = false;
            state.background_waiting = false;
            state.stall_reported_at_ms = None;
            state.orphaned_at_ms = None;
            state.allowed_models = allowed_models;
            if !resumed {
                state.context = None;
                state.session_start = None;
            } else if state.session_start.is_none() {
                state.session_start = seeded_start;
            }
        }
        #[cfg(debug_assertions)]
        self.arm_env_fault(&task.id, provider).await;
        let manager = self.arc();
        let pumped = (live.clone(), cli.clone());
        self.spawn(async move { manager.pump_worker(pumped.0, pumped.1, events).await });
        // A Stop while its CLI started ends it before its brief, and it stays stopped (#457).
        let was_final = task.state.is_final();
        let mut stopped = false;
        self.update_task(&conversation_id, &task.id, |t| {
            if self.is_stopping(&t.id) || (t.state.is_final() && !was_final) {
                stopped = true;
                return;
            }
            // Only the takeover moves a task out of the user's terminal.
            if t.state != TaskState::TakenOver {
                t.state = TaskState::Running;
                t.blocked_reason = None;
            }
        })
        .await?;
        if stopped {
            live.close_cli().await;
            self.grants.revoke_owner(&cli.owner);
            return Err(stopped_as_it_started(task));
        }
        if live.state.lock().await.taken_over {
            // Opened in the user's terminal as it started (the open closed it): what it was
            // to be told waits for the report request.
            let text: Vec<&str> = first
                .parts
                .iter()
                .filter_map(|part| match part {
                    brigadier_providers::InputPart::Text(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            if !text.is_empty() {
                let text = text.join("\n");
                if live.held_for_terminal(&text).await {
                    self.keep_held(task, &text).await;
                }
            }
            return Ok(());
        }
        cli.session
            .send(first)
            .await
            .map_err(|err| Error::Provider(err.to_string()))?;
        Ok(())
    }

    /// Whether `task` is being stopped, or ended since it was read as `task`.
    async fn stopped_meanwhile(&self, task: &Task) -> bool {
        self.is_stopping(&task.id)
            || (!task.state.is_final()
                && self
                    .task_by_id(&task.conversation_id, &task.id)
                    .await
                    .is_ok_and(|now| now.state.is_final()))
    }

    /// Why a worker can't be resumed; a run lists it under What got in the way.
    async fn cannot_resume(&self, task: &Task) -> Error {
        if let Some(context) = &task.run {
            self.note_obstacle(
                &task.conversation_id,
                &context.run_id,
                crate::overnight::ObstacleKind::ResumeFailed,
                "A worker's session couldn't be resumed, so its work started over.",
                Some(task.number),
            )
            .await;
        }
        Error::Invalid(format!(
            "task-{} cannot be resumed; delegate a new task",
            task.number
        ))
    }

    /// Starts a worker whose CLI session stopped (the conversation hibernated) again,
    /// resuming its CLI session.
    async fn revive_worker(&self, live: &Arc<TaskLive>, task: &Task, text: String) -> Result<()> {
        // One start at a time: a worker resumed meanwhile, after its CLI ended, takes the
        // message in the turn it runs.
        let _handing = live.reroute.lock().await;
        if let Some(cli) = live.cli().await {
            return cli
                .session
                .steer(TurnInput::text(text))
                .await
                .map_err(|err| Error::Provider(err.to_string()));
        }
        let Some(native_id) = self.last_worker_native_id(task).await else {
            return Err(self.cannot_resume(task).await);
        };
        let subject = match &task.subject {
            Some(id) => self.task_by_id(&task.conversation_id, id).await.ok(),
            None => None,
        };
        let task = self
            .update_task(&task.conversation_id, &task.id, |t| {
                t.candidate = None;
                if t.report.is_some() {
                    t.rework_rounds += 1;
                }
                // Sent work after it stopped for dying twice: it gets its chances again.
                t.deaths.clear();
            })
            .await?;
        let from = live.generation().await;
        if !live.plain && self.worker_over_handoff(live, &task.id).await {
            // Boxed: unboxed it added its 18 KB to every future that can send a worker work.
            Box::pin(self.hand_over(live, &task, from, None, Some(text), Handover::Size)).await?;
        } else {
            self.launch_worker(
                live,
                &task,
                subject.as_ref(),
                Origin::Resume { native_id },
                TurnInput::text(text),
            )
            .await?;
        }
        Ok(())
    }

    /// The worker's CLI session to resume: the one recorded on the task, else (a task recorded
    /// before it was kept there) the latest session start in its events, which is then
    /// recorded.
    pub(crate) async fn last_worker_native_id(&self, task: &Task) -> Option<String> {
        if let Some(native_id) = &task.native_session {
            return Some(native_id.clone());
        }
        let native_id = latest_session_start(self.core.store(), &task.id).await?;
        self.note_native_session(&task.conversation_id, &task.id, &native_id)
            .await;
        Some(native_id)
    }

    /// Records the worker's latest CLI session on its task.
    async fn note_native_session(
        &self,
        conversation_id: &ConversationId,
        id: &TaskId,
        native_id: &str,
    ) {
        let known = self
            .task_by_id(conversation_id, id)
            .await
            .is_ok_and(|task| task.native_session.as_deref() == Some(native_id));
        if known {
            return;
        }
        if let Err(err) = self
            .update_task(conversation_id, id, |t| {
                t.native_session = Some(native_id.to_owned());
            })
            .await
        {
            tracing::warn!(task = %id, error = %err, "could not record the worker's session");
        }
    }

    /// The folder every Claude worker of the conversation starts in (see `launch_admitted`),
    /// made on first use. It belongs to the conversation (owner `workers:<id>`): only its
    /// archive or deletion removes it, never one worker's end.
    pub(crate) async fn worker_home(&self, conversation_id: &ConversationId) -> Result<PathBuf> {
        let home = self.owned_dir("worker-home", &conversation_id.0);
        self.runtime
            .ledger()
            .record(
                &worker_home_owner(conversation_id),
                Artifact::ScratchDir {
                    path: home.to_string_lossy().into_owned(),
                },
            )
            .await?;
        let dir = home.clone();
        blocking(move || {
            std::fs::create_dir_all(&dir).map_err(|err| Error::Invalid(err.to_string()))
        })
        .await?;
        Ok(home)
    }

    /// The repository `task` of `conversation` works on: its session's. A Chat has none (Q76):
    /// its tasks, which never get a checkout, stand on the Chat's own folder.
    fn tasks_repo(&self, conversation: &Conversation, task: &Task) -> Result<PathBuf> {
        match &conversation.setup {
            Some(Setup::Session { repo, .. }) => Ok(PathBuf::from(repo)),
            Some(Setup::Chat { .. }) if task.access.repo == RepoAccess::None => {
                Ok(self.owned_dir("chat", &conversation.id.0))
            }
            _ => Err(Error::Invalid("tasks belong to a session".into())),
        }
    }

    /// Creates the task's scratch folder and worktree, recorded in the ledger first.
    async fn prepare_workspace(
        &self,
        owner: &str,
        task: &Task,
        subject: Option<&Task>,
    ) -> Result<Workspace> {
        let conversation = self.core.conversation(&task.conversation_id)?;
        let repo = self.tasks_repo(&conversation, task)?;
        let scratch = self
            .session_dir(&task.conversation_id)
            .worker(task)
            .join("tmp");
        self.prepare_owned_dir(owner, &scratch).await?;
        if let Some(plain) = &task.plain {
            return self
                .plain_workspace(owner, task, plain, &conversation, repo, scratch)
                .await;
        }
        if task.access.repo == RepoAccess::None {
            return Ok(Workspace {
                repo,
                worktree: None,
                branch: None,
                base: None,
                on_snapshot: false,
                target: None,
                scratch,
                warmed: Vec::new(),
                merge: None,
            });
        }
        // An overnight run's work lands on the run's branch, recorded with the task so it
        // stays there whatever happens to the run later.
        let run_branch = match &task.run {
            Some(run) => Some(self.run_target(&task.conversation_id, run).await?),
            None => None,
        };
        let Some(Setup::Session { environment, .. }) = &conversation.setup else {
            return Err(Error::Invalid("a Chat's tasks have no repository".into()));
        };
        let target = match &run_branch {
            Some(branch) => branch.clone(),
            None => {
                self.ensure_target(&task.conversation_id, &repo, environment)
                    .await?
            }
        };
        // Reviews and checks look at the candidate commit; a merge task continues from the
        // conflicting task's work (kept as a WIP commit on its branch) and merges the target
        // into it, or from the session branch and merges the base into it.
        let mut merge = None;
        let (base, start, on_snapshot) = match (task.kind, subject) {
            (TaskKind::Review | TaskKind::Verify, Some(subject)) if subject.candidate.is_some() => {
                let commit = Oid(subject
                    .candidate
                    .as_ref()
                    .map(|c| c.commit.clone())
                    .unwrap_or_default());
                (commit.clone(), commit, false)
            }
            (TaskKind::Merge, Some(subject)) => {
                let (onto, work, rebuilt) = self.merge_start(subject, &target).await?;
                merge = Some(crate::work::MergeInputs {
                    incoming: work.0.clone(),
                    label: format!("task-{}'s work", subject.number),
                    rebuilt: rebuilt
                        .into_iter()
                        .map(|(from, to)| crate::work::Rebuilt {
                            from: from.0,
                            to: to.0,
                        })
                        .collect(),
                });
                (onto, work, false)
            }
            // With no subject, the session's base merged into its branch.
            (TaskKind::Merge, None) => {
                let Environment::NewWorktree { base, .. } = environment else {
                    return Err(Error::Invalid(
                        "a merge task without subject needs a new-worktree session".into(),
                    ));
                };
                let (branch_tip, base_tip) = self.base_merge_start(&repo, &target, base).await?;
                merge = Some(crate::work::MergeInputs {
                    incoming: base_tip.0,
                    label: format!("`{base}`"),
                    rebuilt: Vec::new(),
                });
                (branch_tip.clone(), branch_tip, false)
            }
            // A review of a worker's work, or a phase's verifier, starts at the work's last
            // commit; the verifier's commits go on top of it. So does a fix of work that hasn't
            // landed: that work lands with it.
            (_, Some(subject))
                if task.kind == TaskKind::Review
                    || task.role == Some(WorkerRole::Verifier)
                    || (task.kind == TaskKind::Implement && continues_work(subject)) =>
            {
                self.work_head(subject).await?
            }
            // A fresh verifier of the thread's own work starts at its candidate commit; its
            // commits go on top of it.
            (_, None) if task.fresh_commit.is_some() => {
                let commit = Oid(task.fresh_commit.clone().unwrap_or_default());
                (commit.clone(), commit, false)
            }
            // A run's workers start from its branch's tip, never the user's uncommitted files.
            _ if run_branch.is_some() => {
                let (git, repo_path, branch) = (self.git.clone(), repo.clone(), target.clone());
                let tip = blocking(move || {
                    git.open(&repo_path)
                        .map_err(git_error)?
                        .branch_tip(&branch)
                        .map_err(git_error)?
                        .ok_or_else(|| Error::Invalid(format!("branch {branch} does not exist")))
                })
                .await?;
                (tip.clone(), tip, false)
            }
            _ => {
                let (base, on_snapshot) = self
                    .worker_base(&task.conversation_id, &repo, &target, true)
                    .await?;
                (base.clone(), base, on_snapshot)
            }
        };
        let worktree = self
            .session_dir(&task.conversation_id)
            .worker(task)
            .join("work");
        let branch = match task.kind.writes() {
            true => Some(task_branch(
                &self.branch_prefix(&task.conversation_id).await,
                &task.conversation_id,
                task.number,
                &task.title,
            )),
            false => None,
        };
        let spec = match &branch {
            Some(name) => WorktreeSpec::NewBranch {
                name: name.clone(),
                start: start.clone(),
            },
            None => WorktreeSpec::Detached { at: start.clone() },
        };
        // Workers that build or test start from copies of the checkout's dependency installs
        // and build caches (best effort; see `warm`).
        let warm = task.kind.writes() || matches!(task.kind, TaskKind::Review | TaskKind::Verify);
        let warmed = self
            .make_worktree(owner, &repo, &worktree, spec, warm, &task.id)
            .await?;
        if let Some(name) = &branch {
            self.note_branch_made(&repo, name).await;
        }
        Ok(Workspace {
            repo,
            worktree: Some(worktree),
            branch,
            base: Some(base),
            on_snapshot,
            target: Some(target),
            scratch,
            warmed,
            merge,
        })
    }

    /// Makes the worktree `path` of `repo` as `spec` says, recorded under `owner` first, and
    /// with `warm` copies the checkout's dependency installs and build caches into it (best
    /// effort; see `warm`). Returns the folders copied.
    pub(super) async fn make_worktree(
        &self,
        owner: &str,
        repo: &Path,
        path: &Path,
        spec: WorktreeSpec,
        warm: bool,
        task_id: &TaskId,
    ) -> Result<Vec<String>> {
        let ledger = self.runtime.ledger();
        ledger
            .record(
                owner,
                Artifact::Worktree {
                    repo: repo.to_string_lossy().into_owned(),
                    path: path.to_string_lossy().into_owned(),
                },
            )
            .await?;
        ledger
            .record(
                owner,
                Artifact::ProcessesIn {
                    dir: path.to_string_lossy().into_owned(),
                },
            )
            .await?;
        let (git, repo_path, path) = (self.git.clone(), repo.to_owned(), path.to_owned());
        let (platform, task_id) = (self.runtime.platform().clone(), task_id.clone());
        blocking(move || {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|err| Error::Invalid(err.to_string()))?;
            }
            let repo = git.open(&repo_path).map_err(git_error)?;
            repo.add_worktree(&path, spec).map_err(git_error)?;
            super::sidecars::stand_in_at(&git, &path);
            let mut copied = Vec::new();
            if warm {
                let started = std::time::Instant::now();
                match git.open(&path) {
                    Ok(worktree) => {
                        let installing = || warm::install_running(&*platform, repo.root());
                        let warmed =
                            warm::warm_worktree(&repo, &[repo_path], &worktree, &installing);
                        tracing::info!(
                            task = %task_id,
                            copied = ?warmed.copied,
                            skipped = ?warmed.skipped,
                            ms = started.elapsed().as_millis() as u64,
                            "warmed the task's worktree"
                        );
                        copied = warmed.copied;
                    }
                    Err(err) => {
                        tracing::warn!(task = %task_id, %err, "couldn't open the worktree to warm it");
                    }
                }
            }
            Ok(copied)
        })
        .await
    }

    /// Where a new-worktree session's worktree is made, each time it is: one path per session.
    pub(crate) fn session_worktree_path(
        &self,
        conversation: &crate::model::Conversation,
    ) -> PathBuf {
        self.session_dir(&conversation.id).work()
    }

    /// The branch accepted work lands on, created for a new-worktree session on first use
    /// (with the session's own worktree).
    pub(crate) async fn ensure_target(
        &self,
        conversation_id: &ConversationId,
        repo: &Path,
        environment: &Environment,
    ) -> Result<String> {
        match environment {
            Environment::LocalCheckout { branch } => Ok(branch.clone()),
            Environment::NewWorktree {
                base,
                branch,
                path,
                start,
            } => {
                if path.is_some() {
                    return Ok(branch.clone());
                }
                // Parallel first tasks: one creates the worktree, the others then find it.
                let _creating = self.session_worktrees.lock().await;
                let conversation = self.core.conversation(conversation_id)?;
                if let Some(Setup::Session {
                    environment: Environment::NewWorktree { path: Some(_), .. },
                    ..
                }) = &conversation.setup
                {
                    return Ok(branch.clone());
                }
                let owner = format!("session:{conversation_id}");
                // A `session:` owner still disposing here is an old post-merge removal; the
                // upgrade reconcile cancelled it (BUILD-PLAN A3), and nothing removes the
                // session's workspace on a merge any more.
                let start = start.clone();
                let worktree = self.session_worktree_path(&conversation);
                self.runtime
                    .ledger()
                    .record(
                        &owner,
                        Artifact::Worktree {
                            repo: repo.to_string_lossy().into_owned(),
                            path: worktree.to_string_lossy().into_owned(),
                        },
                    )
                    .await?;
                let (git, repo_path, path, base_name, name, from) = (
                    self.git.clone(),
                    repo.to_owned(),
                    worktree.clone(),
                    base.clone(),
                    branch.clone(),
                    start.clone(),
                );
                let branch_name = branch.clone();
                let platform = self.runtime.platform().clone();
                // A new branch starts from the newest base: its remote is fetched first.
                let fresh = match &start {
                    None => Some(self.fresh_tip(repo, base).await?),
                    Some(_) => None,
                };
                let newer = fresh.as_ref().and_then(|(_, gap)| gap.clone());
                let secret_files = conversation
                    .project_id
                    .as_ref()
                    .and_then(|id| self.core.project(id).ok())
                    .map(|project| project.prefs.secret_files)
                    .unwrap_or_default();
                let (made, warmed, copied, setup) = blocking(move || {
                    let repo = git.open(&repo_path).map_err(git_error)?;
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(|err| Error::Invalid(err.to_string()))?;
                    }
                    let (spec, made) = match repo.branch_tip(&name).map_err(git_error)? {
                        Some(_) => (WorktreeSpec::Branch { name }, None),
                        None => {
                            let start = match (&from, fresh) {
                                (Some(commit), _) => repo.resolve(commit),
                                (None, Some((tip, _))) => Ok(tip),
                                (None, None) => repo.branch_commit(&base_name),
                            }
                            .map_err(git_error)?;
                            (
                                WorktreeSpec::NewBranch {
                                    name,
                                    start: start.clone(),
                                },
                                Some(start),
                            )
                        }
                    };
                    repo.add_worktree(&path, spec).map_err(git_error)?;
                    // What the session's branch merges into, for `gh pr create` and whoever
                    // reads the target later (S7): the base chosen on the rail.
                    if let Err(err) = repo.set_merge_base(&branch_name, &base_name) {
                        tracing::warn!(%err, "couldn't record the session branch's base in git config");
                    }
                    super::sidecars::stand_in_at(&git, &path);
                    // A new branch's worktree gets the checkout's gitignored env files (and
                    // what `.worktreeinclude` names), so the app runs here as it does there.
                    let copied = match made {
                        Some(_) => super::secrets::copy_into(&git, &repo_path, &path, &secret_files)
                            .map(|(copied, _)| copied)
                            .unwrap_or_else(|err| {
                                tracing::warn!(worktree = %path.display(), %err, "couldn't copy the checkout's env files in");
                                Vec::new()
                            }),
                        None => Vec::new(),
                    };
                    let setup = made.is_some() && has_setup(&git, &path);
                    // The thread builds and tests here as a worker does in its own worktree,
                    // so it starts from the same copies of the checkout's dependency installs
                    // and build caches (best effort; see `warm`): without them its first check
                    // fails and it reinstalls them (BUILD-PLAN C7).
                    let started = std::time::Instant::now();
                    let warmed = match git.open(&path) {
                        Ok(worktree) => {
                            let installing = || warm::install_running(&*platform, repo.root());
                            let warmed =
                                warm::warm_worktree(&repo, &[repo_path], &worktree, &installing);
                            tracing::info!(
                                worktree = %path.display(),
                                copied = ?warmed.copied,
                                skipped = ?warmed.skipped,
                                ms = started.elapsed().as_millis() as u64,
                                "warmed the session's worktree"
                            );
                            warmed.copied
                        }
                        Err(err) => {
                            tracing::warn!(worktree = %path.display(), %err, "couldn't open the session's worktree to warm it");
                            Vec::new()
                        }
                    };
                    Ok((made, warmed, copied, setup))
                })
                .await?;
                if !copied.is_empty()
                    && let Ok(conv) = self.conv(conversation_id)
                {
                    conv.note(format!(
                        "[setup] {}",
                        super::prompts::copied_files_line(&copied)
                    ))
                    .await;
                }
                if !warmed.is_empty()
                    && let Ok(conv) = self.conv(conversation_id)
                {
                    conv.note(format!(
                        "[workspace] {}",
                        super::prompts::copied_in_line(&warmed)
                    ))
                    .await;
                }
                if made.is_some()
                    && let Some(gap) = &newer
                    && let Ok(conv) = self.conv(conversation_id)
                {
                    conv.note(super::fresh_base::started_from_upstream_line(base, gap))
                        .await;
                }
                // A branch that was there already (the user's, or one a restored session
                // made before) is only Brigadier's when its records say so.
                if made.is_some() {
                    self.note_branch_made(repo, branch).await;
                }
                // A session's worktree made again (after a restore, or the thread removed it):
                // the thread hears it is new and its own, or it takes it for a leftover and
                // removes it (bug 5). After Close workspace it heard so already (#39).
                let closed = self.conv(conversation_id).is_ok_and(|conv| {
                    conv.workspace_closed
                        .swap(false, std::sync::atomic::Ordering::SeqCst)
                });
                if let Some(start) = made
                    && !closed
                    && let Ok(board) = self.core.board(conversation_id).await
                    && board
                        .orchestrator_steps
                        .iter()
                        .any(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
                    && let Ok(conv) = self.conv(conversation_id)
                {
                    conv.note(format!(
                        "[workspace] A fresh branch `{branch}` from `{base}` at {} was made for this message, in the same folder {}. The earlier one is gone; this one is new and yours: work in it, and never remove it.",
                        &start.0[..start.0.len().min(10)],
                        worktree.display(),
                    ))
                    .await;
                }
                if let Some(Setup::Session {
                    repo,
                    permission,
                    orchestrator,
                    workers_see_uncommitted,
                    plan_mode,
                    ..
                }) = conversation.setup
                {
                    self.core
                        .set_setup(
                            conversation_id.clone(),
                            Setup::Session {
                                repo,
                                environment: Environment::NewWorktree {
                                    base: base.clone(),
                                    branch: branch.clone(),
                                    path: Some(worktree.to_string_lossy().into_owned()),
                                    start: start.clone(),
                                },
                                permission,
                                orchestrator,
                                workers_see_uncommitted,
                                plan_mode,
                            },
                        )
                        .await?;
                }
                // The repository's setup script runs once in the new worktree before the
                // thread's first turn, under the thread's access; never in a folder the user
                // doesn't trust. Other sessions' worktrees needn't wait for it.
                if setup && self.repo_trust(conversation_id) == Some(true) {
                    drop(_creating);
                    let line = self.run_thread_setup(conversation_id, &worktree).await;
                    if let Ok(conv) = self.conv(conversation_id) {
                        conv.note(format!("[setup] {line}")).await;
                    }
                }
                Ok(branch.clone())
            }
        }
    }

    /// Runs the setup script in the thread's new `worktree`.
    async fn run_thread_setup(&self, id: &ConversationId, worktree: &Path) -> String {
        let tmp = match self.thread_temp(id).await {
            Ok(tmp) => tmp,
            Err(err) => {
                tracing::warn!(conversation = %id, %err, "can't run the setup script; the thread runs it");
                return super::prompts::SETUP_LINE.into();
            }
        };
        // The user's Stop ends it.
        let stoppable = self.stoppable(id, "setup");
        self.run_setup_script(
            id,
            &super::setup_script::setup_owner(id),
            worktree,
            &worker_env(&tmp),
            stoppable.cancel.clone(),
        )
        .await
    }

    /// Where workers start: the target's tip, or a snapshot of the user's uncommitted changes
    /// on top of it when the user chose to show them (local checkout only; then `true`).
    /// Without `may_ask`, a choice the user hasn't made yet is an error instead of a question.
    pub(super) async fn worker_base(
        &self,
        conversation_id: &ConversationId,
        repo: &Path,
        target: &str,
        may_ask: bool,
    ) -> Result<(Oid, bool)> {
        // A worker starts from the newest target: its remote is fetched first.
        let (git, repo_path, branch) = (self.git.clone(), repo.to_owned(), target.to_owned());
        let exists = blocking(move || {
            Ok(git
                .open(&repo_path)
                .map_err(git_error)?
                .branch_tip(&branch)
                .map_err(git_error)?
                .is_some())
        })
        .await?;
        if !exists {
            return Err(Error::Invalid(format!("branch {target} does not exist")));
        }
        let (tip, _) = self.fresh_tip(repo, target).await?;
        let (git, repo_path) = (self.git.clone(), repo.to_owned());
        let (dirty, current) = blocking(move || {
            let state = git
                .open(&repo_path)
                .map_err(git_error)?
                .state()
                .map_err(git_error)?;
            Ok((state.dirty_files, state.current_branch))
        })
        .await?;
        let conversation = self.core.conversation(conversation_id)?;
        let Some(Setup::Session {
            environment: Environment::LocalCheckout { branch },
            workers_see_uncommitted,
            ..
        }) = conversation.setup
        else {
            return Ok((tip, false));
        };
        // Uncommitted changes matter only when they sit on the target branch.
        if dirty.is_empty() || current.as_deref() != Some(branch.as_str()) {
            return Ok((tip, false));
        }
        let see = match workers_see_uncommitted {
            Some(see) => see,
            None if may_ask => {
                self.ask_about_uncommitted(conversation_id, repo, dirty)
                    .await?
            }
            None => {
                return Err(Error::Invalid(
                    "the user hasn't said whether workers see uncommitted changes".into(),
                ));
            }
        };
        if !see {
            return Ok((tip, false));
        }
        let (git, repo_path) = (self.git.clone(), repo.to_owned());
        blocking(move || {
            let repo = git.open(&repo_path).map_err(git_error)?;
            Ok(repo
                .snapshot_uncommitted()
                .map_err(git_error)?
                .map(|snapshot| (snapshot.commit, true))
                .unwrap_or((tip, false)))
        })
        .await
    }

    /// Asks once whether workers see the checkout's uncommitted changes; tasks wait for it.
    /// The user's own are told apart from those the thread's edits left (#88).
    async fn ask_about_uncommitted(
        &self,
        conversation_id: &ConversationId,
        repo: &Path,
        files: Vec<String>,
    ) -> Result<bool> {
        // Only one card at a time: later tasks wait for the same answer.
        let asking = self.uncommitted_asks.lock().await;
        // A card answered since this task looked: its answer holds for this task too (#461).
        if let Some(Setup::Session {
            workers_see_uncommitted: Some(see),
            ..
        }) = self.core.conversation(conversation_id)?.setup
        {
            return Ok(see);
        }
        let board = self.core.board(conversation_id).await?;
        let open = board.questions.values().find(|q| {
            q.answer.is_none() && matches!(q.kind, QuestionKind::UncommittedChanges { .. })
        });
        let rx = match open {
            Some(question) => self.waiters_for_question(question.id.clone()),
            None => {
                let threads = self
                    .threads_uncommitted(conversation_id, repo, &files)
                    .await;
                let (thread_files, yours): (Vec<String>, Vec<String>) = files
                    .iter()
                    .cloned()
                    .partition(|file| threads.contains(file));
                let changes =
                    |n: usize| format!("{n} uncommitted change{}", if n == 1 { "" } else { "s" });
                let whose = match (yours.len(), thread_files.len()) {
                    (_, 0) => changes(yours.len()),
                    (0, theirs) => format!("{}, all made by the thread", changes(theirs)),
                    (mine, theirs) => format!(
                        "{}: {mine} of yours and {theirs} made by the thread",
                        changes(mine + theirs)
                    ),
                };
                let (_, rx) = self
                    .open_question(
                        conversation_id,
                        None,
                        QuestionKind::UncommittedChanges {
                            files: yours.into_iter().take(50).collect(),
                            thread_files: thread_files.into_iter().take(50).collect(),
                        },
                        format!(
                            "Your checkout has {whose}. Should workers see them? They are never committed either way."
                        ),
                        vec!["Yes, show them to workers".into(), "No, use the last commit".into()],
                        None,
                    )
                    .await?;
                rx
            }
        };
        drop(asking);
        rx.await.map_err(|_| {
            Error::Invalid("the question about uncommitted changes was withdrawn".into())
        })?;
        let conversation = self.core.conversation(conversation_id)?;
        Ok(matches!(
            conversation.setup,
            Some(Setup::Session {
                workers_see_uncommitted: Some(true),
                ..
            })
        ))
    }

    fn waiters_for_question(
        &self,
        id: crate::work::CardId,
    ) -> oneshot::Receiver<super::cards::CardAnswer> {
        self.waiters.register(id)
    }

    /// The user's attachments the task was given, written into its scratch folder.
    pub(crate) async fn worker_files(&self, task: &Task, scratch: &Path) -> Vec<InputFile> {
        let mut files = Vec::new();
        for attachment in &task.attachments {
            let Ok(hash) = attachment.id.parse::<brigadier_store::BlobHash>() else {
                continue;
            };
            let Ok(Some(bytes)) = self.core.store().blobs().get(hash).await else {
                continue;
            };
            let dir = scratch.join("attachments");
            let name = safe_file_name(&attachment.name);
            let path = dir.join(&name);
            // `message_worker` writes here while the worker runs: never through its links.
            let written =
                blocking(move || super::outputs::write_unfollowed(&dir, &name, &bytes)).await;
            if written.is_ok() {
                files.push(InputFile {
                    path,
                    name: attachment.name.clone(),
                    mime: attachment.mime.clone(),
                });
            }
        }
        files
    }

    /// Stores a worker's events on `task:<id>`, answering its approvals on the way.
    async fn pump_worker(
        self: Arc<Self>,
        live: Arc<TaskLive>,
        cli: Arc<Cli>,
        mut events: mpsc::Receiver<ProviderEvent>,
    ) {
        let mut deltas: Vec<ProviderEvent> = Vec::new();
        let mut deadline: Option<tokio::time::Instant> = None;
        // How its CLI ended, once it did.
        let mut exit: Option<String> = None;
        loop {
            let flush_at = async {
                match deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                event = events.recv() => match event {
                    // It only shows the worker is alive, or how many background jobs it runs:
                    // never stored.
                    Some(event @ (ProviderEvent::Progress { .. } | ProviderEvent::BackgroundJobs { .. })) => {
                        live.note_event(&cli, &event).await;
                        self.note_worker_cli(&live.conversation_id, &live.id, &event);
                    }
                    // Kept on the conversation's board while the CLI runs, never in its log.
                    Some(ProviderEvent::McpServers { servers, at_start }) => {
                        self.agent_mcp_servers(&live.conversation_id, Some(&live.id), cli.provider, McpUpdate::All { servers, at_start }).await;
                    }
                    Some(ProviderEvent::McpServerChanged { server }) => {
                        self.agent_mcp_servers(&live.conversation_id, Some(&live.id), cli.provider, McpUpdate::One(server)).await;
                    }
                    // Only the thread's reads are kept (`super::reads`); the call that made it
                    // is stored as itself.
                    Some(ProviderEvent::Looked { .. }) => {}
                    Some(event) if is_delta(&event) => {
                        live.note_event(&cli, &event).await;
                        self.note_worker_cli(&live.conversation_id, &live.id, &event);
                        merge_delta(&mut deltas, event);
                        deadline.get_or_insert_with(|| tokio::time::Instant::now() + DELTA_WINDOW);
                    }
                    Some(mut event) => {
                        live.note_event(&cli, &event).await;
                        self.note_worker_cli(&live.conversation_id, &live.id, &event);
                        deadline = None;
                        self.record_worker_events(&live.id, std::mem::take(&mut deltas)).await;
                        // A CLI that starts (again) tells its MCP servers anew.
                        if matches!(event, ProviderEvent::SessionStarted { .. }) {
                            self.agent_mcp_servers(&live.conversation_id, Some(&live.id), cli.provider, McpUpdate::Ended).await;
                        }
                        // Stored whole before it is recorded cut (BUILD-PLAN D2.1).
                        self.keep_output(&mut event).await;
                        let mut exited = false;
                        for event in self.session_events(EventSource::Task(&live.id), &cli, event) {
                            if let ProviderEvent::Exited {
                                code,
                                signal,
                                stderr_tail,
                            } = &event
                            {
                                exited = true;
                                exit = Some(super::worker_resume::exit_words(
                                    *code,
                                    *signal,
                                    stderr_tail.as_deref(),
                                ));
                            }
                            self.on_worker_event(&live, &cli, event).await;
                        }
                        if exited {
                            break;
                        }
                    }
                    None => break,
                },
                () = flush_at => {
                    deadline = None;
                    self.record_worker_events(&live.id, std::mem::take(&mut deltas)).await;
                }
            }
        }
        self.record_worker_events(&live.id, deltas).await;
        self.grants.revoke_owner(&cli.owner);
        let (stopping, from, replaced) = {
            let mut state = live.state.lock().await;
            if state.cli.as_ref().is_some_and(|c| Arc::ptr_eq(c, &cli)) {
                state.cli = None;
            }
            state.busy = false;
            state.question = None;
            (state.stopping, state.generation, state.cli.is_some())
        };
        cli.ended.cancel();
        // A fresh session that already took the task over keeps its run's worker slot, and
        // tells its own MCP servers.
        if !replaced {
            self.release_run_task(&live.id);
            self.agent_mcp_servers(
                &live.conversation_id,
                Some(&live.id),
                cli.provider,
                McpUpdate::Ended,
            )
            .await;
        }
        if !stopping
            && let Ok(task) = self.task_by_id(&live.conversation_id, &live.id).await
            && !task.state.is_final()
            && task.report.is_none()
        {
            let manager = self.arc();
            let live = live.clone();
            if task.plain.is_some() {
                let exit = exit.unwrap_or_else(|| "its CLI ended".to_owned());
                self.spawn(async move { manager.plain_gone(&task, &exit).await });
                return;
            }
            match live.take_cutoff().await {
                // Its model was cut off (a limit, a logout, an outage): another takes over.
                Some(end) => self.spawn(async move { manager.hand_off(&live, end, from).await }),
                // It crashed or was killed: the same session resumes (SESSION-FIXES-PLAN.md Q4).
                None => {
                    let exit = exit.unwrap_or_else(|| "its CLI ended".to_owned());
                    self.spawn(async move { manager.worker_died(&live, exit, from).await });
                }
            }
        }
    }

    async fn on_worker_event(&self, live: &Arc<TaskLive>, cli: &Arc<Cli>, event: ProviderEvent) {
        match &event {
            ProviderEvent::ApprovalRequested { request } => {
                let request = request.clone();
                self.record_worker_event(&live.id, event).await;
                self.route_worker_approval(live, cli, request).await;
                return;
            }
            ProviderEvent::RateLimits { quota } => {
                self.runtime
                    .note_quota_snapshot(&cli.account, quota.clone())
                    .await;
            }
            ProviderEvent::Usage { total, last, calls } => {
                self.note_tokens(
                    &cli.meter,
                    cli.provider,
                    cli.model.model.as_deref(),
                    TokenOwner::Task(&live.conversation_id, &live.id),
                    total,
                    last.as_ref(),
                    calls,
                )
                .await;
            }
            // A plain worker's CLI keeps its own context, as it would in a terminal.
            ProviderEvent::ContextSize { .. } if live.plain => {}
            ProviderEvent::CompactionStarted { .. } if live.plain => {
                self.plain_fresh_context(live);
            }
            ProviderEvent::ContextSize {
                used_tokens,
                window_tokens,
            } => {
                let window =
                    self.model_window(cli.provider, cli.model.model.as_deref(), *window_tokens);
                self.worker_context(live, cli, *used_tokens, window).await;
            }
            // Its CLI's own compaction was refused: a fresh session takes over (BUILD-PLAN D2.3).
            ProviderEvent::CompactionRefused => {
                self.force_worker_handoff(
                    live,
                    super::worker_handoff::NearLimit::CompactionRefused,
                )
                .await;
            }
            // A worker is never compacted by its CLI: one that is anyway says so in its
            // transcript, where the long-task bench counts it.
            ProviderEvent::CompactionStarted { automatic } => {
                tracing::warn!(task = %live.id, automatic, "a worker's CLI compacted its context");
                self.record_worker_event(
                    &live.id,
                    ProviderEvent::Notice {
                        level: brigadier_providers::NoticeLevel::Warning,
                        message: format!(
                            "Its {} CLI compacted its context, which Brigadier never asks for.",
                            cli.provider
                        ),
                    },
                )
                .await;
            }
            ProviderEvent::TurnStarted { .. } => {
                cli.meter.turn_started(now_ms());
                live.state.lock().await.last_message = None;
            }
            ProviderEvent::SessionStarted {
                native_id,
                cli_version,
                file,
                ..
            } => {
                cli.meter.session_started(cli_version.as_deref());
                self.note_native_session(&live.conversation_id, &live.id, native_id)
                    .await;
                // Its thread is its CLI's own file, as a session's is (S14).
                if let Ok(task) = self.task_by_id(&live.conversation_id, &live.id).await {
                    let record = self
                        .session_dir(&live.conversation_id)
                        .worker(&task)
                        .join("record");
                    let entry = super::session_dir::LineageEntry {
                        provider: cli.provider,
                        session_id: native_id.clone(),
                        file: file.clone(),
                        started_at: now_ms(),
                    };
                    if let Err(err) = super::session_dir::note_lineage_at(record, entry).await {
                        tracing::warn!(task = %live.id, error = %err, "could not note the worker's CLI session");
                    }
                }
            }
            ProviderEvent::Message {
                role: brigadier_providers::Role::Assistant,
                text,
                ..
            } => {
                live.state.lock().await.last_message = Some(text.clone());
            }
            ProviderEvent::Image {
                status: brigadier_providers::ItemStatus::Completed,
                path: Some(path),
                ..
            } => {
                let outputs = live.state.lock().await.outputs.clone();
                if let Some(outputs) = outputs {
                    self.keep_generated_image(outputs, PathBuf::from(path))
                        .await;
                }
            }
            _ => {}
        }
        let finished_call = matches!(
            &event,
            ProviderEvent::ToolCall { status, .. } | ProviderEvent::Command { status, .. }
                if *status != brigadier_providers::ItemStatus::InProgress
        );
        let completed = match &event {
            ProviderEvent::TurnCompleted { status, .. } => Some(*status),
            _ => None,
        };
        let error = match &event {
            ProviderEvent::Error { error } if !error.will_retry => {
                match fallback::verdict(error) {
                    // Another model takes over when the turn ends; this is not the task's
                    // error.
                    fallback::ErrorVerdict::HandOff(end) => {
                        live.set_cutoff(end).await;
                        None
                    }
                    fallback::ErrorVerdict::Transient => {
                        if live.transient_error().await >= 2 {
                            live.set_cutoff(AttemptEnd::Error {
                                kind: error.kind,
                                message: error.message.clone(),
                            })
                            .await;
                            None
                        } else {
                            Some(error.message.clone())
                        }
                    }
                    fallback::ErrorVerdict::Keep => Some(error.message.clone()),
                }
            }
            _ => None,
        };
        self.record_worker_event(&live.id, event).await;
        if let Some(message) = error {
            self.update_task(&live.conversation_id, &live.id, |task| {
                task.error = Some(message);
            })
            .await
            .ok();
        }
        if finished_call {
            self.note_worker_call(live).await;
        }
        if let Some(status) = completed {
            self.save_worker_calls(live).await;
            // What its Codex auto-reviews used, which its own totals leave out.
            self.meter_child_threads(cli, TokenOwner::Task(&live.conversation_id, &live.id))
                .await;
            self.worker_turn_completed(live, cli, status).await;
            // No turn followed (a nudge or retry would have started one): an overnight run's
            // worker slot is free for another task.
            if !live.holds_run_slot().await {
                self.release_run_task(&live.id);
            }
        }
    }

    async fn worker_turn_completed(
        &self,
        live: &Arc<TaskLive>,
        cli: &Arc<Cli>,
        status: TurnStatus,
    ) {
        // Taken before the turn counts as over: see `TaskLive::turn_over`.
        let _end = live.turn_end.lock().await;
        let (nudge, handoff_asked, from, background) = {
            let mut state = live.state.lock().await;
            state.busy = false;
            let handoff_asked = std::mem::take(&mut state.handoff_asked);
            state.steered_at = None;
            if state.stopping {
                return;
            }
            // Its question waits for an answer, which starts its next turn (BUILD-PLAN E2).
            if state.question.is_some() {
                return;
            }
            let background = state.background_jobs > 0 && !state.background_overdue;
            // Waiting for its own background jobs is no turn without a report: the nudge is
            // kept for one.
            let nudge = !background && !state.nudged;
            if !background {
                state.nudged = true;
            }
            (nudge, handoff_asked, state.generation, background)
        };
        let Ok(task) = self.task_by_id(&live.conversation_id, &live.id).await else {
            return;
        };
        if task.plain.is_some() {
            if !task.state.is_final() {
                self.plain_turn_ended(live, &task, status).await;
            }
            return;
        }
        if task.state == TaskState::Paused || task.state.is_final() {
            return;
        }
        let cutoff = live.take_cutoff().await;
        if task.report.is_some() && task.state != TaskState::Running {
            // Reported; the worker waits (write tasks can be sent back to fix things). What it
            // wrote after its report reaches the orchestrator first.
            let task = self.late_findings(live, task).await;
            // A write task waits for its landing, unless it changed nothing that could land.
            if !task.kind.writes() || self.changed_nothing(&task).await {
                // Not from inside the worker's own event pump: closing waits for it.
                let manager = self.arc();
                self.spawn(async move { manager.finish_read_task(&task).await });
            }
            return;
        }
        if let Some(end) = cutoff {
            // Its model is cut off (a limit, or an error another model may not have): another
            // takes the task over. Before the interrupted-turn return, since an injected limit
            // ends the turn that way. The CLI exiting now is part of this hand-off, not a
            // reason for another.
            live.state.lock().await.stopping = true;
            let manager = self.arc();
            let live = live.clone();
            self.spawn(async move { manager.hand_off(&live, end, from).await });
            return;
        }
        if status == TurnStatus::Interrupted {
            return;
        }
        if handoff_asked && status == TurnStatus::Completed {
            // It ended its turn with a handoff note, as asked: a fresh session takes over.
            // Not from inside the worker's own event pump: closing waits for it.
            live.clear_transient().await;
            let note = live.state.lock().await.last_message.clone();
            let manager = self.arc();
            let live = live.clone();
            self.spawn(async move {
                if let Err(err) = manager
                    .hand_over_worker(&live, &task, from, note, None, Handover::Size)
                    .await
                {
                    let reason = format!("The task could not continue in a fresh session: {err}");
                    manager.worker_failed(&task, &reason).await;
                }
            });
            return;
        }
        if status == TurnStatus::Completed {
            live.clear_transient().await;
        } else if task.error.is_some() && live.transient().await == 1 {
            // A transient provider error: the same model tries once more.
            let cleared = self
                .update_task(&task.conversation_id, &task.id, |task| task.error = None)
                .await;
            live.state.lock().await.begin_turn();
            let sent = cli
                .session
                .send(TurnInput::text("Continue the task."))
                .await;
            if cleared.is_ok() && sent.is_ok() {
                return;
            }
        }
        // A lead that sent its outline ends its turn to wait for the go-ahead.
        if Self::waits_for_go_ahead(&task) {
            return;
        }
        // Its own background job (a command or sub-agent) still runs: the job's end starts its
        // next turn, so it waits, unless it waits silently too long.
        if background && status == TurnStatus::Completed {
            live.state.lock().await.background_waiting = true;
            self.watch_background_wait(live, cli, from);
            return;
        }
        if nudge {
            let text = match self.keep_last_message(live, task.number).await {
                Some(_) => {
                    "You ended your turn without calling submit_report. The thread reads only your report, never your messages, so your last message has not reached it. Brigadier kept it and attaches it to your report as an artifact. If the task is done or you cannot continue, call submit_report now with a short summary; otherwise continue working."
                }
                None => {
                    "You ended your turn without calling submit_report. The thread reads only your report, never your messages. If the task is done or you cannot continue, call submit_report now; otherwise continue working."
                }
            };
            live.state.lock().await.begin_turn();
            let sent = cli.session.send(TurnInput::text(text)).await;
            if sent.is_ok() {
                return;
            }
        }
        let reason = task
            .error
            .clone()
            .unwrap_or_else(|| "The worker stopped without a report.".into());
        let manager = self.arc();
        self.spawn(async move { manager.worker_failed(&task, &reason).await });
    }

    /// A worker waits for its own background jobs after a turn that ended without a report.
    /// Once its CLI stays silent for [`BACKGROUND_SILENCE`] with no turn started, or its jobs
    /// end and no turn follows, the wait is over and the turn's end counts as one without a
    /// report (a nudge, then a failure), so a hung job is still caught.
    fn watch_background_wait(&self, live: &Arc<TaskLive>, cli: &Arc<Cli>, from: u64) {
        let manager = self.arc();
        let (live, cli) = (live.clone(), cli.clone());
        self.spawn(async move {
            // Silence is counted in checks, from the last event seen.
            let mut last_seen = None;
            let mut quiet = Duration::ZERO;
            // Checks since its jobs were gone without a turn starting.
            let mut gone = 0;
            loop {
                tokio::time::sleep(BACKGROUND_CHECK).await;
                {
                    let mut state = live.state.lock().await;
                    if state.generation != from || state.busy || state.stopping {
                        return;
                    }
                    if last_seen.replace(state.last_event_ms) == Some(state.last_event_ms) {
                        quiet += BACKGROUND_CHECK;
                    } else {
                        quiet = Duration::ZERO;
                    }
                    if state.background_jobs == 0 {
                        // The jobs ended, but no turn followed: it stopped without a report.
                        gone += 1;
                        if gone < 2 {
                            continue;
                        }
                    } else if quiet < BACKGROUND_SILENCE {
                        continue;
                    } else {
                        state.background_overdue = true;
                    }
                    state.background_waiting = false;
                }
                manager
                    .worker_turn_completed(&live, &cli, TurnStatus::Completed)
                    .await;
                if !live.holds_run_slot().await {
                    manager.release_run_task(&live.id);
                }
                return;
            }
        });
    }

    /// B7 for worker approvals: what the user already allowed "for this session" in this
    /// conversation goes; anything else asks the user (the CLI asks only what its level
    /// leaves to the user).
    async fn route_worker_approval(
        &self,
        live: &Arc<TaskLive>,
        cli: &Arc<Cli>,
        mut request: ApprovalRequest,
    ) {
        let worktree = live.state.lock().await.worktree.clone();
        if !self.allowed_before(&live.conversation_id, &mut request, worktree.as_deref()) {
            self.ask_user_for_worker(live, cli, request).await;
            return;
        }
        let decision = ApprovalDecision::Allow;
        if let Err(err) = cli
            .session
            .answer(request.id.clone(), decision.clone())
            .await
        {
            tracing::warn!(task = %live.id, error = %err, "could not answer an approval");
            return;
        }
        self.record_worker_resolution(&live.id, request.id, decision, Decider::User)
            .await;
    }

    /// The user's card for the worker CLI's request, while that CLI runs; the worker shows it
    /// waits on it.
    async fn ask_user_for_worker(&self, live: &TaskLive, cli: &Cli, request: ApprovalRequest) {
        let what = request
            .command
            .clone()
            .unwrap_or_else(|| request.tool.clone());
        match self
            .open_cli_card(cli, &live.conversation_id, Some(live.id.clone()), request)
            .await
        {
            Ok(Some(card)) if live.plain => self.plain_needs_input(live, &card, &what),
            Ok(Some(_)) => {}
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(task = %live.id, error = %err, "could not open an approval card");
                return;
            }
        }
        self.set_task_blocked(&live.id, Some(format!("Waiting for approval: {what}")))
            .await;
    }

    /// Whether the user already allowed something similar to a CLI's approval request (a
    /// worker's or the thread's) in the conversation, "for this session". A file change that
    /// asks, all inside the asker's own `workspace`, may be allowed for the session: its card
    /// offers that (`request.grant`).
    pub(crate) fn allowed_before(
        &self,
        conversation_id: &ConversationId,
        request: &mut ApprovalRequest,
        workspace: Option<&Path>,
    ) -> bool {
        if self
            .waiters
            .similar_allowed(conversation_id, request, workspace)
        {
            return true;
        }
        if request.kind == ApprovalKind::FileChange {
            request.grant =
                workspace.and_then(|workspace| policy::workspace_grant(request, workspace));
        }
        false
    }

    /// Passes the user's answer to the worker's CLI; "Allow … for this session" also allows
    /// similar requests from every worker of the conversation from now on.
    pub(crate) async fn answer_worker_approval(
        &self,
        task_id: &TaskId,
        request: &ApprovalRequest,
        decision: ApprovalDecision,
    ) -> Result<()> {
        let approval_id = request.id.clone();
        let live = self
            .existing_task_live(task_id)
            .ok_or_else(|| Error::Invalid("the worker has ended".into()))?;
        let cli = live
            .state
            .lock()
            .await
            .cli
            .clone()
            .ok_or_else(|| Error::Invalid("the worker has ended".into()))?;
        // The CLI's own "for this session" answer exists only for some requests; Brigadier
        // keeps the wider grant either way.
        let answer = match &decision {
            ApprovalDecision::AllowSimilar => ApprovalDecision::Allow,
            other => other.clone(),
        };
        cli.session
            .answer(approval_id.clone(), answer)
            .await
            .map_err(|err| Error::Provider(err.to_string()))?;
        if decision == ApprovalDecision::AllowSimilar {
            self.waiters.allow_similar(&live.conversation_id, request);
        }
        self.record_worker_resolution(task_id, approval_id, decision, Decider::User)
            .await;
        self.set_task_blocked(task_id, None).await;
        Ok(())
    }

    pub(crate) async fn record_worker_event(&self, task_id: &TaskId, event: ProviderEvent) {
        self.record_worker_events(task_id, vec![event]).await;
    }

    async fn record_worker_events(&self, task_id: &TaskId, events: Vec<ProviderEvent>) {
        if events.is_empty() {
            return;
        }
        let stream = streams::task(task_id);
        let events = events
            .into_iter()
            .map(|event| {
                (
                    stream.clone(),
                    DomainEvent::WorkerEvent {
                        task_id: task_id.clone(),
                        event,
                    },
                )
            })
            .collect();
        if let Err(err) = self.core.record(events).await {
            tracing::debug!(task = %task_id, error = %err, "could not store worker events");
        }
    }

    /// `ask_orchestrator`: blocks the worker until the orchestrator answers. When nobody does
    /// within the hour, an operational question goes on with its recorded safe default; any
    /// other keeps waiting, and its answer comes as a message (BUILD-PLAN E2).
    pub(crate) async fn worker_question(
        &self,
        conversation_id: &ConversationId,
        task_id: &TaskId,
        args: crate::tools::AskOrchestrator,
    ) -> Result<String> {
        let question = args.question;
        let live = self
            .existing_task_live(task_id)
            .ok_or_else(|| Error::Invalid("the task has ended".into()))?;
        let task = self.task_by_id(conversation_id, task_id).await?;
        // A gate member has no one to ask (see `Role::Worker`).
        if task.gate_link.is_some() {
            return Err(Error::Invalid(
                "You check this work on your own: decide from what you were given and your own evidence. Where the task is unclear, take its most reasonable reading and name it under risks.".into(),
            ));
        }
        let question = self.redact_for(&live, &question).await;
        let (tx, rx) = oneshot::channel();
        live.state.lock().await.question = Some((question.clone(), tx));
        self.set_task_blocked(task_id, Some(format!("{ASKED_THE_THREAD}{question}")))
            .await;
        self.deliver(
            conversation_id,
            Envelope {
                kind: InjectionKind::WorkerQuestion,
                label: format!("question from task-{}", task.number),
                task_id: Some(task_id.clone()),
                text: format!(
                    "[question from task-{} \"{}\"]\n{question}\n[/question] Answer it now with answer_worker (task-{}): the worker's recommendation if it fits, else what the brief implies; never reopen a settled decision. The worker waits.",
                    task.number, task.title, task.number
                ),
            },
        )
        .await;
        let timeout = Duration::from_millis(
            self.question_timeout_ms
                .load(std::sync::atomic::Ordering::Relaxed),
        );
        let answer = tokio::time::timeout(timeout, rx).await;
        match answer {
            Ok(Ok(answer)) => {
                self.set_task_blocked(task_id, None).await;
                Ok(answer)
            }
            Ok(Err(_)) => {
                self.set_task_blocked(task_id, None).await;
                Err(Error::Invalid(
                    "the question was withdrawn (the task ended)".into(),
                ))
            }
            Err(_) => match safe_default(args.about, &question, args.safe_default.as_deref()) {
                Some(default) => {
                    live.state.lock().await.question = None;
                    self.set_task_blocked(task_id, None).await;
                    self.record_answer(
                        &task,
                        question,
                        default.clone(),
                        "No answer came within the hour: it went on with the safe default it recorded.".into(),
                    )
                    .await;
                    Ok(format!(
                        "No answer came within the hour. Go on with your safe default: {default}. Name it under decisions in your report."
                    ))
                }
                None => {
                    // It keeps waiting: the answer comes as a message, and its turn may end.
                    let (tx, _) = oneshot::channel();
                    live.state.lock().await.question = Some((question.clone(), tx));
                    Ok("No answer yet, and this question waits for one: it is about consent or a product choice, or you gave no safe default. Don't go on with that choice. End your turn now; the answer starts your next one.".into())
                }
            },
        }
    }

    /// What a Claude worker's `Stop` hook answers (BUILD-PLAN E2): why its turn may not end
    /// yet, or `None` to let it. It refuses only a turn ending with no report, or with a
    /// hand-off note missing its parts, and only once per stop (`active`: the CLI already
    /// continued for this hook): the turn-end nudge covers what is left. A worker rightly
    /// waiting (its outline's go-ahead, its own review, its background jobs, its question) is
    /// never refused.
    pub(crate) async fn stop_decision(&self, task_id: &TaskId, active: bool) -> Option<String> {
        if active {
            return None;
        }
        let live = self.existing_task_live(task_id)?;
        let task = self.task_by_id(&live.conversation_id, task_id).await.ok()?;
        if task.state.is_final()
            || task.state == TaskState::Paused
            || task.report.is_some()
            || task.error.is_some()
            || Self::waits_for_go_ahead(&task)
        {
            return None;
        }
        let (handoff, note) = {
            let state = live.state.lock().await;
            if state.stopping
                || state.question.is_some()
                || (state.background_jobs > 0 && !state.background_overdue)
                || state.cutoff.is_some()
            {
                return None;
            }
            (state.handoff_asked, state.last_message.clone())
        };
        if handoff {
            let missing = handoff_gaps(note.as_deref().unwrap_or_default());
            return (!missing.is_empty()).then(|| {
                format!(
                    "Your handoff note is missing: {}. End your turn with the whole note, under each of its headings, so a fresh session can carry on.",
                    missing.join(", ")
                )
            });
        }
        Some("You are ending your turn without calling submit_report, and the thread reads only your report. If the task is done or you cannot continue, call submit_report now; otherwise keep working.".into())
    }

    /// The Stop hook of `grant`'s worker: `None` for a grant that isn't one.
    pub async fn stop_hook(&self, grant: &str, active: bool) -> Option<Option<String>> {
        let Some(Role::StopHook { task_id, .. }) = self.grants.resolve(grant) else {
            return None;
        };
        Some(self.stop_decision(&task_id, active).await)
    }

    /// Shortens how long a worker's question waits before its safe default may stand in (for
    /// the flow tests).
    #[cfg(test)]
    pub(crate) fn set_question_timeout(&self, timeout: Duration) {
        self.question_timeout_ms.store(
            timeout.as_millis() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// `answer_worker`: answers the question a worker waits on, and why.
    pub(crate) async fn answer_worker(
        &self,
        task: &Task,
        answer: String,
        why: String,
    ) -> Result<String> {
        let live = self
            .existing_task_live(&task.id)
            .ok_or_else(|| Error::Invalid(format!("task-{} has ended", task.number)))?;
        let Some((question, waiter)) = live.state.lock().await.question.take() else {
            return Err(Error::Invalid(format!(
                "task-{} isn't waiting on a question. To steer it, use message_worker.",
                task.number
            )));
        };
        if let Err(answer) = waiter.send(answer.clone()) {
            if live.cli().await.is_none() {
                // Its call ended with its CLI (Brigadier restarted): the answer resumes it.
                self.answer_after_restart(&live, task, &question, &answer)
                    .await?;
            } else {
                // A question that outlived its call: the answer comes as a message.
                self.set_task_blocked(&task.id, None).await;
                self.tell_worker(
                    &task.conversation_id,
                    task,
                    format!("[answer to your question] {question}\n{answer}"),
                    "Brigadier",
                )
                .await?;
            }
        }
        self.record_answer(task, question, answer, why).await;
        Ok(format!("Answered task-{}; it continues.", task.number))
    }

    /// After a restart, the thread hears again the question a worker asked before it: the
    /// worker's call ended with its CLI, so the answer resumes its session
    /// ([`Self::answer_after_restart`]) (BUILD-PLAN D1.4).
    pub(crate) async fn ask_again(&self, live: &Arc<TaskLive>, task: &Task, question: &str) {
        let (waiter, _) = oneshot::channel();
        live.state.lock().await.question = Some((question.to_owned(), waiter));
        self.deliver(
            &task.conversation_id,
            Envelope {
                kind: InjectionKind::WorkerQuestion,
                label: format!("question from task-{}", task.number),
                task_id: Some(task.id.clone()),
                text: format!(
                    "[question from task-{} \"{}\"]\n{question}\n[/question] It asked this before Brigadier restarted and still waits. Answer it now with answer_worker (task-{}): the worker's recommendation if it fits, else what the brief implies; never reopen a settled decision.",
                    task.number, task.title, task.number
                ),
            },
        )
        .await;
    }

    /// The answer to a question whose call ended (Brigadier restarted while the worker
    /// waited): its session resumes with the answer, and the task runs again. If it can't,
    /// the question stays open.
    async fn answer_after_restart(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        question: &str,
        answer: &str,
    ) -> Result<()> {
        let text = format!(
            "[Brigadier] Brigadier restarted while you waited. The thread's answer to your question \"{question}\":\n{answer}\nContinue the task."
        );
        // Boxed: a session's start is a big future, and answers come in from tool calls.
        if let Err(err) = crate::heap(|| self.revive_worker(live, task, text)).await {
            let (waiter, _) = oneshot::channel();
            live.state.lock().await.question = Some((question.to_owned(), waiter));
            return Err(err);
        }
        self.set_task_blocked(&task.id, None).await;
        Ok(())
    }

    /// Logs an answer to a worker's question: the thread's "Answered" row, and the ledger's
    /// decision (the morning report lists it).
    async fn record_answer(&self, task: &Task, question: String, answer: String, why: String) {
        let why = match why.trim() {
            "" => "steer".to_owned(),
            why => why.to_owned(),
        };
        self.orchestrator_step(
            &task.conversation_id,
            crate::work::OrchestratorStepKind::Answered {
                task_id: task.id.clone(),
                question,
                answer: answer.clone(),
                why: why.clone(),
            },
        )
        .await;
        let request = self
            .request_for(&task.conversation_id, Some(&task.id))
            .await;
        self.record_decision(
            &task.conversation_id,
            request,
            crate::work::DecisionSource::Task {
                task_id: task.id.clone(),
            },
            crate::work::DecisionKind::Answer,
            format!("Answered task-{}: {answer}", task.number),
            why,
        )
        .await;
    }

    /// `submit_report`: stores the worker's final report and hands it on.
    pub(crate) async fn worker_report(
        &self,
        conversation_id: &ConversationId,
        task_id: &TaskId,
        input: crate::tools::SubmitReport,
    ) -> Result<String> {
        let live = self
            .existing_task_live(task_id)
            .ok_or_else(|| Error::Invalid("the task has ended".into()))?;
        let task = self.task_by_id(conversation_id, task_id).await?;
        if task.state.is_final() {
            return Err(Error::Invalid("the task has already ended".into()));
        }
        // Its report stands until the orchestrator sends it back to work (message_worker):
        // a second one would overwrite it and wake the orchestrator for nothing.
        if !matches!(
            task.state,
            TaskState::Starting | TaskState::Running | TaskState::Blocked
        ) {
            return Err(Error::Invalid(format!(
                "Your report for task-{} was already received and stands; nothing was changed. A new report is taken only after the thread sends you back to work. End your turn now.",
                task.number
            )));
        }
        if self.held_by_plan_mode(&task).await {
            return Err(Error::Invalid(super::phases::PLAN_MODE_HOLD.into()));
        }
        // A new file no report names is left out of the landing, and a [met] with no evidence
        // is a claim: the report is taken, each marked for the thread to judge (#249).
        let unreported = self
            .unreported_files(&task, &input.changes)
            .await
            .unwrap_or_else(|err| {
                tracing::warn!(error = %err, "could not look for new files the report leaves out");
                Vec::new()
            });
        let unevidenced = crate::heap(|| self.unevidenced_met(&task, &input.done_when)).await;
        let mut input = input;
        let marks: Vec<String> = unreported
            .iter()
            .map(|path| format!("new file not named: `{path}` (left out of the landing)"))
            .collect();
        input.risks.splice(0..0, marks);
        // A criterion the report doesn't list goes under its risks: its "done when" lines stay
        // the worker's own, as the phase's acceptance reads them.
        for line in unevidenced {
            match input.done_when.iter_mut().find(|item| **item == line) {
                Some(item) => item.push_str(" [unevidenced]"),
                None => input.risks.push(format!("{line} [unevidenced]")),
            }
        }
        // A development build's planted `learned` line (the continuity battery's false claim).
        #[cfg(debug_assertions)]
        input.learned.extend(self.planted_learned(conversation_id));
        // Everything it names is stored now, before its folders can go.
        let texts: Vec<String> = std::iter::once(&input.summary)
            .chain(&input.decisions)
            .chain(&input.learned)
            .chain(&input.verification)
            .chain(&input.done_when)
            .chain(&input.open_questions)
            .chain(&input.risks)
            .chain(&input.needs_user)
            .cloned()
            .collect();
        let files = self
            .report_files(&task, live.redactor().await, &input.artifacts, texts)
            .await?;
        let mut artifacts = files.artifacts;
        let diff = self.worktree_diff(&task).await;
        // Nothing it could land: it ends once its turn is over, with nothing to accept.
        let unchanged = task.kind == TaskKind::Implement && diff.as_deref() == Some("");
        if let Some(diff) = self.diff_artifact(&live, &task, diff).await {
            artifacts.push(diff);
        }
        // What it wrote as a message instead: the report may only point at it.
        if let Some(message) = self.keep_last_message(&live, task.number).await {
            artifacts.push(message);
        }
        let outputs = files.outputs;
        // A gate member's report goes to its gate, not to the orchestrator.
        let reviewing = task.gate_link.is_some();
        // The report of a merge worker Brigadier sent back during its landing (its merge was
        // refused, or the target moved under it) lands on its own, unless the worker says only the user can unblock
        // it: then the orchestrator reads the report.
        let relanding = task.kind.writes()
            && task.landing.is_some()
            && input.needs_user.is_empty()
            && !unchanged;
        // Its work is committed before anyone reads or builds on it (a Codex worker can't
        // commit from its sandbox).
        if task.kind.writes() && !unchanged {
            let subject = input.summary.lines().next().unwrap_or_default().trim();
            let message = if subject.is_empty() {
                task.title.clone()
            } else {
                format!("{}\n\n{subject}", task.title)
            };
            // Uncommitted work would be left out of what the verifier checks and what lands:
            // the worker commits it first.
            if let Err(err) = self.commit_leftovers(&task, &input.changes, &message).await {
                return Err(Error::Invalid(format!(
                    "Your work could not be committed, so nothing was reported: {err}\nFix that and commit your work, then call submit_report again."
                )));
            }
        }
        let mut report = Report {
            summary: self.redact_for(&live, &input.summary).await,
            changes: input.changes.clone(),
            decisions: self.redact_all(&live, &input.decisions).await,
            learned: self.redact_all(&live, &input.learned).await,
            verification: self.redact_all(&live, &input.verification).await,
            done_when: self.redact_all(&live, &input.done_when).await,
            open_questions: self.redact_all(&live, &input.open_questions).await,
            risks: self.redact_all(&live, &input.risks).await,
            needs_user: self.redact_all(&live, &input.needs_user).await,
            deleted: self.redact_all(&live, &input.deleted).await,
            verdict: input.verdict,
            checks: input.checks.filter(|_| task.kind == TaskKind::Verify),
            artifacts,
            submitted_at_ms: now_ms(),
        };
        // A report over what the thread reads inline is taken whole: every field stays for
        // what reads it (landing, relanding, the Brain), the thread's envelope is bounded, and
        // the whole of it is kept as a file for read_artifact and the user.
        if prompts::report_size(&report) > REPORT_MAX_BYTES
            && let Some(full) = self.full_report_artifact(&task, &report).await
        {
            report.artifacts.insert(0, full);
        }
        let reported = |task: &mut Task| {
            task.report = Some(report.clone());
            // It reached a report: a later crash resumes it again.
            task.deaths.clear();
            task.outputs.clone_from(&outputs);
            task.state = TaskState::Reported;
            task.blocked_reason = None;
            if task.kind.writes() && !relanding {
                // The orchestrator decides about this report: Brigadier no longer lands it on
                // its own.
                task.landing = None;
            }
        };
        // A cheap scout or research task whose check failed reruns once, one tier up; the
        // thread hears of it with the report.
        let rerun = match super::reruns::failed_check(&report.done_when) {
            Some(line) if !reviewing && Self::reruns_by_itself(&task) => {
                let reason = format!("its check \"{line}\" failed");
                match crate::heap(|| self.rerun(&task, &reason)).await {
                    Ok(rerun) => Some(rerun),
                    Err(why) => {
                        tracing::info!(task = %task.id, why, "a cheap task's check failed; no rerun");
                        None
                    }
                }
            }
            _ => None,
        };
        let settled = live.settle.lock().await;
        // Stopped while its report was being stored: the stop stands.
        let task = self.task_by_id(conversation_id, task_id).await?;
        if task.state.is_final() {
            if let Some(rerun) = &rerun {
                let _ = self
                    .stop_worker(conversation_id, rerun, "Its task was stopped".into())
                    .await;
            }
            return Err(Error::Invalid("the task has already ended".into()));
        }
        // The report is in the orchestrator's inbox before the task counts as reported, so
        // its request never looks over in between.
        let queued = if reviewing || relanding {
            None
        } else {
            let mut shown = task.clone();
            reported(&mut shown);
            let mut text = prompts::report_envelope(&shown, &report, &route_label(&shown));
            if unchanged {
                text.push_str(&format!(
                    "\n[nothing to land task-{}] It changed no files, so it is done; there is nothing to land.",
                    task.number
                ));
            }
            if let Some(rerun) = &rerun {
                text.push('\n');
                text.push_str(&super::reruns::rerun_note(&shown, rerun));
            }
            if task.kind == TaskKind::Research {
                text.push('\n');
                text.push_str(prompts::RESEARCH_REPORT_NOTE);
            }
            let envelope = Envelope {
                kind: InjectionKind::Report,
                label: format!("report task-{}", task.number),
                task_id: Some(task.id.clone()),
                text,
            };
            let request = self.request_for(conversation_id, Some(task_id)).await;
            self.queue_envelope(conversation_id, envelope, request)
                .await
        };
        let task = self.update_task(conversation_id, task_id, reported).await?;
        drop(settled);
        live.state.lock().await.nudged = true;
        // What only the user can do is listed for them ("Waiting on you"); a later report
        // that no longer lists an item ends it. A gate member's go to its gate.
        if !reviewing {
            self.sync_waiting(
                &task,
                WaitingSource::Task {
                    task_id: task.id.clone(),
                },
                &report.needs_user,
            )
            .await;
        }
        // A write task's claims are knowledge only once its work lands (see `landed`).
        if !task.kind.writes() {
            self.learn_report(&task, &report, Some(live.learning.clone()));
        }
        if relanding && !reviewing {
            // Its commits land once its turn is over (fast-forward, or rebased and checked
            // once more if the branch moved again).
            let manager = self.arc();
            let conversation_id = conversation_id.clone();
            let live = live.clone();
            self.spawn(async move {
                live.turn_over().await;
                // A stop being recorded now is recorded first; one that came meanwhile, a
                // steer or a newer report took the task over, and nothing is landed for it.
                let _settled = live.settle.lock().await;
                let Ok(now) = manager.task_by_id(&conversation_id, &task.id).await else {
                    return;
                };
                if !relanding_pending(&now) || !same_report(&now, &task) {
                    return;
                }
                manager.land_after_rework(&now).await;
            });
        } else if let Some(conv) = queued {
            self.settle_requests(conversation_id).await;
            self.kick(&conv);
        }
        Ok("Report received. Your part is done: end your turn now.".into())
    }

    /// Where a check of `subject`'s work starts: its base, its worktree's last commit, and
    /// whether that base is a snapshot of the user's uncommitted files.
    async fn work_head(&self, subject: &Task) -> Result<(Oid, Oid, bool)> {
        let workspace = subject
            .workspace
            .clone()
            .ok_or_else(|| Error::Invalid(format!("task-{} has no workspace", subject.number)))?;
        let (Some(worktree), Some(base)) = (workspace.worktree, workspace.base) else {
            return Err(Error::Invalid(format!(
                "task-{} has no worktree",
                subject.number
            )));
        };
        let git = self.git.clone();
        let head = blocking(move || {
            git.open_worktree(Path::new(&worktree))
                .map_err(git_error)?
                .head()
                .map_err(git_error)
        })
        .await?;
        Ok((Oid(base), head, workspace.on_snapshot))
    }

    /// The worker's changes so far against its base, untracked files included (write tasks);
    /// `None` when its worktree can't be read.
    async fn worktree_diff(&self, task: &Task) -> Option<String> {
        let workspace = task.workspace.as_ref()?;
        if !task.kind.writes() {
            return None;
        }
        let (git, path, base) = (
            self.git.clone(),
            PathBuf::from(workspace.worktree.as_ref()?),
            Oid(workspace.base.clone()?),
        );
        blocking(move || {
            let worktree = git.open_worktree(&path).map_err(git_error)?;
            worktree.diff_from(&base).map_err(git_error)
        })
        .await
        .ok()
    }

    /// Whether an implement task's worktree holds no change at all, so nothing of it could
    /// land. A merge task always ends through its landing.
    pub(super) async fn changed_nothing(&self, task: &Task) -> bool {
        task.kind == TaskKind::Implement
            && self
                .worktree_diff(task)
                .await
                .is_some_and(|diff| diff.is_empty())
    }

    /// `diff` (the worker's changes so far, see [`Self::worktree_diff`]) as a diff artifact.
    async fn diff_artifact(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        diff: Option<String>,
    ) -> Option<ArtifactRef> {
        let diff = diff.filter(|diff| !diff.is_empty())?;
        let diff = self.redact_for(live, &diff).await;
        let bytes = diff.into_bytes();
        let size = bytes.len() as u64;
        let hash = self.core.store().blobs().put(bytes).await.ok()?;
        Some(ArtifactRef {
            id: hash.to_string(),
            title: format!("Diff of task-{}", task.number),
            kind: ArtifactKind::Diff,
            mime: "text/x-diff".into(),
            bytes: size,
            file_name: Some(format!("task-{}.diff", task.number)),
        })
    }

    async fn redact_all(&self, live: &Arc<TaskLive>, items: &[String]) -> Vec<String> {
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            out.push(self.redact_for(live, item).await);
        }
        out
    }

    pub(crate) async fn redact_for(&self, live: &Arc<TaskLive>, text: &str) -> String {
        match live.redactor().await {
            Some(redactor) => redactor.redact(text).into_owned(),
            None => text.to_owned(),
        }
    }

    /// The worker's last message of this turn, when it is long enough to hold what its report
    /// left out, stored as an artifact (the orchestrator never sees messages). Stored once;
    /// later calls return it again.
    async fn keep_last_message(&self, live: &Arc<TaskLive>, number: u32) -> Option<ArtifactRef> {
        let (message, kept) = {
            let mut state = live.state.lock().await;
            (state.last_message.take(), state.unsent.clone())
        };
        let Some(message) = message.filter(|text| text.trim().len() >= KEEP_MESSAGE_MIN_BYTES)
        else {
            return kept;
        };
        let bytes = self.redact_for(live, &message).await.into_bytes();
        let size = bytes.len() as u64;
        let hash = match self.core.store().blobs().put(bytes).await {
            Ok(hash) => hash,
            Err(err) => {
                tracing::warn!(task = %live.id, error = %err, "could not keep the worker's message");
                return kept;
            }
        };
        let artifact = ArtifactRef {
            id: hash.to_string(),
            title: "The worker's last message, which its report leaves out".into(),
            kind: ArtifactKind::Note,
            mime: "text/markdown".into(),
            bytes: size,
            file_name: Some(format!("task-{number}-message.md")),
        };
        live.state.lock().await.unsent = Some(artifact.clone());
        Some(artifact)
    }

    /// A long report kept whole as a file, from its fields as stored (already redacted).
    async fn full_report_artifact(&self, task: &Task, report: &Report) -> Option<ArtifactRef> {
        let text = prompts::report_text(task, report, &route_label(task));
        let bytes = text.into_bytes();
        let size = bytes.len() as u64;
        match self.core.store().blobs().put(bytes).await {
            Ok(hash) => Some(ArtifactRef {
                id: hash.to_string(),
                title: prompts::FULL_REPORT_TITLE.into(),
                kind: ArtifactKind::Note,
                mime: "text/markdown".into(),
                bytes: size,
                file_name: Some(format!("task-{}-report.md", task.number)),
            }),
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "could not keep a long report whole");
                None
            }
        }
    }

    /// What the worker wrote after its report, in the turn that reported: kept as an artifact
    /// of the report and sent to the orchestrator, when it is long enough to be findings or
    /// the report points at it ("the findings are below"). A review's or check's is left
    /// alone: whoever asked for it reads its report. Answers the task as it is now.
    async fn late_findings(&self, live: &Arc<TaskLive>, task: Task) -> Task {
        let message = live.state.lock().await.last_message.take();
        let (Some(message), Some(report)) = (message, task.report.as_ref()) else {
            return task;
        };
        let message = message.trim();
        if checks_a_change(&task) || !is_late_findings(&report.summary, message) {
            return task;
        }
        let text = self.redact_for(live, message).await;
        let bytes = text.clone().into_bytes();
        let size = bytes.len() as u64;
        let hash = match self.core.store().blobs().put(bytes).await {
            Ok(hash) => hash,
            Err(err) => {
                tracing::warn!(task = %live.id, error = %err, "could not keep the worker's late findings");
                return task;
            }
        };
        let artifact = ArtifactRef {
            id: hash.to_string(),
            title: "What the worker wrote after its report".into(),
            kind: ArtifactKind::Note,
            mime: "text/markdown".into(),
            bytes: size,
            file_name: Some(format!("task-{}-after-report.md", task.number)),
        };
        let shown = prompts::late_findings_text(&artifact, &text);
        // A read task's scratch folder goes as it ends, right after this.
        let scratch_gone = !task.kind.writes()
            || task
                .workspace
                .as_ref()
                .is_none_or(|w| !std::path::Path::new(&w.scratch).exists());
        let added = artifact.clone();
        let updated = self
            .update_task(&task.conversation_id, &task.id, move |task| {
                if let Some(report) = task.report.as_mut()
                    && !report.artifacts.iter().any(|kept| kept.id == added.id)
                {
                    report.artifacts.push(added);
                }
            })
            .await;
        let Ok(updated) = updated else {
            return task;
        };
        tracing::info!(task = %task.id, bytes = size, "kept what the worker wrote after its report");
        // Its report not read yet: the thread reads both in one turn, with no wake of its own.
        if self
            .fold_into_queued_report(
                &updated,
                &prompts::late_findings_folded(&updated, &shown, scratch_gone),
            )
            .await
        {
            if !updated.kind.writes()
                && let Some(report) = &updated.report
            {
                self.learn_report(&updated, report, Some(live.learning.clone()));
            }
            return updated;
        }
        let envelope = Envelope {
            kind: InjectionKind::Report,
            label: addendum_label(task.number),
            task_id: Some(task.id.clone()),
            text: prompts::late_findings_envelope(&updated, &shown, scratch_gone),
        };
        let request = self
            .request_for(&task.conversation_id, Some(&task.id))
            .await;
        if let Some(conv) = self
            .queue_envelope(&task.conversation_id, envelope, request)
            .await
        {
            self.settle_requests(&task.conversation_id).await;
            self.kick(&conv);
        }
        if !updated.kind.writes()
            && let Some(report) = &updated.report
        {
            self.learn_report(&updated, report, Some(live.learning.clone()));
        }
        updated
    }

    /// `message_worker`: answers a blocking question, steers a running worker, or sends a
    /// reported worker back to work. `from` names who speaks: the orchestrator, or Brigadier
    /// itself (a gate's findings). Returns the reply, and whether the text answered a question
    /// the worker was waiting on (`ask_orchestrator`).
    pub(crate) async fn message_worker(
        &self,
        conversation_id: &ConversationId,
        task: &Task,
        text: String,
        from: &str,
    ) -> Result<(String, bool)> {
        self.send_worker(conversation_id, task, text, from, true)
            .await
    }

    /// [`Self::message_worker`] for news that answers nothing (a review's outcome): a question
    /// the worker is waiting on stays open for the orchestrator.
    pub(crate) async fn tell_worker(
        &self,
        conversation_id: &ConversationId,
        task: &Task,
        text: String,
        from: &str,
    ) -> Result<(String, bool)> {
        self.send_worker(conversation_id, task, text, from, false)
            .await
    }

    async fn send_worker(
        &self,
        conversation_id: &ConversationId,
        task: &Task,
        text: String,
        from: &str,
        answers: bool,
    ) -> Result<(String, bool)> {
        drop(self.enter(conversation_id)?);
        // Open in the user's terminal: it waits for the report request.
        if let Some(held) = self.hold_for_terminal(task, &text).await {
            return Ok((held, false));
        }
        // An idle worker of an overnight run starts a turn only with a free worker slot: when
        // its run has none, the message waits for one rather than holding up the caller.
        if task.run.is_some()
            && !task.state.is_final()
            && !self.task_live(task).busy().await
            && let super::overnight::admission::Slot::Full { cap } =
                self.try_admit_run_task(task)?
        {
            let manager = self.arc();
            let (conversation_id, waiting, from) =
                (conversation_id.clone(), task.clone(), from.to_owned());
            self.spawn(async move {
                if manager.admit_run_task(&waiting).await.is_err() {
                    return;
                }
                let Ok(now) = manager.task_by_id(&conversation_id, &waiting.id).await else {
                    manager.release_run_task(&waiting.id);
                    return;
                };
                if let Err(err) = manager
                    .message_worker_admitted(&conversation_id, &now, text, &from, answers)
                    .await
                {
                    tracing::warn!(task = %now.id, error = %err, "could not deliver a message that waited for a worker");
                }
                manager.release_if_idle(&now).await;
            });
            return Ok((
                format!(
                    "task-{} waits for a free worker (the run works with at most {cap} at once); your message reaches it then.",
                    task.number
                ),
                false,
            ));
        }
        let sent = self
            .message_worker_admitted(conversation_id, task, text, from, answers)
            .await;
        self.release_if_idle(task).await;
        sent
    }

    /// [`Self::message_worker`], admitted.
    async fn message_worker_admitted(
        &self,
        conversation_id: &ConversationId,
        task: &Task,
        text: String,
        from: &str,
        answers: bool,
    ) -> Result<(String, bool)> {
        let live = self.task_live(task);
        // What the task may use now, for a worker between turns (routing reads the board, so
        // not under the worker's lock).
        let idle = {
            let state = live.state.lock().await;
            !state.busy && state.cli.is_some()
        };
        let models = if idle && !task.state.is_final() {
            Some(self.rules_models(task.route.choice.provider)?)
        } else {
            None
        };
        let mut state = live.state.lock().await;
        if answers && let Some((question, waiter)) = state.question.take() {
            drop(state);
            if waiter.send(text.clone()).is_err() {
                self.answer_after_restart(&live, task, &question, &text)
                    .await?;
            }
            // An answer, however it was sent: the thread shows it as one.
            self.record_answer(task, question, text, "steer".into())
                .await;
            return Ok((
                format!("Answered task-{}; it continues.", task.number),
                true,
            ));
        }
        if task.state.is_final() {
            return Err(Error::Invalid(format!("task-{} has ended", task.number)));
        }
        // Its model was cut off at a limit: the message waits for the model that takes the task
        // over (its hand-off carries every message), rather than reviving the one cut off.
        if let Some(wait) = &task.quota_wait {
            return Ok((
                format!(
                    "task-{} is waiting for quota ({}); the model that takes it over gets this \
                     message with its hand-off.",
                    task.number, wait.reason
                ),
                false,
            ));
        }
        // A plain worker's message goes as written, to the session its CLI keeps.
        let words = match live.plain {
            true => text.clone(),
            false => format!("Message from {from}:\n{text}"),
        };
        let hand_over = !live.plain
            && !state.busy
            && state.cli.is_some()
            && match state.context {
                Some(tokens) => self.worker_handoff_due(tokens, state.session_start, state.window),
                None => false,
            };
        let restart = !live.plain
            && !state.busy
            && state.cli.is_some()
            && models
                .as_ref()
                .is_some_and(|models| state.model_rules_changed(models));
        if hand_over || restart {
            // Between turns, with a context past the hand-off size: the message starts a fresh
            // session rather than one more large turn (PLAN.md §7). After the models the task
            // may use changed (a rule added since its session started), it resumes the session
            // in a new one that follows them. Taken under the lock, so no session started
            // meanwhile is closed.
            let cli = TaskLive::detach_cli(&mut state);
            drop(state);
            TaskLive::end_cli(cli).await;
            state = live.state.lock().await;
        }
        let Some(cli) = state.cli.clone() else {
            drop(state);
            self.revive_worker(&live, task, words).await?;
            return Ok((
                format!(
                    "task-{} is working on it; a new report will follow.",
                    task.number
                ),
                false,
            ));
        };
        let input = TurnInput::text(words);
        // A worker still in the turn that reported is sent back all the same: its next
        // report must be taken, and its end of turn must not finish the task. The task is
        // reopened before that turn can end (the end waits for this lock).
        let working = matches!(
            task.state,
            TaskState::Starting | TaskState::Running | TaskState::Blocked
        );
        if state.busy {
            cli.session
                .steer(input)
                .await
                .map_err(|err| Error::Provider(err.to_string()))?;
            if working {
                return Ok((
                    format!("Sent to task-{} (it is working).", task.number),
                    false,
                ));
            }
        } else {
            state.begin_turn();
            cli.session
                .send(input)
                .await
                .map_err(|err| Error::Provider(err.to_string()))?;
        }
        state.nudged = false;
        self.reopen_task(conversation_id, task).await?;
        drop(state);
        Ok((
            format!(
                "task-{} is working on it; a new report will follow.",
                task.number
            ),
            false,
        ))
    }

    /// A reported task goes back to work: its candidate and review are void, and its request
    /// works again until the new report is answered.
    async fn reopen_task(&self, conversation_id: &ConversationId, task: &Task) -> Result<()> {
        self.update_task(conversation_id, &task.id, |task| {
            task.state = TaskState::Running;
            task.candidate = None;
            task.rework_rounds += 1;
        })
        .await?;
        Ok(())
    }

    /// Stops a worker for good (`stop_worker`, or the user's stop button). Unfinished changes
    /// are kept on the task branch as a WIP commit.
    pub async fn stop_task(&self, task_id: TaskId) -> Result<()> {
        self.stop_if(task_id, unfinished).await.map(drop)
    }

    /// [`Self::stop_task`] if its state still passes `eligible`, saying whether it stopped it.
    /// Decided under its settle lock, so a report recorded meanwhile is judged by its new state.
    async fn stop_if(&self, task_id: TaskId, eligible: fn(TaskState) -> bool) -> Result<bool> {
        let conversation_id = self.conversation_of_task(&task_id).await?;
        let live = self.existing_task_live(&task_id);
        // A report being recorded right now is recorded first (or not at all).
        let settled = match &live {
            Some(live) => Some(live.settle.lock().await),
            None => None,
        };
        let task = self.task_by_id(&conversation_id, &task_id).await?;
        if !eligible(task.state) {
            return Ok(false);
        }
        // Until it is recorded stopped, a landing that fails as its worktree goes hands
        // nothing back to the orchestrator.
        let _stopping = Stopping::mark(self, &task_id);
        if let Some(live) = &live {
            live.close_cli().await;
        }
        self.dispose_task(&task, stopped_state(&task)).await;
        drop(settled);
        Ok(true)
    }

    /// Stops `task` for good and files a "Stopped" row saying `reason` (the orchestrator's
    /// `stop_worker`, the user's Stop all). False when it had already ended: nothing is filed.
    pub(crate) async fn stop_worker(
        &self,
        conversation_id: &ConversationId,
        task: &Task,
        reason: String,
    ) -> Result<bool> {
        self.stop_worker_if(conversation_id, task, reason, unfinished)
            .await
    }

    /// [`Self::stop_worker`] if its state, looked at again under its settle lock, passes
    /// `eligible`.
    pub(crate) async fn stop_worker_if(
        &self,
        conversation_id: &ConversationId,
        task: &Task,
        reason: String,
        eligible: fn(TaskState) -> bool,
    ) -> Result<bool> {
        // It may have moved on since the caller looked: only a real stop is filed.
        if !self.stop_if(task.id.clone(), eligible).await? {
            return Ok(false);
        }
        self.orchestrator_step(
            conversation_id,
            OrchestratorStepKind::Stopped {
                task_id: task.id.clone(),
                reason,
            },
        )
        .await;
        Ok(true)
    }

    /// The user's Stop for one worker (its row, its view, or the stop of a turn): stops it
    /// for good, files "Stopped by the user", and tells the thread at once: into its running
    /// turn; with a turn of its own when it is idle and the worker's request is still open,
    /// so that request ends on the thread's word (#65), unless the user stopped the session
    /// meanwhile; else before its next turn.
    pub async fn stop_task_by_user(&self, task_id: TaskId) -> Result<()> {
        let conversation_id = self.conversation_of_task(&task_id).await?;
        let task = self.task_by_id(&conversation_id, &task_id).await?;
        self.mark_user_stop(&task.id);
        let conv = self.conv(&conversation_id).ok();
        // The composer's Stop stops the session's workers one by one after it: no turn.
        let wake = match (&conv, &task.request_id) {
            (Some(conv), Some(request))
                if !conv.turn_running().await && !conv.stopped_open(request).await =>
            {
                self.request_open(&task).await
            }
            _ => false,
        };
        // The request keeps working from the stop to the thread's turn on it.
        if wake {
            self.announcing(&task).await;
        }
        let stopped = self
            .stop_worker(&conversation_id, &task, USER_STOP_REASON.to_owned())
            .await;
        let Some(conv) = conv else {
            return stopped.map(drop);
        };
        if !matches!(stopped, Ok(true)) {
            if wake {
                conv.not_announcing(&task.id).await;
            }
            return stopped.map(drop);
        }
        let note = stopped_note(std::slice::from_ref(&task), false);
        if !wake || conv.turn_running().await {
            if wake {
                conv.not_announcing(&task.id).await;
            }
            self.tell_thread_now(&conv, note).await;
            return Ok(());
        }
        self.deliver(
            &conversation_id,
            Envelope {
                kind: InjectionKind::Decision,
                label: format!("stopped task-{}", task.number),
                task_id: Some(task.id.clone()),
                text: format!("{note} Tell the user in a line what this leaves of their request."),
            },
        )
        .await;
        Ok(())
    }

    /// Whether `task`'s request is still open: working or waiting, not answered and over.
    async fn request_open(&self, task: &Task) -> bool {
        let Some(request) = &task.request_id else {
            return false;
        };
        self.core
            .board(&task.conversation_id)
            .await
            .is_ok_and(|board| {
                board.requests.get(request).is_some_and(|request| {
                    matches!(
                        request.state,
                        crate::work::RequestState::Working | crate::work::RequestState::Waiting
                    )
                })
            })
    }

    /// The user's Stop all: stops every worker of the conversation that runs or waits (to
    /// start, for an answer, for quota), files a "Stopped" row for each, and tells the thread
    /// in one note naming them, into its running turn or before its next one. Returns the
    /// tasks stopped.
    pub async fn stop_workers(&self, conversation_id: ConversationId) -> Result<Vec<TaskId>> {
        let board = self.core.board(&conversation_id).await?;
        let mut tasks: Vec<Task> = board
            .tasks
            .values()
            .filter(|task| user_stoppable(task.state))
            .cloned()
            .collect();
        tasks.sort_by_key(|task| task.number);
        for task in &tasks {
            self.mark_user_stop(&task.id);
        }
        let mut stopped = Vec::new();
        for task in tasks {
            match self
                .stop_worker_if(
                    &conversation_id,
                    &task,
                    USER_STOP_REASON.to_owned(),
                    user_stoppable,
                )
                .await
            {
                Ok(true) => stopped.push(task),
                Ok(false) => {}
                Err(err) => {
                    tracing::warn!(task = %task.id, error = %err, "could not stop a worker for Stop all");
                }
            }
        }
        if !stopped.is_empty()
            && let Ok(conv) = self.conv(&conversation_id)
        {
            self.tell_thread_now(&conv, stopped_note(&stopped, true))
                .await;
        }
        Ok(stopped.into_iter().map(|task| task.id).collect())
    }

    /// Restores a task's kept patch as a new branch on its target branch's current tip.
    pub async fn restore_kept_work(&self, task_id: TaskId) -> Result<crate::work::RestoreOutcome> {
        use crate::work::{KeptWork, RestoreOutcome};
        let conversation_id = self.conversation_of_task(&task_id).await?;
        let task = self.task_by_id(&conversation_id, &task_id).await?;
        let Some(KeptWork::Diff { artifact, restored }) = &task.kept else {
            return Err(Error::Invalid("this task kept no patch".into()));
        };
        if let Some(branch) = restored {
            return Err(Error::Invalid(format!("already restored as {branch}")));
        }
        let workspace = task
            .workspace
            .as_ref()
            .ok_or_else(|| Error::Invalid("the task has no workspace".into()))?;
        let target = workspace
            .target
            .clone()
            .ok_or_else(|| Error::Invalid("the task has no target branch".into()))?;
        let name = match workspace.branch.clone() {
            Some(name) => name,
            None => task_branch(
                &self.branch_prefix(&conversation_id).await,
                &conversation_id,
                task.number,
                &task.title,
            ),
        };
        let hash = artifact
            .id
            .parse()
            .map_err(|_| Error::Invalid(format!("{} is not an artifact id", artifact.id)))?;
        let patch = self
            .core
            .store()
            .blobs()
            .get(hash)
            .await?
            .ok_or_else(|| Error::NotFound("the saved patch".into()))?;
        let repo = self.task_repo(&task)?;
        let message = self.commit_message(format!(
            "task-{} {} (restored from its saved patch)",
            task.number, task.title
        ));
        let git = self.git.clone();
        let outcome = blocking(move || {
            let repo = git.open(&repo).map_err(git_error)?;
            let tip = repo
                .branch_tip(&target)
                .map_err(git_error)?
                .ok_or_else(|| Error::Invalid(format!("branch {target} does not exist")))?;
            // The task branch's own name while it is free.
            let mut branch = name.clone();
            let mut n = 1;
            while repo.branch_tip(&branch).map_err(git_error)?.is_some() {
                n += 1;
                branch = format!("{name}-restored-{n}");
            }
            Ok(
                match repo
                    .branch_from_patch(&branch, &tip, &patch, &message)
                    .map_err(git_error)?
                {
                    PatchOutcome::Applied { commit } => RestoreOutcome::Restored {
                        branch,
                        commit: commit.0,
                    },
                    PatchOutcome::Conflicts { paths } => RestoreOutcome::Conflicts { paths },
                    PatchOutcome::Failed { reason } => RestoreOutcome::Failed { reason },
                },
            )
        })
        .await?;
        if let RestoreOutcome::Restored { branch, .. } = &outcome {
            let branch = branch.clone();
            self.update_task(&conversation_id, &task_id, |t| {
                if let Some(KeptWork::Diff { restored, .. }) = &mut t.kept {
                    *restored = Some(branch);
                }
            })
            .await?;
        }
        Ok(outcome)
    }

    /// Pauses a worker: its turn is interrupted, the session stays.
    pub async fn pause_task(&self, task_id: TaskId) -> Result<()> {
        let conversation_id = self.conversation_of_task(&task_id).await?;
        let live = self
            .existing_task_live(&task_id)
            .ok_or_else(|| Error::Invalid("the worker has ended".into()))?;
        let cli = live.state.lock().await.cli.clone();
        if let Some(cli) = cli {
            cli.session
                .interrupt()
                .await
                .map_err(|err| Error::Provider(err.to_string()))?;
        }
        self.set_task_state(&conversation_id, &task_id, TaskState::Paused)
            .await?;
        if live
            .state
            .lock()
            .await
            .cli
            .as_ref()
            .is_some_and(|c| c.provider == ProviderKind::Codex)
        {
            self.notice(
                &conversation_id,
                brigadier_providers::NoticeLevel::Info,
                "Codex finishes the command it is running before it pauses.",
            )
            .await;
        }
        Ok(())
    }

    /// Resumes a paused worker.
    pub async fn resume_task(&self, task_id: TaskId) -> Result<()> {
        // A paused worker of an overnight run continues only with a free worker slot.
        let conversation_id = self.conversation_of_task(&task_id).await?;
        let task = self.task_by_id(&conversation_id, &task_id).await?;
        if let super::overnight::admission::Slot::Full { .. } = self.try_admit_run_task(&task)? {
            // It shows that it waits for a worker, and continues on its own.
            let manager = self.arc();
            self.spawn(async move {
                if manager.admit_run_task(&task).await.is_err() {
                    return;
                }
                if let Err(err) = manager.resume_admitted(task.id.clone()).await {
                    tracing::warn!(task = %task.id, error = %err, "could not resume a task that waited for a worker");
                }
                manager.release_if_idle(&task).await;
            });
            return Ok(());
        }
        let resumed = self.resume_admitted(task_id).await;
        self.release_if_idle(&task).await;
        resumed
    }

    /// [`Self::resume_task`], admitted.
    async fn resume_admitted(&self, task_id: TaskId) -> Result<()> {
        let conversation_id = self.conversation_of_task(&task_id).await?;
        let live = self
            .existing_task_live(&task_id)
            .ok_or_else(|| Error::Invalid("the worker has ended".into()))?;
        let task = self.task_by_id(&conversation_id, &task_id).await?;
        if task.quota_wait.is_some() {
            // Waiting for quota: no CLI runs; route it again now.
            self.continue_task(&live, task).await;
            return Ok(());
        }
        if let Some(end) = live.take_cutoff().await {
            // Paused by hand after its model was cut off.
            self.set_task_state(&conversation_id, &task_id, TaskState::Running)
                .await?;
            let from = live.generation().await;
            let manager = self.arc();
            self.spawn(async move { manager.hand_off(&live, end, from).await });
            return Ok(());
        }
        {
            // Paused while it was being handed to a fresh session: that session starts now
            // (under the hand-over lock, so a message meanwhile finds it running).
            let _handing = live.reroute.lock().await;
            if let Some(first) = live.take_held_handover().await {
                return self.start_fresh(&live, &task, first).await;
            }
        }
        let continued = TurnInput::text("Continue the task.");
        // The models the task may use changed while it was paused (a rule added meanwhile):
        // its CLI session starts again, resumed, so its sub-agents follow them.
        let models = self.rules_models(task.route.choice.provider)?;
        {
            let _handing = live.reroute.lock().await;
            let changed = {
                let state = live.state.lock().await;
                state.cli.is_some() && state.model_rules_changed(&models)
            };
            if changed {
                return self.restart_worker(&live, &task, continued).await;
            }
        }
        let cli = {
            let mut state = live.state.lock().await;
            let cli = state.cli.clone();
            if cli.is_some() {
                state.begin_turn();
                state.nudged = false;
            }
            cli
        };
        let Some(cli) = cli else {
            // Paused before Brigadier restarted: no CLI runs, so its session starts again
            // (BUILD-PLAN D1.4). Any other task without a CLI (taken over in a terminal,
            // ended) has nothing to resume here.
            if task.state != TaskState::Paused {
                return Err(Error::Invalid("the worker has ended".into()));
            }
            self.set_task_state(&conversation_id, &task_id, TaskState::Running)
                .await?;
            let revived =
                crate::heap(|| self.revive_worker(&live, &task, "Continue the task.".into())).await;
            if revived.is_err() {
                let _ = self
                    .set_task_state(&conversation_id, &task_id, TaskState::Paused)
                    .await;
            }
            return revived;
        };
        self.set_task_state(&conversation_id, &task_id, TaskState::Running)
            .await?;
        cli.session
            .send(continued)
            .await
            .map_err(|err| Error::Provider(err.to_string()))
    }

    /// Ends the worker's CLI session and resumes it in a new one (which takes the task's
    /// limits as they are now), with `first`.
    async fn restart_worker(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        first: TurnInput,
    ) -> Result<()> {
        let Some(native_id) = self.last_worker_native_id(task).await else {
            return Err(self.cannot_resume(task).await);
        };
        let subject = match &task.subject {
            Some(id) => self.task_by_id(&task.conversation_id, id).await.ok(),
            None => None,
        };
        live.close_cli().await;
        self.launch_worker(
            live,
            task,
            subject.as_ref(),
            Origin::Resume { native_id },
            first,
        )
        .await
    }

    pub(crate) async fn conversation_of_task(&self, task_id: &TaskId) -> Result<ConversationId> {
        if let Some(live) = self.existing_task_live(task_id) {
            return Ok(live.conversation_id.clone());
        }
        for conversation in self.core.catalog().conversations {
            if let Ok(board) = self.core.board(&conversation.id).await
                && board.tasks.contains_key(task_id)
            {
                return Ok(conversation.id);
            }
        }
        Err(Error::NotFound(format!("task {task_id}")))
    }

    /// A worker failed: the task ends and the orchestrator hears why.
    pub(crate) async fn worker_failed(&self, task: &Task, reason: &str) {
        // Brigadier quitting closed its CLI (a start cut short fails to send its brief): the
        // task carries on at the next launch, as any the quit cut off.
        if !self.admitting.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        // Stopped while it waited to start (for the machine, or a run's worker slot): it stays
        // stopped.
        if self
            .task_by_id(&task.conversation_id, &task.id)
            .await
            .is_ok_and(|now| now.state.is_final())
        {
            return;
        }
        // Being stopped (a start the stop cut short fails too): the stop ends it, keeping its
        // work; ending it here as well would take its worktree from under that (#164).
        if self.is_stopping(&task.id) {
            return;
        }
        let reviewing = task.gate_link.is_some();
        let mut kept = None;
        if let Some(live) = self.existing_task_live(&task.id) {
            live.close_cli().await;
            kept = self.keep_last_message(&live, task.number).await;
        }
        let _ = self
            .update_task(&task.conversation_id, &task.id, |t| {
                t.error = Some(reason.to_owned());
            })
            .await;
        if !reviewing {
            self.announcing(task).await;
        }
        self.dispose_task(task, TaskState::Failed).await;
        // Kept with the task, as the orchestrator reads it by its id (read_artifact).
        if let Some(message) = &kept {
            let _ = self
                .update_task(&task.conversation_id, &task.id, |t| {
                    if !t.outputs.iter().any(|known| known.id == message.id) {
                        t.outputs.push(message.clone());
                    }
                })
                .await;
        }
        // A check from an earlier version's gate had nobody to tell.
        if reviewing {
            return;
        }
        self.deliver(
            &task.conversation_id,
            Envelope {
                kind: InjectionKind::TaskFailed,
                label: format!("task-{} failed", task.number),
                task_id: Some(task.id.clone()),
                text: match kept {
                    Some(message) => format!(
                        "[failed task-{} \"{}\"] {reason} Its last message was kept: artifact {} ({} bytes), for read_artifact.",
                        task.number, task.title, message.id, message.bytes
                    ),
                    None => format!("[failed task-{} \"{}\"] {reason}", task.number, task.title),
                },
            },
        )
        .await;
    }

    /// A read task, or a write task that changed nothing, reported: its worker and workspace
    /// go.
    async fn finish_read_task(&self, task: &Task) {
        if let Some(live) = self.existing_task_live(&task.id) {
            live.close_cli().await;
        }
        // A verifier or fix continues the work it names, so changing nothing in all means that
        // work has nothing to land either.
        let ends = if task.kind.writes() {
            self.landed_with(task).await
        } else {
            vec![task.clone()]
        };
        for done in &ends {
            self.dispose_task(done, TaskState::Done).await;
        }
        if task.role == Some(WorkerRole::Verifier) {
            self.set_phase_stage(task, crate::work::PhaseStage::Done)
                .await;
        }
    }

    /// Ends a task: unfinished changes are kept (B15), then everything recorded under
    /// `task:<id>` is removed. Branches with unlanded work stay; a task branch with nothing to
    /// keep goes with the worktree.
    pub(crate) async fn dispose_task(&self, task: &Task, state: TaskState) {
        // A yes the user gave it in the browser goes with it.
        self.browser.forget_task(&task.conversation_id, &task.id);
        // A terminal it is open in ends first, handing nothing back.
        self.end_takeover(task, &format!("{state:?}")).await;
        if let Some(live) = self.existing_task_live(&task.id) {
            live.close_cli().await;
        }
        // Its worker's previews run in the folder that goes below: they stop before its
        // unfinished work is kept, so none writes into the kept commit.
        self.stop_task_previews(&task.conversation_id, &task.id)
            .await;
        let (kept, drop_branch) = match state {
            TaskState::Stopped | TaskState::Failed => self.keep_unfinished(task).await,
            // A write task that ends done changed nothing that could land.
            TaskState::Done => (None, true),
            // A landed branch is deleted once the landing checked it is merged.
            _ => (None, false),
        };
        // Its outputs folder goes with the scratch folder below.
        let outputs = self.final_outputs(task).await;
        let result = self
            .update_task(&task.conversation_id, &task.id, |t| {
                t.state = state;
                t.blocked_reason = None;
                // A task that ends while waiting for quota waits no more.
                t.quota_wait = None;
                // Nor does it land any more: landed, or ended without landing.
                t.landing = None;
                // Its last model's part is over.
                fallback::end_attempt(t, None);
                if kept.is_some() {
                    t.kept = kept;
                }
                if let Some(outputs) = outputs {
                    t.outputs = outputs;
                }
            })
            .await;
        match result {
            Ok(ended) => self.record_task_outcome(&ended, state).await,
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "could not record the task's end");
            }
        }
        self.task_ended_waiting(task, state).await;
        let owner = format!("task:{}", task.id);
        self.grants.revoke_owner(&owner);
        // Its CLI's session folder goes with what the task leaves (BUILD-PLAN D2.2).
        self.keep_saved_files(&task.id).await;
        // Its CLI's session files are its thread, which its session's Workers row still opens:
        // they stay with the session's record until the session is deleted (S14).
        let lineage = crate::ledger::lineage_owner(&task.conversation_id.0);
        if let Err(err) = self
            .runtime
            .ledger()
            .hand_over_session_files(&owner, &lineage)
            .await
        {
            tracing::warn!(task = %task.id, error = %err, "the worker's session files go with it");
        }
        let leftovers = self.runtime.ledger().dispose(&owner).await;
        if !leftovers.is_clean() {
            tracing::warn!(task = %task.id, ?leftovers, "some of the task's leftovers will be retried at the next launch");
        }
        if drop_branch {
            self.drop_task_branch(task).await;
        }
        self.tasks_lock().remove(&task.id);
        self.forget_task_previews(&task.id).await;
    }

    /// B15: a worker's unfinished changes survive the removal of its worktree, as a WIP commit
    /// on its task branch. Work that sits on the user's uncommitted changes (when they let
    /// workers see them) is replayed onto the target without them; if it overlaps them, it is
    /// kept as a diff artifact instead, because their content must never stay in a commit.
    ///
    /// Also says whether the task branch goes with the worktree: when it holds no work, or when
    /// it is built on the uncommitted changes and its work is safely stored as a diff.
    async fn keep_unfinished(&self, task: &Task) -> (Option<crate::work::KeptWork>, bool) {
        enum Unfinished {
            None,
            Commit(Oid),
            Overlaps { paths: Vec<String>, head: Oid },
        }
        let Some(workspace) = task.workspace.as_ref() else {
            return (None, false);
        };
        let (Some(path), Some(branch)) = (workspace.worktree.as_ref(), workspace.branch.clone())
        else {
            return (None, false);
        };
        let (git, path) = (self.git.clone(), PathBuf::from(path));
        let message = self.commit_message(format!(
            "WIP: task-{} {} (unfinished, kept by Brigadier)",
            task.number, task.title
        ));
        let base = workspace.base.clone().map(Oid);
        let on_snapshot = workspace.on_snapshot;
        let omit_ai_coauthors = self.core.settings().omit_ai_coauthors;
        let (records, kept_branch) = (self.ref_records.clone(), branch.clone());
        let result = blocking(move || {
            let worktree = git.open_worktree(&path).map_err(git_error)?;
            worktree.commit_wip(&message).map_err(git_error)?;
            let head = worktree.head().map_err(git_error)?;
            let Some(base) = base.filter(|base| *base != head) else {
                return Ok(Unfinished::None);
            };
            // The rewrites below needn't be forward: recorded as the engine's own, so no ref
            // check puts the branch back once its worker is gone.
            let repo = git.open(&path).map_err(git_error)?;
            if !on_snapshot {
                // Kept as the user wants its messages (work on a snapshot is kept as one
                // commit of Brigadier's instead).
                if omit_ai_coauthors {
                    match engine_rewrite(&repo, &records, &kept_branch, || {
                        worktree.clean_ai_coauthors(&base, &head)
                    }) {
                        Ok(Some(cleaned)) => return Ok(Unfinished::Commit(cleaned)),
                        Ok(None) => {}
                        Err(err) => {
                            tracing::warn!(error = %err, "could not leave AI co-authors out of the kept commits");
                        }
                    }
                }
                return Ok(Unfinished::Commit(head));
            }
            Ok(
                match engine_rewrite(&repo, &records, &kept_branch, || {
                    worktree.drop_snapshot(&base, &message)
                })
                .map_err(git_error)?
                {
                    Ok(commit) => Unfinished::Commit(commit),
                    Err(paths) => Unfinished::Overlaps { paths, head },
                },
            )
        })
        .await;
        match result {
            Ok(Unfinished::None) => (None, true),
            Ok(Unfinished::Commit(commit)) => (
                Some(crate::work::KeptWork::Branch {
                    branch,
                    commit: commit.0,
                }),
                false,
            ),
            Ok(Unfinished::Overlaps { paths, head }) => match self.kept_diff(task).await {
                Some(artifact) => {
                    tracing::info!(task = %task.id, ?paths, "unfinished work overlaps the user's uncommitted changes; kept as a diff");
                    (
                        Some(crate::work::KeptWork::Diff {
                            artifact,
                            restored: None,
                        }),
                        true,
                    )
                }
                None => {
                    // Losing the work would be worse: the branch stays, on the snapshot.
                    tracing::error!(task = %task.id, "could not store the unfinished work as a diff; its branch stays, on the user's uncommitted changes");
                    (
                        Some(crate::work::KeptWork::Branch {
                            branch,
                            commit: head.0,
                        }),
                        false,
                    )
                }
            },
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "could not keep unfinished work as a commit");
                match self.kept_diff(task).await {
                    Some(artifact) => (
                        Some(crate::work::KeptWork::Diff {
                            artifact,
                            restored: None,
                        }),
                        on_snapshot,
                    ),
                    None => (None, false),
                }
            }
        }
    }

    /// The task's diff as an artifact, read back from the blob store to be sure it is there.
    async fn kept_diff(&self, task: &Task) -> Option<ArtifactRef> {
        let diff = self.worktree_diff(task).await;
        let artifact = self
            .diff_artifact(&self.task_live(task), task, diff)
            .await?;
        let hash = artifact.id.parse().ok()?;
        match self.core.store().blobs().get(hash).await {
            Ok(Some(bytes)) if bytes.len() as u64 == artifact.bytes => Some(artifact),
            other => {
                tracing::warn!(task = %task.id, found = ?other.map(|b| b.map(|b| b.len())), "the stored diff does not read back");
                None
            }
        }
    }

    /// Deletes the task branch once its worktree is gone.
    async fn drop_task_branch(&self, task: &Task) {
        let Some(branch) = task.workspace.as_ref().and_then(|w| w.branch.clone()) else {
            return;
        };
        let Ok(repo) = self.task_repo(task) else {
            return;
        };
        self.forget_branch(&repo, &branch);
        let git = self.git.clone();
        let result = blocking(move || {
            let repo = git.open(&repo).map_err(git_error)?;
            match repo.branch_tip(&branch).map_err(git_error)? {
                Some(tip) => repo.delete_branch_at(&branch, &tip).map_err(git_error),
                None => Ok(()),
            }
        })
        .await;
        if let Err(err) = result {
            tracing::warn!(task = %task.id, error = %err, "could not delete the task branch");
        }
    }
}

/// What a task needs from its model: its attachments, the capabilities it asked for, and what
/// its kind takes (an operator looks at the screen, so only a model that reads images can).
pub(crate) fn needs_of(
    kind: TaskKind,
    attachments: &[AttachmentRef],
    capabilities: &[brigadier_router::Capability],
) -> brigadier_router::Needs {
    brigadier_router::Needs {
        image_input: kind == TaskKind::Operate
            || attachments
                .iter()
                .any(|attachment| attachment.mime.starts_with("image/")),
        image_generation: capabilities.contains(&brigadier_router::Capability::ImageGeneration),
        context_tokens: None,
    }
}

/// What only Brigadier sets on a task it makes itself.
#[derive(Debug, Clone, Default)]
pub(crate) struct TaskExtra {
    /// Its id, chosen before it starts (a review someone waits for is waited for first).
    pub id: Option<TaskId>,
    /// Its overnight run context, instead of the one the session's active run gives.
    pub run: Option<crate::overnight::RunTaskContext>,
    /// The routing category, instead of the one its kind maps to (a phase's judge routes as
    /// orchestration).
    pub category: Option<brigadier_router::TaskCategory>,
    /// The request it belongs to, instead of the one the orchestrator serves now (a phase's
    /// checks belong to the phase).
    pub request: Option<String>,
    /// Its part in the request's flow.
    pub role: Option<WorkerRole>,
    /// The phase of the request's plan it works on.
    pub phase: Option<u32>,
    /// An operate task's target and end state.
    pub target: Option<String>,
    pub end_state: Option<String>,
    /// Its part in a mockup first.
    pub mockup: Option<crate::work::MockupPart>,
    /// It makes designs or images (`delegate_task`'s `design`).
    pub design: bool,
    /// The live look at the integrated tree's dev build (`delegate_task`'s `live_look`).
    pub live_look: bool,
    /// A fresh verifier's candidate commit (`start_verifier` with `fresh`).
    pub fresh_commit: Option<String>,
    /// It reruns a cheaper model's task one tier up (see `reruns`).
    pub rerun: Option<brigadier_router::Rerun>,
    /// The catalog ids of the MCP servers it gets besides its agent's own
    /// (`delegate_task`'s `integrations`).
    pub integrations: Vec<String>,
    /// A plain worker of `brigadier start`, on the CLI the route names.
    pub plain: Option<crate::work::PlainWorker>,
}

/// The router's category for a task: design work when it makes designs or images
/// ([`Task::design`]), else its kind's.
pub(crate) fn task_category(task: &Task) -> brigadier_router::TaskCategory {
    category(task.kind, task.design)
}

/// Whether a new task makes designs or images: the thread said so, it builds a mockup, or it
/// must generate images.
pub(crate) fn makes_designs(
    design: bool,
    mockup: Option<&crate::work::MockupPart>,
    needs: &[brigadier_router::Capability],
) -> bool {
    design
        || matches!(mockup, Some(crate::work::MockupPart::Builds))
        || needs.contains(&brigadier_router::Capability::ImageGeneration)
}

/// The router's category for a task kind, `design` when it makes designs or images. A task
/// that checks other work (a review, merge or check run) stays that work, whatever it checks.
pub(crate) fn category(kind: TaskKind, design: bool) -> brigadier_router::TaskCategory {
    use brigadier_router::TaskCategory;
    if design && !matches!(kind, TaskKind::Review | TaskKind::Merge | TaskKind::Verify) {
        return TaskCategory::Design;
    }
    match kind {
        TaskKind::Scout => TaskCategory::Scout,
        TaskKind::Research => TaskCategory::Research,
        TaskKind::Implement => TaskCategory::Implement,
        TaskKind::Review => TaskCategory::Review,
        TaskKind::Merge => TaskCategory::Merge,
        TaskKind::Verify => TaskCategory::Verify,
        TaskKind::Operate => TaskCategory::Operate,
    }
}

/// The cleanup owner of a conversation's worker folder ([`SessionManager::worker_home`]).
pub(crate) fn worker_home_owner(conversation_id: &ConversationId) -> String {
    format!("workers:{conversation_id}")
}

/// How a worker's repo note names the folder its CLI starts in.
fn start_note(cwd: &Path, scratch: &Path) -> &'static str {
    if cwd == scratch {
        "You start in your scratch folder"
    } else {
        "You start in a folder shared with this session's other workers, not your worktree; don't write there"
    }
}

/// Where a task's tests and smoke runs keep their data: under the system's temporary folder,
/// never the app's own data folder (a smoke run of a debug build must not touch it).
pub(crate) fn test_data_dir(id: &TaskId) -> PathBuf {
    let name = format!("brigadier-test-{}", &id.0[id.0.len().saturating_sub(8)..]);
    test_data_root().join(name)
}

/// The cleanup owner of a session thread's temporary folder (its folder's `tmp/`).
pub(crate) fn thread_temp_owner(id: &ConversationId) -> String {
    format!("thread-temp:{id}")
}

/// The folder every task's test data folder is made in: `/tmp`, whose short paths hold the
/// Unix sockets made there. In the core's own tests, the test process's temp directory when it
/// lies in `/tmp` already: a session's full checks run them with their own
/// `/tmp/brigadier-test-k…`, the only folder of `/tmp` their sandbox lets them write, so the
/// sessions a test starts make everything there (#493, #382).
pub(crate) fn test_data_root() -> PathBuf {
    #[cfg(all(test, unix))]
    if let Some(temp) = temp_dir_in_tmp() {
        return temp;
    }
    if cfg!(unix) {
        PathBuf::from("/tmp")
    } else {
        std::env::temp_dir()
    }
}

/// The temp directory, when it lies in `/tmp` (resolved: `/tmp` is `/private/tmp` on macOS).
#[cfg(all(test, unix))]
pub(crate) fn temp_dir_in_tmp() -> Option<PathBuf> {
    let resolved = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_owned());
    let temp = std::env::temp_dir();
    resolved(&temp)
        .starts_with(resolved(Path::new("/tmp")))
        .then_some(temp)
}

/// How a stopped task ends: a read task that already reported (a check still ending its turn,
/// stopped by the orchestrator or a run's end) is done; anything else is stopped.
/// Whether `subject` is a write task's work still to land, in a worktree of its own: a task
/// that names it continues from it (and lands it, see `landed_with`).
fn continues_work(subject: &Task) -> bool {
    subject.kind.writes()
        && !subject.state.is_final()
        && subject.landed.is_none()
        && subject
            .workspace
            .as_ref()
            .is_some_and(|w| w.worktree.is_some() && w.base.is_some())
}

/// The reason a "Stopped" row gives for the user's Stop all.
pub(crate) const USER_STOP_REASON: &str = "Stopped by the user";

/// The start of a worker that was stopped as it started.
fn stopped_as_it_started(task: &Task) -> Error {
    Error::Invalid(format!("task-{} was stopped as it started", task.number))
}

/// Whether a plain stop still stops a worker in `state`: it hasn't ended.
fn unfinished(state: TaskState) -> bool {
    !state.is_final()
}

/// Whether the user's Stop all stops a worker in `state`: it runs or waits to run. A worker that
/// reported, or is landing or ready to land, keeps its work (the desktop's `workerStoppable`).
pub(crate) fn user_stoppable(state: TaskState) -> bool {
    matches!(
        state,
        TaskState::Queued
            | TaskState::Starting
            | TaskState::Running
            | TaskState::Blocked
            | TaskState::Paused
    )
}

/// The thread's note about the workers the user stopped (`all`: with Stop all).
fn stopped_note(tasks: &[Task], all: bool) -> String {
    let names: Vec<String> = tasks
        .iter()
        .map(|task| format!("task-{} \"{}\"", task.number, task.title))
        .collect();
    let names = match names.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    let (what, them) = match (all, tasks.len()) {
        (true, _) => ("all workers: ", "them"),
        (false, 1) => ("", "it"),
        (false, _) => ("", "them"),
    };
    format!(
        "[worker] The user stopped {what}{names}. You didn't stop {them} and nothing failed. \
         Don't message, steer or approve {them}, and don't start {them} again unless the user \
         asks."
    )
}

/// The thread's refusal for a task the user stopped.
pub(crate) fn user_stopped_refusal(task: &Task) -> String {
    format!(
        "task-{} was stopped by the user. Don't message, steer or approve it, and don't start \
         it again unless the user asks.",
        task.number
    )
}

fn stopped_state(task: &Task) -> TaskState {
    if !task.kind.writes() && task.report.is_some() {
        TaskState::Done
    } else {
        TaskState::Stopped
    }
}

/// B12: a task's repository access by its kind (a worktree to write, one to read, or none),
/// with the level it was filed at (its CLI runs at its session's level, passed straight
/// through).
pub(crate) fn repo_access_for(kind: TaskKind, permission: PermissionLevel) -> WorkerAccess {
    WorkerAccess {
        repo: match kind {
            TaskKind::Research | TaskKind::Operate => RepoAccess::None,
            TaskKind::Implement | TaskKind::Merge => RepoAccess::Write,
            TaskKind::Scout | TaskKind::Review | TaskKind::Verify => RepoAccess::Read,
        },
        network: true,
        unsandboxed: permission == PermissionLevel::Full,
    }
}

/// A worker's temporary files go to its scratch folder, which its CLI's sandbox lets it write:
/// other programs' by `TMPDIR`, zsh's here-documents by `TMPPREFIX` (zsh 5.9 makes them under
/// `/tmp/zsh` by default, and a sandboxed `cat <<EOF` there fails with "can't create temp file
/// for here document").
pub(crate) fn worker_env(scratch: &Path) -> Vec<(String, String)> {
    vec![
        ("TMPDIR".into(), scratch.to_string_lossy().into_owned()),
        (
            "TMPPREFIX".into(),
            scratch.join("zsh").to_string_lossy().into_owned(),
        ),
    ]
}

/// The native id of the latest session start among a task's worker events, paging back
/// from the newest until one is found.
async fn latest_session_start(store: &brigadier_store::Store, id: &TaskId) -> Option<String> {
    let mut before = None;
    loop {
        let page = store
            .read_stream(
                streams::task(id),
                brigadier_store::StreamPage {
                    before,
                    kinds: vec!["worker.event".into()],
                    limit: 1_000,
                },
            )
            .await
            .ok()?;
        let oldest = page.iter().map(|stored| stored.stream_seq).min()?;
        let found = page.iter().find_map(|stored| {
            match serde_json::from_str::<DomainEvent>(stored.payload.get()) {
                Ok(DomainEvent::WorkerEvent {
                    event: ProviderEvent::SessionStarted { native_id, .. },
                    ..
                }) => Some(native_id),
                _ => None,
            }
        });
        if found.is_some() {
            return found;
        }
        before = Some(oldest);
    }
}

/// Names remain short and unique even for automatically created reviewers and old callers.
fn job_name(title: &str, kind: TaskKind, subject: Option<&str>, used: &[String]) -> String {
    let contains_id = title
        .split_whitespace()
        .any(|word| word.trim_matches(['`', '“', '”', '"']).starts_with("task-"));
    let source = if contains_id {
        match kind {
            TaskKind::Review => format!("Review {}", subject.unwrap_or("changes")),
            TaskKind::Verify => format!("Check {}", subject.unwrap_or("changes")),
            _ => title.to_owned(),
        }
    } else {
        title.to_owned()
    };
    let mut words = source
        .split_whitespace()
        .map(|word| word.trim_matches(['`', '#', '*', '“', '”', '"', '(', ')', ':', '·', '—', '–']))
        .filter(|word| !word.is_empty() && !word.starts_with("task-"))
        .take(4)
        .collect::<Vec<_>>();
    while words.len() > 2
        && words
            .last()
            .is_some_and(|word| ["to", "of", "for", "with", "and", "the", "a", "an"].contains(word))
    {
        words.pop();
    }
    if words.is_empty() {
        words.extend(["Worker", "task"]);
    }
    if words.len() == 1 {
        words.push("task");
    }
    let base = words.join(" ");
    if !used.iter().any(|name| name.eq_ignore_ascii_case(&base)) {
        return base;
    }
    let mut ordinal = 2;
    loop {
        let name = format!("{base} {ordinal}");
        if !used.iter().any(|used| used.eq_ignore_ascii_case(&name)) {
            return name;
        }
        ordinal += 1;
    }
}

/// `<prefix><session>/task-<n>-<slug>`.
pub(super) fn task_branch(
    prefix: &str,
    conversation_id: &ConversationId,
    number: u32,
    title: &str,
) -> String {
    let session = conversation_id.short();
    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join("-");
    let slug: String = slug.chars().take(40).collect();
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        format!("{prefix}{session}/task-{number}")
    } else {
        format!("{prefix}{session}/task-{number}-{slug}")
    }
}

/// The step the thread shows when `task` just left `was` (absent: it was just created);
/// `reported` tells whether it had reported before.
fn worker_step(task: &Task, was: Option<TaskState>, reported: bool) -> Option<DomainEvent> {
    let kind = crate::work::WorkerStepKind::between(was, task.state, reported)?;
    Some(DomainEvent::WorkerStepped {
        step: crate::work::WorkerStep {
            task_id: task.id.clone(),
            request_id: task.request_id.clone(),
            kind,
            at_ms: task.updated_at_ms,
            position: 0,
        },
    })
}

pub(crate) fn route_label(task: &Task) -> String {
    let choice = &task.route.choice;
    let mut label = match &choice.model {
        Some(model) => format!("{} {model}", choice.provider.label()),
        None => choice.provider.label().to_owned(),
    };
    // Hand-offs wake no one; the report or failure says who ran the task before.
    let before: Vec<String> = task
        .attempts
        .iter()
        .filter_map(|attempt| {
            let end = attempt.end.as_ref()?;
            Some(format!(
                "{} ({})",
                fallback::model_label(&attempt.route.choice),
                fallback::end_reason(end)
            ))
        })
        .collect();
    if !before.is_empty() {
        let _ = write!(label, ", took over from {}", before.join(", then "));
    }
    label
}

/// The label of the envelope with what a worker wrote after its report.
fn addendum_label(number: u32) -> String {
    format!("report task-{number} (addendum)")
}

/// Whether a task checks another task's change (a gate member, or a review or verification
/// of a subject): its result is its report alone, and what it writes after it goes nowhere.
fn checks_a_change(task: &Task) -> bool {
    task.kind == TaskKind::Review
        || task.gate_link.is_some()
        || (task.kind == TaskKind::Verify && task.subject.is_some())
}

/// Whether a write task's report is a fix Brigadier checks and lands on its own, once the
/// worker's turn is over (see `worker_report`): its request still works, and the orchestrator
/// has nothing to decide about it.
pub(super) fn relanding_pending(task: &Task) -> bool {
    task.kind.writes() && task.state == TaskState::Reported && task.landing.is_some()
}

/// Marks a task as being stopped while it lives (the first of concurrent stops does).
struct Stopping<'a> {
    manager: &'a SessionManager,
    task: Option<TaskId>,
}

impl<'a> Stopping<'a> {
    fn mark(manager: &'a SessionManager, task: &TaskId) -> Self {
        let first = manager.stopping_lock().insert(task.clone());
        Self {
            manager,
            task: first.then(|| task.clone()),
        }
    }
}

impl Drop for Stopping<'_> {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            self.manager.stopping_lock().remove(task);
        }
    }
}

/// Whether `now` still holds the report `then` had.
pub(super) fn same_report(now: &Task, then: &Task) -> bool {
    now.report.as_ref().map(|report| report.submitted_at_ms)
        == then.report.as_ref().map(|report| report.submitted_at_ms)
}

/// Whether `message`, written after a report with `summary`, holds findings the report left
/// out: any message the summary points at ("reproduced below"), else a long one.
fn is_late_findings(summary: &str, message: &str) -> bool {
    const POINTERS: [&str; 7] = [
        "below",
        "following message",
        "next message",
        "final message",
        "last message",
        "my message",
        "in a message",
    ];
    if message.is_empty() {
        return false;
    }
    let summary = summary.to_lowercase();
    message.len() >= KEEP_MESSAGE_MIN_BYTES || POINTERS.iter().any(|word| summary.contains(word))
}

/// The headings a worker's handoff note must hold (the wrap-up steer's), as words to find.
const HANDOFF_PARTS: &[(&str, &str)] = &[
    ("goal", "Goal and where it stands"),
    ("done", "Done"),
    ("in progress", "In progress"),
    ("next steps", "Next steps"),
    ("decisions", "Decisions and approvals already given"),
    ("gotchas", "Gotchas learned"),
    ("checks", "Checks that passed"),
    ("how to verify", "How to verify"),
];

/// The parts a handoff note leaves out, by heading.
fn handoff_gaps(note: &str) -> Vec<&'static str> {
    let note = note.to_lowercase();
    HANDOFF_PARTS
        .iter()
        .filter(|(word, _)| !note.contains(word))
        .map(|(_, heading)| *heading)
        .collect()
}

/// What may stand in for a worker's question nobody answered: its safe default, only for an
/// operational question whose words and default ask no consent (BUILD-PLAN E2, Q35: nothing
/// happens on its own).
fn safe_default(
    about: crate::tools::QuestionAbout,
    question: &str,
    default: Option<&str>,
) -> Option<String> {
    let default = default
        .map(str::trim)
        .filter(|default| !default.is_empty())?;
    if about != crate::tools::QuestionAbout::Operational {
        return None;
    }
    let asks_consent = |text: &str| {
        text.to_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| CONSENT_WORDS.contains(&word))
    };
    (!asks_consent(question) && !asks_consent(default)).then(|| default.to_owned())
}

/// Words of a question that asks for consent, whatever it says it is about.
const CONSENT_WORDS: &[&str] = &[
    "merge", "merging", "push", "pushing", "delete", "deleting", "remove", "removing", "deploy",
    "publish", "release", "pay", "buy", "spend", "purchase", "send", "email", "drop", "force",
];

#[cfg(test)]
mod tests {
    #[test]
    fn watchdog_worker_progress_clock_ignores_chatter_and_repeated_content() {
        use brigadier_providers::NoticeLevel;
        let mut state = super::TaskLiveState {
            busy: true,
            ..Default::default()
        };
        for now in [60_000, 120_000, 180_000] {
            state.note_progress(
                &brigadier_providers::ProviderEvent::Notice {
                    level: NoticeLevel::Info,
                    message: format!("status at {now}"),
                },
                now,
            );
            state.note_progress(
                &brigadier_providers::ProviderEvent::Progress {
                    item_id: "t".into(),
                },
                now,
            );
            assert_eq!(state.last_event_ms, 0);
        }
        let event = brigadier_providers::ProviderEvent::MessageDelta {
            item_id: "m".into(),
            text: "new content".into(),
            phase: None,
        };
        state.note_progress(&event, 200_000);
        assert_eq!(state.last_event_ms, 200_000);
        state.note_progress(&event, 300_000);
        assert_eq!(state.last_event_ms, 200_000);
    }

    use super::*;

    /// A folder in the temp directory, removed when dropped however the test ends.
    struct Temp(PathBuf);

    impl std::ops::Deref for Temp {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_workers_temporary_files_and_here_documents_go_to_its_scratch() {
        let scratch =
            Temp(std::env::temp_dir().join(format!("brigadier-worker-env-{}", std::process::id())));
        std::fs::create_dir_all(&*scratch).unwrap();
        let env = worker_env(&scratch);
        for (name, value) in &env {
            assert!(Path::new(value).starts_with(&*scratch), "{name}={value}");
        }
        assert!(env.iter().any(|(name, _)| name == "TMPPREFIX"));
        // zsh, the shell Codex runs commands in, writes a here-document under TMPPREFIX.
        if Path::new("/bin/zsh").exists() {
            let out = std::process::Command::new("/bin/zsh")
                .args(["-fc", "cat <<EOF\nhere\nEOF"])
                .env_clear()
                .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
            assert_eq!(String::from_utf8_lossy(&out.stdout), "here\n");
        }
        let unwritable = worker_env(Path::new("/nonexistent/scratch"));
        if Path::new("/bin/zsh").exists() {
            // The same here-document fails where TMPPREFIX can't be written: the sandboxed case.
            let out = std::process::Command::new("/bin/zsh")
                .args(["-fc", "cat <<EOF\nhere\nEOF"])
                .env_clear()
                .envs(unwritable.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                .output()
                .unwrap();
            assert!(!out.status.success());
        }
    }

    #[test]
    fn an_operator_needs_a_model_that_takes_images_and_no_checkout() {
        assert!(needs_of(TaskKind::Operate, &[], &[]).image_input);
        assert!(!needs_of(TaskKind::Research, &[], &[]).image_input);
        assert_eq!(
            category(TaskKind::Operate, false),
            brigadier_router::TaskCategory::Operate
        );
        for permission in [PermissionLevel::Ask, PermissionLevel::Full] {
            assert_eq!(
                repo_access_for(TaskKind::Operate, permission).repo,
                RepoAccess::None
            );
        }
    }

    #[test]
    fn a_message_the_report_points_at_is_kept_however_short() {
        let summary = "Zustand stores; the full findings are reproduced below.";
        assert!(is_late_findings(summary, "Stores live in src/store/*.ts."));
        assert!(!is_late_findings(summary, ""));
    }

    #[test]
    fn a_short_message_after_a_full_report_is_not() {
        let summary = "The app uses Zustand; its stores are in src/store/.";
        assert!(!is_late_findings(summary, "Report submitted."));
        assert!(is_late_findings(summary, &"Details. ".repeat(60)));
    }

    #[test]
    fn what_a_checking_task_writes_after_its_report_goes_nowhere() {
        let task = |kind: &str| -> Task {
            serde_json::from_value(serde_json::json!({
                "id": "t2",
                "conversationId": "c1",
                "number": 2,
                "position": 0,
                "title": "Verify task-1",
                "kind": kind,
                "spec": "Verify it.",
                "access": { "repo": "read", "network": false, "unsandboxed": false },
                "route": { "choice": { "provider": "claude", "model": null, "effort": null }, "reason": "" },
                "state": "reported",
                "attachments": [],
                "createdAtMs": 0,
                "updatedAtMs": 0
            }))
            .expect("a task")
        };
        // A verifier of the gate.
        let mut member = task("verify");
        member.gate_link = Some(crate::work::GateLink {
            owner: crate::work::GateOwner::Task {
                task_id: TaskId("t1".into()),
            },
            round: 1,
            role: crate::work::GateRole::Verify,
        });
        assert!(checks_a_change(&member));
        // A verification the orchestrator delegated for another task's change.
        let mut delegated = task("verify");
        delegated.subject = Some(TaskId("t1".into()));
        assert!(checks_a_change(&delegated));
        assert!(checks_a_change(&task("review")));
        // A verification of its own (run the suite, say what fails) is research: its
        // findings may come after the report.
        assert!(!checks_a_change(&task("verify")));
        assert!(!checks_a_change(&task("scout")));
    }

    #[test]
    fn a_fix_is_landed_after_its_turn_only_while_nothing_moved_the_task_on() {
        let mut then: Task = serde_json::from_value(serde_json::json!({
            "id": "t1",
            "conversationId": "c1",
            "number": 1,
            "position": 0,
            "title": "Add cube",
            "kind": "implement",
            "spec": "Add cube.",
            "access": { "repo": "write", "network": false, "unsandboxed": false },
            "route": { "choice": { "provider": "claude", "model": null, "effort": null }, "reason": "" },
            "state": "reported",
            "attachments": [],
            "createdAtMs": 0,
            "updatedAtMs": 0
        }))
        .expect("a task");
        then.landing = Some("Add cube".into());
        then.report = Some(Report {
            summary: "Fixed.".into(),
            changes: Vec::new(),
            decisions: Vec::new(),
            learned: Vec::new(),
            verification: Vec::new(),
            done_when: Vec::new(),
            open_questions: Vec::new(),
            risks: Vec::new(),
            needs_user: Vec::new(),
            deleted: Vec::new(),
            verdict: None,
            checks: None,
            artifacts: Vec::new(),
            submitted_at_ms: 10,
        });
        let pending = |now: &Task| relanding_pending(now) && same_report(now, &then);
        assert!(pending(&then));
        // Steered by the orchestrator: back at work, and the fix loop is its.
        let mut steered = then.clone();
        steered.landing = None;
        steered.state = TaskState::Running;
        assert!(!pending(&steered));
        // Stopped.
        let mut stopped = then.clone();
        stopped.state = TaskState::Stopped;
        stopped.landing = None;
        assert!(!pending(&stopped));
        // Sent back and reported again: that report has its own landing.
        let mut newer = then.clone();
        newer.report.as_mut().expect("a report").submitted_at_ms = 20;
        assert!(!pending(&newer));
        // A read task is never landed.
        let mut scout = then.clone();
        scout.kind = TaskKind::Scout;
        assert!(!pending(&scout));
    }

    #[tokio::test]
    async fn waiting_for_a_turn_to_end_gives_up_even_while_its_end_is_being_handled() {
        let live = TaskLive::new(TaskId("t1".into()), ConversationId("c1".into()));
        // The end of the turn is being handled, and that never finishes.
        let _end = live.turn_end.lock().await;
        let waited = tokio::time::timeout(
            Duration::from_secs(5),
            live.turn_over_within(Duration::from_millis(50)),
        )
        .await;
        assert!(waited.is_ok(), "it waited for the lock past its limit");
    }

    #[tokio::test]
    async fn a_stall_counts_only_once_its_session_is_taken() {
        let live = TaskLive::new(TaskId("t1".into()), ConversationId("c1".into()));
        live.set_cutoff(AttemptEnd::Error {
            kind: brigadier_providers::ErrorKind::Auth,
            message: "logged out".into(),
        })
        .await;
        // No longer due (the worker moved on): nothing is taken or counted.
        assert!(live.detach_stalled(|_| false, true).await.is_none());
        assert_eq!(live.watch(0).await.stalls, 0);
        assert!(!live.watch(0).await.stopping);
        // Taken for a fresh session: a stall of the attempt, the cut-off left for later.
        let (cli, cutoff) = live.detach_stalled(|_| true, true).await.expect("taken");
        assert!(cli.is_none() && cutoff.is_none());
        let watch = live.watch(0).await;
        assert_eq!(watch.stalls, 1);
        assert!(watch.stopping);
        // Taken for another model: the cut-off wins, and no stall is counted.
        let (_, cutoff) = live.detach_stalled(|_| true, false).await.expect("taken");
        assert!(matches!(cutoff, Some(AttemptEnd::Error { .. })));
        assert_eq!(live.watch(0).await.stalls, 1);
    }

    #[test]
    fn a_session_started_with_other_model_limits_starts_again() {
        let opus = AllowedModels {
            ids: vec!["claude-opus-5-5".into(), "claude-haiku-4-5-20251001".into()],
            outside: vec!["claude-fable-5-1".into()],
        };
        let mut state = TaskLiveState::default();
        // No session recorded: nothing shows it holds them.
        assert!(state.models_changed(&opus));
        state.allowed_models = Some(opus.clone());
        assert!(!state.models_changed(&opus));
        // A Never-Haiku rule added while it was paused.
        let no_haiku = AllowedModels {
            ids: vec!["claude-opus-5-5".into()],
            outside: vec![
                "claude-fable-5-1".into(),
                "claude-haiku-4-5-20251001".into(),
            ],
        };
        assert!(state.models_changed(&no_haiku));
        // Haiku allowed again: the limits only widened, so the session stays.
        state.allowed_models = Some(no_haiku);
        assert!(!state.models_changed(&opus));
    }

    #[tokio::test]
    async fn a_long_workers_session_is_found_behind_a_thousand_later_events() {
        let dir =
            Temp(std::env::temp_dir().join(format!("brigadier-resume-{}", uuid::Uuid::new_v4())));
        let store = tokio::task::spawn_blocking({
            let dir = dir.to_path_buf();
            move || {
                brigadier_store::Store::open(brigadier_store::StoreConfig {
                    db_path: dir.join("db.sqlite"),
                    blobs_dir: dir.join("blobs"),
                    readers: 1,
                })
            }
        })
        .await
        .expect("joined")
        .expect("a store");
        let task = TaskId("t1".into());
        let event = |event: ProviderEvent| {
            let event = DomainEvent::WorkerEvent {
                task_id: task.clone(),
                event,
            };
            brigadier_store::NewEvent::new(streams::task(&task), event.kind(), 0, &event)
                .expect("an event")
        };
        // A task recorded before its session was kept on it: an older session, the latest,
        // then more than a page of reasoning after it.
        let mut events = vec![
            event(ProviderEvent::SessionStarted {
                native_id: "older".into(),
                model: None,
                cwd: None,
                cli_version: None,
                file: None,
            }),
            event(ProviderEvent::SessionStarted {
                native_id: "latest".into(),
                model: None,
                cwd: None,
                cli_version: None,
                file: None,
            }),
        ];
        events.extend((0..2_500).map(|_| {
            event(ProviderEvent::ReasoningDelta {
                item_id: "r".into(),
                text: "thinking".into(),
            })
        }));
        store.append(events).await.expect("stored");
        assert_eq!(
            latest_session_start(&store, &task).await.as_deref(),
            Some("latest")
        );
        assert_eq!(
            latest_session_start(&store, &TaskId("none".into())).await,
            None
        );
        let _ = store.shutdown().await;
    }

    #[tokio::test]
    async fn a_silence_report_no_longer_due_is_not_counted() {
        let live = TaskLive::new(TaskId("t1".into()), ConversationId("c1".into()));
        assert!(!live.report_stall(|_| false, 1).await);
        // Without a CLI there is no live silence to report either.
        assert!(!live.report_stall(|_| true, 1).await);
        assert_eq!(live.watch(0).await.reported_at_ms, None);
    }

    #[test]
    fn a_check_stopped_after_its_report_ends_done() {
        let task = |kind: &str, reported: bool| -> Task {
            let mut task: Task = serde_json::from_value(serde_json::json!({
                "id": "t2",
                "conversationId": "c1",
                "number": 2,
                "position": 0,
                "title": "Review task-1",
                "kind": kind,
                "spec": "Review it.",
                "access": { "repo": "read", "network": false, "unsandboxed": false },
                "route": { "choice": { "provider": "claude", "model": null, "effort": null }, "reason": "" },
                "state": "reported",
                "attachments": [],
                "createdAtMs": 0,
                "updatedAtMs": 0
            }))
            .expect("a task");
            if reported {
                task.report = Some(crate::work::Report {
                    summary: "Approve.".into(),
                    changes: Vec::new(),
                    decisions: Vec::new(),
                    learned: Vec::new(),
                    verification: Vec::new(),
                    done_when: Vec::new(),
                    open_questions: Vec::new(),
                    risks: Vec::new(),
                    needs_user: Vec::new(),
                    deleted: Vec::new(),
                    verdict: Some(crate::work::ReviewVerdict::Approve),
                    checks: None,
                    artifacts: Vec::new(),
                    submitted_at_ms: 0,
                });
            }
            task
        };
        assert_eq!(stopped_state(&task("review", true)), TaskState::Done);
        assert_eq!(stopped_state(&task("verify", true)), TaskState::Done);
        // Stopped before its result, or a change that hasn't landed: stopped.
        assert_eq!(stopped_state(&task("review", false)), TaskState::Stopped);
        assert_eq!(stopped_state(&task("implement", true)), TaskState::Stopped);
    }
}

#[cfg(test)]
mod job_name_tests {
    use super::*;

    #[test]
    fn duplicate_names_keep_the_whole_base_before_the_ordinal() {
        let title = "Beginner trading in Moldova";
        let mut used = vec![title.to_owned()];
        for ordinal in 2..=3 {
            let name = job_name(title, TaskKind::Scout, None, &used);
            assert_eq!(name, format!("{title} {ordinal}"));
            used.push(name);
        }
    }

    #[test]
    fn names_are_short_plain_and_unique() {
        assert_eq!(
            job_name(
                "Add JSON output to the list command",
                TaskKind::Implement,
                None,
                &[]
            ),
            "Add JSON output"
        );
        assert_eq!(
            job_name(
                "Review `task-3`",
                TaskKind::Review,
                Some("File summary"),
                &[]
            ),
            "Review File summary"
        );
        assert_eq!(
            job_name(
                "Check results",
                TaskKind::Verify,
                None,
                &["Check results".into(), "Check results 2".into()]
            ),
            "Check results 3"
        );
        assert_eq!(
            job_name("task-3", TaskKind::Scout, None, &[]),
            "Worker task"
        );
    }
}

#[cfg(test)]
mod safe_default_tests {
    use super::safe_default;
    use crate::tools::QuestionAbout;

    /// Only an operational question goes on with its default; consent and product questions
    /// wait whatever default they give, and so does one whose words ask for consent.
    #[test]
    fn only_an_operational_question_goes_on_with_its_default() {
        let default = Some("put it in src/util.ts");
        assert_eq!(
            safe_default(
                QuestionAbout::Operational,
                "Which file holds the helper?",
                default
            ),
            Some("put it in src/util.ts".to_owned())
        );
        assert_eq!(
            safe_default(QuestionAbout::Consent, "Which file?", default),
            None
        );
        assert_eq!(
            safe_default(QuestionAbout::Product, "Which file?", default),
            None
        );
        assert_eq!(
            safe_default(
                QuestionAbout::Operational,
                "Should I merge it into main?",
                default
            ),
            None
        );
        assert_eq!(
            safe_default(
                QuestionAbout::Operational,
                "Old files?",
                Some("delete them")
            ),
            None
        );
        assert_eq!(
            safe_default(QuestionAbout::Operational, "Which file?", Some(" ")),
            None
        );
        assert_eq!(
            safe_default(QuestionAbout::Operational, "Which file?", None),
            None
        );
    }
}

#[cfg(test)]
mod handoff_gap_tests {
    use super::handoff_gaps;

    #[test]
    fn a_note_missing_headings_names_them() {
        let whole = "Goal and where it stands: x\nDone: a1\nIn progress: y\nNext steps: 1\nDecisions and approvals already given: none\nGotchas learned: z\nChecks that passed: cargo test at a1\nHow to verify: cargo test";
        assert!(handoff_gaps(whole).is_empty());
        assert_eq!(
            handoff_gaps("Goal: x. Done: y. Next steps: z."),
            [
                "In progress",
                "Decisions and approvals already given",
                "Gotchas learned",
                "Checks that passed",
                "How to verify"
            ]
        );
    }
}

/// Whether the worktree at `path` holds the repository's tracked setup script for new
/// worktrees ([`super::prompts::SETUP_SCRIPT`]).
pub(super) fn has_setup(git: &brigadier_git::Git, path: &Path) -> bool {
    path.join(super::prompts::SETUP_SCRIPT).is_file()
        && git
            .open(path)
            .and_then(|worktree| worktree.tracks_under(super::prompts::SETUP_SCRIPT))
            .unwrap_or(false)
}
