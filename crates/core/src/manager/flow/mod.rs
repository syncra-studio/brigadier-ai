//! Scripted runs of a whole session: the real manager, store, git and worktrees, with each CLI
//! replaced by a script that answers its turns and calls Brigadier's tools as a model would.

// Helpers the flow tests grow into.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use brigadier_providers::model::{
    Artifact, ModelCatalog, ModelInfo, ProviderStatus, QuotaSnapshot, QuotaSource, Role as Speaker,
    SessionSpec, TurnInput, TurnStatus,
};
use brigadier_providers::{
    ApprovalDecision, BoxFuture, Ledger, Provider, ProviderEvent, ProviderKind, ProviderSession,
    Replayer, Started,
};
use serde_json::Value;
use tokio::sync::mpsc;

use super::{ManagerConfig, SessionManager};
use crate::Core;
use crate::board::Board;
use crate::model::{
    ConversationId, ConversationKind, DomainEvent, EnvironmentRequest, ModelChoice,
    PermissionLevel, SetupRequest,
};
use crate::runtime::{Runtime, Spawner};
use crate::tools::{OrchestratorCall, ToolCall, ToolHost, ToolReply, WorkerCall};
use crate::work::{RequestState, Task};

/// How long a scripted run may take before the test fails.
const PATIENCE: Duration = Duration::from_secs(60);

/// One turn a scripted CLI is asked to take.
pub(crate) struct Turn {
    pub provider: ProviderKind,
    /// Brigadier's instructions for the session (its system prompt addition), and a worker's
    /// task, its new session's first message.
    pub prompt: String,
    /// What the turn was started with.
    pub input: String,
    /// Where it works: a worker's worktree (its CLI starts in the session's worker folder with
    /// the worktree added), else the CLI's working directory.
    pub cwd: PathBuf,
    /// The session's extra folders (a thread's workspace).
    pub add_dirs: Vec<PathBuf>,
    /// The session's turns before this one.
    pub earlier: u32,
    grant: String,
    host: Arc<SessionManager>,
    events: mpsc::Sender<ProviderEvent>,
    answers: Answers,
    steers: Steers,
    stops: Stops,
}

/// The user's Stop of a session's running turn: the turn may wait for it ([`Turn::stopped`]),
/// and then ends as interrupted.
type Stops = Arc<(std::sync::atomic::AtomicBool, tokio::sync::Notify)>;

/// What was steered into a session's running turn, for the turn to read.
type Steers = Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<String>>>;

/// The approvals a scripted CLI asked for and waits on, by request id.
type Answers =
    Arc<Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<ApprovalDecision>>>>;

/// What a scripted turn ends with.
#[derive(Default)]
pub(crate) struct Reply {
    pub text: String,
    /// The context it reports filled at the end of the turn.
    pub context_tokens: Option<i64>,
}

impl Reply {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            context_tokens: None,
        }
    }
}

pub(crate) type Script = Arc<dyn Fn(Turn) -> BoxFuture<'static, Reply> + Send + Sync>;

impl Turn {
    pub fn is_orchestrator(&self) -> bool {
        self.prompt
            .contains(crate::manager::prompts::THREAD_OPENING)
    }

    /// A one-shot review's session ([`Options::reviews`] answers it).
    pub fn is_review(&self) -> bool {
        self.prompt == brigadier_review::REVIEW_ROLE
    }

    /// The number of the task a worker works on (`task-N`).
    pub fn task_number(&self) -> Option<u32> {
        let rest = self.prompt.split("Task task-").nth(1)?;
        rest.split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok()
    }

    /// Calls a Brigadier tool as the session's model would.
    pub async fn call(&self, name: &str, args: Value) -> ToolReply {
        let call = tool_call(name, args, self.is_orchestrator());
        ToolHost::call(&*self.host, &self.grant, call).await
    }

    /// Asks for approval to run `command` outside the sandbox, as a CLI would, and waits for
    /// the answer (`None`: none came in time).
    pub async fn ask_approval(&self, command: &str, grant: &str) -> Option<ApprovalDecision> {
        self.ask_tool_approval("Bash", command, Some(grant)).await
    }

    /// The same for a call of `tool` (a prompted MCP tool, such as `run_unsandboxed`).
    pub async fn ask_tool_approval(
        &self,
        tool: &str,
        command: &str,
        grant: Option<&str>,
    ) -> Option<ApprovalDecision> {
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.answers.lock().unwrap().insert(id.clone(), tx);
        let request = brigadier_providers::ApprovalRequest {
            id,
            kind: brigadier_providers::model::ApprovalKind::Command,
            tool: tool.into(),
            command: Some(command.into()),
            // A prompted MCP tool's request carries the call's own `workdir` argument (none
            // here); a CLI's Bash, its working directory.
            cwd: (tool == "Bash").then(|| self.cwd.display().to_string()),
            paths: Vec::new(),
            reason: Some("It needs the network.".into()),
            escalation: true,
            input: None,
            grant: grant.map(Into::into),
        };
        self.events
            .send(ProviderEvent::ApprovalRequested { request })
            .await
            .ok()?;
        tokio::time::timeout(PATIENCE, rx).await.ok()?.ok()
    }

    /// Waits for the next message steered into this running turn (`None`: none came in time).
    pub async fn steered(&self) -> Option<String> {
        let mut steers = self.steers.lock().await;
        tokio::time::timeout(PATIENCE, steers.recv())
            .await
            .ok()
            .flatten()
    }

    /// Reports the session's context size mid-turn, as a CLI does after each model call.
    pub async fn report_context(&self, tokens: i64) {
        let _ = self
            .events
            .send(ProviderEvent::ContextSize {
                used_tokens: tokens,
                window_tokens: Some(1_000_000),
            })
            .await;
    }

    /// Waits for the user's Stop of this turn (`false`: none came in time). The turn then ends
    /// as interrupted, whatever it replies.
    pub async fn stopped(&self) -> bool {
        let (stopped, notify) = &*self.stops;
        let wait = async {
            loop {
                let notified = notify.notified();
                if stopped.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                notified.await;
            }
        };
        tokio::time::timeout(PATIENCE, wait).await.is_ok()
    }

    /// Reports `event` as the session's CLI would, mid-turn.
    pub async fn emit(&self, event: ProviderEvent) {
        let _ = self.events.send(event).await;
    }

    /// Runs git in the session's folder.
    pub fn git(&self, args: &[&str]) -> String {
        git(&self.cwd, args)
    }

    /// Writes a file in the session's folder.
    pub fn write(&self, path: &str, text: &str) {
        let path = self.cwd.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

/// The tool call `name` with `args`, as the MCP server would build it for the orchestrator or
/// a worker.
fn tool_call(name: &str, args: Value, orchestrator: bool) -> ToolCall {
    fn arg<T: serde::de::DeserializeOwned>(name: &str, args: Value) -> T {
        serde_json::from_value(args).unwrap_or_else(|err| panic!("{name} arguments: {err}"))
    }
    use OrchestratorCall as O;
    use WorkerCall as W;
    match name {
        "delegate_task" => ToolCall::Orchestrator(O::DelegateTask(arg(name, args))),
        "message_worker" => ToolCall::Orchestrator(O::MessageWorker(arg(name, args))),
        "answer_worker" => ToolCall::Orchestrator(O::AnswerWorker(arg(name, args))),
        "stop_worker" => ToolCall::Orchestrator(O::StopWorker(arg(name, args))),
        "ask_user" => ToolCall::Orchestrator(O::AskUser(arg(name, args))),
        "read_report" => ToolCall::Orchestrator(O::ReadReport(arg(name, args))),
        "query_brain" if orchestrator => ToolCall::Orchestrator(O::QueryBrain(arg(name, args))),
        "query_brain" => ToolCall::Worker(W::QueryBrain(arg(name, args))),
        "plan_phases" => ToolCall::Orchestrator(O::PlanPhases(arg(name, args))),
        "approve_outline" => ToolCall::Orchestrator(O::ApproveOutline(arg(name, args))),
        "start_verifier" => ToolCall::Orchestrator(O::StartVerifier(arg(name, args))),
        "request_approval" => ToolCall::Orchestrator(O::RequestApproval(arg(name, args))),
        "land_phase" => ToolCall::Orchestrator(O::LandPhase(arg(name, args))),
        "finish_session" => ToolCall::Orchestrator(O::FinishSession(arg(name, args))),
        "note_for_user" => ToolCall::Orchestrator(O::NoteForUser(arg(name, args))),
        "list_tasks" => ToolCall::Orchestrator(O::ListTasks),
        "settle_step" => ToolCall::Orchestrator(O::SettleStep(arg(name, args))),
        "end_run" => ToolCall::Orchestrator(O::EndRun(arg(name, args))),
        "ask_orchestrator" => ToolCall::Worker(W::AskOrchestrator(arg(name, args))),
        "submit_outline" => ToolCall::Worker(W::SubmitOutline(arg(name, args))),
        "submit_report" => ToolCall::Worker(W::SubmitReport(arg(name, args))),
        "review_code" => ToolCall::Worker(W::ReviewCode),
        "review_plan" => ToolCall::Orchestrator(O::ReviewPlan(arg(name, args))),
        "read_artifact" => ToolCall::Orchestrator(O::ReadArtifact(arg(name, args))),
        "run" => ToolCall::Orchestrator(O::Run(arg(name, args))),
        "run_unsandboxed" => ToolCall::Orchestrator(O::RunUnsandboxed(arg(name, args))),
        "run_check" if orchestrator => ToolCall::Orchestrator(O::RunCheck(arg(name, args))),
        "run_check" => ToolCall::Worker(W::RunCheck(arg(name, args))),
        "start_preview" => ToolCall::Orchestrator(O::StartPreview(arg(name, args))),
        "stop_preview" => ToolCall::Orchestrator(O::StopPreview(arg(name, args))),
        "preview_log" => ToolCall::Orchestrator(O::PreviewLog(arg(name, args))),
        "project_map" if orchestrator => ToolCall::Orchestrator(O::ProjectMap),
        "project_map" => ToolCall::Worker(W::ProjectMap),
        other => panic!("the flow harness doesn't know the tool {other}"),
    }
}

/// Stored events from JSONL lines of `stream`, `kind` and `payload`.
fn seed_events(jsonl: &str) -> Vec<brigadier_store::NewEvent> {
    #[derive(serde::Deserialize)]
    struct Line {
        stream: String,
        kind: String,
        payload: Box<serde_json::value::RawValue>,
    }
    jsonl
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
        .map(|(index, line)| {
            let line: Line = serde_json::from_str(line).expect("a seed line");
            brigadier_store::NewEvent {
                stream: line.stream,
                kind: line.kind,
                at_ms: 1_759_000_000_000 + index as i64,
                payload: line.payload,
            }
        })
        .collect()
}

/// Runs git in `dir` and returns its output; panics when it fails.
pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Flow Test")
        .env("GIT_AUTHOR_EMAIL", "flow@example.com")
        .env("GIT_COMMITTER_NAME", "Flow Test")
        .env("GIT_COMMITTER_EMAIL", "flow@example.com")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} in {}: {}{}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

// ----- the scripted CLIs ------------------------------------------------------------------

/// Every session the scripted CLIs were started with, in order.
pub(crate) type Specs = Arc<Mutex<Vec<(ProviderKind, SessionSpec)>>>;

/// Native sessions the scripted CLIs refuse to resume (a test makes a resume fail). Shared by
/// every test in the process; each test names its own sessions.
pub(crate) static REFUSED_RESUMES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// A scripted stand-in for one CLI.
struct FakeCli {
    kind: ProviderKind,
    specs: Specs,
    script: Script,
    /// What answers its one-shot reviews ([`no_findings`] unless a test scripts them).
    reviews: Script,
    host: Arc<OnceLock<Weak<SessionManager>>>,
}

/// A one-shot reviewer that finds nothing.
fn no_findings() -> Script {
    Arc::new(|_| Box::pin(async { Reply::text("No findings.") }))
}

impl FakeCli {
    fn models(&self) -> Vec<ModelInfo> {
        let ids: &[(&str, &str)] = match self.kind {
            ProviderKind::Claude => &[("claude-opus-5-5", "Opus 5.5")],
            // Each vendor's frontier model, as the registry rates them: the roles that write,
            // check or merge code run on a vendor's best.
            ProviderKind::Codex => &[("gpt-6-astra", "GPT-6-Astra")],
        };
        ids.iter()
            .map(|(id, name)| ModelInfo {
                id: (*id).into(),
                display_name: (*name).into(),
                description: String::new(),
                resolved: None,
                efforts: vec!["medium".into(), "high".into()],
                default_effort: Some("high".into()),
                is_default: true,
                input_modalities: vec!["text".into()],
                fast: None,
                legacy: false,
            })
            .collect()
    }
}

impl Provider for FakeCli {
    fn kind(&self) -> ProviderKind {
        self.kind
    }

    fn status(&self) -> BoxFuture<'_, ProviderStatus> {
        Box::pin(async move {
            ProviderStatus {
                provider: self.kind,
                path: Some("/fake".into()),
                version: Some("1.0.0".into()),
                logged_in: true,
                auth_method: Some("fake".into()),
                plan: None,
                guidance: None,
                compacts: false,
            }
        })
    }

    fn models(&self) -> BoxFuture<'_, brigadier_providers::Result<ModelCatalog>> {
        Box::pin(async move {
            Ok(ModelCatalog {
                provider: self.kind,
                models: FakeCli::models(self),
                cli_version: Some("1.0.0".into()),
                fetched_at_ms: crate::now_ms(),
            })
        })
    }

    fn quota(&self) -> BoxFuture<'_, brigadier_providers::Result<QuotaSnapshot>> {
        Box::pin(async move {
            Ok(QuotaSnapshot {
                provider: self.kind,
                windows: Vec::new(),
                limit: None,
                observed_at_ms: crate::now_ms(),
                source: QuotaSource::Read,
            })
        })
    }

    fn start(
        &self,
        spec: SessionSpec,
        _ledger: Arc<dyn Ledger>,
    ) -> BoxFuture<'_, brigadier_providers::Result<Started>> {
        Box::pin(async move {
            if let brigadier_providers::model::Origin::Resume { native_id } = &spec.origin
                && REFUSED_RESUMES.lock().unwrap().contains(native_id)
            {
                return Err(brigadier_providers::Error::Spawn(format!(
                    "no session {native_id}"
                )));
            }
            self.specs.lock().unwrap().push((self.kind, spec.clone()));
            let (tx, events) = mpsc::channel(256);
            let grant = spec
                .mcp_servers
                .iter()
                .flat_map(|server| &server.env)
                .find(|(key, _)| key == "BRIGADIER_MCP_GRANT")
                .map(|(_, value)| value.clone())
                .unwrap_or_default();
            let native_id = match &spec.origin {
                brigadier_providers::model::Origin::Resume { native_id } => native_id.clone(),
                _ => uuid::Uuid::new_v4().to_string(),
            };
            let _ = tx
                .send(ProviderEvent::SessionStarted {
                    native_id: native_id.clone(),
                    model: spec.model.clone(),
                    cwd: Some(spec.cwd.display().to_string()),
                    cli_version: Some("1.0.0".into()),
                })
                .await;
            let (steer_tx, steers) = mpsc::unbounded_channel();
            let prompt = spec.append_system_prompt.clone().unwrap_or_default();
            let script = if prompt == brigadier_review::REVIEW_ROLE {
                self.reviews.clone()
            } else {
                self.script.clone()
            };
            let session = Arc::new(FakeSession {
                kind: self.kind,
                native_id,
                prompt,
                cwd: spec.cwd.clone(),
                add_dirs: spec.add_dirs.clone(),
                grant,
                script,
                host: self.host.clone(),
                events: Mutex::new(Some(tx)),
                turns: std::sync::atomic::AtomicU32::new(0),
                answers: Answers::default(),
                brief: Mutex::new(None),
                running: Arc::default(),
                stops: Arc::default(),
                steer_tx,
                steers: Arc::new(tokio::sync::Mutex::new(steers)),
            });
            Ok(Started { session, events })
        })
    }

    fn remove(&self, _artifacts: Vec<Artifact>) -> BoxFuture<'_, brigadier_providers::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn replayer(&self) -> Box<dyn Replayer> {
        Box::new(NoRecordings)
    }

    /// `<cli> --resume <id>` in `cwd`, with the session's MCP variables (its grant).
    fn terminal(
        &self,
        spec: SessionSpec,
        cwd: PathBuf,
    ) -> BoxFuture<'_, brigadier_providers::Result<brigadier_providers::TerminalCommand>> {
        Box::pin(async move {
            let brigadier_providers::model::Origin::Resume { native_id } = &spec.origin else {
                return Err(brigadier_providers::Error::Invalid("not a resume".into()));
            };
            Ok(brigadier_providers::TerminalCommand {
                program: PathBuf::from(format!("/fake/{}", self.kind)),
                args: vec!["--resume".into(), native_id.clone()],
                cwd,
                env: spec
                    .mcp_servers
                    .iter()
                    .flat_map(|server| server.env.iter())
                    .map(|(name, value)| (name.into(), value.into()))
                    .collect(),
            })
        })
    }
}

/// Scripted CLIs have no recordings to replay.
struct NoRecordings;

impl Replayer for NoRecordings {
    fn feed(
        &mut self,
        _dir: brigadier_providers::record::Direction,
        _line: &str,
    ) -> Vec<ProviderEvent> {
        Vec::new()
    }
}

struct FakeSession {
    kind: ProviderKind,
    native_id: String,
    prompt: String,
    cwd: PathBuf,
    add_dirs: Vec<PathBuf>,
    grant: String,
    script: Script,
    host: Arc<OnceLock<Weak<SessionManager>>>,
    events: Mutex<Option<mpsc::Sender<ProviderEvent>>>,
    turns: std::sync::atomic::AtomicU32,
    answers: Answers,
    /// A worker's task, from its first message.
    brief: Mutex<Option<String>>,
    /// A turn is running: a steer joins it rather than starting another.
    running: Arc<std::sync::atomic::AtomicBool>,
    /// Steers into a running turn, which the turn may read ([`Turn::steered`]).
    steer_tx: mpsc::UnboundedSender<String>,
    steers: Steers,
    stops: Stops,
}

impl FakeSession {
    fn sender(&self) -> Option<mpsc::Sender<ProviderEvent>> {
        self.events.lock().unwrap().clone()
    }
}

fn input_text(input: &TurnInput) -> String {
    input
        .parts
        .iter()
        .filter_map(|part| match part {
            brigadier_providers::model::InputPart::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl ProviderSession for FakeSession {
    fn native_id(&self) -> String {
        self.native_id.clone()
    }

    fn send(&self, input: TurnInput) -> BoxFuture<'_, brigadier_providers::Result<()>> {
        Box::pin(async move {
            let Some(tx) = self.sender() else {
                return Err(brigadier_providers::Error::Invalid("the CLI exited".into()));
            };
            // A manager that starts again may resume a session before it is reachable here.
            let host = tokio::time::timeout(PATIENCE, async {
                loop {
                    if let Some(host) = self.host.get().and_then(Weak::upgrade) {
                        break host;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("the manager is running");
            let earlier = self.turns.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let worker = self.prompt.starts_with("You are a Brigadier worker.");
            let prompt = {
                let mut brief = self.brief.lock().unwrap();
                if worker && brief.is_none() {
                    *brief = Some(input_text(&input));
                }
                match &*brief {
                    Some(brief) => format!("{}\n\n{brief}", self.prompt),
                    None => self.prompt.clone(),
                }
            };
            let cwd = match self.add_dirs.first() {
                Some(worktree) if worker => worktree.clone(),
                _ => self.cwd.clone(),
            };
            let turn = Turn {
                provider: self.kind,
                prompt,
                input: input_text(&input),
                cwd,
                add_dirs: self.add_dirs.clone(),
                earlier,
                grant: self.grant.clone(),
                host,
                events: tx.clone(),
                answers: self.answers.clone(),
                steers: self.steers.clone(),
                stops: self.stops.clone(),
            };
            let script = self.script.clone();
            let running = self.running.clone();
            running.store(true, std::sync::atomic::Ordering::SeqCst);
            let stops = self.stops.clone();
            stops.0.store(false, std::sync::atomic::Ordering::SeqCst);
            tokio::spawn(async move {
                let _ = tx.send(ProviderEvent::TurnStarted { turn_id: None }).await;
                let reply = script(turn).await;
                if let Some(used) = reply.context_tokens {
                    let _ = tx
                        .send(ProviderEvent::ContextSize {
                            used_tokens: used,
                            window_tokens: Some(1_000_000),
                        })
                        .await;
                }
                if !reply.text.is_empty() {
                    let _ = tx
                        .send(ProviderEvent::Message {
                            item_id: uuid::Uuid::new_v4().to_string(),
                            role: Speaker::Assistant,
                            text: reply.text,
                        })
                        .await;
                }
                running.store(false, std::sync::atomic::Ordering::SeqCst);
                let status = if stops.0.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    TurnStatus::Interrupted
                } else {
                    TurnStatus::Completed
                };
                let _ = tx
                    .send(ProviderEvent::TurnCompleted {
                        turn_id: None,
                        status,
                        duration_ms: Some(1),
                        usage: None,
                    })
                    .await;
            });
            Ok(())
        })
    }

    fn steer(&self, input: TurnInput) -> BoxFuture<'_, brigadier_providers::Result<()>> {
        // A running turn reads it if its script asks for it ([`Turn::steered`]); otherwise its
        // reply is already decided and the steer changes nothing.
        if self.running.load(std::sync::atomic::Ordering::SeqCst) {
            let _ = self.steer_tx.send(input_text(&input));
            return Box::pin(async { Ok(()) });
        }
        self.send(input)
    }

    fn interrupt(&self) -> BoxFuture<'_, brigadier_providers::Result<()>> {
        if self.running.load(std::sync::atomic::Ordering::SeqCst) {
            self.stops
                .0
                .store(true, std::sync::atomic::Ordering::SeqCst);
            self.stops.1.notify_waiters();
        }
        Box::pin(async { Ok(()) })
    }

    fn answer(
        &self,
        approval_id: String,
        decision: ApprovalDecision,
    ) -> BoxFuture<'_, brigadier_providers::Result<()>> {
        if let Some(waiter) = self.answers.lock().unwrap().remove(&approval_id) {
            let _ = waiter.send(decision);
        }
        Box::pin(async { Ok(()) })
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let tx = self.events.lock().unwrap().take();
            if let Some(tx) = tx {
                let _ = tx
                    .send(ProviderEvent::Exited {
                        code: Some(0),
                        stderr_tail: None,
                    })
                    .await;
            }
        })
    }

    fn is_running(&self) -> bool {
        self.events.lock().unwrap().is_some()
    }
}

/// The store in `data`.
async fn open_store(data: &Path) -> brigadier_store::Store {
    let data = data.to_owned();
    tokio::task::spawn_blocking(move || {
        brigadier_store::Store::open(brigadier_store::StoreConfig {
            db_path: data.join("brigadier.db"),
            blobs_dir: data.join("blobs"),
            readers: 2,
        })
    })
    .await
    .unwrap()
    .unwrap()
}

/// The manager on `store`, its CLIs running `script` (and `reviews` for one-shot reviews),
/// once it has seen both CLIs' models.
async fn boot(
    data: &Path,
    store: brigadier_store::Store,
    script: &Script,
    reviews: &Script,
    specs: &Specs,
) -> (Arc<SessionManager>, Arc<Core>) {
    let data = data.to_owned();
    let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
        data_dir: Some(data.clone()),
    })
    .unwrap();
    let core = Core::load(store).await.unwrap();
    let spawner: Spawner = Arc::new(|task| {
        tokio::spawn(task);
    });
    let host: Arc<OnceLock<Weak<SessionManager>>> = Arc::default();
    let fake = |kind| -> Arc<dyn Provider> {
        Arc::new(FakeCli {
            kind,
            specs: specs.clone(),
            script: script.clone(),
            reviews: reviews.clone(),
            host: host.clone(),
        })
    };
    let runtime = Runtime::start_faked(
        core.clone(),
        platform,
        spawner.clone(),
        [fake(ProviderKind::Claude), fake(ProviderKind::Codex)],
    )
    .await
    .unwrap();
    let manager = SessionManager::start(
        core.clone(),
        runtime.clone(),
        spawner,
        ManagerConfig {
            daemon_exe: PathBuf::from("/fake/brigadierd"),
        },
    )
    .await
    .unwrap();
    host.set(Arc::downgrade(&manager)).ok();
    // The router picks only among models it has seen.
    let mut checked = runtime.provider_checks();
    tokio::time::timeout(PATIENCE, async {
        while ProviderKind::ALL.iter().any(|kind| {
            runtime
                .overview(*kind)
                .is_none_or(|overview| overview.models.is_none())
        }) {
            checked.changed().await.unwrap();
        }
    })
    .await
    .expect("the scripted CLIs are checked");
    (manager, core)
}

// ----- a scripted session -----------------------------------------------------------------

/// A session in a fresh data folder on a fresh repository, its CLIs scripted.
pub(crate) struct Flow {
    pub manager: Arc<SessionManager>,
    pub core: Arc<Core>,
    pub repo: PathBuf,
    pub conversation: ConversationId,
    dir: PathBuf,
    script: Script,
    reviews: Script,
    specs: Specs,
}

/// What a scripted session is set up with.
pub(crate) struct Options {
    pub permission: PermissionLevel,
    pub plan_mode: bool,
    /// Stored events the data folder starts with (JSONL of `stream`, `kind`, `payload`), as
    /// an earlier version left them.
    pub seed: Option<&'static str>,
    /// A copy of a real store's database the data folder starts with.
    pub store: Option<PathBuf>,
    /// What answers the one-shot reviews (both vendors'); by default they find nothing.
    pub reviews: Option<Script>,
    /// The thread's vendor (Claude by default).
    pub thread: ProviderKind,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            permission: PermissionLevel::FullAccess,
            plan_mode: false,
            seed: None,
            store: None,
            reviews: None,
            thread: ProviderKind::Claude,
        }
    }
}

impl Flow {
    /// Starts a session whose Claude and Codex CLIs run `script`.
    pub async fn start(name: &str, options: Options, script: Script) -> Flow {
        let dir = std::env::temp_dir().join(format!(
            "brigadier-flow-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo = repo.canonicalize().unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("README.md"), "# Flow\n").unwrap();
        std::fs::write(repo.join(".gitignore"), "*.log\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "Start"]);
        let data = dir.join("data");
        if let Some(store) = &options.store {
            std::fs::create_dir_all(&data).unwrap();
            std::fs::copy(store, data.join("brigadier.db")).unwrap();
        }
        let store = open_store(&data).await;
        if let Some(seed) = options.seed {
            store.append(seed_events(seed)).await.unwrap();
        }
        let reviews = options.reviews.clone().unwrap_or_else(no_findings);
        let specs = Specs::default();
        let (manager, core) = boot(&data, store, &script, &reviews, &specs).await;
        let project = core
            .create_project("Flow".into(), Some(repo.display().to_string()))
            .await
            .unwrap();
        let conversation = manager
            .create_conversation(
                ConversationKind::Session,
                Some(project.id.clone()),
                Some("Flow".into()),
                Some(SetupRequest::Session {
                    repo: repo.display().to_string(),
                    environment: EnvironmentRequest::NewWorktree {
                        base: "main".into(),
                        branch: None,
                    },
                    permission: options.permission,
                    orchestrator: ModelChoice {
                        provider: options.thread,
                        model: Some(
                            match options.thread {
                                ProviderKind::Claude => "claude-opus-5-5",
                                ProviderKind::Codex => "gpt-6-astra",
                            }
                            .into(),
                        ),
                        effort: None,
                        fast: None,
                    },
                    plan_mode: options.plan_mode,
                }),
            )
            .await
            .unwrap();
        Flow {
            manager,
            core,
            repo,
            conversation: conversation.id,
            dir,
            script,
            reviews,
            specs,
        }
    }

    /// Quits the manager as a daemon that stops does, and starts a new one on the same data
    /// folder: what was recorded carries on.
    pub async fn restart(&mut self) {
        self.manager.shutdown().await;
        let data = self.dir.join("data");
        let store = open_store(&data).await;
        let (manager, core) = boot(&data, store, &self.script, &self.reviews, &self.specs).await;
        self.manager = manager;
        self.core = core;
    }

    /// Sends the user's message.
    pub async fn say(&self, text: &str) {
        self.manager
            .send_message(
                self.conversation.clone(),
                text.into(),
                Vec::new(),
                Vec::new(),
                false,
                None,
            )
            .await
            .unwrap();
    }

    /// The sessions the thread's CLIs were started with, in order.
    pub fn thread_specs(&self) -> Vec<(ProviderKind, SessionSpec)> {
        self.specs
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, spec)| {
                spec.append_system_prompt
                    .as_deref()
                    .is_some_and(|prompt| prompt.contains(crate::manager::prompts::THREAD_OPENING))
                    && matches!(
                        spec.origin,
                        brigadier_providers::model::Origin::New
                            | brigadier_providers::model::Origin::Resume { .. }
                    )
            })
            .cloned()
            .collect()
    }

    pub async fn board(&self) -> Board {
        self.core.board(&self.conversation).await.unwrap()
    }

    /// Waits until `done` holds for the conversation's board.
    pub async fn until(&self, what: &str, done: impl Fn(&Board) -> bool) -> Board {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        loop {
            let board = self.board().await;
            if done(&board) {
                return board;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {what}; tasks: {:#?}",
                board
                    .tasks
                    .values()
                    .map(|task| (task.number, task.kind, task.state, &task.error))
                    .collect::<Vec<_>>()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Waits until every request of the conversation is over.
    pub async fn settled(&self) -> Board {
        self.until("the requests to settle", |board| {
            !board.requests.is_empty()
                && board
                    .requests
                    .values()
                    .all(|request| !matches!(request.state, RequestState::Working))
        })
        .await
    }

    /// The conversation's stored events, oldest first.
    pub async fn events(&self) -> Vec<DomainEvent> {
        let stream = crate::model::streams::conversation(&self.conversation);
        let mut events = Vec::new();
        let mut before = None;
        loop {
            let page = self
                .core
                .store()
                .read_stream(
                    stream.clone(),
                    brigadier_store::StreamPage {
                        before,
                        limit: 500,
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            before = page.last().map(|event| event.stream_seq);
            let full = page.len() == 500;
            events.extend(page);
            if !full {
                break;
            }
        }
        events
            .iter()
            .rev()
            .map(|event| crate::sessions::decode(event).unwrap())
            .collect()
    }

    pub fn task(board: &Board, number: u32) -> &Task {
        board
            .tasks
            .values()
            .find(|task| task.number == number)
            .unwrap_or_else(|| panic!("no task-{number}"))
    }

    pub async fn stop(self) {
        self.manager.shutdown().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod checks_tests;
#[cfg(test)]
mod engine_tests;
#[cfg(test)]
mod overnight_tests;
mod preview_tests;
#[cfg(test)]
mod prewarm_tests;
#[cfg(test)]
mod reads_tests;
#[cfg(test)]
mod takeover_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod thread_tests;
#[cfg(test)]
mod trim_tests;
