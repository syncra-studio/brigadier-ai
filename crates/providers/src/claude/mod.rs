//! Claude Code adapter.
//!
//! Brigadier runs the user's own, unmodified `claude` binary in print mode with stream-json on
//! both sides, one persistent process per session. It never uses the Agent SDK and never reads
//! or handles OAuth tokens: the CLI keeps its own login. Everything else goes over stdio:
//!
//! - turns and steering are user messages (Claude folds a message sent mid-turn into the
//!   running turn);
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
//! Sessions load only the project's settings plus Brigadier's own (`--setting-sources
//! project`, `--settings`), so the user's personal hooks, plugins and allow rules never apply,
//! and only Brigadier's MCP servers (`--strict-mcp-config`).
//!
//! A worker's sub-agents run only on the models its task may use ([`SessionSpec::allowed_models`]):
//! `availableModels` in its settings lists their exact ids, and when Claude's prefix matching
//! would let another model in too, or the project's own settings could widen the list, the
//! worker runs without its Agent tool ([`sub_agents`]).

mod files;
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
    BoxFuture, Error, Ledger, Provider, ProviderSession, Replayer, Result, Started, now_ms,
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
/// Built-in tools a worker's lean start leaves out ([`ToolSet::Lean`]): scheduling, background
/// agents and messaging, worktrees (Brigadier manages them), notebooks and the like. Each tool's
/// description is part of every request; without these a worker starts at about 10k tokens
/// instead of 15k (measured on 2.1.285). Skills, subagents, web and tool search stay.
const LEAN_DENIED_TOOLS: &str = "Workflow,ScheduleWakeup,CronCreate,CronDelete,CronList,\
RemoteTrigger,PushNotification,DesignSync,ReportFindings,EnterWorktree,ExitWorktree,ListAgents,\
SendMessage,TaskStop,Monitor,NotebookEdit";
/// Claude's built-in tool that starts a sub-agent (`Task` is its older name).
const SUB_AGENT_TOOLS: &str = "Agent,Task";
/// A thread's built-in tools ([`ToolSet::Thread`]): no sub-agents (`Agent`/`Task`) and nothing
/// that runs on in the background (`Monitor`, scheduling); its workers are Brigadier's.
const THREAD_TOOLS: &str = "Read,Grep,Glob,Bash,Edit,Write,WebSearch,WebFetch";
/// A one-shot reviewer's built-in tools ([`ToolSet::Review`]).
const REVIEW_TOOLS: &str = "Read,Grep,Glob,Bash";
/// What a one-shot reviewer may run without asking (`--allowedTools`): under `dontAsk`
/// everything else is denied, and nothing prompts.
const REVIEW_ALLOWED: &str = "Read Grep Glob Bash(git diff:*) Bash(git log:*) Bash(git show:*)";

pub struct Claude {
    platform: Arc<dyn Platform>,
    env: Arc<CliEnv>,
    binary: Option<PathBuf>,
}

impl Claude {
    pub fn new(platform: Arc<dyn Platform>, env: Arc<CliEnv>) -> Self {
        let binary = env.resolve(ProviderKind::Claude);
        Self {
            platform,
            env,
            binary,
        }
    }

    fn binary(&self) -> Result<&Path> {
        self.binary
            .as_deref()
            .ok_or(Error::NotInstalled(ProviderKind::Claude))
    }

    /// Claude's configuration directory, where it keeps transcripts and session state.
    fn config_dir(&self) -> Option<PathBuf> {
        match self.env.var("CLAUDE_CONFIG_DIR") {
            Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
            _ => self.env.home().map(|home| home.join(".claude")),
        }
    }

    /// The user's `settings.json` (the model and effort `claude` runs with when not told).
    async fn settings(&self) -> Value {
        let Some(path) = self.config_dir().map(|dir| dir.join("settings.json")) else {
            return Value::Null;
        };
        tokio::fs::read_to_string(path)
            .await
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    async fn version(&self) -> Option<String> {
        let spec = self.env.spec(self.binary().ok()?).arg("--version");
        let output = process::run(&self.platform, &spec, STATUS_TIMEOUT)
            .await
            .ok()?;
        parse_version(&output.stdout)
    }

    /// Runs a throwaway process that answers control requests, then exits. It loads no
    /// settings, no MCP servers and writes no session.
    async fn control(&self, requests: Vec<Map<String, Value>>) -> Result<Vec<Value>> {
        let mut spec = self.env.spec(self.binary()?);
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

    fn session_args(spec: &SessionSpec, cwd: &Path, native_id: &str) -> Result<Vec<String>> {
        // Its extra folders are writable wherever its working directory is.
        let spec = &SessionSpec {
            access: spec.access_with_dirs(),
            ..spec.clone()
        };
        let mut args = Self::stream_args();
        args.extend(
            [
                "--include-partial-messages",
                "--verbose",
                "--replay-user-messages",
                "--permission-prompt-tool",
                "stdio",
                "--strict-mcp-config",
                "--setting-sources",
                "project",
            ]
            .map(str::to_owned),
        );
        args.push("--mcp-config".into());
        args.push(mcp_config(&spec.mcp_servers).to_string());
        // Decided once, for both the tools and the settings.
        let sub_agents = sub_agents(spec, cwd);
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
        }
        args.push("--settings".into());
        args.push(settings(spec, cwd, &sub_agents).to_string());
        args.push("--permission-mode".into());
        args.push(permission_mode(spec).into());
        // The session's own folders and the roots it may write. Both spellings of a root behind
        // a symlink (`/tmp` → `/private/tmp`): Claude matches the path as a tool was given it.
        // `--add-dir <directories...>` is variadic (claude --help, 2.1.292): only flags follow
        // it here, and turns go on stdin, so no positional argument is taken for a folder.
        let mut dirs: Vec<&PathBuf> = spec.add_dirs.iter().collect();
        for root in spec.access.writable_roots() {
            if !dirs.contains(&root) {
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
        if let Some(model) = &spec.model {
            args.push("--model".into());
            args.push(model.clone());
        }
        if let Some(effort) = &spec.effort {
            args.push("--effort".into());
            args.push(effort.clone());
        }
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
/// `cwd`.
fn sub_agents(spec: &SessionSpec, cwd: &Path) -> SubAgents {
    let Some(allowed) = &spec.allowed_models else {
        return SubAgents::Any;
    };
    if project_widens_models(cwd) {
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
        ["settings.json", "settings.local.json"].iter().any(|name| {
            match std::fs::read_to_string(dir.join(".claude").join(name)) {
                Ok(text) => serde_json::from_str::<Value>(&text)
                    .map_or(true, |settings| widens_models(&settings)),
                Err(err) => err.kind() != std::io::ErrorKind::NotFound,
            }
        })
    })
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
fn settings(spec: &SessionSpec, cwd: &Path, sub_agents: &SubAgents) -> Value {
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
            for root in writable_roots.iter().filter(|root| is_git_dir(root)) {
                let root = resolved(root);
                for tool in ["Edit", "Write", "NotebookEdit"] {
                    deny.push(format!("{tool}({})", rule_path(&root.join("hooks"))));
                    deny.push(format!("{tool}(/{})", root.join("config").display()));
                }
                deny_write.extend([root.join("hooks"), root.join("config")]);
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
    settings
}

/// A thread's output hook ([`SessionSpec::output_hook`]) as settings `hooks`: the same command
/// after every `Bash` call, successful (`PostToolUse`) or not (`PostToolUseFailure`). Settings
/// passed with `--settings` hold under `--setting-sources project`, and a successful call's
/// output is replaced by the hook's `hookSpecificOutput.updatedToolOutput` (code.claude.com/
/// docs/en/hooks, "PostToolUse decision control"; checked on 2.1.292,
/// docs/evidence/2026-10-07-thread-phase2-contracts.md §2).
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
    let entry = json!([{
        "matcher": "Bash",
        "hooks": [{ "type": "command", "command": command, "timeout": hook.timeout_secs }],
    }]);
    Some(json!({ "PostToolUse": entry, "PostToolUseFailure": entry }))
}

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
            let mut status = ProviderStatus {
                provider: ProviderKind::Claude,
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
                .spec(binary)
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
            let binary = self.binary()?.to_owned();
            let cwd = spec.cwd.canonicalize().map_err(|err| {
                Error::Invalid(format!("working directory {}: {err}", spec.cwd.display()))
            })?;
            let native_id = match &spec.origin {
                Origin::Resume { native_id } => native_id.clone(),
                Origin::New | Origin::Fork { .. } => uuid::Uuid::new_v4().to_string(),
            };
            let args = Self::session_args(&spec, &cwd, &native_id)?;

            // Recorded before the CLI can create them.
            if let Some(config) = self.config_dir() {
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
                })
                .await?;
            for path in files::staging_dirs(&cwd) {
                let artifact = Artifact::ClaudeStagingDir {
                    path: path.display().to_string(),
                };
                if !path.exists() || ledger.holds(&artifact) {
                    ledger.record(artifact).await?;
                    // Its parent is either ours as well, or was there before.
                    break;
                }
            }

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
            if let Some(secs) = spec
                .mcp_servers
                .iter()
                .filter_map(|server| server.tool_timeout_secs)
                .max()
            {
                // The overall limit, and the limit on a stdio call that sends nothing back
                // while it waits (30 minutes by default), which a blocking question can exceed.
                let ms = (secs * 1000).to_string();
                env.push(("MCP_TOOL_TIMEOUT".into(), ms.clone()));
                env.push(("CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT".into(), ms));
            }
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
            if !spec.auto_compact {
                env.push(("DISABLE_AUTO_COMPACT".into(), "1".into()));
            }
            crate::cli::apply_session_env(&mut process_spec, &env, &spec.unset_env);
            process_spec.low_priority = spec.low_priority;
            let process::Spawned { process, stdout } = process::spawn(
                self.platform.clone(),
                &process_spec,
                process::Options {
                    recorder,
                    redactor: spec.redactor.clone(),
                    ledger: Some(ledger.clone()),
                    owned_dir: spec.owned_cwd.then(|| cwd.clone()),
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
            ));
            let cli_version = self.version().await;
            let session = Arc::new(ClaudeSession {
                native_id: native_id.clone(),
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
            let Some(config) = self.config_dir() else {
                return Ok(());
            };
            tokio::task::spawn_blocking(move || files::remove(&config, &artifacts))
                .await
                .map_err(|err| Error::Io(std::io::Error::other(err)))?
        })
    }

    fn replayer(&self) -> Box<dyn Replayer> {
        Box::new(ClaudeReplayer {
            parser: Parser::replay(),
            asked: HashSet::new(),
        })
    }

    fn past_folders(&self) -> BoxFuture<'_, Vec<crate::history::PastFolder>> {
        let dir = self.config_dir();
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
            let response = match &decision {
                // Brigadier answers the same command again itself.
                ApprovalDecision::Allow | ApprovalDecision::AllowSimilar => {
                    json!({ "behavior": "allow", "updatedInput": input })
                }
                ApprovalDecision::Deny { message } => {
                    json!({ "behavior": "deny", "message": message })
                }
            };
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
}

/// Reads Claude's output until it exits: forwards events, pairs control responses, keeps
/// permission requests until they are answered.
async fn read_loop(
    process: Arc<CliProcess>,
    mut stdout: mpsc::Receiver<String>,
    shared: Arc<Shared>,
    mut commands: mpsc::UnboundedReceiver<ParserCommand>,
) {
    let mut parser = Parser::live();
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
    lock(&shared.pending).clear();
    shared.turn_active.store(false, Ordering::Release);
    shared
        .events
        .send(ProviderEvent::Exited {
            code: exit.code,
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

#[cfg(test)]
mod tests {
    use super::*;

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

    fn spec(cwd: &Path, ids: &[&str]) -> SessionSpec {
        SessionSpec {
            cwd: cwd.to_owned(),
            model: Some("opus".into()),
            effort: None,
            fast: false,
            origin: Origin::New,
            access: Access::Full,
            append_system_prompt: None,
            mcp_servers: Vec::new(),
            tools: ToolSet::Lean,
            add_dirs: Vec::new(),
            env: Vec::new(),
            unset_env: Vec::new(),
            low_priority: false,
            record_to: None,
            redactor: None,
            owned_cwd: false,
            auto_compact: true,
            allowed_models: Some(allowed(ids, &["claude-opus-5-5", "claude-fable-5-1"])),
            auto_review: false,
            omit_ai_coauthors: false,
            output_hook: None,
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
            ] {
                spec.tools = tools;
                let args = Claude::session_args(&spec, dir.path(), native_id).unwrap();
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
        let permissions = |spec: &SessionSpec| settings(spec, cwd, &models)["permissions"].clone();

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
            settings(&full, cwd, &models)["sandbox"]["enabled"],
            json!(false)
        );
        assert_eq!(
            settings(&full, cwd, &models)["availableModels"],
            json!(["claude-sonnet-5"])
        );

        // Approve for me: auto mode decides what leaves the sandbox.
        let auto = with(scoped.clone(), true);
        assert_eq!(permission_mode(&auto), "auto");
        assert!(permissions(&auto).get("ask").is_none());
        assert_eq!(
            permissions(&auto)["disableBypassPermissionsMode"],
            json!("disable")
        );

        // Ask for approval: leaving the sandbox asks.
        let ask = with(scoped, false);
        assert_eq!(permission_mode(&ask), "acceptEdits");
        assert_eq!(
            permissions(&ask)["ask"],
            json!(["Bash(dangerouslyDisableSandbox:true)"])
        );
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
        let args = Claude::session_args(&review, cwd.path(), native_id).expect("args");
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
        let args = Claude::session_args(&worker, cwd.path(), native_id).expect("args");
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
            let args = Claude::session_args(&spec, cwd.path(), native_id).expect("args");
            let after = |flag: &str| {
                args.windows(2)
                    .find(|pair| pair[0] == flag)
                    .map(|pair| pair[1].clone())
            };
            assert_eq!(
                after("--tools").as_deref(),
                Some("Read,Grep,Glob,Bash,Edit,Write,WebSearch,WebFetch")
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
        let hooks = &settings(&thread, cwd.path(), &SubAgents::Any)["hooks"];
        let expected = json!([{
            "matcher": "Bash",
            "hooks": [{
                "type": "command",
                "command": "'/Applications/Brigadier App.app/brigadierd' hook post-tool-use \
                    --data-dir '/Users/me/Library/Application Support/Brigadier'",
                "timeout": 60,
            }],
        }]);
        assert_eq!(hooks["PostToolUse"], expected);
        assert_eq!(hooks["PostToolUseFailure"], expected);
        assert_eq!(hooks.as_object().map(Map::len), Some(2));
        let args =
            Claude::session_args(&thread, cwd.path(), "00000000-0000-4000-8000-000000000000")
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
                settings(&other, cwd.path(), &SubAgents::Any)
                    .get("hooks")
                    .is_none(),
                "{tools:?}"
            );
        }
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
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
        let settings = settings(&spec, cwd.path(), &models);
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
            add_dirs: vec![checkout.clone()],
            ..spec(cwd.path(), &["claude-sonnet-5"])
        };
        let settings = settings(&spec, cwd.path(), &SubAgents::Any);
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

    #[test]
    fn the_agent_tool_is_denied_only_when_the_list_cannot_hold() {
        let cwd = Temp::new();
        let cwd = cwd.path();
        let held = spec(cwd, &["claude-sonnet-5"]);
        let models = sub_agents(&held, cwd);
        assert_eq!(models, SubAgents::Only(vec!["claude-sonnet-5".to_owned()]));
        assert_eq!(
            settings(&held, cwd, &models)["availableModels"],
            json!(["claude-sonnet-5"])
        );
        assert_eq!(
            denied_tools(&held, &models).as_deref(),
            Some(LEAN_DENIED_TOOLS)
        );
        let leaky = spec(cwd, &["claude-opus-5"]);
        let models = sub_agents(&leaky, cwd);
        assert_eq!(models, SubAgents::Off);
        assert!(
            settings(&leaky, cwd, &models)
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
        assert_eq!(sub_agents(&unlimited, cwd), SubAgents::Any);
        assert_eq!(denied_tools(&unlimited, &SubAgents::Any), None);
    }

    #[test]
    fn project_settings_that_name_models_turn_sub_agents_off() {
        let write = |dir: &Path, name: &str, text: &str| {
            std::fs::create_dir_all(dir.join(".claude")).expect("folder");
            std::fs::write(dir.join(".claude").join(name), text).expect("settings");
        };
        let held = |cwd: &Path| sub_agents(&spec(cwd, &["claude-sonnet-5"]), cwd);
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
}
