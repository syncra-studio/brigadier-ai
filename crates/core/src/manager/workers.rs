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
//! - **Access** per task (B12) and the session's permission level: Full access runs the
//!   worker like the user's own terminal (no sandbox, nothing asks); Approve for me runs it in
//!   the OS sandbox and lets the CLI's own reviewer settle what leaves it; Ask for approval
//!   runs it in the sandbox without network and asks the user for each step outside, once per
//!   kind of command with "Allow similar commands". The sandbox lets a worker write its
//!   worktree's git folder and the toolchains' caches, so builds and commits just work.
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
use brigadier_providers::policy::{self, ApprovalMode, Route as PolicyRoute};
use brigadier_providers::{
    Access, AllowedModels, ApprovalDecision, ApprovalRequest, Artifact, Decider, InputFile, Origin,
    ProviderEvent, ProviderKind, SessionSpec, Started, ToolSet, TurnInput, TurnStatus,
};
use brigadier_router::QualityTier;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::brains::ReportLearning;
use super::conversation::{Cli, Envelope, safe_file_name};
use super::outputs::outputs_dir;
use super::usage::TokenOwner;
use super::watchdog::WorkerWatch;
use super::worker_handoff::Handover;
use super::{
    EventSource, SessionManager, blocking, fallback, git_error, instructions, prompts, secrets,
    warm,
};
use crate::model::{
    ConversationId, DomainEvent, Environment, ModelChoice, PermissionLevel, Setup, streams,
};
use crate::routing::TokenMeter;
use crate::runtime::{is_delta, merge_delta};
use crate::tools::Role;
use crate::work::{
    ApprovalSubject, ArtifactKind, ArtifactRef, AttachmentRef, Attempt, AttemptEnd, GateLink,
    GateOwner, InjectionKind, QuestionKind, QuotaWait, RepoAccess, Report, Route, Task, TaskId,
    TaskKind, TaskState, TaskWorkspace, WaitingSource, WorkerAccess, WorkerRole,
};
use crate::{Error, Result, now_ms};

/// Text deltas arriving within this window are stored as one event.
const DELTA_WINDOW: Duration = Duration::from_millis(30);
/// The CLIs' own limit on a worker's MCP calls: effectively none, since Codex does not cancel
/// a call it timed out; Brigadier bounds `ask_orchestrator` itself.
const WORKER_TOOL_TIMEOUT_SECS: u64 = 24 * 60 * 60;
/// How long the watchdog's nudge may take to reach a silent worker's CLI.
const NUDGE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a fix Brigadier lands waits for the worker's turn that reported it to end.
const TURN_END_WAIT: Duration = Duration::from_secs(30);
/// How long `ask_orchestrator` waits for the orchestrator's answer.
const QUESTION_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// Report size cap: about 800 tokens.
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
    access: Option<Access>,
    /// Where the worker's CLI runs (a command without its own cwd runs here).
    cwd: Option<PathBuf>,
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
    /// A fresh session's first message, held while the task is paused: resuming starts it.
    held_handover: Option<TurnInput>,
    /// When the worker last showed it works (an event from its CLI, or a turn started); the
    /// stall watchdog measures silence from it.
    last_event_ms: i64,
    /// The commands and tool calls the worker started that have not finished.
    running_items: std::collections::HashSet<String>,
    /// When the watchdog nudged the silent worker of this CLI session.
    stall_nudged_at_ms: Option<i64>,
    /// Fresh sessions the watchdog started for stalls in the current attempt.
    stalls: u32,
    /// Since when the task is live without a running CLI, as the watchdog saw it.
    orphaned_at_ms: Option<i64>,
    /// The models the CLI session's sub-agents were held to when it started (PLAN.md §7).
    allowed_models: Option<AllowedModels>,
}

impl TaskLiveState {
    /// A turn starts: the worker works, and its silence is measured from now.
    fn begin_turn(&mut self) {
        self.busy = true;
        self.last_event_ms = now_ms();
    }

    /// Whether the CLI session started with looser model limits than `now` (a rule took a
    /// model away since): it must start again before its next turn, or its sub-agents keep
    /// the old ones. Limits that only widened leave the session as it is.
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
}

impl TaskLive {
    /// Records why the running model must hand the task on when its turn ends.
    pub(crate) async fn set_cutoff(&self, end: AttemptEnd) {
        self.state.lock().await.cutoff = Some(end);
    }

    /// Takes the pending hand-off, if any.
    pub(crate) async fn take_cutoff(&self) -> Option<AttemptEnd> {
        self.state.lock().await.cutoff.take()
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

    /// Whether to ask the worker, now, to end its turn with a handoff note: once per turn,
    /// only while a turn runs that no hand-on or question waits in.
    pub(crate) async fn ask_handoff(&self) -> bool {
        let mut state = self.state.lock().await;
        let ask = state.busy
            && !state.handoff_asked
            && !state.stopping
            && state.cutoff.is_none()
            && state.question.is_none();
        if ask {
            state.handoff_asked = true;
        }
        ask
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
            ProviderEvent::TurnCompleted { .. } | ProviderEvent::Exited { .. } => {
                state.running_items.clear();
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
        state.last_event_ms = now_ms();
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
            last_event_ms: state.last_event_ms,
            nudged_at_ms: state.stall_nudged_at_ms,
            stalls: state.stalls,
            orphaned_at_ms: state.orphaned_at_ms,
            reroute_free,
        }
    }

    /// Asks the silent worker (a steer) whether it is stuck, if `due` still holds for it as it
    /// is now ([`super::watchdog::still_due`]). The steer is bounded and runs without the
    /// worker's state held, so a CLI that stopped reading holds up nothing else. Delivered or
    /// not, the nudge counts for the CLI session it was meant for: a worker that stays silent
    /// is handed over after the grace. `None` when it was no longer due; else whether it
    /// was delivered.
    pub(crate) async fn nudge_stall(
        &self,
        due: impl FnOnce(&WorkerWatch) -> bool,
        text: String,
    ) -> Option<bool> {
        let reroute_free = self.reroute.try_lock().is_ok();
        let (cli, generation) = {
            let mut state = self.state.lock().await;
            if !due(&Self::watched(&mut state, now_ms(), reroute_free)) {
                return None;
            }
            (state.cli.clone()?, state.generation)
        };
        // Activity from here on shows the worker heard it, or was never stuck.
        let at = now_ms();
        let delivered = match tokio::time::timeout(
            NUDGE_TIMEOUT,
            cli.session.steer(TurnInput::text(text)),
        )
        .await
        {
            Ok(Ok(())) => true,
            Ok(Err(err)) => {
                tracing::warn!(task = %self.id, error = %err, "could not nudge a silent worker");
                false
            }
            Err(_) => {
                tracing::warn!(task = %self.id, "nudging a silent worker timed out");
                false
            }
        };
        let mut state = self.state.lock().await;
        if state.generation == generation {
            state.stall_nudged_at_ms = Some(at);
        }
        Some(delivered)
    }

    /// Takes the CLI session of a stalled worker off its task, if `due` still holds for it as
    /// it is now ([`super::watchdog::still_due`]); the caller holds `reroute`, and `settle`
    /// so no report is recorded meanwhile. From now on the session's end is this hand-on, not
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

    /// The worker's CLI session, while one runs.
    #[cfg(debug_assertions)]
    pub(crate) async fn cli(&self) -> Option<Arc<Cli>> {
        self.state.lock().await.cli.clone()
    }

    fn new(id: TaskId, conversation_id: ConversationId) -> Self {
        Self {
            id,
            conversation_id,
            state: tokio::sync::Mutex::new(TaskLiveState::default()),
            settle: tokio::sync::Mutex::new(()),
            reroute: tokio::sync::Mutex::new(()),
            wait_timer: std::sync::atomic::AtomicU64::new(0),
            watchdog_busy: std::sync::atomic::AtomicBool::new(false),
            learning: Arc::default(),
            turn_end: tokio::sync::Mutex::new(()),
        }
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
                Arc::new(TaskLive::new(task.id.clone(), task.conversation_id.clone()))
            })
            .clone()
    }

    /// The session's permission level. While an overnight run is active nobody is there to
    /// ask: the run keeps the session's access and approves for the user, so Ask for approval
    /// counts as Approve for me until the run ends (PLAN.md §10.8).
    pub(crate) fn permission(&self, id: &ConversationId) -> PermissionLevel {
        let saved = match self.core.conversation(id).map(|c| c.setup) {
            Ok(Some(Setup::Session { permission, .. })) => permission,
            _ => PermissionLevel::ApproveForMe,
        };
        match saved {
            PermissionLevel::AskForApproval if self.overnight.active.get(id).is_some() => {
                PermissionLevel::ApproveForMe
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
        Ok(Some(task))
    }

    pub(crate) async fn set_task_state(
        &self,
        conversation_id: &ConversationId,
        id: &TaskId,
        state: TaskState,
    ) -> Result<Task> {
        self.update_task(conversation_id, id, |task| {
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
                if task.state.is_final() {
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
        let conversation = self.core.conversation(conversation_id)?;
        if !matches!(conversation.setup, Some(Setup::Session { .. })) {
            return Err(Error::Invalid("tasks belong to a session".into()));
        }
        let permission = self.permission(conversation_id);
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
        // Phase 0 only writes the plan: nothing changes the code before its phases are judged.
        if kind.writes()
            && extra.run.is_none()
            && active_run.as_ref().is_some_and(|active| active.planning)
        {
            return Err(Error::Invalid(
                "Phase 0 of this overnight run only writes the plan: propose its phases with propose_phases. Scouts and research may look around; nothing is changed before the phases are reviewed and judged.".into(),
            ));
        }
        let run = match extra.run {
            Some(run) => Some(run),
            None => super::overnight::policy::ActiveRun::context_for(
                active_run.as_ref(),
                subject.as_ref(),
            ),
        };
        let category = extra.category.unwrap_or_else(|| category(kind));
        let areas = areas.unwrap_or_else(|| brigadier_router::infer_areas(&spec));
        let floor = floor.unwrap_or_else(|| brigadier_router::default_floor(category));
        let (preview, trial_slot) = self
            .preview(&super::routing::Ask {
                category,
                areas: &areas,
                floor,
                needs: needs_of(&attachments, &needs),
                pin: pin.clone(),
                hold_pin: false,
                avoid,
                distinct_from,
                exclude: &[],
                project_id: conversation.project_id.as_ref(),
                // Checks of another task's work never go to a model on trial.
                trial: if gate_link.is_some() {
                    super::routing::Trial::Never
                } else {
                    super::routing::Trial::Take
                },
            })
            .await;
        let now = now_ms();
        let (route, wait) = match preview.decision {
            brigadier_router::Decision::Run(routed) => (super::routing::route_from(routed), None),
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
                };
                (route, Some(wait))
            }
        };
        let waits = wait.is_some();
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
            (_, _, Some(request)) => Some(request),
            (Some(subject), _, None) => subject.request_id.clone(),
            (None, Some(request), None) => Some(request),
            (None, None, None) => self.request_for(conversation_id, None).await,
        };
        let task = Task {
            id: TaskId::generate(),
            conversation_id: conversation_id.clone(),
            number,
            position: 0,
            title: title.trim().to_owned(),
            kind,
            spec,
            access: access_for(kind, permission),
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
            quota_wait: wait,
            subject: subject.as_ref().map(|task| task.id.clone()),
            plan: None,
            attachments,
            workspace: None,
            report: None,
            candidate: None,
            gate_link,
            role: extra.role,
            phase: extra.phase,
            landing: None,
            landed: None,
            error: None,
            kept: None,
            outputs: Vec::new(),
            request_id,
            run,
            messages: Vec::new(),
            rework_rounds: 0,
            native_session: None,
            trial_slot: waits && trial_slot,
            created_at_ms: now,
            updated_at_ms: now,
        };
        let mut events = vec![DomainEvent::TaskUpdated {
            task: Box::new(task.clone()),
        }];
        events.extend(worker_step(&task, None, false));
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
                });
            })
            .await?;
        let files = self.worker_files(&task, &workspace.scratch).await;
        // Instructions the orchestrator sent while the task waited to start go with it.
        let mut text = String::from("Start the task.");
        if !task.messages.is_empty() {
            text.push_str("\n\nLater instructions from the orchestrator, oldest first:\n");
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
        .await
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
        // A new worker waits while the machine is hot or short on memory (PLAN.md §10.7).
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
        let launched = self
            .launch_admitted(live, task, subject, origin, first)
            .await;
        if launched.is_err() {
            self.release_run_task(&task.id);
        }
        launched
    }

    /// [`Self::launch_worker`] once admitted.
    async fn launch_admitted(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        subject: Option<&Task>,
        origin: Origin,
        first: TurnInput,
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
        let recorded = task
            .workspace
            .clone()
            .ok_or_else(|| Error::Invalid("the task has no workspace".into()))?;
        let conversation = self.core.conversation(&conversation_id)?;
        let Some(Setup::Session { repo, .. }) = &conversation.setup else {
            return Err(Error::Invalid("tasks belong to a session".into()));
        };
        let workspace = Workspace {
            repo: PathBuf::from(repo),
            worktree: recorded.worktree.map(PathBuf::from),
            branch: recorded.branch,
            base: recorded.base.map(Oid),
            on_snapshot: recorded.on_snapshot,
            target: recorded.target,
            scratch: PathBuf::from(recorded.scratch),
            warmed: recorded.warmed,
        };
        let project = conversation
            .project_id
            .as_ref()
            .and_then(|id| self.core.project(id).ok());
        let secret_files = project
            .as_ref()
            .map(|p| p.prefs.secret_files.clone())
            .unwrap_or_default();
        // A successor taking the task over finds the worktree's secrets already in place.
        let mut secret_values = match (&workspace.worktree, &origin) {
            (Some(worktree), Origin::New) if task.attempts.len() <= 1 => {
                secrets::copy_secrets(self, &owner, &workspace.repo, worktree, &secret_files)
                    .await?
            }
            _ => secrets::values(&workspace.repo, &secret_files).await,
        };

        let provider = task.route.choice.provider;
        let write = task.kind.writes();
        // Codex cannot run with a read-only cwd: a read-only Codex worker works from its
        // scratch folder and reads the worktree by path.
        let cwd = match (&workspace.worktree, provider, write) {
            (Some(worktree), ProviderKind::Claude, _) => worktree.clone(),
            (Some(worktree), _, true) => worktree.clone(),
            _ => workspace.scratch.clone(),
        };
        let repo_note = match (&workspace.worktree, write) {
            (Some(worktree), true) if cwd != *worktree => format!(
                "Your worktree (a checkout of the repository on branch `{}`): {}\nYou start in your scratch folder: run every command in your worktree (`cd` there first, or `git -C <worktree>`), and edit its files by their full path.\nYour scratch folder: {}",
                workspace.branch.clone().unwrap_or_default(),
                worktree.display(),
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
            "{repo_note}\nYour outputs folder (files for the orchestrator and the user): {}",
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
        let access = self.worker_access(task, &workspace, &cwd);
        repo_note.push_str("\n\n");
        repo_note.push_str(&prompts::environment(&prompts::WorkerEnvironment {
            access: &access,
            warmed: &workspace.warmed,
            test_dir: &test_dir,
            run_repo: task.run.as_ref().map(|_| workspace.repo.as_path()),
            branch: workspace.branch.as_deref(),
            low_priority: true,
        }));
        let native = match &workspace.worktree {
            Some(worktree) => instructions::for_worker(provider, worktree).await,
            None => String::new(),
        };
        let mut extra = match (&task.kind, subject) {
            (TaskKind::Review | TaskKind::Verify, Some(subject)) => {
                self.review_brief(subject, &workspace.scratch).await
            }
            (TaskKind::Merge, Some(subject)) => self.merge_brief(subject, &workspace).await,
            _ => String::new(),
        };
        if self.held_by_plan_mode(task).await {
            extra.push_str("\n\n");
            extra.push_str(super::phases::PLAN_MODE_HOLD);
        }
        let prompt = prompts::worker(task, &repo_note, &native, &extra);

        let worker_grant = self.grants.issue(
            &owner,
            Role::Worker {
                conversation_id: conversation_id.clone(),
                task_id: task.id.clone(),
                checks: task.gate_link.is_some() || task.role == Some(WorkerRole::Reviewer),
            },
        );
        // B7: the grant is a secret too.
        secret_values.push(worker_grant.clone());
        let redactor = secrets::redactor(secret_values);
        let allowed_models = self.allowed_models(task).await;
        let spec = SessionSpec {
            cwd: cwd.clone(),
            model: task.route.choice.model.clone(),
            effort: task.route.choice.effort.clone(),
            fast: false,
            origin,
            access: access.clone(),
            append_system_prompt: Some(prompt),
            mcp_servers: vec![self.brigadier_server(worker_grant, WORKER_TOOL_TIMEOUT_SECS, true)],
            tools: ToolSet::Lean,
            env: vec![(
                "TMPDIR".into(),
                workspace.scratch.to_string_lossy().into_owned(),
            )],
            unset_env: Vec::new(),
            low_priority: true,
            record_to: None,
            redactor: redactor.clone(),
            owned_cwd: true,
            auto_compact: true,
            allowed_models: Some(allowed_models.clone()),
            auto_review: self.permission(&conversation_id) == PermissionLevel::ApproveForMe,
        };
        let Started { session, events } =
            match self.runtime.start_hosted(&owner, provider, spec).await {
                Ok(started) => started,
                Err(err) => {
                    self.grants.revoke_owner(&owner);
                    return Err(err);
                }
            };
        let cli = Arc::new(Cli {
            provider,
            meter: TokenMeter::new(continues),
            model: task.route.choice.clone(),
            chosen: None,
            session,
            owner,
            ended: CancellationToken::new(),
        });
        self.brains.jobs.user_work(provider);
        {
            let mut state = live.state.lock().await;
            state.cli = Some(cli.clone());
            state.access = Some(access);
            state.cwd = Some(cwd);
            state.outputs = Some(outputs);
            state.redactor = redactor;
            state.begin_turn();
            state.nudged = false;
            state.stopping = false;
            state.handoff_asked = false;
            // Any start supersedes a fresh session held for a paused task.
            state.held_handover = None;
            state.generation += 1;
            state.running_items.clear();
            state.stall_nudged_at_ms = None;
            state.orphaned_at_ms = None;
            state.allowed_models = Some(allowed_models);
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
        self.set_task_state(&conversation_id, &task.id, TaskState::Running)
            .await?;
        cli.session
            .send(first)
            .await
            .map_err(|err| Error::Provider(err.to_string()))
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
            })
            .await?;
        let from = live.generation().await;
        if self.worker_over_handoff(live, &task.id).await {
            self.hand_over_worker(live, &task, from, None, Some(text), Handover::Size)
                .await?;
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
        self.sent_back(&task).await;
        Ok(())
    }

    /// The worker's CLI session to resume: the one recorded on the task, else (a task recorded
    /// before it was kept there) the latest session start in its events, which is then
    /// recorded.
    async fn last_worker_native_id(&self, task: &Task) -> Option<String> {
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

    /// B12: what the worker may touch.
    fn worker_access(&self, task: &Task, workspace: &Workspace, cwd: &Path) -> Access {
        // Full access: like the user's own terminal.
        if task.access.unsandboxed {
            return Access::Full;
        }
        // A worker working from its scratch folder writes there (Codex needs a writable cwd);
        // one in a worktree writes to the worktree only for write tasks.
        let in_scratch = cwd == workspace.scratch;
        let write_cwd = in_scratch || task.kind.writes();
        let mut writable_roots = if in_scratch {
            Vec::new()
        } else {
            vec![workspace.scratch.clone()]
        };
        writable_roots.push(test_data_dir(&task.id));
        if let Some(worktree) = &workspace.worktree
            && task.kind == TaskKind::Verify
        {
            // Checks write build output inside the checkout; nothing from it lands.
            writable_roots.push(worktree.clone());
        }
        // A writer commits: its worktree's git folder (the repository's own, which a linked
        // worktree shares) takes the objects and refs.
        if task.kind.writes()
            && let Some(worktree) = &workspace.worktree
            && let Ok(repo) = self.git.open(worktree)
        {
            writable_roots.push(repo.common_dir().to_owned());
        }
        // Builds and installs write the toolchains' shared caches.
        for root in toolchain_roots(self.runtime.cli_env()) {
            if !writable_roots.contains(&root) {
                writable_roots.push(root);
            }
        }
        Access::Scoped {
            write_cwd,
            writable_roots,
            network: task.access.network,
            deny_read: vec![self.runtime.platform().paths().run_dir.clone()],
            unix_sockets: self.socket_path().into_iter().collect(),
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
        let Some(Setup::Session {
            repo, environment, ..
        }) = &conversation.setup
        else {
            return Err(Error::Invalid("tasks belong to a session".into()));
        };
        let scratch = self.owned_dir("scratch", &task.id.0);
        self.prepare_owned_dir(owner, &scratch).await?;
        let repo = PathBuf::from(repo);
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
            });
        }
        // An overnight run's work lands on the run's branch, recorded with the task so it
        // stays there whatever happens to the run later.
        let run_branch = match &task.run {
            Some(run) => Some(self.run_target(&task.conversation_id, run).await?),
            None => None,
        };
        let target = match &run_branch {
            Some(branch) => branch.clone(),
            None => {
                self.ensure_target(&task.conversation_id, &repo, environment)
                    .await?
            }
        };
        // Reviews and checks look at the candidate commit; a merge task continues from the
        // conflicting task's work (kept as a WIP commit on its branch).
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
                let (base, start) = self.merge_start(subject).await?;
                (base, start, false)
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
            // A whole-phase check looks at the phase's candidate exactly.
            _ if let Some(candidate) = task.run.as_ref().and_then(|run| run.candidate.clone()) => {
                let commit = Oid(candidate);
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
                    .worker_base(&task.conversation_id, &repo, &target)
                    .await?;
                (base.clone(), base, on_snapshot)
            }
        };
        let project = conversation
            .project_id
            .as_ref()
            .map(|id| id.0.clone())
            .unwrap_or_else(|| "none".into());
        let worktree = self.owned_dir("worktrees", &project).join(format!(
            "task-{}-{}",
            task.number,
            &task.id.0[task.id.0.len() - 8..]
        ));
        let branch = task
            .kind
            .writes()
            .then(|| task_branch(&task.conversation_id, task.number, &task.title));
        let ledger = self.runtime.ledger();
        ledger
            .record(
                owner,
                Artifact::Worktree {
                    repo: repo.to_string_lossy().into_owned(),
                    path: worktree.to_string_lossy().into_owned(),
                },
            )
            .await?;
        ledger
            .record(
                owner,
                Artifact::ProcessesIn {
                    dir: worktree.to_string_lossy().into_owned(),
                },
            )
            .await?;
        let (git, repo_path, path, spec) = (
            self.git.clone(),
            repo.clone(),
            worktree.clone(),
            match &branch {
                Some(name) => WorktreeSpec::NewBranch {
                    name: name.clone(),
                    start: start.clone(),
                },
                None => WorktreeSpec::Detached { at: start.clone() },
            },
        );
        // Workers that build or test start from copies of the checkout's dependency installs
        // and build caches (best effort; see `warm`).
        let warm = task.kind.writes() || matches!(task.kind, TaskKind::Review | TaskKind::Verify);
        let (platform, task_id) = (self.runtime.platform().clone(), task.id.clone());
        let warmed = blocking(move || {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|err| Error::Invalid(err.to_string()))?;
            }
            let repo = git.open(&repo_path).map_err(git_error)?;
            repo.add_worktree(&path, spec).map_err(git_error)?;
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
        .await?;
        Ok(Workspace {
            repo,
            worktree: Some(worktree),
            branch,
            base: Some(base),
            on_snapshot,
            target: Some(target),
            scratch,
            warmed,
        })
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
                let project = conversation
                    .project_id
                    .as_ref()
                    .map(|id| id.0.clone())
                    .unwrap_or_else(|| "none".into());
                let worktree = self
                    .owned_dir("worktrees", &project)
                    .join(format!("session-{}", conversation_id.short()));
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
                blocking(move || {
                    let repo = git.open(&repo_path).map_err(git_error)?;
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(|err| Error::Invalid(err.to_string()))?;
                    }
                    let spec = match repo.branch_tip(&name).map_err(git_error)? {
                        Some(_) => WorktreeSpec::Branch { name },
                        None => WorktreeSpec::NewBranch {
                            name,
                            start: match &from {
                                Some(commit) => repo.resolve(commit),
                                None => repo.branch_commit(&base_name),
                            }
                            .map_err(git_error)?,
                        },
                    };
                    repo.add_worktree(&path, spec)
                        .map(|_| ())
                        .map_err(git_error)
                })
                .await?;
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
                Ok(branch.clone())
            }
        }
    }

    /// Where workers start: the target's tip, or a snapshot of the user's uncommitted changes
    /// on top of it when the user chose to show them (local checkout only; then `true`).
    async fn worker_base(
        &self,
        conversation_id: &ConversationId,
        repo: &Path,
        target: &str,
    ) -> Result<(Oid, bool)> {
        let (git, repo_path, branch) = (self.git.clone(), repo.to_owned(), target.to_owned());
        let (tip, dirty, current) = blocking(move || {
            let repo = git.open(&repo_path).map_err(git_error)?;
            let tip = repo
                .branch_tip(&branch)
                .map_err(git_error)?
                .ok_or_else(|| Error::Invalid(format!("branch {branch} does not exist")))?;
            let state = repo.state().map_err(git_error)?;
            Ok((tip, state.dirty_files, state.current_branch))
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
            None => self.ask_about_uncommitted(conversation_id, dirty).await?,
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

    /// Asks once whether workers see the user's uncommitted changes; tasks wait for it.
    async fn ask_about_uncommitted(
        &self,
        conversation_id: &ConversationId,
        files: Vec<String>,
    ) -> Result<bool> {
        // Only one card at a time: later tasks wait for the same answer.
        let board = self.core.board(conversation_id).await?;
        let open = board.questions.values().find(|q| {
            q.answer.is_none() && matches!(q.kind, QuestionKind::UncommittedChanges { .. })
        });
        let rx = match open {
            Some(question) => self.waiters_for_question(question.id.clone()),
            None => {
                let shown: Vec<String> = files.iter().take(50).cloned().collect();
                let (_, rx) = self
                    .open_question(
                        conversation_id,
                        None,
                        QuestionKind::UncommittedChanges { files: shown },
                        format!(
                            "Your checkout has {} uncommitted change{}. Should workers see them? They are never committed either way.",
                            files.len(),
                            if files.len() == 1 { "" } else { "s" }
                        ),
                        vec!["Yes, show them to workers".into(), "No, use the last commit".into()],
                        None,
                    )
                    .await?;
                rx
            }
        };
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
            let path = scratch
                .join("attachments")
                .join(safe_file_name(&attachment.name));
            let target = path.clone();
            let written = blocking(move || {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|err| Error::Invalid(err.to_string()))?;
                }
                std::fs::write(&target, bytes).map_err(|err| Error::Invalid(err.to_string()))
            })
            .await;
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
        loop {
            let flush_at = async {
                match deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                event = events.recv() => match event {
                    // It only shows the worker is alive: never stored.
                    Some(event @ ProviderEvent::Progress { .. }) => {
                        live.note_event(&cli, &event).await;
                    }
                    Some(event) if is_delta(&event) => {
                        live.note_event(&cli, &event).await;
                        merge_delta(&mut deltas, event);
                        deadline.get_or_insert_with(|| tokio::time::Instant::now() + DELTA_WINDOW);
                    }
                    Some(event) => {
                        live.note_event(&cli, &event).await;
                        deadline = None;
                        self.record_worker_events(&live.id, std::mem::take(&mut deltas)).await;
                        let mut exited = false;
                        for event in self.session_events(EventSource::Task(&live.id), &cli, event) {
                            exited |= matches!(event, ProviderEvent::Exited { .. });
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
        // A fresh session that already took the task over keeps its run's worker slot.
        if !replaced {
            self.release_run_task(&live.id);
        }
        if !stopping
            && let Ok(task) = self.task_by_id(&live.conversation_id, &live.id).await
            && !task.state.is_final()
            && task.report.is_none()
        {
            // Another model takes over (up to a few times for errors, then the task fails).
            let end = live.take_cutoff().await.unwrap_or(AttemptEnd::Error {
                kind: brigadier_providers::ErrorKind::Process,
                message: "The worker's CLI exited before it reported.".into(),
            });
            let manager = self.arc();
            let live = live.clone();
            self.spawn(async move { manager.hand_off(&live, end, from).await });
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
                self.runtime.note_quota_snapshot(quota.clone()).await;
            }
            ProviderEvent::Usage { total, last } => {
                self.note_tokens(
                    &cli.meter,
                    cli.provider,
                    cli.model.model.as_deref(),
                    TokenOwner::Task(&live.conversation_id, &live.id),
                    total,
                    last.as_ref(),
                )
                .await;
            }
            ProviderEvent::ContextSize {
                used_tokens,
                window_tokens,
            } => {
                self.worker_context(live, cli, *used_tokens, *window_tokens)
                    .await;
            }
            ProviderEvent::TurnStarted { .. } => {
                live.state.lock().await.last_message = None;
            }
            ProviderEvent::SessionStarted { native_id, .. } => {
                self.note_native_session(&live.conversation_id, &live.id, native_id)
                    .await;
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
        if let Some(status) = completed {
            self.worker_turn_completed(live, cli, status).await;
            // No turn followed (a nudge or retry would have started one): an overnight run's
            // worker slot is free for another task.
            if !live.busy().await {
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
        let (nudge, handoff_asked, from) = {
            let mut state = live.state.lock().await;
            state.busy = false;
            let handoff_asked = std::mem::take(&mut state.handoff_asked);
            if state.stopping {
                return;
            }
            let nudge = !state.nudged;
            state.nudged = true;
            (nudge, handoff_asked, state.generation)
        };
        let Ok(task) = self.task_by_id(&live.conversation_id, &live.id).await else {
            return;
        };
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
        if nudge {
            let text = match self.keep_last_message(live, task.number).await {
                Some(_) => {
                    "You ended your turn without calling submit_report. The orchestrator reads only your report, never your messages, so your last message has not reached it. Brigadier kept it and attaches it to your report as an artifact. If the task is done or you cannot continue, call submit_report now with a short summary; otherwise continue working."
                }
                None => {
                    "You ended your turn without calling submit_report. The orchestrator reads only your report, never your messages. If the task is done or you cannot continue, call submit_report now; otherwise continue working."
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

    /// B7 for worker approvals: routed by the task's access, then by what the user already
    /// allowed with "Allow similar commands" in this conversation; anything else asks the user.
    /// Under Full access and Approve for me hardly anything gets here: the CLI never asks, or
    /// its own reviewer answers.
    async fn route_worker_approval(
        &self,
        live: &Arc<TaskLive>,
        cli: &Arc<Cli>,
        request: ApprovalRequest,
    ) {
        let access = live
            .state
            .lock()
            .await
            .access
            .clone()
            .unwrap_or(Access::ReadOnly);
        let mut route = policy::route(&request, &access, ApprovalMode::Delegated);
        let mut decider = Decider::Policy;
        if route == PolicyRoute::AskUser
            && self
                .waiters
                .similar_allowed(&live.conversation_id, &request)
        {
            route = PolicyRoute::Allow;
            decider = Decider::User;
        }
        match route {
            PolicyRoute::Allow | PolicyRoute::Deny => {
                let decision = if route == PolicyRoute::Allow {
                    ApprovalDecision::Allow
                } else {
                    ApprovalDecision::Deny {
                        message: "This session may not do that.".into(),
                    }
                };
                if let Err(err) = cli
                    .session
                    .answer(request.id.clone(), decision.clone())
                    .await
                {
                    tracing::warn!(task = %live.id, error = %err, "could not answer an approval");
                    return;
                }
                self.record_worker_resolution(&live.id, request.id, decision, decider)
                    .await;
            }
            PolicyRoute::AskUser => {
                let what = request
                    .command
                    .clone()
                    .unwrap_or_else(|| request.tool.clone());
                if let Err(err) = self
                    .open_approval(
                        &live.conversation_id,
                        Some(live.id.clone()),
                        ApprovalSubject::Cli { request },
                    )
                    .await
                {
                    tracing::warn!(task = %live.id, error = %err, "could not open an approval card");
                    return;
                }
                self.set_task_blocked(&live.id, Some(format!("Waiting for approval: {what}")))
                    .await;
            }
        }
    }

    /// Passes the user's answer to the worker's CLI; "Allow similar commands" also allows
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

    /// `ask_orchestrator`: blocks the worker until the orchestrator answers.
    pub(crate) async fn worker_question(
        &self,
        conversation_id: &ConversationId,
        task_id: &TaskId,
        question: String,
    ) -> Result<String> {
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
        self.set_task_blocked(task_id, Some(format!("Asked the orchestrator: {question}")))
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
        let answer = tokio::time::timeout(QUESTION_TIMEOUT, rx).await;
        self.set_task_blocked(task_id, None).await;
        match answer {
            Ok(Ok(answer)) => Ok(answer),
            Ok(Err(_)) => Err(Error::Invalid(
                "the question was withdrawn (the task ended)".into(),
            )),
            Err(_) => {
                live.state.lock().await.question = None;
                Err(Error::Invalid(
                    "The orchestrator did not answer within an hour. Continue with your best judgement and say so in the report.".into(),
                ))
            }
        }
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
        let _ = waiter.send(answer.clone());
        self.record_answer(task, question, answer, why).await;
        Ok(format!("Answered task-{}; it continues.", task.number))
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
                "Your report for task-{} was already received and stands; nothing was changed. A new report is taken only after the orchestrator sends you back to work. End your turn now.",
                task.number
            )));
        }
        if self.held_by_plan_mode(&task).await {
            return Err(Error::Invalid(super::phases::PLAN_MODE_HOLD.into()));
        }
        let size = input.summary.len()
            + [
                &input.changes,
                &input.decisions,
                &input.verification,
                &input.done_when,
                &input.open_questions,
                &input.risks,
                &input.needs_user,
            ]
            .iter()
            .flat_map(|items| items.iter())
            .map(|item| item.len() + 3)
            .sum::<usize>();
        if size > REPORT_MAX_BYTES {
            return Err(Error::Invalid(format!(
                "The report is {size} bytes; the limit is about {REPORT_MAX_BYTES} (≈800 tokens). Move the details into a file in your outputs folder, name it under `artifacts`, and submit a shorter report."
            )));
        }
        // Everything it names is stored now, before its folders can go.
        let texts: Vec<String> = std::iter::once(&input.summary)
            .chain(&input.decisions)
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
        // The report after a self-check Brigadier asked for (its commits were rebased during
        // their landing) lands on its own, unless the worker says only the user can unblock
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
        // A phase with an outline ends with a fresh verifier, which the orchestrator lands.
        let verify = !relanding && !reviewing && !unchanged && self.needs_verifier(&task).await;
        let report = Report {
            summary: self.redact_for(&live, &input.summary).await,
            changes: input.changes.clone(),
            decisions: self.redact_all(&live, &input.decisions).await,
            verification: self.redact_all(&live, &input.verification).await,
            done_when: self.redact_all(&live, &input.done_when).await,
            open_questions: self.redact_all(&live, &input.open_questions).await,
            risks: self.redact_all(&live, &input.risks).await,
            needs_user: self.redact_all(&live, &input.needs_user).await,
            verdict: input.verdict,
            checks: input.checks.filter(|_| task.kind == TaskKind::Verify),
            artifacts,
            submitted_at_ms: now_ms(),
        };
        // Started before the orchestrator reads the report, so it never lands the lead alone.
        let verifier = if verify {
            Some(self.start_verifier(&task, &report).await)
        } else {
            None
        };
        let reported = |task: &mut Task| {
            task.report = Some(report.clone());
            task.outputs.clone_from(&outputs);
            task.state = TaskState::Reported;
            task.blocked_reason = None;
            if task.kind.writes() && !relanding {
                // The orchestrator decides about this report: Brigadier no longer lands it on
                // its own.
                task.landing = None;
            }
        };
        let settled = live.settle.lock().await;
        // Stopped while its report was being stored: the stop stands.
        let task = self.task_by_id(conversation_id, task_id).await?;
        if task.state.is_final() {
            return Err(Error::Invalid("the task has already ended".into()));
        }
        // A review someone waits for goes to them (a worker's request_review, an outline's
        // review for the orchestrator), not to the orchestrator as a report of its own.
        let reviewing = reviewing
            || (task.role == Some(WorkerRole::Reviewer)
                && self
                    .reviews
                    .settle(&task.id, Ok(super::phases::review_text(&report))));
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
            match &verifier {
                Some(Ok(verifier)) => text.push_str(&format!(
                    "\n[phase verifier] Brigadier started task-{v}, a fresh verifier of this phase, on top of task-{n}'s commits. Land the phase with land_phase on task-{v} once it reports, not on task-{n}.",
                    v = verifier.number,
                    n = task.number
                )),
                Some(Err(err)) => text.push_str(&format!(
                    "\n[phase verifier] The phase's verifier could not start: {err}. Delegate one, or land task-{} with land_phase yourself.",
                    task.number
                )),
                None => {}
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
        if reviewing {
            self.gate_member_reported(&task).await;
        } else if relanding {
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
                manager.land_after_self_check(&now).await;
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
        let envelope = Envelope {
            kind: InjectionKind::Report,
            label: addendum_label(task.number),
            task_id: Some(task.id.clone()),
            text: prompts::late_findings_envelope(&updated, &shown),
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
                    .message_worker_admitted(&conversation_id, &now, text, &from)
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
            .message_worker_admitted(conversation_id, task, text, from)
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
    ) -> Result<(String, bool)> {
        let live = self.task_live(task);
        // What the task may use now, for a worker between turns (routing reads the board, so
        // not under the worker's lock).
        let idle = {
            let state = live.state.lock().await;
            !state.busy && state.cli.is_some()
        };
        let models = if idle && !task.state.is_final() {
            Some(self.allowed_models(task).await)
        } else {
            None
        };
        let mut state = live.state.lock().await;
        if let Some((question, waiter)) = state.question.take() {
            drop(state);
            let _ = waiter.send(text.clone());
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
        let hand_over = !state.busy
            && state.cli.is_some()
            && match state.context {
                Some(tokens) => self.worker_handoff_due(tokens, state.session_start, state.window),
                None => false,
            };
        let restart = !state.busy
            && state.cli.is_some()
            && models
                .as_ref()
                .is_some_and(|models| state.models_changed(models));
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
            self.revive_worker(&live, task, format!("Message from {from}:\n{text}"))
                .await?;
            return Ok((
                format!(
                    "task-{} is working on it; a new report will follow.",
                    task.number
                ),
                false,
            ));
        };
        let input = TurnInput::text(format!("Message from {from}:\n{text}"));
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
        let task = self
            .update_task(conversation_id, &task.id, |task| {
                task.state = TaskState::Running;
                task.candidate = None;
                task.rework_rounds += 1;
            })
            .await?;
        self.sent_back(&task).await;
        Ok(())
    }

    /// Its request must not end without an answer the user sees. An overnight run's request
    /// is answered by the run's report instead.
    async fn sent_back(&self, task: &Task) {
        if task.run.is_none()
            && let Some(request) = &task.request_id
            && let Ok(conv) = self.conv(&task.conversation_id)
        {
            conv.sent_back(request).await;
        }
    }

    /// Stops a worker for good (`stop_worker`, or the user's stop button). Unfinished changes
    /// are kept on the task branch as a WIP commit.
    pub async fn stop_task(&self, task_id: TaskId) -> Result<()> {
        let conversation_id = self.conversation_of_task(&task_id).await?;
        let live = self.existing_task_live(&task_id);
        // A report being recorded right now is recorded first (or not at all).
        let settled = match &live {
            Some(live) => Some(live.settle.lock().await),
            None => None,
        };
        let task = self.task_by_id(&conversation_id, &task_id).await?;
        if task.state.is_final() {
            return Ok(());
        }
        // Until it is recorded stopped, a landing that fails as its worktree goes hands
        // nothing back to the orchestrator.
        let _stopping = Stopping::mark(self, &task_id);
        // A gate member that gave its result already has its outcome under way.
        let reviewing = task.gate_link.is_some() && task.report.is_none();
        if let Some(live) = &live {
            live.close_cli().await;
        }
        self.dispose_task(&task, stopped_state(&task)).await;
        drop(settled);
        // Whoever waits for its review hears it gave none.
        self.reviews
            .settle(&task.id, Err("the reviewer was stopped".into()));
        // A landing's or plan's reviewer stopped before its verdict releases what it was
        // reviewing, as one that failed does: otherwise it would wait for a verdict that never
        // comes.
        if reviewing {
            self.gate_member_failed(&task, "It was stopped before it gave a result.")
                .await;
        }
        Ok(())
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
        let name = workspace
            .branch
            .clone()
            .unwrap_or_else(|| task_branch(&conversation_id, task.number, &task.title));
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
        let message = format!(
            "task-{} {} (restored from its saved patch)",
            task.number, task.title
        );
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
        let models = self.allowed_models(&task).await;
        {
            let _handing = live.reroute.lock().await;
            let changed = {
                let state = live.state.lock().await;
                state.cli.is_some() && state.models_changed(&models)
            };
            if changed {
                return self.restart_worker(&live, &task, continued).await;
            }
        }
        let cli = {
            let mut state = live.state.lock().await;
            state.begin_turn();
            state.nudged = false;
            state.cli.clone()
        }
        .ok_or_else(|| Error::Invalid("the worker has ended".into()))?;
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

    /// A worker failed: the task ends and the orchestrator hears why. A landing's or plan's
    /// reviewer failing releases what it was reviewing.
    pub(crate) async fn worker_failed(&self, task: &Task, reason: &str) {
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
        if reviewing {
            self.gate_member_failed(task, reason).await;
            return;
        }
        // Whoever waits for its review hears why there is none.
        if self.reviews.settle(&task.id, Err(reason.to_owned())) {
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
        if let Some(live) = self.existing_task_live(&task.id) {
            live.close_cli().await;
        }
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
        let leftovers = self.runtime.ledger().dispose(&owner).await;
        if !leftovers.is_clean() {
            tracing::warn!(task = %task.id, ?leftovers, "some of the task's leftovers will be retried at the next launch");
        }
        if drop_branch {
            self.drop_task_branch(task).await;
        }
        self.tasks_lock().remove(&task.id);
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
        let message = format!(
            "WIP: task-{} {} (unfinished, kept by Brigadier)",
            task.number, task.title
        );
        let base = workspace.base.clone().map(Oid);
        let on_snapshot = workspace.on_snapshot;
        let result = blocking(move || {
            let worktree = git.open_worktree(&path).map_err(git_error)?;
            worktree.commit_wip(&message).map_err(git_error)?;
            let head = worktree.head().map_err(git_error)?;
            let Some(base) = base.filter(|base| *base != head) else {
                return Ok(Unfinished::None);
            };
            if !on_snapshot {
                return Ok(Unfinished::Commit(head));
            }
            Ok(
                match worktree.drop_snapshot(&base, &message).map_err(git_error)? {
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

/// What a task needs from its model: its attachments, and the capabilities it asked for.
pub(crate) fn needs_of(
    attachments: &[AttachmentRef],
    capabilities: &[brigadier_router::Capability],
) -> brigadier_router::Needs {
    brigadier_router::Needs {
        image_input: attachments
            .iter()
            .any(|attachment| attachment.mime.starts_with("image/")),
        image_generation: capabilities.contains(&brigadier_router::Capability::ImageGeneration),
        context_tokens: None,
    }
}

/// The router's category for a task kind.
/// What only Brigadier sets on a task it makes itself.
#[derive(Debug, Clone, Default)]
pub(crate) struct TaskExtra {
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
}

pub(crate) fn category(kind: TaskKind) -> brigadier_router::TaskCategory {
    use brigadier_router::TaskCategory;
    match kind {
        TaskKind::Scout => TaskCategory::Scout,
        TaskKind::Research => TaskCategory::Research,
        TaskKind::Implement => TaskCategory::Implement,
        TaskKind::Review => TaskCategory::Review,
        TaskKind::Merge => TaskCategory::Merge,
        TaskKind::Verify => TaskCategory::Verify,
    }
}

/// Where a task's tests and smoke runs keep their data: under the system's temporary folder,
/// never the app's own data folder (a smoke run of a debug build must not touch it).
pub(crate) fn test_data_dir(id: &TaskId) -> PathBuf {
    let name = format!("brigadier-test-{}", &id.0[id.0.len().saturating_sub(8)..]);
    if cfg!(unix) {
        PathBuf::from("/tmp").join(name)
    } else {
        std::env::temp_dir().join(name)
    }
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

fn stopped_state(task: &Task) -> TaskState {
    if !task.kind.writes() && task.report.is_some() {
        TaskState::Done
    } else {
        TaskState::Stopped
    }
}

/// B12: repository access, network and sandbox per task kind and permission level. Under
/// Ask for approval a worker's sandbox has no network: reaching a host asks the user (research
/// tasks, which live on the web, keep it).
fn access_for(kind: TaskKind, permission: PermissionLevel) -> WorkerAccess {
    WorkerAccess {
        repo: match kind {
            TaskKind::Research => RepoAccess::None,
            TaskKind::Implement | TaskKind::Merge => RepoAccess::Write,
            TaskKind::Scout | TaskKind::Review | TaskKind::Verify => RepoAccess::Read,
        },
        network: permission != PermissionLevel::AskForApproval || kind == TaskKind::Research,
        unsandboxed: permission == PermissionLevel::FullAccess,
    }
}

/// Folders a sandboxed worker's builds, tests and installs write besides its own: the
/// toolchains' homes and caches (Rust, Node package managers, the system's caches) and the
/// system temporary folder, those that exist, and pnpm's store lock folder.
fn toolchain_roots(env: &brigadier_providers::cli::CliEnv) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for name in [
        "CARGO_HOME",
        "RUSTUP_HOME",
        "CARGO_TARGET_DIR",
        "PNPM_HOME",
        "npm_config_cache",
        "TMPDIR",
    ] {
        if let Some(value) = env.var(name).filter(|value| !value.is_empty()) {
            roots.push(PathBuf::from(value));
        }
    }
    if let Some(home) = env.home() {
        for rest in [
            ".cargo",
            ".rustup",
            ".npm",
            ".pnpm-store",
            ".yarn",
            ".bun",
            ".cache",
            ".local/share/pnpm",
            "Library/pnpm",
            "Library/Caches",
        ] {
            roots.push(home.join(rest));
        }
    }
    roots.retain(|root| root.is_absolute() && root.is_dir());
    // pnpm locks its store in a fixed folder of `/tmp` (not `$TMPDIR`), made on first use.
    #[cfg(unix)]
    if let Ok(tmp) = Path::new("/tmp").canonicalize() {
        let uid = nix::unistd::getuid();
        roots.push(tmp.join(format!("pnpm-store-operation-locks-{uid}")));
    }
    roots.dedup();
    roots
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

/// `brigadier/<session>/task-<n>-<slug>`.
fn task_branch(conversation_id: &ConversationId, number: u32, title: &str) -> String {
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
        format!("brigadier/{session}/task-{number}")
    } else {
        format!("brigadier/{session}/task-{number}-{slug}")
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

#[cfg(test)]
mod tests {
    use super::*;

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
            verification: Vec::new(),
            done_when: Vec::new(),
            open_questions: Vec::new(),
            risks: Vec::new(),
            needs_user: Vec::new(),
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
        let dir = std::env::temp_dir().join(format!("brigadier-resume-{}", uuid::Uuid::new_v4()));
        let store = tokio::task::spawn_blocking({
            let dir = dir.clone();
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
            }),
            event(ProviderEvent::SessionStarted {
                native_id: "latest".into(),
                model: None,
                cwd: None,
                cli_version: None,
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_nudge_no_longer_due_is_not_sent_or_counted() {
        let live = TaskLive::new(TaskId("t1".into()), ConversationId("c1".into()));
        assert_eq!(live.nudge_stall(|_| false, "carry on".into()).await, None);
        // Without a CLI there is nothing to nudge either.
        assert_eq!(live.nudge_stall(|_| true, "carry on".into()).await, None);
        assert_eq!(live.watch(0).await.nudged_at_ms, None);
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
                    verification: Vec::new(),
                    done_when: Vec::new(),
                    open_questions: Vec::new(),
                    risks: Vec::new(),
                    needs_user: Vec::new(),
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
