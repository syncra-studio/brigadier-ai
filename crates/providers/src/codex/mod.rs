//! Codex adapter: `codex app-server` over stdio JSON-RPC, one app-server per session.
//!
//! Sessions use `thread/*` and `turn/*` (steer and interrupt included). Approvals are
//! server requests that reach the user ([`REVIEWER`]). The session's level passes straight
//! through as Codex's own approval policy and sandbox ([`approval_policy`], [`sandbox_policy`]):
//!
//! - Ask: `untrusted` in the `workspace-write` sandbox: Codex asks before every command it
//!   doesn't know to be safe.
//! - Accept edits: `on-request` in the `workspace-write` sandbox: commands run inside the
//!   sandbox without asking; Codex asks to leave it.
//! - Full access: `never` with `danger-full-access`: Codex never asks.
//! - Read-only (Brigadier's own readers): `untrusted` in the `read-only` sandbox. A side chat's
//!   and a tab chat's has network on, for `brigadier`'s socket ([`sandbox_policy`]). A Chat runs
//!   as a thread does, at its own level (STRIP-DOWN-PLAN S28).
//!
//! Threads and workers ([`ToolSet::user_setup`]) run with the user's own Codex setup: their
//! config, `AGENTS.md`, plugins, hooks and MCP servers, beside Brigadier's. Threads keep the
//! skills catalog; workers leave it out of their instructions to keep their start small;
//! the project's `.codex` config too where Codex reads it (a worker working in its worktree of
//! a trusted project), and a thread working from a folder of Brigadier's gets that config's MCP
//! servers and compatible options ([`project_options`]). Every other session keeps the user's
//! setup out: plugins and hooks are switched off per process, and the MCP servers from their
//! config are disabled per thread.
//! Apps, computer and browser use, memories and `notify` are off for every session.
//!
//! Brigadier never makes Codex write to `~/.codex/config.toml`. Starting a thread with a
//! writable sandbox in a project the user never trusted makes Codex persist a trust entry for
//! the project (for a worktree: the user's main checkout), and no per-process override stops it;
//! so threads start with no sandbox of their own and every turn sets it ([`thread_sandbox`]).
//! Should Codex still add an entry for the exact folder of a Brigadier-owned session
//! ([`SessionSpec::owned_cwd`]), it is recorded and removed with the session through Codex's
//! config API, only while it is still exactly `trusted`.

pub(crate) mod files;
pub mod parse;
#[allow(clippy::all, clippy::pedantic, dead_code, unused_imports)]
pub mod protocol;
pub mod rollout;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_sandbox::Platform;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::cli::{CliEnv, parse_version, version_at_least};
use crate::events::Events;
use crate::model::*;
use crate::process::{self, CliProcess};
use crate::record::{self, Direction, Recorder};
use crate::{
    BoxFuture, Error, Ledger, Provider, ProviderSession, Replayer, Result, Started,
    TerminalCommand, now_ms,
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
/// Built-ins switched off for sessions that must not act on their own (a handoff note, a
/// one-shot job, a control connection): viewing local images, generating images, Codex's own sub-agents,
/// goals, the sleep tool, and every shell tool. Without a shell tool there is no command to approve at all,
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

/// The same for a side chat ([`ToolSet::SideChat`], [`ToolSet::TabChat`]), which keeps Codex's own
/// image generation (its images show in it as a thread's do) and its shell, for `brigadier`
/// (STRIP-DOWN-PLAN S16c). Codex's exec policy can't hold a shell to one program: its rules
/// match literal command prefixes only (no catch-all), a command no rule matches is left to
/// Codex's own judgement of safe reads, and rules load only from `.rules` files in a Codex home
/// or a trusted project. So a side chat's commands run in the read-only sandbox, and Brigadier
/// declines every one Codex asks about but `brigadier` (`policy::runs_only_brigadier`); on
/// 0.163.0 Codex asked about every command, `ls` and `cat` too (checked live). An approved command
/// the model asked to run outside the sandbox runs unsandboxed, so that answer is the guard; the
/// network proxy (`features.network_proxy`) has no effect over the per-turn sandbox policy.
const CHAT_FEATURES: &[&str] = &[
    "view_image",
    "multi_agent",
    "multi_agent_v2",
    "goals",
    "sleep_tool",
];

/// Per-process overrides for the same sessions. The sub-agent tools (`collaboration.*`) come
/// with the model, whatever the feature flags say; only `agents.enabled` removes them. The
/// user's personal skills catalog stays out of their context.
const RESTRICTED_OVERRIDES: &[&str] = &[NO_SUB_AGENTS, NO_SKILLS_CATALOG];

/// Leaves the user's skills catalog out of the session's instructions.
const NO_SKILLS_CATALOG: &str = "skills.include_instructions=false";

/// Removes Codex's sub-agent tools. Codex can't hold a sub-agent to a set of models, so a
/// session whose models are limited ([`SessionSpec::allowed_models`]) runs without them.
const NO_SUB_AGENTS: &str = "agents.enabled=false";

/// Desktop integrations, off for every session (its browser and computer tools are Brigadier's
/// own).
const DISABLED_FEATURES: &[&str] = &["apps", "computer_use", "browser_use"];

/// Codex's own memory follows the user's setting (STRIP-DOWN-PLAN S19), except where nobody's
/// memory should grow: a lean worker (G23), a side chat, which leaves nothing, Brigadier's own
/// one-shots and a control connection.
const MEMORIES: &str = "memories";

/// The user's personal Codex setup, off for a session without it ([`ToolSet::user_setup`]).
const USER_SETUP_FEATURES: &[&str] = &["plugins", "hooks"];

pub struct Codex {
    platform: Arc<dyn Platform>,
    env: Arc<CliEnv>,
    /// An extra account's ([`crate::accounts`]): the user's own `CODEX_HOME`, where its
    /// history, settings and generated images are (its own home links them).
    main_home: Option<PathBuf>,
}

impl Codex {
    pub fn new(platform: Arc<dyn Platform>, env: Arc<CliEnv>) -> Self {
        Self {
            platform,
            env,
            main_home: None,
        }
    }

    /// The adapter for an extra account whose `CODEX_HOME` is `home` (made with
    /// [`crate::accounts::prepare_home`]); `env` is the user's own environment.
    pub fn for_account(platform: Arc<dyn Platform>, env: &CliEnv, home: &Path) -> Self {
        let main_home = crate::accounts::main_home(ProviderKind::Codex, env);
        let env = Arc::new(env.for_account(ProviderKind::Codex, home));
        Self {
            platform,
            env,
            main_home,
        }
    }

    /// Who is logged in: Codex's own account record (`account/read`, which hands out no
    /// token), as (email, plan).
    async fn account(&self) -> Option<(Option<String>, Option<String>)> {
        let read: p::GetAccountResponse = self
            .control(async |rpc: &Rpc| {
                rpc.call("account/read", &p::GetAccountParams::default())
                    .await
            })
            .await
            .ok()?;
        match read.account? {
            p::Account::Chatgpt { email, plan_type } => Some((
                email.filter(|email| !email.is_empty()),
                plan_name(plan_type).map(str::to_owned),
            )),
            p::Account::ApiKey | p::Account::AmazonBedrock { .. } => Some((None, None)),
        }
    }

    /// Archives threads no Brigadier session has open (see [`archive_thread`]), except those
    /// `open` says a session has opened meanwhile.
    pub async fn archive_threads(
        &self,
        thread_ids: Vec<String>,
        private_logs: bool,
        open: impl Fn(&str) -> bool + Send + Sync,
    ) -> Result<()> {
        self.control_with_privacy(private_logs, async |rpc: &Rpc| {
            for thread_id in thread_ids.iter().filter(|id| !open(id)) {
                archive_thread(rpc, thread_id).await?;
            }
            Ok(())
        })
        .await
    }

    /// The binary it runs, looked up each time: one installed or chosen after launch counts.
    fn binary(&self) -> Result<PathBuf> {
        self.env
            .resolve(ProviderKind::Codex)
            .ok_or(Error::NotInstalled(ProviderKind::Codex))
    }

    /// Where Codex saves generated images, one folder per thread: `$CODEX_HOME` (by default
    /// `~/.codex`) `/generated_images`.
    fn images_root(&self) -> Option<PathBuf> {
        Some(self.history_home()?.join("generated_images"))
    }

    /// Where the history, settings and generated images are: `$CODEX_HOME`, or an extra
    /// account's main one (its own home links them).
    fn history_home(&self) -> Option<PathBuf> {
        self.main_home.clone().or_else(|| self.codex_home())
    }

    /// An extra account's own `CODEX_HOME`, recorded with its threads.
    fn account_home(&self) -> Option<String> {
        self.main_home.as_ref()?;
        self.codex_home().map(|dir| dir.display().to_string())
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
        let Some(path) = self.history_home().map(|home| home.join("config.toml")) else {
            return Configured::default();
        };
        tokio::fs::read_to_string(path)
            .await
            .map(|text| Configured::parse(&text))
            .unwrap_or_default()
    }

    async fn version(&self) -> Option<String> {
        let spec = self.env.spec(&self.binary().ok()?).arg("--version");
        let output = process::run(&self.platform, &spec, STATUS_TIMEOUT)
            .await
            .ok()?;
        parse_version(&output.stdout)
    }

    /// How an app-server for `cwd` is spawned: its arguments, and a session's own environment
    /// on top of the CLI's.
    fn server_spec(
        &self,
        binary: &Path,
        cwd: &Path,
        session: Option<&SessionSpec>,
    ) -> brigadier_sandbox::SpawnSpec {
        let mut spec = self.env.spec(binary);
        spec.args = app_server_args(session)
            .into_iter()
            .map(Into::into)
            .collect();
        spec.cwd = Some(cwd.to_owned());
        if let Some(session) = session {
            crate::cli::apply_session_env(&mut spec, &session.env, &session.unset_env);
            spec.low_priority = session.low_priority;
        }
        spec
    }

    /// Spawns an app-server for `cwd` with the user's personal setup switched off, and
    /// initializes it.
    async fn app_server(
        &self,
        cwd: &Path,
        session: Option<&SessionSpec>,
        recorder: Option<Arc<Recorder>>,
        ledger: Option<Arc<dyn Ledger>>,
        private_logs: bool,
    ) -> Result<(Arc<Rpc>, mpsc::Receiver<String>)> {
        let spec = self.server_spec(&self.binary()?, cwd, session);
        let redactor = session.and_then(|session| session.redactor.clone());
        let process::Spawned { process, stdout } = process::spawn(
            self.platform.clone(),
            &spec,
            process::Options {
                private_logs,
                recorder,
                redactor,
                ledger,
                sweep_dir: session.and_then(|session| session.sweep_dir.clone()),
            },
        )?;
        Ok((Arc::new(Rpc::new(process)), stdout))
    }

    /// Runs `work` against a throwaway app-server, then shuts it down.
    async fn control<T>(&self, work: impl AsyncFnOnce(&Rpc) -> Result<T>) -> Result<T> {
        self.control_with_privacy(false, work).await
    }

    async fn control_with_privacy<T>(
        &self,
        private_logs: bool,
        work: impl AsyncFnOnce(&Rpc) -> Result<T>,
    ) -> Result<T> {
        let cwd = self.platform.paths().data_dir.clone();
        let (rpc, stdout) = self
            .app_server(&cwd, None, None, None, private_logs)
            .await?;
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

impl Codex {
    /// Copies `spec`'s installed skills into the user's skills folder (`$CODEX_HOME/skills`),
    /// where this session finds them, and a plain `codex` too.
    async fn install_skills(&self, spec: &SessionSpec) {
        let Some(home) = self.history_home() else {
            return;
        };
        let plugins = spec.installed_skills.clone();
        let installed =
            tokio::task::spawn_blocking(move || install_skills(&home.join("skills"), &plugins))
                .await;
        if let Ok(Err(error)) = installed {
            tracing::warn!(%error, "couldn't install Brigadier's skills for Codex");
        }
    }
}

/// Copies each plugin's `skills/<name>/SKILL.md` to `<skills>/<name>/SKILL.md`, unless it is
/// already the same.
fn install_skills(skills: &Path, plugins: &[PathBuf]) -> std::io::Result<()> {
    for plugin in plugins {
        for entry in std::fs::read_dir(plugin.join("skills"))? {
            let entry = entry?;
            let text = std::fs::read_to_string(entry.path().join("SKILL.md"))?;
            let dir = skills.join(entry.file_name());
            let path = dir.join("SKILL.md");
            if std::fs::read_to_string(&path).is_ok_and(|current| current == text) {
                continue;
            }
            std::fs::create_dir_all(&dir)?;
            std::fs::write(path, text)?;
        }
    }
    Ok(())
}

/// The plan's name as people know it, for "Connected · Pro plan"; none when Codex doesn't know.
fn plan_name(plan: p::PlanType) -> Option<&'static str> {
    use p::PlanType as P;
    Some(match plan {
        P::Free => "Free",
        P::Go => "Go",
        P::Plus => "Plus",
        P::Pro => "Pro",
        P::Prolite => "Pro Lite",
        P::Promax => "Pro Max",
        P::Team => "Team",
        P::SelfServeBusinessProlite | P::SelfServeBusinessUsageBased | P::Business => "Business",
        P::Ent26 | P::EnterpriseCbpAutomation | P::EnterpriseCbpUsageBased | P::Enterprise => {
            "Enterprise"
        }
        P::Edu => "Edu",
        P::EduPlus => "Edu Plus",
        P::EduPro => "Edu Pro",
        P::Unknown => return None,
    })
}

/// `codex app-server`'s arguments for a session (`None`: a control connection).
fn app_server_args(session: Option<&SessionSpec>) -> Vec<String> {
    let mut args: Vec<String> = vec!["app-server".into()];
    let tools = session.map(|session| session.tools).unwrap_or_default();
    // The built-ins switched off, and whether the restricted overrides come with them.
    let (role_features, restricted): (&[&str], bool) = match tools {
        // A thread keeps Codex's own sub-agents, goals and sleep tool, as in a plain CLI
        // (STRIP-DOWN-PLAN S3).
        ToolSet::Default | ToolSet::Lean | ToolSet::Thread => (&[], false),
        ToolSet::None | ToolSet::Web | ToolSet::Review => (RESTRICTED_FEATURES, true),
        ToolSet::SideChat | ToolSet::TabChat => (CHAT_FEATURES, true),
    };
    let user_setup = tools.user_setup();
    let sealed: &[&str] = if user_setup { &[] } else { USER_SETUP_FEATURES };
    let memory_off = session.is_none_or(|session| {
        session.lean_worker
            || matches!(
                tools,
                ToolSet::None | ToolSet::Web | ToolSet::Review | ToolSet::TabChat
            )
    });
    let memories: &[&str] = if memory_off { &[MEMORIES] } else { &[] };
    for feature in DISABLED_FEATURES
        .iter()
        .chain(sealed)
        .chain(role_features)
        .chain(memories)
    {
        args.push("--disable".into());
        args.push((*feature).into());
    }
    if restricted {
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
    // The user's own skills they switched off for Brigadier, by file, beside their own
    // entries: the override replaces the list (docs/research/phase7-probes.md (d)).
    if let Some(off) = session
        .map(|session| &session.switched_off)
        .filter(|off| user_setup && !off.skill_files.is_empty())
    {
        let entries: Vec<Value> = off
            .codex_skill_config
            .iter()
            .cloned()
            .chain(
                off.skill_files
                    .iter()
                    .map(|file| json!({ "path": file.display().to_string(), "enabled": false })),
            )
            .collect();
        args.push("-c".into());
        args.push(format!(
            "skills.config={}",
            toml_value(&Value::Array(entries))
        ));
    }
    // A plugin's server, by its plugin's key: an `mcp_servers` entry of its name would be a
    // server without a command (checked on 0.163.0). A dotted key splits at each `.`.
    for (plugin, server) in session
        .filter(|_| user_setup)
        .map(|session| session.switched_off.plugin_servers.as_slice())
        .unwrap_or_default()
    {
        if plugin.contains('.') || server.contains('.') {
            tracing::warn!(
                plugin,
                server,
                "a switched-off plugin server's name has a dot; left on"
            );
            continue;
        }
        args.push("-c".into());
        args.push(format!(
            "plugins.{plugin}.mcp_servers.{server}.enabled=false"
        ));
    }
    args.push("-c".into());
    args.push("notify=[]".into());
    // Commands run in a plain (non-login) shell. A login shell, and the login environment
    // Codex snapshots from one, would rebuild PATH and lose the folder `brigadier` ships in,
    // which the session's environment puts first.
    args.push("-c".into());
    args.push("allow_login_shell=false".into());
    args
}

/// `codex resume` for `spec`'s thread in a terminal: the worker's own `--disable`/`-c` flags
/// and thread config (as `-c` overrides), its model, effort and access as flags. A thread's
/// sandbox and approvals are per process, not stored with it, so they are repeated; its
/// developer instructions are stored. No MCP server's value (a grant, a token, a header) goes
/// on the command line ([`terminal_servers`]).
fn terminal_command(
    program: PathBuf,
    spec: &SessionSpec,
    native_id: &str,
    cwd: PathBuf,
    mut config: Map<String, Value>,
    env: &CliEnv,
) -> TerminalCommand {
    let mut args: Vec<String> = vec![
        "resume".into(),
        native_id.into(),
        // The thread lives in the terminal's own process (`codex resume --help`).
        "--no-daemon".into(),
        "--no-alt-screen".into(),
    ];
    args.extend(app_server_args(Some(spec)).into_iter().skip(1));
    let mut variables = spec.env.clone();
    if let Some(Value::Object(servers)) = config.get_mut("mcp_servers") {
        terminal_servers(servers, env, &mut variables);
    }
    if let Some(model) = &spec.model {
        args.extend(["-m".into(), model.clone()]);
    }
    if let Some(effort) = &spec.effort {
        args.extend([
            "-c".into(),
            format!("model_reasoning_effort={}", toml_value(&json!(effort))),
        ]);
    }
    for (key, value) in &config {
        args.extend(["-c".into(), format!("{key}={}", toml_value(value))]);
    }
    // The terminal's `-a` takes only `on-request` and `never` (0.163.0): Ask's `untrusted` goes
    // in as config.
    match spec.access {
        Access::Full => args.push("--dangerously-bypass-approvals-and-sandbox".into()),
        Access::ReadOnly => args.extend(["-s", "read-only", "-a", "on-request"].map(String::from)),
        Access::AcceptEdits => {
            args.extend(["-s", "workspace-write", "-a", "on-request"].map(String::from));
        }
        Access::Ask => args.extend(
            [
                "-s",
                "workspace-write",
                "-c",
                "approval_policy=\"untrusted\"",
            ]
            .map(String::from),
        ),
    }
    let mut process = env.spec(&program);
    crate::cli::apply_session_env(&mut process, &variables, &spec.unset_env);
    TerminalCommand {
        program,
        args,
        cwd,
        env: process.env,
    }
}

/// Every server of a terminal's config (Brigadier's, the ones a task was given and a project's
/// copied ones alike) with its values moved off the command line into the terminal's
/// environment (`variables`). A stdio server's `env` becomes `env_vars`, which Codex passes on
/// by name only (docs/research/phase7-probes.md (h)): a name another server or the terminal
/// itself has with a different value is set from an alias of its own by a shell around the
/// server (`/bin/sh -c 'export NAME="$ALIAS"; exec "$0" "$@"'`). Windows has no such shell, so
/// there such a server is left out of the terminal. HTTP headers become `env_http_headers`
/// with aliases (e).
fn terminal_servers(
    servers: &mut Map<String, Value>,
    env: &CliEnv,
    variables: &mut Vec<(String, String)>,
) {
    let value_of = |entry: &Value| entry.as_str().map(str::to_owned);
    // Every value each name is given, the terminal's own included.
    let mut given: HashMap<String, Vec<String>> = HashMap::new();
    for (name, value) in variables.iter() {
        given.entry(name.clone()).or_default().push(value.clone());
    }
    for server in servers.values() {
        for (name, value) in server
            .get("env")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            if let Some(value) = value_of(value) {
                given.entry(name.clone()).or_default().push(value);
            }
        }
    }
    // Names given different values (by two servers, or by a server and the terminal).
    let clashing: HashSet<String> = given
        .iter()
        .filter(|(name, values)| {
            let own = variables
                .iter()
                .find(|(own, _)| own == *name)
                .map(|(_, value)| value.clone())
                .or_else(|| {
                    env.var(name)
                        .map(|value| value.to_string_lossy().into_owned())
                });
            values.iter().any(|value| *value != values[0])
                || own.is_some_and(|own| own != values[0])
        })
        .map(|(name, _)| name.clone())
        .collect();
    let mut next_alias = 0;
    let mut alias = |value: String, variables: &mut Vec<(String, String)>| {
        let name = format!("BRIGADIER_MCP_{next_alias}");
        next_alias += 1;
        variables.push((name.clone(), value));
        name
    };
    let mut left_out = Vec::new();
    for (name, server) in servers.iter_mut() {
        let Some(entry) = server.as_object_mut() else {
            continue;
        };
        if let Some(Value::Object(headers)) = entry.remove("http_headers") {
            let mut by_env = entry
                .remove("env_http_headers")
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default();
            for (header, value) in headers {
                if let Some(value) = value_of(&value) {
                    by_env.insert(header, json!(alias(value, variables)));
                }
            }
            entry.insert("env_http_headers".into(), Value::Object(by_env));
        }
        let Some(Value::Object(values)) = entry.remove("env") else {
            continue;
        };
        let values: Vec<(String, String)> = values
            .iter()
            .filter_map(|(key, value)| Some((key.clone(), value_of(value)?)))
            .collect();
        let mut passed: Vec<Value> = entry
            .remove("env_vars")
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        if values.iter().any(|(key, _)| clashing.contains(key)) {
            let shell_name = |key: &str| {
                !key.is_empty()
                    && !key.starts_with(|c: char| c.is_ascii_digit())
                    && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            };
            let command = entry
                .get("command")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let (Some(command), true) = (
                command,
                cfg!(unix) && values.iter().all(|(key, _)| shell_name(key)),
            ) else {
                left_out.push(name.clone());
                continue;
            };
            let mut exports = Vec::new();
            for (key, value) in values {
                let alias = alias(value, variables);
                exports.push(format!("{key}=\"${alias}\""));
                passed.push(json!(alias));
            }
            let args: Vec<Value> = entry
                .get("args")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let script = format!("export {}; exec \"$0\" \"$@\"", exports.join(" "));
            entry.insert("command".into(), json!("/bin/sh"));
            entry.insert(
                "args".into(),
                Value::Array(
                    [json!("-c"), json!(script), json!(command)]
                        .into_iter()
                        .chain(args)
                        .collect(),
                ),
            );
        } else {
            for (key, value) in values {
                if !variables.iter().any(|(own, _)| *own == key) {
                    variables.push((key.clone(), value));
                }
                passed.push(json!(key));
            }
        }
        entry.insert("env_vars".into(), Value::Array(passed));
    }
    for name in left_out {
        tracing::warn!(server = %name, "MCP server left out of the terminal: its variables need a shell around it");
        servers.remove(&name);
    }
}

/// Serves a control app-server's output: only responses matter.
async fn control_reader(rpc: Arc<Rpc>, mut stdout: mpsc::Receiver<String>) {
    let mut parser = Parser::live().with_private_logs(rpc.process.private_logs());
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

/// What the status says when Codex is missing or signed out: Connect does either in one click,
/// so no command and no terminal.
const NOT_INSTALLED: &str = "Codex isn't installed. Install it from Settings › Providers.";
const SIGNED_OUT: &str = "Codex isn't signed in. Sign in from Settings › Providers.";

impl Provider for Codex {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Codex
    }

    fn status(&self) -> BoxFuture<'_, ProviderStatus> {
        Box::pin(async move {
            let found = self.binary();
            let mut status = ProviderStatus {
                provider: ProviderKind::Codex,
                path: found.as_ref().ok().map(|path| path.display().to_string()),
                version: None,
                logged_in: false,
                auth_method: None,
                plan: None,
                email: None,
                organization: None,
                guidance: None,
                compacts: false,
            };
            let Ok(binary) = found else {
                status.guidance = Some(NOT_INSTALLED.into());
                return status;
            };
            let spec = self.env.spec(&binary).arg("login").arg("status");
            // Side by side: Connect asks this the moment a sign-in may have finished.
            let (version, login) = tokio::join!(
                self.version(),
                process::run(&self.platform, &spec, STATUS_TIMEOUT)
            );
            status.version = version;
            status.compacts = status
                .version
                .as_deref()
                .is_some_and(|version| version_at_least(version, COMPACT_SINCE));
            match login {
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
                    if status.logged_in
                        && let Some((email, plan)) = self.account().await
                    {
                        status.email = email;
                        status.plan = plan;
                    }
                }
                Err(err) => status.guidance = Some(format!("Could not ask Codex: {err}")),
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
            self.install_skills(&spec).await;
            let spawned = self
                .app_server(
                    &cwd,
                    Some(&spec),
                    recorder,
                    Some(ledger.clone()),
                    ledger.private_logs(),
                )
                .await;
            let (rpc, stdout) = spawned?;
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
                thread: Mutex::new(None),
                own_mcp: spec
                    .mcp_servers
                    .iter()
                    .map(|server| server.name.clone())
                    .collect(),
                mcp_listing: AtomicBool::new(true),
                mcp_relist: Mutex::new(McpRelist::default()),
            });
            let trusted: HashSet<String> = spec
                .mcp_servers
                .iter()
                .filter(|server| server.trusted)
                .map(|server| server.name.clone())
                .collect();
            tokio::spawn(read_loop(rpc.clone(), stdout, shared.clone(), trusted));

            let started = tokio::time::timeout(
                START_TIMEOUT,
                open_thread(&rpc, &spec, &cwd, self, ledger.as_ref()),
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

            *lock(&shared.thread) = Some(thread.id.clone());
            let session = Arc::new(CodexSession {
                thread_id: thread.id.clone(),
                ephemeral: thread.ephemeral,
                compacts: version_at_least(&thread.cli_version, COMPACT_SINCE),
                rpc,
                shared,
                access: spec.access,
                tools: spec.tools,
                writable_roots: writable_roots(&spec),
                effort: spec.effort.clone(),
            });
            session
                .emit(ProviderEvent::SessionStarted {
                    native_id: thread.id,
                    model: Some(model),
                    cwd: Some(thread.cwd.to_string()),
                    cli_version: Some(thread.cli_version.clone()),
                    file: thread.path.clone(),
                })
                .await;
            list_mcp_servers(&session.rpc, &session.shared);
            tokio::spawn(watch_background_terminals(
                session.rpc.clone(),
                session.shared.clone(),
                session.thread_id.clone(),
            ));
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
        self.remove_with_privacy(artifacts, false)
    }

    fn remove_with_privacy(
        &self,
        artifacts: Vec<Artifact>,
        private_logs: bool,
    ) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let mut threads = Vec::new();
            let mut trusts = Vec::new();
            let mut images = Vec::new();
            for artifact in artifacts {
                match artifact {
                    Artifact::CodexThread { thread_id, .. } => threads.push(thread_id),
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
            self.control_with_privacy(private_logs, async |rpc: &Rpc| {
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
                        Err(Error::Rejected(message)) if already_gone(&message) => {}
                        Err(err) => return Err(err),
                    }
                }
                Ok(())
            })
            .await
        })
    }

    fn terminal(&self, spec: SessionSpec, cwd: PathBuf) -> BoxFuture<'_, Result<TerminalCommand>> {
        Box::pin(async move {
            let Origin::Resume { native_id } = &spec.origin else {
                return Err(Error::Invalid(
                    "only a session that exists can open in a terminal".into(),
                ));
            };
            let native_id = native_id.clone();
            let program = self.binary()?;
            let cwd = cwd.canonicalize().map_err(|err| {
                Error::Invalid(format!("working directory {}: {err}", cwd.display()))
            })?;
            // Brigadier archived the thread when it closed it; the terminal would ask first.
            let config = self
                .control(async |rpc: &Rpc| {
                    unarchive_thread(rpc, &native_id).await?;
                    thread_config(rpc, &spec, &cwd).await
                })
                .await?;
            Ok(terminal_command(
                program, &spec, &native_id, cwd, config, &self.env,
            ))
        })
    }

    fn replayer(&self) -> Box<dyn Replayer> {
        Box::new(CodexReplayer {
            parser: Parser::replay(),
            asked: HashSet::new(),
        })
    }

    fn took_input(&self, native_id: &str, needle: &str) -> BoxFuture<'_, bool> {
        let dir = self.history_home();
        let (id, needle) = (native_id.to_owned(), needle.to_owned());
        Box::pin(async move {
            let Some(dir) = dir else {
                return false;
            };
            tokio::task::spawn_blocking(move || crate::history::codex_took(&dir, &id, &needle))
                .await
                .unwrap_or(false)
        })
    }

    fn past_folders(&self) -> BoxFuture<'_, Vec<crate::history::PastFolder>> {
        let dir = self.history_home();
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

/// The call that opens a session's thread.
enum Opening {
    Start(p::ThreadStartParams),
    Resume(p::ThreadResumeParams),
    Fork(p::ThreadForkParams),
}

/// The call that opens `spec`'s thread with the settings in `start`. A one-shot run's new or
/// forked thread is ephemeral (never saved); a resumed thread was saved.
/// A thread's start: Brigadier's instructions for it go, word for word, as its developer
/// instructions (STRIP-DOWN-PLAN S17).
fn start_params(
    spec: &SessionSpec,
    cwd: &Path,
    config: Map<String, Value>,
) -> p::ThreadStartParams {
    // Always named: an omitted tier inherits the resumed thread's or the user's config.
    let service_tier = Some(if spec.fast { FAST_TIER } else { STANDARD_TIER }.to_owned());
    p::ThreadStartParams {
        cwd: Some(cwd.display().to_string()),
        model: spec.model.clone(),
        sandbox: thread_sandbox(spec.access),
        approval_policy: Some(approval_policy(spec.access)),
        approvals_reviewer: Some(REVIEWER),
        developer_instructions: spec.append_system_prompt.clone(),
        config: Some(config),
        service_tier,
        ..Default::default()
    }
}

fn opening(spec: &SessionSpec, start: p::ThreadStartParams) -> Opening {
    let ephemeral = spec.ephemeral.then_some(true);
    match &spec.origin {
        Origin::New | Origin::NewAs { .. } => {
            Opening::Start(p::ThreadStartParams { ephemeral, ..start })
        }
        Origin::Resume { native_id } => Opening::Resume(p::ThreadResumeParams {
            thread_id: native_id.clone(),
            cwd: start.cwd,
            model: start.model,
            sandbox: start.sandbox,
            approval_policy: start.approval_policy,
            approvals_reviewer: start.approvals_reviewer,
            developer_instructions: start.developer_instructions,
            config: start.config,
            exclude_turns: Some(true),
            service_tier: start.service_tier,
            ..Default::default()
        }),
        // An ephemeral fork needs `excludeTurns`.
        Origin::Fork { native_id } => Opening::Fork(p::ThreadForkParams {
            thread_id: native_id.clone(),
            cwd: start.cwd,
            model: start.model,
            sandbox: start.sandbox,
            approval_policy: start.approval_policy,
            approvals_reviewer: start.approvals_reviewer,
            developer_instructions: start.developer_instructions,
            config: start.config,
            exclude_turns: Some(true),
            service_tier: start.service_tier,
            ephemeral,
            ..Default::default()
        }),
    }
}

/// Starts, resumes or forks the session's thread, recording it if saved (and the folder its
/// generated images would go to, under `codex`'s images root) in the ledger.
async fn open_thread(
    rpc: &Rpc,
    spec: &SessionSpec,
    cwd: &Path,
    codex: &Codex,
    ledger: &dyn Ledger,
) -> Result<Opened> {
    let images_root = codex.images_root();
    let home = codex.account_home();
    rpc.initialize().await?;
    // Brigadier's own skills join the ones Codex finds itself, for this app-server only. A Codex
    // without extra skill roots still runs; the agent's prompt names the tools either way.
    if !spec.skill_plugins.is_empty() {
        let roots: Vec<String> = spec
            .skill_plugins
            .iter()
            .map(|plugin| plugin.join("skills").display().to_string())
            .collect();
        if let Err(error) = rpc
            .call::<_, Value>("skills/extraRoots/set", &json!({ "extraRoots": roots }))
            .await
        {
            tracing::warn!(%error, "Codex didn't take Brigadier's skills");
        }
    }
    let config = thread_config(rpc, spec, cwd).await?;
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
    let start = start_params(spec, cwd, config);
    let (thread, model) = match opening(spec, start) {
        Opening::Start(params) => {
            let started: p::ThreadStartResponse = rpc.call("thread/start", &params).await?;
            (started.thread, started.model)
        }
        Opening::Resume(params) => {
            // Brigadier archived it when it last closed the thread.
            unarchive_thread(rpc, &params.thread_id).await?;
            let resumed: p::ThreadResumeResponse = rpc.call("thread/resume", &params).await?;
            (resumed.thread, resumed.model)
        }
        Opening::Fork(params) => {
            let archived = unarchive_thread(rpc, &params.thread_id).await?;
            let forked: p::ThreadForkResponse = rpc.call("thread/fork", &params).await?;
            if archived {
                archive_thread(rpc, &params.thread_id).await?;
            }
            (forked.thread, forked.model)
        }
    };
    // An ephemeral thread leaves nothing on disk: nothing to archive or delete later.
    if !thread.ephemeral {
        ledger
            .record(Artifact::CodexThread {
                thread_id: thread.id.clone(),
                home,
                cwd: Some(cwd.display().to_string()),
            })
            .await?;
    }
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

/// Whether `thread/delete` refused because the thread is already gone: never saved (no turn
/// ran, or an ephemeral thread), or deleted before. Nothing to do then.
fn already_gone(message: &str) -> bool {
    message.contains("not found")
        || message.contains("no rollout found")
        || message.contains("not persisted")
}

/// The trust entry Codex writes, exactly.
fn trusted() -> Value {
    json!({ "trust_level": "trusted" })
}

/// The `projects` table of the user's own `config.toml`, by project path.
/// Archives a thread Brigadier is done with for now, so the user's own Codex apps don't list
/// it among their threads (their Recents). The app-server offers no unlisted threads
/// that can still be resumed: every thread it starts is recorded as a `vscode` one (a one-shot
/// run's is ephemeral instead, never saved). A thread with no turn yet has nothing to archive.
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

/// The session's `web_search` setting; `None`: Codex's own default.
fn web_search(tools: ToolSet) -> Option<&'static str> {
    match tools {
        ToolSet::Default | ToolSet::Lean => None,
        ToolSet::None | ToolSet::Review | ToolSet::TabChat => Some("disabled"),
        ToolSet::Web | ToolSet::SideChat | ToolSet::Thread => Some("live"),
    }
}

/// Per-thread config: the servers the session is given on, the user's off but in a session with
/// their setup ([`ToolSet::user_setup`]), and the workspace sandbox with network access, or the
/// session's permission profile.
async fn thread_config(rpc: &Rpc, spec: &SessionSpec, cwd: &Path) -> Result<Map<String, Value>> {
    let effective: Value = rpc
        .call(
            "config/read",
            &json!({ "includeLayers": false, "cwd": cwd.display().to_string() }),
        )
        .await?;
    let mut servers = mcp_config(&effective, spec);
    let mut config = Map::new();
    // A thread works from a folder of Brigadier's, where Codex reads no project config: its
    // trusted project's MCP servers and compatible options come along (#315).
    if let Some(project) = spec
        .project
        .as_deref()
        .filter(|project| *project != cwd && spec.tools.user_setup())
    {
        let read: Result<Value> = rpc
            .call(
                "config/read",
                &json!({ "includeLayers": true, "cwd": project.display().to_string() }),
            )
            .await;
        match read {
            Ok(read) => {
                config.extend(project_options(&read));
                for (name, server) in project_servers(&read) {
                    servers.entry(name).or_insert(server);
                }
            }
            Err(err) => {
                tracing::warn!(project = %project.display(), error = %err, "could not read the project's Codex config");
            }
        }
    }
    config.insert("mcp_servers".into(), Value::Object(servers));
    shell_sees_brigadier(&mut config, &effective, spec);
    if !spec.auto_compact {
        // Codex clamps the limit to 90% of the model's window.
        config.insert(
            "model_auto_compact_token_limit".into(),
            json!(NO_AUTO_COMPACT_TOKENS),
        );
    }
    if let Some(mode) = web_search(spec.tools) {
        config.insert("web_search".into(), json!(mode));
    }
    if matches!(spec.access, Access::Ask | Access::AcceptEdits) {
        config.insert(
            "sandbox_workspace_write".into(),
            json!({
                // The `brigadier` CLI reaches the daemon over a Unix socket; Codex's sandbox
                // allows that only with network on (its `unix_sockets` allow-list needs network
                // on too), so Ask and Accept edits keep network on.
                "network_access": true,
                "writable_roots": writable_roots(spec)
                    .iter()
                    .map(|root| root.display().to_string())
                    .collect::<Vec<_>>(),
            }),
        );
    }
    Ok(config)
}

/// The session's `BRIGADIER_*` variables (who calls `brigadier`, its grant, the data folder) in
/// the commands Codex runs, whatever the user's `shell_environment_policy` inherits: added to
/// its `set`, on top of the policy Codex already merged (the override replaces the whole
/// table). `PATH` is inherited as is, so Codex's own additions to it stay.
fn shell_sees_brigadier(config: &mut Map<String, Value>, effective: &Value, spec: &SessionSpec) {
    let ours: Vec<&(String, String)> = spec
        .env
        .iter()
        .filter(|(name, _)| name.starts_with("BRIGADIER_"))
        .collect();
    if ours.is_empty() {
        return;
    }
    let mut policy = config
        .remove("shell_environment_policy")
        .or_else(|| {
            effective
                .pointer("/config/shell_environment_policy")
                .cloned()
        })
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    remove_unset_options(&mut policy);
    let set = policy
        .as_object_mut()
        .expect("an object")
        .entry("set")
        .or_insert_with(|| json!({}));
    if !set.is_object() {
        *set = json!({});
    }
    let set = set.as_object_mut().expect("an object");
    for (name, value) in ours {
        set.insert(name.clone(), Value::String(value.clone()));
    }
    config.insert("shell_environment_policy".into(), policy);
}

/// Loaded project layers only: Codex itself decides whether the project is trusted.
fn project_layers(read: &Value) -> impl Iterator<Item = &Value> {
    read.get("layers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|layer| layer.pointer("/name/type").and_then(Value::as_str) == Some("project"))
        .filter(|layer| layer.get("disabledReason").is_none_or(Value::is_null))
}

/// Project options that do not conflict with Brigadier's thread controls (Codex's config
/// schema). Use an allowlist so new upstream settings cannot override permissions, model,
/// prompts or process features. Project profiles are ignored by Codex itself; hooks need
/// their own working-folder handling before they can come along.
fn project_options(read: &Value) -> Map<String, Value> {
    const KEYS: &[&str] = &[
        "shell_environment_policy",
        "tools",
        "tool_output_token_limit",
        "background_terminal_max_timeout",
    ];
    KEYS.iter()
        .filter_map(|key| {
            let value = project_layers(read)
                .find_map(|layer| layer.get("config").and_then(|config| config.get(*key)))?;
            // Codex has already merged the project with the user's settings, including
            // nested tool options and environment policy. Keep that layering intact.
            let value = read
                .get("config")
                .and_then(|all| all.get(*key))
                .unwrap_or(value);
            let mut value = value.clone();
            remove_unset_options(&mut value);
            Some(((*key).into(), value))
        })
        .collect()
}

/// `config/read` serializes unset optional fields as null. The thread config is converted
/// back to TOML, which has no null: forwarding those fields makes Codex reject the override.
fn remove_unset_options(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            fields.retain(|_, value| !value.is_null());
            for value in fields.values_mut() {
                remove_unset_options(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                remove_unset_options(value);
            }
        }
        _ => {}
    }
}

/// MCP servers from every enabled project layer, using Codex's effective settings.
/// Layers are reported nearest first: the first layer defining a name supplies its default
/// working folder, so inherited servers still run in their own project outside a thread's cwd.
fn project_servers(read: &Value) -> Map<String, Value> {
    let effective = read
        .pointer("/config/mcp_servers")
        .and_then(Value::as_object);
    let mut servers = Map::new();
    for layer in project_layers(read) {
        let root = layer
            .pointer("/name/dotCodexFolder")
            .and_then(Value::as_str)
            .and_then(|folder| Path::new(folder).parent());
        let Some(project) = layer
            .pointer("/config/mcp_servers")
            .and_then(Value::as_object)
        else {
            continue;
        };
        for (name, server) in project {
            servers.entry(name.clone()).or_insert_with(|| {
                let mut server = effective
                    .and_then(|servers| servers.get(name))
                    .unwrap_or(server)
                    .clone();
                remove_unset_options(&mut server);
                if let Some(config) = server.as_object_mut()
                    && let Some(root) = root
                    && config.contains_key("command")
                    && !config.contains_key("cwd")
                {
                    config.insert("cwd".into(), json!(root.display().to_string()));
                }
                server
            });
        }
    }
    servers
}

/// MCP overrides leave the user's servers inherited in sessions with their setup, and add the
/// servers the session is given beside them. Other sessions explicitly disable the user's
/// servers.
fn mcp_config(effective: &Value, spec: &SessionSpec) -> Map<String, Value> {
    let mut servers: Map<String, Value> = effective
        .pointer("/config/mcp_servers")
        .and_then(Value::as_object)
        // A sealed session, or a lean worker (G23), goes without every server of the user's.
        .filter(|_| !spec.tools.user_setup() || spec.lean_worker)
        .map(|servers| {
            servers
                .keys()
                .map(|name| (name.clone(), json!({ "enabled": false })))
                .collect()
        })
        .unwrap_or_default();
    // The thread's config goes over the app-server's stdin, and Codex keeps none of it on
    // disk (docs/research/phase7-probes.md (i)): each server's values are in its own entry.
    for server in &spec.mcp_servers {
        let values: Map<String, Value> = server
            .env
            .iter()
            .map(|(name, value)| (name.clone(), Value::String(value.clone())))
            .collect();
        let mut config = match &server.transport {
            McpTransport::Stdio { command, args, cwd } => {
                let mut config = json!({
                    "command": command.display().to_string(),
                    "args": args,
                    "env": values,
                });
                if let Some(cwd) = cwd {
                    config["cwd"] = json!(cwd.display().to_string());
                }
                config
            }
            McpTransport::Http { url } => json!({ "url": url, "http_headers": values }),
        };
        if let Some(secs) = server.tool_timeout_secs {
            config["tool_timeout_sec"] = json!(secs);
        }
        if server.trusted {
            config["default_tools_approval_mode"] = json!("approve");
        }
        servers.insert(server.name.clone(), config);
    }
    // The user's own servers they switched off for Brigadier, a project's copied ones too
    // (docs/research/phase7-probes.md (c)).
    for name in &spec.switched_off.servers {
        servers.insert(name.clone(), json!({ "enabled": false }));
    }
    servers
}

/// A JSON value as a TOML inline value, for a `-c` override (strings, booleans, numbers,
/// arrays and tables; a null is an empty string).
fn toml_value(value: &Value) -> String {
    match value {
        Value::Null => toml_string(""),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => toml_string(text),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(toml_value).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(table) => format!(
            "{{{}}}",
            table
                .iter()
                .map(|(key, value)| format!("{} = {}", toml_string(key), toml_value(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// When Codex asks, by level (see the module docs).
fn approval_policy(access: Access) -> p::AskForApproval {
    match access {
        Access::Ask | Access::ReadOnly => p::AskForApproval::Untrusted,
        Access::AcceptEdits => p::AskForApproval::OnRequest,
        Access::Full => p::AskForApproval::Never,
    }
}

/// Who answers Codex's approval requests: the user, at every level ("Configures who approval
/// requests are routed to for review … Defaults to `user`", the app-server's JSON Schema,
/// codex-cli 0.162.1). Always named, so a reviewer set in the user's own config never applies.
const REVIEWER: p::ApprovalsReviewer = p::ApprovalsReviewer::User;

/// The sandbox a thread starts with. Only read-only is set here: starting a thread with a
/// writable sandbox in a project the user never trusted makes Codex persist a trust entry for
/// that project in `~/.codex/config.toml` (for a worktree, the user's main checkout), which a
/// `-c projects.….trust_level` override does not prevent. Every turn sets the full sandbox
/// policy instead ([`sandbox_policy`]), which holds for that turn and the ones after it; nothing
/// runs in a thread outside a turn.
fn thread_sandbox(access: Access) -> Option<p::SandboxMode> {
    match access {
        Access::ReadOnly => Some(p::SandboxMode::ReadOnly),
        Access::Ask | Access::AcceptEdits | Access::Full => None,
    }
}

/// Codex's own sandbox at `access`: `workspace-write` (the working directory and
/// `writable_roots`, with the network) below Full access, none at Full access. The `brigadier`
/// CLI reaches the daemon over a Unix socket; Codex's sandbox allows that only with network on
/// (its `unix_sockets` allow-list needs network on too), so Ask and Accept edits keep network on,
/// and so does a side chat's or tab chat's read-only sandbox (`tools`).
fn sandbox_policy(access: Access, tools: ToolSet, writable_roots: &[PathBuf]) -> p::SandboxPolicy {
    match access {
        Access::Ask | Access::AcceptEdits => p::SandboxPolicy::WorkspaceWrite {
            exclude_slash_tmp: false,
            exclude_tmpdir_env_var: false,
            network_access: true,
            writable_roots: writable_roots
                .iter()
                .map(|root| p::AbsolutePathBuf(root.display().to_string()))
                .collect(),
        },
        Access::ReadOnly => p::SandboxPolicy::ReadOnly {
            network_access: matches!(tools, ToolSet::SideChat | ToolSet::TabChat),
        },
        Access::Full => p::SandboxPolicy::DangerFullAccess,
    }
}

/// The folders a session's workspace sandbox writes besides its working directory: its extra
/// folders ([`SessionSpec::add_dirs`]), as the file system resolves them. Codex resolves its
/// working directory and so the paths it writes; a root given as `/tmp/x` would not hold
/// `/private/tmp/x`.
fn writable_roots(spec: &SessionSpec) -> Vec<PathBuf> {
    spec.add_dirs
        .iter()
        .map(|dir| crate::policy::real_path(dir).unwrap_or_else(|| dir.clone()))
        .collect()
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
                    // Its background terminals are listed only to a client that takes its
                    // experimental API (`thread/backgroundTerminals/*`, codex 0.163).
                    capabilities: Some(p::InitializeCapabilities {
                        experimental_api: true,
                        ..Default::default()
                    }),
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
            crate::logging::private_trace!(self.process.private_logs(), warn, "Could not decode a Codex response"; method, error = %err);
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
    /// The thread, once it is open: its MCP servers are listed.
    thread: Mutex<Option<String>>,
    /// Brigadier's own MCP servers, left out of the servers reported.
    own_mcp: HashSet<String>,
    /// Codex lists the thread's MCP servers (`mcpServerStatus/list`); off for good once it
    /// refuses (an older app-server).
    mcp_listing: AtomicBool,
    mcp_relist: Mutex<McpRelist>,
}

/// The MCP server listing in flight, and whether another was asked for meanwhile: a burst of
/// startup steps makes one more listing, not one each.
#[derive(Default)]
struct McpRelist {
    running: bool,
    again: bool,
}

pub struct CodexSession {
    thread_id: String,
    /// Never saved ([`SessionSpec::ephemeral`]): there is nothing to archive.
    ephemeral: bool,
    /// The app-server serves `thread/compact/start`.
    compacts: bool,
    rpc: Arc<Rpc>,
    shared: Arc<Shared>,
    access: Access,
    /// Its tools: a side chat's read-only sandbox keeps network on ([`sandbox_policy`]).
    tools: ToolSet,
    /// What its workspace sandbox writes besides its working directory.
    writable_roots: Vec<PathBuf>,
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
                    sandbox_policy: Some(sandbox_policy(
                        self.access,
                        self.tools,
                        &self.writable_roots,
                    )),
                    approval_policy: Some(approval_policy(self.access)),
                    approvals_reviewer: Some(REVIEWER),
                    ..Default::default()
                },
            )
            .await?;
        crate::logging::private_trace!(self.rpc.process.private_logs(), debug, "codex turn started"; turn = %started.turn.id);
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

    fn stop_task(&self, task_id: String) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let _: Value = self
                .rpc
                .call(
                    "thread/backgroundTerminals/terminate",
                    &json!({ "threadId": self.thread_id, "processId": task_id }),
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
                // An elicitation's answer (`mcpServer/elicitation/request`); declined, the
                // model reads "user rejected MCP tool call".
            };
            self.rpc.respond(rpc_id, result).await?;
            // A decline carries no words (the app-server's approval responses, 0.162.1), so
            // the reason reaches the model as a message into its turn.
            if let ApprovalDecision::Deny { message } = &decision
                && !message.trim().is_empty()
                && let Some(turn_id) = self.turn_id()
            {
                let why = TurnInput::text(format!("[Declined] {message}"));
                if let Err(err) = self.steer_turn(turn_id, &why).await {
                    crate::logging::private_trace!(self.rpc.process.private_logs(), debug, "could not pass a decline's reason"; thread = %self.thread_id, error = %err);
                }
            }
            Ok(())
        })
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if self.rpc.process.is_running() && !self.ephemeral {
                match tokio::time::timeout(EXIT_GRACE, archive_thread(&self.rpc, &self.thread_id))
                    .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => {
                        crate::logging::private_trace!(self.rpc.process.private_logs(), debug, "could not archive a codex thread"; thread = %self.thread_id, error = %err);
                    }
                    Err(_) => {
                        crate::logging::private_trace!(self.rpc.process.private_logs(), debug, "archiving a codex thread timed out"; thread = %self.thread_id);
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

/// Lists the thread's MCP servers with their state and tools, as
/// [`ProviderEvent::McpServers`]. It runs apart, once the thread is open; asked again while it
/// runs, it lists once more after. A refusal ends the listing for this session (an app-server
/// without it), quietly: the startup steps still tell each server's state.
fn list_mcp_servers(rpc: &Arc<Rpc>, shared: &Arc<Shared>) {
    if !shared.mcp_listing.load(Ordering::Acquire) {
        return;
    }
    let Some(thread) = lock(&shared.thread).clone() else {
        return;
    };
    {
        let mut relist = lock(&shared.mcp_relist);
        if relist.running {
            relist.again = true;
            return;
        }
        relist.running = true;
    }
    let (rpc, shared) = (rpc.clone(), shared.clone());
    tokio::spawn(async move {
        loop {
            lock(&shared.mcp_relist).again = false;
            let params = p::ListMcpServerStatusParams {
                thread_id: Some(thread.clone()),
                detail: Some(p::McpServerStatusDetail::ToolsAndAuthOnly),
                ..Default::default()
            };
            match rpc
                .call::<_, p::ListMcpServerStatusResponse>("mcpServerStatus/list", &params)
                .await
            {
                Ok(listed) => {
                    let servers = parse::listed_mcp_servers(listed.data, &shared.own_mcp);
                    shared
                        .events
                        .send(ProviderEvent::McpServers {
                            servers,
                            at_start: false,
                        })
                        .await;
                }
                Err(err @ Error::Timeout(_)) => {
                    crate::logging::private_trace!(rpc.process.private_logs(), debug, "Codex did not list its MCP servers in time"; thread = %thread, error = %err);
                }
                Err(err) => {
                    shared.mcp_listing.store(false, Ordering::Release);
                    crate::logging::private_trace!(rpc.process.private_logs(), debug, "Codex does not list its MCP servers here"; thread = %thread, error = %err);
                }
            }
            let mut relist = lock(&shared.mcp_relist);
            if !relist.again || !shared.mcp_listing.load(Ordering::Acquire) {
                relist.running = false;
                return;
            }
        }
    });
}

/// Serves a session's app-server output until it exits. `trusted` names the MCP servers whose
/// tool calls run without asking.
async fn read_loop(
    rpc: Arc<Rpc>,
    mut stdout: mpsc::Receiver<String>,
    shared: Arc<Shared>,
    trusted: HashSet<String>,
) {
    let mut parser = Parser::live().with_private_logs(rpc.process.private_logs());
    parser.own_mcp = shared.own_mcp.clone();
    while let Some(line) = stdout.recv().await {
        if shared.compact_asked.swap(false, Ordering::AcqRel) {
            parser.compact_requested();
        }
        let outputs = parser.feed(&line);
        // Before its events go: whoever hears a turn start can interrupt it.
        *lock(&shared.turn_id) = parser.turn_id().map(str::to_owned);
        for output in outputs {
            match output {
                Output::Event(event) => {
                    // A server that came up or gave up: its tools and state, as Codex lists
                    // them now.
                    let settled = matches!(
                        &event,
                        ProviderEvent::McpServerChanged { server }
                            if server.status != McpStatus::Connecting
                    );
                    shared.events.send(event).await;
                    if settled {
                        list_mcp_servers(&rpc, &shared);
                    }
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
                    ..
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
    }

    let exit = rpc.process.exited().await;
    rpc.fail_all();
    lock(&shared.turn_id).take();
    shared
        .events
        .send(ProviderEvent::Exited {
            code: exit.code,
            signal: exit.signal,
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

/// How often the thread's background terminals are listed while a turn runs or some run.
const TERMINALS_EVERY: Duration = Duration::from_secs(2);

/// Follows the thread's background terminals as Codex's `/ps` lists them
/// (`thread/backgroundTerminals/list`): each new one is reported running, each one gone,
/// ended. Listed while a turn runs (only a turn starts one), once as it ends, and while any
/// run; it stops with the app-server, or for good when the app-server won't list them.
async fn watch_background_terminals(rpc: Arc<Rpc>, shared: Arc<Shared>, thread: String) {
    let mut known: HashMap<String, BackgroundTask> = HashMap::new();
    let mut was_active = false;
    while rpc.process.is_running() {
        tokio::time::sleep(TERMINALS_EVERY).await;
        let active = lock(&shared.turn_id).is_some();
        let ended = std::mem::replace(&mut was_active, active) && !active;
        if !active && !ended && known.is_empty() {
            continue;
        }
        let listed = match rpc
            .call::<_, Value>(
                "thread/backgroundTerminals/list",
                &json!({ "threadId": thread }),
            )
            .await
        {
            Ok(listed) => listed,
            Err(Error::Timeout(_)) => continue,
            Err(err) => {
                crate::logging::private_trace!(rpc.process.private_logs(), debug, "Codex does not list its background terminals here"; thread = %thread, error = %err);
                return;
            }
        };
        for task in parse::background_terminals(&listed, &mut known, now_ms()) {
            shared
                .events
                .send(ProviderEvent::BackgroundTask { task })
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_read_as_people_name_them() {
        assert_eq!(plan_name(p::PlanType::Prolite), Some("Pro Lite"));
        assert_eq!(plan_name(p::PlanType::Plus), Some("Plus"));
        assert_eq!(
            plan_name(p::PlanType::SelfServeBusinessUsageBased),
            Some("Business")
        );
        assert_eq!(plan_name(p::PlanType::Ent26), Some("Enterprise"));
        assert_eq!(plan_name(p::PlanType::Unknown), None);
    }

    /// Codex's own browser stays off for every session: agents never get a browser of their
    /// own beside Brigadier's (the user's alone, gated by Settings › Browser).
    #[test]
    fn every_session_runs_without_codex_browser_use() {
        let args = app_server_args(None);
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "--disable" && pair[1] == "browser_use"),
            "{args:?}"
        );
    }

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

    pub(super) fn spec(tools: ToolSet, allowed_models: Option<AllowedModels>) -> SessionSpec {
        SessionSpec {
            cwd: PathBuf::from("/tmp"),
            model: Some("gpt-6.1-sol".into()),
            effort: None,
            fast: false,
            origin: Origin::New,
            ephemeral: false,
            access: Access::Full,
            append_system_prompt: None,
            mcp_servers: Vec::new(),
            tools,
            skill_plugins: Vec::new(),
            installed_skills: Vec::new(),
            switched_off: Default::default(),
            add_dirs: Vec::new(),
            project: None,
            env: Vec::new(),
            unset_env: Vec::new(),
            low_priority: false,
            record_to: None,
            redactor: None,
            owned_cwd: false,
            sweep_dir: None,
            auto_compact: true,
            allowed_models,
            omit_ai_coauthors: false,
            output_hook: None,
            stop_hook: None,
            deny_tools: None,
            lean_worker: false,
        }
    }

    fn without_sub_agents(args: &[String]) -> bool {
        args.windows(2)
            .any(|pair| pair[0] == "-c" && pair[1] == NO_SUB_AGENTS)
    }

    /// What `spec`'s opening call sends: its method and its `ephemeral` field.
    fn opened(spec: &SessionSpec) -> (&'static str, Option<Value>) {
        let start = p::ThreadStartParams {
            model: spec.model.clone(),
            ..Default::default()
        };
        let (method, params) = match opening(spec, start) {
            Opening::Start(params) => ("thread/start", serde_json::to_value(params)),
            Opening::Resume(params) => ("thread/resume", serde_json::to_value(params)),
            Opening::Fork(params) => ("thread/fork", serde_json::to_value(params)),
        };
        let params = params.unwrap();
        assert_eq!(params["model"], "gpt-6.1-sol");
        (method, params.get("ephemeral").cloned())
    }

    /// STRIP-DOWN-PLAN S17: Brigadier's instructions reach a Codex thread word for word as its
    /// developer instructions, whether it starts, resumes or forks, and the app-server gets the
    /// session's folder and ledger in its environment.
    #[test]
    fn the_standing_instructions_and_their_env_reach_codex_as_given() {
        let text = "This is a plain Claude Code / Codex session inside Brigadier.\nKeep a ledger at /tmp/s/record/ledger.md as you work. `brigadier ledger add` works in every role except chats beside a tab; `brigadier ledger show` reads it back.";
        let env = vec![
            ("BRIGADIER_SESSION_DIR".to_owned(), "/s/abc".to_owned()),
            (
                "BRIGADIER_LEDGER".to_owned(),
                "/s/abc/record/ledger.md".to_owned(),
            ),
        ];
        for tools in [ToolSet::Thread, ToolSet::Lean, ToolSet::SideChat] {
            for origin in [
                Origin::New,
                Origin::Resume {
                    native_id: "t1".into(),
                },
                Origin::Fork {
                    native_id: "t1".into(),
                },
            ] {
                let session = SessionSpec {
                    origin,
                    append_system_prompt: Some(text.into()),
                    env: env.clone(),
                    ..spec(tools, None)
                };
                let start = start_params(&session, Path::new("/tmp"), Map::new());
                let params = match opening(&session, start) {
                    Opening::Start(params) => serde_json::to_value(params),
                    Opening::Resume(params) => serde_json::to_value(params),
                    Opening::Fork(params) => serde_json::to_value(params),
                }
                .unwrap();
                assert_eq!(params["developerInstructions"], json!(text), "{params}");
            }
            let codex = Codex::new(
                brigadier_sandbox::native(brigadier_sandbox::PlatformOptions::default()).unwrap(),
                Arc::new(CliEnv::from_vars([("HOME".into(), "/home/me".into())])),
            );
            let session = SessionSpec {
                env: env.clone(),
                ..spec(tools, None)
            };
            let server =
                codex.server_spec(Path::new("/bin/codex"), Path::new("/tmp"), Some(&session));
            for (name, value) in &env {
                assert!(
                    server
                        .env
                        .iter()
                        .any(|(key, set)| key.to_str() == Some(name) && set.to_str() == Some(value)),
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn only_a_one_shot_runs_new_or_forked_thread_is_ephemeral() {
        let with = |origin: Origin, ephemeral: bool| SessionSpec {
            origin,
            ephemeral,
            ..spec(ToolSet::None, None)
        };
        let fork = || Origin::Fork {
            native_id: "t1".into(),
        };
        let resume = || Origin::Resume {
            native_id: "t1".into(),
        };
        assert_eq!(
            opened(&with(Origin::New, true)),
            ("thread/start", Some(json!(true)))
        );
        assert_eq!(
            opened(&with(fork(), true)),
            ("thread/fork", Some(json!(true)))
        );
        // Resumable threads are opened as before: saved.
        assert_eq!(opened(&with(Origin::New, false)), ("thread/start", None));
        assert_eq!(opened(&with(fork(), false)), ("thread/fork", None));
        // A resumed thread was saved; resume takes no such field.
        assert_eq!(opened(&with(resume(), true)), ("thread/resume", None));
        assert_eq!(opened(&with(resume(), false)), ("thread/resume", None));
    }

    #[test]
    fn deleting_a_thread_that_was_never_saved_is_nothing_to_do() {
        // What codex-cli 0.162.1 answers `thread/delete` for an ephemeral thread, while its
        // app-server runs and after.
        assert!(already_gone(
            "thread is not persisted and cannot be deleted: 01a127ee-2c00-7dd0-b6d7-2834a44cb8fa"
        ));
        assert!(already_gone(
            "no rollout found for thread id 01a127ed-edbb-7160-85ba-837c49f64a14"
        ));
        assert!(already_gone("thread not found: 01a1"));
        assert!(!already_gone("permission denied"));
    }

    /// Threads keep the user's skills catalog; workers omit it but keep plugins, hooks and
    /// MCP servers. Every other session, and a control connection, keeps a sealed setup.
    /// Apps, computer and browser use and memories stay off for all.
    #[test]
    fn threads_and_workers_run_with_the_users_own_setup() {
        let disabled = |args: &[String]| -> Vec<String> {
            args.windows(2)
                .filter(|pair| pair[0] == "--disable")
                .map(|pair| pair[1].clone())
                .collect()
        };
        for tools in [ToolSet::Thread, ToolSet::Lean] {
            let args = app_server_args(Some(&spec(tools, None)));
            let off = disabled(&args);
            for feature in USER_SETUP_FEATURES {
                assert!(!off.iter().any(|f| f == feature), "{tools:?}: {args:?}");
            }
            for feature in DISABLED_FEATURES {
                assert!(off.iter().any(|f| f == feature), "{tools:?}: {args:?}");
            }
            // Threads and workers alike see the skills' descriptions (PLAN.md §6 Phase 7).
            assert!(
                !args.iter().any(|arg| arg == NO_SKILLS_CATALOG),
                "{tools:?}: {args:?}"
            );
            // No override disables the user's inherited server, and Brigadier adds none of its
            // own (S16c): the session's MCP servers are the user's.
            let effective = json!({ "config": { "mcp_servers": {
                "personal": { "command": "personal-mcp", "enabled": true }
            }}});
            let servers = mcp_config(&effective, &spec(tools, None));
            assert!(servers.is_empty(), "{tools:?}: {servers:?}");
        }
        for args in [
            ToolSet::Default,
            ToolSet::None,
            ToolSet::Web,
            ToolSet::SideChat,
            ToolSet::Review,
            ToolSet::TabChat,
        ]
        .map(|tools| app_server_args(Some(&spec(tools, None))))
        .into_iter()
        .chain([app_server_args(None)])
        {
            let off = disabled(&args);
            for feature in USER_SETUP_FEATURES {
                assert!(off.iter().any(|f| f == feature), "{args:?}");
            }
        }
        let review = app_server_args(Some(&spec(ToolSet::Review, None)));
        assert!(review.iter().any(|arg| arg == NO_SKILLS_CATALOG));
    }

    /// A side chat and a tab chat generate images (Q24) and have a shell, for `brigadier` (S16c), and
    /// are otherwise locked down as before: no viewing of local files, sub-agents, goals or
    /// sleep tool, the web as it was. Every one-shot role (a handoff note, a git job or
    /// escalation, research, a review) still goes without image generation or a shell.
    #[test]
    fn only_a_chat_of_the_locked_down_sessions_generates_images() {
        let disabled = |args: &[String]| -> Vec<String> {
            args.windows(2)
                .filter(|pair| pair[0] == "--disable")
                .map(|pair| pair[1].clone())
                .collect()
        };
        for (tools, locked_like, web) in [
            (ToolSet::SideChat, ToolSet::Web, Some("live")),
            (ToolSet::TabChat, ToolSet::Review, Some("disabled")),
        ] {
            let args = app_server_args(Some(&spec(tools, None)));
            let mut off = disabled(&args);
            assert!(!off.iter().any(|f| f == "image_generation"), "{args:?}");
            // Codex's memory follows the user's setting in a Chat (S19).
            off.retain(|f| f != MEMORIES);
            let mut before = disabled(&app_server_args(Some(&spec(locked_like, None))));
            before.retain(|f| {
                !matches!(
                    f.as_str(),
                    "image_generation" | "shell_tool" | "unified_exec" | MEMORIES
                )
            });
            assert_eq!(off, before, "{tools:?}");
            assert!(without_sub_agents(&args), "{args:?}");
            assert!(args.iter().any(|arg| arg == NO_SKILLS_CATALOG));
            assert_eq!(web_search(tools), web_search(locked_like));
            assert_eq!(web_search(tools), web);
        }
        for tools in [ToolSet::None, ToolSet::Web, ToolSet::Review] {
            let off = disabled(&app_server_args(Some(&spec(tools, None))));
            for feature in RESTRICTED_FEATURES {
                assert!(off.iter().any(|f| f == feature), "{tools:?}: {off:?}");
            }
        }
        // A worker and a thread make images.
        for tools in [ToolSet::Lean, ToolSet::Default, ToolSet::Thread] {
            let off = disabled(&app_server_args(Some(&spec(tools, None))));
            assert!(!off.iter().any(|f| f == "image_generation"), "{tools:?}");
        }
    }

    /// S16c: a side chat's and a tab chat's commands run in Codex's read-only sandbox with network
    /// on, so `brigadier` reaches the daemon's socket; Codex asks about every command not on
    /// its own safe list (`untrusted`). A reviewer's read-only sandbox stays offline.
    #[test]
    fn a_side_chats_shell_runs_read_only_with_network_for_brigadiers_socket() {
        for tools in [ToolSet::SideChat, ToolSet::TabChat] {
            let mut chat = spec(tools, None);
            chat.access = Access::ReadOnly;
            let off = disabled_features(&app_server_args(Some(&chat)));
            for shell in ["shell_tool", "unified_exec"] {
                assert!(!off.iter().any(|f| f == shell), "{tools:?}: {off:?}");
            }
            assert_eq!(thread_sandbox(chat.access), Some(p::SandboxMode::ReadOnly));
            assert_eq!(
                serde_json::to_value(approval_policy(chat.access)).unwrap(),
                json!("untrusted")
            );
            let policy = sandbox_policy(chat.access, tools, &writable_roots(&chat));
            assert!(
                matches!(
                    policy,
                    p::SandboxPolicy::ReadOnly {
                        network_access: true
                    }
                ),
                "{tools:?}: {policy:?}"
            );
        }
        for tools in [ToolSet::Review, ToolSet::Web] {
            let policy = sandbox_policy(Access::ReadOnly, tools, &[]);
            assert!(
                matches!(
                    policy,
                    p::SandboxPolicy::ReadOnly {
                        network_access: false
                    }
                ),
                "{tools:?}: {policy:?}"
            );
        }
    }

    fn disabled_features(args: &[String]) -> Vec<String> {
        args.windows(2)
            .filter(|pair| pair[0] == "--disable")
            .map(|pair| pair[1].clone())
            .collect()
    }

    /// A thread's command timeouts are Codex's own (S2): Brigadier sets no ceiling of its own,
    /// and a project's own `background_terminal_max_timeout` passes through unchanged
    /// ([`project_options_keep_thread_controls_out_and_require_trust`]).
    #[test]
    fn a_thread_gets_no_command_timeout_of_brigadiers() {
        let args = app_server_args(Some(&spec(ToolSet::Thread, None)));
        assert!(!args.iter().any(|arg| arg.contains("timeout")), "{args:?}");
    }

    #[test]
    fn project_options_keep_thread_controls_out_and_require_trust() {
        let mut read = json!({ "layers": [{
            "name": { "type": "project", "dotCodexFolder": "/repo/.codex" },
            "config": {
                "shell_environment_policy": { "set": { "PROJECT": "yes" } },
                "tools": { "update_plan": { "enabled": false } },
                "tool_output_token_limit": 1234,
                "background_terminal_max_timeout": 2000,
                "model": "other", "model_reasoning_effort": "low",
                "sandbox_mode": "danger-full-access", "approval_policy": "never",
                "allow_login_shell": true, "features": { "hooks": true },
                "hooks": { "SessionStart": [] }, "profile": "other",
                "profiles": { "other": { "model": "other" } },
                "web_search": "disabled", "developer_instructions": "replace"
            }
        }] });
        assert_eq!(
            Value::Object(project_options(&read)),
            json!({
                "shell_environment_policy": { "set": { "PROJECT": "yes" } },
                "tools": { "update_plan": { "enabled": false } },
                "tool_output_token_limit": 1234,
                "background_terminal_max_timeout": 2000
            })
        );
        read["config"] = json!({ "shell_environment_policy": {
            "inherit": "core", "set": { "PROJECT": "yes", "PERSONAL": "kept" },
            "exclude": null, "include_only": null, "experimental_use_profile": null
        }, "tools": { "update_plan": { "enabled": false }, "web_search": null } });
        assert_eq!(
            project_options(&read)["shell_environment_policy"],
            json!({ "inherit": "core", "set": { "PROJECT": "yes", "PERSONAL": "kept" } })
        );
        assert_eq!(
            project_options(&read)["tools"],
            json!({ "update_plan": { "enabled": false } })
        );
        read["layers"][0]["disabledReason"] = json!("untrusted");
        assert!(project_options(&read).is_empty());
        assert!(project_options(&json!({})).is_empty());
    }

    /// A command Codex runs sees the session's `BRIGADIER_*` variables whatever the user's
    /// policy inherits: they join its `set`, the rest of the policy kept, `PATH` untouched.
    #[test]
    fn commands_see_brigadiers_variables_under_the_users_shell_policy() {
        let mut session = spec(ToolSet::Thread, None);
        session.env = vec![
            ("TMPDIR".into(), "/t".into()),
            ("PATH".into(), "/app/bin:/usr/bin".into()),
            ("BRIGADIER_SESSION".into(), "s-1".into()),
            ("BRIGADIER_SESSION_GRANT".into(), "brg_1".into()),
        ];
        let effective = json!({ "config": { "shell_environment_policy": {
            "inherit": "core", "set": { "MINE": "kept" }, "exclude": ["X*"],
            "include_only": null
        } } });
        let mut config = Map::new();
        shell_sees_brigadier(&mut config, &effective, &session);
        assert_eq!(
            config["shell_environment_policy"],
            json!({ "inherit": "core", "exclude": ["X*"], "set": {
                "MINE": "kept", "BRIGADIER_SESSION": "s-1", "BRIGADIER_SESSION_GRANT": "brg_1"
            } })
        );
        // A project's merged policy is the one added to; with none at all, only `set`.
        let mut config = Map::new();
        config.insert(
            "shell_environment_policy".into(),
            json!({ "inherit": "all" }),
        );
        shell_sees_brigadier(&mut config, &json!({}), &session);
        assert_eq!(
            config["shell_environment_policy"]["set"]["BRIGADIER_SESSION"],
            "s-1"
        );
        assert_eq!(config["shell_environment_policy"]["inherit"], "all");
        let mut config = Map::new();
        shell_sees_brigadier(&mut config, &json!({}), &session);
        assert_eq!(
            config["shell_environment_policy"],
            json!({ "set": { "BRIGADIER_SESSION": "s-1", "BRIGADIER_SESSION_GRANT": "brg_1" } })
        );
        // A session without them leaves the user's policy alone.
        session
            .env
            .retain(|(name, _)| !name.starts_with("BRIGADIER_"));
        let mut config = Map::new();
        shell_sees_brigadier(&mut config, &effective, &session);
        assert!(config.is_empty());
    }

    #[test]
    fn project_options_include_inherited_project_layers() {
        let read = json!({
            "layers": [
                { "name": { "type": "project", "dotCodexFolder": "/workspace/repo/.codex" },
                  "config": { "tool_output_token_limit": 1234 } },
                { "name": { "type": "project", "dotCodexFolder": "/workspace/.codex" },
                  "config": { "shell_environment_policy": { "set": { "PROJECT": "yes" } } } },
                { "name": { "type": "project", "dotCodexFolder": "/ignored/.codex" },
                  "disabledReason": "untrusted", "config": { "tools": { "view_image": false } } },
            ],
            "config": {
                "tool_output_token_limit": 1234,
                "shell_environment_policy": { "set": { "PROJECT": "yes", "PERSONAL": "kept" } }
            }
        });
        assert_eq!(
            Value::Object(project_options(&read)),
            json!({
                "tool_output_token_limit": 1234,
                "shell_environment_policy": { "set": { "PROJECT": "yes", "PERSONAL": "kept" } }
            })
        );
    }

    /// A trusted project's own Codex MCP servers, for a thread working outside it: started in
    /// the project; none while Codex leaves the project's config unloaded (#315).
    #[test]
    fn a_trusted_projects_codex_servers_start_in_the_project() {
        let read = |disabled: Option<&str>| {
            json!({ "layers": [
                { "name": { "type": "sessionFlags" }, "config": {} },
                {
                    "name": { "type": "project", "dotCodexFolder": "/repo/wt/.codex" },
                    "config": { "mcp_servers": {
                        "docs": { "command": "node", "args": ["scripts/mcp.js"] },
                        "own": { "command": "uv", "cwd": "/elsewhere" },
                        "remote": { "url": "https://mcp.example.invalid" },
                    }},
                    "disabledReason": disabled,
                },
                { "name": { "type": "user", "file": "/home/u/.codex/config.toml" },
                  "config": { "mcp_servers": { "personal": { "command": "p" } } } },
            ]})
        };
        let servers = project_servers(&read(None));
        assert_eq!(
            Value::Object(servers),
            json!({
                "docs": { "command": "node", "args": ["scripts/mcp.js"], "cwd": "/repo/wt" },
                "own": { "command": "uv", "cwd": "/elsewhere" },
                "remote": { "url": "https://mcp.example.invalid" },
            })
        );
        assert!(project_servers(&read(Some("add it as a trusted project"))).is_empty());
        assert!(project_servers(&json!({})).is_empty());
    }

    #[test]
    fn project_servers_include_inherited_layers_with_effective_settings() {
        let read = json!({
            "layers": [
                { "name": { "type": "project", "dotCodexFolder": "/workspace/repo/.codex" },
                  "config": { "mcp_servers": {
                      "repository": { "command": "node", "args": ["repo.js"] },
                      "shared": { "args": ["child.js"] }
                  } } },
                { "name": { "type": "project", "dotCodexFolder": "/workspace/.codex" },
                  "config": { "mcp_servers": {
                      "workspace": { "command": "node", "args": ["workspace.js"] },
                      "shared": { "command": "node", "args": ["parent.js"],
                                  "env": { "PARENT": "kept" } },
                      "own": { "command": "uv", "cwd": "/elsewhere" },
                      "remote": { "url": "https://mcp.example.invalid" }
                  } } },
                { "name": { "type": "project", "dotCodexFolder": "/ignored/.codex" },
                  "disabledReason": "untrusted",
                  "config": { "mcp_servers": { "ignored": { "command": "false" } } } },
                { "name": { "type": "user" },
                  "config": { "mcp_servers": { "personal": { "command": "false" } } } }
            ],
            "config": { "mcp_servers": {
                "repository": { "command": "node", "args": ["repo.js"],
                                "tool_timeout_sec": null },
                "workspace": { "command": "node", "args": ["workspace.js"] },
                "shared": { "command": "node", "args": ["child.js"],
                            "env": { "PARENT": "kept" }, "cwd": null },
                "own": { "command": "uv", "cwd": "/elsewhere" },
                "remote": { "url": "https://mcp.example.invalid" },
                "personal": { "command": "false" }
            } }
        });
        assert_eq!(
            Value::Object(project_servers(&read)),
            json!({
                "repository": { "command": "node", "args": ["repo.js"],
                                "cwd": "/workspace/repo" },
                "workspace": { "command": "node", "args": ["workspace.js"],
                               "cwd": "/workspace" },
                "shared": { "command": "node", "args": ["child.js"],
                            "env": { "PARENT": "kept" }, "cwd": "/workspace/repo" },
                "own": { "command": "uv", "cwd": "/elsewhere" },
                "remote": { "url": "https://mcp.example.invalid" }
            })
        );
    }

    #[test]
    fn a_session_with_limited_models_has_no_sub_agents() {
        let worker = spec(ToolSet::Lean, Some(AllowedModels::default()));
        assert!(without_sub_agents(&app_server_args(Some(&worker))));
        let raw = spec(ToolSet::Default, None);
        assert!(!without_sub_agents(&app_server_args(Some(&raw))));
        assert!(!without_sub_agents(&app_server_args(None)));
        // Restricted sessions never had them, and say so once.
        let restricted = spec(ToolSet::None, Some(AllowedModels::default()));
        let args = app_server_args(Some(&restricted));
        assert_eq!(args.iter().filter(|arg| *arg == NO_SUB_AGENTS).count(), 1);
    }

    /// STRIP-DOWN-PLAN S19: Codex's own memory follows the user's setting. Every session reads
    /// the user's `config.toml`, so a thread, a worker and a Chat are given nothing about memory
    /// and start with the user's `features.memories`, on or off. Only a lean worker (G23), a side
    /// chat, Brigadier's own one-shots and a control connection go without it.
    #[test]
    fn codexs_memory_follows_the_users_setting() {
        let about_memory = |args: &[String]| -> Vec<String> {
            args.iter()
                .filter(|arg| arg.contains("memor"))
                .cloned()
                .collect()
        };
        let off = |args: &[String]| {
            args.windows(2)
                .any(|pair| pair[0] == "--disable" && pair[1] == MEMORIES)
        };
        for tools in [
            ToolSet::Thread,
            ToolSet::Lean,
            ToolSet::Default,
            ToolSet::SideChat,
        ] {
            let args = app_server_args(Some(&spec(tools, None)));
            assert!(about_memory(&args).is_empty(), "{tools:?}: {args:?}");
        }
        let mut lean = spec(ToolSet::Lean, None);
        lean.lean_worker = true;
        assert!(off(&app_server_args(Some(&lean))), "a lean worker");
        for tools in [
            ToolSet::TabChat,
            ToolSet::Review,
            ToolSet::Web,
            ToolSet::None,
        ] {
            let args = app_server_args(Some(&spec(tools, None)));
            assert!(off(&args), "{tools:?}: {args:?}");
        }
        assert!(off(&app_server_args(None)), "a control connection");
    }

    #[test]
    fn a_thread_keeps_codexs_own_tools_and_sub_agents() {
        let thread = spec(ToolSet::Thread, None);
        let args = app_server_args(Some(&thread));
        let disabled: Vec<&str> = args
            .windows(2)
            .filter(|pair| pair[0] == "--disable")
            .map(|pair| pair[1].as_str())
            .collect();
        // Only what is off for every session: sub-agents, goals, the sleep tool and the shell
        // are Codex's own, as in a plain CLI (STRIP-DOWN-PLAN S3).
        assert_eq!(disabled, DISABLED_FEATURES, "{args:?}");
        assert!(!without_sub_agents(&args), "{args:?}");
        let overrides: Vec<&str> = args
            .windows(2)
            .filter(|pair| pair[0] == "-c")
            .map(|pair| pair[1].as_str())
            .collect();
        assert!(
            !overrides.iter().any(|value| value.starts_with("agents.")),
            "{args:?}"
        );
    }

    /// Each level passes straight through as Codex's own approval policy and sandbox, for a
    /// thread and a worker alike, in the app-server and in a terminal (G5). Below Full access
    /// the workspace sandbox writes the session's extra folders; the user answers every
    /// request.
    #[test]
    fn each_level_is_codexs_own_approval_policy_and_sandbox_for_threads_and_workers() {
        let dir =
            std::env::temp_dir().join(format!("brigadier-codex-levels-{}", std::process::id()));
        let workspace = dir.join("session");
        std::fs::create_dir_all(&workspace).unwrap();
        let real = workspace.canonicalize().unwrap().display().to_string();
        let env = CliEnv::from_vars([("PATH".into(), "/usr/bin".into())]);
        for tools in [ToolSet::Thread, ToolSet::Lean] {
            for (access, approval, sandbox, flags) in [
                (
                    Access::Ask,
                    "untrusted",
                    "workspace-write",
                    "-s workspace-write -c approval_policy=\"untrusted\"",
                ),
                (
                    Access::AcceptEdits,
                    "on-request",
                    "workspace-write",
                    "-s workspace-write -a on-request",
                ),
                (
                    Access::Full,
                    "never",
                    "danger-full-access",
                    "--dangerously-bypass-approvals-and-sandbox",
                ),
            ] {
                let mut session = spec(tools, None);
                session.access = access;
                session.add_dirs = vec![workspace.clone()];
                assert_eq!(
                    serde_json::to_value(approval_policy(access)).unwrap(),
                    json!(approval),
                    "{tools:?} {access:?}"
                );
                assert_eq!(thread_sandbox(access), None, "every turn sets it");
                let policy = sandbox_policy(access, tools, &writable_roots(&session));
                match (&policy, sandbox) {
                    (p::SandboxPolicy::DangerFullAccess, "danger-full-access") => {}
                    (
                        p::SandboxPolicy::WorkspaceWrite {
                            writable_roots,
                            network_access,
                            ..
                        },
                        "workspace-write",
                    ) => {
                        let roots: Vec<&str> =
                            writable_roots.iter().map(|root| root.0.as_str()).collect();
                        assert_eq!(roots, [real.as_str()]);
                        assert!(*network_access);
                    }
                    _ => panic!("{access:?}: {policy:?}"),
                }
                let command = terminal_command(
                    "/usr/local/bin/codex".into(),
                    &session,
                    "thread-1",
                    "/tmp".into(),
                    Map::new(),
                    &env,
                );
                let line = command.args.join(" ");
                assert!(line.contains(flags), "{access:?}: {line}");
                assert!(!line.contains("--approve-for-me"), "{line}");
            }
        }
        assert_eq!(REVIEWER, p::ApprovalsReviewer::User);
        // Brigadier's own readers: read-only, and Codex asks before anything not known safe.
        assert_eq!(
            thread_sandbox(Access::ReadOnly),
            Some(p::SandboxMode::ReadOnly)
        );
        assert_eq!(
            serde_json::to_value(approval_policy(Access::ReadOnly)).unwrap(),
            json!("untrusted")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An installed skill lands in the user's skills folder, where a plain `codex` finds it
    /// too, and is written again only when it changed (S15).
    #[test]
    fn installed_skills_go_into_the_users_skills_folder() {
        let dir = std::env::temp_dir().join(format!("brigadier-codex-{}", uuid::Uuid::new_v4()));
        let plugin = dir.join("brigade");
        std::fs::create_dir_all(plugin.join("skills/brigade")).unwrap();
        std::fs::write(plugin.join("skills/brigade/SKILL.md"), "one").unwrap();
        let skills = dir.join("codex-home/skills");
        std::fs::create_dir_all(skills.join("theirs")).unwrap();
        install_skills(&skills, std::slice::from_ref(&plugin)).unwrap();
        let installed = skills.join("brigade/SKILL.md");
        assert_eq!(std::fs::read_to_string(&installed).unwrap(), "one");
        std::fs::write(plugin.join("skills/brigade/SKILL.md"), "two").unwrap();
        install_skills(&skills, std::slice::from_ref(&plugin)).unwrap();
        assert_eq!(std::fs::read_to_string(&installed).unwrap(), "two");
        // The user's own skills stay.
        assert!(skills.join("theirs").is_dir());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Missing or signed out, the status points at Connect in plain words: no command to run,
    /// no terminal (#323).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_missing_or_signed_out_codex_points_at_connect() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("brigadier-codex-{}", uuid::Uuid::new_v4()));
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(dir.join("data")),
        })
        .unwrap();
        let codex = |env| Codex::new(platform.clone(), Arc::new(env));
        let env = || {
            CliEnv::from_vars([
                ("HOME".into(), dir.as_os_str().to_owned()),
                ("PATH".into(), bin.as_os_str().to_owned()),
            ])
        };
        let missing = codex(env()).status().await;
        assert_eq!(missing.guidance.as_deref(), Some(NOT_INSTALLED));
        let binary = bin.join("codex");
        std::fs::write(
            &binary,
            "#!/bin/sh\nif [ \"$1\" = login ]; then echo 'Not logged in' >&2; exit 1; else echo 'codex-cli 0.163.0'; fi\n",
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let signed_out = codex(env()).status().await;
        assert!(!signed_out.logged_in);
        assert_eq!(signed_out.guidance.as_deref(), Some(SIGNED_OUT));
        for text in [NOT_INSTALLED, SIGNED_OUT] {
            assert!(!text.contains('`') && !text.contains("terminal"), "{text}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn server(name: &str, transport: McpTransport, env: &[(&str, &str)]) -> McpServer {
        McpServer {
            name: name.into(),
            transport,
            env: env
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            tool_timeout_secs: None,
            trusted: false,
            always_load: false,
        }
    }

    fn stdio(command: &str, cwd: Option<&str>) -> McpTransport {
        McpTransport::Stdio {
            command: command.into(),
            args: vec!["serve".into()],
            cwd: cwd.map(PathBuf::from),
        }
    }

    /// A thread's config gives each server its own values (two servers may give one name
    /// different ones), an HTTP server its URL and headers, a stdio server its folder, and
    /// switches off what the user switched off, a project's copied server included.
    #[test]
    fn thread_config_servers_by_transport_and_switched_off() {
        let mut session = spec(ToolSet::Lean, None);
        session.mcp_servers = vec![
            server("one", stdio("/bin/one", None), &[("TOKEN", "a")]),
            server(
                "two",
                stdio("/bin/two", Some("/srv/two")),
                &[("TOKEN", "b")],
            ),
            server(
                "web",
                McpTransport::Http {
                    url: "https://mcp.example.com/mcp".into(),
                },
                &[("Authorization", "Bearer c")],
            ),
        ];
        session.switched_off.servers = vec!["railway".into()];
        let servers = mcp_config(&json!({}), &session);
        assert_eq!(servers["one"]["env"], json!({"TOKEN": "a"}));
        assert!(servers["one"].get("cwd").is_none());
        assert_eq!(servers["two"]["env"], json!({"TOKEN": "b"}));
        assert_eq!(servers["two"]["cwd"], "/srv/two");
        assert_eq!(
            servers["web"],
            json!({"url": "https://mcp.example.com/mcp",
                   "http_headers": {"Authorization": "Bearer c"}})
        );
        assert_eq!(servers["railway"], json!({"enabled": false}));
    }

    /// A lean worker (`brigadier start --lean`, G23) goes without every server of the user's
    /// config and writes no memories; the skills it goes without come switched off by file,
    /// and those `--skill` kept are simply not among them.
    #[test]
    fn a_lean_worker_goes_without_the_users_servers_and_memories() {
        let effective = json!({"config": {"mcp_servers": {"railway": {"command": "r"},
                                                          "docs": {"url": "https://d"}}}});
        let mut session = spec(ToolSet::Lean, None);
        session.mcp_servers = vec![server("given", stdio("/bin/given", None), &[])];
        let full = mcp_config(&effective, &session);
        assert!(full.get("railway").is_none() && full.get("docs").is_none());
        session.lean_worker = true;
        let lean = mcp_config(&effective, &session);
        assert_eq!(lean["railway"], json!({"enabled": false}));
        assert_eq!(lean["docs"], json!({"enabled": false}));
        assert_eq!(
            lean["given"]["command"], "/bin/given",
            "what Brigadier gives stays"
        );
        session.switched_off.skill_files = vec![PathBuf::from("/h/.agents/skills/x/SKILL.md")];
        let args = app_server_args(Some(&session));
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "--disable" && pair[1] == "memories"),
            "{args:?}"
        );
        assert!(
            args.iter().any(|arg| arg
                == r#"skills.config=[{"enabled" = false, "path" = "/h/.agents/skills/x/SKILL.md"}]"#),
            "{args:?}"
        );
    }

    /// Workers and threads see their skills' descriptions; a skill the user switched off is
    /// disabled by its file, beside the user's own `skills.config` entries.
    #[test]
    fn switched_off_skills_keep_the_users_own_entries() {
        let mut session = spec(ToolSet::Lean, None);
        session.switched_off.skill_files = vec![PathBuf::from("/h/.agents/skills/x/SKILL.md")];
        session.switched_off.codex_skill_config =
            vec![json!({"path": "/h/own/SKILL.md", "enabled": false})];
        session.switched_off.plugin_servers = vec![("docs@market".into(), "server".into())];
        let args = app_server_args(Some(&session));
        let plugin = args
            .iter()
            .position(|arg| arg == "plugins.docs@market.mcp_servers.server.enabled=false")
            .expect("the plugin's server switched off");
        assert_eq!(args[plugin - 1], "-c");
        let at = args
            .iter()
            .position(|arg| arg.starts_with("skills.config="))
            .expect("skills.config");
        assert_eq!(args[at - 1], "-c");
        assert_eq!(
            args[at],
            r#"skills.config=[{"enabled" = false, "path" = "/h/own/SKILL.md"}, {"enabled" = false, "path" = "/h/.agents/skills/x/SKILL.md"}]"#
        );
        // A sealed session doesn't load the user's skills at all.
        let mut sealed = spec(ToolSet::Review, None);
        sealed.switched_off = session.switched_off.clone();
        assert!(
            !app_server_args(Some(&sealed))
                .iter()
                .any(|arg| arg.starts_with("skills.config=") || arg.starts_with("plugins."))
        );
    }

    /// `codex resume` in a terminal: no server's value is on the command line, a project's
    /// copied server with literal values included. Names given different values start through
    /// a shell that sets each from an alias (Unix); headers come from aliases.
    #[test]
    fn terminal_resume_keeps_every_servers_values_off_the_command_line() {
        let mut session = spec(ToolSet::Lean, None);
        session.mcp_servers = vec![
            server("one", stdio("/bin/one", None), &[("TOKEN", "first-secret")]),
            server(
                "two",
                stdio("/bin/two", None),
                &[("TOKEN", "second-secret")],
            ),
            server(
                "own",
                stdio("/bin/own", None),
                &[("BRIGADIER_GRANT", "grant-secret")],
            ),
            server(
                "web",
                McpTransport::Http {
                    url: "https://mcp.example.com/mcp".into(),
                },
                &[("Authorization", "Bearer third-secret")],
            ),
        ];
        let mut servers = mcp_config(&json!({}), &session);
        // A trusted project's server, copied with the literal values of its config.
        servers.insert(
            "project".into(),
            json!({"command": "/repo/bin/db", "args": [], "cwd": "/repo",
                   "env": {"DB_PASSWORD": "fourth-secret"},
                   "http_headers": {}}),
        );
        servers.insert(
            "remote".into(),
            json!({"url": "https://r.example.com", "http_headers": {"X-Key": "fifth-secret"},
                   "env_http_headers": {"X-Other": "OTHER_VAR"}}),
        );
        let mut config = Map::new();
        config.insert("mcp_servers".into(), Value::Object(servers));
        let env = CliEnv::from_vars([("PATH".into(), "/usr/bin".into())]);
        let command = terminal_command(
            "/usr/local/bin/codex".into(),
            &session,
            "thread-1",
            "/tmp".into(),
            config,
            &env,
        );
        let line = command.args.join(" ");
        for secret in [
            "first-secret",
            "second-secret",
            "grant-secret",
            "third-secret",
            "fourth-secret",
            "fifth-secret",
        ] {
            assert!(
                !line.contains(secret),
                "{secret} on the command line: {line}"
            );
        }
        let var = |name: &str| {
            command
                .env
                .iter()
                .find(|(key, _)| key.to_str() == Some(name))
                .and_then(|(_, value)| value.to_str().map(str::to_owned))
        };
        let override_of = |server: &str| -> String {
            let at = command
                .args
                .iter()
                .position(|arg| arg.starts_with("mcp_servers={"))
                .unwrap();
            let all = &command.args[at];
            let start = all.find(&format!("\"{server}\" = ")).unwrap();
            all[start..].to_owned()
        };
        // A name only one server has keeps its own name.
        assert_eq!(var("BRIGADIER_GRANT").as_deref(), Some("grant-secret"));
        assert!(override_of("own").contains(r#""env_vars" = ["BRIGADIER_GRANT"]"#));
        assert_eq!(var("DB_PASSWORD").as_deref(), Some("fourth-secret"));
        // The header's value comes from an alias.
        let web = override_of("web");
        assert!(
            web.contains(r#""env_http_headers" = {"Authorization" = "BRIGADIER_MCP_"#),
            "{web}"
        );
        let remote = override_of("remote");
        assert!(remote.contains(r#""X-Other" = "OTHER_VAR""#), "{remote}");
        assert!(remote.contains(r#""X-Key" = "BRIGADIER_MCP_"#), "{remote}");
        if cfg!(unix) {
            // TOKEN has two values: each server sets it from an alias of its own.
            let one = override_of("one");
            assert!(
                one.starts_with(r#""one" = {"args" = ["-c", "export TOKEN=\"$BRIGADIER_MCP_"#),
                "{one}"
            );
            assert!(one.contains(r#""/bin/one", "serve"]"#), "{one}");
            assert!(one.contains(r#""command" = "/bin/sh""#), "{one}");
            assert_eq!(var("TOKEN"), None);
            let aliases: Vec<String> = command
                .env
                .iter()
                .filter_map(|(key, value)| {
                    Some((key.to_str()?.to_owned(), value.to_str()?.to_owned()))
                })
                .filter(|(key, _)| key.starts_with("BRIGADIER_MCP_"))
                .map(|(_, value)| value)
                .collect();
            for secret in [
                "first-secret",
                "second-secret",
                "Bearer third-secret",
                "fifth-secret",
            ] {
                assert!(aliases.iter().any(|value| value == secret), "{aliases:?}");
            }
        } else {
            assert!(!line.contains(r#""one" = "#) && !line.contains(r#""two" = "#));
        }
    }
}
