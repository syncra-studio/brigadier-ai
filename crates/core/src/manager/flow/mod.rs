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
use crate::tools::{ComputerCall, OrchestratorCall, ToolCall, ToolHost, ToolReply, WorkerCall};
use crate::work::{RequestState, Task};

/// How long a scripted run may take before the test fails: only a hang takes this long. Every
/// wait under it is for the condition the test needs, so this only turns a hang into a failure.
/// A scripted run is CPU-bound, and the machine running the tests may be loaded far past its
/// cores: the longest flows take 5 to 10 s on a calm 14-core Mac, and took 66 to 81 s (and
/// then passed) with six suites and sixty busy loops running beside them (load ~200).
const PATIENCE: Duration = Duration::from_secs(300);

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
    /// The extra account its CLI runs on (`None`: the user's own login).
    pub account: Option<String>,
    /// Its CLI session.
    pub native_id: String,
    grant: String,
    /// A worker's computer-use grant (macOS only).
    computer_grant: String,
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
    /// The turn fails on this usage limit of its account.
    pub limit: Option<brigadier_providers::LimitHit>,
}

impl Reply {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            context_tokens: None,
            limit: None,
        }
    }

    /// The turn fails on its account's 5-hour limit, which resets in an hour.
    pub fn limited() -> Self {
        Self {
            limit: Some(brigadier_providers::LimitHit {
                kind: brigadier_providers::LimitKind::UsageWindow,
                window: Some("five_hour".into()),
                resets_at_ms: Some(crate::now_ms() + 60 * 60 * 1000),
            }),
            ..Self::default()
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

    /// Calls a computer tool with the worker's computer grant, as its `computer` server would.
    pub async fn computer(&self, call: ComputerCall) -> ToolReply {
        ToolHost::call(&*self.host, &self.computer_grant, ToolCall::Computer(call)).await
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
        "propose_merge" => ToolCall::Orchestrator(O::ProposeMerge(arg(name, args))),
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

/// Terminals a test holds as they open, by native session: the open tells the first sender
/// it got there, then waits for the receiver's answer (`true`: the terminal fails to open).
#[allow(clippy::type_complexity)]
pub(crate) static GATED_TERMINALS: Mutex<
    Vec<(
        String,
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<bool>,
    )>,
> = Mutex::new(Vec::new());

/// One cleanup call: provider, account and the native artifacts sent to it.
type Removal = (ProviderKind, Option<String>, Vec<Artifact>);

/// Per-flow knobs and observations; shared across a restart, never across tests.
#[derive(Default)]
pub(crate) struct FakeBehavior {
    pub signed_out: Mutex<Vec<ProviderKind>>,
    pub refuse_steers: bool,
    pub cleanup: bool,
    pub fail_cleanup: std::sync::atomic::AtomicBool,
    pub removals: Mutex<Vec<Removal>>,
    /// The next CLI start signals the first and waits for the second before it records
    /// anything.
    pub hold_start: Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
}

/// Files a scripted CLI keeps in its own home, even when a session is shared.
fn fake_files(home: &Path, id: &str) -> [PathBuf; 2] {
    [home.join("sessions").join(id), home.join("local").join(id)]
}

/// A scripted stand-in for one CLI.
struct FakeCli {
    kind: ProviderKind,
    /// The extra account it stands in for.
    account: Option<String>,
    specs: Specs,
    home: PathBuf,
    behavior: Arc<FakeBehavior>,
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
                // Claude's takes images, so operate work has a model.
                input_modalities: match self.kind {
                    ProviderKind::Claude => vec!["text".into(), "image".into()],
                    ProviderKind::Codex => vec!["text".into()],
                },
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
                logged_in: self.account.is_some()
                    || !self
                        .behavior
                        .signed_out
                        .lock()
                        .unwrap()
                        .contains(&self.kind),
                auth_method: Some("fake".into()),
                plan: None,
                email: None,
                organization: None,
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
        ledger: Arc<dyn Ledger>,
    ) -> BoxFuture<'_, brigadier_providers::Result<Started>> {
        Box::pin(async move {
            if let brigadier_providers::model::Origin::Resume { native_id } = &spec.origin
                && REFUSED_RESUMES.lock().unwrap().contains(native_id)
            {
                return Err(brigadier_providers::Error::Spawn(format!(
                    "no session {native_id}"
                )));
            }
            let held = self.behavior.hold_start.lock().unwrap().take();
            if let Some((reached, release)) = held {
                reached.notify_one();
                release.notified().await;
            }
            self.specs.lock().unwrap().push((self.kind, spec.clone()));
            let (tx, events) = mpsc::channel(256);
            let env = |name: &str| {
                spec.mcp_servers
                    .iter()
                    .flat_map(|server| &server.env)
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value.clone())
                    .unwrap_or_default()
            };
            let grant = env("BRIGADIER_MCP_GRANT");
            let computer_grant = env(crate::manager::computer::GRANT_ENV);
            let native_id = match &spec.origin {
                brigadier_providers::model::Origin::Resume { native_id } => native_id.clone(),
                _ => uuid::Uuid::new_v4().to_string(),
            };
            if self.behavior.cleanup {
                let home = self
                    .account
                    .as_ref()
                    .map(|_| self.home.display().to_string());
                let artifact = match self.kind {
                    ProviderKind::Claude => Artifact::ClaudeSession {
                        session_id: native_id.clone(),
                        home,
                    },
                    ProviderKind::Codex => Artifact::CodexThread {
                        thread_id: native_id.clone(),
                        home,
                    },
                };
                ledger.record(artifact).await?;
                for path in fake_files(&self.home, &native_id) {
                    std::fs::create_dir_all(path.parent().unwrap())?;
                    std::fs::write(path, "session state")?;
                }
            }
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
                behavior: self.behavior.clone(),
                account: self.account.clone(),
                native_id,
                prompt,
                cwd: spec.cwd.clone(),
                add_dirs: spec.add_dirs.clone(),
                grant,
                computer_grant,
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

    fn remove(&self, artifacts: Vec<Artifact>) -> BoxFuture<'_, brigadier_providers::Result<()>> {
        Box::pin(async move {
            self.behavior.removals.lock().unwrap().push((
                self.kind,
                self.account.clone(),
                artifacts.clone(),
            ));
            if self
                .behavior
                .fail_cleanup
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err(brigadier_providers::Error::Invalid(
                    "cleanup refused".into(),
                ));
            }
            for artifact in artifacts {
                let (id, home) = match artifact {
                    Artifact::ClaudeSession { session_id, home } => (session_id, home),
                    Artifact::CodexThread { thread_id, home } => (thread_id, home),
                    _ => continue,
                };
                let expected = self
                    .account
                    .as_ref()
                    .map(|_| self.home.display().to_string());
                if home != expected {
                    return Err(brigadier_providers::Error::Invalid(
                        "cleanup sent to wrong home".into(),
                    ));
                }
                for path in fake_files(&self.home, &id) {
                    if path.exists() {
                        std::fs::remove_file(path)?;
                    }
                }
            }
            Ok(())
        })
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
            let gate = {
                let mut gates = GATED_TERMINALS.lock().unwrap();
                let at = gates.iter().position(|(id, _, _)| id == native_id);
                at.map(|at| gates.remove(at))
            };
            if let Some((_, reached, verdict)) = gate {
                let _ = reached.send(());
                if verdict.await.unwrap_or(false) {
                    return Err(brigadier_providers::Error::Spawn("no terminal".into()));
                }
            }
            Ok(brigadier_providers::TerminalCommand {
                program: PathBuf::from(format!("/fake/{}", self.kind)),
                args: vec!["--resume".into(), native_id.clone()],
                cwd,
                // What the CLI would be started with, for tests to read.
                env: spec
                    .mcp_servers
                    .iter()
                    .flat_map(|server| server.env.iter())
                    .map(|(name, value)| (name.into(), value.into()))
                    .chain([
                        (
                            "FAKE_ACCESS".into(),
                            format!("{:?} auto_review={}", spec.access, spec.auto_review).into(),
                        ),
                        (
                            "FAKE_ACCOUNT".into(),
                            self.account.clone().unwrap_or_default().into(),
                        ),
                    ])
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
    behavior: Arc<FakeBehavior>,
    account: Option<String>,
    native_id: String,
    prompt: String,
    cwd: PathBuf,
    add_dirs: Vec<PathBuf>,
    grant: String,
    computer_grant: String,
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
                account: self.account.clone(),
                native_id: self.native_id.clone(),
                grant: self.grant.clone(),
                computer_grant: self.computer_grant.clone(),
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
                let status = if let Some(limit) = reply.limit {
                    let _ = tx
                        .send(ProviderEvent::Error {
                            error: brigadier_providers::ProviderError {
                                kind: brigadier_providers::ErrorKind::UsageLimit,
                                message: "You've hit your usage limit.".into(),
                                will_retry: false,
                                limit: Some(limit),
                                code: None,
                            },
                        })
                        .await;
                    TurnStatus::Failed
                } else if stops.0.swap(false, std::sync::atomic::Ordering::SeqCst) {
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
        if self.behavior.refuse_steers {
            return Box::pin(async {
                Err(brigadier_providers::Error::Invalid("steer refused".into()))
            });
        }
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
    behavior: &Arc<FakeBehavior>,
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
    let fake = {
        let (specs, script, reviews, host, behavior, data) = (
            specs.clone(),
            script.clone(),
            reviews.clone(),
            host.clone(),
            behavior.clone(),
            data.clone(),
        );
        move |kind: ProviderKind, account: Option<String>| -> Arc<dyn Provider> {
            Arc::new(FakeCli {
                kind,
                home: match &account {
                    Some(id) => data.join("accounts").join(id),
                    None => data.join("own").join(kind.to_string()),
                },
                account,
                behavior: behavior.clone(),
                specs: specs.clone(),
                script: script.clone(),
                reviews: reviews.clone(),
                host: host.clone(),
            })
        }
    };
    let accounts = {
        let fake = fake.clone();
        Arc::new(move |account: &crate::accounts::AccountRef| {
            fake(account.provider, account.account.clone())
        })
    };
    let runtime = Runtime::start_faked(
        core.clone(),
        platform,
        spawner.clone(),
        [
            fake(ProviderKind::Claude, None),
            fake(ProviderKind::Codex, None),
        ],
        accounts,
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

/// Waits until `done` holds, failing after [`PATIENCE`] with `what`.
pub(crate) async fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !done() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// [`eventually`] for a condition that has to wait to be read.
pub(crate) async fn eventually_async<F, Fut>(what: &str, mut done: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !done().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// ----- what a test leaves -----------------------------------------------------------------

/// How a test's folders are named: `brigadier-flow-<name>-<pid>-<uuid>`.
const SCRATCH: &str = "brigadier-flow-";

/// A folder a test made in the temp directory, and the test data folders of the session
/// working in it: removed when dropped, however the test ends (it passed, failed part-way, or
/// failed while its session started), after what that session's daemon still runs is ended.
/// Work the daemon still has in flight then (on the runtime's blocking threads) may write
/// there afterwards, so they are removed once more as the test's thread ends, when its runtime
/// and those threads are gone. A test process killed before it drops them leaves them to the
/// next one's first [`Scratch::new`].
pub(crate) struct Scratch {
    dir: PathBuf,
    /// The ledger of the session working in it.
    ledger: Option<Arc<crate::ledger::CleanupLedger>>,
}

impl Scratch {
    /// A fresh folder named for `name`.
    pub fn new(name: &str) -> Self {
        sweep_killed_tests();
        let scratch = Self {
            dir: std::env::temp_dir().join(format!(
                "{SCRATCH}{name}-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4().simple()
            )),
            ledger: None,
        };
        std::fs::create_dir_all(&scratch.dir).unwrap();
        scratch
    }
}

impl std::ops::Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.dir
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Scratch {
    // Runs while a failed test unwinds, so nothing in it may panic.
    fn drop(&mut self) {
        let folders = self
            .ledger
            .take()
            .map(|ledger| ledger.abandon(&self.dir))
            .unwrap_or_default();
        let left = Left {
            dir: std::mem::take(&mut self.dir),
            folders,
        };
        left.remove();
        let _ = LEFT.try_with(|all| all.0.try_borrow_mut().map(|mut all| all.push(left)));
    }
}

/// What a dropped [`Scratch`] removed.
struct Left {
    dir: PathBuf,
    folders: Vec<PathBuf>,
}

impl Left {
    fn remove(&self) {
        for folder in &self.folders {
            let _ = crate::ledger::remove_test_data_folder(folder);
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// This thread's dropped folders, removed again as it ends.
struct Leftovers(std::cell::RefCell<Vec<Left>>);

impl Drop for Leftovers {
    fn drop(&mut self) {
        for left in self.0.get_mut().drain(..) {
            left.remove();
        }
    }
}

thread_local! {
    static LEFT: Leftovers = const { Leftovers(std::cell::RefCell::new(Vec::new())) };
}

/// Removes the folders of test processes that are gone, once per test process.
fn sweep_killed_tests() {
    static SWEPT: std::sync::Once = std::sync::Once::new();
    SWEPT.call_once(|| remove_killed_tests(&std::env::temp_dir()));
}

/// Removes the folders in `temp` that test processes now gone made with [`Scratch::new`],
/// after ending what still runs in them.
fn remove_killed_tests(temp: &Path) {
    let Ok(platform) = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
        data_dir: Some(temp.to_owned()),
    }) else {
        return;
    };
    let processes = platform.processes();
    let Ok(entries) = std::fs::read_dir(temp) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(made_by) else {
            continue;
        };
        let folder = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if folder && pid != std::process::id() && !processes.is_alive(pid) {
            // Its previews and commands outlived it.
            for orphan in processes.in_dir(&entry.path()).unwrap_or_default() {
                let _ = processes.kill_tree(orphan);
            }
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// The test process that made the folder named `name` with [`Scratch::new`].
fn made_by(name: &str) -> Option<u32> {
    let mut parts = name.strip_prefix(SCRATCH)?.rsplitn(3, '-');
    let id = parts.next()?;
    let pid = parts.next()?;
    parts.next()?;
    let ours = id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit());
    ours.then(|| pid.parse().ok()).flatten()
}

// ----- a scripted session -----------------------------------------------------------------

/// A session in a fresh data folder on a fresh repository, its CLIs scripted.
pub(crate) struct Flow {
    pub manager: Arc<SessionManager>,
    pub core: Arc<Core>,
    pub repo: PathBuf,
    pub conversation: ConversationId,
    dir: Scratch,
    script: Script,
    reviews: Script,
    specs: Specs,
    pub behavior: Arc<FakeBehavior>,
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
    pub behavior: Arc<FakeBehavior>,
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
            behavior: Arc::default(),
        }
    }
}

impl Flow {
    /// Starts a session whose Claude and Codex CLIs run `script`.
    pub async fn start(name: &str, options: Options, script: Script) -> Flow {
        let mut dir = Scratch::new(name);
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
        let (manager, core) =
            boot(&data, store, &script, &reviews, &specs, &options.behavior).await;
        dir.ledger = Some(manager.runtime.ledger().clone());
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
                        account: None,
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
            behavior: options.behavior,
        }
    }

    /// Quits the manager as a daemon that stops does, and starts a new one on the same data
    /// folder: what was recorded carries on.
    pub async fn restart(&mut self) {
        self.manager.shutdown().await;
        // The daemon's quit: nothing more of the old one is written, as its process ends.
        self.core.store().shutdown().await.unwrap();
        let data = self.dir.join("data");
        let store = open_store(&data).await;
        let (manager, core) = boot(
            &data,
            store,
            &self.script,
            &self.reviews,
            &self.specs,
            &self.behavior,
        )
        .await;
        self.dir.ledger = Some(manager.runtime.ledger().clone());
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

    /// Adds extra accounts, each a scripted CLI of its own, sets account switching, and waits
    /// until each was checked.
    pub async fn add_accounts(&self, accounts: &[(ProviderKind, &str)], switch: bool) {
        let mut settings = self.core.settings();
        settings.accounts = accounts
            .iter()
            .map(|(provider, id)| crate::model::AccountEntry {
                id: (*id).into(),
                provider: *provider,
                name: format!("Account {id}"),
                default: false,
                added_at_ms: 0,
            })
            .collect();
        settings.switch_accounts = switch;
        self.core.update_settings(settings).await.unwrap();
        self.manager.runtime.sync_accounts().await;
        let deadline = tokio::time::Instant::now() + PATIENCE;
        while !self
            .manager
            .runtime
            .accounts_view()
            .accounts
            .iter()
            .filter(|view| view.account.account.is_some())
            .all(|view| view.status.is_some() && !view.checking)
        {
            assert!(tokio::time::Instant::now() < deadline, "accounts checked");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
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

    /// Quits the manager as a daemon that stops does. Its folders go with the session (a
    /// daemon that quits keeps its live tasks' test data folders; a test's tasks end with it).
    pub async fn stop(self) {
        self.manager.shutdown().await;
    }
}

#[cfg(test)]
mod accounts_tests;
#[cfg(test)]
mod card_tests;
#[cfg(test)]
mod checks_tests;
#[cfg(all(test, target_os = "macos"))]
mod computer_tests;
#[cfg(test)]
mod engine_tests;
#[cfg(test)]
mod litter_tests;
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
#[cfg(test)]
mod trust_tests;
