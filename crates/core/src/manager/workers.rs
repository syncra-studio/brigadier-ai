//! The worker runtime: one plain CLI session per worker (STRIP-DOWN-PLAN S13, S16b).
//!
//! `brigadier start` makes the task ([`super::plain_workers`]); Brigadier gives it a workspace
//! and starts its CLI, passes its session's messages on, stores its events and ends it. It
//! routes, briefs, reviews and lands nothing for it.
//!
//! - **Workspace.** A worktree on a new branch from its base, or its session's own checkout
//!   (`--local`), with a scratch folder outside the repository that is also its TMPDIR.
//!   Everything is recorded in the cleanup ledger under `task:<id>` before it is created,
//!   including any process running inside those folders.
//! - **Permissions**: the worker runs at the level it was started with, passed straight to
//!   its CLI as its own permission mode (G5). What its CLI asks reaches the user as a card.
//! - **Secrets**: the project's gitignored env files are copied in, and their values are
//!   redacted from everything the worker produces.
//!
//! The worker streams its events to `task:<id>`; its session hears its turns end through
//! `brigadier wait`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use brigadier_git::{Oid, PatchOutcome, WorktreeSpec};
use brigadier_providers::policy;
use brigadier_providers::{
    ApprovalDecision, ApprovalKind, ApprovalRequest, Artifact, Decider, InputFile, InputPart,
    Origin, ProviderEvent, ProviderKind, SessionSpec, Started, ToolSet, TurnInput, TurnStatus,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::conversation::{Cli, safe_file_name};
use super::outputs::outputs_dir;
use super::refcheck::engine_rewrite;
use super::usage::TokenOwner;
use super::{EventSource, SessionManager, blocking, fallback, git_error, secrets, warm};
use crate::model::{
    Conversation, ConversationId, DomainEvent, Environment, ModelChoice, PermissionLevel, Setup,
    streams,
};
use crate::routing::{TokenMeter, WriteCause};
use crate::runtime::{is_delta, merge_delta};
use crate::sessions::McpUpdate;
use crate::tools::Role;
use crate::work::{
    ArtifactKind, ArtifactRef, AttachmentRef, Attempt, AttemptEnd, MockupPart,
    OrchestratorStepKind, PlainWorker, RepoAccess, Route, Task, TaskId, TaskKind, TaskState,
    TaskWorkspace, WorkerAccess, WorkerStepKind,
};
use crate::{Error, Result, now_ms};

/// Text deltas arriving within this window are stored as one event.
const DELTA_WINDOW: Duration = Duration::from_millis(30);
/// Why `brigadier start` refuses a worker while its session is in plan mode (harness D42).
pub(crate) const PLAN_MODE_NO_WORKERS: &str = "Plan mode is on: no worker starts until the user presses Build it on your plan. Finish the plan and show it.";

/// What a task's workspace is made of, once prepared.
#[derive(Debug, Clone)]
pub(crate) struct Workspace {
    /// None for a Chat's worker in a folder of its own (S28).
    pub repo: Option<PathBuf>,
    /// The task's worktree, if it has one.
    pub worktree: Option<PathBuf>,
    pub branch: Option<String>,
    /// The commit the worker started from.
    pub base: Option<Oid>,
    /// `base` is a snapshot of the user's uncommitted changes.
    pub on_snapshot: bool,
    /// The branch it merges back into.
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
    /// The task's worktree, if it has one.
    pub worktree: Option<PathBuf>,
    /// The grant its `brigadier` calls run under.
    pub grant: String,
}

mod seam;

#[derive(Default)]
struct TaskLiveState {
    cli: Option<Arc<Cli>>,
    /// A turn is running.
    busy: bool,
    /// The current turn's last assistant message, for its session's `brigadier wait`.
    last_message: Option<String>,
    /// Its outputs folder.
    outputs: Option<PathBuf>,
    stopping: bool,
    /// Where the worker's CLI runs (a command without its own cwd runs here).
    cwd: Option<PathBuf>,
    /// The task's worktree, whose file changes the user may allow for the session.
    worktree: Option<PathBuf>,
    redactor: Option<Arc<brigadier_providers::redact::Redactor>>,
    /// Why the turn ended early, once it is over (a usage limit, or an error).
    cutoff: Option<AttemptEnd>,
    /// Its hand-off to a fresh CLI session, under way (D14).
    seam: Option<seam::SeamLive>,
    /// What its session sent while it hands off: its successor's first input carries it.
    seam_held: Vec<String>,
}

/// A task's live worker.
pub(crate) struct TaskLive {
    pub id: TaskId,
    pub conversation_id: ConversationId,
    state: tokio::sync::Mutex<TaskLiveState>,
    /// Held while the worker is stopped, so two stops never both take effect.
    pub(crate) settle: tokio::sync::Mutex<()>,
    /// Held while the worker's CLI starts again, so two messages never start two CLIs.
    pub(crate) reroute: tokio::sync::Mutex<()>,
}

impl TaskLive {
    fn new(id: TaskId, conversation_id: ConversationId) -> Self {
        Self {
            id,
            conversation_id,
            state: tokio::sync::Mutex::new(TaskLiveState::default()),
            settle: tokio::sync::Mutex::new(()),
            reroute: tokio::sync::Mutex::new(()),
        }
    }

    /// Records why the turn ends early, for its end.
    pub(crate) async fn set_cutoff(&self, end: AttemptEnd) {
        self.state.lock().await.cutoff = Some(end);
    }

    /// Takes the reason the turn ended early, if any.
    pub(crate) async fn take_cutoff(&self) -> Option<AttemptEnd> {
        self.state.lock().await.cutoff.take()
    }

    /// Where its CLI runs.
    pub(crate) async fn cwd(&self) -> Option<PathBuf> {
        self.state.lock().await.cwd.clone()
    }

    /// What its CLI last said.
    pub(crate) async fn last_message(&self) -> Option<String> {
        self.state.lock().await.last_message.clone()
    }

    /// An event came from `cli`: a turn its CLI started on its own (a background job ended)
    /// runs like one we started.
    async fn note_event(&self, cli: &Arc<Cli>, event: &ProviderEvent) {
        let mut state = self.state.lock().await;
        if !state.cli.as_ref().is_some_and(|c| Arc::ptr_eq(c, cli)) {
            return;
        }
        if matches!(event, ProviderEvent::TurnStarted { .. }) {
            state.busy = true;
        }
    }

    /// The worker's CLI session, while one runs.
    pub(crate) async fn cli(&self) -> Option<Arc<Cli>> {
        self.state.lock().await.cli.clone()
    }

    /// Whether the worker can move its branch now: a CLI session of it runs (mid-turn or
    /// between turns).
    pub(crate) async fn executing(&self) -> bool {
        self.state.lock().await.cli.is_some()
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

    /// The session's permission level, which its thread and its workers run at. In a folder
    /// the user doesn't trust it is Ask.
    pub(crate) fn permission(&self, id: &ConversationId) -> PermissionLevel {
        if self.repo_trust(id) == Some(false) {
            return PermissionLevel::Ask;
        }
        match self.core.conversation(id).map(|c| c.setup) {
            Ok(Some(Setup::Session { permission, .. } | Setup::Chat { permission, .. })) => {
                permission
            }
            _ => PermissionLevel::default(),
        }
    }

    /// Whether the session is in plan mode (see [`Setup::Session`]).
    pub(crate) fn plan_mode(&self, id: &ConversationId) -> bool {
        matches!(
            self.core.conversation(id).map(|c| c.setup),
            Ok(Some(Setup::Session {
                plan_mode: true,
                ..
            }))
        )
    }

    pub(crate) fn existing_task_live(&self, id: &TaskId) -> Option<Arc<TaskLive>> {
        self.tasks_lock().get(id).cloned()
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

    /// D44/D47: a plain worker stays available after a failed or interrupted turn, but its
    /// thread step records the turn's outcome rather than inferring success from Reported.
    pub(super) async fn update_task_with_step(
        &self,
        conversation_id: &ConversationId,
        id: &TaskId,
        step: Option<WorkerStepKind>,
        change: impl FnOnce(&mut Task),
    ) -> Result<Task> {
        self.update_task_if_with_step(conversation_id, id, |_| true, step, change)
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
        self.update_task_if_with_step(conversation_id, id, still, None, change)
            .await
    }

    async fn update_task_if_with_step(
        &self,
        conversation_id: &ConversationId,
        id: &TaskId,
        still: impl FnOnce(&Task) -> bool,
        step: Option<WorkerStepKind>,
        change: impl FnOnce(&mut Task),
    ) -> Result<Option<Task>> {
        let task = {
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
            events.extend(worker_step(&task, Some(was), reported, step));
            self.core
                .record_conversation(conversation_id, events)
                .await?;
            task
        };
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

    /// Makes a plain worker of `conversation_id` on `choice` (the CLI, model and effort
    /// `brigadier start` named; the CLI's own default model when it named none), and starts
    /// it; `mockup`: its part in the session's mockup (`--mockup`). Brigadier routes nothing
    /// for it (G18).
    pub(crate) async fn create_plain_task(
        &self,
        conversation_id: &ConversationId,
        title: String,
        spec: String,
        choice: ModelChoice,
        plain: PlainWorker,
        mockup: Option<MockupPart>,
    ) -> Result<Task> {
        self.admit()?;
        let fence = self.enter(conversation_id)?;
        let conversation = self.core.conversation(conversation_id)?;
        // A session's or a Chat's (S28); a side chat is its session's helper.
        let owns_workers = match conversation.setup {
            Some(Setup::Session { .. }) => true,
            Some(Setup::Chat { .. }) => conversation.side_of.is_none(),
            None => false,
        };
        if !owns_workers {
            return Err(Error::Invalid(
                "workers belong to a session or a Chat".into(),
            ));
        }
        // The CLIs' plan mode lets a shell command run (Claude 2.1.310 ran one under an allow
        // rule and without; Codex's is a collaboration mode), so no worker starts before the
        // user's Build it (harness D42).
        if self.plan_mode(conversation_id) {
            return Err(Error::Invalid(PLAN_MODE_NO_WORKERS.into()));
        }
        let now = now_ms();
        let route = Route {
            choice,
            reason: "Started by its session with `brigadier start`.".into(),
            explanation: None,
        };
        let number = self.core.next_task_number(conversation_id).await?;
        let names = self
            .core
            .board(conversation_id)
            .await?
            .tasks
            .values()
            .map(|task| task.title.clone())
            .collect::<Vec<_>>();
        let kind = TaskKind::Implement;
        let title = job_name(&title, kind, None, &names);
        let task = Task {
            id: TaskId::generate(),
            conversation_id: conversation_id.clone(),
            number,
            position: 0,
            title: title.trim().to_owned(),
            kind,
            spec,
            access: repo_access_for(kind, plain.permission),
            attempts: vec![Attempt {
                route: route.clone(),
                started_at_ms: now,
                ended_at_ms: None,
                end: None,
            }],
            route,
            floor: Default::default(),
            areas: Vec::new(),
            pin: None,
            needs: Vec::new(),
            state: TaskState::Queued,
            blocked_reason: None,
            quota_wait: None,
            subject: None,
            target: None,
            end_state: None,
            mockup,
            design: false,
            plan: None,
            attachments: Vec::new(),
            workspace: None,
            report: None,
            candidate: None,
            gate_link: None,
            role: None,
            landing: None,
            landing_from: None,
            landed: None,
            error: None,
            kept: None,
            outputs: Vec::new(),
            request_id: None,
            messages: Vec::new(),
            rework_rounds: 0,
            native_session: None,
            report_version: None,
            deaths: Vec::new(),
            live_look: false,
            fresh_commit: None,
            integrations: Vec::new(),
            brain: Default::default(),
            resuming: false,
            takeover: None,
            trial_slot: false,
            rerun: None,
            plain: Some(plain),
            created_at_ms: now,
            updated_at_ms: now,
        };
        let mut events = vec![DomainEvent::TaskUpdated {
            task: Box::new(task.clone()),
        }];
        events.extend(worker_step(&task, None, false, None));
        // A cleanup that stopped waiting for this has already passed the session's tasks (it
        // may have been restored since): the task isn't made.
        if fence.cut_off() {
            return Err(super::closing::closing_error());
        }
        self.core
            .record_conversation(conversation_id, events)
            .await?;
        let live = self.task_live(&task);
        let manager = self.arc();
        let started = task.clone();
        self.spawn(async move {
            if let Err(err) = manager.start_worker(&live, started.clone()).await {
                manager.worker_failed(&started, &err.to_string()).await;
            }
        });
        Ok(task)
    }

    pub(crate) async fn start_worker(&self, live: &Arc<TaskLive>, task: Task) -> Result<()> {
        let conversation_id = task.conversation_id.clone();
        // Held while its worktree is made and recorded; the launch below enters again.
        let fence = self.enter(&conversation_id)?;
        self.set_task_state(&conversation_id, &task.id, TaskState::Starting)
            .await?;
        let owner = format!("task:{}", task.id);
        let workspace = self.prepare_workspace(&owner, &task).await?;
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
        drop(fence);
        self.record_plain_workspace(&task).await;
        self.launch_first(live, &task, &workspace.scratch).await
    }

    /// Starts the worker's CLI on its first message, its prompt as `brigadier start` gave it
    /// (a mockup's says where its images go), in its prepared workspace.
    async fn launch_first(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        scratch: &std::path::Path,
    ) -> Result<()> {
        let files = self.worker_files(task, scratch).await;
        let prompt = match &task.mockup {
            Some(MockupPart::Builds) => {
                format!("{}\n\n{}", task.spec, mockup_line(&outputs_dir(scratch)))
            }
            _ => task.spec.clone(),
        };
        self.launch_worker(
            live,
            task,
            Origin::New,
            TurnInput::with_files(prompt, files),
        )
        .await
    }

    /// A worker a restart cut off before its CLI session began starts again on its prompt:
    /// in the worktree it has, or from the start when it had none yet (D9 a).
    pub(crate) async fn relaunch_worker(&self, live: &Arc<TaskLive>, task: &Task) -> Result<()> {
        match &task.workspace {
            Some(workspace) => {
                let scratch = PathBuf::from(&workspace.scratch);
                self.launch_first(live, task, &scratch).await
            }
            None => self.start_worker(live, task.clone()).await,
        }
    }

    /// Starts (or resumes) the worker's CLI session in the task's prepared workspace and sends
    /// it `first`.
    pub(crate) async fn launch_worker(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        origin: Origin,
        first: TurnInput,
    ) -> Result<()> {
        // Held until its CLI session is registered, where the session's cleanup finds it.
        let fence = self.enter(&task.conversation_id)?;
        let launched = self
            .launch_admitted(live, task, origin, first, &fence, None)
            .await;
        // A cleanup that stopped waiting for it has already passed this task.
        if launched.is_ok() && fence.cut_off() {
            live.close_cli().await;
            return Err(super::closing::closing_error());
        }
        launched
    }

    /// The worker's CLI session for `task`, with a fresh worker grant issued to `grant_owner`
    /// for its `brigadier` calls.
    pub(crate) async fn worker_session(
        &self,
        task: &Task,
        origin: Origin,
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
        // A worker in its session's checkout (`--local`) works in a folder that is not its
        // own: nothing is set up, copied or swept there for it.
        let shared_checkout = task.plain.as_ref().is_some_and(|plain| plain.local);
        // Its new worktree gets the repository's setup script run before the worker starts
        // (never in a folder the user doesn't trust).
        let setup = match (&workspace.worktree, &origin) {
            (Some(worktree), Origin::New) if !shared_checkout => {
                self.repo_trust(&conversation_id) == Some(true) && {
                    let (git, worktree) = (self.git.clone(), worktree.clone());
                    blocking(move || Ok(has_setup(&git, &worktree)))
                        .await
                        .unwrap_or(false)
                }
            }
            _ => false,
        };
        let mut secret_values = match (&workspace.repo, &workspace.worktree, &origin) {
            (None, ..) => Vec::new(),
            (Some(repo), Some(worktree), Origin::New) if !shared_checkout => {
                secrets::copy_secrets(self, &owner, repo, worktree, &secret_files).await?
            }
            (Some(repo), ..) => secrets::values(&self.git, repo, &secret_files).await,
        };
        let provider = task.route.choice.provider;
        // It starts in its worktree, as a CLI started there would.
        let cwd = workspace
            .worktree
            .clone()
            .unwrap_or_else(|| workspace.scratch.clone());
        let outputs = outputs_dir(&workspace.scratch);
        {
            let outputs = outputs.clone();
            blocking(move || {
                std::fs::create_dir_all(&outputs).map_err(|err| Error::Invalid(err.to_string()))
            })
            .await?;
        }
        // It runs at the level it was started with (Q8), Ask in a folder the user doesn't
        // trust.
        let access = match &task.plain {
            _ if self.repo_trust(&task.conversation_id) == Some(false) => {
                PermissionLevel::Ask.access()
            }
            Some(plain) => plain.permission.access(),
            None => self.permission(&task.conversation_id).access(),
        };
        if let (Some(worktree), true) = (&workspace.worktree, setup) {
            self.run_setup_script(
                &conversation_id,
                &owner,
                worktree,
                &worker_env(&workspace.scratch),
                // A stopped worker's processes end with its owner's.
                CancellationToken::new(),
            )
            .await;
        }
        let worker_grant = self.grants.issue(
            grant_owner,
            Role::Worker {
                conversation_id: conversation_id.clone(),
                task_id: task.id.clone(),
            },
        );
        // B7: the grant is a secret too.
        secret_values.push(worker_grant.clone());
        // A worker `brigadier start` started is naked (D13, D17): none of the user's setup,
        // no skills, and only the servers it was granted (D18). The project's instruction file
        // still loads.
        let (switched_off, granted) = match task.plain.as_ref() {
            Some(plain) => {
                self.naked_worker_setup(&conversation_id, provider, &cwd, &plain.mcps)
                    .await
            }
            None => Default::default(),
        };
        let mut env = worker_env(&workspace.scratch);
        env.extend(self.cli_env(&task.id.0, &worker_grant));
        // Its own folder and ledger, which the standing instructions name (S17). The ledger's
        // `record/` is outside its workspace, so it is one of the folders it may write.
        let worker_dir = self.session_dir(&conversation_id).worker(task);
        env.extend(super::session_dir::standing_env(&worker_dir));
        let record = worker_dir.join("record");
        let made = record.clone();
        let _ = blocking(move || {
            std::fs::create_dir_all(&made).map_err(|err| Error::Invalid(err.to_string()))
        })
        .await;
        // The computer tools, while the setting is on (S35); a naked worker only when the
        // thread asked (`--with computer-use`), and then loaded from the start.
        let computer = match task.plain.as_ref() {
            Some(plain) if !plain.computer_use => None,
            plain => self.computer_use_kit(&task.id.0, plain.is_some()).await,
        };
        if let Some(kit) = &computer {
            secret_values.push(kit.token.clone());
        }
        let redactor = secrets::redactor(secret_values);
        // Its sub-agents run on what its CLI allows (G18).
        let allowed_models = self.rules_models(provider)?;
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
            // Resumed when its session sends it more.
            ephemeral: false,
            access,
            // The standing instructions, as every session gets them (S17), but for the line on
            // running workers, which a worker can't.
            append_system_prompt: Some(super::prompts::standing_text(
                &super::prompts::worker_instructions(&self.core.settings(), provider),
            )),
            mcp_servers: computer
                .iter()
                .map(|kit| kit.server.clone())
                .chain(granted)
                .collect(),
            tools: ToolSet::Lean,
            skill_plugins: computer.into_iter().map(|kit| kit.plugin).collect(),
            installed_skills: Vec::new(),
            switched_off,
            add_dirs: vec![record],
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
            sweep_dir: Some(if shared_checkout {
                workspace.scratch.clone()
            } else {
                cwd.clone()
            }),
            keeps_detached: false,
            // It compacts as its CLI does.
            auto_compact: true,
            allowed_models,
            omit_ai_coauthors: self.core.settings().omit_ai_coauthors,
            output_hook: None,
            stop_hook: None,
            naked_worker: task.plain.is_some(),
            native_compaction: None,
            // Its temp folder names its session, so Delete and the sweep find it (D7).
            temp_tag: Some(task.conversation_id.short().to_owned()),
        };
        let worktree = workspace.worktree.clone();
        Ok(WorkerSession {
            spec,
            cwd,
            outputs,
            redactor,
            worktree,
            grant: worker_grant,
        })
    }

    /// [`Self::launch_worker`] once admitted.
    async fn launch_admitted(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        origin: Origin,
        first: TurnInput,
        fence: &super::closing::WorkGuard,
        // A hand-off's successor (D14): its worker's `record/`, whose `seam.json` is pointed
        // at the id its CLI took before its first input goes.
        successor_of: Option<&Path>,
    ) -> Result<()> {
        let conversation_id = task.conversation_id.clone();
        let owner = format!("task:{}", task.id);
        // A resumed Codex thread's token totals include the turns counted before.
        let resumed = matches!(origin, Origin::Resume { .. });
        let continues = task.route.choice.provider == ProviderKind::Codex && resumed;
        let WorkerSession {
            spec,
            cwd,
            outputs,
            redactor,
            worktree,
            grant,
        } = self.worker_session(task, origin, &owner).await?;
        // Stopped while its new worktree's setup script ran (#457): its CLI never starts.
        if self.stopped_meanwhile(task).await {
            self.grants.revoke(&[grant]);
            return Err(stopped_as_it_started(task));
        }
        let provider = task.route.choice.provider;
        let account = self.runtime.account_for(&task.route.choice);
        let tools_key = crate::routing::meter::tools_key(&spec);
        let Started { session, events } =
            match self.runtime.start_hosted(&owner, &account, spec).await {
                Ok(started) => started,
                Err(err) => {
                    self.grants.revoke(&[grant]);
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
        let cause = if resumed {
            WriteCause::Resume
        } else {
            WriteCause::FreshStart
        };
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
            grants: vec![grant],
            ended: CancellationToken::new(),
            launch: None,
        });
        {
            let mut state = live.state.lock().await;
            state.cli = Some(cli.clone());
            state.cwd = Some(cwd);
            state.worktree = worktree;
            state.outputs = Some(outputs);
            state.redactor = redactor;
            state.busy = true;
            state.stopping = false;
            state.cutoff = None;
        }
        #[cfg(debug_assertions)]
        self.arm_env_fault(&task.id, provider).await;
        let manager = self.arc();
        let pumped = (live.clone(), cli.clone());
        self.spawn(async move { manager.pump_worker(pumped.0, pumped.1, events).await });
        // A Stop while its CLI started ends it before its first message, and it stays stopped
        // (#457).
        let was_final = task.state.is_final();
        let mut stopped = false;
        self.update_task(&conversation_id, &task.id, |t| {
            if self.is_stopping(&t.id) || (t.state.is_final() && !was_final) {
                stopped = true;
                return;
            }
            t.state = TaskState::Running;
            t.blocked_reason = None;
        })
        .await?;
        if stopped {
            live.close_cli().await;
            self.grants.revoke(&cli.grants);
            return Err(stopped_as_it_started(task));
        }
        if let Some(record) = successor_of {
            let (record, native_id) = (record.to_owned(), cli.session.native_id());
            let _ = blocking(move || {
                seam::name_successor(&record, &native_id, provider);
                Ok(())
            })
            .await;
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

    /// Starts a worker whose CLI session stopped (the conversation hibernated, or Brigadier
    /// restarted) again, resuming its CLI session with `input`.
    async fn revive_worker(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        input: TurnInput,
    ) -> Result<()> {
        // One start at a time: a worker resumed meanwhile takes the message in the turn it
        // runs.
        let _handing = live.reroute.lock().await;
        if let Some(cli) = live.cli().await {
            return cli
                .session
                .steer(input)
                .await
                .map_err(|err| Error::Provider(err.to_string()));
        }
        // Its generation closed at a hand-off (D14): the message goes to its successor.
        if self.worker_seam_closed(task).await {
            return self
                .worker_successor(live, task, vec![held_text(&input)])
                .await;
        }
        let Some(native_id) = self.last_worker_native_id(task).await else {
            return Err(Error::Invalid(format!(
                "worker {} cannot be resumed; start a new one",
                task.number
            )));
        };
        self.launch_worker(live, task, Origin::Resume { native_id }, input)
            .await
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

    /// The repository `task` of `conversation` works on: its session's, or the one a Chat's
    /// worker was started in (`--repo`); none for a Chat's worker in a folder of its own (S28).
    fn tasks_repo(&self, conversation: &Conversation, task: &Task) -> Result<Option<PathBuf>> {
        if let Some(repo) = task.plain.as_ref().and_then(|plain| plain.repo.as_ref()) {
            return Ok(Some(PathBuf::from(repo)));
        }
        match &conversation.setup {
            Some(Setup::Session { repo, .. }) => Ok(Some(PathBuf::from(repo))),
            Some(Setup::Chat { .. }) => Ok(None),
            None => Err(Error::Invalid(
                "workers belong to a session or a Chat".into(),
            )),
        }
    }

    /// Creates the task's scratch folder and worktree, recorded in the ledger first.
    async fn prepare_workspace(&self, owner: &str, task: &Task) -> Result<Workspace> {
        let conversation = self.core.conversation(&task.conversation_id)?;
        let repo = self.tasks_repo(&conversation, task)?;
        let scratch = self
            .session_dir(&task.conversation_id)
            .worker(task)
            .join("tmp");
        self.prepare_owned_dir(owner, &scratch, true).await?;
        let plain = task
            .plain
            .as_ref()
            .ok_or_else(|| Error::Invalid("only `brigadier start` starts workers".into()))?;
        self.plain_workspace(owner, task, plain, &conversation, repo, scratch)
            .await
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
                self.note_branch_namespace(conversation_id, repo, branch)
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
                let (made, copied, setup) = blocking(move || {
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
                    Ok((made, copied, setup))
                })
                .await?;
                self.warm_session_worktree(conversation_id, repo, &worktree);
                if !copied.is_empty()
                    && let Ok(conv) = self.conv(conversation_id)
                {
                    conv.note(format!(
                        "[setup] {}",
                        super::prompts::copied_files_line(&copied)
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

    /// Copies the checkout's dependency installs and build caches into the session's new
    /// `worktree` (best effort; see `warm`), so the thread builds and tests there as a worker
    /// does in its own, instead of failing its first check and reinstalling them (BUILD-PLAN
    /// C7). It goes on behind the thread's first turn, which never waits for it (it took 30 to
    /// 40 s in a big checkout); the thread hears what came once it is in. A copy is moved in
    /// only where nothing is yet, so an install the thread started meanwhile wins. The
    /// session's cleanup waits for it.
    fn warm_session_worktree(
        &self,
        conversation_id: &ConversationId,
        repo: &Path,
        worktree: &Path,
    ) {
        let Ok(guard) = self.enter(conversation_id) else {
            return;
        };
        let (manager, id) = (self.arc(), conversation_id.clone());
        let (git, repo, path) = (self.git.clone(), repo.to_owned(), worktree.to_owned());
        let platform = self.runtime.platform().clone();
        self.spawn(async move {
            let warmed = blocking(move || {
                let alias = repo.clone();
                let repo = git.open(&repo).map_err(git_error)?;
                let worktree = git.open(&path).map_err(git_error)?;
                let started = std::time::Instant::now();
                let installing = || warm::install_running(&*platform, repo.root());
                let warmed = warm::warm_worktree(&repo, &[alias], &worktree, &installing);
                tracing::info!(
                    worktree = %path.display(),
                    copied = ?warmed.copied,
                    skipped = ?warmed.skipped,
                    ms = started.elapsed().as_millis() as u64,
                    "warmed the session's worktree"
                );
                Ok(warmed.copied)
            })
            .await
            .unwrap_or_else(|err| {
                tracing::warn!(conversation = %id, %err, "couldn't warm the session's worktree");
                Vec::new()
            });
            if !warmed.is_empty()
                && !guard.cut_off()
                && let Ok(conv) = manager.conv(&id)
            {
                conv.note(format!(
                    "[workspace] {}",
                    super::prompts::copied_in_line(&warmed)
                ))
                .await;
            }
            drop(guard);
        });
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

    /// The user's attachments the task was given, written into its scratch folder.
    pub(crate) async fn worker_files(&self, task: &Task, scratch: &Path) -> Vec<InputFile> {
        self.attachment_files(&task.attachments, scratch).await
    }

    /// `attachments` written in the `attachments` folder of a worker's `scratch`.
    pub(crate) async fn attachment_files(
        &self,
        attachments: &[AttachmentRef],
        scratch: &Path,
    ) -> Vec<InputFile> {
        let mut files = Vec::new();
        for attachment in attachments {
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
                        self.watch_worker(&live.conversation_id, &live.id, &event);
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
                        self.watch_worker(&live.conversation_id, &live.id, &event);
                        merge_delta(&mut deltas, event);
                        deadline.get_or_insert_with(|| tokio::time::Instant::now() + DELTA_WINDOW);
                    }
                    Some(mut event) => {
                        live.note_event(&cli, &event).await;
                        if let Some(line) = self.note_worker_cli(&live.conversation_id, &live.id, &event) {
                            self.note_worker_turn(&live.conversation_id, &live.id, &line).await;
                        }
                        self.watch_worker(&live.conversation_id, &live.id, &event);
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
                                exit = Some(exit_words(*code, *signal, stderr_tail.as_deref()));
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
        self.grants.revoke(&cli.grants);
        let (stopping, replaced) = {
            let mut state = live.state.lock().await;
            if state.cli.as_ref().is_some_and(|c| Arc::ptr_eq(c, &cli)) {
                state.cli = None;
            }
            state.busy = false;
            (state.stopping, state.cli.is_some())
        };
        cli.ended.cancel();
        // A fresh session that already took the task over tells its own MCP servers.
        if !replaced {
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
        {
            let manager = self.arc();
            let exit = exit.unwrap_or_else(|| "its CLI ended".to_owned());
            self.spawn(async move { manager.plain_gone(&task, &exit).await });
        }
    }

    async fn on_worker_event(&self, live: &Arc<TaskLive>, cli: &Arc<Cli>, event: ProviderEvent) {
        let cwd = live.cwd().await;
        self.follow_worker_commands(&live.conversation_id, &live.id, cwd, &event)
            .await;
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
            // Its CLI keeps its own context, as it would in a terminal; its session hears when
            // it compacts. Its third compaction in one CLI session, or a failed one, hands it
            // to a fresh one (D14).
            ProviderEvent::CompactionStarted { .. } => {
                self.plain_fresh_context(live);
            }
            ProviderEvent::CompactionEnded { error, .. } => {
                self.worker_compacted(live, cli, error.as_deref()).await;
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
                        forgotten: false,
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
        let completed = match &event {
            ProviderEvent::TurnCompleted { status, .. } => Some(*status),
            _ => None,
        };
        let error = match &event {
            // Claude's thrash stop, or a context too long for the model: the hand-off takes
            // it (D14), not its session.
            ProviderEvent::Error { error } if self.worker_seam_error(live, cli, error).await => {
                None
            }
            ProviderEvent::Error { error } if !error.will_retry => match fallback::verdict(error) {
                // A usage limit (or a logout, an outage): its session hears it when the turn
                // ends.
                fallback::ErrorVerdict::HandOff(end) => {
                    live.set_cutoff(end).await;
                    None
                }
                fallback::ErrorVerdict::Transient | fallback::ErrorVerdict::Keep => {
                    Some(error.message.clone())
                }
            },
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
            // What its Codex auto-reviews used, which its own totals leave out.
            self.meter_child_threads(cli, TokenOwner::Task(&live.conversation_id, &live.id))
                .await;
            self.worker_turn_completed(live, status).await;
        }
    }

    /// The worker's turn ended: it waits for its session's next message, and its session's
    /// `brigadier wait` hears it.
    async fn worker_turn_completed(&self, live: &Arc<TaskLive>, status: TurnStatus) {
        let seam = {
            let mut state = live.state.lock().await;
            state.busy = false;
            if state.stopping {
                return;
            }
            state.seam.clone()
        };
        let Ok(task) = self.task_by_id(&live.conversation_id, &live.id).await else {
            return;
        };
        if task.state.is_final() {
            return;
        }
        // A hand-off under way (D14): its successor goes on, and its session hears nothing.
        match seam {
            Some(seam) => {
                let (manager, live) = (self.arc(), live.clone());
                self.spawn(async move {
                    manager
                        .worker_seam_turn_end(&live, &task, seam, status)
                        .await;
                });
            }
            None => self.plain_turn_ended(live, &task, status).await,
        }
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
            Ok(Some(card)) => self.plain_needs_input(live, &card, &what),
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

    /// `diff` (the worker's changes so far, see [`Self::worktree_diff`]) as a diff artifact.
    async fn diff_artifact(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        diff: Option<String>,
    ) -> Option<ArtifactRef> {
        let diff = diff.filter(|diff| !diff.is_empty())?;
        let diff = match live.redactor().await {
            Some(redactor) => redactor.redact(&diff).into_owned(),
            None => diff,
        };
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

    /// `brigadier send`: `text` goes to the worker as written, into its running turn, or as a
    /// new turn of the session its CLI keeps.
    pub(crate) async fn message_worker(
        &self,
        conversation_id: &ConversationId,
        task: &Task,
        text: String,
    ) -> Result<()> {
        // Something new from its session: a later cut-off may go on by itself again (D9 a).
        if task.plain.as_ref().is_some_and(|plain| plain.auto_resumed) {
            self.update_task(conversation_id, &task.id, |t| {
                if let Some(plain) = &mut t.plain {
                    plain.auto_resumed = false;
                }
            })
            .await?;
        }
        self.send_worker_text(conversation_id, task, text).await
    }

    /// [`Self::message_worker`] without its being something new from the session: a turn
    /// going on by itself after a restart (D9 a).
    pub(crate) async fn send_worker_text(
        &self,
        conversation_id: &ConversationId,
        task: &Task,
        text: String,
    ) -> Result<()> {
        drop(self.enter(conversation_id)?);
        self.deliver_to_worker(conversation_id, task, TurnInput::text(text))
            .await
    }

    /// `input` to a worker's CLI: into its running turn, or as a new turn of the session its
    /// CLI keeps (started again when it had stopped). An error means its CLI did not take it;
    /// once it did, failing to record the worker running is only logged, so nobody sends it
    /// twice.
    pub(crate) async fn deliver_to_worker(
        &self,
        conversation_id: &ConversationId,
        task: &Task,
        input: TurnInput,
    ) -> Result<()> {
        if task.state.is_final() {
            return Err(Error::Invalid(format!("worker {} has ended", task.number)));
        }
        let live = self.task_live(task);
        let mut state = live.state.lock().await;
        // Handing off (D14): its successor's first input carries it.
        if state.seam.as_ref().is_some_and(|seam| seam.holds()) {
            state.seam_held.push(held_text(&input));
            return Ok(());
        }
        let Some(cli) = state.cli.clone() else {
            drop(state);
            return self.revive_worker(&live, task, input).await;
        };
        if state.busy {
            cli.session.steer(input).await
        } else {
            // Busy only once a turn really started: a refused send leaves it idle.
            let sent = cli.session.send(input).await;
            state.busy = sent.is_ok();
            sent
        }
        .map_err(|err| Error::Provider(err.to_string()))?;
        drop(state);
        let running = self
            .update_task(conversation_id, &task.id, |task| {
                if !task.state.is_final() {
                    task.state = TaskState::Running;
                    task.blocked_reason = None;
                }
            })
            .await;
        if let Err(err) = running {
            tracing::warn!(task = %task.id, error = %err, "could not record a messaged worker running");
        }
        Ok(())
    }

    /// Stops a worker for good (`brigadier close`, or the user's stop button). Unfinished
    /// changes are kept on the task branch as a WIP commit.
    pub async fn stop_task(&self, task_id: TaskId) -> Result<()> {
        self.stop_if(task_id, unfinished).await.map(drop)
    }

    /// [`Self::stop_task`] if its state still passes `eligible`, saying whether it stopped it.
    /// Decided under its settle lock, so two stops never both take effect.
    async fn stop_if(&self, task_id: TaskId, eligible: fn(TaskState) -> bool) -> Result<bool> {
        let conversation_id = self.conversation_of_task(&task_id).await?;
        let live = self.existing_task_live(&task_id);
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

    /// Stops `task` for good and files a "Stopped" row saying `reason` (its session's
    /// `brigadier close`, the user's Stop all). False when it had already ended: nothing is
    /// filed.
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
    /// for good, files "Stopped by the user", and tells the thread, into its running turn or
    /// before its next one.
    pub async fn stop_task_by_user(&self, task_id: TaskId) -> Result<()> {
        let conversation_id = self.conversation_of_task(&task_id).await?;
        let task = self.task_by_id(&conversation_id, &task_id).await?;
        self.mark_user_stop(&task.id);
        let stopped = self
            .stop_worker(&conversation_id, &task, USER_STOP_REASON.to_owned())
            .await;
        if matches!(stopped, Ok(true))
            && let Ok(conv) = self.conv(&conversation_id)
        {
            let note = stopped_note(std::slice::from_ref(&task), false);
            self.tell_thread_now(&conv, note).await;
        }
        stopped.map(drop)
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

    /// Resumes a paused worker: its turn continues, in its CLI or in its resumed session.
    pub async fn resume_task(&self, task_id: TaskId) -> Result<()> {
        let conversation_id = self.conversation_of_task(&task_id).await?;
        let task = self.task_by_id(&conversation_id, &task_id).await?;
        if task.state != TaskState::Paused {
            return Err(Error::Invalid("the worker isn't paused".into()));
        }
        self.message_worker(&conversation_id, &task, "Continue the task.".into())
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

    /// A worker failed to start or run: the task ends and its session hears why.
    pub(crate) async fn worker_failed(&self, task: &Task, reason: &str) {
        // Brigadier quitting closed its CLI (a start cut short fails to send its first
        // message): nothing more happens to it.
        if !self.admitting.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        // Stopped while it started: it stays stopped.
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
        self.plain_failed(task, reason.to_owned()).await;
    }

    /// Ends a task: unfinished changes are kept (B15), then everything recorded under
    /// `task:<id>` is removed. Branches with unlanded work stay; a task branch with nothing to
    /// keep goes with the worktree.
    pub(crate) async fn dispose_task(&self, task: &Task, state: TaskState) {
        // A yes the user gave it in the browser goes with it.
        self.browser.forget_task(&task.conversation_id, &task.id);
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
        if let Err(err) = result {
            tracing::warn!(task = %task.id, error = %err, "could not record the task's end");
        }
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
        // Its worktree went already (an archive removed it, merged and clean): nothing is
        // left to keep.
        if !path.join(".git").exists() {
            return (None, false);
        }
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

/// The cleanup owner of a conversation's worker folder ([`SessionManager::worker_home`]).
pub(crate) fn worker_home_owner(conversation_id: &ConversationId) -> String {
    format!("workers:{conversation_id}")
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

/// The reason a "Stopped" row gives for the user's Stop all.
pub(crate) const USER_STOP_REASON: &str = "Stopped by the user";

/// A message a worker's hand-off holds for its successor, which takes text (D14): an image
/// goes by its path, like an attached file.
fn held_text(input: &TurnInput) -> String {
    input
        .parts_with_file_notes()
        .into_iter()
        .map(|part| match part {
            InputPart::Text(text) => text,
            InputPart::Image(file) => format!(
                "[Attached image \"{}\" ({}): {}]",
                file.name,
                file.mime,
                file.path.display()
            ),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

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
/// What a mockup worker's first message adds to its prompt: where its images go, by name.
fn mockup_line(outputs: &Path) -> String {
    format!(
        "[mockup] You build the session's mockup. Save each screen as an image named \
         mockup-1.png, mockup-2.png… in {}; they show in the session's Mockup tab when your \
         turn ends, and the user's notes on them come back to you.",
        outputs.display()
    )
}

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
/// `reported` tells whether it had reported before. `kind` overrides the inferred step when
/// the task stays available after a failed or interrupted turn (D44/D47).
fn worker_step(
    task: &Task,
    was: Option<TaskState>,
    reported: bool,
    kind: Option<WorkerStepKind>,
) -> Option<DomainEvent> {
    let kind = kind.or_else(|| WorkerStepKind::between(was, task.state, reported))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exit_is_named_by_its_signal_or_code_and_last_stderr_line() {
        assert_eq!(exit_words(None, Some(9), None), "signal 9");
        assert_eq!(
            exit_words(Some(1), None, Some("first\nBroken pipe\n\n")),
            "exit code 1: Broken pipe"
        );
        assert_eq!(exit_words(None, None, Some("  ")), "ended");
    }

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

/// How a CLI ended, in a few words: its signal or exit code, and its last line of stderr.
fn exit_words(code: Option<i32>, signal: Option<i32>, stderr: Option<&str>) -> String {
    let how = match (signal, code) {
        (Some(signal), _) => format!("signal {signal}"),
        (None, Some(code)) => format!("exit code {code}"),
        (None, None) => "ended".to_owned(),
    };
    match stderr
        .and_then(|tail| tail.lines().rev().find(|line| !line.trim().is_empty()))
        .map(str::trim)
    {
        Some(line) => format!("{how}: {line}"),
        None => how,
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
