//! Claude Code adapter.
//!
//! Brigadier runs the user's own, unmodified `claude` binary in print mode with stream-json on
//! both sides, one persistent process per session. It never uses the Agent SDK and never reads
//! or handles OAuth tokens: the CLI keeps its own login. Everything else goes over stdio:
//!
//! - turns and steering are user messages (Claude folds a message sent mid-turn into the
//!   running turn at its next step: one written while a tool runs waits for the tool's end and
//!   never cancels it, checked live on 2.1.302 with a running Bash; after an interrupt it
//!   starts a turn of its own, so Brigadier holds other sessions' and its own messages while a
//!   command runs, see `ConvState::held_peers`);
//! - `initialize`, `get_usage` and `interrupt` are control requests;
//! - permission prompts arrive as `can_use_tool` control requests (`--permission-prompt-tool
//!   stdio`) and Brigadier answers them.
//!
//! The permission mode follows the session's access ([`permission_mode`]): full access runs in
//! `bypassPermissions` with no sandbox and never asks; with [`SessionSpec::auto_review`]
//! (Approve for me) Claude's `auto` mode lets its classifier decide what leaves the sandbox,
//! and declines what it judges unsafe to the model instead of asking; otherwise
//! (`acceptEdits`) leaving the sandbox asks Brigadier. A read-only session (`dontAsk`) never
//! asks: what its allow rules don't cover is denied.
//!
//! Threads and workers ([`ToolSet::user_setup`]) run with the user's own Claude setup: their
//! settings, the project's and their local ones (`--setting-sources user,project,local`), so
//! their hooks, plugins, skills, allow rules, `CLAUDE.md` and MCP servers apply; skills come as
//! descriptions until used and MCP tools behind Claude's tool search. Claude reads a project's
//! setup from its working folder only, and these sessions work from a folder of Brigadier's (a
//! stable one, which a resume and the cached start of a session's workers need), so the
//! project's `.mcp.json` and `.claude` entries are linked there ([`files::link_project`]) for a
//! folder the user trusts ([`SessionSpec::project`]); the instruction files are left out, as
//! the session's prompt carries them. Brigadier's own `--settings` win over all of these
//! (permission prompt tool, sandbox, ask and deny rules, the model allowlist). A hook of the
//! user's that answers a permission request answers it before Brigadier is asked: their own
//! rule, as in their terminal; the sandbox and deny rules still hold. Every other session
//! (reviewers, Brain jobs, Chats, one-shot runs) loads only the project's settings plus
//! Brigadier's own (`--setting-sources project`), and only Brigadier's MCP servers
//! (`--strict-mcp-config`).
//!
//! A worker's sub-agents run only on the models its task may use ([`SessionSpec::allowed_models`]):
//! `availableModels` in its settings lists their exact ids, and when Claude's prefix matching
//! would let another model in too, or the project's or the user's own settings could widen
//! the list, the worker runs without its Agent tool ([`sub_agents`]).

pub(crate) mod files;
pub mod parse;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use brigadier_sandbox::Platform;
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
use parse::{Control, Output, Parser};

const EVENTS: usize = 512;
const STATUS_TIMEOUT: Duration = Duration::from_secs(20);
/// `initialize` loads the CLI, its settings and MCP servers.
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);
const EXIT_GRACE: Duration = Duration::from_secs(3);
/// The first version seen to run `/compact` sent as a stream-json message.
const COMPACT_SINCE: &str = "2.1.282";
/// The first version seen with `--prompt-suggestions` and `set_prompt_suggestions_paused`
/// (2.1.301: its help, the CLI reference and its SDK schema). An older CLI would refuse the flag.
const SUGGESTIONS_SINCE: &str = "2.1.301";
/// Built-in tools a worker's lean start leaves out ([`ToolSet::Lean`]): scheduling, background
/// agents and messaging, worktrees (Brigadier manages them), notebooks and the like. Each tool's
/// description is part of every request; without these a worker starts at about 10k tokens
/// instead of 15k (measured on 2.1.285). Subagents, web and tool search stay. A worker asks
/// through Brigadier's ask_orchestrator, never Claude's own question or plan-mode tools
/// (SESSION-FIXES-PLAN.md Q6.5; the three names checked in 2.1.296).
const LEAN_DENIED_TOOLS: &str = "Workflow,ScheduleWakeup,CronCreate,CronDelete,CronList,\
RemoteTrigger,PushNotification,DesignSync,ReportFindings,EnterWorktree,ExitWorktree,ListAgents,\
SendMessage,TaskStop,Monitor,NotebookEdit,AskUserQuestion,EnterPlanMode,ExitPlanMode";
/// Claude's built-in tool that starts a sub-agent (`Task` is its older name).
const SUB_AGENT_TOOLS: &str = "Agent,Task";
/// A thread's built-in tools ([`ToolSet::Thread`]): no sub-agents (`Agent`/`Task`) and nothing
/// that runs on in the background (`Monitor`, scheduling); its workers are Brigadier's. Tool
/// search stays (BUILD-PLAN C2): Brigadier marks its everyday tools to load from the start, and
/// the rest wait behind the search until the thread needs one.
const THREAD_TOOLS: &str = "Read,Grep,Glob,Bash,Edit,Write,WebSearch,WebFetch,ToolSearch";
/// A one-shot reviewer's built-in tools ([`ToolSet::Review`]).
const REVIEW_TOOLS: &str = "Read,Grep,Glob,Bash";
/// A Brain job's built-in tools ([`ToolSet::Reader`]).
const READER_TOOLS: &str = "Read,Grep,Glob";
/// What a one-shot reviewer may run without asking (`--allowedTools`): under `dontAsk`
/// everything else is denied, and nothing prompts.
const REVIEW_ALLOWED: &str = "Read Grep Glob Bash(git diff:*) Bash(git log:*) Bash(git show:*)";

pub struct Claude {
    platform: Arc<dyn Platform>,
    env: Arc<CliEnv>,
    /// An extra account's ([`crate::accounts`]): the user's own config directory, where its
    /// history and settings are (its own home links them).
    main_config: Option<PathBuf>,
}

impl Claude {
    pub fn new(platform: Arc<dyn Platform>, env: Arc<CliEnv>) -> Self {
        Self {
            platform,
            env,
            main_config: None,
        }
    }

    /// The adapter for an extra account whose config directory is `home` (made with
    /// [`crate::accounts::prepare_home`]); `env` is the user's own environment.
    pub fn for_account(platform: Arc<dyn Platform>, env: &CliEnv, home: &Path) -> Self {
        let main_config = crate::accounts::main_home(ProviderKind::Claude, env);
        let env = Arc::new(env.for_account(ProviderKind::Claude, home));
        Self {
            platform,
            env,
            main_config,
        }
    }

    /// The binary it runs, looked up each time: one installed or chosen after launch counts.
    fn binary(&self) -> Result<PathBuf> {
        self.env
            .resolve(ProviderKind::Claude)
            .ok_or(Error::NotInstalled(ProviderKind::Claude))
    }

    /// Claude's configuration directory, where it keeps transcripts and session state.
    fn config_dir(&self) -> Option<PathBuf> {
        match self.env.var("CLAUDE_CONFIG_DIR") {
            Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
            _ => self.env.home().map(|home| home.join(".claude")),
        }
    }

    /// Where the transcripts and the user's settings are: the configuration directory, or an
    /// extra account's main one (its `projects` links there).
    fn history_dir(&self) -> Option<PathBuf> {
        self.main_config.clone().or_else(|| self.config_dir())
    }

    /// An extra account's own config directory, recorded with its sessions.
    fn account_home(&self) -> Option<String> {
        self.main_config.as_ref()?;
        self.config_dir().map(|dir| dir.display().to_string())
    }

    /// The user's `settings.json` (the model and effort `claude` runs with when not told).
    async fn settings(&self) -> Value {
        let Some(path) = self.history_dir().map(|dir| dir.join("settings.json")) else {
            return Value::Null;
        };
        tokio::fs::read_to_string(path)
            .await
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    async fn version(&self) -> Option<String> {
        let spec = self.env.spec(&self.binary().ok()?).arg("--version");
        let output = process::run(&self.platform, &spec, STATUS_TIMEOUT)
            .await
            .ok()?;
        parse_version(&output.stdout)
    }

    /// Runs a throwaway process that answers control requests, then exits. It loads no
    /// settings, no MCP servers and writes no session.
    async fn control(&self, requests: Vec<Map<String, Value>>) -> Result<Vec<Value>> {
        let mut spec = self.env.spec(&self.binary()?);
        spec.args = Self::control_args().into_iter().map(Into::into).collect();
        spec.cwd = Some(self.platform.paths().data_dir.clone());
        let process::Spawned {
            process,
            mut stdout,
        } = process::spawn(self.platform.clone(), &spec, process::Options::default())?;

        let result = async {
            let mut answers = Vec::with_capacity(requests.len());
            for (index, request) in requests.into_iter().enumerate() {
                let id = format!("control-{index}");
                process
                    .write_line(&parse::control_request(&id, request))
                    .await?;
                let answer = tokio::time::timeout(CONTROL_TIMEOUT, async {
                    let mut parser = Parser::live();
                    while let Some(line) = stdout.recv().await {
                        for output in parser.feed(&line) {
                            if let Output::Control(Control::Response { request_id, result }) =
                                output
                                && request_id == id
                            {
                                return result.map_err(Error::Rejected);
                            }
                        }
                    }
                    Err(Error::Protocol(process_failure(&process)))
                })
                .await
                .map_err(|_| Error::Timeout("Claude to answer"))??;
                answers.push(answer);
            }
            Ok(answers)
        }
        .await;
        process.shutdown(EXIT_GRACE).await;
        result
    }

    /// Every print-mode process requests summaries, regardless of role or settings.
    fn stream_args() -> Vec<String> {
        [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            // The boolean showThinkingSummaries setting is not read in print mode.
            "--thinking-display",
            "summarized",
        ]
        .map(str::to_owned)
        .to_vec()
    }

    fn control_args() -> Vec<String> {
        let mut args = Self::stream_args();
        args.extend(
            [
                "--verbose",
                "--no-session-persistence",
                "--strict-mcp-config",
                "--mcp-config",
                r#"{"mcpServers":{}}"#,
                "--setting-sources",
                "",
                "--settings",
                r#"{"autoMemoryEnabled":false}"#,
            ]
            .map(str::to_owned),
        );
        args
    }

    /// `home_login`: an extra account's session, which must sign in with its home's login
    /// only ([`settings`]). `user_config`: Claude's configuration directory, whose settings a
    /// session with the user's setup loads.
    fn session_args(
        spec: &SessionSpec,
        cwd: &Path,
        native_id: &str,
        home_login: bool,
        user_config: Option<&Path>,
    ) -> Result<Vec<String>> {
        let mut args = Self::stream_args();
        args.extend(
            [
                "--include-partial-messages",
                "--verbose",
                "--replay-user-messages",
                "--permission-prompt-tool",
                "stdio",
            ]
            .map(str::to_owned),
        );
        args.extend(Self::session_flags(spec, cwd, home_login, user_config));
        if let Some(prompt) = &spec.append_system_prompt {
            args.push("--append-system-prompt".into());
            args.push(prompt.clone());
        }
        match &spec.origin {
            Origin::New => {
                args.push("--session-id".into());
                args.push(native_id.into());
            }
            Origin::Resume { native_id: resumed } => {
                files::check_session_id(resumed)?;
                args.push("--resume".into());
                args.push(resumed.clone());
            }
            Origin::Fork { native_id: parent } => {
                files::check_session_id(parent)?;
                args.push("--resume".into());
                args.push(parent.clone());
                args.push("--fork-session".into());
                args.push("--session-id".into());
                args.push(native_id.into());
            }
        }
        Ok(args)
    }

    /// The session's flags that hold in print mode and in an interactive terminal alike (none
    /// of them is stored in the transcript, so a resume repeats them all).
    fn session_flags(
        spec: &SessionSpec,
        cwd: &Path,
        home_login: bool,
        user_config: Option<&Path>,
    ) -> Vec<String> {
        // Its extra folders are writable wherever its working directory is.
        let spec = &SessionSpec {
            access: spec.access_with_dirs(),
            ..spec.clone()
        };
        // A thread or a worker loads the user's settings, the project's and their local ones,
        // with their MCP servers beside Brigadier's; `--settings` wins over all of them. Every
        // other session loads only the project's, and only Brigadier's servers.
        let user_setup = spec.tools.user_setup();
        let mut args: Vec<String> = if user_setup {
            vec!["--setting-sources".into(), "user,project,local".into()]
        } else {
            ["--strict-mcp-config", "--setting-sources", "project"]
                .map(str::to_owned)
                .to_vec()
        };
        args.push("--mcp-config".into());
        args.push(mcp_config(&spec.mcp_servers).to_string());
        // Decided once, for both the tools and the settings.
        let sub_agents = sub_agents(spec, cwd, user_config.filter(|_| user_setup));
        if let Some(denied) = denied_tools(spec, &sub_agents) {
            args.push("--disallowedTools".into());
            args.push(denied);
        }
        match spec.tools {
            ToolSet::Default | ToolSet::Lean => {}
            ToolSet::None => {
                args.push("--tools".into());
                args.push(String::new());
            }
            ToolSet::Web => {
                args.push("--tools".into());
                args.push("WebSearch,WebFetch".into());
            }
            ToolSet::Thread => {
                args.push("--tools".into());
                args.push(THREAD_TOOLS.into());
            }
            ToolSet::Review => {
                args.push("--tools".into());
                args.push(REVIEW_TOOLS.into());
                args.push("--allowedTools".into());
                args.push(REVIEW_ALLOWED.into());
            }
            ToolSet::Reader => {
                args.push("--tools".into());
                args.push(READER_TOOLS.into());
                // The working folder, git status and the like go in the first message, so
                // the system prompt is the same for every job and their start is cached once.
                args.push("--exclude-dynamic-system-prompt-sections".into());
            }
        }
        args.push("--settings".into());
        args.push(settings(spec, cwd, &sub_agents, home_login).to_string());
        args.push("--permission-mode".into());
        args.push(permission_mode(spec).into());
        // The session's own folders and the roots it may write. Both spellings of a root behind
        // a symlink (`/tmp` → `/private/tmp`): Claude matches the path as a tool was given it.
        // `--add-dir <directories...>` is variadic (claude --help, 2.1.292): only flags follow
        // it here, and turns go on stdin, so no positional argument is taken for a folder.
        // A writable root that isn't a folder (a branch's ref, its `.lock`, `packed-refs.lock`)
        // is granted by the sandbox's `allowWrite` alone: given as `--add-dir`, a `.lock` not
        // there yet can't be made in the sandbox at all (2.1.296, checked by hand), so no
        // commit takes its branch's lock.
        let mut dirs: Vec<&PathBuf> = spec.add_dirs.iter().collect();
        for root in spec.access.writable_roots() {
            if root.is_dir() && !dirs.contains(&root) {
                dirs.push(root);
            }
        }
        for root in dirs {
            let real = resolved(root);
            args.push("--add-dir".into());
            args.push(root.display().to_string());
            if real != *root {
                args.push("--add-dir".into());
                args.push(real.display().to_string());
            }
        }
        // Brigadier's own skills, as plugins: they load beside the project's settings without
        // reading the user's.
        for plugin in &spec.skill_plugins {
            args.push("--plugin-dir".into());
            args.push(plugin.display().to_string());
        }
        if let Some(model) = &spec.model {
            args.push("--model".into());
            args.push(model.clone());
        }
        if let Some(effort) = &spec.effort {
            args.push("--effort".into());
            args.push(effort.clone());
        }
        args
    }
}

/// The session's own variables on top of the CLI environment: its env, what its MCP config
/// refers to by name, what the output hook reads, its MCP time limits and auto-compaction.
fn session_env(spec: &SessionSpec) -> Vec<(String, String)> {
    let mut env = spec.env.clone();
    // What the MCP config refers to by name, and what the output hook reads.
    for server in &spec.mcp_servers {
        env.extend(server.env.iter().cloned());
    }
    if let Some(hook) = spec
        .output_hook
        .as_ref()
        .filter(|_| spec.tools == ToolSet::Thread)
    {
        env.extend(hook.env.iter().cloned());
    }
    if let Some(hook) = &spec.stop_hook {
        env.extend(hook.env.iter().cloned());
    }
    if let Some(secs) = spec
        .mcp_servers
        .iter()
        .filter_map(|server| server.tool_timeout_secs)
        .max()
    {
        // The overall limit, and the limit on a stdio call that sends nothing back while it
        // waits (30 minutes by default), which a blocking question can exceed.
        let ms = (secs * 1000).to_string();
        env.push(("MCP_TOOL_TIMEOUT".into(), ms.clone()));
        env.push(("CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT".into(), ms));
    }
    if !spec.auto_compact {
        env.push(("DISABLE_AUTO_COMPACT".into(), "1".into()));
    }
    env
}

/// Claude's `--mcp-config` for the session's servers.
///
/// The config is on Claude's command line, which any process of the user can read, so a
/// server's environment values (such as a grant) are not in it: they are in Claude's own
/// environment, and the config names them (`${NAME}`, expanded by Claude).
fn mcp_config(servers: &[McpServer]) -> Value {
    let servers: Map<String, Value> = servers
        .iter()
        .map(|server| {
            let env: Map<String, Value> = server
                .env
                .iter()
                .map(|(name, _)| (name.clone(), Value::String(format!("${{{name}}}"))))
                .collect();
            (server.name.clone(), {
                let mut config = json!({
                    "type": "stdio",
                    "command": server.command.display().to_string(),
                    "args": server.args,
                    "env": env,
                });
                if let Some(secs) = server.tool_timeout_secs {
                    config["timeout"] = json!(secs * 1000);
                }
                if server.always_load {
                    config["alwaysLoad"] = json!(true);
                }
                config
            })
        })
        .collect();
    json!({ "mcpServers": servers })
}

/// The models a session's sub-agents may run on.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SubAgents {
    /// Any model: no limit was asked for.
    Any,
    /// Only these (`availableModels`).
    Only(Vec<String>),
    /// None: Claude can't hold them to the allowed set, so they are not started at all.
    Off,
}

/// How `spec`'s sub-agents are held to [`SessionSpec::allowed_models`], for a session in
/// `cwd` that loads the user's settings from `user_config` too, when given.
fn sub_agents(spec: &SessionSpec, cwd: &Path, user_config: Option<&Path>) -> SubAgents {
    let Some(allowed) = &spec.allowed_models else {
        return SubAgents::Any;
    };
    if project_widens_models(cwd)
        || user_config.is_some_and(|dir| file_widens_models(&dir.join("settings.json")))
    {
        return SubAgents::Off;
    }
    available_models(allowed).map_or(SubAgents::Off, SubAgents::Only)
}

/// The built-in tools a session runs without (`--disallowedTools`): a worker's lean start
/// leaves some out, and sub-agents that could run on a model the task may not use are not
/// started at all.
fn denied_tools(spec: &SessionSpec, sub_agents: &SubAgents) -> Option<String> {
    let mut denied: Vec<&str> = Vec::new();
    if spec.tools == ToolSet::Lean {
        denied.push(LEAN_DENIED_TOOLS);
    }
    if *sub_agents == SubAgents::Off {
        denied.push(SUB_AGENT_TOOLS);
    }
    (!denied.is_empty()).then(|| denied.join(","))
}

/// Whether the project's own Claude settings could widen the models a session in `cwd` may
/// use: Claude joins the `availableModels` lists of all the settings it loads, and a session
/// loads the project's (`--setting-sources project`). So a `.claude/settings.json` or
/// `.claude/settings.local.json` (in `cwd`, up to its repository's root, and in the main
/// checkout of a worktree) that lists models, maps one to another (`modelOverrides`) or names
/// one in `env` (`ANTHROPIC_DEFAULT_OPUS_MODEL`, `CLAUDE_CODE_SUBAGENT_MODEL`) could let a
/// sub-agent run on a model the task may not use. A file that can't be read counts too.
fn project_widens_models(cwd: &Path) -> bool {
    project_dirs(cwd).iter().any(|dir| {
        ["settings.json", "settings.local.json"]
            .iter()
            .any(|name| file_widens_models(&dir.join(".claude").join(name)))
    })
}

/// Whether a settings file names models ([`widens_models`]); one that can't be read counts.
fn file_widens_models(path: &Path) -> bool {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            serde_json::from_str::<Value>(&text).map_or(true, |settings| widens_models(&settings))
        }
        Err(err) => err.kind() != std::io::ErrorKind::NotFound,
    }
}

/// Whether a settings file's content names models.
fn widens_models(settings: &Value) -> bool {
    settings.get("availableModels").is_some()
        || settings.get("modelOverrides").is_some()
        || settings
            .get("env")
            .and_then(Value::as_object)
            .is_some_and(|env| {
                env.keys()
                    .any(|key| key.to_ascii_uppercase().contains("MODEL"))
            })
}

/// The folders whose `.claude` settings a session in `cwd` may load: `cwd` and the folders
/// above it up to its repository's root, and the main checkout of a linked worktree. Only
/// `cwd` outside a repository.
fn project_dirs(cwd: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for dir in cwd.ancestors() {
        dirs.push(dir.to_owned());
        let git = dir.join(".git");
        if git.is_dir() {
            return dirs;
        }
        if git.is_file() {
            dirs.extend(main_checkout(&git));
            return dirs;
        }
    }
    vec![cwd.to_owned()]
}

/// The main checkout of a linked worktree, from its `.git` file (`gitdir: <main>/.git/
/// worktrees/<name>`, whose `commondir` names the main `.git`).
fn main_checkout(git_file: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(git_file).ok()?;
    let gitdir = git_file
        .parent()?
        .join(text.trim().strip_prefix("gitdir:")?.trim());
    let common = std::fs::read_to_string(gitdir.join("commondir")).ok()?;
    let common = gitdir.join(common.trim()).canonicalize().ok()?;
    if common.file_name()? != ".git" {
        return None;
    }
    common.parent().map(Path::to_owned)
}

/// Claude's permission mode for a session (see the module docs).
fn permission_mode(spec: &SessionSpec) -> &'static str {
    match spec.access {
        Access::Full => "bypassPermissions",
        // A read-only session never asks: what its allow rules don't cover is denied.
        Access::ReadOnly => "dontAsk",
        Access::Workspace { .. } | Access::Scoped { .. } if spec.auto_review => "auto",
        Access::Scoped {
            write_cwd: false, ..
        } => "default",
        Access::Workspace { .. } | Access::Scoped { .. } => "acceptEdits",
    }
}

/// A permission rule path for an absolute path (`//abs/path/**`).
fn rule_path(path: &Path) -> String {
    format!("/{}/**", path.display())
}

/// Brigadier's settings layer for a session, passed with `--settings` (above project settings).
fn settings(spec: &SessionSpec, cwd: &Path, sub_agents: &SubAgents, home_login: bool) -> Value {
    let mut ask: Vec<String> = Vec::new();
    // Unless Claude's own reviewer decides (auto mode), leaving the sandbox always goes through
    // the permission prompt, even if a project rule would allow the command.
    if spec.access != Access::Full && !spec.auto_review {
        ask.push("Bash(dangerouslyDisableSandbox:true)".into());
    }
    let mut allow: Vec<String> = spec
        .mcp_servers
        .iter()
        .filter(|server| server.trusted)
        .map(|server| format!("mcp__{}", server.name))
        .collect();
    // A trusted server's tools that still ask (a thread's command leaving its sandbox): an ask
    // rule wins over the server's allow rule.
    for server in spec.mcp_servers.iter().filter(|server| server.trusted) {
        for tool in &server.prompt_tools {
            ask.push(format!("mcp__{}__{tool}", server.name));
        }
    }
    let mut deny: Vec<String> = Vec::new();
    let sandbox = match &spec.access {
        Access::Workspace { extra_roots } => json!({
            "enabled": true,
            "failIfUnavailable": true,
            "autoAllowBashIfSandboxed": true,
            "allowUnsandboxedCommands": true,
            "network": { "allowedDomains": ["*"], "allowLocalBinding": true },
            "filesystem": {
                "allowWrite": paths(extra_roots),
            },
        }),
        Access::Scoped {
            write_cwd,
            writable_roots,
            network,
            deny_read,
            unix_sockets,
        } => {
            // Its added folders are writable exactly when its working directory is.
            let read_only: Vec<&Path> = if *write_cwd {
                Vec::new()
            } else {
                std::iter::once(cwd)
                    .chain(spec.add_dirs.iter().map(PathBuf::as_path))
                    .collect()
            };
            for dir in &read_only {
                for tool in ["Edit", "Write", "NotebookEdit"] {
                    deny.push(format!("{tool}({})", rule_path(dir)));
                }
            }
            for path in deny_read {
                // Permission rules match the path as the tool was given it: deny both spellings.
                deny.push(format!("Read({})", rule_path(path)));
                let real = resolved(path);
                if real != *path {
                    deny.push(format!("Read({})", rule_path(&real)));
                }
            }
            let mut filesystem = json!({
                "allowWrite": paths(writable_roots),
                "denyRead": paths(deny_read),
            });
            let mut deny_write: Vec<PathBuf> =
                read_only.iter().map(|dir| dir.to_path_buf()).collect();
            // A writer may commit into a repository's git folder, but not plant hooks or
            // change its config: those run outside the sandbox the next time the user runs git.
            // Claude's file tools work outside the OS sandbox: deny them there too.
            // A checkout among the writable folders (a thread on the user's own checkout) holds
            // its git folder inside.
            let writable_dirs = writable_roots.iter().map(PathBuf::as_path).chain(
                write_cwd
                    .then(|| std::iter::once(cwd).chain(spec.add_dirs.iter().map(PathBuf::as_path)))
                    .into_iter()
                    .flatten(),
            );
            let git_dirs: Vec<PathBuf> = writable_dirs
                .flat_map(|root| [root.to_owned(), root.join(".git")])
                .filter(|root| is_git_dir(root))
                .collect();
            for root in &git_dirs {
                let root = resolved(root);
                for tool in ["Edit", "Write", "NotebookEdit"] {
                    deny.push(format!("{tool}({})", rule_path(&root.join("hooks"))));
                    deny.push(format!("{tool}(/{})", root.join("config").display()));
                }
                deny_write.extend([root.join("hooks"), root.join("config")]);
            }
            // Other sessions' branches and the branches its work lands on (PLAN bug 6): its
            // sandbox grants none of them already, unless a checkout is writable whole (a
            // thread on the user's own checkout). A deny wins over every allow in Claude's
            // sandbox, so one that would cover the session's own ref is left to the grant.
            for path in &spec.protection.deny {
                if spec
                    .protection
                    .allow
                    .iter()
                    .any(|own| own.starts_with(path))
                {
                    continue;
                }
                let path = resolved(path);
                // A rule names a file exactly and a folder's contents with `/**`.
                for tool in ["Edit", "Write", "NotebookEdit"] {
                    deny.push(format!("{tool}(/{})", path.display()));
                    deny.push(format!("{tool}({})", rule_path(&path)));
                }
                deny_write.push(path);
            }
            if !deny_write.is_empty() {
                filesystem["denyWrite"] = json!(paths(&deny_write));
            }
            json!({
                "enabled": true,
                "failIfUnavailable": true,
                "autoAllowBashIfSandboxed": true,
                "allowUnsandboxedCommands": true,
                // A check's own dev server (a test's Vite) listens on localhost; Codex's
                // sandbox allows that already.
                "network": {
                    "allowedDomains": if *network { json!(["*"]) } else { json!([]) },
                    "allowUnixSockets": paths(unix_sockets),
                    "allowLocalBinding": true,
                },
                "filesystem": filesystem,
            })
        }
        Access::ReadOnly => json!({
            "enabled": true,
            "failIfUnavailable": true,
            "autoAllowBashIfSandboxed": false,
            "allowUnsandboxedCommands": false,
        }),
        // Like the user's own terminal: `bypassPermissions` asks for nothing.
        Access::Full => json!({ "enabled": false }),
    };
    if matches!(spec.tools, ToolSet::Web | ToolSet::Thread) {
        allow.extend(["WebSearch".to_owned(), "WebFetch".to_owned()]);
    }
    let mut permissions = json!({});
    if !ask.is_empty() {
        permissions["ask"] = json!(ask);
    }
    // Only full access may run without permission checks.
    if spec.access != Access::Full {
        permissions["disableBypassPermissionsMode"] = json!("disable");
    }
    if !allow.is_empty() {
        permissions["allow"] = json!(allow);
    }
    if !deny.is_empty() {
        permissions["deny"] = json!(deny);
    }
    let mut settings = json!({
        // The Project Brain is Brigadier's memory; workers do not write Claude's.
        "autoMemoryEnabled": false,
        // At a usage limit the session reports it and stops, so Brigadier can hand the work to
        // another model, instead of waiting for the reset on its own.
        "autoContinueAtUsageLimit": false,
        // Claude compacts a long conversation on its own while it sits idle, before its prompt
        // cache expires (a server-side rollout, 2.1.296); a session's context changes only
        // where Brigadier or the user asks for it.
        "idleCompaction": false,
        // Claude's own messages between sessions (`SendMessage` from the user's other Claude
        // sessions) never reach a Brigadier session: its messages come through Brigadier,
        // which records and delivers them at a tool boundary (2.1.302 takes accept, hold,
        // refuse).
        "crossSessionInbound": "refuse",
        "permissions": permissions,
        "sandbox": sandbox,
        // A repository's `AGENTS.md` files load next to its `CLAUDE.md` files (by default
        // Claude reads them only where there is no `CLAUDE.md`), nested ones included once
        // Claude reads a file in their directory.
        "pluginConfigs": {
            "agents-md@builtin": {
                "options": { "instructionFiles": "claude-md-and-agents-md" },
            },
        },
    });
    // A worker, a thread or a Brain job needs none of what ships with Claude Code for a person at the
    // keyboard, and each of it is in every request or connects at start: bundled skills and
    // workflows, claude.ai's MCP connectors, the Artifact tool, Remote Control
    // (SESSION-FIXES-PLAN.md Q6.5). Each key is in 2.1.296's settings schema; `enableArtifact:
    // false` replaces the deprecated `disableArtifact`, and set in `--settings` it wins.
    if matches!(
        spec.tools,
        ToolSet::Lean | ToolSet::Thread | ToolSet::Reader
    ) {
        settings["disableBundledSkills"] = json!(true);
        settings["disableClaudeAiConnectors"] = json!(true);
        settings["enableArtifact"] = json!(false);
        settings["disableRemoteControl"] = json!(true);
    }
    // The models the session and its sub-agents may run on (a sub-agent asking for another
    // steps down to an allowed one).
    if let SubAgents::Only(ids) = sub_agents {
        settings["availableModels"] = json!(ids);
    }
    // No Co-Authored-By trailer or generated-with line in its commits and pull requests.
    if spec.omit_ai_coauthors {
        settings["includeCoAuthoredBy"] = json!(false);
        settings["attribution"] = json!({ "commit": "", "pr": "" });
    }
    if let Some(hooks) = output_hooks(spec) {
        settings["hooks"] = hooks;
    }
    if let Some(stop) = stop_hooks(spec) {
        settings["hooks"]["Stop"] = stop;
    }
    if let Some(reason) = &spec.deny_tools {
        settings["hooks"]["PreToolUse"] = deny_hook(reason);
    }
    // A session that must not compact refuses Claude's own compaction, which the parser turns
    // into a hand-off or a rebirth (BUILD-PLAN D2.3, D2.3b).
    if !spec.auto_compact {
        settings["hooks"]["PreCompact"] = refuse_compaction_hook();
    }
    // The user's `disableAllHooks` would switch Brigadier's own hooks off with theirs.
    if spec.tools.user_setup() && settings.get("hooks").is_some() {
        settings["disableAllHooks"] = json!(false);
    }
    // In auto mode Claude's classifier settles a command that leaves the sandbox: it refuses
    // what the protection keeps out, whatever the user said. `hard_deny` rules block whatever
    // the user intends, and `$defaults` keeps the built-in ones; the classifier reads `autoMode`
    // from `--settings` (docs/en/auto-mode-config, 2.1.296).
    if permission_mode(spec) == "auto"
        && let Some(rule) = crate::approver_rule(&spec.protection)
    {
        settings["autoMode"] = json!({ "hard_deny": ["$defaults", rule] });
    }
    // Full access is the user's own choice in Brigadier, so its terminal ("Open in terminal")
    // doesn't ask again with Claude's bypass-mode warning, whose default answer exits. Flag
    // settings count as consent (2.1.294 reads the key from user, local, flag and policy
    // settings; docs/evidence/2026-10-08-trust-dialog.md).
    if spec.access == Access::Full {
        settings["skipDangerousModePermissionPrompt"] = json!(true);
    }
    // An extra account signs in with its home's login only: a key or token a project's
    // settings set would win over it, and flag settings win over project settings. Claude
    // takes a blank one as none (2.1.295 then says "Not logged in";
    // docs/evidence/2026-10-09-accounts.md). The process gets none either.
    if home_login {
        let blank: Map<String, Value> = crate::accounts::auth_vars(ProviderKind::Claude)
            .iter()
            .map(|name| ((*name).to_owned(), json!("")))
            .collect();
        settings["env"] = Value::Object(blank);
    }
    settings
}

/// A thread's output hook ([`SessionSpec::output_hook`]) as settings `hooks`: the same command
/// after every `Bash` call, successful (`PostToolUse`) or not (`PostToolUseFailure`). Settings
/// passed with `--settings` hold under `--setting-sources project`, and a successful call's
/// output is replaced by the hook's `hookSpecificOutput.updatedToolOutput` (code.claude.com/
/// docs/en/hooks, "PostToolUse decision control"; checked on 2.1.292,
/// docs/evidence/2026-10-07-thread-phase2-contracts.md §2). A successful file tool's call runs
/// it too, and its `hookSpecificOutput.additionalContext` (the touched folder's rules) follows
/// the call's result (BUILD-PLAN D1.6). Before a `Bash` call runs (`PreToolUse`) it runs as
/// well, and the CLI waits for it: the daemon reads the files the command names, so Undo can
/// tell what the call changed (#87).
fn output_hooks(spec: &SessionSpec) -> Option<Value> {
    let hook = spec
        .output_hook
        .as_ref()
        .filter(|_| spec.tools == ToolSet::Thread)?;
    let command = std::iter::once(hook.command.display().to_string())
        .chain(hook.args.iter().cloned())
        .map(|part| shell_quote(&part))
        .collect::<Vec<_>>()
        .join(" ");
    let run = json!({ "type": "command", "command": command, "timeout": hook.timeout_secs });
    let entry = json!([{ "matcher": "Bash", "hooks": [run.clone()] }]);
    // A file tool's call brings its folder's rules (BUILD-PLAN D1.6).
    let files = json!({ "matcher": FILE_TOOLS_MATCHER, "hooks": [run] });
    let mut success = entry.clone();
    success.as_array_mut()?.push(files);
    Some(json!({
        "PreToolUse": entry.clone(),
        "PostToolUse": success,
        "PostToolUseFailure": entry,
    }))
}

/// The file tools whose successful calls also run a thread's output hook.
const FILE_TOOLS_MATCHER: &str = "Read|Edit|Write|MultiEdit|NotebookEdit";

/// A worker's `Stop` hook ([`SessionSpec::stop_hook`]) as settings `hooks.Stop`: its command
/// reads the stop's event (`stop_hook_active` among it) and answers `decision: "block"` with a
/// reason when the turn may not end yet, which the CLI gives the model to continue with
/// (code.claude.com/docs/en/hooks, "Stop decision control"; the CLI stops continuing after 8).
fn stop_hooks(spec: &SessionSpec) -> Option<Value> {
    let hook = spec.stop_hook.as_ref()?;
    let command = std::iter::once(hook.command.display().to_string())
        .chain(hook.args.iter().cloned())
        .map(|part| shell_quote(&part))
        .collect::<Vec<_>>()
        .join(" ");
    Some(json!([{
        "hooks": [{ "type": "command", "command": command, "timeout": hook.timeout_secs }],
    }]))
}

/// A `PreToolUse` hook that refuses every tool call, native and MCP, before it runs, telling
/// the model `reason` ([`SessionSpec::deny_tools`]). The command reads the call's event and
/// answers `permissionDecision: "deny"` (code.claude.com/docs/en/hooks, "PreToolUse decision
/// control").
fn deny_hook(reason: &str) -> Value {
    let answer = json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        },
    });
    let command = format!(
        "cat >/dev/null; printf '%s\\n' {}",
        shell_quote(&answer.to_string())
    );
    json!([{
        "matcher": "*",
        "hooks": [{ "type": "command", "command": command }],
    }])
}

/// A `PreCompact` hook that refuses every compaction: exit 2 skips one Claude starts on its
/// own before the context limit, and fails a request whose compaction would recover from the
/// limit (code.claude.com/docs/en/hooks; P5 in docs/research/long-sessions/PROBES.md: a
/// refused automatic compaction leaves only a `compacting` status and an empty one).
fn refuse_compaction_hook() -> Value {
    let command = format!(
        "cat >/dev/null; printf '%s\\n' {} >&2; exit 2",
        shell_quote(REFUSED_COMPACTION)
    );
    json!([{ "hooks": [{ "type": "command", "command": command }] }])
}

/// Why the hook refuses, as Claude reports it.
const REFUSED_COMPACTION: &str =
    "Brigadier continues this session in a fresh one instead of compacting it.";

/// `text` as one word for the shell Claude runs a hook command with.
fn shell_quote(text: &str) -> String {
    if !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/-_.,:=@%+".contains(c))
    {
        return text.to_owned();
    }
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The `availableModels` list that holds a session to `allowed`, or `None` when Claude can't
/// hold it exactly. Claude compares ids without their context suffix, and matches an entry as
/// a prefix of a model id up to a `-` (`claude-opus-5` also allows `claude-opus-5-5`, and
/// `claude-opus-5-5` allows `claude-opus-5-5-fast` and `claude-opus-5-5[1m]`;
/// `availableModelsMatch`, which turns that off, is honored only from managed settings). So
/// when a model the session may not use has an allowed id, or extends one, the list would let
/// it in; and an empty list allows the default model. The session then runs without
/// sub-agents.
fn available_models(allowed: &AllowedModels) -> Option<Vec<String>> {
    let ids: Vec<String> = allowed.ids.iter().map(|id| model_key(id)).collect();
    if ids.is_empty() {
        return None;
    }
    let admitted = allowed
        .outside
        .iter()
        .map(|id| model_key(id))
        .any(|outside| {
            ids.iter().any(|id| {
                outside
                    .strip_prefix(id.as_str())
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
            })
        });
    (!admitted).then_some(ids)
}

/// A model id as `availableModels` compares it: lower case, without its context suffix.
fn model_key(id: &str) -> String {
    bare(id.trim()).to_ascii_lowercase()
}

/// Sandbox paths as Seatbelt matches them: resolved through symlinks (`/tmp` and
/// `/var/folders` are `/private/…` on macOS). A unix-socket rule on the unresolved path never
/// matches, so the connection is refused.
fn paths(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| resolved(path).display().to_string())
        .collect()
}

/// Whether a session runs with `--prompt-suggestions`: one that asks for them
/// ([`SessionSpec::suggestions`]), on a CLI that has the flag.
fn suggests(spec: &SessionSpec, cli_version: Option<&str>) -> bool {
    spec.suggestions
        && cli_version.is_some_and(|version| version_at_least(version, SUGGESTIONS_SINCE))
}

/// Whether `path` is a repository's git folder (its `HEAD` and object store).
fn is_git_dir(path: &Path) -> bool {
    path.join("HEAD").is_file() && path.join("objects").is_dir()
}

fn resolved(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}

impl Provider for Claude {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Claude
    }

    fn status(&self) -> BoxFuture<'_, ProviderStatus> {
        Box::pin(async move {
            let found = self.binary();
            let mut status = ProviderStatus {
                provider: ProviderKind::Claude,
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
                status.guidance = Some(
                    "Install Claude Code (https://code.claude.com) so `claude` is on your login \
                     shell's PATH, then refresh."
                        .into(),
                );
                return status;
            };
            status.version = self.version().await;
            status.compacts = status
                .version
                .as_deref()
                .is_some_and(|version| version_at_least(version, COMPACT_SINCE));
            let spec = self
                .env
                .spec(&binary)
                .arg("auth")
                .arg("status")
                .arg("--json");
            match process::run(&self.platform, &spec, STATUS_TIMEOUT).await {
                Ok(output) => match serde_json::from_str::<Value>(output.stdout.trim()) {
                    Ok(auth) => {
                        status.logged_in =
                            auth.get("loggedIn").and_then(Value::as_bool) == Some(true);
                        status.auth_method = auth
                            .get("authMethod")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                        status.plan = auth
                            .get("subscriptionType")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                        let field = |name: &str| {
                            auth.get(name)
                                .and_then(Value::as_str)
                                .filter(|value| !value.is_empty())
                                .map(str::to_owned)
                        };
                        status.email = field("email");
                        status.organization = field("orgName");
                    }
                    Err(_) => {
                        status.guidance = Some(format!(
                            "`claude auth status --json` gave no status: {}",
                            output.stderr.trim()
                        ));
                    }
                },
                Err(err) => status.guidance = Some(format!("Could not ask Claude Code: {err}")),
            }
            if !status.logged_in && status.guidance.is_none() {
                status.guidance = Some(
                    "Log in to Claude: run `claude auth login` in a terminal (or start `claude` \
                     and use /login), then refresh."
                        .into(),
                );
            }
            status
        })
    }

    fn models(&self) -> BoxFuture<'_, Result<ModelCatalog>> {
        Box::pin(async move {
            let version = self.version().await;
            let mut initialize = Map::new();
            initialize.insert("subtype".into(), "initialize".into());
            let answers = self.control(vec![initialize]).await?;
            let models = answers
                .first()
                .and_then(|answer| answer.get("models"))
                .and_then(Value::as_array)
                .ok_or_else(|| Error::Protocol("initialize returned no models".into()))?;
            Ok(ModelCatalog {
                provider: ProviderKind::Claude,
                models: model_infos(models, &self.settings().await),
                cli_version: version,
                fetched_at_ms: now_ms(),
            })
        })
    }

    fn quota(&self) -> BoxFuture<'_, Result<QuotaSnapshot>> {
        Box::pin(async move {
            let mut usage = Map::new();
            usage.insert("subtype".into(), "get_usage".into());
            usage.insert("skip_behaviors".into(), true.into());
            let answers = self.control(vec![usage]).await?;
            let answer = answers.first().cloned().unwrap_or(Value::Null);
            Ok(parse::usage_snapshot(&answer))
        })
    }

    fn start(&self, spec: SessionSpec, ledger: Arc<dyn Ledger>) -> BoxFuture<'_, Result<Started>> {
        Box::pin(async move {
            let binary = self.binary()?;
            let cwd = spec.cwd.canonicalize().map_err(|err| {
                Error::Invalid(format!("working directory {}: {err}", spec.cwd.display()))
            })?;
            let native_id = match &spec.origin {
                Origin::Resume { native_id } => native_id.clone(),
                Origin::New | Origin::Fork { .. } => uuid::Uuid::new_v4().to_string(),
            };
            // A session whose saved transcript is gone can't be resumed: said here, rather than
            // by a CLI that starts and dies, so the caller goes on from its own record.
            if let Origin::Resume { native_id } | Origin::Fork { native_id } = &spec.origin
                && let Some(config) = self.history_dir()
                && files::session_missing(&config, native_id)
            {
                return Err(Error::Spawn(format!(
                    "no saved Claude session {native_id} to resume"
                )));
            }
            let user_config = self.config_dir();
            let mut args = Self::session_args(
                &spec,
                &cwd,
                &native_id,
                self.main_config.is_some(),
                user_config.as_deref(),
            )?;
            let cli_version = self.version().await;
            // The CLI's own next-message guess: after each turn, one small request on the
            // conversation's warm cache, emitted as a `prompt_suggestion` message.
            let suggests = suggests(&spec, cli_version.as_deref());
            if suggests {
                args.push("--prompt-suggestions".into());
            }

            // The project's own setup, where Claude looks for it: before its staging folders
            // are recorded, so the `.claude` folder holding the links is never taken for one.
            if spec.tools.user_setup()
                && spec.owned_cwd
                && let Some(project) = &spec.project
                && project.canonicalize().ok().as_ref() != Some(&cwd)
                && let Err(err) = files::link_project(&cwd, project)
            {
                tracing::warn!(error = %err, "could not link the project's Claude setup");
            }
            // Recorded before the CLI can create them.
            if let Some(config) = self.history_dir() {
                let project = files::project_dir(&config, &cwd);
                let artifact = Artifact::ClaudeProjectDir {
                    path: project.display().to_string(),
                };
                if !project.exists() || ledger.holds(&artifact) {
                    ledger.record(artifact).await?;
                }
            }
            ledger
                .record(Artifact::ClaudeSession {
                    session_id: native_id.clone(),
                    home: self.account_home(),
                    cwd: Some(cwd.display().to_string()),
                })
                .await?;
            let mut staging = None;
            for path in files::staging_dirs(&cwd) {
                let artifact = Artifact::ClaudeStagingDir {
                    path: path.display().to_string(),
                };
                if !path.exists() || ledger.holds(&artifact) {
                    ledger.record(artifact).await?;
                    staging = Some(path);
                    // Its parent is either ours as well, or was there before.
                    break;
                }
            }

            // Registered before it can start: a CLI of ours ending in this folder meanwhile leaves
            // its staging folders to this one.
            let in_folder = files::cli_started(&cwd, staging);

            let recorder = match &spec.record_to {
                Some(path) => Some(Arc::new(Recorder::create(
                    path,
                    &record::Header {
                        fixture: record::FORMAT,
                        provider: ProviderKind::Claude,
                        cli_version: self.version().await,
                        recorded_at_ms: now_ms(),
                        title: "Claude session".into(),
                    },
                    spec.redactor.clone(),
                )?)),
                None => None,
            };
            let mut process_spec = self.env.spec(&binary);
            process_spec.args = args.into_iter().map(Into::into).collect();
            process_spec.cwd = Some(cwd.clone());
            let mut env = session_env(&spec);
            // Claude keeps its own temp files, and points sandboxed commands' TMPDIR, under
            // `CLAUDE_CODE_TMPDIR` (`/tmp` by default). A session with a TMPDIR of its own
            // gets a short folder of its own there, removed with it.
            if let Some((_, tmp)) = spec.env.iter().find(|(name, _)| name == "TMPDIR")
                && !spec
                    .env
                    .iter()
                    .any(|(name, _)| name == "CLAUDE_CODE_TMPDIR")
            {
                let dir = match files::temp_dir_path() {
                    Some(dir) => {
                        let path = dir.display().to_string();
                        ledger
                            .record(Artifact::ClaudeTempDir { path: path.clone() })
                            .await?;
                        ledger
                            .record(Artifact::ProcessesIn { dir: path.clone() })
                            .await?;
                        files::create_temp_dir(&dir, self.platform.paths())?;
                        path
                    }
                    None => tmp.clone(),
                };
                env.push(("CLAUDE_CODE_TMPDIR".into(), dir));
            }
            crate::cli::apply_session_env(&mut process_spec, &env, &spec.unset_env);
            process_spec.low_priority = spec.low_priority;
            let homes: Vec<PathBuf> = self
                .config_dir()
                .into_iter()
                .chain(self.main_config.clone())
                .collect();
            let process_spec =
                crate::cli::protect(self.platform.as_ref(), process_spec, &spec, &homes)?;
            let process::Spawned { process, stdout } = process::spawn(
                self.platform.clone(),
                &process_spec,
                process::Options {
                    private_logs: ledger.private_logs(),
                    recorder,
                    redactor: spec.redactor.clone(),
                    ledger: Some(ledger.clone()),
                    sweep_dir: spec.sweep_dir.clone(),
                },
            )?;
            ledger
                .record(Artifact::Process {
                    pid: process.pid(),
                    started_at_ms: process.started_at_ms(),
                })
                .await?;

            let (events_tx, events) = mpsc::channel(EVENTS);
            let shared = Arc::new(Shared {
                pending: Mutex::new(HashMap::new()),
                approvals: Mutex::new(HashMap::new()),
                events: Events::new(events_tx, spec.redactor.clone()),
                turn_active: AtomicBool::new(false),
                next_request: AtomicU64::new(1),
            });
            let (parser_tx, parser_rx) = mpsc::unbounded_channel();
            tokio::spawn(read_loop(
                process.clone(),
                stdout,
                shared.clone(),
                parser_rx,
                !spec.auto_compact,
                in_folder,
            ));
            let session = Arc::new(ClaudeSession {
                native_id: native_id.clone(),
                suggests,
                compacts: cli_version
                    .as_deref()
                    .is_some_and(|version| version_at_least(version, COMPACT_SINCE)),
                process,
                shared,
                parser: parser_tx,
            });

            let mut initialize = Map::new();
            initialize.insert("subtype".into(), "initialize".into());
            let answer = match session.request(initialize, INITIALIZE_TIMEOUT).await {
                Ok(answer) => answer,
                Err(err) => {
                    session.close().await;
                    return Err(err);
                }
            };
            let model = spec.model.clone().or_else(|| {
                answer
                    .get("models")
                    .and_then(Value::as_array)
                    .and_then(|models| models.first())
                    .and_then(|model| model.get("value"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
            session
                .emit(ProviderEvent::SessionStarted {
                    native_id,
                    model,
                    cwd: Some(cwd.display().to_string()),
                    cli_version,
                })
                .await;
            Ok(Started { session, events })
        })
    }

    fn remove(&self, artifacts: Vec<Artifact>) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let Some(config) = self.history_dir() else {
                return Ok(());
            };
            tokio::task::spawn_blocking(move || files::remove(&config, &artifacts))
                .await
                .map_err(|err| Error::Io(std::io::Error::other(err)))?
        })
    }

    fn terminal(&self, spec: SessionSpec, cwd: PathBuf) -> BoxFuture<'_, Result<TerminalCommand>> {
        Box::pin(async move {
            let Origin::Resume { native_id } = &spec.origin else {
                return Err(Error::Invalid(
                    "only a session that exists can open in a terminal".into(),
                ));
            };
            files::check_session_id(native_id)?;
            let program = self.binary()?;
            let cwd = cwd.canonicalize().map_err(|err| {
                Error::Invalid(format!("working directory {}: {err}", cwd.display()))
            })?;
            // The session's flags without print mode's (an interactive Claude refuses
            // `--input-format`, `--include-partial-messages` and `--replay-user-messages`).
            // Its instructions are in the session record (`--system-prompt-snapshot`, on by
            // default), so `--append-system-prompt` isn't repeated.
            let user_config = self.config_dir();
            let mut args = Self::session_flags(
                &spec,
                &cwd,
                self.main_config.is_some(),
                user_config.as_deref(),
            );
            args.push("--resume".into());
            args.push(native_id.clone());
            let mut process = self.env.spec(&program);
            crate::cli::apply_session_env(&mut process, &session_env(&spec), &spec.unset_env);
            Ok(TerminalCommand {
                program,
                args,
                cwd,
                env: process.env,
            })
        })
    }

    fn replayer(&self) -> Box<dyn Replayer> {
        Box::new(ClaudeReplayer {
            parser: Parser::replay(),
            asked: HashSet::new(),
        })
    }

    fn took_input(&self, native_id: &str, needle: &str) -> BoxFuture<'_, bool> {
        let dir = self.history_dir();
        let (id, needle) = (native_id.to_owned(), needle.to_owned());
        Box::pin(async move {
            let Some(dir) = dir else {
                return false;
            };
            tokio::task::spawn_blocking(move || crate::history::claude_took(&dir, &id, &needle))
                .await
                .unwrap_or(false)
        })
    }

    fn past_folders(&self) -> BoxFuture<'_, Vec<crate::history::PastFolder>> {
        let dir = self.history_dir();
        Box::pin(async move {
            let Some(dir) = dir else {
                return Vec::new();
            };
            tokio::task::spawn_blocking(move || crate::history::claude(&dir))
                .await
                .unwrap_or_default()
        })
    }
}

/// The CLI's model list as the picker offers it: each model under its real name ("Opus 5.5",
/// not "Default (recommended)"), the `default` alias folded into the entry it resolves to, the
/// model `settings.json` picks marked default, each model's effort as `claude` would run it, and
/// the older models of a family ("Opus 4.8" beside "Opus 5.5") marked legacy.
fn model_infos(models: &[Value], settings: &Value) -> Vec<ModelInfo> {
    let mut infos: Vec<ModelInfo> = models
        .iter()
        .filter_map(|model| model_info(model, settings))
        .collect();
    // The `default` alias is a second row for a model already listed: keep the model's own row.
    if let Some(alias) = infos.iter().position(|info| info.id == "default")
        && let Some(resolved) = infos[alias].resolved.clone()
        && infos
            .iter()
            .any(|info| info.id != "default" && info.resolved.as_deref() == Some(&resolved))
    {
        infos.remove(alias);
        for info in &mut infos {
            info.is_default = info.resolved.as_deref() == Some(&resolved);
        }
    }
    // The model `claude` runs when not told one, per `settings.json`, wins over the CLI's pick.
    if let Some(chosen) = settings.get("model").and_then(Value::as_str)
        && let Some(index) = infos
            .iter()
            .position(|info| info.id == chosen)
            .or_else(|| infos.iter().position(|info| bare(&info.id) == chosen))
            .or_else(|| {
                infos
                    .iter()
                    .position(|info| info.resolved.as_deref().map(bare) == Some(bare(chosen)))
            })
    {
        for (at, info) in infos.iter_mut().enumerate() {
            info.is_default = at == index;
        }
    }
    // Two rows under one name: the long-context one says so.
    let names: Vec<String> = infos.iter().map(|info| info.display_name.clone()).collect();
    for info in &mut infos {
        let twins = names
            .iter()
            .filter(|name| **name == info.display_name)
            .count();
        if twins > 1 && info.id.ends_with("[1m]") {
            info.display_name.push_str(" (1M)");
        }
    }
    mark_superseded(&mut infos);
    infos
}

/// A model id without its context suffix: `opus[1m]` → `opus`.
fn bare(id: &str) -> &str {
    id.strip_suffix("[1m]").unwrap_or(id)
}

/// "Opus 5.5" from a description such as "Opus 5.5 with 1M context · Best for everyday tasks";
/// the CLI's display name when the description doesn't start with one.
fn real_name(display_name: &str, description: &str) -> String {
    let lead = description.split(" · ").next().unwrap_or_default();
    let name = lead.split(" with ").next().unwrap_or_default().trim();
    let named = name.split_whitespace().count() <= 3
        && name.chars().next().is_some_and(char::is_uppercase)
        && name.chars().any(|c| c.is_ascii_digit());
    if named {
        name.to_owned()
    } else {
        display_name.to_owned()
    }
}

/// The effort `claude` runs `model` at: its `modelSettings` entry, then the global
/// `effortLevel`, kept only if the model accepts it; else High (the API's default).
fn settings_effort(settings: &Value, resolved: Option<&str>, efforts: &[String]) -> Option<String> {
    let accepted = |level: &str| efforts.iter().any(|effort| effort == level);
    let per_model = resolved.and_then(|model| {
        settings
            .get("modelSettings")?
            .get(bare(model))?
            .get("effortLevel")?
            .as_str()
    });
    let global = settings.get("effortLevel").and_then(Value::as_str);
    per_model
        .filter(|level| accepted(level))
        .or(global.filter(|level| accepted(level)))
        .or(Some("high").filter(|level| accepted(level)))
        .or(efforts.last().map(String::as_str))
        .map(str::to_owned)
}

fn model_info(model: &Value, settings: &Value) -> Option<ModelInfo> {
    let id = model.get("value")?.as_str()?.to_owned();
    let text = |key: &str| {
        model
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let description = text("description");
    let resolved = model
        .get("resolvedModel")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let efforts: Vec<String> = model
        .get("supportedEffortLevels")
        .and_then(Value::as_array)
        .map(|levels| {
            levels
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Some(ModelInfo {
        is_default: id == "default",
        display_name: real_name(&text("displayName"), &description),
        default_effort: settings_effort(settings, resolved.as_deref(), &efforts),
        description,
        resolved,
        efforts,
        input_modalities: vec!["text".into(), "image".into()],
        fast: None,
        legacy: false,
        id,
    })
}

struct ClaudeReplayer {
    parser: Parser,
    /// Approval requests not answered yet.
    asked: HashSet<String>,
}

impl Replayer for ClaudeReplayer {
    fn feed(&mut self, dir: Direction, line: &str) -> Vec<ProviderEvent> {
        if dir == Direction::In {
            let sent: Value = serde_json::from_str(line).unwrap_or_default();
            if sent["type"] == "control_request" && sent["request"]["subtype"] == "interrupt" {
                self.parser.interrupt_requested();
            }
            if sent["type"] == "user" && sent["message"]["content"][0]["text"] == "/compact" {
                self.parser.compact_requested();
            }
            let answered = sent["response"]["request_id"].as_str().unwrap_or_default();
            if sent["type"] != "control_response" || !self.asked.remove(answered) {
                return Vec::new();
            }
            let answer = &sent["response"]["response"];
            let decision = if answer["behavior"] == "allow" {
                ApprovalDecision::Allow
            } else {
                ApprovalDecision::Deny {
                    message: answer["message"].as_str().unwrap_or_default().to_owned(),
                }
            };
            return vec![ProviderEvent::ApprovalResolved {
                id: answered.to_owned(),
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

/// State shared by the session handle and its reader.
struct Shared {
    pending: Mutex<HashMap<String, oneshot::Sender<std::result::Result<Value, String>>>>,
    /// Tool inputs of unanswered permission requests, by request id.
    approvals: Mutex<HashMap<String, Value>>,
    events: Events,
    turn_active: AtomicBool,
    next_request: AtomicU64,
}

enum ParserCommand {
    Interrupting,
    Compacting,
    WroteMessage,
    WriteFailed,
}

pub struct ClaudeSession {
    native_id: String,
    /// It runs with `--prompt-suggestions` ([`SessionSpec::suggestions`]).
    suggests: bool,
    /// The CLI's version takes `/compact` in stream-json.
    compacts: bool,
    process: Arc<CliProcess>,
    shared: Arc<Shared>,
    parser: mpsc::UnboundedSender<ParserCommand>,
}

impl ClaudeSession {
    async fn emit(&self, event: ProviderEvent) {
        self.shared.events.send(event).await;
    }

    async fn request(&self, mut request: Map<String, Value>, timeout: Duration) -> Result<Value> {
        let id = format!(
            "brigadier-{}",
            self.shared.next_request.fetch_add(1, Ordering::Relaxed)
        );
        let (tx, rx) = oneshot::channel();
        lock(&self.shared.pending).insert(id.clone(), tx);
        let subtype = request
            .remove("subtype")
            .unwrap_or_else(|| Value::String(String::new()));
        let mut body = Map::new();
        body.insert("subtype".into(), subtype);
        body.extend(request);
        if let Err(err) = self
            .process
            .write_line(&parse::control_request(&id, body))
            .await
        {
            lock(&self.shared.pending).remove(&id);
            return Err(err);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result.map_err(Error::Rejected),
            Ok(Err(_)) => Err(Error::Protocol(process_failure(&self.process))),
            Err(_) => {
                lock(&self.shared.pending).remove(&id);
                Err(Error::Timeout("Claude to answer"))
            }
        }
    }

    async fn input_content(input: &TurnInput) -> Result<Vec<Value>> {
        let mut content = Vec::with_capacity(input.parts.len());
        for part in &input.parts_with_file_notes() {
            match part {
                crate::InputPart::Text(text) if !text.trim().is_empty() => {
                    content.push(json!({ "type": "text", "text": text }));
                }
                crate::InputPart::Text(_) => {}
                crate::InputPart::Image(file) => {
                    let bytes = tokio::fs::read(&file.path).await.map_err(|err| {
                        Error::Invalid(format!("attachment {}: {err}", file.path.display()))
                    })?;
                    content.push(json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": file.mime,
                            "data": BASE64.encode(bytes),
                        },
                    }));
                }
            }
        }
        Ok(content)
    }

    async fn write_message(&self, input: TurnInput) -> Result<()> {
        if input.is_empty() {
            return Err(Error::Invalid("the message is empty".into()));
        }
        let content = Self::input_content(&input).await?;
        // Told before writing, so the parser knows of it before Claude can answer.
        let _ = self.parser.send(ParserCommand::WroteMessage);
        let written = self.process.write_line(&parse::user_message(content)).await;
        if written.is_err() {
            let _ = self.parser.send(ParserCommand::WriteFailed);
        }
        written
    }
}

impl ProviderSession for ClaudeSession {
    fn native_id(&self) -> String {
        self.native_id.clone()
    }

    fn send(&self, input: TurnInput) -> BoxFuture<'_, Result<()>> {
        // A message sent during a turn is folded into it, exactly like a steer.
        Box::pin(self.write_message(input))
    }

    fn steer(&self, input: TurnInput) -> BoxFuture<'_, Result<()>> {
        // Claude takes a message sent mid-turn at its next step; when the turn already ended,
        // the message starts the next one.
        Box::pin(self.write_message(input))
    }

    fn can_compact(&self) -> bool {
        self.compacts
    }

    fn pause_suggestions(&self, paused: bool) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if !self.suggests {
                return Ok(());
            }
            let mut request = Map::new();
            request.insert("subtype".into(), "set_prompt_suggestions_paused".into());
            request.insert("paused".into(), paused.into());
            self.request(request, CONTROL_TIMEOUT).await.map(drop)
        })
    }

    fn compact(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if !self.compacts {
                return Err(Error::Invalid(format!(
                    "compacting needs Claude Code {COMPACT_SINCE} or newer"
                )));
            }
            if self.shared.turn_active.load(Ordering::Acquire) {
                return Err(Error::Invalid(
                    "Claude is still answering; compact once it is done".into(),
                ));
            }
            // Claude runs the slash command when it arrives as a message.
            let _ = self.parser.send(ParserCommand::Compacting);
            self.write_message(TurnInput::text("/compact")).await
        })
    }

    fn interrupt(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if !self.shared.turn_active.load(Ordering::Acquire) {
                return Ok(());
            }
            let _ = self.parser.send(ParserCommand::Interrupting);
            let mut request = Map::new();
            request.insert("subtype".into(), "interrupt".into());
            self.request(request, CONTROL_TIMEOUT).await.map(drop)
        })
    }

    fn answer(&self, approval_id: String, decision: ApprovalDecision) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let input = lock(&self.shared.approvals)
                .remove(&approval_id)
                .ok_or_else(|| Error::Invalid(format!("no pending approval {approval_id}")))?;
            let response = approval_response(input, &decision);
            let line = json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": approval_id,
                    "response": response,
                },
            });
            self.process.write_line(&line.to_string()).await
        })
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.process.shutdown(EXIT_GRACE).await;
        })
    }

    fn is_running(&self) -> bool {
        self.process.is_running()
    }

    fn left_behind(&self) -> Vec<(u32, Option<f64>)> {
        self.process.left_behind()
    }
}

/// Reads Claude's output until it exits: forwards events, pairs control responses, keeps
/// permission requests until they are answered. Once it exited, `in_folder` goes: the staging
/// folders it may have left empty are tidied ([`files::CliInFolder`]).
async fn read_loop(
    process: Arc<CliProcess>,
    mut stdout: mpsc::Receiver<String>,
    shared: Arc<Shared>,
    mut commands: mpsc::UnboundedReceiver<ParserCommand>,
    refuses_compaction: bool,
    in_folder: files::CliInFolder,
) {
    let mut parser = Parser::live();
    parser.refuses_compaction = refuses_compaction;
    while let Some(line) = stdout.recv().await {
        while let Ok(command) = commands.try_recv() {
            match command {
                ParserCommand::Interrupting => parser.interrupt_requested(),
                ParserCommand::Compacting => parser.compact_requested(),
                ParserCommand::WroteMessage => parser.wrote_message(),
                ParserCommand::WriteFailed => parser.write_failed(),
            }
        }
        for output in parser.feed(&line) {
            match output {
                Output::Event(event) => {
                    // When nobody listens any more, keep draining so the CLI never blocks.
                    shared.events.send(event).await;
                }
                Output::Control(Control::Response { request_id, result }) => {
                    if let Some(reply) = lock(&shared.pending).remove(&request_id) {
                        let _ = reply.send(result);
                    }
                }
                Output::Control(Control::Approval { request_id, input }) => {
                    lock(&shared.approvals).insert(request_id, input);
                }
                Output::Control(Control::Cancelled { request_id }) => {
                    if lock(&shared.approvals).remove(&request_id).is_some() {
                        shared
                            .events
                            .send(ProviderEvent::ApprovalResolved {
                                id: request_id,
                                decision: ApprovalDecision::Deny {
                                    message: "withdrawn by Claude".into(),
                                },
                                decided_by: Decider::Policy,
                            })
                            .await;
                    }
                }
                Output::Control(Control::Unsupported {
                    request_id,
                    subtype,
                }) => {
                    let line = json!({
                        "type": "control_response",
                        "response": {
                            "subtype": "error",
                            "request_id": request_id,
                            "error": format!("Brigadier does not handle {subtype}"),
                        },
                    });
                    let _ = process.write_line(&line.to_string()).await;
                }
            }
        }
        shared
            .turn_active
            .store(parser.turn_active(), Ordering::Release);
    }

    let exit = process.exited().await;
    drop(in_folder);
    lock(&shared.pending).clear();
    shared.turn_active.store(false, Ordering::Release);
    shared
        .events
        .send(ProviderEvent::Exited {
            code: exit.code,
            signal: exit.signal,
            stderr_tail: (exit.code != Some(0))
                .then(|| process.stderr_tail())
                .flatten(),
        })
        .await;
}

fn process_failure(process: &CliProcess) -> String {
    match process.stderr_tail() {
        Some(tail) => format!("Claude exited: {tail}"),
        None => "Claude exited".into(),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// The permission answer for the CLI. A deny always has words: the CLI turns its message into
/// an error tool result, and the API fails the whole turn on an empty one.
fn approval_response(input: Value, decision: &ApprovalDecision) -> Value {
    match decision {
        // Brigadier answers the same command again itself.
        ApprovalDecision::Allow | ApprovalDecision::AllowSimilar => {
            json!({ "behavior": "allow", "updatedInput": input })
        }
        ApprovalDecision::Deny { message } => {
            let message = if message.trim().is_empty() {
                crate::DECLINED
            } else {
                message.as_str()
            };
            json!({ "behavior": "deny", "message": message })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A resume is refused up front only when the config's projects folder is there and no
    /// project holds the session's transcript (BUILD-PLAN D2.4).
    #[test]
    fn a_session_is_missing_only_when_no_project_holds_its_transcript() {
        let dir = Temp::new();
        let id = "0b9a3c52-6a43-4f2f-9b1e-1c0f2b7d5e10";
        // No projects folder: unknown, not missing.
        assert!(!files::session_missing(dir.path(), id));
        let project = dir.path().join("projects").join("-tmp-work");
        std::fs::create_dir_all(&project).unwrap();
        assert!(files::session_missing(dir.path(), id));
        std::fs::write(project.join(format!("{id}.jsonl")), "{}\n").unwrap();
        assert!(!files::session_missing(dir.path(), id));
    }

    #[tokio::test]
    async fn inline_content_preserves_text_image_text_order() {
        let dir = Temp::new();
        let path = dir.path().join("image.png");
        std::fs::write(&path, [1, 2, 3]).unwrap();
        let file = crate::InputFile {
            path,
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
        let content = ClaudeSession::input_content(&input).await.unwrap();
        assert_eq!(
            content
                .iter()
                .map(|part| part["type"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["text", "image", "text", "image", "text"]
        );
        assert_eq!(content[0]["text"], "before");
        assert_eq!(content[2]["text"], "between");
        assert_eq!(content[4]["text"], "after");
        assert_eq!(content[1]["source"]["data"], "AQID");
        assert_eq!(content[1], content[3]);
    }

    /// The CLI puts a deny's message into an error tool result, which the API refuses empty:
    /// a Deny with no words still says the user declined.
    #[test]
    fn a_deny_always_tells_the_model_why() {
        let blank = approval_response(
            json!({}),
            &ApprovalDecision::Deny {
                message: " ".into(),
            },
        );
        assert_eq!(blank["behavior"], "deny");
        assert_eq!(blank["message"], crate::DECLINED);
        let given = approval_response(
            json!({}),
            &ApprovalDecision::Deny {
                message: "This session may not do that.".into(),
            },
        );
        assert_eq!(given["message"], "This session may not do that.");
        // The user's own Deny: their reason when they gave one.
        assert_eq!(crate::user_deny_message("  "), crate::DECLINED);
        assert_eq!(
            crate::user_deny_message(" not now "),
            "The user declined this: not now"
        );
        let allowed = approval_response(json!({ "command": "ls" }), &ApprovalDecision::Allow);
        assert_eq!(allowed["behavior"], "allow");
        assert_eq!(allowed["updatedInput"]["command"], "ls");
    }

    /// The extra home keeps local state even though its transcript lives in the shared
    /// main history. Same-id state on an unrelated account is not part of this cleanup.
    #[tokio::test]
    async fn account_cleanup_removes_shared_history_and_only_its_local_session_files() {
        let dir = Temp::new();
        let main = dir.path().join("main");
        let extra = dir.path().join("extra");
        let unrelated = dir.path().join("unrelated");
        let id = uuid::Uuid::new_v4().to_string();
        let other_id = uuid::Uuid::new_v4().to_string();
        let paths = |home: &Path, id: &str| {
            vec![
                home.join("projects/cwd").join(format!("{id}.jsonl")),
                home.join("projects/cwd")
                    .join(id)
                    .join("subagents/agent.jsonl"),
                home.join("tasks").join(id).join("task.json"),
                home.join("session-env").join(id).join("env"),
                home.join("file-history").join(id).join("file"),
                home.join("debug").join(format!("{id}.txt")),
                home.join("todos").join(format!("{id}-agent.json")),
            ]
        };
        let removed: Vec<_> = paths(&main, &id)
            .into_iter()
            .chain(paths(&extra, &id))
            .collect();
        let kept: Vec<_> = paths(&unrelated, &id)
            .into_iter()
            .chain(paths(&main, &other_id))
            .chain(paths(&extra, &other_id))
            .collect();
        for path in removed.iter().chain(&kept) {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "session state").unwrap();
        }
        let env = CliEnv::from_vars([
            ("HOME".into(), dir.path().as_os_str().to_owned()),
            ("CLAUDE_CONFIG_DIR".into(), main.as_os_str().to_owned()),
        ]);
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(dir.path().join("data")),
        })
        .unwrap();
        let adapter = Claude::for_account(platform, &env, &extra);
        adapter
            .remove(vec![Artifact::ClaudeSession {
                session_id: id,
                home: Some(extra.display().to_string()),
                cwd: None,
            }])
            .await
            .unwrap();
        assert!(removed.iter().all(|path| !path.exists()), "{removed:?}");
        assert!(
            kept.iter()
                .all(|path| std::fs::read_to_string(path).unwrap() == "session state")
        );
    }

    /// BUILD-PLAN B5, live: a read-only session with the deny hook, the thread's tools and an
    /// MCP server, asked to run `Bash`, `Edit` and the server's tool, is refused each time and
    /// nothing changes on disk. It runs the machine's own `claude` (signed in), so only on
    /// request: `BRIGADIER_LIVE_CLAUDE=1 cargo test -p brigadier-providers --lib -- --ignored
    /// live_a_denied_session`.
    #[tokio::test]
    #[ignore = "runs the real Claude CLI"]
    async fn live_a_denied_session_runs_no_tool() {
        if std::env::var_os("BRIGADIER_LIVE_CLAUDE").is_none() {
            return;
        }
        struct Kept(std::sync::Mutex<Vec<Artifact>>);
        impl Ledger for Kept {
            fn record(&self, artifact: Artifact) -> BoxFuture<'_, Result<()>> {
                self.0.lock().unwrap().push(artifact);
                Box::pin(async { Ok(()) })
            }
            fn holds(&self, _: &Artifact) -> bool {
                false
            }
        }
        let cwd = Temp::new();
        let dir = cwd.path().to_path_buf();
        std::fs::write(dir.join("file.txt"), "one\n").unwrap();
        // A stdio MCP server whose one tool writes a marker file.
        let server = dir.parent().unwrap().join("probe_server.py");
        let marker = dir.parent().unwrap().join("mcp-marker");
        std::fs::write(
            &server,
            format!(
                r#"import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg:
        continue
    method, result = msg.get("method"), {{}}
    if method == "initialize":
        result = {{"protocolVersion": msg["params"]["protocolVersion"], "capabilities": {{"tools": {{}}}}, "serverInfo": {{"name": "probe", "version": "1"}}}}
    elif method == "tools/list":
        result = {{"tools": [{{"name": "touch_marker", "description": "Writes the marker file.", "inputSchema": {{"type": "object", "properties": {{}}}}}}]}}
    elif method == "tools/call":
        open({marker:?}, "w").write("called")
        result = {{"content": [{{"type": "text", "text": "marker written"}}]}}
    print(json.dumps({{"jsonrpc": "2.0", "id": msg["id"], "result": result}}), flush=True)
"#,
                marker = marker.display().to_string()
            ),
        )
        .unwrap();
        let reason = "This is the read-only note fork: no tool runs. Answer in text.";
        let spec = SessionSpec {
            ephemeral: true,
            access: Access::ReadOnly,
            tools: ToolSet::Thread,
            allowed_models: None,
            model: Some("claude-sonnet-5-5".into()),
            mcp_servers: vec![McpServer {
                name: "probe".into(),
                command: PathBuf::from("python3"),
                args: vec![server.display().to_string()],
                env: Vec::new(),
                tool_timeout_secs: Some(60),
                trusted: true,
                always_load: true,
                prompt_tools: Vec::new(),
            }],
            deny_tools: Some(reason.into()),
            suggestions: false,
            ..spec(&dir, &[])
        };
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(dir.parent().unwrap().join("data")),
        })
        .unwrap();
        let env = Arc::new(CliEnv::capture(&platform));
        let claude = Claude::new(platform, env);
        let kept = Arc::new(Kept(Default::default()));
        let Started {
            session,
            mut events,
        } = claude
            .start(spec, kept.clone())
            .await
            .expect("claude started");
        session
            .send(TurnInput::text(
                "This is a test of a tool denial hook. Make exactly these three tool calls, in \
                 order, even if one is refused: 1) Bash: `touch bash-marker`; 2) Edit file.txt, \
                 replacing `one` with `two`; 3) the MCP tool mcp__probe__touch_marker. Then quote \
                 what each call returned.",
            ))
            .await
            .unwrap();
        let mut refused = Vec::new();
        let mut seen = Vec::new();
        let turn = async {
            while let Some(event) = events.recv().await {
                let output = match &event {
                    ProviderEvent::ToolCall { name, output, .. } => {
                        output.clone().map(|output| (name.clone(), output))
                    }
                    ProviderEvent::Command { output, .. } => {
                        output.clone().map(|output| ("Bash".to_owned(), output))
                    }
                    ProviderEvent::FileChanges {
                        status: ItemStatus::Failed,
                        ..
                    } => Some(("Edit".to_owned(), reason.to_owned())),
                    _ => None,
                };
                seen.push(format!("{event:?}"));
                if let Some((name, output)) = output
                    && output.contains("no tool runs")
                {
                    refused.push(name);
                }
                if matches!(
                    event,
                    ProviderEvent::TurnCompleted { .. } | ProviderEvent::Exited { .. }
                ) {
                    break;
                }
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(300), turn)
            .await
            .expect("the turn ended");
        session.close().await;
        let artifacts = kept.0.lock().unwrap().clone();
        let _ = claude.remove(artifacts).await;
        let _ = std::fs::remove_file(&server);
        eprintln!("refused: {refused:?}\n{}", seen.join("\n"));
        assert!(!dir.join("bash-marker").exists(), "Bash ran");
        assert!(!marker.exists(), "the MCP tool ran");
        assert_eq!(
            std::fs::read_to_string(dir.join("file.txt")).unwrap(),
            "one\n",
            "Edit ran"
        );
        assert!(refused.len() >= 3, "each call refused: {refused:?}");
    }

    /// Item 8 of the day-one hardening, live: a thread working from a folder of Brigadier's
    /// runs the project's own `SessionStart` hook and calls the project's own MCP server (from
    /// its `.mcp.json`, found behind tool search), both linked from the project's checkout.
    /// `BRIGADIER_LIVE_CLAUDE=1 cargo test -p brigadier-providers --lib -- --ignored
    /// live_a_thread_loads`.
    #[tokio::test]
    #[ignore = "runs the real Claude CLI"]
    async fn live_a_thread_loads_the_projects_own_hook_and_mcp_server() {
        if std::env::var_os("BRIGADIER_LIVE_CLAUDE").is_none() {
            return;
        }
        struct Kept(std::sync::Mutex<Vec<Artifact>>);
        impl Ledger for Kept {
            fn record(&self, artifact: Artifact) -> BoxFuture<'_, Result<()>> {
                self.0.lock().unwrap().push(artifact);
                Box::pin(async { Ok(()) })
            }
            fn holds(&self, _: &Artifact) -> bool {
                false
            }
        }
        let temp = Temp::new();
        let project = temp.path().to_path_buf();
        let root = project.parent().unwrap().to_path_buf();
        let (cwd, hook_marker, mcp_marker) = (
            root.join("orch"),
            root.join("hook-marker"),
            root.join("mcp-marker"),
        );
        std::fs::create_dir_all(project.join(".claude")).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let server = root.join("probe_server.py");
        std::fs::write(
            &server,
            format!(
                r#"import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg:
        continue
    method, result = msg.get("method"), {{}}
    if method == "initialize":
        result = {{"protocolVersion": msg["params"]["protocolVersion"], "capabilities": {{"tools": {{}}}}, "serverInfo": {{"name": "probe", "version": "1"}}}}
    elif method == "tools/list":
        result = {{"tools": [{{"name": "touch_marker", "description": "Writes the marker file.", "inputSchema": {{"type": "object", "properties": {{}}}}}}]}}
    elif method == "tools/call":
        open({marker:?}, "w").write("called")
        result = {{"content": [{{"type": "text", "text": "marker written"}}]}}
    print(json.dumps({{"jsonrpc": "2.0", "id": msg["id"], "result": result}}), flush=True)
"#,
                marker = mcp_marker.display().to_string()
            ),
        )
        .unwrap();
        std::fs::write(
            project.join(".mcp.json"),
            json!({ "mcpServers": { "probe": { "command": "python3", "args": ["-I", server] } } })
                .to_string(),
        )
        .unwrap();
        std::fs::write(
            project.join(".claude/settings.json"),
            json!({ "hooks": { "SessionStart": [{ "hooks": [{
                "type": "command",
                "command": format!("echo started >> {}", hook_marker.display()),
            }] }] } })
            .to_string(),
        )
        .unwrap();
        let spec = SessionSpec {
            ephemeral: true,
            access: Access::Full,
            tools: ToolSet::Thread,
            allowed_models: None,
            model: Some("claude-haiku-5-5".into()),
            owned_cwd: true,
            project: Some(project.clone()),
            suggestions: false,
            ..spec(&cwd, &[])
        };
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(root.join("data")),
        })
        .unwrap();
        let env = Arc::new(CliEnv::capture(&platform));
        let claude = Claude::new(platform, env);
        let kept = Arc::new(Kept(Default::default()));
        let Started {
            session,
            mut events,
        } = claude
            .start(spec, kept.clone())
            .await
            .expect("claude started");
        session
            .send(TurnInput::text(
                "This is a test of the project's MCP server. Find the tool \
                 mcp__probe__touch_marker with your tool search, call it once, then say done.",
            ))
            .await
            .unwrap();
        let mut seen = Vec::new();
        let turn = async {
            while let Some(event) = events.recv().await {
                seen.push(format!("{event:?}"));
                if matches!(
                    event,
                    ProviderEvent::TurnCompleted { .. } | ProviderEvent::Exited { .. }
                ) {
                    break;
                }
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(300), turn)
            .await
            .expect("the turn ended");
        session.close().await;
        let artifacts = kept.0.lock().unwrap().clone();
        let _ = claude.remove(artifacts).await;
        eprintln!("{}", seen.join("\n"));
        assert!(hook_marker.is_file(), "the project's hook ran");
        assert!(mcp_marker.is_file(), "the project's MCP server was called");
    }

    /// A fresh folder, removed after the test.
    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            let dir = std::env::temp_dir()
                .join(format!("brigadier-claude-{}", uuid::Uuid::new_v4()))
                .join("project");
            std::fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            if let Some(parent) = self.0.parent() {
                let _ = std::fs::remove_dir_all(parent);
            }
        }
    }

    fn allowed(ids: &[&str], outside: &[&str]) -> AllowedModels {
        AllowedModels {
            ids: ids.iter().map(|id| (*id).to_owned()).collect(),
            outside: outside.iter().map(|id| (*id).to_owned()).collect(),
        }
    }

    #[test]
    fn exact_ids_are_listed_when_no_other_model_extends_them() {
        let outside = [
            "claude-haiku-4-5-20251001",
            "claude-fable-5-1",
            "claude-opus-4-8",
        ];
        assert_eq!(
            available_models(&allowed(
                &["claude-opus-5-5[1m]", "claude-sonnet-5"],
                &outside
            )),
            Some(vec![
                "claude-opus-5-5".to_owned(),
                "claude-sonnet-5".to_owned()
            ])
        );
    }

    #[test]
    fn a_prefix_of_a_model_left_out_turns_sub_agents_off() {
        // `claude-opus-5` would also allow `claude-opus-5-5`.
        assert_eq!(
            available_models(&allowed(
                &["claude-opus-5", "claude-sonnet-5"],
                &["claude-opus-5-5"]
            )),
            None
        );
        // A model that only shares the start of a word is not admitted.
        assert!(available_models(&allowed(&["claude-opus-5"], &["claude-opus-55"])).is_some());
    }

    #[test]
    fn a_context_variant_left_out_turns_sub_agents_off() {
        // `claude-opus-5-5` allows `claude-opus-5-5[1m]` too.
        assert_eq!(
            available_models(&allowed(&["claude-opus-5-5"], &["claude-opus-5-5[1m]"])),
            None
        );
        assert_eq!(
            available_models(&allowed(&["claude-opus-5-5[1m]"], &["claude-opus-5-5"])),
            None
        );
    }

    #[test]
    fn a_dated_or_fast_model_left_out_turns_sub_agents_off() {
        assert_eq!(
            available_models(&allowed(&["claude-opus-5-5"], &["claude-opus-5-5-fast"])),
            None
        );
        assert_eq!(
            available_models(&allowed(
                &["claude-haiku-4-5"],
                &["claude-haiku-4-5-20251001"]
            )),
            None
        );
        // One routing allows is not outside: the list holds.
        assert_eq!(
            available_models(&allowed(&["claude-haiku-4-5"], &["claude-opus-5-5-fast"])),
            Some(vec!["claude-haiku-4-5".to_owned()])
        );
    }

    #[test]
    fn no_exact_id_turns_sub_agents_off() {
        assert_eq!(available_models(&allowed(&[], &["claude-opus-5-5"])), None);
    }

    #[test]
    fn an_extra_accounts_session_blanks_every_login_variable_in_its_flag_settings() {
        let cwd = Temp::new();
        let spec = spec(cwd.path(), &["claude-opus-5-5"]);
        let models = SubAgents::Any;
        let own = settings(&spec, cwd.path(), &models, false);
        assert!(
            own.get("env").is_none(),
            "the user's own login is left alone"
        );
        let account = settings(&spec, cwd.path(), &models, true);
        for name in crate::accounts::auth_vars(ProviderKind::Claude) {
            assert_eq!(account["env"][name], json!(""), "{name}");
        }
        let args = Claude::session_args(
            &spec,
            cwd.path(),
            "00000000-0000-4000-8000-000000000000",
            true,
            None,
        )
        .unwrap();
        let flag = &args[args.iter().position(|arg| arg == "--settings").unwrap() + 1];
        assert!(flag.contains("\"CLAUDE_CODE_OAUTH_TOKEN\":\"\""), "{flag}");
    }

    /// Only a session that asks for next-message guesses gets the flag, and only on a CLI that
    /// has it: an older one would refuse to start.
    #[test]
    fn only_a_session_that_asks_guesses_the_next_message_on_a_cli_that_can() {
        let cwd = Path::new("/nonexistent/suggestions");
        let mut thread = spec(cwd, &[]);
        thread.suggestions = true;
        assert!(suggests(&thread, Some("2.1.301")));
        assert!(suggests(&thread, Some("2.2.0")));
        assert!(!suggests(&thread, Some("2.1.300")));
        assert!(!suggests(&thread, None));
        let worker = spec(cwd, &[]);
        assert!(!worker.suggestions);
        assert!(!suggests(&worker, Some("2.1.301")));
    }

    fn spec(cwd: &Path, ids: &[&str]) -> SessionSpec {
        SessionSpec {
            cwd: cwd.to_owned(),
            model: Some("opus".into()),
            effort: None,
            fast: false,
            origin: Origin::New,
            ephemeral: false,
            access: Access::Full,
            append_system_prompt: None,
            mcp_servers: Vec::new(),
            tools: ToolSet::Lean,
            skill_plugins: Vec::new(),
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
            allowed_models: Some(allowed(ids, &["claude-opus-5-5", "claude-fable-5-1"])),
            auto_review: false,
            omit_ai_coauthors: false,
            output_hook: None,
            stop_hook: None,
            deny_tools: None,
            suggestions: false,
            protection: Default::default(),
        }
    }

    fn assert_thinking_args(args: &[String]) {
        assert_eq!(
            args.windows(2)
                .filter(|pair| *pair == ["--thinking-display", "summarized"])
                .count(),
            1
        );
    }

    #[test]
    fn every_session_requests_readable_thinking_in_print_mode() {
        let dir = Temp::new();
        let mut spec = spec(dir.path(), &["claude-sonnet-5-5"]);
        let native_id = "00000000-0000-4000-8000-000000000000";
        for origin in [
            Origin::New,
            Origin::Resume {
                native_id: native_id.into(),
            },
            Origin::Fork {
                native_id: native_id.into(),
            },
        ] {
            spec.origin = origin;
            // Threads, workers, reviewers and jobs share this builder.
            for tools in [
                ToolSet::None,
                ToolSet::Lean,
                ToolSet::Default,
                ToolSet::Web,
                ToolSet::Thread,
                ToolSet::Review,
                ToolSet::Reader,
            ] {
                spec.tools = tools;
                let args = Claude::session_args(&spec, dir.path(), native_id, false, None).unwrap();
                assert_thinking_args(&args);
                assert!(args.iter().any(|arg| arg == "--include-partial-messages"));
            }
        }
    }

    #[test]
    fn control_processes_also_request_readable_thinking() {
        assert_thinking_args(&Claude::control_args());
    }

    #[test]
    fn each_permission_level_gets_its_mode_and_rules() {
        let cwd = Temp::new();
        let cwd = cwd.path();
        let scoped = Access::Scoped {
            write_cwd: true,
            writable_roots: Vec::new(),
            network: true,
            deny_read: Vec::new(),
            unix_sockets: Vec::new(),
        };
        let with = |access: Access, auto_review: bool| SessionSpec {
            access,
            auto_review,
            ..spec(cwd, &["claude-sonnet-5"])
        };
        let models = SubAgents::Only(vec!["claude-sonnet-5".to_owned()]);
        let permissions =
            |spec: &SessionSpec| settings(spec, cwd, &models, false)["permissions"].clone();

        // Full access: no permission checks, no sandbox, nothing that asks; the model list
        // still holds.
        let full = with(Access::Full, false);
        assert_eq!(permission_mode(&full), "bypassPermissions");
        assert!(permissions(&full).get("ask").is_none());
        assert!(
            permissions(&full)
                .get("disableBypassPermissionsMode")
                .is_none()
        );
        assert_eq!(
            settings(&full, cwd, &models, false)["sandbox"]["enabled"],
            json!(false)
        );
        assert_eq!(
            settings(&full, cwd, &models, false)["availableModels"],
            json!(["claude-sonnet-5"])
        );
        assert_eq!(
            settings(&full, cwd, &models, false)["skipDangerousModePermissionPrompt"],
            json!(true)
        );

        // Approve for me: auto mode decides what leaves the sandbox.
        let auto = with(scoped.clone(), true);
        assert_eq!(permission_mode(&auto), "auto");
        assert!(permissions(&auto).get("ask").is_none());
        assert_eq!(
            permissions(&auto)["disableBypassPermissionsMode"],
            json!("disable")
        );
        assert!(
            settings(&auto, cwd, &models, false)
                .get("skipDangerousModePermissionPrompt")
                .is_none()
        );

        // Ask for approval: leaving the sandbox asks.
        let ask = with(scoped, false);
        assert_eq!(permission_mode(&ask), "acceptEdits");
        assert_eq!(
            permissions(&ask)["ask"],
            json!(["Bash(dangerouslyDisableSandbox:true)"])
        );
        // A trusted server's tool that leaves the sandbox asks too, though the server is
        // allowed (an ask rule wins over an allow rule).
        let server = McpServer {
            name: "brigadier".into(),
            command: PathBuf::from("/bin/true"),
            args: Vec::new(),
            env: Vec::new(),
            tool_timeout_secs: None,
            trusted: true,
            always_load: true,
            prompt_tools: vec!["run_unsandboxed".into()],
        };
        let thread = SessionSpec {
            mcp_servers: vec![server],
            ..ask.clone()
        };
        assert_eq!(
            permissions(&thread)["ask"],
            json!([
                "Bash(dangerouslyDisableSandbox:true)",
                "mcp__brigadier__run_unsandboxed"
            ])
        );
        assert_eq!(permissions(&thread)["allow"], json!(["mcp__brigadier"]));
        // Read-only: nothing asks, whatever the review setting.
        assert_eq!(permission_mode(&with(Access::ReadOnly, true)), "dontAsk");
        assert_eq!(permission_mode(&with(Access::ReadOnly, false)), "dontAsk");
    }

    #[test]
    fn a_one_shot_reviewer_reads_and_runs_only_git_diff_log_and_show() {
        let cwd = Temp::new();
        let review = SessionSpec {
            access: Access::ReadOnly,
            tools: ToolSet::Review,
            allowed_models: Some(AllowedModels::default()),
            ..spec(cwd.path(), &["claude-opus-5-5"])
        };
        let native_id = "00000000-0000-4000-8000-000000000000";
        let args = Claude::session_args(&review, cwd.path(), native_id, false, None).expect("args");
        let after = |flag: &str| {
            args.windows(2)
                .find(|pair| pair[0] == flag)
                .map(|pair| pair[1].clone())
        };
        assert_eq!(after("--tools").as_deref(), Some("Read,Grep,Glob,Bash"));
        assert_eq!(
            after("--allowedTools").as_deref(),
            Some("Read Grep Glob Bash(git diff:*) Bash(git log:*) Bash(git show:*)")
        );
        assert_eq!(after("--permission-mode").as_deref(), Some("dontAsk"));
        // Other sessions get no allow list on the command line.
        let worker = spec(cwd.path(), &["claude-opus-5-5"]);
        let args = Claude::session_args(&worker, cwd.path(), native_id, false, None).expect("args");
        assert!(!args.iter().any(|arg| arg == "--allowedTools"));
    }

    #[test]
    fn a_thread_gets_its_tools_its_workspace_and_its_levels_mode() {
        let cwd = Temp::new();
        let workspace = Temp::new();
        let ws = workspace.path().to_owned();
        let native_id = "00000000-0000-4000-8000-000000000000";
        let thread = |access: Access, auto_review: bool| SessionSpec {
            access,
            auto_review,
            tools: ToolSet::Thread,
            skill_plugins: Vec::new(),
            add_dirs: vec![ws.clone()],
            allowed_models: None,
            ..spec(cwd.path(), &[])
        };
        let scoped = Access::Scoped {
            write_cwd: true,
            writable_roots: Vec::new(),
            network: true,
            deny_read: Vec::new(),
            unix_sockets: Vec::new(),
        };
        for (access, auto_review, mode) in [
            (Access::Full, false, "bypassPermissions"),
            (scoped.clone(), true, "auto"),
            (scoped, false, "acceptEdits"),
        ] {
            let spec = thread(access.clone(), auto_review);
            let args =
                Claude::session_args(&spec, cwd.path(), native_id, false, None).expect("args");
            let after = |flag: &str| {
                args.windows(2)
                    .find(|pair| pair[0] == flag)
                    .map(|pair| pair[1].clone())
            };
            assert_eq!(
                after("--tools").as_deref(),
                Some("Read,Grep,Glob,Bash,Edit,Write,WebSearch,WebFetch,ToolSearch")
            );
            assert_eq!(after("--permission-mode").as_deref(), Some(mode));
            assert_eq!(
                args.windows(2)
                    .filter(|pair| pair[0] == "--add-dir" && pair[1] == ws.display().to_string())
                    .count(),
                1,
                "the workspace once: {args:?}"
            );
            // Nothing positional follows the variadic `--add-dir`.
            let last = args.iter().rposition(|arg| arg == "--add-dir").unwrap();
            assert!(args[last + 2].starts_with("--"), "{args:?}");
            let settings = settings(
                &SessionSpec {
                    access: spec.access_with_dirs(),
                    ..spec.clone()
                },
                cwd.path(),
                &SubAgents::Any,
                false,
            );
            let allow = settings["permissions"]["allow"].as_array().expect("allow");
            assert!(allow.contains(&json!("WebSearch")) && allow.contains(&json!("WebFetch")));
            if access == Access::Full {
                assert_eq!(settings["sandbox"]["enabled"], json!(false));
            } else {
                assert_eq!(
                    settings["sandbox"]["filesystem"]["allowWrite"],
                    json!([resolved(&ws).display().to_string()]),
                    "the workspace is writable in the sandbox"
                );
            }
        }
        // A sandbox that keeps its working directory read-only keeps the workspace so too.
        let read_only = SessionSpec {
            access: Access::Scoped {
                write_cwd: false,
                writable_roots: Vec::new(),
                network: true,
                deny_read: Vec::new(),
                unix_sockets: Vec::new(),
            },
            ..thread(Access::Full, false)
        };
        assert!(read_only.access_with_dirs().writable_roots().is_empty());
    }

    #[test]
    fn only_a_writable_root_that_is_a_folder_is_added_as_one() {
        let cwd = Temp::new();
        let git = cwd.path().join("repo/.git");
        let objects = git.join("objects");
        let refs = git.join("refs/heads/brigadier/aa");
        std::fs::create_dir_all(&objects).expect("objects");
        std::fs::create_dir_all(&refs).expect("refs");
        let own = refs.join("task-1");
        std::fs::write(&own, "0000000000000000000000000000000000000000\n").expect("ref");
        let lock = refs.join("task-1.lock");
        let spec = SessionSpec {
            access: Access::Scoped {
                write_cwd: true,
                writable_roots: vec![objects.clone(), own.clone(), lock.clone()],
                network: true,
                deny_read: Vec::new(),
                unix_sockets: Vec::new(),
            },
            ..spec(cwd.path(), &["claude-sonnet-5"])
        };
        let args = Claude::session_args(
            &spec,
            cwd.path(),
            "00000000-0000-4000-8000-000000000000",
            false,
            None,
        )
        .expect("args");
        let added: Vec<&String> = args
            .windows(2)
            .filter(|pair| pair[0] == "--add-dir")
            .map(|pair| &pair[1])
            .collect();
        assert!(added.contains(&&objects.display().to_string()), "{added:?}");
        for file in [&own, &lock] {
            let real = resolved(file).display().to_string();
            assert!(
                !added
                    .iter()
                    .any(|dir| **dir == file.display().to_string() || **dir == real),
                "{added:?}"
            );
        }
        let settings = settings(&spec, cwd.path(), &SubAgents::Any, false);
        let allow = settings["sandbox"]["filesystem"]["allowWrite"]
            .as_array()
            .expect("allowWrite");
        assert!(
            allow.contains(&json!(resolved(&lock).display().to_string())),
            "{allow:?}"
        );
    }

    #[test]
    fn only_auto_mode_tells_its_classifier_never_to_allow_the_protected_paths() {
        let cwd = Temp::new();
        let protection = brigadier_sandbox::Protection {
            deny: vec![cwd.path().join("repo/.git/worktrees")],
            ..Default::default()
        };
        let session = |auto_review: bool| SessionSpec {
            access: Access::Scoped {
                write_cwd: true,
                writable_roots: Vec::new(),
                network: true,
                deny_read: Vec::new(),
                unix_sockets: Vec::new(),
            },
            auto_review,
            protection: protection.clone(),
            ..spec(cwd.path(), &["claude-sonnet-5"])
        };
        let auto = settings(&session(true), cwd.path(), &SubAgents::Any, false);
        let hard_deny = auto["autoMode"]["hard_deny"].as_array().expect("hard_deny");
        assert_eq!(hard_deny[0], json!("$defaults"), "the built-in rules stay");
        let rule = hard_deny[1].as_str().expect("rule");
        let worktrees = cwd.path().join("repo/.git/worktrees");
        assert!(
            rule.contains(&format!("`{}`", worktrees.display())),
            "{rule}"
        );
        let asks = settings(&session(false), cwd.path(), &SubAgents::Any, false);
        assert!(asks.get("autoMode").is_none(), "{asks}");
    }

    /// Only a thread gets the output hook: one command after every `Bash` call, successful or
    /// not, its grant in the CLI's environment and never on the command line.
    #[test]
    fn only_a_thread_gets_the_output_hook() {
        let cwd = Temp::new();
        let hook = OutputHook {
            command: PathBuf::from("/Applications/Brigadier App.app/brigadierd"),
            args: vec![
                "hook".into(),
                "post-tool-use".into(),
                "--data-dir".into(),
                "/Users/me/Library/Application Support/Brigadier".into(),
            ],
            env: vec![("BRIGADIER_HOOK_GRANT".into(), "brg_secret".into())],
            timeout_secs: 60,
        };
        let thread = SessionSpec {
            tools: ToolSet::Thread,
            allowed_models: None,
            output_hook: Some(hook.clone()),
            ..spec(cwd.path(), &[])
        };
        let hooks = &settings(&thread, cwd.path(), &SubAgents::Any, false)["hooks"];
        let expected = json!([{
            "matcher": "Bash",
            "hooks": [{
                "type": "command",
                "command": "'/Applications/Brigadier App.app/brigadierd' hook post-tool-use \
                    --data-dir '/Users/me/Library/Application Support/Brigadier'",
                "timeout": 60,
            }],
        }]);
        assert_eq!(hooks["PostToolUseFailure"], expected);
        // A successful file tool's call runs it too, for its folder's rules (BUILD-PLAN D1.6).
        let mut success = expected.clone();
        let mut files = expected[0].clone();
        files["matcher"] = json!("Read|Edit|Write|MultiEdit|NotebookEdit");
        success.as_array_mut().unwrap().push(files);
        assert_eq!(hooks["PostToolUse"], success);
        // Before a `Bash` call runs, for the files it names (#87).
        assert_eq!(hooks["PreToolUse"], expected);
        assert_eq!(hooks.as_object().map(Map::len), Some(3));
        let args = Claude::session_args(
            &thread,
            cwd.path(),
            "00000000-0000-4000-8000-000000000000",
            false,
            None,
        )
        .expect("args");
        assert!(!args.iter().any(|arg| arg.contains("brg_secret")));
        // Any other session has none, even if one was given.
        for tools in [
            ToolSet::Lean,
            ToolSet::Default,
            ToolSet::Web,
            ToolSet::Review,
        ] {
            let other = SessionSpec {
                tools,
                ..thread.clone()
            };
            assert!(
                settings(&other, cwd.path(), &SubAgents::Any, false)
                    .get("hooks")
                    .is_none(),
                "{tools:?}"
            );
        }
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    /// A worker's `Stop` hook is a `hooks.Stop` command with no matcher, beside nothing else,
    /// and its grant reaches the CLI's environment (BUILD-PLAN E2).
    #[test]
    fn a_worker_gets_its_stop_hook() {
        let cwd = Temp::new();
        let worker = SessionSpec {
            stop_hook: Some(OutputHook {
                command: PathBuf::from("/Applications/Brigadier.app/Contents/MacOS/brigadierd"),
                args: vec!["hook".into(), "stop".into()],
                env: vec![("BRIGADIER_STOP_GRANT".into(), "brg_stop".into())],
                timeout_secs: 60,
            }),
            ..spec(cwd.path(), &[])
        };
        let hooks = &settings(&worker, cwd.path(), &SubAgents::Any, false)["hooks"];
        assert_eq!(hooks.as_object().map(Map::len), Some(1), "{hooks}");
        assert_eq!(
            hooks["Stop"],
            json!([{"hooks": [{
                "type": "command",
                "command": "/Applications/Brigadier.app/Contents/MacOS/brigadierd hook stop",
                "timeout": 60,
            }]}])
        );
        assert!(
            session_env(&worker)
                .iter()
                .any(|(name, value)| name == "BRIGADIER_STOP_GRANT" && value == "brg_stop")
        );
        // Without one, no hooks at all.
        let plain = spec(cwd.path(), &[]);
        assert!(
            settings(&plain, cwd.path(), &SubAgents::Any, false)
                .get("hooks")
                .is_none_or(Value::is_null)
        );
    }

    /// A note fork's hook refuses every tool, native and MCP, before it runs, with the reason
    /// for the model; it keeps the thread's output hook, and nothing else the CLI is started
    /// with changes: the tool list and the prompt stay the thread's (BUILD-PLAN B5).
    #[test]
    fn a_denied_session_refuses_every_tool_with_the_same_tools() {
        let cwd = Temp::new();
        let thread = SessionSpec {
            tools: ToolSet::Thread,
            allowed_models: None,
            output_hook: Some(OutputHook {
                command: PathBuf::from("/bin/true"),
                args: Vec::new(),
                env: Vec::new(),
                timeout_secs: 60,
            }),
            ..spec(cwd.path(), &[])
        };
        let reason = "This is the read-only note fork: no tool runs. Answer in text.";
        let fork = SessionSpec {
            deny_tools: Some(reason.into()),
            suggestions: false,
            ..thread.clone()
        };
        let hooks = &settings(&fork, cwd.path(), &SubAgents::Any, false)["hooks"];
        assert_eq!(hooks.as_object().map(Map::len), Some(3), "{hooks}");
        assert_eq!(hooks["PreToolUse"][0]["matcher"], json!("*"));
        let command = hooks["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .expect("command");
        // The command answers a deny for any call it is given.
        let out = std::process::Command::new("sh")
            .args(["-c", command])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write as _;
                child
                    .stdin
                    .take()
                    .expect("stdin")
                    .write_all(br#"{"tool_name":"Bash","tool_input":{"command":"touch x"}}"#)?;
                child.wait_with_output()
            })
            .expect("hook ran");
        assert!(out.status.success());
        let answer: Value = serde_json::from_slice(&out.stdout).expect("json");
        assert_eq!(
            answer,
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": reason,
                },
            })
        );
        // Everything but the settings is as the thread starts.
        let without_settings = |spec: &SessionSpec| {
            let mut args = Claude::session_args(
                spec,
                cwd.path(),
                "00000000-0000-4000-8000-000000000000",
                false,
                None,
            )
            .expect("args");
            let at = args
                .iter()
                .position(|arg| arg == "--settings")
                .expect("settings");
            args.remove(at + 1);
            args
        };
        assert_eq!(without_settings(&fork), without_settings(&thread));
        // The thread's own `PreToolUse` hook only reads what a `Bash` call names (#87).
        let hooks = &settings(&thread, cwd.path(), &SubAgents::Any, false)["hooks"];
        assert_eq!(hooks["PreToolUse"][0]["matcher"], json!("Bash"), "{hooks}");
    }

    #[test]
    fn a_writer_may_commit_but_not_change_git_hooks_or_config() {
        let cwd = Temp::new();
        let git = cwd.path().join("repo.git");
        std::fs::create_dir_all(git.join("objects")).expect("objects");
        std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").expect("HEAD");
        let spec = SessionSpec {
            access: Access::Scoped {
                write_cwd: true,
                writable_roots: vec![git.clone()],
                network: true,
                deny_read: Vec::new(),
                unix_sockets: Vec::new(),
            },
            ..spec(cwd.path(), &["claude-sonnet-5"])
        };
        let models = SubAgents::Only(vec!["claude-sonnet-5".to_owned()]);
        let settings = settings(&spec, cwd.path(), &models, false);
        let filesystem = &settings["sandbox"]["filesystem"];
        let git = resolved(&git);
        let deny = settings["permissions"]["deny"]
            .as_array()
            .expect("deny rules");
        for tool in ["Edit", "Write", "NotebookEdit"] {
            for rule in [
                format!("{tool}(/{}/**)", git.join("hooks").display()),
                format!("{tool}(/{})", git.join("config").display()),
            ] {
                assert!(deny.contains(&json!(rule)), "{rule}");
            }
        }
        assert_eq!(filesystem["allowWrite"], json!([git.display().to_string()]));
        assert_eq!(
            filesystem["denyWrite"],
            json!([
                git.join("hooks").display().to_string(),
                git.join("config").display().to_string(),
            ])
        );
    }

    #[test]
    fn a_writable_checkout_keeps_its_git_hooks_and_config_out_of_reach() {
        let cwd = Temp::new();
        let checkout = cwd.path().join("checkout");
        let git = checkout.join(".git");
        std::fs::create_dir_all(git.join("objects")).expect("objects");
        std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").expect("HEAD");
        let spec = SessionSpec {
            access: Access::Scoped {
                write_cwd: true,
                writable_roots: Vec::new(),
                network: true,
                deny_read: Vec::new(),
                unix_sockets: Vec::new(),
            },
            skill_plugins: Vec::new(),
            add_dirs: vec![checkout.clone()],
            ..spec(cwd.path(), &["claude-sonnet-5"])
        };
        let settings = settings(&spec, cwd.path(), &SubAgents::Any, false);
        let git = resolved(&git);
        assert_eq!(
            settings["sandbox"]["filesystem"]["denyWrite"],
            json!([
                git.join("hooks").display().to_string(),
                git.join("config").display().to_string(),
            ])
        );
    }

    #[test]
    fn only_a_full_access_cli_runs_inside_the_protection_with_its_own_temp_given_back() {
        let cwd = Temp::new();
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(cwd.path().join("data")),
        })
        .expect("platform");
        let protection = brigadier_sandbox::Protection {
            deny: vec![cwd.path().join("data")],
            ..Default::default()
        };
        let mut process = brigadier_sandbox::SpawnSpec::new("/bin/true");
        process.env.push((
            "CLAUDE_CODE_TMPDIR".into(),
            "/tmp/brigadier-0123456789ab".into(),
        ));
        let full = SessionSpec {
            access: Access::Full,
            protection: protection.clone(),
            ..spec(cwd.path(), &["claude-sonnet-5"])
        };
        let home = cwd.path().join("home");
        let wrapped = crate::cli::protect(
            platform.as_ref(),
            process.clone(),
            &full,
            std::slice::from_ref(&home),
        )
        .expect("wrapped");
        if cfg!(target_os = "macos") {
            assert_eq!(wrapped.program, Path::new("/usr/bin/sandbox-exec"));
            let args: Vec<String> = wrapped
                .args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            assert!(
                args.iter()
                    .any(|arg| arg.ends_with("/private/tmp/brigadier-0123456789ab"))
            );
            assert!(
                args.iter().any(|arg| arg.contains("vnode-type DIRECTORY")),
                "kept in place"
            );
            assert!(
                args.iter()
                    .any(|arg| arg.ends_with(&*home.display().to_string()))
            );
        }
        let scoped = SessionSpec {
            access: Access::ReadOnly,
            protection,
            ..spec(cwd.path(), &["claude-sonnet-5"])
        };
        let unwrapped =
            crate::cli::protect(platform.as_ref(), process, &scoped, &[]).expect("as is");
        assert_eq!(unwrapped.program, Path::new("/bin/true"));
    }

    #[test]
    fn protected_refs_are_denied_except_where_a_deny_would_cover_its_own_branch() {
        let cwd = Temp::new();
        let common = cwd.path().join("repo/.git");
        let own = common.join("refs/heads/brigadier/aa/task-1");
        let spec = SessionSpec {
            access: Access::Scoped {
                write_cwd: true,
                writable_roots: vec![own.clone()],
                network: true,
                deny_read: Vec::new(),
                unix_sockets: Vec::new(),
            },
            protection: brigadier_sandbox::Protection {
                deny: vec![
                    common.join("refs/heads/brigadier"),
                    common.join("refs/heads/main"),
                    common.join("packed-refs"),
                ],
                allow: vec![own.clone()],
                ..Default::default()
            },
            ..spec(cwd.path(), &["claude-sonnet-5"])
        };
        let settings = settings(&spec, cwd.path(), &SubAgents::Any, false);
        let common = resolved(&common);
        assert_eq!(
            settings["sandbox"]["filesystem"]["denyWrite"],
            json!([
                common.join("refs/heads/main").display().to_string(),
                common.join("packed-refs").display().to_string(),
            ]),
            "a deny wins over every allow: one over its own ref would stop its commits"
        );
        let deny = settings["permissions"]["deny"].as_array().expect("deny");
        for tool in ["Edit", "Write", "NotebookEdit"] {
            let rule = format!("{tool}(/{})", common.join("packed-refs").display());
            assert!(deny.contains(&json!(rule)), "{rule}");
        }
        assert!(
            !deny
                .iter()
                .any(|rule| rule.as_str().unwrap().contains("refs/heads/brigadier"))
        );
    }

    #[test]
    fn a_reader_started_outside_its_checkout_may_not_edit_the_checkout_either() {
        let cwd = Temp::new();
        let checkout = cwd.path().join("checkout");
        let spec = SessionSpec {
            access: Access::Scoped {
                write_cwd: false,
                writable_roots: Vec::new(),
                network: true,
                deny_read: Vec::new(),
                unix_sockets: Vec::new(),
            },
            skill_plugins: Vec::new(),
            add_dirs: vec![checkout.clone()],
            ..spec(cwd.path(), &["claude-sonnet-5"])
        };
        let settings = settings(&spec, cwd.path(), &SubAgents::Any, false);
        let deny = settings["permissions"]["deny"]
            .as_array()
            .expect("deny rules");
        for dir in [cwd.path(), checkout.as_path()] {
            assert!(deny.contains(&json!(format!("Edit({})", rule_path(dir)))));
        }
        assert_eq!(
            settings["sandbox"]["filesystem"]["denyWrite"],
            json!(paths(&[cwd.path().to_owned(), checkout.clone()]))
        );
    }

    /// BUILD-PLAN C5: a Brain job reads and searches only, and its system prompt has no
    /// per-session sections, so every job of a batch starts on the same cached prefix.
    #[test]
    fn a_brain_job_reads_only_on_a_system_prompt_every_job_shares() {
        let cwd = Temp::new();
        let session = SessionSpec {
            tools: ToolSet::Reader,
            ..spec(cwd.path(), &[])
        };
        let args = Claude::session_args(&session, cwd.path(), "n", false, None).unwrap();
        let tools = args
            .iter()
            .position(|arg| arg == "--tools")
            .expect("--tools");
        assert_eq!(args[tools + 1], "Read,Grep,Glob");
        assert!(
            args.iter()
                .any(|arg| arg == "--exclude-dynamic-system-prompt-sections")
        );
        let worker = SessionSpec {
            tools: ToolSet::Lean,
            ..spec(cwd.path(), &[])
        };
        let args = Claude::session_args(&worker, cwd.path(), "n", false, None).unwrap();
        assert!(
            !args
                .iter()
                .any(|arg| arg == "--exclude-dynamic-system-prompt-sections")
        );
    }

    /// SESSION-FIXES-PLAN.md Q6.5: workers and threads start without what ships for a person
    /// at the keyboard, and workers can't open Claude's own questions or plan mode.
    #[test]
    fn workers_and_threads_start_without_bundled_extras_and_workers_without_questions() {
        let cwd = Temp::new();
        let cwd = cwd.path();
        for tools in [ToolSet::Lean, ToolSet::Thread, ToolSet::Reader] {
            let session = SessionSpec {
                tools,
                ..spec(cwd, &[])
            };
            let settings = settings(&session, cwd, &SubAgents::Any, false);
            assert_eq!(settings["disableBundledSkills"], json!(true));
            assert_eq!(settings["disableClaudeAiConnectors"], json!(true));
            assert_eq!(settings["enableArtifact"], json!(false));
            assert_eq!(settings["disableRemoteControl"], json!(true));
        }
        for tools in [ToolSet::Default, ToolSet::Web, ToolSet::Review] {
            let session = SessionSpec {
                tools,
                ..spec(cwd, &[])
            };
            assert!(
                settings(&session, cwd, &SubAgents::Any, false)
                    .get("disableBundledSkills")
                    .is_none()
            );
        }
        let worker = spec(cwd, &[]);
        let denied = denied_tools(&worker, &SubAgents::Any).unwrap();
        for tool in ["AskUserQuestion", "EnterPlanMode", "ExitPlanMode"] {
            assert!(denied.split(',').any(|name| name == tool), "{denied}");
        }
    }

    /// Every kind of session, compacting ones included, keeps its context while it sits idle,
    /// and takes no message from Claude's own messaging between sessions.
    #[test]
    fn no_session_compacts_while_idle() {
        let cwd = Temp::new();
        let cwd = cwd.path();
        for tools in [
            ToolSet::Default,
            ToolSet::Lean,
            ToolSet::None,
            ToolSet::Web,
            ToolSet::Thread,
            ToolSet::Review,
            ToolSet::Reader,
        ] {
            let session = SessionSpec {
                tools,
                auto_compact: true,
                ..spec(cwd, &[])
            };
            let settings = settings(&session, cwd, &SubAgents::Any, false);
            assert_eq!(settings["idleCompaction"], json!(false));
            // Messages from the user's other Claude sessions never come in around Brigadier.
            assert_eq!(settings["crossSessionInbound"], json!("refuse"));
        }
    }

    #[test]
    fn the_agent_tool_is_denied_only_when_the_list_cannot_hold() {
        let cwd = Temp::new();
        let cwd = cwd.path();
        let held = spec(cwd, &["claude-sonnet-5"]);
        let models = sub_agents(&held, cwd, None);
        assert_eq!(models, SubAgents::Only(vec!["claude-sonnet-5".to_owned()]));
        assert_eq!(
            settings(&held, cwd, &models, false)["availableModels"],
            json!(["claude-sonnet-5"])
        );
        assert_eq!(
            denied_tools(&held, &models).as_deref(),
            Some(LEAN_DENIED_TOOLS)
        );
        let leaky = spec(cwd, &["claude-opus-5"]);
        let models = sub_agents(&leaky, cwd, None);
        assert_eq!(models, SubAgents::Off);
        assert!(
            settings(&leaky, cwd, &models, false)
                .get("availableModels")
                .is_none()
        );
        assert_eq!(
            denied_tools(&leaky, &models),
            Some(format!("{LEAN_DENIED_TOOLS},{SUB_AGENT_TOOLS}"))
        );
        let unlimited = SessionSpec {
            allowed_models: None,
            tools: ToolSet::Default,
            ..spec(cwd, &[])
        };
        assert_eq!(sub_agents(&unlimited, cwd, None), SubAgents::Any);
        assert_eq!(denied_tools(&unlimited, &SubAgents::Any), None);
    }

    #[test]
    fn project_settings_that_name_models_turn_sub_agents_off() {
        let write = |dir: &Path, name: &str, text: &str| {
            std::fs::create_dir_all(dir.join(".claude")).expect("folder");
            std::fs::write(dir.join(".claude").join(name), text).expect("settings");
        };
        let held = |cwd: &Path| sub_agents(&spec(cwd, &["claude-sonnet-5"]), cwd, None);
        // Settings that leave models alone change nothing.
        let repo = Temp::new();
        let repo = repo.path();
        std::fs::create_dir(repo.join(".git")).expect("git");
        write(
            repo,
            "settings.json",
            r#"{"permissions": {"allow": ["Bash"]}}"#,
        );
        assert!(matches!(held(repo), SubAgents::Only(_)));
        // A list of their own (Claude joins the lists), a model in `env`, a mapping, or a
        // file that can't be read as JSON.
        for (name, text) in [
            (
                "settings.json",
                r#"{"availableModels": ["claude-fable-5-1"]}"#,
            ),
            (
                "settings.local.json",
                r#"{"env": {"ANTHROPIC_DEFAULT_OPUS_MODEL": "claude-fable-5-1"}}"#,
            ),
            ("settings.local.json", r#"{"modelOverrides": {}}"#),
            ("settings.json", "{ // not JSON"),
        ] {
            let repo = Temp::new();
            let repo = repo.path();
            std::fs::create_dir(repo.join(".git")).expect("git");
            write(repo, name, text);
            assert_eq!(held(repo), SubAgents::Off, "{name}: {text}");
            // From a folder inside the repository too.
            let inner = repo.join("crates");
            std::fs::create_dir(&inner).expect("folder");
            assert_eq!(held(&inner), SubAgents::Off, "{name}: {text}");
        }
        // A linked worktree: the main checkout's settings count.
        let main = Temp::new();
        let main = main.path();
        let gitdir = main.join(".git").join("worktrees").join("w1");
        std::fs::create_dir_all(&gitdir).expect("gitdir");
        std::fs::write(gitdir.join("commondir"), "../..\n").expect("commondir");
        let worktree = Temp::new();
        let worktree = worktree.path();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", gitdir.display()),
        )
        .expect("git file");
        assert!(matches!(held(worktree), SubAgents::Only(_)));
        write(
            main,
            "settings.local.json",
            r#"{"availableModels": ["claude-fable-5-1"]}"#,
        );
        assert_eq!(held(worktree), SubAgents::Off);
    }

    /// BUILD-PLAN D2.3: a session that must not compact refuses Claude's compaction with a
    /// `PreCompact` hook beside its other hooks; one that may compact has none. The hook exits
    /// 2 whatever it is sent.
    #[test]
    fn a_session_that_must_not_compact_refuses_compaction_with_a_hook() {
        let cwd = Temp::new();
        let worker = SessionSpec {
            auto_compact: false,
            deny_tools: Some("no".into()),
            ..spec(cwd.path(), &[])
        };
        let hooks = &settings(&worker, cwd.path(), &SubAgents::Any, false)["hooks"];
        assert!(hooks["PreToolUse"].is_array(), "the other hooks stay");
        let command = hooks["PreCompact"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .to_owned();
        let run = std::process::Command::new("sh")
            .args(["-c", &command])
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert_eq!(run.status.code(), Some(2));
        assert_eq!(
            String::from_utf8_lossy(&run.stderr).trim(),
            REFUSED_COMPACTION
        );
        let chat = spec(cwd.path(), &[]);
        assert!(
            settings(&chat, cwd.path(), &SubAgents::Any, false)["hooks"]
                .get("PreCompact")
                .is_none()
        );
    }

    /// A thread and a worker load the user's settings, the project's and their local ones, and
    /// their MCP servers beside Brigadier's; every other role keeps the sealed setup.
    #[test]
    fn threads_and_workers_run_with_the_users_own_setup() {
        let cwd = Temp::new();
        let flag = |args: &[String], name: &str| {
            args.iter()
                .position(|arg| arg == name)
                .map(|at| args[at + 1].clone())
        };
        for tools in [ToolSet::Thread, ToolSet::Lean] {
            let session = SessionSpec {
                tools,
                ..spec(cwd.path(), &[])
            };
            let args = Claude::session_args(&session, cwd.path(), "n", false, None).unwrap();
            assert_eq!(
                flag(&args, "--setting-sources").as_deref(),
                Some("user,project,local"),
                "{tools:?}"
            );
            assert!(
                !args.contains(&"--strict-mcp-config".to_owned()),
                "{tools:?}"
            );
            assert!(
                flag(&args, "--mcp-config").is_some(),
                "Brigadier's servers too"
            );
        }
        for tools in [
            ToolSet::Default,
            ToolSet::None,
            ToolSet::Web,
            ToolSet::Review,
            ToolSet::Reader,
        ] {
            let session = SessionSpec {
                tools,
                ..spec(cwd.path(), &[])
            };
            let args = Claude::session_args(&session, cwd.path(), "n", false, None).unwrap();
            assert_eq!(
                flag(&args, "--setting-sources").as_deref(),
                Some("project"),
                "{tools:?}"
            );
            assert!(
                args.contains(&"--strict-mcp-config".to_owned()),
                "{tools:?}"
            );
        }
    }

    /// The user's own settings can widen the models a worker's sub-agents may use (Claude joins
    /// the lists of all the settings it loads): a worker then runs without its Agent tool. A
    /// reviewer, which doesn't load them, is unaffected.
    #[test]
    fn user_settings_that_name_models_turn_a_workers_sub_agents_off() {
        let cwd = Temp::new();
        let config = cwd.path().parent().unwrap().join("config");
        std::fs::create_dir_all(&config).unwrap();
        let worker = spec(cwd.path(), &["claude-sonnet-5"]);
        let models = |text: &str, spec: &SessionSpec| {
            std::fs::write(config.join("settings.json"), text).unwrap();
            let args = Claude::session_args(spec, cwd.path(), "n", false, Some(&config)).unwrap();
            args.windows(2)
                .find(|pair| pair[0] == "--disallowedTools")
                .is_some_and(|pair| pair[1].contains(SUB_AGENT_TOOLS))
        };
        assert!(!models(r#"{"permissions": {"allow": ["Bash"]}}"#, &worker));
        assert!(models(
            r#"{"availableModels": ["claude-fable-5-1"]}"#,
            &worker
        ));
        assert!(models(
            r#"{"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "x"}}"#,
            &worker
        ));
        let reviewer = SessionSpec {
            tools: ToolSet::Review,
            ..worker.clone()
        };
        assert!(!models(
            r#"{"availableModels": ["claude-fable-5-1"]}"#,
            &reviewer
        ));
    }

    /// The user's `disableAllHooks` can't switch Brigadier's own hooks off in a session that
    /// loads their settings.
    #[test]
    fn brigadier_hooks_stay_on_under_the_users_settings() {
        let cwd = Temp::new();
        let worker = SessionSpec {
            auto_compact: false,
            ..spec(cwd.path(), &[])
        };
        let models = sub_agents(&worker, cwd.path(), None);
        let on = settings(&worker, cwd.path(), &models, false);
        assert_eq!(on["disableAllHooks"], json!(false));
        let reviewer = SessionSpec {
            tools: ToolSet::Review,
            ..worker
        };
        assert!(
            settings(&reviewer, cwd.path(), &models, false)
                .get("disableAllHooks")
                .is_none()
        );
    }
}
