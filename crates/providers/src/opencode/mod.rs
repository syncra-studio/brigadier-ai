//! OpenCode adapter: one `opencode serve --stdio` per session, driven over its HTTP API, its
//! events read from the server's event stream (`GET /api/event`). The server listens on a
//! loopback port of its own, behind a password made for it, and exits when its stdin closes.
//!
//! A session's level becomes OpenCode's own permission rules on the session
//! ([`parse::permissions`]), after its agent's defaults (everything allowed but folders outside
//! the session and `.env` files, which ask):
//!
//! - Ask: edits and commands ask, but commands that only read (`ls`, `cat`, `git status`, …).
//!   Accept edits: those and `brigadier …` run too, any other command asks. Full access: nothing
//!   asks.
//! - Read-only (Brigadier's own readers): edits, commands, sub-agents and anything outside the
//!   folder are declined, never asked about. A side chat's and a tab chat's shell runs
//!   `brigadier …` and nothing else, as on the other CLIs.
//!
//! Every request OpenCode asks reaches the user as a card ([`ProviderEvent::ApprovalRequested`]).
//!
//! Threads and workers ([`ToolSet::user_setup`]) run with the user's own OpenCode setup. A naked
//! worker ([`SessionSpec::naked_worker`]) and every other role run with an empty private config
//! folder and home (`OPENCODE_CONFIG_DIR`, `OPENCODE_TEST_HOME`): none of the user's skills, MCP
//! servers, plugins or global instructions; the repository's `AGENTS.md` still loads, and the
//! login stays, being in OpenCode's data folder. Brigadier's MCP servers and skills reach every
//! session through `OPENCODE_CONFIG_CONTENT`. OpenCode keeps no memory of its own across
//! sessions, so there is none to switch off (D9).
//!
//! OpenCode keeps its sessions in a database of its own, not in files: the adapter keeps each
//! session's messages, as its API gives them, in a transcript file (`<data>/opencode/<id>.jsonl`,
//! [`write_transcript`]) after every turn, which is the session file its thread is read back from.

mod parse;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_sandbox::Platform;
use reqwest::Method;
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::cli::CliEnv;
use crate::events::Events;
use crate::model::*;
use crate::process::{self, CliProcess};
use crate::record::{self, Direction, Recorder};
use crate::{
    BoxFuture, Error, Ledger, Provider, ProviderSession, Replayer, Result, Started, now_ms,
};
use parse::Parser;

/// A session's messages kept as its transcript ([`write_transcript`]).
const TRANSCRIPT: &str = "transcript";
/// A command's output file OpenCode keeps (`~/.local/share/opencode/shell/<project>/sh_<id>.out`).
const SHELL_OUTPUT: &str = "shellOutput";

pub static ADAPTER: crate::adapters::Adapter = crate::adapters::Adapter {
    kind: ProviderKind::OpenCode,
    label: "OpenCode",
    binary: "opencode",
    // Its script; on Windows, which it has no script for, its npm package.
    install: [
        "curl -fsSL https://opencode.ai/v2/install | bash",
        "npm i -g @opencode/cli@latest",
    ],
    login: "opencode auth login",
    sign_in: &["auth", "login"],
    sign_out: &["auth", "logout"],
    // One login store, in its data folder's database.
    home: None,
    skill_prefix: "/",
    // Switching its built-in tools off isn't implemented.
    tools: &[],
    session_files: &[TRANSCRIPT],
    vendor: "OpenCode",
    maker: "OpenCode by Anomaly",
    cli_name: "OpenCode CLI",
    install_guide: "https://opencode.ai/docs",
    // It reports no usage windows (D17).
    usual_windows: &[],
    // The frame of its mark (opencode.ai/favicon.svg), in the current colour.
    glyph: crate::adapters::Glyph {
        view_box: "96 96 320 320",
        path: "M384 416H128V96H384V416ZM320 160H192V352H320V160Z",
        class_name: None,
    },
    // Its prompt takes a list of skills.
    one_skill_per_message: false,
    // Its step and usage tokens carry `cache.write`.
    reports_cache_writes: true,
    // A message `/init` is the model's text; commands run by `POST /session/{id}/command`, which
    // keeps the expanded template as the user's message.
    commands_as_text: false,
    // Its sub-agent tool (`task` before 2.0).
    subagent_tools: &["subagent"],
    // Its `POST /session/{id}/compact`. Its own `init` and `review` run only by their endpoint.
    builtin_commands: &[("compact", "Compact this conversation", "session/compact")],
    mcp_prompts: false,
    // `session.usage.updated` holds the session's totals.
    cumulative_usage: true,
    // Its free small model.
    chore_family: "flint",
    chore_effort: None,
    coauthors_in_instructions: true,
    // Nothing to set: Brigadier asks it to compact, its own auto-compaction stays the backstop.
    compaction: Some(|_, _| crate::NativeCompaction::default()),
    brigadier_compacts: true,
    subagent_transcripts: false,
    output_hook: false,
    pause_finishes_command: false,
    child_usage: None,
    // Through its server's `DELETE /api/session/{id}`.
    removes_by_id: true,
    sign_in_code_page: false,
    installer_off_path: false,
    space_files: &[TRANSCRIPT, SHELL_OUTPUT],
    temp_dir: None,
    npm_package: "@opencode/cli",
    update_limits: |_| crate::adapters::UpdateLimits::default(),
    // Its config's `skills` folders are `<plugin>/skills`.
    skills_overlay: |spec, overlay| {
        spec.skill_plugins.push(overlay.join(".claude"));
        Ok(())
    },
    new: |platform, env| Arc::new(OpenCode::new(platform, env)),
    // `opencode v2.0.19`.
    names_itself: |first| first.starts_with("opencode"),
    // Its script installs into `~/.opencode/bin` and adds that to a shell profile.
    installer_folder: |env| env.home().map(|home| home.join(".opencode").join("bin")),
    // Its login is a row of its database, which changes with every session.
    login_mark: |_| None,
    mcp_stores: |_, _| Vec::new(),
    item_sources: crate::items::own_file,
    // Its display reader is a 1:1 read of its transcript's lines, with the same ids.
    items: |_, display, describe| crate::items::from_display(display, describe),
    read_session: parse::read_transcript,
    session_context: |path| crate::session_file::newest(path, parse::context_of),
    // Read whole each time.
    follow_session: |path| {
        let path = path.to_owned();
        Box::new(move || parse::read_transcript(&path))
    },
    moved_session_file: |_| None,
    // Its sessions are in its database, never found as files.
    leftovers: |_, _, _| Vec::new(),
    delta_location: parse::delta_location,
    stream_end: parse::stream_end,
    usage_limit: parse::usage_limit,
    trust: None,
};

const EVENTS: usize = 512;
const STATUS_TIMEOUT: Duration = Duration::from_secs(20);
const START_TIMEOUT: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const EXIT_GRACE: Duration = Duration::from_secs(3);
/// Brigadier's standing instructions, as an instruction entry of the session's own.
const INSTRUCTIONS_KEY: &str = "brigadier";
/// Messages asked for at a time.
const PAGE: usize = 200;

pub struct OpenCode {
    platform: Arc<dyn Platform>,
    env: Arc<CliEnv>,
}

impl OpenCode {
    pub fn new(platform: Arc<dyn Platform>, env: Arc<CliEnv>) -> Self {
        Self { platform, env }
    }

    fn binary(&self) -> Result<PathBuf> {
        self.env
            .resolve(ProviderKind::OpenCode)
            .ok_or(Error::NotInstalled(ProviderKind::OpenCode))
    }

    async fn version(&self) -> Option<String> {
        let spec = self.env.spec(&self.binary().ok()?).arg("--version");
        let output = process::run(&self.platform, &spec, STATUS_TIMEOUT)
            .await
            .ok()?;
        crate::cli::parse_version(&output.stdout)
    }

    /// Runs `work` against a throwaway server with the user's own setup, then ends it.
    async fn control<T>(&self, work: impl AsyncFnOnce(&Server) -> Result<T>) -> Result<T> {
        let cwd = self.platform.paths().data_dir.clone();
        let mut spec = self.env.spec(&self.binary()?);
        spec.cwd = Some(cwd);
        let server = Server::start(
            self.platform.clone(),
            spec,
            process::Options::default(),
            None,
        )
        .await?;
        let result = work(&server).await;
        server.process.shutdown(EXIT_GRACE).await;
        result
    }

    fn transcript(&self, id: &str) -> PathBuf {
        self.platform
            .paths()
            .data_dir
            .join("opencode")
            .join(format!("{id}.jsonl"))
    }
}

/// Whether OpenCode can run a model, and the provider signed in to: a saved credential, or its
/// key in the environment. With none, any model it lists (its free ones, a local one) needs no
/// credential, as OpenCode lists only the models it can run.
fn signed_in(integrations: &Value, models: &[Value], env: &CliEnv) -> (bool, Option<String>) {
    let connected = integrations["data"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|it| {
            it["connections"].as_array().is_some_and(|c| !c.is_empty())
                || it["methods"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|method| {
                        method["type"] == "env"
                            && method["names"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .any(|name| {
                                    name.as_str()
                                        .and_then(|name| env.var(name))
                                        .is_some_and(|value| !value.is_empty())
                                })
                    })
        });
    match connected {
        Some(it) => (true, it["name"].as_str().map(str::to_owned)),
        None => (models.iter().any(|model| model["enabled"] != false), None),
    }
}

/// The models OpenCode lists, once it has loaded them: a fresh server answers its first asks
/// with none.
async fn list_models(server: &Server) -> Result<Vec<Value>> {
    for _ in 0..40 {
        let listed = server.call(Method::GET, "/api/model", None).await?;
        if let Some(models) = listed["data"]
            .as_array()
            .filter(|models| !models.is_empty())
        {
            return Ok(models.clone());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Ok(Vec::new())
}

/// A model's id as Brigadier shows and passes it: `<provider>/<model>`.
fn model_id(model: &Value) -> Option<String> {
    Some(format!(
        "{}/{}",
        model["providerID"].as_str()?,
        model["id"].as_str()?
    ))
}

/// OpenCode's model reference for `id` (`<provider>/<model>`) at `effort` (its variant).
fn model_ref(id: &str, effort: Option<&str>) -> Result<Value> {
    let (provider, model) = id
        .split_once('/')
        .ok_or_else(|| Error::Invalid(format!("{id} is not an OpenCode model (provider/model)")))?;
    let mut reference = json!({ "providerID": provider, "id": model });
    if let Some(effort) = effort {
        reference["variant"] = effort.into();
    }
    Ok(reference)
}

fn model_info(model: &Value, default: Option<&str>) -> Option<ModelInfo> {
    let id = model_id(model)?;
    Some(ModelInfo {
        display_name: model["name"].as_str().unwrap_or(&id).to_owned(),
        description: model["providerID"].as_str().unwrap_or_default().to_owned(),
        resolved: None,
        efforts: model["variants"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|variant| variant["id"].as_str().map(str::to_owned))
            .collect(),
        default_effort: None,
        is_default: default == Some(id.as_str()),
        input_modalities: model
            .pointer("/capabilities/input")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|kind| kind.as_str().map(str::to_owned))
            .collect(),
        fast: None,
        legacy: false,
        id,
    })
}

/// The config a session's server adds on top of whatever setup it loads: Brigadier's MCP
/// servers and skills, and no compaction of its own when the session must not compact.
fn config_content(spec: &SessionSpec) -> Value {
    let mut servers = Map::new();
    for server in &spec.mcp_servers {
        let values: Map<String, Value> = server
            .env
            .iter()
            .map(|(name, value)| (name.clone(), value.clone().into()))
            .collect();
        let mut entry = match &server.transport {
            McpTransport::Stdio { command, args, cwd } => {
                let mut command = vec![command.display().to_string()];
                command.extend(args.iter().cloned());
                let mut entry =
                    json!({ "type": "local", "command": command, "environment": values });
                if let Some(cwd) = cwd {
                    entry["cwd"] = cwd.display().to_string().into();
                }
                entry
            }
            McpTransport::Http { url } => {
                json!({ "type": "remote", "url": url, "headers": values, "oauth": false })
            }
        };
        if let Some(secs) = server.tool_timeout_secs {
            entry["timeout"] = json!({ "execution": secs.saturating_mul(1000) });
        }
        // Its tools in the model's own list, not behind OpenCode's code tool.
        if server.always_load {
            entry["codemode"] = false.into();
        }
        servers.insert(server.name.clone(), entry);
    }
    let mut config = json!({ "mcp": { "servers": servers } });
    let skills: Vec<String> = spec
        .skill_plugins
        .iter()
        .chain(&spec.installed_skills)
        .map(|plugin| plugin.join("skills").display().to_string())
        .collect();
    if !skills.is_empty() {
        config["skills"] = skills.into();
    }
    if !spec.auto_compact && spec.native_compaction.is_none() {
        config["compaction"] = json!({ "auto": false });
    }
    config
}

/// Whether a session runs without the user's own setup: a naked worker, and every role but a
/// thread's or a worker's.
fn sealed(spec: &SessionSpec) -> bool {
    spec.naked_worker || !spec.tools.user_setup()
}

/// The server's environment for a session: its own variables, Brigadier's config, and for a
/// sealed one the empty private folders under `private`.
fn session_env(spec: &SessionSpec, private: Option<&Path>) -> Vec<(String, String)> {
    let mut env = spec.env.clone();
    env.push((
        "OPENCODE_CONFIG_CONTENT".into(),
        config_content(spec).to_string(),
    ));
    if let Some(private) = private {
        env.push((
            "OPENCODE_CONFIG_DIR".into(),
            private.join("config").display().to_string(),
        ));
        env.push((
            "OPENCODE_TEST_HOME".into(),
            private.join("home").display().to_string(),
        ));
    }
    env
}

/// `path` with `pairs` as its query, encoded.
fn with_query(path: &str, pairs: &[(&str, &str)]) -> String {
    let url = reqwest::Url::parse_with_params(&format!("http://server{path}"), pairs)
        .expect("a fixed base parses");
    format!("{}?{}", url.path(), url.query().unwrap_or_default())
}

/// The `/` commands OpenCode runs in `folder` (its built-ins, the user's and the project's
/// commands and skills), once it has loaded them: a fresh server answers its first asks with
/// none.
async fn list_commands(server: &Server, folder: &Path) -> Result<Vec<CliCommand>> {
    let folder = folder.display().to_string();
    let path = with_query("/api/command", &[("location[directory]", &folder)]);
    for _ in 0..40 {
        let listed = server.call(Method::GET, &path, None).await?;
        let commands: Vec<CliCommand> = listed["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|command| {
                let name = command["name"].as_str()?;
                Some(CliCommand {
                    kind: Some(CliCommandKind::Command),
                    source: Some(ADAPTER.label.into()),
                    invocation: Some(name.to_owned()),
                    agent_only: Some(ProviderKind::OpenCode),
                    name: name.to_owned(),
                    text: format!("/{name}"),
                    description: command["description"].as_str().map(str::to_owned),
                    argument_hint: None,
                    action: None,
                })
            })
            .collect();
        if !commands.is_empty() {
            return Ok(commands);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Ok(Vec::new())
}

/// Keeps session `id`'s messages, oldest first, one per line, at `path` (written aside, then
/// moved in: a reader never sees half of it).
async fn write_transcript(server: &Server, id: &str, path: &Path) -> Result<()> {
    let mut lines = String::new();
    let limit = PAGE.to_string();
    let mut query = vec![("order", "asc".to_owned()), ("limit", limit.clone())];
    // A long session's messages, a page at a time.
    loop {
        let pairs: Vec<(&str, &str)> = query.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let page = server
            .call(
                Method::GET,
                &with_query(&format!("/api/session/{id}/message"), &pairs),
                None,
            )
            .await?;
        let messages = page["data"].as_array().cloned().unwrap_or_default();
        for message in &messages {
            lines.push_str(&message.to_string());
            lines.push('\n');
        }
        match page.pointer("/cursor/next").and_then(Value::as_str) {
            Some(next) if messages.len() >= PAGE => {
                query = vec![("cursor", next.to_owned()), ("limit", limit.clone())];
            }
            _ => break,
        }
    }
    let aside = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    std::fs::write(&aside, lines)?;
    std::fs::rename(&aside, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&aside);
    })?;
    Ok(())
}

/// Whether `path` is a file of `kind` this adapter records, by its place and name: what its
/// `remove` may delete.
fn removable(kind: &str, path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let parent = |up: usize| {
        path.ancestors()
            .nth(up)
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
    };
    match kind {
        TRANSCRIPT => {
            name.starts_with("ses") && name.ends_with(".jsonl") && parent(1) == Some("opencode")
        }
        SHELL_OUTPUT => {
            name.starts_with("sh_") && name.ends_with(".out") && parent(2) == Some("shell")
        }
        _ => false,
    }
}

const NOT_INSTALLED: &str = "OpenCode isn't installed. Install it from Settings › Providers.";
const SIGNED_OUT: &str = "OpenCode isn't signed in. Sign in from Settings › Providers.";

impl Provider for OpenCode {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenCode
    }

    fn status(&self) -> BoxFuture<'_, ProviderStatus> {
        Box::pin(async move {
            let found = self.binary();
            let mut status = ProviderStatus {
                provider: ProviderKind::OpenCode,
                path: found.as_ref().ok().map(|path| path.display().to_string()),
                version: None,
                logged_in: false,
                auth_method: None,
                plan: None,
                email: None,
                organization: None,
                guidance: None,
                compacts: true,
            };
            if found.is_err() {
                status.guidance = Some(NOT_INSTALLED.into());
                return status;
            }
            let (version, ready) = tokio::join!(
                self.version(),
                self.control(async |server: &Server| {
                    let listed = server.call(Method::GET, "/api/integration", None).await?;
                    if let ready @ (true, _) = signed_in(&listed, &[], &self.env) {
                        return Ok(ready);
                    }
                    Ok(signed_in(&listed, &list_models(server).await?, &self.env))
                })
            );
            status.version = version;
            match ready {
                Ok((logged_in, auth_method)) => {
                    status.logged_in = logged_in;
                    status.auth_method = auth_method;
                }
                Err(err) => status.guidance = Some(format!("Could not ask OpenCode: {err}")),
            }
            if !status.logged_in && status.guidance.is_none() {
                status.guidance = Some(SIGNED_OUT.into());
            }
            status
        })
    }

    fn models(&self) -> BoxFuture<'_, Result<ModelCatalog>> {
        Box::pin(async move {
            let version = self.version().await;
            let (models, default) = self
                .control(async |server: &Server| {
                    let models = list_models(server).await?;
                    let default = server.call(Method::GET, "/api/model/default", None).await?;
                    Ok((models, model_id(&default["data"])))
                })
                .await?;
            let mut models: Vec<ModelInfo> = models
                .iter()
                .filter(|model| model["enabled"] != false)
                .filter_map(|model| model_info(model, default.as_deref()))
                .collect();
            mark_superseded(&mut models);
            Ok(ModelCatalog {
                provider: ProviderKind::OpenCode,
                models,
                cli_version: version,
                fetched_at_ms: now_ms(),
            })
        })
    }

    /// OpenCode reports no usage windows; a limit it meets arrives as an error (D17).
    fn quota(&self) -> BoxFuture<'_, Result<QuotaSnapshot>> {
        Box::pin(async {
            Ok(QuotaSnapshot {
                provider: ProviderKind::OpenCode,
                windows: Vec::new(),
                limit: None,
                observed_at_ms: now_ms(),
                source: QuotaSource::Read,
            })
        })
    }

    fn start(&self, spec: SessionSpec, ledger: Arc<dyn Ledger>) -> BoxFuture<'_, Result<Started>> {
        Box::pin(async move {
            let binary = self.binary()?;
            let cwd = spec.cwd.canonicalize().map_err(|err| {
                Error::Invalid(format!("working directory {}: {err}", spec.cwd.display()))
            })?;
            let version = self.version().await;
            let recorder = match &spec.record_to {
                Some(path) => Some(Arc::new(Recorder::create(
                    path,
                    &record::Header {
                        fixture: record::FORMAT,
                        provider: ProviderKind::OpenCode,
                        cli_version: version.clone(),
                        recorded_at_ms: now_ms(),
                        title: "OpenCode session".into(),
                    },
                    spec.redactor.clone(),
                )?)),
                None => None,
            };
            // Recorded before it exists, so a crash between the two leaves nothing unknown.
            let private = if sealed(&spec) {
                let dir = self.platform.paths().data_dir.join(format!(
                    "opencode-{}",
                    &uuid::Uuid::new_v4().simple().to_string()[..12]
                ));
                ledger
                    .record(Artifact::ScratchDir {
                        path: dir.display().to_string(),
                    })
                    .await?;
                std::fs::create_dir_all(dir.join("config"))?;
                std::fs::create_dir_all(dir.join("home"))?;
                Some(dir)
            } else {
                None
            };
            let mut process_spec = self.env.spec(&binary);
            process_spec.cwd = Some(cwd.clone());
            process_spec.low_priority = spec.low_priority;
            crate::cli::apply_session_env(
                &mut process_spec,
                &session_env(&spec, private.as_deref()),
                &spec.unset_env,
            );
            let server = Arc::new(
                Server::start(
                    self.platform.clone(),
                    process_spec,
                    process::Options {
                        private_logs: ledger.private_logs(),
                        recorder: recorder.clone(),
                        redactor: spec.redactor.clone(),
                        ledger: Some(ledger.clone()),
                        sweep_dir: spec.sweep_dir.clone(),
                        keeps_detached: spec.keeps_detached,
                    },
                    recorder,
                )
                .await?,
            );
            let pid = server.process.pid();
            let started_at_ms = server.process.started_at_ms();
            let recorded = ledger
                .record(if spec.keeps_detached {
                    Artifact::KeptCli { pid, started_at_ms }
                } else {
                    Artifact::Process { pid, started_at_ms }
                })
                .await;
            let opened = match recorded {
                Ok(()) => tokio::time::timeout(
                    START_TIMEOUT,
                    open_session(&server, &spec, &cwd, ledger.as_ref()),
                )
                .await
                .unwrap_or(Err(Error::Timeout("OpenCode to open the session"))),
                Err(err) => Err(err),
            };
            let opened = match opened {
                Ok((id, model, window)) => {
                    let transcript = self.transcript(&id);
                    self.keep_transcript(&server, &id, &transcript, ledger.as_ref())
                        .await
                        .map(|()| (id, model, window, transcript))
                }
                Err(err) => Err(err),
            };
            let (id, model, window, transcript) = match opened {
                Ok(opened) => opened,
                Err(err) => {
                    server.process.shutdown(EXIT_GRACE).await;
                    return Err(err);
                }
            };
            let (events_tx, events) = mpsc::channel(EVENTS);
            let session = Arc::new(OpenCodeSession {
                server: server.clone(),
                id: id.clone(),
                cwd: cwd.clone(),
                parser: Arc::new(Mutex::new(Parser::new(Some(id.clone()), window))),
                events: Arc::new(Events::new(events_tx, spec.redactor.clone())),
                window,
                steered: Arc::new(Mutex::new(Vec::new())),
            });
            // Listening before anything is sent: a turn's first events are never missed.
            let (connected, listening) = oneshot::channel();
            tokio::spawn(read_loop(
                session.clone(),
                ledger,
                transcript.clone(),
                connected,
            ));
            if !matches!(
                tokio::time::timeout(START_TIMEOUT, listening).await,
                Ok(Ok(()))
            ) {
                session.server.process.shutdown(EXIT_GRACE).await;
                return Err(Error::Timeout("OpenCode's event stream"));
            }
            session
                .events
                .send(ProviderEvent::SessionStarted {
                    native_id: id,
                    model,
                    cwd: Some(cwd.display().to_string()),
                    cli_version: version,
                    file: Some(transcript.display().to_string()),
                })
                .await;
            Ok(Started { session, events })
        })
    }

    fn remove(&self, artifacts: Vec<Artifact>) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let mut sessions = Vec::new();
            for artifact in artifacts {
                match artifact {
                    Artifact::Session { provider, id, .. }
                        if provider == ProviderKind::OpenCode =>
                    {
                        sessions.push(id);
                    }
                    Artifact::CliFile {
                        provider,
                        kind,
                        path,
                        ..
                    } if provider == ProviderKind::OpenCode
                        && removable(&kind, Path::new(&path)) =>
                    {
                        match std::fs::remove_file(&path) {
                            Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                                return Err(err.into());
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            if sessions.is_empty() {
                return Ok(());
            }
            self.control(async |server: &Server| {
                for id in &sessions {
                    match server
                        .call(Method::DELETE, &format!("/api/session/{id}"), None)
                        .await
                    {
                        Ok(_) => {}
                        // Never saved, or deleted before: nothing to do.
                        Err(Error::Rejected(message)) if message.contains("404") => {}
                        Err(err) => return Err(err),
                    }
                }
                Ok(())
            })
            .await
        })
    }

    fn replayer(&self) -> Box<dyn Replayer> {
        Box::new(OpenCodeReplayer {
            parser: Parser::new(None, None),
        })
    }

    /// Asked of OpenCode itself, by the session's own messages: only what the user sent or
    /// steered in is a `user` message, never a tool's output or the model's words.
    fn took_input(&self, native_id: &str, needle: &str) -> BoxFuture<'_, bool> {
        let (id, needle) = (native_id.to_owned(), needle.to_owned());
        Box::pin(async move {
            let found = self
                .control(async |server: &Server| {
                    let mut query = "type=user&limit=100".to_owned();
                    // A long session's messages, a page at a time, newest first.
                    for _ in 0..100 {
                        let page = server
                            .call(
                                Method::GET,
                                &format!("/api/session/{id}/message?{query}"),
                                None,
                            )
                            .await?;
                        let messages = page["data"].as_array().cloned().unwrap_or_default();
                        if messages.iter().any(|message| {
                            message["type"] == "user"
                                && message["text"]
                                    .as_str()
                                    .is_some_and(|text| text.contains(&needle))
                        }) {
                            return Ok(true);
                        }
                        match page.pointer("/cursor/next").and_then(Value::as_str) {
                            Some(next) if !messages.is_empty() => {
                                query = format!("type=user&limit=100&cursor={next}");
                            }
                            _ => break,
                        }
                    }
                    Ok(false)
                })
                .await;
            found.unwrap_or_else(|err| {
                tracing::debug!(error = %err, "OpenCode didn't list the session's messages");
                false
            })
        })
    }

    /// The commands OpenCode lists for `folder` (its default location without one, or when
    /// the folder isn't trusted, so the project's own commands don't count).
    fn commands(
        &self,
        folder: Option<PathBuf>,
        trusted: bool,
    ) -> BoxFuture<'_, Result<Vec<CliCommand>>> {
        Box::pin(async move {
            let folder = folder
                .filter(|_| trusted)
                .unwrap_or_else(|| self.platform.paths().data_dir.clone());
            self.control(async |server: &Server| list_commands(server, &folder).await)
                .await
        })
    }
}

impl OpenCode {
    /// Records session `id`'s transcript at `path`, then writes what it holds so far (a resumed
    /// or forked session's history).
    async fn keep_transcript(
        &self,
        server: &Server,
        id: &str,
        path: &Path,
        ledger: &dyn Ledger,
    ) -> Result<()> {
        ledger
            .record(Artifact::CliFile {
                provider: ProviderKind::OpenCode,
                kind: TRANSCRIPT.into(),
                path: path.display().to_string(),
                identities: Vec::new(),
            })
            .await?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        write_transcript(server, id, path).await
    }
}

/// Creates, resumes or forks `spec`'s session at its level, model and instructions, recording
/// it. Returns its id, its model and that model's context window.
async fn open_session(
    server: &Server,
    spec: &SessionSpec,
    cwd: &Path,
    ledger: &dyn Ledger,
) -> Result<(String, Option<String>, Option<i64>)> {
    let folders: Vec<String> = spec
        .add_dirs
        .iter()
        .map(|dir| crate::policy::real_path(dir).unwrap_or_else(|| dir.clone()))
        .map(|dir| dir.display().to_string())
        .collect();
    let permissions = parse::permissions(spec.access, spec.tools, &folders);
    // No model named: OpenCode's own default (D18), at the effort asked for, if any.
    let model = match (&spec.model, &spec.effort) {
        (Some(model), effort) => Some(model_ref(model, effort.as_deref())?),
        (None, Some(effort)) => {
            let default = server.call(Method::GET, "/api/model/default", None).await?;
            model_id(&default["data"])
                .map(|id| model_ref(&id, Some(effort)))
                .transpose()?
        }
        (None, None) => None,
    };
    let session = match &spec.origin {
        // OpenCode names its own sessions: a chosen id is recorded by the caller as it comes.
        Origin::New | Origin::NewAs { .. } => {
            let mut body = json!({
                "location": { "directory": cwd.display().to_string() },
                "permissions": permissions,
            });
            if let Some(model) = &model {
                body["model"] = model.clone();
            }
            server
                .call(Method::POST, "/api/session", Some(&body))
                .await?
        }
        Origin::Resume { native_id } | Origin::Fork { native_id } => {
            let opened = if matches!(spec.origin, Origin::Fork { .. }) {
                let path = format!("/api/session/{native_id}/fork");
                server.call(Method::POST, &path, Some(&json!({}))).await?
            } else {
                let path = format!("/api/session/{native_id}");
                server.call(Method::GET, &path, None).await?
            };
            let id = opened["data"]["id"].as_str().unwrap_or_default();
            let path = format!("/api/session/{id}");
            let body = json!({ "permissions": permissions });
            server.call(Method::PATCH, &path, Some(&body)).await?;
            if let Some(model) = &model {
                let path = format!("/api/session/{id}/model");
                let body = json!({ "model": model });
                server.call(Method::POST, &path, Some(&body)).await?;
            }
            opened
        }
    };
    let info = &session["data"];
    let id = info["id"]
        .as_str()
        .filter(|id| id.starts_with("ses"))
        .ok_or_else(|| Error::Protocol("OpenCode opened no session".into()))?
        .to_owned();
    ledger
        .record(Artifact::Session {
            provider: ProviderKind::OpenCode,
            id: id.clone(),
            home: None,
            cwd: Some(cwd.display().to_string()),
        })
        .await?;
    if let Some(instructions) = &spec.append_system_prompt {
        let path =
            format!("/api/experimental/session/{id}/instructions/entries/{INSTRUCTIONS_KEY}");
        let body = json!({ "value": instructions });
        server.call(Method::PUT, &path, Some(&body)).await?;
    }
    let named = model.as_ref().unwrap_or(&info["model"]);
    let model = model_id(named);
    let window = match &model {
        Some(model) => list_models(server)
            .await?
            .iter()
            .find(|listed| model_id(listed).as_ref() == Some(model))
            .and_then(|listed| listed.pointer("/limit/context").and_then(Value::as_i64)),
        None => None,
    };
    Ok((id, model, window))
}

/// A session's `opencode serve`, and the HTTP client that speaks to it.
struct Server {
    process: Arc<CliProcess>,
    http: reqwest::Client,
    url: String,
    password: String,
    recorder: Option<Arc<Recorder>>,
}

impl Server {
    /// Spawns `opencode serve` (with `spec`'s program, folder and environment) on a free
    /// loopback port behind a fresh password, and waits until it says where it listens.
    async fn start(
        platform: Arc<dyn Platform>,
        mut spec: brigadier_sandbox::SpawnSpec,
        options: process::Options,
        recorder: Option<Arc<Recorder>>,
    ) -> Result<Self> {
        let password = uuid::Uuid::new_v4().simple().to_string();
        spec.args = ["serve", "--stdio", "--hostname", "127.0.0.1", "--port", "0"]
            .into_iter()
            .map(Into::into)
            .collect();
        crate::cli::apply_session_env(
            &mut spec,
            &[("OPENCODE_SERVER_PASSWORD".into(), password.clone())],
            &[],
        );
        let process::Spawned {
            process,
            mut stdout,
        } = process::spawn(platform, &spec, options)?;
        let first = tokio::time::timeout(START_TIMEOUT, stdout.recv()).await;
        // What it prints later (its logs) is read and dropped, so it never blocks on a full pipe.
        tokio::spawn(async move { while stdout.recv().await.is_some() {} });
        let url = match first {
            Ok(Some(line)) => serde_json::from_str::<Value>(&line)
                .ok()
                .and_then(|said| said["url"].as_str().map(str::to_owned)),
            _ => None,
        };
        let Some(url) = url else {
            process.shutdown(EXIT_GRACE).await;
            return Err(Error::Spawn(match process.stderr_tail() {
                Some(tail) => format!("OpenCode's server didn't start: {tail}"),
                None => "OpenCode's server didn't start".into(),
            }));
        };
        let http = reqwest::Client::builder()
            .no_proxy()
            .build()
            .map_err(|err| Error::Spawn(err.to_string()))?;
        Ok(Self {
            process,
            http,
            url,
            password,
            recorder,
        })
    }

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.url))
            .basic_auth("opencode", Some(&self.password))
    }

    /// Calls the API; an answer that isn't a success is [`Error::Rejected`] with its status and
    /// OpenCode's words.
    async fn call(&self, method: Method, path: &str, body: Option<&Value>) -> Result<Value> {
        if let Some(recorder) = &self.recorder {
            let sent = json!({ "method": method.as_str(), "path": path, "body": body });
            recorder.record(Direction::In, &sent.to_string());
        }
        let mut request = self.request(method, path).timeout(REQUEST_TIMEOUT);
        if let Some(body) = body {
            request = request.json(body);
        }
        let failed = |err: reqwest::Error| match self.process.is_running() {
            true => Error::Protocol(format!("OpenCode didn't answer: {err}")),
            false => Error::Closed,
        };
        let response = request.send().await.map_err(failed)?;
        let status = response.status();
        let text = response.text().await.map_err(failed)?;
        if !status.is_success() {
            let said: Value = serde_json::from_str(&text).unwrap_or_default();
            let message = said["message"].as_str().unwrap_or(&text);
            return Err(Error::Rejected(format!("{status}: {message}")));
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }
}

/// Reads the server's event stream into the session's events until the server exits, then
/// reports how it exited. `connected` fires once the stream is open. Keeps the session's
/// transcript at `transcript` after each turn and compaction, before the turn's end is told,
/// and records each command output file OpenCode keeps.
async fn read_loop(
    session: Arc<OpenCodeSession>,
    ledger: Arc<dyn Ledger>,
    transcript: PathBuf,
    connected: oneshot::Sender<()>,
) {
    let server = &session.server;
    let mut connected = Some(connected);
    while server.process.is_running() {
        let response = server.request(Method::GET, "/api/event").send().await;
        let Ok(mut response) = response.and_then(reqwest::Response::error_for_status) else {
            tokio::time::sleep(Duration::from_millis(250)).await;
            continue;
        };
        if let Some(connected) = connected.take() {
            let _ = connected.send(());
        }
        let mut pending = Vec::new();
        while let Ok(Some(chunk)) = response.chunk().await {
            pending.extend_from_slice(&chunk);
            while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = pending.drain(..=end).collect();
                let line = String::from_utf8_lossy(&line);
                let Some(data) = line.trim_end().strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim_start();
                if let Some(recorder) = &server.recorder {
                    recorder.record(Direction::Out, data);
                }
                let out = lock(&session.parser).feed(data);
                session.noticed(data, &ledger, &transcript).await;
                for event in out {
                    session.events.send(event).await;
                }
            }
        }
    }
    let exit = server.process.exited().await;
    // A turn the server took with it ends here.
    if lock(&session.parser).running() {
        let ended = lock(&session.parser)
            .feed(&json!({"type": "session.execution.interrupted", "data": {}}).to_string());
        for event in ended {
            session.events.send(event).await;
        }
    }
    session
        .events
        .send(ProviderEvent::Exited {
            code: exit.code,
            signal: exit.signal,
            stderr_tail: (exit.code != Some(0))
                .then(|| server.process.stderr_tail())
                .flatten(),
        })
        .await;
}

pub struct OpenCodeSession {
    server: Arc<Server>,
    id: String,
    cwd: PathBuf,
    parser: Arc<Mutex<Parser>>,
    events: Arc<Events>,
    window: Option<i64>,
    /// The inbox items [`ProviderSession::steer_running`] gave the running turn that it hasn't
    /// taken yet.
    steered: Arc<Mutex<Vec<String>>>,
}

impl OpenCodeSession {
    /// Admits `input` into the session at OpenCode's `steer` delivery: it joins the running
    /// turn at its next step, or starts one when `resume` and none runs. Answers its inbox id.
    async fn prompt(&self, input: &TurnInput, resume: bool) -> Result<String> {
        if input.is_empty() {
            return Err(Error::Invalid("the message is empty".into()));
        }
        let mut text = Vec::new();
        let mut files = Vec::new();
        let mut skills = Vec::new();
        for part in input.parts_with_file_notes() {
            match part {
                InputPart::Text(part) if !part.trim().is_empty() => text.push(part),
                InputPart::Text(_) => {}
                InputPart::Skill { invocation, .. } => skills.push(json!({ "id": invocation })),
                InputPart::Image(file) => files.push(json!({
                    "uri": format!("file://{}", file.path.display()),
                    "name": file.name,
                })),
            }
        }
        let body = json!({
            "text": text.join("\n\n"),
            "files": files,
            "skills": skills,
            "delivery": "steer",
            "resume": resume,
        });
        let path = format!("/api/session/{}/prompt", self.id);
        let admitted = self.server.call(Method::POST, &path, Some(&body)).await?;
        Ok(admitted["data"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_owned())
    }

    fn running(&self) -> bool {
        lock(&self.parser).running()
    }

    /// Whether OpenCode runs a turn of the session now (its own word, which a turn just asked
    /// for may not have reached the stream yet).
    async fn active(&self) -> Result<bool> {
        let active = self
            .server
            .call(Method::GET, "/api/session/active", None)
            .await?;
        Ok(active["data"].get(&self.id).is_some())
    }

    /// What an event line means beyond its events: a command's output file is recorded; a
    /// delivered steer is taken; at a turn's or compaction's end the transcript is written, and
    /// a steer the ended turn didn't take starts the next one, as it does on the other CLIs.
    async fn noticed(&self, line: &str, ledger: &Arc<dyn Ledger>, transcript: &Path) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let data = &event["data"];
        let kind = event["type"].as_str().unwrap_or_default();
        match kind {
            // Every shell on this server is this session's or one of its sub-agents'.
            "shell.created" => {
                if let Some(file) = data.pointer("/info/file").and_then(Value::as_str) {
                    let recorded = ledger
                        .record(Artifact::CliFile {
                            provider: ProviderKind::OpenCode,
                            kind: SHELL_OUTPUT.into(),
                            path: file.to_owned(),
                            identities: Vec::new(),
                        })
                        .await;
                    if let Err(err) = recorded {
                        tracing::warn!(error = %err, "could not record OpenCode's command output");
                    }
                }
            }
            "session.inbox.delivered" => {
                let item = data["inboxID"].as_str().unwrap_or_default();
                lock(&self.steered).retain(|steered| steered != item);
            }
            "session.execution.succeeded"
            | "session.execution.failed"
            | "session.execution.interrupted"
            | "session.compaction.ended"
                if data["sessionID"] == self.id.as_str() =>
            {
                if let Err(err) = write_transcript(&self.server, &self.id, transcript).await {
                    tracing::warn!(error = %err, "could not keep OpenCode's transcript");
                }
                // An interrupted turn's steer waits for the next message instead.
                if !kind.starts_with("session.execution.")
                    || kind == "session.execution.interrupted"
                {
                    return;
                }
                let untaken = std::mem::take(&mut *lock(&self.steered));
                for item in untaken {
                    let path = format!("/api/session/{}/inbox/{item}", self.id);
                    let body = json!({ "delivery": "steer" });
                    if let Err(err) = self.server.call(Method::PATCH, &path, Some(&body)).await {
                        tracing::warn!(error = %err, "could not deliver a steer to OpenCode");
                    }
                }
            }
            _ => {}
        }
    }
}

impl ProviderSession for OpenCodeSession {
    fn native_id(&self) -> String {
        self.id.clone()
    }

    fn context_window(&self) -> Option<i64> {
        self.window
    }

    fn send(&self, input: TurnInput) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move { self.prompt(&input, true).await.map(drop) })
    }

    fn steer(&self, input: TurnInput) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move { self.prompt(&input, true).await.map(drop) })
    }

    /// Admitted without starting a turn (`resume: false`); when no turn runs to take it, it is
    /// taken back out of the session's inbox before anything delivers it.
    fn steer_running(&self, input: TurnInput) -> BoxFuture<'_, Result<bool>> {
        Box::pin(async move {
            let item = self.prompt(&input, false).await?;
            if self.running() || self.active().await? {
                lock(&self.steered).push(item);
                return Ok(true);
            }
            let path = format!("/api/session/{}/inbox/{item}", self.id);
            self.server.call(Method::DELETE, &path, None).await?;
            Ok(false)
        })
    }

    fn interrupt(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if !self.running() {
                return Ok(());
            }
            let path = format!("/api/session/{}/interrupt", self.id);
            self.server
                .call(Method::POST, &path, Some(&json!({})))
                .await
                .map(drop)
        })
    }

    fn can_compact(&self) -> bool {
        true
    }

    /// OpenCode's compaction takes no instructions.
    fn compact(&self, _instructions: Option<String>) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if self.running() {
                return Err(Error::Invalid(
                    "OpenCode is still answering; compact once it is done".into(),
                ));
            }
            let path = format!("/api/session/{}/compact", self.id);
            self.server
                .call(Method::POST, &path, Some(&json!({})))
                .await
                .map(drop)
        })
    }

    fn commands(&self) -> BoxFuture<'_, Vec<CliCommand>> {
        Box::pin(async move {
            list_commands(&self.server, &self.cwd)
                .await
                .unwrap_or_else(|err| {
                    tracing::warn!(error = %err, "OpenCode didn't list its commands");
                    Vec::new()
                })
        })
    }

    fn answer(&self, approval_id: String, decision: ApprovalDecision) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let session = lock(&self.parser)
                .asked
                .get(&approval_id)
                .map(|(session, _)| session.clone())
                .ok_or_else(|| Error::Invalid(format!("no pending approval {approval_id}")))?;
            // OpenCode's "always" is saved for good; Brigadier's grants last a conversation.
            let body = match decision {
                ApprovalDecision::Allow => json!({ "decision": "once" }),
                ApprovalDecision::AllowSimilar => {
                    return Err(Error::Invalid(
                        "this request has no \"don't ask again\" option".into(),
                    ));
                }
                ApprovalDecision::Deny { message } if message.trim().is_empty() => {
                    json!({ "decision": "reject" })
                }
                ApprovalDecision::Deny { message } => {
                    json!({ "decision": "reject", "message": message })
                }
            };
            let path = format!("/api/session/{session}/permission/{approval_id}/reply");
            self.server
                .call(Method::POST, &path, Some(&body))
                .await
                .map(drop)
        })
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.server.process.shutdown(EXIT_GRACE).await;
        })
    }

    fn is_running(&self) -> bool {
        self.server.process.is_running()
    }
}

struct OpenCodeReplayer {
    parser: Parser,
}

impl Replayer for OpenCodeReplayer {
    fn feed(&mut self, dir: Direction, line: &str) -> Vec<ProviderEvent> {
        match dir {
            Direction::Out => self.parser.feed(line),
            Direction::In => {
                // The session prompted is the recording's own: a sub-agent's events are left
                // out, as live.
                if self.parser.session.is_none() {
                    let sent: Value = serde_json::from_str(line).unwrap_or_default();
                    self.parser.session = sent["path"]
                        .as_str()
                        .and_then(|path| {
                            path.strip_prefix("/api/session/")?.strip_suffix("/prompt")
                        })
                        .map(str::to_owned);
                }
                Vec::new()
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A replay keeps to the session it prompted: a sub-agent's text and its turn's end are
    /// not the recording's.
    #[test]
    fn a_replay_leaves_out_a_sub_agents_events() {
        let line = |kind: &str, data: Value| {
            json!({"id": "evt", "created": 1, "type": kind, "data": data}).to_string()
        };
        // As `replayer` makes it: no session known until the recording names it.
        let mut replay = OpenCodeReplayer {
            parser: Parser::new(None, None),
        };
        let prompt = json!({"method": "POST", "path": "/api/session/ses_1/prompt", "body": {}});
        assert!(replay.feed(Direction::In, &prompt.to_string()).is_empty());
        let events: Vec<_> = [
            line("session.execution.started", json!({"sessionID": "ses_1"})),
            line("session.execution.started", json!({"sessionID": "ses_sub"})),
            line("session.text.ended", json!({"sessionID": "ses_sub", "assistantMessageID": "m2", "ordinal": 0, "text": "child"})),
            line("session.execution.succeeded", json!({"sessionID": "ses_sub"})),
        ]
        .iter()
        .flat_map(|line| replay.feed(Direction::Out, line))
        .collect();
        assert!(
            !events.iter().any(|event| matches!(
                event,
                ProviderEvent::Message { .. } | ProviderEvent::TurnCompleted { .. }
            )),
            "{events:?}"
        );
    }

    fn spec() -> SessionSpec {
        SessionSpec {
            cwd: PathBuf::from("/w"),
            model: None,
            effort: None,
            fast: false,
            origin: Origin::New,
            ephemeral: false,
            access: Access::Ask,
            append_system_prompt: None,
            mcp_servers: Vec::new(),
            tools: ToolSet::Lean,
            add_dirs: Vec::new(),
            project: None,
            skill_plugins: Vec::new(),
            installed_skills: Vec::new(),
            switched_off: SwitchedOff::default(),
            env: vec![("TMPDIR".into(), "/t".into())],
            unset_env: Vec::new(),
            low_priority: false,
            record_to: None,
            redactor: None,
            owned_cwd: false,
            sweep_dir: None,
            keeps_detached: false,
            auto_compact: true,
            allowed_models: None,
            omit_ai_coauthors: false,
            output_hook: None,
            stop_hook: None,
            naked_worker: false,
            native_compaction: None,
            temp_tag: None,
        }
    }

    fn var<'a>(env: &'a [(String, String)], name: &str) -> Option<&'a str> {
        env.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn a_naked_worker_runs_with_empty_private_folders_and_only_its_grants() {
        let mut worker = spec();
        worker.naked_worker = true;
        worker.mcp_servers = vec![McpServer {
            gateway: None,
            name: "brigadier".into(),
            transport: McpTransport::Http {
                url: "http://127.0.0.1:9/mcp".into(),
            },
            env: vec![("Authorization".into(), "Bearer s".into())],
            tool_timeout_secs: Some(600),
            trusted: true,
            always_load: true,
        }];
        worker.installed_skills = vec![PathBuf::from("/skills/brigade")];
        assert!(sealed(&worker));
        let env = session_env(&worker, Some(Path::new("/data/opencode-1")));
        assert_eq!(
            var(&env, "OPENCODE_CONFIG_DIR"),
            Some("/data/opencode-1/config")
        );
        assert_eq!(
            var(&env, "OPENCODE_TEST_HOME"),
            Some("/data/opencode-1/home")
        );
        assert_eq!(var(&env, "TMPDIR"), Some("/t"));
        let config: Value =
            serde_json::from_str(var(&env, "OPENCODE_CONFIG_CONTENT").unwrap()).unwrap();
        assert_eq!(
            config["mcp"]["servers"]["brigadier"],
            json!({"type": "remote", "url": "http://127.0.0.1:9/mcp", "headers": {"Authorization": "Bearer s"}, "oauth": false, "timeout": {"execution": 600_000}, "codemode": false})
        );
        assert_eq!(config["skills"], json!(["/skills/brigade/skills"]));
        assert!(config.get("compaction").is_none());

        // A thread keeps the user's own setup; one that must not compact doesn't.
        let mut thread = spec();
        thread.tools = ToolSet::Thread;
        thread.auto_compact = false;
        assert!(!sealed(&thread));
        let env = session_env(&thread, None);
        assert!(var(&env, "OPENCODE_CONFIG_DIR").is_none());
        let config: Value =
            serde_json::from_str(var(&env, "OPENCODE_CONFIG_CONTENT").unwrap()).unwrap();
        assert_eq!(config["compaction"], json!({"auto": false}));
        // A one-shot reader never loads the user's setup.
        let mut reader = spec();
        reader.tools = ToolSet::Review;
        assert!(sealed(&reader));
    }

    /// With no provider signed in, a model OpenCode lists needing no credential (its free
    /// ones, as `opencode serve` 2.2.3 lists them) still makes it ready.
    #[test]
    fn a_credential_free_model_makes_it_ready() {
        let env = CliEnv::from_vars([("OPENCODE_API_KEY".into(), "".into())]);
        let none = json!({"data": [{"id": "opencode", "name": "OpenCode Console", "connections": [],
            "methods": [{"type": "key"}, {"type": "env", "names": ["OPENCODE_API_KEY"]}]}]});
        let free = json!({"id": "big-pickle", "providerID": "opencode", "settings": {"apiKey": "public"}, "enabled": true});
        assert_eq!(signed_in(&none, &[], &env), (false, None));
        assert_eq!(signed_in(&none, &[free], &env), (true, None));
        let off = json!({"id": "big-pickle", "providerID": "opencode", "enabled": false});
        assert_eq!(signed_in(&none, &[off], &env), (false, None));
        let keyed = CliEnv::from_vars([("OPENCODE_API_KEY".into(), "k".into())]);
        assert_eq!(
            signed_in(&none, &[], &keyed),
            (true, Some("OpenCode Console".into()))
        );
    }

    #[test]
    fn a_model_is_named_by_provider_and_takes_its_effort_as_variant() {
        assert_eq!(
            model_ref("opencode/flint-40b-free", Some("high")).unwrap(),
            json!({"providerID": "opencode", "id": "flint-40b-free", "variant": "high"})
        );
        assert_eq!(
            model_ref("openrouter/anthropic/x", None).unwrap(),
            json!({"providerID": "openrouter", "id": "anthropic/x"})
        );
        assert!(model_ref("flint", None).is_err());
        let listed = json!({"id": "flint-40b-free", "providerID": "opencode", "name": "Flint", "variants": [{"id": "low"}, {"id": "high"}], "capabilities": {"input": ["text"]}});
        let info = model_info(&listed, Some("opencode/flint-40b-free")).unwrap();
        assert_eq!(info.id, "opencode/flint-40b-free");
        assert_eq!(info.efforts, ["low", "high"]);
        assert!(info.is_default);
    }

    /// A stand-in for `opencode serve --stdio`: says where it listens, answers the session's
    /// requests as OpenCode 2.0.19 does (`ACTIVE` set: a turn runs) and logs each one.
    #[cfg(unix)]
    const FAKE_SERVER: &str = r#"#!/usr/bin/env python3
import http.server, json, os, sys, threading
log = open(os.environ["LOG"], "a")
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def answer(self, code, body=None):
        self.send_response(code); self.send_header("content-type", "application/json"); self.end_headers()
        if body is not None: self.wfile.write(json.dumps(body).encode())
    def handle_any(self):
        n = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        log.write(f"{self.command} {self.path} {body}\n"); log.flush()
        if self.path == "/api/session/active":
            self.answer(200, {"data": {"ses_1": {"type": "running"}} if os.path.exists(os.environ["ACTIVE"]) else {}})
        elif self.path.endswith("/prompt"):
            self.answer(200, {"data": {"id": "msg_1", "type": "user", "delivery": "steer"}})
        elif self.path.startswith("/api/command"):
            self.answer(200, {"location": {}, "data": [{"name": "init", "description": "guided AGENTS.md setup"}, {"name": "review"}]})
        elif self.command == "DELETE":
            self.answer(204)
        else:
            self.answer(404, {"message": "not here"})
    do_GET = do_POST = do_DELETE = do_PATCH = handle_any
server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
print(json.dumps({"url": f"http://127.0.0.1:{server.server_address[1]}"}), flush=True)
threading.Thread(target=server.serve_forever, daemon=True).start()
sys.stdin.read()
"#;

    /// A turn that isn't running never takes a steer: OpenCode admits it without starting one,
    /// and it is taken back out. A running turn keeps it. The `/` commands are OpenCode's own.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_steer_without_a_running_turn_is_refused_and_commands_are_opencodes() {
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(format!("brigadier-opencode-fake-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("opencode");
        std::fs::write(&script, FAKE_SERVER).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (log, active) = (root.join("log"), root.join("active"));
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(root.join("data")),
        })
        .unwrap();
        let mut spec = brigadier_sandbox::SpawnSpec::new(script.display().to_string());
        spec.env.push(("LOG".into(), log.clone().into()));
        spec.env.push(("ACTIVE".into(), active.clone().into()));
        let server = Server::start(platform, spec, process::Options::default(), None)
            .await
            .unwrap();
        let (events, _rx) = mpsc::channel(8);
        let session = OpenCodeSession {
            server: Arc::new(server),
            id: "ses_1".into(),
            cwd: root.clone(),
            parser: Arc::new(Mutex::new(Parser::new(Some("ses_1".into()), None))),
            events: Arc::new(Events::new(events, None)),
            window: None,
            steered: Arc::new(Mutex::new(Vec::new())),
        };
        let said = || std::fs::read_to_string(&log).unwrap();

        assert!(
            !session
                .steer_running(TurnInput::text("also this"))
                .await
                .unwrap()
        );
        let requests = said();
        assert!(
            requests.contains(r#""resume":false"#) && requests.contains(r#""delivery":"steer""#),
            "{requests}"
        );
        assert!(
            requests.contains("DELETE /api/session/ses_1/inbox/msg_1"),
            "{requests}"
        );
        assert!(lock(&session.steered).is_empty());

        std::fs::write(&active, "").unwrap();
        std::fs::write(&log, "").unwrap();
        assert!(
            session
                .steer_running(TurnInput::text("also this"))
                .await
                .unwrap()
        );
        assert!(!said().contains("DELETE"), "a running turn keeps it");
        assert_eq!(*lock(&session.steered), ["msg_1"]);

        let commands = session.commands().await;
        assert_eq!(
            commands
                .iter()
                .map(|c| (
                    c.name.as_str(),
                    c.text.as_str(),
                    c.invocation.as_deref(),
                    c.kind
                ))
                .collect::<Vec<_>>(),
            [
                ("init", "/init", Some("init"), Some(CliCommandKind::Command)),
                (
                    "review",
                    "/review",
                    Some("review"),
                    Some(CliCommandKind::Command)
                )
            ]
        );
        assert_eq!(
            commands[0].description.as_deref(),
            Some("guided AGENTS.md setup")
        );
        assert!(said().contains("GET /api/command?location%5Bdirectory%5D="));
        session.close().await;
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn remove_takes_only_files_named_and_placed_as_opencode_makes_them() {
        let shell = "/u/.local/share/opencode/shell/abc/sh_18c9d4c15002tCmpLdYKCr2Cav.out";
        assert!(removable(SHELL_OUTPUT, Path::new(shell)));
        assert!(!removable(
            SHELL_OUTPUT,
            Path::new("/u/.local/share/opencode/shell/abc")
        ));
        assert!(!removable(SHELL_OUTPUT, Path::new("/u/notes/sh_1.out")));
        assert!(removable(TRANSCRIPT, Path::new("/d/opencode/ses_1.jsonl")));
        assert!(!removable(TRANSCRIPT, Path::new("/d/opencode/other.jsonl")));
        assert!(!removable("other", Path::new("/d/opencode/ses_1.jsonl")));
    }

    /// Live: a real session on a free model runs a command (asked, allowed), takes a steer
    /// mid-turn and refuses one with no turn running, is stopped, reads back from its
    /// transcript as the live rows, resumes on a fresh server with the same rows, and is removed
    /// with its files (its project's shell folder stays). `opencode/big-pickle`:
    /// `cargo test -p brigadier-providers opencode -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "runs the installed opencode against its free model"]
    async fn live_session_runs_steers_reads_back_resumes_and_is_removed() {
        use crate::cli::CliEnv;
        use crate::session_file::{FileEntry, item_id};
        struct Kept(Mutex<Vec<Artifact>>);
        impl Ledger for Kept {
            fn record(&self, artifact: Artifact) -> BoxFuture<'_, Result<()>> {
                lock(&self.0).push(artifact);
                Box::pin(async { Ok(()) })
            }
            fn holds(&self, artifact: &Artifact) -> bool {
                lock(&self.0).contains(artifact)
            }
        }
        /// Until the turn ends: allows what it asks, keeps the ids of its finished rows, and
        /// runs `mid` once at its first command.
        async fn wait(
            session: &dyn ProviderSession,
            events: &mut mpsc::Receiver<ProviderEvent>,
            rows: &mut Vec<String>,
            mut mid: Option<TurnInput>,
        ) -> (TurnStatus, Option<bool>) {
            let mut steered = None;
            let mut commands = Vec::new();
            loop {
                let event = tokio::time::timeout(Duration::from_secs(300), events.recv()).await;
                let event = match event {
                    Ok(Some(event)) => event,
                    other => panic!("no turn end: {other:?}"),
                };
                if let ProviderEvent::Command { command, .. } = &event {
                    commands.push(command.clone());
                }
                match &event {
                    ProviderEvent::ApprovalRequested { request } => {
                        // A command's card shows the command its call runs, not its parts.
                        if request.kind == ApprovalKind::Command {
                            let asked = request.command.clone().unwrap_or_default();
                            assert!(commands.contains(&asked), "{asked:?} not in {commands:?}");
                        }
                        session
                            .answer(request.id.clone(), ApprovalDecision::Allow)
                            .await
                            .unwrap();
                    }
                    ProviderEvent::Command { .. } if mid.is_some() => {
                        steered = Some(session.steer_running(mid.take().unwrap()).await.unwrap());
                    }
                    ProviderEvent::TurnCompleted { status, .. } => return (*status, steered),
                    _ => {}
                }
                if let Some(id) = crate::session_file::item_id(&event)
                    && !matches!(
                        event,
                        ProviderEvent::MessageDelta { .. } | ProviderEvent::ReasoningDelta { .. }
                    )
                    && !rows.iter().any(|row| row == id)
                {
                    rows.push(id.to_owned());
                }
            }
        }
        let user_rows = |entries: &[FileEntry]| -> Vec<(String, bool)> {
            entries
                .iter()
                .filter_map(|entry| match &entry.event {
                    ProviderEvent::Message {
                        role: Role::User,
                        text,
                        ..
                    } => Some((text.clone(), entry.steer)),
                    _ => None,
                })
                .collect()
        };
        let root = std::env::temp_dir().join(format!("brigadier-opencode-live-{}", now_ms()));
        let dir = root.join("work");
        std::fs::create_dir_all(&dir).unwrap();
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(root.join("data")),
        })
        .unwrap();
        let env = Arc::new(CliEnv::capture(&platform));
        let opencode = OpenCode::new(platform, env);
        let ledger = Arc::new(Kept(Mutex::new(Vec::new())));
        let mut first = spec();
        first.cwd = dir.clone();
        first.env = Vec::new();
        first.model = Some("opencode/big-pickle".into());
        first.naked_worker = true;
        first.append_system_prompt = Some("Be brief. Use your shell tool when asked.".into());
        let Started {
            session,
            mut events,
        } = opencode.start(first.clone(), ledger.clone()).await.unwrap();
        let id = session.native_id();
        let transcript = loop {
            if let Some(ProviderEvent::SessionStarted { file, .. }) = events.recv().await {
                break PathBuf::from(file.unwrap());
            }
        };
        let mut live = Vec::new();

        // At Ask, a command waits for the user's answer.
        let ask = "Run the shell command `echo ok > ok.txt` with your shell tool, then reply DONE.";
        session.send(TurnInput::text(ask)).await.unwrap();
        assert_eq!(
            wait(session.as_ref(), &mut events, &mut live, None).await.0,
            TurnStatus::Completed
        );
        assert!(dir.join("ok.txt").exists());
        assert!(opencode.took_input(&id, "echo ok > ok.txt").await);
        assert!(!opencode.took_input(&id, "never sent").await);

        // No turn runs: the steer is refused, and nothing starts.
        assert!(
            !session
                .steer_running(TurnInput::text("STRAY STEER"))
                .await
                .unwrap()
        );

        // A turn running a command takes a steer.
        let slow =
            "Run the shell command `sleep 8; echo one` with your shell tool, then reply DONE.";
        let steer = "Also end your final reply with the word BANANA.";
        session.send(TurnInput::text(slow)).await.unwrap();
        let (status, steered) = wait(
            session.as_ref(),
            &mut events,
            &mut live,
            Some(TurnInput::text(steer)),
        )
        .await;
        assert_eq!((status, steered), (TurnStatus::Completed, Some(true)));

        // Stopped mid-turn.
        session
            .send(TurnInput::text(
                "Count slowly from 1 to 500, one number per line.",
            ))
            .await
            .unwrap();
        loop {
            match events.recv().await {
                Some(ProviderEvent::TurnStarted { .. }) => break,
                Some(_) => {}
                None => panic!("ended"),
            }
        }
        session.interrupt().await.unwrap();
        assert_eq!(
            wait(session.as_ref(), &mut events, &mut live, None).await.0,
            TurnStatus::Interrupted
        );
        session.close().await;
        assert!(!session.is_running());

        // Read back as core does after a restart: the same rows, by the same ids.
        let read = crate::session_file::read(ProviderKind::OpenCode, &transcript).unwrap();
        eprintln!(
            "live rows {live:?}\nread back {:#?}",
            read.iter()
                .map(|e| (item_id(&e.event), e.steer))
                .collect::<Vec<_>>()
        );
        for row in &live {
            assert!(
                read.iter()
                    .any(|entry| item_id(&entry.event) == Some(row.as_str())),
                "{row} not read back"
            );
        }
        // The stream said each user message, the steer's too, by its id in the transcript.
        for entry in &read {
            if let ProviderEvent::Message {
                role: Role::User,
                item_id: id,
                ..
            } = &entry.event
            {
                assert!(live.contains(id), "{id} wasn't said live");
            }
        }
        assert_eq!(
            user_rows(&read)[..3],
            [
                (ask.to_owned(), false),
                (slow.to_owned(), false),
                (steer.to_owned(), true)
            ]
        );
        assert!(
            !user_rows(&read)
                .iter()
                .any(|(text, _)| text.contains("STRAY"))
        );
        assert!(read.iter().any(|entry| matches!(&entry.event, ProviderEvent::Command { command, .. } if command.contains("sleep 8"))));
        assert!(read.iter().any(|entry| matches!(
            entry.event,
            ProviderEvent::TurnCompleted {
                status: TurnStatus::Interrupted,
                ..
            }
        )));

        // A fresh server resumes it and writes the same transcript.
        let mut again = first;
        again.origin = Origin::Resume {
            native_id: id.clone(),
        };
        let Started {
            session,
            mut events,
        } = opencode.start(again, ledger.clone()).await.unwrap();
        assert_eq!(session.native_id(), id);
        assert_eq!(
            crate::session_file::read(ProviderKind::OpenCode, &transcript).unwrap(),
            read
        );
        session
            .send(TurnInput::text("reply DONE again"))
            .await
            .unwrap();
        assert_eq!(
            wait(session.as_ref(), &mut events, &mut Vec::new(), None)
                .await
                .0,
            TurnStatus::Completed
        );
        session.close().await;

        let artifacts = lock(&ledger.0).clone();
        let shells: Vec<PathBuf> = artifacts
            .iter()
            .filter_map(|artifact| match artifact {
                Artifact::CliFile { kind, path, .. } if kind == SHELL_OUTPUT => {
                    Some(PathBuf::from(path))
                }
                _ => None,
            })
            .collect();
        assert!(
            !shells.is_empty() && shells.iter().all(|shell| shell.exists()),
            "{shells:?}"
        );
        opencode.remove(artifacts.clone()).await.unwrap();
        assert!(
            !opencode.took_input(&id, "echo ok").await,
            "the session is gone"
        );
        assert!(!transcript.exists());
        assert!(
            shells
                .iter()
                .all(|shell| !shell.exists() && shell.parent().unwrap().exists())
        );
        assert!(artifacts.iter().any(|a| matches!(a, Artifact::ScratchDir { path } if path.starts_with(&*root.to_string_lossy()))));
        std::fs::remove_dir_all(root).unwrap();
    }
}
