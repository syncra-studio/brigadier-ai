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
    /// Brigadier's instructions for the session (its system prompt addition).
    pub prompt: String,
    /// What the turn was started with.
    pub input: String,
    pub cwd: PathBuf,
    /// The session's turns before this one.
    pub earlier: u32,
    grant: String,
    host: Arc<SessionManager>,
}

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
            .contains("You are the orchestrator of a Brigadier session")
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
        let call = tool_call(name, args);
        ToolHost::call(&*self.host, &self.grant, call).await
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

/// The tool call `name` with `args`, as the MCP server would build it.
fn tool_call(name: &str, args: Value) -> ToolCall {
    fn arg<T: serde::de::DeserializeOwned>(name: &str, args: Value) -> T {
        serde_json::from_value(args).unwrap_or_else(|err| panic!("{name} arguments: {err}"))
    }
    use OrchestratorCall as O;
    use WorkerCall as W;
    match name {
        "delegate_task" => ToolCall::Orchestrator(O::DelegateTask(arg(name, args))),
        "message_worker" => ToolCall::Orchestrator(O::MessageWorker(arg(name, args))),
        "stop_worker" => ToolCall::Orchestrator(O::StopWorker(arg(name, args))),
        "ask_user" => ToolCall::Orchestrator(O::AskUser(arg(name, args))),
        "read_report" => ToolCall::Orchestrator(O::ReadReport(arg(name, args))),
        "query_brain" => ToolCall::Orchestrator(O::QueryBrain(arg(name, args))),
        "plan_phases" => ToolCall::Orchestrator(O::PlanPhases(arg(name, args))),
        "approve_outline" => ToolCall::Orchestrator(O::ApproveOutline(arg(name, args))),
        "request_approval" => ToolCall::Orchestrator(O::RequestApproval(arg(name, args))),
        "land_phase" => ToolCall::Orchestrator(O::LandPhase(arg(name, args))),
        "finish_session" => ToolCall::Orchestrator(O::FinishSession(arg(name, args))),
        "note_for_user" => ToolCall::Orchestrator(O::NoteForUser(arg(name, args))),
        "list_tasks" => ToolCall::Orchestrator(O::ListTasks),
        "ask_orchestrator" => ToolCall::Worker(W::AskOrchestrator(arg(name, args))),
        "submit_outline" => ToolCall::Worker(W::SubmitOutline(arg(name, args))),
        "submit_report" => ToolCall::Worker(W::SubmitReport(arg(name, args))),
        "request_review" => ToolCall::Worker(W::RequestReview(arg(name, args))),
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
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

// ----- the scripted CLIs ------------------------------------------------------------------

/// A scripted stand-in for one CLI.
struct FakeCli {
    kind: ProviderKind,
    script: Script,
    host: Arc<OnceLock<Weak<SessionManager>>>,
}

impl FakeCli {
    fn models(&self) -> Vec<ModelInfo> {
        let ids: &[(&str, &str)] = match self.kind {
            ProviderKind::Claude => &[("claude-opus-5-5", "Opus 5.5")],
            ProviderKind::Codex => &[("gpt-6.1-sol", "GPT-6.1-Sol")],
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
            let session = Arc::new(FakeSession {
                kind: self.kind,
                native_id,
                prompt: spec.append_system_prompt.clone().unwrap_or_default(),
                cwd: spec.cwd.clone(),
                grant,
                script: self.script.clone(),
                host: self.host.clone(),
                events: Mutex::new(Some(tx)),
                turns: std::sync::atomic::AtomicU32::new(0),
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
    grant: String,
    script: Script,
    host: Arc<OnceLock<Weak<SessionManager>>>,
    events: Mutex<Option<mpsc::Sender<ProviderEvent>>>,
    turns: std::sync::atomic::AtomicU32,
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
            let host = self
                .host
                .get()
                .and_then(Weak::upgrade)
                .expect("the manager is running");
            let earlier = self.turns.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let turn = Turn {
                provider: self.kind,
                prompt: self.prompt.clone(),
                input: input_text(&input),
                cwd: self.cwd.clone(),
                earlier,
                grant: self.grant.clone(),
                host,
            };
            let script = self.script.clone();
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
                let _ = tx
                    .send(ProviderEvent::TurnCompleted {
                        turn_id: None,
                        status: TurnStatus::Completed,
                        duration_ms: Some(1),
                        usage: None,
                    })
                    .await;
            });
            Ok(())
        })
    }

    fn steer(&self, input: TurnInput) -> BoxFuture<'_, brigadier_providers::Result<()>> {
        self.send(input)
    }

    fn interrupt(&self) -> BoxFuture<'_, brigadier_providers::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn answer(
        &self,
        _approval_id: String,
        _decision: ApprovalDecision,
    ) -> BoxFuture<'_, brigadier_providers::Result<()>> {
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

// ----- a scripted session -----------------------------------------------------------------

/// A session in a fresh data folder on a fresh repository, its CLIs scripted.
pub(crate) struct Flow {
    pub manager: Arc<SessionManager>,
    pub core: Arc<Core>,
    pub repo: PathBuf,
    pub conversation: ConversationId,
    dir: PathBuf,
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
}

impl Default for Options {
    fn default() -> Self {
        Self {
            permission: PermissionLevel::FullAccess,
            plan_mode: false,
            seed: None,
            store: None,
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
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(data.clone()),
        })
        .unwrap();
        let store = {
            let data = data.clone();
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
        };
        if let Some(seed) = options.seed {
            store.append(seed_events(seed)).await.unwrap();
        }
        let core = Core::load(store).await.unwrap();
        let spawner: Spawner = Arc::new(|task| {
            tokio::spawn(task);
        });
        let host: Arc<OnceLock<Weak<SessionManager>>> = Arc::default();
        let fake = |kind| -> Arc<dyn Provider> {
            Arc::new(FakeCli {
                kind,
                script: script.clone(),
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
                        provider: ProviderKind::Claude,
                        model: Some("claude-opus-5-5".into()),
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
        }
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
mod tests;
