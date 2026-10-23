//! Scripted runs of a whole session: the real manager, store, git and worktrees, with each CLI
//! replaced by a script that answers its turns and calls Brigadier's tools as a model would.

// Helpers the flow tests grow into.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use brigadier_providers::model::{
    Artifact, ItemStatus, ModelCatalog, ModelInfo, ProviderStatus, QuotaSnapshot, QuotaSource,
    Role as Speaker, SessionSpec, TurnInput, TurnStatus,
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

/// How long a scripted run may take before the test fails: only a hang takes this long. Every
/// wait under it is for the condition the test needs, so this only turns a hang into a failure.
/// A scripted run is CPU-bound, and the machine running the tests may be loaded far past its
/// cores: the longest flows take 5 to 10 s on a calm 14-core Mac, and took 66 to 81 s (and
/// then passed) with six suites and sixty busy loops running beside them (load ~200). Shorter
/// on CI ([`crate::test_patience`]).
static PATIENCE: std::sync::LazyLock<Duration> = std::sync::LazyLock::new(crate::test_patience);

/// One turn a scripted CLI is asked to take.
#[derive(Clone)]
pub(crate) struct Turn {
    pub provider: ProviderKind,
    /// The tool set its session was started with: a thread's is [`ToolSet::Thread`].
    pub tools: brigadier_providers::ToolSet,
    /// Brigadier's instructions for the session (its system prompt addition), and a worker's
    /// task, its new session's first message.
    pub prompt: String,
    /// What the turn was started with.
    pub input: String,
    /// Where it works: a worker's worktree (its CLI starts in the session's worker folder with
    /// the worktree added), else the CLI's working directory (a thread's workspace).
    pub cwd: PathBuf,
    /// The session's extra folders (a worker's worktree, a tab chat's checkout).
    pub add_dirs: Vec<PathBuf>,
    /// The session's turns before this one.
    pub earlier: u32,
    /// The extra account its CLI runs on (`None`: the user's own login).
    pub account: Option<String>,
    /// Its CLI session.
    pub native_id: String,
    /// Its CLI session's transcript ([`transcribe`]).
    transcript: PathBuf,
    /// Its session file in Claude Code's shape ([`FakeBehavior::session_file`]).
    session_file: Option<PathBuf>,
    grant: String,
    /// What its CLI was started with on top of the user's environment, as its commands see it.
    env: Vec<(String, String)>,
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
    /// The CLI dies mid-turn on this signal, saying "Killed" on stderr.
    pub crash: Option<i32>,
    /// The turn fails on this error (an API error, say), which isn't retried.
    pub error: Option<String>,
    /// Its text streams first, the CLI saying it is the turn's final answer.
    pub answer: bool,
}

impl Reply {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    /// The turn fails on `message`, an error the CLI doesn't retry.
    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            error: Some(message.into()),
            ..Self::default()
        }
    }

    /// The CLI dies mid-turn, killed by `signal`.
    pub fn crashed(signal: i32) -> Self {
        Self {
            crash: Some(signal),
            ..Self::default()
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
    /// A variable of the CLI's environment that Brigadier set, as a command in the turn reads it.
    pub fn env(&self, name: &str) -> Option<&str> {
        self.env
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn is_orchestrator(&self) -> bool {
        self.tools == brigadier_providers::ToolSet::Thread
    }

    /// The number of the task a worker works on (`task-N`).
    pub fn task_number(&self) -> Option<u32> {
        let rest = self.prompt.split("Task task-").nth(1)?;
        rest.split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok()
    }

    /// The conversation whose thread takes this turn (`None` for a worker or a review).
    pub fn conversation(&self) -> Option<ConversationId> {
        match self.host.grants().resolve(&self.grant)? {
            crate::tools::Role::Orchestrator {
                conversation_id, ..
            } => Some(conversation_id),
            _ => None,
        }
    }

    /// The temporary folder (`TMPDIR`) of the session whose thread takes this turn.
    pub fn session_tmp(&self) -> PathBuf {
        let id = self.conversation().expect("a session's thread turn");
        self.host.session_dir(&id).tmp()
    }

    /// The Chat whose CLI takes this turn (`None` for a session's thread, a worker or a
    /// review).
    pub fn chat(&self) -> Option<ConversationId> {
        match self.host.grants().resolve(&self.grant)? {
            crate::tools::Role::Chat { conversation_id } => Some(conversation_id),
            _ => None,
        }
    }

    /// The plain worker whose CLI takes this turn, by the grant its `brigadier` calls carry
    /// (`None` for a session's thread or a Chat).
    pub fn worker(&self) -> Option<crate::work::TaskId> {
        let grant = self.env(brigadier_cli::protocol::GRANT_ENV)?;
        match self.host.grants().resolve(grant)? {
            crate::tools::Role::Worker { task_id, .. } => Some(task_id),
            _ => None,
        }
    }

    /// Starts a plain worker titled `title` on `prompt` as the session's `brigadier start`
    /// would, on its own CLI; answers its task.
    pub async fn start_worker(&self, title: &str, prompt: &str) -> crate::work::TaskId {
        use brigadier_cli::protocol::{CliCall, CliReply, CliRequest, PROTOCOL, StartRequest};
        let session = self
            .env(brigadier_cli::protocol::SESSION_ENV)
            .expect("the CLI knows its session");
        let grant = self
            .env(brigadier_cli::protocol::GRANT_ENV)
            .expect("the CLI has its session's grant");
        let caller = self
            .host
            .cli_caller(session, grant)
            .expect("the session's own grant is accepted");
        let request = CliRequest::Start(Box::new(StartRequest {
            title: Some(title.into()),
            agent: None,
            model: None,
            effort: None,
            local: false,
            base: None,
            permission: None,
            lean: false,
            skills: Vec::new(),
            prompt: prompt.into(),
        }));
        let call = CliCall {
            protocol: PROTOCOL,
            request,
        };
        let shown = match self.host.cli_call(&caller, call).await {
            CliReply::Ok { output } => output["worker"]["id"]
                .as_str()
                .expect("the worker's id")
                .to_owned(),
            CliReply::Error { error } => panic!("brigadier start: {error:?}"),
        };
        let id = self
            .conversation()
            .expect("a session's thread starts workers");
        self.host
            .core
            .tasks(&id)
            .await
            .unwrap()
            .into_iter()
            .find(|task| task.id.0.ends_with(&shown))
            .expect("the started worker's task")
            .id
    }

    /// Calls a Brigadier tool as the session's model would.
    pub async fn call(&self, name: &str, args: Value) -> ToolReply {
        let call = match name {
            "save_report" if self.chat().is_some() => {
                ToolCall::Chat(crate::tools::ChatCall::SaveReport(
                    serde_json::from_value(args).expect("save_report arguments"),
                ))
            }
            _ => tool_call(name, args, self.is_orchestrator()),
        };
        let reply = ToolHost::call(&*self.host, &self.grant, call).await;
        transcribe(&self.transcript, false, &reply.text);
        reply
    }

    /// Records `text` as a tool's output in its CLI session's transcript, as if a tool of
    /// the turn had shown it.
    pub fn tool_output(&self, text: &str) {
        transcribe(&self.transcript, false, text);
    }

    /// Asks for approval to run `command` outside the sandbox, as a CLI would, and waits for
    /// the answer (`None`: none came in time).
    pub async fn ask_approval(&self, command: &str, grant: &str) -> Option<ApprovalDecision> {
        self.ask_tool_approval("Bash", command, Some(grant)).await
    }

    /// The same for a call of `tool`.
    pub async fn ask_tool_approval(
        &self,
        tool: &str,
        command: &str,
        grant: Option<&str>,
    ) -> Option<ApprovalDecision> {
        self.ask(brigadier_providers::ApprovalRequest {
            id: String::new(),
            kind: brigadier_providers::model::ApprovalKind::Command,
            tool: tool.into(),
            command: Some(command.into()),
            // A CLI's Bash request carries its working directory.
            cwd: (tool == "Bash").then(|| self.cwd.display().to_string()),
            paths: Vec::new(),
            reason: Some("It needs the network.".into()),
            escalation: true,
            input: None,
            grant: grant.map(Into::into),
        })
        .await
    }

    /// Asks to change `paths` outside the sandbox, as Codex does, and waits for the answer.
    pub async fn ask_file_approval(&self, paths: &[&Path]) -> Option<ApprovalDecision> {
        self.ask(brigadier_providers::ApprovalRequest {
            id: String::new(),
            kind: brigadier_providers::model::ApprovalKind::FileChange,
            tool: "fileChange".into(),
            command: None,
            cwd: None,
            paths: paths
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
            reason: None,
            escalation: true,
            input: None,
            grant: None,
        })
        .await
    }

    /// Asks to reach `host` from a sandbox without network, as Claude does.
    pub async fn ask_network_approval(&self, host: &str) -> Option<ApprovalDecision> {
        self.ask(brigadier_providers::ApprovalRequest {
            id: String::new(),
            kind: brigadier_providers::model::ApprovalKind::Tool,
            tool: brigadier_providers::policy::NETWORK_TOOL.into(),
            command: None,
            cwd: None,
            paths: Vec::new(),
            reason: None,
            escalation: false,
            input: None,
            grant: Some(host.into()),
        })
        .await
    }

    /// Sends `request` (under a fresh id) as the CLI's and waits for the answer.
    async fn ask(
        &self,
        mut request: brigadier_providers::ApprovalRequest,
    ) -> Option<ApprovalDecision> {
        request.id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.answers.lock().unwrap().insert(request.id.clone(), tx);
        self.events
            .send(ProviderEvent::ApprovalRequested { request })
            .await
            .ok()?;
        tokio::time::timeout(*PATIENCE, rx).await.ok()?.ok()
    }

    /// Waits for the next message steered into this running turn (`None`: none came in time).
    pub async fn steered(&self) -> Option<String> {
        let mut steers = self.steers.lock().await;
        tokio::time::timeout(*PATIENCE, steers.recv())
            .await
            .ok()
            .flatten()
    }

    /// Waits until a message is steered into this running turn, leaving it unread for the
    /// turn to take in once its script ends.
    pub async fn until_steered(&self) {
        tokio::time::timeout(*PATIENCE, async {
            while self.steers.lock().await.is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("a message steered in");
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

    /// Waits for the user's Stop of this turn, or its CLI's close (`false`: none came in
    /// time). The turn then ends as interrupted, whatever it replies.
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
        tokio::time::timeout(*PATIENCE, wait).await.is_ok()
    }

    /// Reports `event` as the session's CLI would, mid-turn.
    pub async fn emit(&self, event: ProviderEvent) {
        // Claude keeps a finished command in its session file: its Bash call, then the result.
        if let (
            Some(file),
            ProviderEvent::Command {
                item_id,
                command,
                status: ItemStatus::Completed,
                output,
                ..
            },
        ) = (&self.session_file, &event)
        {
            session_line(
                file,
                serde_json::json!({"type": "assistant", "message": {
                    "id": format!("msg_{}", uuid::Uuid::new_v4().simple()), "role": "assistant",
                    "content": [{"type": "tool_use", "id": item_id, "name": "Bash",
                        "input": {"command": command}}]}}),
            );
            session_line(
                file,
                serde_json::json!({"type": "user", "message": {"role": "user",
                    "content": [{"type": "tool_result", "tool_use_id": item_id,
                        "content": output.clone().unwrap_or_default()}]}}),
            );
        }
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

/// The tool call `name` with `args`, as `brigadier`'s subcommand builds it for the
/// orchestrator or a worker.
fn tool_call(name: &str, args: Value, orchestrator: bool) -> ToolCall {
    fn arg<T: serde::de::DeserializeOwned>(name: &str, args: Value) -> T {
        serde_json::from_value(args).unwrap_or_else(|err| panic!("{name} arguments: {err}"))
    }
    use crate::tools::BrowserToolCall as B;
    use OrchestratorCall as O;
    use WorkerCall as W;
    match name {
        "request_approval" => ToolCall::Orchestrator(O::RequestApproval(arg(name, args))),
        "offer_feedback" => ToolCall::Orchestrator(O::OfferFeedback(arg(name, args))),
        "read_page" => ToolCall::Orchestrator(O::ReadPage(arg(name, args))),
        "write_page" => ToolCall::Orchestrator(O::WritePage(arg(name, args))),
        "save_report" => ToolCall::Orchestrator(O::SaveReport(arg(name, args))),
        "start_preview" if orchestrator => ToolCall::Orchestrator(O::StartPreview(arg(name, args))),
        "stop_preview" if orchestrator => ToolCall::Orchestrator(O::StopPreview(arg(name, args))),
        "preview_log" if orchestrator => ToolCall::Orchestrator(O::PreviewLog(arg(name, args))),
        "start_preview" => ToolCall::Worker(W::StartPreview(arg(name, args))),
        "stop_preview" => ToolCall::Worker(W::StopPreview(arg(name, args))),
        "preview_log" => ToolCall::Worker(W::PreviewLog(arg(name, args))),
        "project_map" => ToolCall::Orchestrator(O::ProjectMap),
        "app_state" => ToolCall::App(crate::tools::AppToolCall::State),
        "read_terminal" => ToolCall::App(crate::tools::AppToolCall::ReadTerminal(arg(name, args))),
        "open_in_app" => ToolCall::App(crate::tools::AppToolCall::Open(arg(name, args))),
        "terminal_run" => ToolCall::App(crate::tools::AppToolCall::TerminalRun(arg(name, args))),
        "show_session" => ToolCall::App(crate::tools::AppToolCall::ShowSession(arg(name, args))),
        "close_tab" => ToolCall::App(crate::tools::AppToolCall::CloseTab(arg(name, args))),
        "browser_open" => ToolCall::Browser(B::Open(arg(name, args))),
        "browser_navigate" => ToolCall::Browser(B::Navigate(arg(name, args))),
        "browser_read" => ToolCall::Browser(B::Read(arg(name, args))),
        "browser_click" => ToolCall::Browser(B::Click(arg(name, args))),
        "browser_type" => ToolCall::Browser(B::Type(arg(name, args))),
        "browser_screenshot" => ToolCall::Browser(B::Screenshot(arg(name, args))),
        "browser_close" => ToolCall::Browser(B::Close(arg(name, args))),
        "browser_show" => ToolCall::Browser(B::Show(arg(name, args))),
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

/// The app's click on "Merge into main", as the daemon records it.
pub(crate) fn click() -> crate::work::AnswerOrigin {
    crate::work::AnswerOrigin {
        input: Some(crate::work::AnswerInput::Click),
        client: "Brigadier".into(),
        pid: 4242,
    }
}

/// Runs git in `dir` and returns its output; panics when it fails.
pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
    let output = crate::test_git_command()
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

/// A worker session's brief, by native session: a resumed session still has it, as a real
/// CLI's transcript does.
static BRIEFS: std::sync::LazyLock<Mutex<std::collections::HashMap<String, String>>> =
    std::sync::LazyLock::new(Mutex::default);

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
    /// Codex also lists a light model, so cheap work routes to it.
    pub light_model: bool,
    pub refuse_steers: bool,
    /// A message steered into a turn that its script never read is taken in once the script
    /// ends: the script runs again on it, in the same turn, as a real CLI's model works on.
    /// Off, it is dropped.
    pub take_in_steers: bool,
    pub cleanup: bool,
    /// Claude keeps each session's thread in a session file of its own shape, as Claude Code
    /// does ([`session_line`]), and names it at its start.
    pub session_file: bool,
    pub fail_cleanup: std::sync::atomic::AtomicBool,
    pub cleanup_error: Mutex<Option<String>>,
    pub removals: Mutex<Vec<Removal>>,
    /// The next CLI start signals the first and waits for the second before it records
    /// anything.
    pub hold_start: Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
    /// The next turn's send signals the first once the CLI took its input, and waits for the
    /// second before its turn starts.
    pub hold_send: Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
    /// The next login check of an extra account signals the first and waits for the second.
    pub hold_status: Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
}

/// Adds a line to a scripted session's transcript, as a real CLI keeps one: an input it took
/// (a turn's message or a steer), or a tool's output.
fn transcribe(path: &Path, input: bool, text: &str) {
    use std::io::Write;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let line = serde_json::json!({ "input": input, "text": text });
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(file, "{line}").unwrap();
}

/// Adds a line to a scripted Claude session's file in Claude Code's own shape, stamped now.
fn session_line(path: &Path, mut line: Value) {
    use std::io::Write;
    let ms = crate::now_ms();
    let (days, rest) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    // Days since 1970-01-01 as a civil date (Howard Hinnant's days_from_civil, inverted).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    line["timestamp"] = Value::String(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1000 % 60,
        rest % 1000
    ));
    line["uuid"] = line
        .get("uuid")
        .cloned()
        .unwrap_or_else(|| Value::String(uuid::Uuid::new_v4().to_string()));
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(file, "{line}").unwrap();
}

/// Files a scripted CLI keeps in its own home, even when a session is shared.
fn fake_files(home: &Path, id: &str) -> [PathBuf; 2] {
    [home.join("sessions").join(id), home.join("local").join(id)]
}

/// A scripted stand-in for one CLI.
struct FakeCli {
    kind: ProviderKind,
    /// Where each session's transcript is kept ([`transcribe`]), shared by every account.
    transcripts: PathBuf,
    /// The extra account it stands in for.
    account: Option<String>,
    specs: Specs,
    home: PathBuf,
    behavior: Arc<FakeBehavior>,
    script: Script,
    host: Arc<OnceLock<Weak<SessionManager>>>,
}

impl FakeCli {
    fn models(&self) -> Vec<ModelInfo> {
        let ids: &[(&str, &str)] = match self.kind {
            ProviderKind::Claude => &[("claude-opus-5-5", "Opus 5.5")],
            ProviderKind::Codex if self.behavior.light_model => {
                &[("gpt-6-astra", "GPT-6-Astra"), ("gpt-6-luna", "GPT-6-Luna")]
            }
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
            let held = self
                .account
                .as_ref()
                .and_then(|_| self.behavior.hold_status.lock().unwrap().take());
            if let Some((reached, release)) = held {
                reached.notify_one();
                release.notified().await;
            }
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
            // The grant its `brigadier` calls carry.
            let grant = spec
                .env
                .iter()
                .find(|(key, _)| key == brigadier_cli::protocol::GRANT_ENV)
                .map(|(_, value)| value.clone())
                .unwrap_or_default();
            let native_id = match &spec.origin {
                brigadier_providers::model::Origin::Resume { native_id } => native_id.clone(),
                _ => uuid::Uuid::new_v4().to_string(),
            };
            // Like Codex's adapter: an ephemeral thread is never saved, so nothing is recorded.
            let saved = !(spec.ephemeral && self.kind == ProviderKind::Codex);
            if self.behavior.cleanup && saved {
                let home = self
                    .account
                    .as_ref()
                    .map(|_| self.home.display().to_string());
                let artifact = match self.kind {
                    ProviderKind::Claude => Artifact::ClaudeSession {
                        session_id: native_id.clone(),
                        home,
                        cwd: None,
                    },
                    ProviderKind::Codex => Artifact::CodexThread {
                        thread_id: native_id.clone(),
                        home,
                        cwd: None,
                    },
                };
                ledger.record(artifact).await?;
                for path in fake_files(&self.home, &native_id) {
                    std::fs::create_dir_all(path.parent().unwrap())?;
                    std::fs::write(path, "session state")?;
                }
            }
            let session_file = (self.behavior.session_file && self.kind == ProviderKind::Claude)
                .then(|| {
                    self.home
                        .join("projects")
                        .join(format!("{native_id}.jsonl"))
                });
            if let Some(file) = &session_file {
                std::fs::create_dir_all(file.parent().unwrap())?;
            }
            let file = match &session_file {
                Some(file) => Some(file.display().to_string()),
                None => (self.behavior.cleanup && saved)
                    .then(|| fake_files(&self.home, &native_id)[0].display().to_string()),
            };
            let _ = tx
                .send(ProviderEvent::SessionStarted {
                    native_id: native_id.clone(),
                    model: spec.model.clone(),
                    cwd: Some(spec.cwd.display().to_string()),
                    cli_version: Some("1.0.0".into()),
                    file,
                })
                .await;
            let (steer_tx, steers) = mpsc::unbounded_channel();
            let prompt = spec.append_system_prompt.clone().unwrap_or_default();
            let script = self.script.clone();
            let brief = BRIEFS.lock().unwrap().get(&native_id).cloned();
            let session = Arc::new(FakeSession {
                kind: self.kind,
                transcript: self.transcripts.join(&native_id),
                session_file,
                behavior: self.behavior.clone(),
                account: self.account.clone(),
                native_id,
                prompt,
                tools: spec.tools,
                cwd: spec.cwd.clone(),
                add_dirs: spec.add_dirs.clone(),
                grant,
                env: spec.env.clone(),
                script,
                host: self.host.clone(),
                events: Mutex::new(Some(tx)),
                turns: std::sync::atomic::AtomicU32::new(0),
                answers: Answers::default(),
                brief: Mutex::new(brief),
                running: Arc::default(),
                stops: Arc::default(),
                steer_tx,
                steers: Arc::new(tokio::sync::Mutex::new(steers)),
            });
            Ok(Started { session, events })
        })
    }

    fn took_input(&self, native_id: &str, needle: &str) -> BoxFuture<'_, bool> {
        let text = std::fs::read_to_string(self.transcripts.join(native_id)).unwrap_or_default();
        let took = text.lines().any(|line| {
            serde_json::from_str::<Value>(line).is_ok_and(|line| {
                line["input"] == true && line["text"].as_str().is_some_and(|t| t.contains(needle))
            })
        });
        Box::pin(async move { took })
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
                    self.behavior
                        .cleanup_error
                        .lock()
                        .unwrap()
                        .clone()
                        .unwrap_or_else(|| "cleanup refused".into()),
                ));
            }
            for artifact in artifacts {
                let (id, home) = match artifact {
                    Artifact::ClaudeSession {
                        session_id, home, ..
                    } => (session_id, home),
                    Artifact::CodexThread {
                        thread_id, home, ..
                    } => (thread_id, home),
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

    /// `<cli> --resume <id>` in `cwd`, with the session's variables (its grant).
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
                    .env
                    .iter()
                    .map(|(name, value)| (name.into(), value.into()))
                    .chain([
                        ("FAKE_ACCESS".into(), format!("{:?}", spec.access).into()),
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
    /// Its transcript ([`transcribe`]).
    transcript: PathBuf,
    /// Its session file in Claude Code's shape ([`FakeBehavior::session_file`]).
    session_file: Option<PathBuf>,
    behavior: Arc<FakeBehavior>,
    account: Option<String>,
    native_id: String,
    prompt: String,
    tools: brigadier_providers::ToolSet,
    cwd: PathBuf,
    add_dirs: Vec<PathBuf>,
    grant: String,
    env: Vec<(String, String)>,
    script: Script,
    host: Arc<OnceLock<Weak<SessionManager>>>,
    events: Mutex<Option<mpsc::Sender<ProviderEvent>>>,
    turns: std::sync::atomic::AtomicU32,
    answers: Answers,
    /// A worker's task, from its first message.
    brief: Mutex<Option<String>>,
    /// A turn is running: a steer joins it rather than starting another. Held while a steer
    /// joins, and while a turn ending finds none unread, so none is lost in between.
    running: Arc<Mutex<bool>>,
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

/// What was steered into a turn and not read yet.
fn unread_steers(steers: &Steers) -> Vec<String> {
    let Ok(mut steers) = steers.try_lock() else {
        return Vec::new();
    };
    std::iter::from_fn(|| steers.try_recv().ok()).collect()
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
            transcribe(&self.transcript, true, &input_text(&input));
            let held = self.behavior.hold_send.lock().unwrap().take();
            if let Some((reached, release)) = held {
                reached.notify_one();
                release.notified().await;
            }
            // A manager that starts again may resume a session before it is reachable here.
            let host = tokio::time::timeout(*PATIENCE, async {
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
                    BRIEFS
                        .lock()
                        .unwrap()
                        .insert(self.native_id.clone(), input_text(&input));
                }
                match &*brief {
                    Some(brief) => format!("{}\n\n{brief}", self.prompt),
                    None => self.prompt.clone(),
                }
            };
            let cwd = self.cwd.clone();
            let turn = Turn {
                provider: self.kind,
                tools: self.tools,
                prompt,
                input: input_text(&input),
                cwd,
                add_dirs: self.add_dirs.clone(),
                earlier,
                account: self.account.clone(),
                native_id: self.native_id.clone(),
                transcript: self.transcript.clone(),
                session_file: self.session_file.clone(),
                grant: self.grant.clone(),
                env: self.env.clone(),
                host,
                events: tx.clone(),
                answers: self.answers.clone(),
                steers: self.steers.clone(),
                stops: self.stops.clone(),
            };
            let script = self.script.clone();
            let running = self.running.clone();
            *running.lock().unwrap() = true;
            let steers = self.steers.clone();
            let take_in_steers = self.behavior.take_in_steers;
            let stops = self.stops.clone();
            stops.0.store(false, std::sync::atomic::Ordering::SeqCst);
            let session_file = self.session_file.clone();
            let said = input_text(&input);
            tokio::spawn(async move {
                let _ = tx.send(ProviderEvent::TurnStarted { turn_id: None }).await;
                // Claude echoes the message it took, with its line's id in the session file.
                if let Some(file) = &session_file {
                    let uuid = uuid::Uuid::new_v4().to_string();
                    session_line(
                        file,
                        serde_json::json!({"type": "user", "uuid": uuid, "parentUuid": null,
                            "message": {"role": "user", "content": said}}),
                    );
                    let _ = tx
                        .send(ProviderEvent::Message {
                            item_id: uuid,
                            role: Speaker::User,
                            text: said,
                        })
                        .await;
                }
                let mut turn = turn;
                let reply = loop {
                    let again = turn.clone();
                    let mut reply = script(turn).await;
                    if let Some(used) = reply.context_tokens {
                        let _ = tx
                            .send(ProviderEvent::ContextSize {
                                used_tokens: used,
                                window_tokens: Some(1_000_000),
                            })
                            .await;
                    }
                    if !reply.text.is_empty() {
                        let item_id = match &session_file {
                            // Claude's text block id: its message's id and the block's index.
                            Some(file) => {
                                let message = format!("msg_{}", uuid::Uuid::new_v4().simple());
                                session_line(
                                    file,
                                    serde_json::json!({"type": "assistant", "message": {
                                        "id": message, "role": "assistant",
                                        "content": [{"type": "text", "text": reply.text}]}}),
                                );
                                format!("{message}:0")
                            }
                            None => uuid::Uuid::new_v4().to_string(),
                        };
                        if reply.answer {
                            let _ = tx
                                .send(ProviderEvent::MessageDelta {
                                    item_id: item_id.clone(),
                                    text: reply.text.clone(),
                                    phase: Some(brigadier_providers::TextPhase::Answer),
                                })
                                .await;
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        }
                        let _ = tx
                            .send(ProviderEvent::Message {
                                item_id,
                                role: Speaker::Assistant,
                                text: std::mem::take(&mut reply.text),
                            })
                            .await;
                    }
                    // A message steered in that the script never read, taken in
                    // ([`FakeBehavior::take_in_steers`]) unless the turn ends otherwise.
                    let ends = !take_in_steers
                        || reply.crash.is_some()
                        || reply.limit.is_some()
                        || reply.error.is_some()
                        || stops.0.load(std::sync::atomic::Ordering::SeqCst);
                    let unread = {
                        let mut running = running.lock().unwrap();
                        let unread = if ends {
                            Vec::new()
                        } else {
                            unread_steers(&steers)
                        };
                        *running = !unread.is_empty();
                        unread
                    };
                    if unread.is_empty() {
                        break reply;
                    }
                    turn = Turn {
                        input: unread.join("\n"),
                        ..again
                    };
                };
                if let Some(signal) = reply.crash {
                    let _ = tx
                        .send(ProviderEvent::Exited {
                            code: None,
                            signal: Some(signal),
                            stderr_tail: Some("Killed".into()),
                        })
                        .await;
                    return;
                }
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
                } else if let Some(message) = reply.error {
                    if let Some(file) = &session_file {
                        session_line(
                            file,
                            serde_json::json!({"type": "assistant", "isApiErrorMessage": true,
                                "error": "invalid_request", "message": {
                                    "id": format!("msg_{}", uuid::Uuid::new_v4().simple()),
                                    "role": "assistant",
                                    "content": [{"type": "text", "text": message}]}}),
                        );
                    }
                    let _ = tx
                        .send(ProviderEvent::Error {
                            error: brigadier_providers::ProviderError {
                                kind: brigadier_providers::ErrorKind::InvalidRequest,
                                message,
                                will_retry: false,
                                limit: None,
                                code: None,
                            },
                        })
                        .await;
                    TurnStatus::Failed
                } else if stops.0.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    if let Some(file) = &session_file {
                        session_line(
                            file,
                            serde_json::json!({"type": "user", "message": {"role": "user",
                                "content": [{"type": "text",
                                    "text": "[Request interrupted by user]"}]}}),
                        );
                    }
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
        // A running turn reads it if its script asks for it ([`Turn::steered`]), or takes it in
        // once its script ends ([`FakeBehavior::take_in_steers`]); otherwise its reply is
        // already decided and the steer changes nothing.
        let running = self.running.lock().unwrap();
        if *running {
            transcribe(&self.transcript, true, &input_text(&input));
            let _ = self.steer_tx.send(input_text(&input));
            return Box::pin(async { Ok(()) });
        }
        drop(running);
        self.send(input)
    }

    fn interrupt(&self) -> BoxFuture<'_, brigadier_providers::Result<()>> {
        if *self.running.lock().unwrap() {
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
            // Its running turn ends with it, as a real CLI's does with its process: a script
            // waiting for its stop sees it.
            if *self.running.lock().unwrap() {
                self.stops
                    .0
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                self.stops.1.notify_waiters();
            }
            let tx = self.events.lock().unwrap().take();
            if let Some(tx) = tx {
                let _ = tx
                    .send(ProviderEvent::Exited {
                        code: Some(0),
                        signal: None,
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

/// The manager on `store`, its CLIs running `script`, once it has seen both CLIs' models.
async fn boot(
    data: &Path,
    store: brigadier_store::Store,
    script: &Script,
    specs: &Specs,
    behavior: &Arc<FakeBehavior>,
) -> (Arc<SessionManager>, Arc<Core>) {
    let data = data.to_owned();
    let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
        data_dir: Some(data.to_owned()),
    })
    .unwrap();
    let core = Core::load(store).await.unwrap();
    let spawner: Spawner = Arc::new(|task| {
        tokio::spawn(task);
    });
    let host: Arc<OnceLock<Weak<SessionManager>>> = Arc::default();
    let fake = {
        let (specs, script, host, behavior, data) = (
            specs.clone(),
            script.clone(),
            host.clone(),
            behavior.clone(),
            data.clone(),
        );
        move |kind: ProviderKind, account: Option<String>| -> Arc<dyn Provider> {
            Arc::new(FakeCli {
                kind,
                transcripts: data.join("transcripts"),
                home: match &account {
                    Some(id) => data.join("accounts").join(id),
                    None => data.join("own").join(kind.to_string()),
                },
                account,
                behavior: behavior.clone(),
                specs: specs.clone(),
                script: script.clone(),
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
            // Inside an app bundle that has no computer-use helper until a test puts one there.
            daemon_exe: data.join("Brigadier.app/Contents/MacOS/brigadierd"),
        },
    )
    .await
    .unwrap();
    // The tests' window answers browser calls on every platform, not only where agents browse.
    *manager.agents_browse_here.lock().unwrap() = Some(true);
    host.set(Arc::downgrade(&manager)).ok();
    // The router picks only among models it has seen.
    let mut checked = runtime.provider_checks();
    tokio::time::timeout(*PATIENCE, async {
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
    let deadline = tokio::time::Instant::now() + *PATIENCE;
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
    let deadline = tokio::time::Instant::now() + *PATIENCE;
    while !done().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Waits until the session's thread scratch sweep after its last turn is over and left `tmp`
/// empty. A folder that reads empty isn't enough: a sweep still at it removes what the test puts
/// there next (#187).
pub(crate) async fn scratch_sweep_over(flow: &Flow, tmp: &std::path::Path) {
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    eventually_async("the thread scratch sweep", || {
        let conv = conv.clone();
        async move {
            !conv.scratch_used.load(std::sync::atomic::Ordering::SeqCst)
                && conv.scratch_idle().await
                && std::fs::read_dir(tmp).is_ok_and(|mut entries| entries.next().is_none())
        }
    })
    .await;
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
        Self::within(&std::env::temp_dir(), name)
    }

    /// A fresh folder named for `name` under `/tmp`, whatever the temp directory is, for paths
    /// that must be there (a preview's `BRIGADIER_DATA_DIR`): where the tests' sessions make
    /// their folders ([`crate::manager::workers::test_data_root`]).
    #[cfg(target_os = "macos")]
    pub fn in_tmp(name: &str) -> Self {
        Self::within(&crate::manager::workers::test_data_root(), name)
    }

    fn within(root: &Path, name: &str) -> Self {
        sweep_killed_tests();
        let scratch = Self {
            dir: root.join(format!(
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
        left.remove(1);
        let _ = LEFT.try_with(|all| all.0.try_borrow_mut().map(|mut all| all.push(left)));
    }
}

/// What a dropped [`Scratch`] removed.
struct Left {
    dir: PathBuf,
    folders: Vec<PathBuf>,
}

impl Left {
    /// Removes them, trying the session's folder up to `tries` times.
    fn remove(&self, tries: u32) {
        for folder in &self.folders {
            let _ = crate::ledger::remove_test_data_folder(folder);
        }
        for _ in 1..tries {
            if std::fs::remove_dir_all(&self.dir).is_ok() || !self.dir.exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// This thread's dropped folders, removed again as it ends.
struct Leftovers(std::cell::RefCell<Vec<Left>>);

impl Drop for Leftovers {
    fn drop(&mut self) {
        // The test's runtime is gone, but its store's threads may still be closing the
        // database, and Windows removes no file still open: there it takes a few tries.
        let tries = if cfg!(windows) { 100 } else { 1 };
        for left in self.0.get_mut().drain(..) {
            left.remove(tries);
        }
    }
}

thread_local! {
    static LEFT: Leftovers = const { Leftovers(std::cell::RefCell::new(Vec::new())) };
}

/// Removes the folders of test processes that are gone, once per test process.
fn sweep_killed_tests() {
    static SWEPT: std::sync::Once = std::sync::Once::new();
    SWEPT.call_once(|| {
        remove_killed_tests(&std::env::temp_dir());
        #[cfg(target_os = "macos")]
        if crate::manager::workers::temp_dir_in_tmp().is_none() {
            remove_killed_tests(Path::new("/tmp"));
        }
    });
}

/// Removes the folders in `temp` that test processes now gone made with [`Scratch::new`] (or
/// [`Scratch::in_tmp`]),
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
    specs: Specs,
    pub behavior: Arc<FakeBehavior>,
    /// The permission level its sessions start on.
    permission: PermissionLevel,
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
    /// The thread's vendor (Claude by default).
    pub thread: ProviderKind,
    pub behavior: Arc<FakeBehavior>,
    /// Files the repository starts with besides its README (path, contents).
    pub files: Vec<(&'static str, &'static str)>,
    /// The user's answer to "Trust this folder?" for the repository (trusted by default, as
    /// the app asks when the project is first entered); `None` leaves it unanswered.
    pub trusted: Option<bool>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            permission: PermissionLevel::Full,
            plan_mode: false,
            seed: None,
            store: None,
            thread: ProviderKind::Claude,
            behavior: Arc::default(),
            files: Vec::new(),
            trusted: Some(true),
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
        for (path, text) in &options.files {
            let path = repo.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
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
        let specs = Specs::default();
        let (manager, core) = boot(&data, store, &script, &specs, &options.behavior).await;
        dir.ledger = Some(manager.runtime.ledger().clone());
        let project = core
            .create_project("Flow".into(), Some(repo.display().to_string()))
            .await
            .unwrap();
        if let Some(trusted) = options.trusted {
            core.set_folder_trust(&project.id, &repo.display().to_string(), trusted)
                .await
                .unwrap();
        }
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
            specs,
            behavior: options.behavior,
            permission: options.permission,
        }
    }

    /// Quits the manager as a daemon that stops does, and starts a new one on the same data
    /// folder: what was recorded carries on.
    pub async fn restart(&mut self) {
        self.restart_after(Duration::ZERO).await;
    }

    /// [`Self::restart`], with Brigadier closed for `closed` in between.
    pub async fn restart_after(&mut self, closed: Duration) {
        self.manager.shutdown().await;
        // The daemon's quit: nothing more of the old one is written, as its process ends.
        self.core.store().shutdown().await.unwrap();
        tokio::time::sleep(closed).await;
        let data = self.dir.join("data");
        let store = open_store(&data).await;
        let (manager, core) = boot(&data, store, &self.script, &self.specs, &self.behavior).await;
        self.dir.ledger = Some(manager.runtime.ledger().clone());
        self.manager = manager;
        self.core = core;
    }

    /// The daemon dies (unless a fault already killed it) and a new one starts on the same
    /// data folder: nothing of the old one is written after it died, and nothing of it is shut
    /// down cleanly.
    pub async fn crash(&mut self) {
        self.manager.halt().await;
        let data = self.dir.join("data");
        let store = open_store(&data).await;
        let (manager, core) = boot(&data, store, &self.script, &self.specs, &self.behavior).await;
        self.dir.ledger = Some(manager.runtime.ledger().clone());
        self.manager = manager;
        self.core = core;
    }

    /// Starts another session on the same manager, its thread on `thread`: in the flow's
    /// project, or in a project of its own named `project` (on a fresh repository).
    pub async fn add_session(
        &self,
        title: &str,
        project: Option<&str>,
        thread: ProviderKind,
    ) -> ConversationId {
        let (project_id, repo) = match project {
            None => (
                self.core
                    .conversation(&self.conversation)
                    .unwrap()
                    .project_id
                    .unwrap(),
                self.repo.clone(),
            ),
            Some(name) => {
                let repo = self.dir.join(format!("repo-{name}"));
                std::fs::create_dir_all(&repo).unwrap();
                let repo = repo.canonicalize().unwrap();
                git(&repo, &["init", "-q", "-b", "main"]);
                std::fs::write(repo.join("README.md"), format!("# {name}\n")).unwrap();
                git(&repo, &["add", "-A"]);
                git(&repo, &["commit", "-q", "-m", "Start"]);
                let project = self
                    .core
                    .create_project(name.into(), Some(repo.display().to_string()))
                    .await
                    .unwrap();
                self.core
                    .set_folder_trust(&project.id, &repo.display().to_string(), true)
                    .await
                    .unwrap();
                (project.id, repo)
            }
        };
        self.manager
            .create_conversation(
                ConversationKind::Session,
                Some(project_id),
                Some(title.into()),
                Some(SetupRequest::Session {
                    repo: repo.display().to_string(),
                    environment: EnvironmentRequest::NewWorktree {
                        base: "main".into(),
                        branch: None,
                    },
                    permission: self.permission,
                    orchestrator: ModelChoice {
                        provider: thread,
                        model: Some(
                            match thread {
                                ProviderKind::Claude => "claude-opus-5-5",
                                ProviderKind::Codex => "gpt-6-astra",
                            }
                            .into(),
                        ),
                        effort: None,
                        fast: None,
                        account: None,
                    },
                    plan_mode: false,
                }),
            )
            .await
            .unwrap()
            .id
    }

    /// Sends the user's message.
    pub async fn say(&self, text: &str) {
        self.say_to(&self.conversation, text).await;
    }

    /// Sends the user's message to the session `id`.
    pub async fn say_to(&self, id: &ConversationId, text: &str) {
        self.manager
            .send_message(id.clone(), text.into(), Vec::new(), Vec::new(), false, None)
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
        settings.manual_accounts = if switch {
            Vec::new()
        } else {
            ProviderKind::ALL.to_vec()
        };
        self.core.update_settings(settings).await.unwrap();
        self.manager.runtime.sync_accounts().await;
        let deadline = tokio::time::Instant::now() + *PATIENCE;
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

    /// Notes `account`'s weekly window as resetting in `hours` (its 5-hour one barely used):
    /// new work starts on the account whose week ends first.
    pub fn weekly_resets_in(&self, provider: ProviderKind, account: Option<&str>, hours: i64) {
        use brigadier_providers::{QuotaSnapshot, QuotaSource, QuotaWindow};

        let now = crate::now_ms();
        let window = |id: &str, minutes: i64, resets_at_ms: i64| QuotaWindow {
            id: id.into(),
            label: id.into(),
            used_percent: 10.0,
            resets_at_ms: Some(resets_at_ms),
            window_minutes: Some(minutes),
            bucket: None,
            model: None,
        };
        self.manager.runtime.monitor().note(
            &crate::accounts::AccountRef::new(provider, account.map(Into::into)),
            &QuotaSnapshot {
                provider,
                windows: vec![
                    window("five_hour", 300, now + 3_600_000),
                    window("seven_day", 7 * 24 * 60, now + hours * 3_600_000),
                ],
                limit: None,
                observed_at_ms: now,
                source: QuotaSource::Read,
            },
            now,
        );
    }

    /// Every session the CLIs were started with, in order.
    pub fn specs(&self) -> Vec<(ProviderKind, SessionSpec)> {
        self.specs.lock().unwrap().clone()
    }

    /// The sessions the thread's CLIs were started with, in order.
    pub fn thread_specs(&self) -> Vec<(ProviderKind, SessionSpec)> {
        self.specs
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, spec)| {
                spec.tools == brigadier_providers::ToolSet::Thread
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
        self.board_of(&self.conversation).await
    }

    /// The board of the session `id`.
    /// The settled state the app reads for session `id` (ready, merged or reverted), if any.
    pub async fn settled_state(&self, id: &ConversationId) -> Option<crate::work::TabState> {
        self.manager
            .settled_tabs()
            .await
            .into_iter()
            .find(|tab| tab.conversation_id == *id)
            .map(|tab| tab.state)
    }

    pub async fn board_of(&self, id: &ConversationId) -> Board {
        self.core.board(id).await.unwrap()
    }

    /// Waits until `done` holds for the conversation's board.
    pub async fn until(&self, what: &str, done: impl Fn(&Board) -> bool) -> Board {
        self.until_in(&self.conversation, what, done).await
    }

    /// Waits until `done` holds for the board of the session `id`.
    pub async fn until_in(
        &self,
        id: &ConversationId,
        what: &str,
        done: impl Fn(&Board) -> bool,
    ) -> Board {
        let deadline = tokio::time::Instant::now() + *PATIENCE;
        loop {
            let board = self.board_of(id).await;
            if done(&board) {
                return board;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {what}; run {:?}, requests {:?}, previews {:?}; tasks: \
                 {:#?}",
                board.run,
                board
                    .requests
                    .values()
                    .map(|request| &request.state)
                    .collect::<Vec<_>>(),
                board
                    .previews
                    .values()
                    .map(|preview| (&preview.name, &preview.state))
                    .collect::<Vec<_>>(),
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
        self.settled_in(&self.conversation).await
    }

    /// Waits until every request of the session `id` is over, and the file snapshot each
    /// ended request takes once it is over has run: it runs git in the checkout after the
    /// request shows as ended.
    pub async fn settled_in(&self, id: &ConversationId) -> Board {
        self.until_in(id, "the requests to settle", |board| {
            !board.requests.is_empty()
                && board.requests.values().all(|request| match request.state {
                    RequestState::Working => false,
                    RequestState::Waiting => true,
                    _ => request
                        .changes
                        .as_ref()
                        .is_none_or(|changes| changes.settled),
                })
        })
        .await
    }

    /// Waits until the requests are over and the thread's turn too: a report's turn for a
    /// request that already ended shows nothing working on the board while it runs.
    pub async fn idle(&self) -> Board {
        let deadline = tokio::time::Instant::now() + *PATIENCE;
        loop {
            let board = self.settled().await;
            let conv = self.manager.conv(&self.conversation).unwrap();
            if conv.idle().await {
                return board;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for the thread's turn to end"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The conversation's stored events, oldest first.
    pub async fn events(&self) -> Vec<DomainEvent> {
        self.events_of(&self.conversation).await
    }

    /// The stored events of the session `id`, oldest first.
    pub async fn events_of(&self, id: &ConversationId) -> Vec<DomainEvent> {
        let stream = crate::model::streams::conversation(id);
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

// Windows runs the browser tool's flows only so far: the others use Unix shells, processes
// and permissions.
#[cfg(unix)]
mod accounts_tests;
#[cfg(unix)]
mod agent_tools_tests;
#[cfg(unix)]
mod answer_tests;
mod app_state_tests;
#[cfg(unix)]
mod auto_merge_tests;
#[cfg(unix)]
mod background_tests;
#[cfg(unix)]
mod branch_prefix_tests;
#[cfg(unix)]
mod brigade_tests;
mod browser_switch_tests;
mod browser_tool_tests;
#[cfg(unix)]
mod card_tests;
#[cfg(unix)]
mod chat_attachments_tests;
#[cfg(unix)]
mod chat_images_tests;
#[cfg(unix)]
mod chat_thread_tests;
#[cfg(unix)]
mod cli_moved_tools_tests;
#[cfg(unix)]
mod cli_tests;
mod cli_thread_tests;
#[cfg(all(test, target_os = "macos"))]
#[cfg(unix)]
mod computer_host_tests;
#[cfg(unix)]
mod decision_index_tests;
#[cfg(unix)]
mod engine_tests;
mod ledger_tests;
#[cfg(unix)]
mod open_in_app_tests;
#[cfg(unix)]
#[cfg(unix)]
mod records_tests;
#[cfg(unix)]
mod standing_tests;
#[cfg(unix)]
mod terminal_run_tests;
// Uses the debug fault injections (`manager::fault`), built in debug only, where tests run.
#[cfg(all(test, debug_assertions))]
#[cfg(unix)]
mod fault_tests;
#[cfg(unix)]
mod feedback_tests;
#[cfg(unix)]
mod fresh_base_tests;
#[cfg(unix)]
#[cfg(unix)]
mod git_writing_tests;
#[cfg(unix)]
mod honesty_tests;
#[cfg(unix)]
mod interrupted_turn_tests;
// Uses the debug fault injections (`manager::fault`), built in debug only, where tests run.
#[cfg(all(test, debug_assertions))]
#[cfg(unix)]
mod litter_tests;
#[cfg(unix)]
mod local_commit_tests;
#[cfg(unix)]
mod mcp_servers_tests;
#[cfg(unix)]
#[cfg(unix)]
mod mention_tests;
#[cfg(unix)]
mod merge_by_user_tests;
#[cfg(unix)]
mod merge_queue_tests;
#[cfg(unix)]
mod no_push_tests;
#[cfg(unix)]
#[cfg(unix)]
#[cfg(unix)]
mod page_tools_tests;
#[cfg(unix)]
#[cfg(unix)]
mod plain_merge_tests;
#[cfg(unix)]
#[cfg(unix)]
mod pr_card_tests;
#[cfg(unix)]
mod preview_tests;
#[cfg(unix)]
mod quota_resume_tests;
#[cfg(unix)]
mod rebirth_words_tests;
#[cfg(unix)]
mod source_branch_tests;
#[cfg(unix)]
mod turn_changes_tests;
// Uses the debug fault injections (`manager::fault`), built in debug only, where tests run.
#[cfg(all(test, debug_assertions))]
#[cfg(unix)]
mod same_session_tests;
#[cfg(unix)]
mod search_tests;
#[cfg(unix)]
mod spare_worktree_tests;
// Uses the debug fault injections (`manager::fault`), built in debug only, where tests run.
#[cfg(all(test, debug_assertions))]
#[cfg(unix)]
mod tab_chat_tests;
#[cfg(unix)]
mod tests;
#[cfg(unix)]
mod thread_items_tests;
// Uses the debug fault injections (`manager::fault`), built in debug only, where tests run.
#[cfg(unix)]
mod older_context_tests;
#[cfg(unix)]
mod project_sessions_tests;
#[cfg(unix)]
mod session_dir_tests;
#[cfg(unix)]
mod session_states_tests;
#[cfg(all(test, debug_assertions))]
#[cfg(unix)]
mod thread_overflow_tests;
#[cfg(unix)]
mod thread_scratch_tests;
#[cfg(unix)]
mod thread_tests;
#[cfg(unix)]
mod trim_tests;
#[cfg(unix)]
mod trust_tests;
#[cfg(unix)]
mod usage_tests;
#[cfg(unix)]
mod user_setup_tests;
#[cfg(unix)]
mod warm_session_tests;
#[cfg(unix)]
mod workspace_tests;
#[cfg(unix)]
mod worktree_copies_tests;

#[cfg(unix)]
mod archive_delete_tests;
