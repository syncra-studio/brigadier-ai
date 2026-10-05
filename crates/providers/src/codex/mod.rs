//! Codex adapter: `codex app-server` over stdio JSON-RPC, one app-server per session.
//!
//! Sessions use `thread/*` and `turn/*` (steer and interrupt included). Approvals are
//! server requests Brigadier answers. A command Codex asks about runs outside its sandbox once
//! approved, so every command and file-change approval is an escalation:
//!
//! - Full access uses `never` with no sandbox: Codex never asks.
//! - Workspace and scoped access use `on-request`: commands run inside the sandbox without
//!   asking; Codex asks only to leave it. With [`SessionSpec::auto_review`] (Approve for me)
//!   Codex's own auto-reviewer answers those requests (`approvalsReviewer: auto_review`) and
//!   nothing reaches Brigadier; otherwise they go to the user.
//! - Read-only access uses `untrusted`: Codex asks before every command, and a read-only
//!   session (such as an orchestrator) has all of them declined, so nothing runs.
//!
//! The user's personal Codex setup stays out: plugins, apps, hooks, computer and browser use,
//! memories and `notify` are switched off per process, and the MCP servers from their config are
//! disabled per thread.
//!
//! Brigadier never makes Codex write to `~/.codex/config.toml`. Starting a thread with a
//! writable sandbox in a project the user never trusted makes Codex persist a trust entry for
//! the project (for a worktree: the user's main checkout), and no per-process override stops it;
//! so threads start with no sandbox of their own and every turn sets it ([`thread_sandbox`]).
//! The exception is a session that must not read some folders, started in a folder Brigadier
//! owns outside any repository: it gets a permission profile instead ([`permission_profile`]).
//! Should Codex still add an entry for the exact folder of a Brigadier-owned session
//! ([`SessionSpec::owned_cwd`]), it is recorded and removed with the session through Codex's
//! config API, only while it is still exactly `trusted`.
//!
//! See [`orchestrator_lockdown`] for what an orchestrator session can and cannot do.

pub mod parse;
#[allow(clippy::all, clippy::pedantic, dead_code, unused_imports)]
pub mod protocol;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_sandbox::Platform;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::cli::{CliEnv, parse_version, version_at_least};
use crate::events::Events;
use crate::model::*;
use crate::process::{self, CliProcess};
use crate::record::{self, Direction, Recorder};
use crate::{
    BoxFuture, Error, Ledger, Provider, ProviderSession, Replayer, Result, Started, now_ms,
};
use parse::{Control, Output, Parser, PendingKind};
use protocol as p;

const EVENTS: usize = 512;
const STATUS_TIMEOUT: Duration = Duration::from_secs(20);
const START_TIMEOUT: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const EXIT_GRACE: Duration = Duration::from_secs(3);
/// `model_auto_compact_token_limit` for a session that must not compact: far above any window,
/// so Codex uses its own ceiling.
const NO_AUTO_COMPACT_TOKENS: i64 = 1_000_000_000;
/// The first version seen to serve `thread/compact/start`.
const COMPACT_SINCE: &str = "0.156.1";
/// The permission profile a session with folders it must not read runs under.
const PROFILE: &str = "brigadier";

/// Built-ins switched off for sessions that must not act on their own (the orchestrator, a
/// Chat): viewing local images, generating images, Codex's own sub-agents, goals, the sleep
/// tool, and every shell tool. Without a shell tool there is no command to approve at all,
/// so neither an exec-policy rule of the user's nor a sandbox gap can let one run.
const RESTRICTED_FEATURES: &[&str] = &[
    "view_image",
    "image_generation",
    "multi_agent",
    "multi_agent_v2",
    "goals",
    "sleep_tool",
    "shell_tool",
    "unified_exec",
];

/// Per-process overrides for the same sessions. The sub-agent tools (`collaboration.*`) come
/// with the model, whatever the feature flags say; only `agents.enabled` removes them. The
/// user's personal skills catalog stays out of their context.
const RESTRICTED_OVERRIDES: &[&str] = &[NO_SUB_AGENTS, "skills.include_instructions=false"];

/// Removes Codex's sub-agent tools. Codex can't hold a sub-agent to a set of models, so a
/// session whose models are limited ([`SessionSpec::allowed_models`]) runs without them.
const NO_SUB_AGENTS: &str = "agents.enabled=false";

/// Features that bring the user's personal Codex setup (or desktop integrations) into a
/// session.
const DISABLED_FEATURES: &[&str] = &[
    "plugins",
    "apps",
    "hooks",
    "computer_use",
    "browser_use",
    "memories",
];

/// What a Codex orchestrator session (read-only access, `ToolSet::None`, Brigadier's MCP server
/// trusted, every approval declined) still has, as verified against Codex 0.156.1 by capturing
/// the model request and by adversarial live turns, and rechecked live on 0.158.0. The shell
/// tools, sub-agents, image viewing, image generation, web search, goals and the sleep tool are
/// gone.
pub const ORCHESTRATOR_RESIDUE: &str = "Codex orchestrators keep these built-ins: `exec` \
    (JavaScript in an isolate with no file system, network or console, which only calls the \
    tools below), `apply_patch` (every patch is an approval request, and Brigadier declines it), \
    the MCP resource tools (list/read resources of Brigadier's server, which serves none), \
    `clock__curr_time`, and `request_user_input` (Brigadier refuses the request). There is no \
    shell, sub-agent, image, web search or goal tool.";

/// Whether a Codex orchestrator is locked down: nothing can run, write or reach the network
/// without an approval that Brigadier declines, and no sub-agent can act for it. `Err` carries
/// the reason to show before falling back to a Claude orchestrator.
///
/// Verified for Codex 0.156.1 and rechecked live on 0.158.0 (the version these bindings come
/// from at the time; version drift is logged for diagnostics): a read-only thread
/// with `untrusted` approvals and the restricted feature set exposes no command tool at all, so
/// no exec-policy rule or sandbox gap can let a command run; `apply_patch` asks and is
/// declined, leaving no file; the Brigadier MCP tools run without an elicitation. What remains
/// is [`ORCHESTRATOR_RESIDUE`].
pub fn orchestrator_lockdown() -> std::result::Result<(), String> {
    Ok(())
}

pub struct Codex {
    platform: Arc<dyn Platform>,
    env: Arc<CliEnv>,
    binary: Option<PathBuf>,
}

impl Codex {
    pub fn new(platform: Arc<dyn Platform>, env: Arc<CliEnv>) -> Self {
        let binary = env.resolve(ProviderKind::Codex);
        Self {
            platform,
            env,
            binary,
        }
    }

    /// Archives threads no Brigadier session has open (see [`archive_thread`]), except those
    /// `open` says a session has opened meanwhile.
    pub async fn archive_threads(
        &self,
        thread_ids: Vec<String>,
        open: impl Fn(&str) -> bool + Send + Sync,
    ) -> Result<()> {
        self.control(async |rpc: &Rpc| {
            for thread_id in thread_ids.iter().filter(|id| !open(id)) {
                archive_thread(rpc, thread_id).await?;
            }
            Ok(())
        })
        .await
    }

    fn binary(&self) -> Result<&Path> {
        self.binary
            .as_deref()
            .ok_or(Error::NotInstalled(ProviderKind::Codex))
    }

    /// Where Codex saves generated images, one folder per thread: `$CODEX_HOME` (by default
    /// `~/.codex`) `/generated_images`.
    fn images_root(&self) -> Option<PathBuf> {
        Some(self.codex_home()?.join("generated_images"))
    }

    /// `$CODEX_HOME`, by default `~/.codex`.
    fn codex_home(&self) -> Option<PathBuf> {
        match self.env.var("CODEX_HOME") {
            Some(home) if !home.is_empty() => Some(PathBuf::from(home)),
            _ => Some(self.env.home()?.join(".codex")),
        }
    }

    /// The model and effort `codex` runs with when not told, from `config.toml`.
    async fn configured(&self) -> Configured {
        let Some(path) = self.codex_home().map(|home| home.join("config.toml")) else {
            return Configured::default();
        };
        tokio::fs::read_to_string(path)
            .await
            .map(|text| Configured::parse(&text))
            .unwrap_or_default()
    }

    async fn version(&self) -> Option<String> {
        let spec = self.env.spec(self.binary().ok()?).arg("--version");
        let output = process::run(&self.platform, &spec, STATUS_TIMEOUT)
            .await
            .ok()?;
        parse_version(&output.stdout)
    }

    /// Spawns an app-server for `cwd` with the user's personal setup switched off, and
    /// initializes it.
    async fn app_server(
        &self,
        cwd: &Path,
        session: Option<&SessionSpec>,
        recorder: Option<Arc<Recorder>>,
        ledger: Option<Arc<dyn Ledger>>,
    ) -> Result<(Arc<Rpc>, mpsc::Receiver<String>)> {
        let mut spec = self.env.spec(self.binary()?);
        spec.args = app_server_args(session)
            .into_iter()
            .map(Into::into)
            .collect();
        spec.cwd = Some(cwd.to_owned());
        if let Some(session) = session {
            crate::cli::apply_session_env(&mut spec, &session.env, &session.unset_env);
            spec.low_priority = session.low_priority;
        }
        let redactor = session.and_then(|session| session.redactor.clone());
        let process::Spawned { process, stdout } = process::spawn(
            self.platform.clone(),
            &spec,
            process::Options {
                recorder,
                redactor,
                ledger,
                owned_dir: session
                    .filter(|session| session.owned_cwd)
                    .map(|_| cwd.to_owned()),
            },
        )?;
        Ok((Arc::new(Rpc::new(process)), stdout))
    }

    /// Runs `work` against a throwaway app-server, then shuts it down.
    async fn control<T>(&self, work: impl AsyncFnOnce(&Rpc) -> Result<T>) -> Result<T> {
        let cwd = self.platform.paths().data_dir.clone();
        let (rpc, stdout) = self.app_server(&cwd, None, None, None).await?;
        let reader = tokio::spawn(control_reader(rpc.clone(), stdout));
        let result = async {
            rpc.initialize().await?;
            work(&rpc).await
        }
        .await;
        rpc.process.shutdown(EXIT_GRACE).await;
        reader.abort();
        result
    }
}

/// `codex app-server`'s arguments for a session (`None`: a control connection).
fn app_server_args(session: Option<&SessionSpec>) -> Vec<String> {
    let mut args: Vec<String> = vec!["app-server".into()];
    let tools = session.map(|session| session.tools).unwrap_or_default();
    let role_features: &[&str] = match tools {
        ToolSet::Default | ToolSet::Lean => &[],
        ToolSet::None | ToolSet::Web => RESTRICTED_FEATURES,
    };
    for feature in DISABLED_FEATURES.iter().chain(role_features) {
        args.push("--disable".into());
        args.push((*feature).into());
    }
    if !role_features.is_empty() {
        for value in RESTRICTED_OVERRIDES {
            args.push("-c".into());
            args.push((*value).into());
        }
    } else if session.is_some_and(|session| session.allowed_models.is_some()) {
        // `spawn_agent` takes any model Codex offers, one Brigadier excluded included (checked
        // live on 0.159.2), so a worker has no sub-agents.
        args.push("-c".into());
        args.push(NO_SUB_AGENTS.into());
    }
    args.push("-c".into());
    args.push("notify=[]".into());
    // Commands run in a plain (non-login) shell. A login shell, and the login environment
    // Codex snapshots from one, would rebuild PATH and drop the command gate's shims.
    args.push("-c".into());
    args.push("allow_login_shell=false".into());
    args
}

/// Serves a control app-server's output: only responses matter.
async fn control_reader(rpc: Arc<Rpc>, mut stdout: mpsc::Receiver<String>) {
    let mut parser = Parser::live();
    while let Some(line) = stdout.recv().await {
        for output in parser.feed(&line) {
            match output {
                Output::Control(Control::Response { id, result }) => rpc.resolve(id, result),
                Output::Control(
                    Control::Approval { rpc_id, .. }
                    | Control::Elicitation { rpc_id, .. }
                    | Control::Unsupported { rpc_id, .. },
                ) => {
                    rpc.reject(rpc_id, "not served by a control connection")
                        .await;
                }
                Output::Event(_) => {}
            }
        }
    }
    rpc.fail_all();
}

impl Provider for Codex {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Codex
    }

    fn status(&self) -> BoxFuture<'_, ProviderStatus> {
        Box::pin(async move {
            let mut status = ProviderStatus {
                provider: ProviderKind::Codex,
                path: self.binary.as_ref().map(|path| path.display().to_string()),
                version: None,
                logged_in: false,
                auth_method: None,
                plan: None,
                guidance: None,
                compacts: false,
            };
            let Ok(binary) = self.binary() else {
                status.guidance = Some(
                    "Install Codex (https://developers.openai.com/codex) so `codex` is on your \
                     login shell's PATH, then refresh."
                        .into(),
                );
                return status;
            };
            status.version = self.version().await;
            status.compacts = status
                .version
                .as_deref()
                .is_some_and(|version| version_at_least(version, COMPACT_SINCE));
            let spec = self.env.spec(binary).arg("login").arg("status");
            match process::run(&self.platform, &spec, STATUS_TIMEOUT).await {
                Ok(output) => {
                    // `codex login status` prints to stderr on some versions.
                    let text = format!("{}\n{}", output.stdout, output.stderr);
                    let line = text
                        .lines()
                        .map(str::trim)
                        .find(|line| line.starts_with("Logged in"));
                    status.logged_in = output.code == Some(0) && line.is_some();
                    status.auth_method = line
                        .and_then(|line| line.split_once(" using "))
                        .map(|(_, method)| method.trim().to_owned());
                }
                Err(err) => status.guidance = Some(format!("Could not ask Codex: {err}")),
            }
            if !status.logged_in && status.guidance.is_none() {
                status.guidance =
                    Some("Log in to Codex: run `codex login` in a terminal, then refresh.".into());
            }
            status
        })
    }

    fn models(&self) -> BoxFuture<'_, Result<ModelCatalog>> {
        Box::pin(async move {
            let version = self.version().await;
            let models = self
                .control(async |rpc: &Rpc| {
                    let mut models = Vec::new();
                    let mut cursor = None;
                    loop {
                        let page: p::ModelListResponse = rpc
                            .call(
                                "model/list",
                                &p::ModelListParams {
                                    cursor: cursor.take(),
                                    ..Default::default()
                                },
                            )
                            .await?;
                        models.extend(page.data);
                        match page.next_cursor {
                            Some(next) if !next.is_empty() => cursor = Some(next),
                            _ => break Ok(models),
                        }
                    }
                })
                .await?;
            let mut models: Vec<ModelInfo> = models
                .into_iter()
                .filter(|model| !model.hidden)
                .map(model_info)
                .collect();
            mark_legacy(&mut models);
            self.configured().await.apply(&mut models);
            Ok(ModelCatalog {
                provider: ProviderKind::Codex,
                models,
                cli_version: version,
                fetched_at_ms: now_ms(),
            })
        })
    }

    fn quota(&self) -> BoxFuture<'_, Result<QuotaSnapshot>> {
        Box::pin(async move {
            let limits: p::GetAccountRateLimitsResponse = self
                .control(async |rpc: &Rpc| rpc.call("account/rateLimits/read", &Value::Null).await)
                .await?;
            Ok(parse::quota_read(&limits))
        })
    }

    fn start(&self, spec: SessionSpec, ledger: Arc<dyn Ledger>) -> BoxFuture<'_, Result<Started>> {
        Box::pin(async move {
            // Codex resolves its working directory (below) and so the paths it writes: its
            // writable roots must be resolved too, in the thread's config and in every turn's
            // sandbox policy alike, or it asks to write inside them.
            let spec = SessionSpec {
                access: spec.access.resolved(),
                ..spec
            };
            if let Access::Scoped {
                write_cwd: false, ..
            } = spec.access
            {
                return Err(Error::Invalid(
                    "Codex cannot keep its working directory read-only in a workspace sandbox; \
                     start a read-only Codex worker in its scratch folder"
                        .into(),
                ));
            }
            let cwd = spec.cwd.canonicalize().map_err(|err| {
                Error::Invalid(format!("working directory {}: {err}", spec.cwd.display()))
            })?;
            let version = self.version().await;
            let recorder = match &spec.record_to {
                Some(path) => Some(Arc::new(Recorder::create(
                    path,
                    &record::Header {
                        fixture: record::FORMAT,
                        provider: ProviderKind::Codex,
                        cli_version: version.clone(),
                        recorded_at_ms: now_ms(),
                        title: "Codex session".into(),
                    },
                    spec.redactor.clone(),
                )?)),
                None => None,
            };
            let (rpc, stdout) = self
                .app_server(&cwd, Some(&spec), recorder, Some(ledger.clone()))
                .await?;
            ledger
                .record(Artifact::Process {
                    pid: rpc.process.pid(),
                    started_at_ms: rpc.process.started_at_ms(),
                })
                .await?;

            let (events_tx, events) = mpsc::channel(EVENTS);
            let shared = Arc::new(Shared {
                approvals: Mutex::new(HashMap::new()),
                turn_id: Mutex::new(None),
                compact_asked: AtomicBool::new(false),
                events: Events::new(events_tx, spec.redactor.clone()),
            });
            let trusted: HashSet<String> = spec
                .mcp_servers
                .iter()
                .filter(|server| server.trusted)
                .map(|server| server.name.clone())
                .collect();
            tokio::spawn(read_loop(rpc.clone(), stdout, shared.clone(), trusted));

            let profile = permission_profile(&spec, &cwd);
            let profiled = profile.is_some();
            let started = tokio::time::timeout(
                START_TIMEOUT,
                open_thread(
                    &rpc,
                    &spec,
                    &cwd,
                    profile,
                    self.images_root().as_deref(),
                    ledger.as_ref(),
                ),
            )
            .await
            .map_err(|_| Error::Timeout("Codex to start the thread"))
            .and_then(|result| result);
            let Opened {
                thread,
                model,
                notices,
            } = match started {
                Ok(started) => started,
                Err(err) => {
                    rpc.process.shutdown(EXIT_GRACE).await;
                    return Err(err);
                }
            };

            let session = Arc::new(CodexSession {
                thread_id: thread.id.clone(),
                compacts: version_at_least(&thread.cli_version, COMPACT_SINCE),
                rpc,
                shared,
                access: spec.access.clone(),
                auto_review: spec.auto_review,
                profiled,
                effort: spec.effort.clone(),
            });
            session
                .emit(ProviderEvent::SessionStarted {
                    native_id: thread.id,
                    model: Some(model),
                    cwd: Some(thread.cwd.to_string()),
                    cli_version: Some(thread.cli_version.clone()),
                })
                .await;
            for message in notices {
                session
                    .emit(ProviderEvent::Notice {
                        level: NoticeLevel::Warning,
                        message,
                    })
                    .await;
            }
            if thread.cli_version != p::SCHEMA_VERSION {
                tracing::warn!(
                    cli_version = %thread.cli_version,
                    schema_version = p::SCHEMA_VERSION,
                    "Codex version differs from the generated bindings; unknown fields are ignored"
                );
            }
            Ok(Started { session, events })
        })
    }

    fn remove(&self, artifacts: Vec<Artifact>) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let mut threads = Vec::new();
            let mut trusts = Vec::new();
            let mut images = Vec::new();
            for artifact in artifacts {
                match artifact {
                    Artifact::CodexThread { thread_id } => threads.push(thread_id),
                    Artifact::CodexProjectTrust { path } => trusts.push(path),
                    Artifact::CodexGeneratedImages { path } => images.push(PathBuf::from(path)),
                    _ => {}
                }
            }
            if !images.is_empty() {
                let root = self.images_root();
                tokio::task::spawn_blocking(move || {
                    images
                        .iter()
                        .try_for_each(|dir| remove_images_dir(root.as_deref(), dir))
                })
                .await
                .map_err(|err| Error::Invalid(err.to_string()))??;
            }
            if threads.is_empty() && trusts.is_empty() {
                return Ok(());
            }
            self.control(async |rpc: &Rpc| {
                for path in trusts {
                    remove_project_trust(rpc, &path).await?;
                }
                for thread_id in threads {
                    let deleted: Result<Value> = rpc
                        .call(
                            "thread/delete",
                            &p::ThreadDeleteParams {
                                thread_id: thread_id.clone(),
                            },
                        )
                        .await;
                    match deleted {
                        Ok(_) => {}
                        // Already gone: never persisted (no turn ran), or deleted before.
                        Err(Error::Rejected(message))
                            if message.contains("not found")
                                || message.contains("no rollout found") => {}
                        Err(err) => return Err(err),
                    }
                }
                Ok(())
            })
            .await
        })
    }

    fn replayer(&self) -> Box<dyn Replayer> {
        Box::new(CodexReplayer {
            parser: Parser::replay(),
            asked: HashSet::new(),
        })
    }

    fn past_folders(&self) -> BoxFuture<'_, Vec<crate::history::PastFolder>> {
        let dir = self.codex_home();
        Box::pin(async move {
            let Some(dir) = dir else {
                return Vec::new();
            };
            tokio::task::spawn_blocking(move || crate::history::codex(&dir))
                .await
                .unwrap_or_default()
        })
    }
}

/// A started thread, its model, and warnings for the user.
struct Opened {
    thread: p::Thread,
    model: String,
    notices: Vec<String>,
}

/// Starts, resumes or forks the session's thread, recording it (and the folder its generated
/// images would go to, under `images_root`) in the ledger.
async fn open_thread(
    rpc: &Rpc,
    spec: &SessionSpec,
    cwd: &Path,
    profile: Option<Value>,
    images_root: Option<&Path>,
    ledger: &dyn Ledger,
) -> Result<Opened> {
    rpc.initialize().await?;
    let profiled = profile.is_some();
    let config = thread_config(rpc, spec, cwd, profile).await?;
    // Only a folder Brigadier created may have its trust entry recorded and undone. Codex keys
    // trust on the main checkout for a worktree, which is the user's: watched, never touched.
    let watched: Vec<String> = if spec.owned_cwd {
        let mut watched = vec![cwd.display().to_string()];
        let root = trust_root(cwd).display().to_string();
        if !watched.contains(&root) {
            watched.push(root);
        }
        watched
    } else {
        Vec::new()
    };
    let trusted_before = if watched.is_empty() {
        Map::new()
    } else {
        trusted_projects(rpc, cwd).await?
    };
    let cwd_text = Some(cwd.display().to_string());
    let sandbox = thread_sandbox(&spec.access);
    let approval = Some(approval_policy(&spec.access));
    let reviewer = Some(reviewer(spec.auto_review));
    let instructions = spec.append_system_prompt.clone();
    // Always named: an omitted tier inherits the resumed thread's or the user's config.
    let service_tier = Some(if spec.fast { FAST_TIER } else { STANDARD_TIER }.to_owned());
    let (thread, model) = match &spec.origin {
        Origin::New => {
            let started: Profiled<p::ThreadStartResponse> = rpc
                .call(
                    "thread/start",
                    &p::ThreadStartParams {
                        cwd: cwd_text,
                        model: spec.model.clone(),
                        sandbox,
                        approval_policy: approval,
                        approvals_reviewer: reviewer,
                        developer_instructions: instructions,
                        config: Some(config),
                        service_tier: service_tier.clone(),
                        ..Default::default()
                    },
                )
                .await?;
            let started = started.checked(profiled)?;
            (started.thread, started.model)
        }
        Origin::Resume { native_id } => {
            // Brigadier archived it when it last closed the thread.
            unarchive_thread(rpc, native_id).await?;
            let resumed: Profiled<p::ThreadResumeResponse> = rpc
                .call(
                    "thread/resume",
                    &p::ThreadResumeParams {
                        thread_id: native_id.clone(),
                        cwd: cwd_text,
                        model: spec.model.clone(),
                        sandbox,
                        approval_policy: approval,
                        approvals_reviewer: reviewer,
                        developer_instructions: instructions,
                        config: Some(config),
                        exclude_turns: Some(true),
                        service_tier: service_tier.clone(),
                        ..Default::default()
                    },
                )
                .await?;
            let resumed = resumed.checked(profiled)?;
            (resumed.thread, resumed.model)
        }
        Origin::Fork { native_id } => {
            let archived = unarchive_thread(rpc, native_id).await?;
            let forked: Profiled<p::ThreadForkResponse> = rpc
                .call(
                    "thread/fork",
                    &p::ThreadForkParams {
                        thread_id: native_id.clone(),
                        cwd: cwd_text,
                        model: spec.model.clone(),
                        sandbox,
                        approval_policy: approval,
                        approvals_reviewer: reviewer,
                        developer_instructions: instructions,
                        config: Some(config),
                        exclude_turns: Some(true),
                        service_tier: service_tier.clone(),
                        ..Default::default()
                    },
                )
                .await?;
            if archived {
                archive_thread(rpc, native_id).await?;
            }
            let forked = forked.checked(profiled)?;
            (forked.thread, forked.model)
        }
    };
    ledger
        .record(Artifact::CodexThread {
            thread_id: thread.id.clone(),
        })
        .await?;
    if let Some(root) = images_root {
        ledger
            .record(Artifact::CodexGeneratedImages {
                path: root.join(&thread.id).display().to_string(),
            })
            .await?;
    }
    let mut notices = Vec::new();
    if !watched.is_empty() {
        let trusted_after = trusted_projects(rpc, cwd).await?;
        for path in watched {
            if trusted_before.contains_key(&path) || trusted_after.get(&path) != Some(&trusted()) {
                continue;
            }
            if path == cwd.display().to_string() {
                ledger.record(Artifact::CodexProjectTrust { path }).await?;
            } else {
                notices.push(format!(
                    "Codex marked {path} as trusted in ~/.codex/config.toml. That folder is \
                     not Brigadier's, so the entry stays; remove it there if you did not want it."
                ));
            }
        }
    }
    Ok(Opened {
        thread,
        model,
        notices,
    })
}

/// The trust entry Codex writes, exactly.
fn trusted() -> Value {
    json!({ "trust_level": "trusted" })
}

/// The `projects` table of the user's own `config.toml`, by project path.
/// Archives a thread Brigadier is done with for now, so the Codex and ChatGPT apps don't list
/// it among the user's own threads (their Recents). The app-server offers no unlisted threads
/// that can still be resumed: every thread it starts is recorded as a `vscode` one. A thread
/// with no turn yet has nothing to archive.
async fn archive_thread(rpc: &Rpc, thread_id: &str) -> Result<()> {
    let archived: Result<Value> = rpc
        .call(
            "thread/archive",
            &p::ThreadArchiveParams {
                thread_id: thread_id.to_owned(),
            },
        )
        .await;
    match archived {
        Ok(_) => Ok(()),
        Err(Error::Rejected(message)) if message.contains("no rollout found") => Ok(()),
        Err(err) => Err(err),
    }
}

/// Brings back a thread [`archive_thread`] archived, since an archived thread can't be resumed
/// or forked. `false` when it wasn't archived.
async fn unarchive_thread(rpc: &Rpc, thread_id: &str) -> Result<bool> {
    let unarchived: Result<Value> = rpc
        .call(
            "thread/unarchive",
            &p::ThreadUnarchiveParams {
                thread_id: thread_id.to_owned(),
            },
        )
        .await;
    match unarchived {
        Ok(_) => Ok(true),
        Err(Error::Rejected(message)) if message.contains("no archived rollout found") => Ok(false),
        Err(err) => Err(err),
    }
}

async fn trusted_projects(rpc: &Rpc, cwd: &Path) -> Result<Map<String, Value>> {
    let read: Value = rpc
        .call(
            "config/read",
            &json!({ "includeLayers": true, "cwd": cwd.display().to_string() }),
        )
        .await?;
    let projects = read
        .get("layers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|layer| layer.pointer("/name/type").and_then(Value::as_str) == Some("user"))
        .and_then(|layer| layer.pointer("/config/projects"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    Ok(projects)
}

/// Removes a thread's generated-images folder, which must be one folder directly in Codex's
/// `generated_images` (a thread id, never anything else).
fn remove_images_dir(root: Option<&Path>, dir: &Path) -> Result<()> {
    let ours = root.is_some_and(|root| dir.parent() == Some(root))
        && dir
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                !name.is_empty() && name.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
            });
    if !ours {
        return Err(Error::Invalid(format!(
            "{} is not a Codex thread's generated-images folder",
            dir.display()
        )));
    }
    match std::fs::remove_dir_all(dir) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(Error::Io(
            std::io::Error::new(err.kind(), format!("{}: {err}", dir.display())),
        )),
        _ => Ok(()),
    }
}

/// Removes a trust entry a thread added, unless it changed since (the user decided on it).
async fn remove_project_trust(rpc: &Rpc, path: &str) -> Result<()> {
    let projects = trusted_projects(rpc, Path::new(path)).await?;
    if projects.get(path) != Some(&trusted()) {
        return Ok(());
    }
    let _: Value = rpc
        .call(
            "config/value/write",
            &json!({
                "keyPath": format!("projects.{}", toml_string(path)),
                "value": null,
                "mergeStrategy": "replace",
            }),
        )
        .await?;
    Ok(())
}

/// Per-thread config: the user's MCP servers off, Brigadier's on, and the workspace sandbox
/// with network access, or the session's permission profile.
async fn thread_config(
    rpc: &Rpc,
    spec: &SessionSpec,
    cwd: &Path,
    profile: Option<Value>,
) -> Result<Map<String, Value>> {
    let effective: Value = rpc
        .call(
            "config/read",
            &json!({ "includeLayers": false, "cwd": cwd.display().to_string() }),
        )
        .await?;
    let mut servers: Map<String, Value> = effective
        .pointer("/config/mcp_servers")
        .and_then(Value::as_object)
        .map(|servers| {
            servers
                .keys()
                .map(|name| (name.clone(), json!({ "enabled": false })))
                .collect()
        })
        .unwrap_or_default();
    for server in &spec.mcp_servers {
        let env: Map<String, Value> = server
            .env
            .iter()
            .map(|(name, value)| (name.clone(), Value::String(value.clone())))
            .collect();
        let mut config = json!({
            "command": server.command.display().to_string(),
            "args": server.args,
            "env": env,
        });
        if let Some(secs) = server.tool_timeout_secs {
            config["tool_timeout_sec"] = json!(secs);
        }
        if server.trusted {
            config["default_tools_approval_mode"] = json!("approve");
        }
        servers.insert(server.name.clone(), config);
    }

    let mut config = Map::new();
    config.insert("mcp_servers".into(), Value::Object(servers));
    if !spec.auto_compact {
        // Codex clamps the limit to 90% of the model's window.
        config.insert(
            "model_auto_compact_token_limit".into(),
            json!(NO_AUTO_COMPACT_TOKENS),
        );
    }
    match spec.tools {
        ToolSet::Default | ToolSet::Lean => {}
        ToolSet::None => {
            config.insert("web_search".into(), json!("disabled"));
        }
        ToolSet::Web => {
            config.insert("web_search".into(), json!("live"));
        }
    }
    if let Some(profile) = profile {
        // Profiles and the legacy sandbox settings don't compose: set only the profile.
        config.insert("default_permissions".into(), json!(PROFILE));
        config.insert("permissions".into(), json!({ PROFILE: profile }));
        return Ok(config);
    }
    let network = match &spec.access {
        Access::Workspace { .. } => Some(true),
        Access::Scoped { network, .. } => Some(*network),
        Access::ReadOnly | Access::Full => None,
    };
    if let Some(network) = network {
        let scoped = matches!(spec.access, Access::Scoped { .. });
        config.insert(
            "sandbox_workspace_write".into(),
            json!({
                "network_access": network,
                "writable_roots": spec
                    .access
                    .writable_roots()
                    .iter()
                    .map(|root| root.display().to_string())
                    .collect::<Vec<_>>(),
                "exclude_slash_tmp": scoped,
                "exclude_tmpdir_env_var": scoped,
            }),
        );
    }
    Ok(config)
}

/// The permission profile of a scoped session with folders it must not read, when Codex can
/// take one ([`can_deny_reads`]).
///
/// Codex's legacy sandbox settings cannot deny reads. A permission profile can, and the OS
/// sandbox enforces it: a denied read fails with "Operation not permitted" (verified against
/// Codex 0.156.1 on macOS). Profiles are set when the thread opens; turns then set no sandbox
/// policy of their own, which would bring back the legacy settings.
fn permission_profile(spec: &SessionSpec, cwd: &Path) -> Option<Value> {
    let Access::Scoped {
        writable_roots,
        network,
        deny_read,
        unix_sockets,
        ..
    } = &spec.access
    else {
        return None;
    };
    if deny_read.is_empty() || !can_deny_reads(spec.owned_cwd, cwd) {
        return None;
    }
    let real = |path: &PathBuf| {
        path.canonicalize()
            .unwrap_or_else(|_| path.clone())
            .display()
            .to_string()
    };
    // Everything readable, the working directory (always writable for Codex) and the roots
    // writable, `/tmp` and `$TMPDIR` read-only as in the legacy scoped sandbox.
    let mut filesystem = Map::new();
    filesystem.insert(":root".into(), json!("read"));
    filesystem.insert(":workspace_roots".into(), json!("write"));
    for root in writable_roots {
        filesystem.insert(real(root), json!("write"));
    }
    for path in deny_read {
        filesystem.insert(real(path), json!("deny"));
    }
    let sockets: Map<String, Value> = unix_sockets
        .iter()
        .map(|socket| (real(socket), json!("allow")))
        .collect();
    Some(json!({
        "filesystem": filesystem,
        "network": { "enabled": network, "unix_sockets": sockets },
    }))
}

/// Whether a Codex session started in `cwd` can be kept from reading folders
/// ([`Access::Scoped`]'s `deny_read`): only in a folder Brigadier owns outside any repository.
/// A thread opened with a writable permission profile makes Codex persist a trust entry in
/// `~/.codex/config.toml` for the repository it runs in (for a worktree: the user's main
/// checkout), so a worker in a worktree keeps the legacy sandbox, which cannot deny reads.
pub fn can_deny_reads(owned_cwd: bool, cwd: &Path) -> bool {
    owned_cwd && !cwd.ancestors().any(|dir| dir.join(".git").exists())
}

/// A thread response with the permission profile Codex reports active (a field its schema
/// doesn't declare yet).
#[derive(Deserialize)]
struct Profiled<R> {
    #[serde(flatten)]
    response: R,
    #[serde(rename = "activePermissionProfile", default)]
    active_profile: Option<ActiveProfile>,
}

#[derive(Deserialize)]
struct ActiveProfile {
    id: String,
}

impl<R> Profiled<R> {
    /// Fails closed when the session asked for its profile and Codex didn't apply it (an older
    /// Codex, or a `sandbox_mode` in the user's config, which turns profiles off): nothing
    /// would keep it out of the folders it must not read.
    fn checked(self, profiled: bool) -> Result<R> {
        let active = self.active_profile.map(|profile| profile.id);
        if profiled && active.as_deref() != Some(PROFILE) {
            return Err(Error::Invalid(format!(
                "Codex did not apply Brigadier's permission profile (active: {}), so it cannot \
                 keep this session out of Brigadier's private folders",
                active.as_deref().unwrap_or("none")
            )));
        }
        Ok(self.response)
    }
}

/// When Codex asks: never under full access, to leave the sandbox otherwise, before every
/// command when read-only.
fn approval_policy(access: &Access) -> p::AskForApproval {
    match access {
        Access::ReadOnly => p::AskForApproval::Untrusted,
        Access::Full => p::AskForApproval::Never,
        Access::Workspace { .. } | Access::Scoped { .. } => p::AskForApproval::OnRequest,
    }
}

/// Who answers Codex's approval requests: its own auto-reviewer, or Brigadier (and through it
/// the user). Always named, so a reviewer set in the user's own config never applies.
fn reviewer(auto_review: bool) -> p::ApprovalsReviewer {
    if auto_review {
        p::ApprovalsReviewer::AutoReview
    } else {
        p::ApprovalsReviewer::User
    }
}

/// The sandbox a thread starts with. Only read-only is set here: starting a thread with a
/// writable sandbox in a project the user never trusted makes Codex persist a trust entry for
/// that project in `~/.codex/config.toml` (for a worktree, the user's main checkout), which a
/// `-c projects.….trust_level` override does not prevent. Every turn sets the full sandbox
/// policy instead ([`sandbox_policy`]), which holds for that turn and the ones after it; nothing
/// runs in a thread outside a turn.
fn thread_sandbox(access: &Access) -> Option<p::SandboxMode> {
    match access {
        Access::ReadOnly => Some(p::SandboxMode::ReadOnly),
        Access::Workspace { .. } | Access::Scoped { .. } | Access::Full => None,
    }
}

fn sandbox_policy(access: &Access) -> p::SandboxPolicy {
    match access {
        Access::Workspace { extra_roots } => p::SandboxPolicy::WorkspaceWrite {
            exclude_slash_tmp: false,
            exclude_tmpdir_env_var: false,
            network_access: true,
            writable_roots: extra_roots
                .iter()
                .map(|root| p::AbsolutePathBuf(root.display().to_string()))
                .collect(),
        },
        // Exactly the working directory and the roots: `/tmp` and `$TMPDIR` stay read-only (a
        // worker's TMPDIR is its scratch folder, one of the roots).
        Access::Scoped {
            writable_roots,
            network,
            ..
        } => p::SandboxPolicy::WorkspaceWrite {
            exclude_slash_tmp: true,
            exclude_tmpdir_env_var: true,
            network_access: *network,
            writable_roots: writable_roots
                .iter()
                .map(|root| p::AbsolutePathBuf(root.display().to_string()))
                .collect(),
        },
        Access::ReadOnly => p::SandboxPolicy::ReadOnly {
            network_access: false,
        },
        Access::Full => p::SandboxPolicy::DangerFullAccess,
    }
}

/// Codex's fast service tier ("Fast"), as `model/list` names it.
const FAST_TIER: &str = "priority";
/// Codex's standard speed, which the app server takes as an explicit "not Fast".
const STANDARD_TIER: &str = "default";

/// The top-level `model` and `model_reasoning_effort` of Codex's `config.toml`: what `codex`
/// runs when not told, which `model/list`'s own default doesn't reflect.
#[derive(Debug, Default)]
struct Configured {
    model: Option<String>,
    effort: Option<String>,
}

impl Configured {
    /// Reads the two keys before the first table; the rest of the file doesn't matter here.
    fn parse(text: &str) -> Self {
        let mut configured = Self::default();
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                break;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.split('#').next().unwrap_or_default().trim();
            let value = value.trim_matches(|c| c == '"' || c == '\'').to_owned();
            match key.trim() {
                "model" => configured.model = Some(value),
                "model_reasoning_effort" => configured.effort = Some(value),
                _ => {}
            }
        }
        configured
    }

    /// Marks the configured model default and gives it the configured effort, if it takes it.
    fn apply(self, models: &mut [ModelInfo]) {
        if let Some(chosen) = &self.model
            && models.iter().any(|model| &model.id == chosen)
        {
            for model in models.iter_mut() {
                model.is_default = &model.id == chosen;
            }
        }
        if let Some(effort) = self.effort
            && let Some(model) = models.iter_mut().find(|model| model.is_default)
            && model.efforts.contains(&effort)
        {
            model.default_effort = Some(effort);
        }
    }
}

/// Marks legacy what `model/list` names an upgrade for, the models of an older generation than
/// the newest ("GPT-5.6-Terra" beside "GPT-6-Sol": Codex versions its lineup together), and
/// older namesakes.
fn mark_legacy(models: &mut [ModelInfo]) {
    mark_superseded(models);
    let generation =
        |model: &ModelInfo| versioned(&model.display_name).and_then(|(_, v)| v.first().copied());
    let newest = models.iter().filter_map(generation).max();
    for model in models.iter_mut() {
        if let (Some(own), Some(newest)) = (generation(model), newest)
            && own < newest
        {
            model.legacy = true;
        }
    }
}

fn model_info(model: p::Model) -> ModelInfo {
    let fast = model
        .service_tiers
        .iter()
        .find(|tier| tier.id == FAST_TIER)
        .map(|tier| tier.description.clone());
    ModelInfo {
        fast,
        legacy: model.upgrade.is_some(),
        id: model.model,
        display_name: model.display_name,
        description: model.description,
        resolved: None,
        efforts: model
            .supported_reasoning_efforts
            .into_iter()
            .map(|option| String::from(option.reasoning_effort))
            .collect(),
        default_effort: Some(String::from(model.default_reasoning_effort)),
        is_default: model.is_default,
        input_modalities: model
            .input_modalities
            .iter()
            .filter_map(|modality| serde_json::to_value(modality).ok())
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect(),
    }
}

/// The folder Codex keys a project's trust on for `cwd`: the root of the main checkout when
/// `cwd` is in a git repository or one of its linked worktrees, else `cwd` itself.
fn trust_root(cwd: &Path) -> PathBuf {
    for dir in cwd.ancestors() {
        let git = dir.join(".git");
        if git.is_dir() {
            return dir.to_owned();
        }
        if git.is_file() {
            // A linked worktree: `.git` names `<main>/.git/worktrees/<name>`, whose
            // `commondir` leads back to `<main>/.git`.
            let Some(gitdir) = std::fs::read_to_string(&git).ok().and_then(|text| {
                text.lines()
                    .find_map(|line| line.strip_prefix("gitdir:"))
                    .map(|path| dir.join(path.trim()))
            }) else {
                return dir.to_owned();
            };
            let common = std::fs::read_to_string(gitdir.join("commondir"))
                .map(|common| gitdir.join(common.trim()))
                .unwrap_or(gitdir);
            return match common.canonicalize() {
                Ok(common) if common.file_name().is_some_and(|name| name == ".git") => common
                    .parent()
                    .map_or_else(|| dir.to_owned(), Path::to_path_buf),
                _ => dir.to_owned(),
            };
        }
    }
    cwd.to_owned()
}

/// A TOML basic string, for `-c` override keys.
fn toml_string(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for c in text.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            c if c.is_control() => quoted.push_str(&format!("\\u{:04X}", c as u32)),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

struct CodexReplayer {
    parser: Parser,
    /// Approval requests not answered yet.
    asked: HashSet<String>,
}

impl Replayer for CodexReplayer {
    fn feed(&mut self, dir: Direction, line: &str) -> Vec<ProviderEvent> {
        if dir == Direction::In {
            // The only thing Brigadier sends that the output does not show is an approval's
            // answer: a response to Codex's request.
            let sent: Value = serde_json::from_str(line).unwrap_or_default();
            let answered = format!("codex-{}", sent["id"]);
            let result = &sent["result"];
            if sent.get("method").is_some() || result.is_null() || !self.asked.remove(&answered) {
                return Vec::new();
            }
            let decision = match result.get("decision").and_then(Value::as_str) {
                Some("acceptForSession") => ApprovalDecision::AllowSimilar,
                Some(decision) if decision.starts_with("accept") => ApprovalDecision::Allow,
                None if result["permissions"]
                    .as_object()
                    .is_some_and(|permissions| !permissions.is_empty()) =>
                {
                    ApprovalDecision::Allow
                }
                _ => ApprovalDecision::Deny {
                    message: String::new(),
                },
            };
            return vec![ProviderEvent::ApprovalResolved {
                id: answered,
                decision,
                decided_by: Decider::Recorded,
            }];
        }
        let events: Vec<ProviderEvent> = self
            .parser
            .feed(line)
            .into_iter()
            .filter_map(|output| match output {
                Output::Event(event) => Some(event),
                Output::Control(_) => None,
            })
            .collect();
        for event in &events {
            if let ProviderEvent::ApprovalRequested { request } = event {
                self.asked.insert(request.id.clone());
            }
        }
        events
    }
}

/// JSON-RPC over the app-server's stdio.
struct Rpc {
    process: Arc<CliProcess>,
    next_id: AtomicI64,
    pending: Mutex<HashMap<i64, oneshot::Sender<std::result::Result<Value, String>>>>,
}

impl Rpc {
    fn new(process: Arc<CliProcess>) -> Self {
        Self {
            process,
            next_id: AtomicI64::new(1),
            pending: Mutex::new(HashMap::new()),
        }
    }

    async fn initialize(&self) -> Result<()> {
        let _: Value = self
            .call(
                "initialize",
                &p::InitializeParams {
                    client_info: p::ClientInfo {
                        name: "brigadier".into(),
                        title: Some("Brigadier".into()),
                        version: env!("CARGO_PKG_VERSION").into(),
                    },
                    capabilities: None,
                },
            )
            .await?;
        self.notify("initialized").await
    }

    async fn call<P: Serialize, R: DeserializeOwned>(&self, method: &str, params: &P) -> Result<R> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id, tx);
        let mut message = json!({ "jsonrpc": "2.0", "id": id, "method": method });
        let params =
            serde_json::to_value(params).map_err(|err| Error::Protocol(err.to_string()))?;
        if !params.is_null() {
            message["params"] = params;
        }
        if let Err(err) = self.process.write_line(&message.to_string()).await {
            lock(&self.pending).remove(&id);
            return Err(err);
        }
        let result = match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(result)) => result.map_err(Error::Rejected)?,
            Ok(Err(_)) => return Err(Error::Protocol(exit_message(&self.process))),
            Err(_) => {
                lock(&self.pending).remove(&id);
                return Err(Error::Timeout("Codex to answer"));
            }
        };
        serde_json::from_value(result).map_err(|err| {
            tracing::warn!(method, error = %err, "Could not decode a Codex response");
            Error::Protocol("Brigadier could not read Codex's reply.".into())
        })
    }

    async fn notify(&self, method: &str) -> Result<()> {
        self.process
            .write_line(&json!({ "jsonrpc": "2.0", "method": method }).to_string())
            .await
    }

    async fn respond(&self, id: Value, result: Value) -> Result<()> {
        self.process
            .write_line(&json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string())
            .await
    }

    async fn reject(&self, id: Value, message: &str) {
        let line = json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": message },
        });
        let _ = self.process.write_line(&line.to_string()).await;
    }

    fn resolve(&self, id: i64, result: std::result::Result<Value, String>) {
        if let Some(reply) = lock(&self.pending).remove(&id) {
            let _ = reply.send(result);
        }
    }

    /// Fails every waiting call (the process is gone).
    fn fail_all(&self) {
        lock(&self.pending).clear();
    }
}

struct Shared {
    /// Unanswered approvals: approval id → JSON-RPC request id and answer shape.
    approvals: Mutex<HashMap<String, (Value, PendingKind)>>,
    turn_id: Mutex<Option<String>>,
    /// `thread/compact/start` is being sent; the reader tells its parser.
    compact_asked: AtomicBool,
    events: Events,
}

pub struct CodexSession {
    thread_id: String,
    /// The app-server serves `thread/compact/start`.
    compacts: bool,
    rpc: Arc<Rpc>,
    shared: Arc<Shared>,
    access: Access,
    /// [`SessionSpec::auto_review`].
    auto_review: bool,
    /// Its sandbox is a permission profile set when the thread opened.
    profiled: bool,
    effort: Option<String>,
}

impl CodexSession {
    async fn emit(&self, event: ProviderEvent) {
        self.shared.events.send(event).await;
    }

    fn turn_id(&self) -> Option<String> {
        lock(&self.shared.turn_id).clone()
    }

    fn input(input: &TurnInput) -> Result<Vec<p::UserInput>> {
        if input.is_empty() {
            return Err(Error::Invalid("the message is empty".into()));
        }
        let mut items = Vec::with_capacity(input.parts.len());
        for part in &input.parts_with_file_notes() {
            match part {
                crate::InputPart::Image(file) => items.push(p::UserInput::LocalImageUserInput {
                    detail: None,
                    path: file.path.display().to_string(),
                    type_: p::LocalImageUserInputType::LocalImage,
                }),
                crate::InputPart::Text(text) if !text.trim().is_empty() => {
                    items.push(p::UserInput::TextUserInput {
                        text: text.clone(),
                        text_elements: Vec::new(),
                        type_: p::TextUserInputType::Text,
                    });
                }
                crate::InputPart::Text(_) => {}
            }
        }
        Ok(items)
    }

    async fn start_turn(&self, input: &TurnInput) -> Result<()> {
        let effort = match &self.effort {
            Some(effort) => Some(
                p::ReasoningEffort::try_from(effort.as_str())
                    .map_err(|err| Error::Invalid(format!("effort {effort}: {err}")))?,
            ),
            None => None,
        };
        let started: p::TurnStartResponse = self
            .rpc
            .call(
                "turn/start",
                &p::TurnStartParams {
                    thread_id: self.thread_id.clone(),
                    input: Self::input(input)?,
                    effort,
                    summary: Some(p::ReasoningSummary::Auto),
                    sandbox_policy: (!self.profiled).then(|| sandbox_policy(&self.access)),
                    approval_policy: Some(approval_policy(&self.access)),
                    approvals_reviewer: Some(reviewer(self.auto_review)),
                    ..Default::default()
                },
            )
            .await?;
        tracing::debug!(turn = %started.turn.id, "codex turn started");
        Ok(())
    }

    async fn steer_turn(&self, turn_id: String, input: &TurnInput) -> Result<()> {
        let _: p::TurnSteerResponse = self
            .rpc
            .call(
                "turn/steer",
                &p::TurnSteerParams {
                    thread_id: self.thread_id.clone(),
                    expected_turn_id: turn_id,
                    input: Self::input(input)?,
                    ..Default::default()
                },
            )
            .await?;
        Ok(())
    }
}

impl ProviderSession for CodexSession {
    fn native_id(&self) -> String {
        self.thread_id.clone()
    }

    fn send(&self, input: TurnInput) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            match self.turn_id() {
                // A message sent during a turn goes into it, as with Claude.
                Some(turn_id) => self.steer_turn(turn_id, &input).await,
                None => self.start_turn(&input).await,
            }
        })
    }

    fn steer(&self, input: TurnInput) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let Some(turn_id) = self.turn_id() else {
                // The turn already ended: the message starts the next one.
                return self.start_turn(&input).await;
            };
            match self.steer_turn(turn_id, &input).await {
                Err(Error::Rejected(_)) if self.turn_id().is_none() => {
                    self.start_turn(&input).await
                }
                other => other,
            }
        })
    }

    fn can_compact(&self) -> bool {
        self.compacts
    }

    fn compact(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if !self.compacts {
                return Err(Error::Invalid(format!(
                    "compacting needs Codex {COMPACT_SINCE} or newer"
                )));
            }
            if self.turn_id().is_some() {
                return Err(Error::Invalid(
                    "Codex is still answering; compact once it is done".into(),
                ));
            }
            self.shared.compact_asked.store(true, Ordering::Release);
            let started: std::result::Result<p::ThreadCompactStartResponse, _> = self
                .rpc
                .call(
                    "thread/compact/start",
                    &p::ThreadCompactStartParams {
                        thread_id: self.thread_id.clone(),
                    },
                )
                .await;
            if started.is_err() {
                self.shared.compact_asked.store(false, Ordering::Release);
            }
            started.map(drop)
        })
    }

    fn interrupt(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let Some(turn_id) = self.turn_id() else {
                return Ok(());
            };
            let _: Value = self
                .rpc
                .call(
                    "turn/interrupt",
                    &p::TurnInterruptParams {
                        thread_id: self.thread_id.clone(),
                        turn_id,
                    },
                )
                .await?;
            Ok(())
        })
    }

    fn answer(&self, approval_id: String, decision: ApprovalDecision) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let (rpc_id, kind) = lock(&self.shared.approvals)
                .remove(&approval_id)
                .ok_or_else(|| Error::Invalid(format!("no pending approval {approval_id}")))?;
            let allow = !matches!(decision, ApprovalDecision::Deny { .. });
            let result = match kind {
                PendingKind::Command { grant: true }
                    if decision == ApprovalDecision::AllowSimilar =>
                {
                    json!({ "decision": "acceptForSession" })
                }
                _ if decision == ApprovalDecision::AllowSimilar => {
                    lock(&self.shared.approvals).insert(approval_id, (rpc_id, kind));
                    return Err(Error::Invalid(
                        "this request has no \"don't ask again\" option".into(),
                    ));
                }
                PendingKind::Command { .. } | PendingKind::FileChange => {
                    json!({ "decision": if allow { "accept" } else { "decline" } })
                }
                PendingKind::Permissions(requested) => json!({
                    "permissions": if allow { requested } else { json!({}) },
                    "scope": "turn",
                }),
            };
            self.rpc.respond(rpc_id, result).await
        })
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if self.rpc.process.is_running() {
                match tokio::time::timeout(EXIT_GRACE, archive_thread(&self.rpc, &self.thread_id))
                    .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => {
                        tracing::debug!(thread = %self.thread_id, error = %err, "could not archive a codex thread");
                    }
                    Err(_) => {
                        tracing::debug!(thread = %self.thread_id, "archiving a codex thread timed out");
                    }
                }
            }
            self.rpc.process.shutdown(EXIT_GRACE).await;
        })
    }

    fn is_running(&self) -> bool {
        self.rpc.process.is_running()
    }
}

/// Serves a session's app-server output until it exits. `trusted` names the MCP servers whose
/// tool calls run without asking.
async fn read_loop(
    rpc: Arc<Rpc>,
    mut stdout: mpsc::Receiver<String>,
    shared: Arc<Shared>,
    trusted: HashSet<String>,
) {
    let mut parser = Parser::live();
    while let Some(line) = stdout.recv().await {
        if shared.compact_asked.swap(false, Ordering::AcqRel) {
            parser.compact_requested();
        }
        for output in parser.feed(&line) {
            match output {
                Output::Event(event) => {
                    shared.events.send(event).await;
                }
                Output::Control(Control::Response { id, result }) => rpc.resolve(id, result),
                Output::Control(Control::Approval {
                    approval_id,
                    rpc_id,
                    kind,
                }) => {
                    lock(&shared.approvals).insert(approval_id, (rpc_id, kind));
                }
                Output::Control(Control::Elicitation {
                    rpc_id,
                    server,
                    tool_approval,
                    message,
                }) => {
                    // Trusted servers' tool calls are configured to run without asking, so
                    // this should not happen; answer rather than leave the turn hanging.
                    let accept = tool_approval && trusted.contains(&server);
                    let action = if accept { "accept" } else { "decline" };
                    let answer = if accept {
                        json!({ "action": "accept", "content": {} })
                    } else {
                        json!({ "action": "decline" })
                    };
                    let _ = rpc.respond(rpc_id, answer).await;
                    shared
                        .events
                        .send(ProviderEvent::Notice {
                            level: NoticeLevel::Warning,
                            message: format!(
                                "The MCP server {server} asked \"{message}\"; Brigadier \
                                 answered {action}."
                            ),
                        })
                        .await;
                }
                Output::Control(Control::Unsupported { rpc_id, method }) => {
                    rpc.reject(rpc_id, &format!("Brigadier does not handle {method}"))
                        .await;
                }
            }
        }
        *lock(&shared.turn_id) = parser.turn_id().map(str::to_owned);
    }

    let exit = rpc.process.exited().await;
    rpc.fail_all();
    lock(&shared.turn_id).take();
    shared
        .events
        .send(ProviderEvent::Exited {
            code: exit.code,
            stderr_tail: (exit.code != Some(0))
                .then(|| rpc.process.stderr_tail())
                .flatten(),
        })
        .await;
}

fn exit_message(process: &CliProcess) -> String {
    match process.stderr_tail() {
        Some(tail) => format!("Codex exited: {tail}"),
        None => "Codex exited".into(),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_input_preserves_text_image_text_order() {
        let file = crate::InputFile {
            path: PathBuf::from("/attachments/image.png"),
            name: "image.png".into(),
            mime: "image/png".into(),
        };
        let input = TurnInput {
            parts: vec![
                crate::InputPart::Text("before".into()),
                crate::InputPart::Image(file.clone()),
                crate::InputPart::Text("between".into()),
                crate::InputPart::Image(file),
                crate::InputPart::Text("after".into()),
            ],
            files: Vec::new(),
        };
        let items = CodexSession::input(&input).unwrap();
        let json: Vec<_> = items
            .iter()
            .map(|item| serde_json::to_value(item).unwrap())
            .collect();
        assert_eq!(
            json.iter()
                .map(|part| part["type"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["text", "localImage", "text", "localImage", "text"]
        );
        assert_eq!(json[0]["text"], "before");
        assert_eq!(json[2]["text"], "between");
        assert_eq!(json[4]["text"], "after");
        assert_eq!(json[1]["path"], "/attachments/image.png");
        assert_eq!(json[1], json[3]);
    }

    fn spec(tools: ToolSet, allowed_models: Option<AllowedModels>) -> SessionSpec {
        SessionSpec {
            cwd: PathBuf::from("/tmp"),
            model: Some("gpt-6.1-sol".into()),
            effort: None,
            fast: false,
            origin: Origin::New,
            access: Access::Full,
            append_system_prompt: None,
            mcp_servers: Vec::new(),
            tools,
            env: Vec::new(),
            unset_env: Vec::new(),
            low_priority: false,
            record_to: None,
            redactor: None,
            owned_cwd: false,
            auto_compact: true,
            allowed_models,
            auto_review: false,
            omit_ai_coauthors: false,
        }
    }

    fn without_sub_agents(args: &[String]) -> bool {
        args.windows(2)
            .any(|pair| pair[0] == "-c" && pair[1] == NO_SUB_AGENTS)
    }

    #[test]
    fn a_session_with_limited_models_has_no_sub_agents() {
        let worker = spec(ToolSet::Lean, Some(AllowedModels::default()));
        assert!(without_sub_agents(&app_server_args(Some(&worker))));
        let raw = spec(ToolSet::Default, None);
        assert!(!without_sub_agents(&app_server_args(Some(&raw))));
        assert!(!without_sub_agents(&app_server_args(None)));
        // Restricted sessions never had them, and say so once.
        let orchestrator = spec(ToolSet::None, Some(AllowedModels::default()));
        let args = app_server_args(Some(&orchestrator));
        assert_eq!(args.iter().filter(|arg| *arg == NO_SUB_AGENTS).count(), 1);
    }
}
