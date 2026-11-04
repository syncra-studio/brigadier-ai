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
//!
//! Its login is a row of that same database, so an extra account ([`crate::accounts`]) is a
//! database of its own (`OPENCODE_DB=<account>/opencode.db`), holding its login and a copy of
//! each session it ran. A session resumed or forked on an account that hasn't its newest copy is
//! carried there first ([`OpenCode::carry`]): exported from the database that has it, imported
//! under the same id. The user's own database is never written to: once a session's newest copy
//! is an extra account's, the user's own login doesn't take it back ([`Provider::resumes`]).

pub mod parse;

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
    // Its own server: the background one may be the user's, on their own database.
    sign_in: &["auth", "login", "--standalone"],
    // None: the account's login is in its database, which goes with its folder.
    sign_out: &[],
    home: Some(&HOME),
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

/// An extra account's database, in its folder.
const DB: &str = "opencode.db";
/// Each session opened on an extra account, in its folder: `<id>` holding when (ms).
const OPENED: &str = "opened";
/// The folder of [`OpenCode::kept_home`]: no account's id starts with a dot.
const KEPT: &str = ".kept";

static HOME: crate::accounts::Home = crate::accounts::Home {
    var: "OPENCODE_DB",
    file: Some(DB),
    default_dir: ".local/share/opencode",
    // The keys of its integrations that sign in with a login (its `GET /api/integration`,
    // 2.2.3). shortcut: GITHUB_TOKEN, GITLAB_TOKEN and DIGITALOCEAN_ACCESS_TOKEN stay, as the
    // session's own commands use them, so one in the environment signs Copilot, GitLab or
    // DigitalOcean in on every account; drop them once one of those logins has limits to switch.
    auth_vars: &[
        "OPENCODE_API_KEY",
        "OPENAI_API_KEY",
        "XAI_API_KEY",
        "POE_API_KEY",
        "SNOWFLAKE_CORTEX_TOKEN",
        "SNOWFLAKE_CORTEX_PAT",
    ],
    unset: &[],
    // Its config, agents and skills are in `~/.config/opencode`, which an account doesn't move.
    shared: &[],
    seed: &[],
    for_account: |platform, env, home| Arc::new(OpenCode::for_account(platform, env, home)),
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
    /// An extra account's: the user's own environment, whose database holds the sessions the
    /// account hasn't run.
    main_env: Option<Arc<CliEnv>>,
}

impl OpenCode {
    pub fn new(platform: Arc<dyn Platform>, env: Arc<CliEnv>) -> Self {
        Self {
            platform,
            env,
            main_env: None,
        }
    }

    /// The adapter for an extra account whose folder is `home` (made with
    /// [`crate::accounts::prepare_home`]); `env` is the user's own environment.
    pub fn for_account(platform: Arc<dyn Platform>, env: &CliEnv, home: &Path) -> Self {
        Self {
            platform,
            env: Arc::new(env.for_account(ProviderKind::OpenCode, home)),
            main_env: Some(Arc::new(env.clone())),
        }
    }

    /// An extra account's folder, recorded with its sessions.
    fn account_home(&self) -> Option<PathBuf> {
        self.main_env.as_ref()?;
        Some(Path::new(self.env.var(HOME.var)?).parent()?.to_owned())
    }

    /// The extra account whose database holds session `id`'s newest copy: the one it was
    /// opened on last. `None`: the user's own database (it never left it).
    fn newest_home(&self, id: &str) -> Option<PathBuf> {
        if !plain_id(id) {
            return None;
        }
        // Where the core keeps extra accounts' folders (`Runtime::account_home`).
        let accounts = self.platform.paths().data_dir.join("accounts");
        std::fs::read_dir(accounts)
            .ok()?
            .flatten()
            .filter_map(|entry| {
                let at = std::fs::read_to_string(entry.path().join(OPENED).join(id)).ok()?;
                Some((at.trim().parse::<i64>().ok()?, entry.path()))
            })
            .max_by_key(|(at, _)| *at)
            .map(|(_, home)| home)
    }

    /// Brings session `id`'s newest copy into this account's database, before it is resumed
    /// or forked here: exported from the database that has it and imported under the same id,
    /// after this database's older copy is deleted (an extra account's database is
    /// Brigadier's). The user's own database is never written: a session whose newest copy is
    /// elsewhere is refused there.
    async fn carry(
        &self,
        server: &Server,
        id: &str,
        cwd: &Path,
        ledger: &dyn Ledger,
    ) -> Result<()> {
        let newest = self.newest_home(id);
        let (Some(main_env), Some(own)) = (&self.main_env, self.account_home()) else {
            return match newest {
                None => Ok(()),
                Some(_) => Err(Error::Invalid(MOVED_ON.into())),
            };
        };
        if newest.as_ref() == Some(&own) {
            return Ok(());
        }
        let from = match &newest {
            Some(home) => Arc::new(main_env.for_account(ProviderKind::OpenCode, home)),
            None => main_env.clone(),
        };
        // Recorded first, so removing the session finds this copy too.
        ledger
            .record(Artifact::Session {
                provider: ProviderKind::OpenCode,
                id: id.to_owned(),
                home: Some(own.display().to_string()),
                cwd: Some(cwd.display().to_string()),
            })
            .await?;
        self.copy_in(server, &from, id, Some(cwd)).await
    }

    /// Imports session `id`'s copy from the database `from` names into `server`'s under the
    /// same id, after that database's older copy is deleted: in `cwd`, else where it ran.
    async fn copy_in(
        &self,
        server: &Server,
        from: &CliEnv,
        id: &str,
        cwd: Option<&Path>,
    ) -> Result<()> {
        let path = format!("/api/experimental/session/{id}/export");
        let copy = self
            .control_in(from, async |source: &Server| {
                source.call(Method::GET, &path, None).await
            })
            .await?;
        let copy = &copy["data"];
        if copy["info"]["id"].as_str() != Some(id) {
            return Err(Error::Protocol(format!(
                "OpenCode exported no session {id}"
            )));
        }
        delete_sessions(server, &[id.to_owned()]).await?;
        let location = match cwd {
            Some(cwd) => json!({ "directory": cwd.display().to_string() }),
            None => copy["info"]["location"].clone(),
        };
        let body = json!({
            "info": copy["info"],
            "messages": copy["messages"],
            "location": location,
        });
        server
            .call(
                Method::POST,
                "/api/experimental/session/import",
                Some(&body),
            )
            .await?;
        Ok(())
    }

    /// Where an extra account's sessions are kept when it is removed: a database of Brigadier's
    /// with no login, beside the accounts' folders, found like theirs (D111).
    fn kept_home(&self) -> PathBuf {
        self.platform.paths().data_dir.join("accounts").join(KEPT)
    }

    /// Notes that session `id` was opened on this extra account now: its newest copy is here.
    fn opened(&self, id: &str) -> Result<()> {
        let Some(home) = self.account_home().filter(|_| plain_id(id)) else {
            return Ok(());
        };
        let dir = home.join(OPENED);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(id), now_ms().to_string())?;
        Ok(())
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
        self.control_in(&self.env, work).await
    }

    /// [`Self::control`] in `env` (another account's database).
    async fn control_in<T>(
        &self,
        env: &CliEnv,
        work: impl AsyncFnOnce(&Server) -> Result<T>,
    ) -> Result<T> {
        let cwd = self.platform.paths().data_dir.clone();
        let mut spec = env.spec(&self.binary()?);
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

/// Brigadier's default OpenCode model (D42), while OpenCode lists it enabled.
const DEFAULT_MODEL: &str = "opencode-go/glm-5.3";

/// The model a session starts on when none is named: [`DEFAULT_MODEL`] among `models`, else
/// OpenCode's own default.
async fn default_model(server: &Server, models: &[Value]) -> Result<Option<String>> {
    if models
        .iter()
        .any(|model| model["enabled"] != false && model_id(model).as_deref() == Some(DEFAULT_MODEL))
    {
        return Ok(Some(DEFAULT_MODEL.into()));
    }
    let default = server.call(Method::GET, "/api/model/default", None).await?;
    Ok(model_id(&default["data"]))
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
const MOVED_ON: &str =
    "This chat went on on another OpenCode account, so this computer's login can't pick it up.";

/// Whether `id` is a session id as OpenCode makes them, safe as a file name.
fn plain_id(id: &str) -> bool {
    id.starts_with("ses_") && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Deletes sessions `ids` from `server`'s database; one it doesn't hold is fine.
async fn delete_sessions(server: &Server, ids: &[String]) -> Result<()> {
    for id in ids {
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
}

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
                    // An extra account by its own login only: the free models are every one's.
                    if self.main_env.is_some() {
                        return Ok((false, None));
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
                    let default = default_model(server, &models).await?;
                    Ok((models, default))
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
                        ..process::Options::default()
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
            let home = self.account_home().map(|home| home.display().to_string());
            let opened = match recorded {
                Ok(()) => tokio::time::timeout(START_TIMEOUT, async {
                    if let Origin::Resume { native_id } | Origin::Fork { native_id } = &spec.origin
                    {
                        self.carry(&server, native_id, &cwd, ledger.as_ref())
                            .await?;
                    }
                    let opened = open_session(&server, &spec, &cwd, home, ledger.as_ref()).await?;
                    self.opened(&opened.0)?;
                    Ok(opened)
                })
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
                transcript: transcript.clone(),
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
            self.control(async |server: &Server| delete_sessions(server, &sessions).await)
                .await?;
            // A removed account's copies kept for them go with them (D111).
            let home = self.kept_home();
            let kept: Vec<String> = sessions
                .into_iter()
                .filter(|id| plain_id(id) && home.join(OPENED).join(id).is_file())
                .collect();
            if kept.is_empty() {
                return Ok(());
            }
            let main = self.main_env.as_deref().unwrap_or(&self.env);
            self.control_in(
                &main.for_account(ProviderKind::OpenCode, &home),
                async |server: &Server| delete_sessions(server, &kept).await,
            )
            .await?;
            for id in &kept {
                std::fs::remove_file(home.join(OPENED).join(id))?;
            }
            Ok(())
        })
    }

    fn keep_sessions(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let (Some(main), Some(own)) = (&self.main_env, self.account_home()) else {
                return Ok(());
            };
            // The sessions whose newest copy is this account's, each with when it opened here.
            let ids: Vec<(String, String)> = std::fs::read_dir(own.join(OPENED))
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|entry| {
                    let at = std::fs::read_to_string(entry.path()).ok()?;
                    Some((entry.file_name().into_string().ok()?, at))
                })
                .filter(|(id, _)| self.newest_home(id).as_ref() == Some(&own))
                .collect();
            if ids.is_empty() {
                return Ok(());
            }
            let home = self.kept_home();
            crate::accounts::create_private_dir(&home.join(OPENED))?;
            let kept = OpenCode::for_account(self.platform.clone(), main, &home);
            kept.control(async |server: &Server| {
                for (id, at) in &ids {
                    kept.copy_in(server, &self.env, id, None).await?;
                    // Its time stays: a copy opened on another account since stays newer.
                    std::fs::write(home.join(OPENED).join(id), at)?;
                }
                Ok(())
            })
            .await
        })
    }

    /// The user's own login doesn't take back a session whose newest copy is an extra
    /// account's: its own database is never rewritten ([`OpenCode::carry`]).
    fn resumes(&self, native_id: &str) -> bool {
        self.main_env.is_some() || self.newest_home(native_id).is_none()
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
                    let mut cursor: Option<String> = None;
                    // A long session's messages, a page at a time, newest first.
                    for _ in 0..100 {
                        let mut query = vec![("type", "user"), ("limit", "100")];
                        query.extend(cursor.as_deref().map(|next| ("cursor", next)));
                        let path = with_query(&format!("/api/session/{id}/message"), &query);
                        let page = server.call(Method::GET, &path, None).await?;
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
                                cursor = Some(next.to_owned());
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
    home: Option<String>,
    ledger: &dyn Ledger,
) -> Result<(String, Option<String>, Option<i64>)> {
    let folders: Vec<String> = spec
        .add_dirs
        .iter()
        .map(|dir| crate::policy::real_path(dir).unwrap_or_else(|| dir.clone()))
        .map(|dir| dir.display().to_string())
        .collect();
    let permissions = parse::permissions(spec.access, spec.tools, &folders);
    // No model named: the default the picker tags (D42), at the effort asked for, if any; a
    // reopened session asked for no effort keeps its own model.
    let reopened = matches!(spec.origin, Origin::Resume { .. } | Origin::Fork { .. });
    let model = match &spec.model {
        Some(model) => Some(model.clone()),
        None if reopened && spec.effort.is_none() => None,
        None => default_model(server, &list_models(server).await?).await?,
    };
    let model = model
        .map(|id| model_ref(&id, spec.effort.as_deref()))
        .transpose()?;
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
            home,
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
    /// How long one request may take ([`REQUEST_TIMEOUT`]).
    timeout: Duration,
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
            timeout: REQUEST_TIMEOUT,
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
        let failed = |err: reqwest::Error| match self.process.is_running() {
            true => Error::Protocol(format!("OpenCode didn't answer: {err}")),
            false => Error::Closed,
        };
        // A folder's plugins still activating answer 503 (2.2.3, slow on a busy machine): asked
        // again until they are, as long as a start may take.
        let deadline = tokio::time::Instant::now() + START_TIMEOUT;
        let (status, text) = loop {
            let mut request = self.request(method.clone(), path).timeout(self.timeout);
            if let Some(body) = body {
                request = request.json(body);
            }
            let response = match request.send().await {
                // A fresh server busy loading (several start at once) can take longer than one
                // request's timeout to answer: a read is asked again, as long as a start may take.
                Err(err)
                    if err.is_timeout()
                        && method == Method::GET
                        && tokio::time::Instant::now() < deadline =>
                {
                    continue;
                }
                sent => sent.map_err(failed)?,
            };
            let status = response.status();
            let text = response.text().await.map_err(failed)?;
            if status != reqwest::StatusCode::SERVICE_UNAVAILABLE
                || tokio::time::Instant::now() >= deadline
            {
                break (status, text);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        };
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
    /// Its transcript ([`write_transcript`]).
    transcript: PathBuf,
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
            // OpenCode's "always" is saved for good; Brigadier keeps its grants for the
            // conversation itself.
            let body = match decision {
                ApprovalDecision::Allow | ApprovalDecision::AllowSimilar => {
                    json!({ "decision": "once" })
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
            // Closed mid-turn (Brigadier quits), no turn end writes the transcript: kept now, so
            // its thread still reads what the turn holds.
            let kept = tokio::time::timeout(
                EXIT_GRACE,
                write_transcript(&self.server, &self.id, &self.transcript),
            )
            .await;
            if !matches!(kept, Ok(Ok(()))) {
                tracing::warn!(?kept, "could not keep OpenCode's transcript as it closed");
            }
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
        assert!(config.get("skills").is_none());
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
import http.server, json, os, sys, threading, time
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
        elif self.path.startswith("/api/session/ses_1/message"):
            self.answer(200, {"data": [{"id": "msg_1", "type": "user", "text": "Hold."}]})
        elif self.path == "/api/model":
            if os.environ.get("SLOW") and not os.path.exists(os.environ["SLOW"]):
                open(os.environ["SLOW"], "w").close(); time.sleep(2)
            self.answer(200, {"data": [{"id": "big-pickle", "providerID": "opencode"}]})
        elif self.path == "/api/model/default":
            self.answer(200, {"data": {"id": "big-pickle", "providerID": "opencode"}})
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

    /// A fresh server slower to answer its first read than one request's timeout (several
    /// starting at once, seen live) is asked again, not taken as gone.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_slow_first_read_is_asked_again() {
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(format!("brigadier-opencode-slow-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("opencode");
        std::fs::write(&script, FAKE_SERVER).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let log = root.join("log");
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(root.join("data")),
        })
        .unwrap();
        let mut spec = brigadier_sandbox::SpawnSpec::new(script.display().to_string());
        spec.env.push(("LOG".into(), log.clone().into()));
        spec.env.push(("ACTIVE".into(), root.join("active").into()));
        spec.env.push(("SLOW".into(), root.join("slow").into()));
        let mut server = Server::start(platform, spec, process::Options::default(), None)
            .await
            .unwrap();
        server.timeout = Duration::from_millis(300);
        let models = list_models(&server).await.unwrap();
        assert_eq!(models[0]["id"], "big-pickle");
        let asked = std::fs::read_to_string(&log).unwrap();
        assert_eq!(asked.matches("GET /api/model ").count(), 2, "{asked}");
        server.process.shutdown(EXIT_GRACE).await;
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A turn that isn't running never takes a steer: OpenCode admits it without starting one,
    /// and it is taken back out. A running turn keeps it. The `/` commands are OpenCode's own.
    /// Closed, it keeps its transcript.
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
            transcript: root.join("ses_1.jsonl"),
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

        // A new session's default is Brigadier's while OpenCode lists it, else OpenCode's own.
        let glm = json!({"id": "glm-5.3", "providerID": "opencode-go"});
        let off = json!({"id": "glm-5.3", "providerID": "opencode-go", "enabled": false});
        for (models, default) in [
            (vec![glm], DEFAULT_MODEL),
            (vec![off], "opencode/big-pickle"),
            (Vec::new(), "opencode/big-pickle"),
        ] {
            assert_eq!(
                default_model(&session.server, &models)
                    .await
                    .unwrap()
                    .as_deref(),
                Some(default)
            );
        }
        // Closed with a turn running (Brigadier quits): what the session holds is kept.
        session.close().await;
        let kept = parse::read_transcript(&root.join("ses_1.jsonl")).unwrap();
        assert!(
            kept.iter().any(|entry| matches!(&entry.event,
                ProviderEvent::Message { role: Role::User, text, .. } if text == "Hold.")),
            "{kept:#?}"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// An OpenCode whose database is a JSON file of sessions at `$OPENCODE_DB` (else the user's
    /// own, under `$HOME`), written only by a request that changes it; every request is logged
    /// with the database it went to.
    #[cfg(unix)]
    const FAKE_DB_SERVER: &str = r#"#!/usr/bin/env python3
import http.server, json, os, re, sys, threading
db = os.environ.get("OPENCODE_DB") or os.path.join(os.environ["HOME"], ".local/share/opencode/opencode.db")
log = open(os.environ["LOG"], "a")
def load():
    return json.load(open(db)) if os.path.exists(db) else {}
def save(sessions):
    json.dump(sessions, open(db, "w"))
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def answer(self, code, body=None):
        self.send_response(code); self.send_header("content-type", "application/json"); self.end_headers()
        if body is not None: self.wfile.write(json.dumps(body).encode())
    def handle_any(self):
        n = int(self.headers.get("content-length") or 0)
        body = json.loads(self.rfile.read(n).decode()) if n else None
        log.write(f"{db} {self.command} {self.path}\n"); log.flush()
        sessions = load()
        export = re.fullmatch(r"/api/experimental/session/(\w+)/export", self.path)
        one = re.fullmatch(r"/api/session/(\w+)", self.path)
        if export and self.command == "GET":
            found = sessions.get(export.group(1))
            self.answer(200, {"data": found}) if found else self.answer(404, {"message": "Session not found"})
        elif one and self.command == "DELETE":
            if one.group(1) in sessions:
                del sessions[one.group(1)]; save(sessions); self.answer(204)
            else:
                self.answer(404, {"message": "Session not found"})
        elif self.path == "/api/experimental/session/import" and self.command == "POST":
            id = body["info"]["id"]
            if id in sessions:
                self.answer(409, {"message": "Session already exists"})
            else:
                sessions[id] = {"info": body["info"], "messages": body["messages"]}; save(sessions)
                self.answer(200, {"data": body["info"]})
        else:
            self.answer(404, {"message": "not here"})
    do_GET = do_POST = do_DELETE = do_PATCH = handle_any
server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
print(json.dumps({"url": f"http://127.0.0.1:{server.server_address[1]}"}), flush=True)
threading.Thread(target=server.serve_forever, daemon=True).start()
sys.stdin.read()
"#;

    /// A server whose folder's plugins are still activating (503) is asked again until it
    /// answers; any other refusal is said at once.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_request_waits_out_plugins_still_activating() {
        use std::os::unix::fs::PermissionsExt;
        const BUSY_SERVER: &str = r#"#!/usr/bin/env python3
import http.server, json, sys, threading
asked = {"n": 0}
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        asked["n"] += 1
        busy = self.path == "/api/x" and asked["n"] <= 2
        code = 503 if busy else (200 if self.path == "/api/x" else 400)
        self.send_response(code); self.send_header("content-type", "application/json"); self.end_headers()
        self.wfile.write(json.dumps({"message": "Location plugins did not finish activating"} if busy else {"asked": asked["n"]}).encode())
server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
print(json.dumps({"url": f"http://127.0.0.1:{server.server_address[1]}"}), flush=True)
threading.Thread(target=server.serve_forever, daemon=True).start()
sys.stdin.read()
"#;
        let root = std::env::temp_dir().join(format!(
            "brigadier-opencode-busy-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("opencode");
        std::fs::write(&script, BUSY_SERVER).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(root.join("data")),
        })
        .unwrap();
        let spec = CliEnv::from_vars([]).spec(&script);
        let server = Server::start(platform, spec, process::Options::default(), None)
            .await
            .unwrap();
        let answer = server.call(Method::GET, "/api/x", None).await.unwrap();
        assert_eq!(answer["asked"], 3);
        assert!(matches!(
            server.call(Method::GET, "/api/other", None).await,
            Err(Error::Rejected(said)) if said.starts_with("400")
        ));
        server.process.shutdown(EXIT_GRACE).await;
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A session moves from the user's own database to one account, on to another and back:
    /// each move imports its newest copy under the same id, an account's older copy is
    /// replaced, the user's own database is never written, and once the session went on
    /// elsewhere the user's own login refuses it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_session_is_carried_to_an_account_and_never_written_back_to_the_users_own() {
        use std::ffi::OsString;
        use std::os::unix::fs::PermissionsExt;
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
        let root = std::env::temp_dir().join(format!(
            "brigadier-opencode-accounts-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let script = root.join("bin/opencode");
        std::fs::write(&script, FAKE_DB_SERVER).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let main_db = root.join("home/.local/share/opencode/opencode.db");
        std::fs::create_dir_all(main_db.parent().unwrap()).unwrap();
        let id = "ses_carried1";
        let session = |messages: &[&str]| json!({ "info": { "id": id, "title": "Carried" }, "messages": messages });
        std::fs::write(&main_db, json!({ id: session(&["m1"]) }).to_string()).unwrap();
        let before = (
            std::fs::read(&main_db).unwrap(),
            std::fs::metadata(&main_db).unwrap().modified().unwrap(),
        );
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(root.join("data")),
        })
        .unwrap();
        let log = root.join("log");
        let env = CliEnv::from_vars([
            (
                OsString::from("PATH"),
                OsString::from(format!("{}:/usr/bin:/bin", root.join("bin").display())),
            ),
            (OsString::from("HOME"), root.join("home").into_os_string()),
            (OsString::from("LOG"), log.clone().into_os_string()),
        ]);
        let own = OpenCode::new(platform.clone(), Arc::new(env.clone()));
        let account = |name: &str| {
            let home = root.join("data/accounts").join(name);
            let main = main_db.parent().unwrap();
            crate::accounts::prepare_home(ProviderKind::OpenCode, main, &home).unwrap();
            (OpenCode::for_account(platform.clone(), &env, &home), home)
        };
        let ((a, a_home), (b, b_home)) = (account("acct-a"), account("acct-b"));
        let ledger = Kept(Mutex::new(Vec::new()));
        let cwd = root.clone();
        let carry = async |to: &OpenCode| {
            let spec = to.env.spec(&to.binary().unwrap());
            let server = Server::start(platform.clone(), spec, process::Options::default(), None)
                .await
                .unwrap();
            let carried = to.carry(&server, id, &cwd, &ledger).await;
            server.process.shutdown(EXIT_GRACE).await;
            carried.map(|()| to.opened(id).unwrap())
        };
        let held = |home: &Path| -> Value {
            serde_json::from_str::<Value>(&std::fs::read_to_string(home.join(DB)).unwrap()).unwrap()
                [id]
                .clone()
        };

        // Never left the user's own database: every login resumes it.
        assert!(own.resumes(id) && a.resumes(id));
        carry(&a).await.unwrap();
        assert_eq!(held(&a_home), session(&["m1"]));
        assert!(ledger.holds(&Artifact::Session {
            provider: ProviderKind::OpenCode,
            id: id.into(),
            home: Some(a_home.display().to_string()),
            cwd: Some(cwd.display().to_string()),
        }));
        // Gone on there: the user's own login takes it no more; another session it still does.
        assert!(!own.resumes(id) && a.resumes(id) && b.resumes(id));
        assert!(own.resumes("ses_other"));
        assert!(matches!(carry(&own).await, Err(Error::Invalid(_))));

        // A turn on A, then on to B: A's copy is the newest.
        std::fs::write(
            a_home.join(DB),
            json!({ id: session(&["m1", "m2"]) }).to_string(),
        )
        .unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
        carry(&b).await.unwrap();
        assert_eq!(held(&b_home), session(&["m1", "m2"]));

        // A turn on B, then back to A: A's older copy is replaced, under the same id.
        std::fs::write(
            b_home.join(DB),
            json!({ id: session(&["m1", "m2", "m3"]) }).to_string(),
        )
        .unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
        carry(&a).await.unwrap();
        assert_eq!(held(&a_home), session(&["m1", "m2", "m3"]));

        // The user's own database: read for its copy once, never written.
        let said = std::fs::read_to_string(&log).unwrap();
        let main: Vec<_> = said
            .lines()
            .filter(|line| line.starts_with(&main_db.display().to_string()))
            .collect();
        assert_eq!(main.len(), 1, "{said}");
        assert!(main[0].ends_with(&format!("GET /api/experimental/session/{id}/export")));
        assert_eq!(
            (
                std::fs::read(&main_db).unwrap(),
                std::fs::metadata(&main_db).unwrap().modified().unwrap(),
            ),
            before
        );
        // Already the newest copy where it opens: nothing is asked of any database.
        std::fs::write(&log, "").unwrap();
        carry(&a).await.unwrap();
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "");

        // A is removed while it holds the newest copy (D111): kept first in Brigadier's database
        // with no login, so B goes on from it, not from B's older copy.
        std::fs::write(
            a_home.join(DB),
            json!({ id: session(&["m1", "m2", "m3", "m4"]) }).to_string(),
        )
        .unwrap();
        b.keep_sessions().await.unwrap();
        let kept = root.join("data/accounts").join(KEPT);
        assert!(!kept.exists(), "B holds no newest copy: nothing kept");
        a.keep_sessions().await.unwrap();
        std::fs::remove_dir_all(&a_home).unwrap();
        assert_eq!(held(&kept), session(&["m1", "m2", "m3", "m4"]));
        assert!(!own.resumes(id));
        carry(&b).await.unwrap();
        assert_eq!(held(&b_home), session(&["m1", "m2", "m3", "m4"]));
        // Deleting the chat takes the kept copy too.
        own.remove(vec![Artifact::Session {
            provider: ProviderKind::OpenCode,
            id: id.into(),
            home: Some(a_home.display().to_string()),
            cwd: Some(cwd.display().to_string()),
        }])
        .await
        .unwrap();
        assert!(held(&kept).is_null() && !kept.join(OPENED).join(id).exists());
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
