//! Native MCP sign-in/list adapters and one-time legacy restoration support.
mod claude;
pub mod codex;
mod migration;
mod process;

use std::path::{Path, PathBuf};

use crate::item::{ServerSpec, TransportSpec};
use crate::{Agent, Catalog, Roots, Source};

pub use migration::{Applied, ApplyError, apply};
pub use process::{output_within, wait_within};
pub const CLI_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
pub use claude::Claude;
pub use codex::Codex;

/// Where an item is on, or off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// Every project: the user's own scope.
    Everywhere,
    /// One project only. `shared`: saved in the form the project commits, for the whole team;
    /// otherwise for this user only and kept out of the repository.
    Project { path: PathBuf, shared: bool },
}

/// One run of an agent's CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliCall {
    /// The binary, looked up on the login shell's PATH.
    pub program: &'static str,
    pub args: Vec<String>,
    /// The folder it runs in: a project's for a project-scoped command.
    pub cwd: Option<PathBuf>,
    /// It must run in a terminal (a PTY): it won't work with piped input.
    pub needs_terminal: bool,
}

impl CliCall {
    fn new(program: &'static str, args: &[&str]) -> Self {
        Self {
            program,
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            cwd: None,
            needs_terminal: false,
        }
    }

    fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    fn in_dir(mut self, dir: &Path) -> Self {
        self.cwd = Some(dir.to_owned());
        self
    }

    /// In `project`'s folder, when there is one.
    fn in_project(self, project: Option<&Path>) -> Self {
        match project {
            Some(dir) => self.in_dir(dir),
            None => self,
        }
    }
}

/// One edit of a key in a TOML config the CLI reads, keeping everything else in it as it was.
#[derive(Debug, Clone, PartialEq)]
pub struct TomlEdit {
    /// The key's path: `["plugins", "x@lib", "enabled"]`.
    pub key: Vec<String>,
    /// The value; `None` removes the key.
    pub value: Option<toml::Value>,
}

/// One step of a plan.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// The CLI's own command.
    Run(CliCall),
    /// A key in the user's own config, through the CLI's own config writer (not a text edit).
    UserConfig(TomlEdit),
    /// A key in a project's file the CLI reads (D8), kept out of the repository unless shared.
    ProjectFile {
        file: PathBuf,
        edit: TomlEdit,
        shared: bool,
    },
}

/// One MCP server as a CLI's own list reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// The name the CLI's own commands take (`plugin:<plugin>:<server>` for a Claude plugin's).
    pub name: String,
    /// The CLI says it needs a sign-in.
    pub needs_sign_in: bool,
    /// The CLI couldn't start it or reach it: its own words.
    pub failed: Option<String>,
}

/// `text` without terminal escapes (colours, links) and carriage returns.
pub fn plain_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                // CSI: up to a final byte in `@`..=`~`.
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC (a link's target): up to BEL or ESC `\`.
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// What an agent can't do, in plain words for the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported(pub String);

/// One agent CLI's skills, MCP servers and plugins.
pub trait AgentAdapter: Send + Sync {
    fn agent(&self) -> Agent;
    fn label(&self) -> &'static str;
    fn program(&self) -> &'static str;
    fn owns(&self, source: Source) -> bool;
    fn home_env(&self, roots: &Roots) -> Vec<(String, String)>;
    fn trusts(&self, catalog: &Catalog, project: &Path) -> bool;
    /// Used only to restore originals during the one-time migration.
    fn add_server(
        &self,
        name: &str,
        spec: &ServerSpec,
        place: &Place,
    ) -> Result<Vec<Step>, Unsupported>;
    fn login(&self, name: &str, project: Option<&Path>) -> CliCall;
    fn logout(&self, name: &str, project: Option<&Path>) -> CliCall;
    fn list_servers(&self, project: Option<&Path>) -> CliCall;
    fn parse_servers(&self, output: &str) -> Vec<Listed>;
    fn is_server(&self, listed: &str, server: &str, plugin: Option<&str>) -> bool;
    fn write_user_config(
        &self,
        roots: &Roots,
        env: &[(String, String)],
        program: &Path,
        edit: &TomlEdit,
    ) -> Result<(), ApplyError> {
        let _ = (roots, env, program, edit);
        Err(ApplyError::Failed(format!(
            "Brigadier has no way to change {}'s own settings.",
            self.label()
        )))
    }
}

static CLAUDE: Claude = Claude;
static CODEX: Codex = Codex;

/// Every agent Brigadier drives, in the order the page shows them.
pub fn adapters() -> [&'static dyn AgentAdapter; 2] {
    [&CLAUDE, &CODEX]
}

/// The adapter of `agent`.
pub fn adapter(agent: Agent) -> &'static dyn AgentAdapter {
    match agent {
        Agent::Claude => &CLAUDE,
        Agent::Codex => &CODEX,
    }
}

/// `spec` as the JSON both CLIs' server entries take: `{"type": "stdio", "command": …}`,
/// `{"type": "http", "url": …}`. `headers_key` is what the CLI calls the headers.
pub(crate) fn server_json(spec: &ServerSpec, headers_key: &str) -> serde_json::Value {
    let pairs = |pairs: &[(String, crate::Secret)]| -> serde_json::Map<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(name, value)| (name.clone(), value.expose().into()))
            .collect()
    };
    let mut entry = serde_json::Map::new();
    match &spec.transport {
        TransportSpec::Stdio {
            command,
            args,
            env,
            cwd,
        } => {
            entry.insert("type".into(), "stdio".into());
            entry.insert("command".into(), command.as_str().into());
            entry.insert("args".into(), args.clone().into());
            if !env.is_empty() {
                entry.insert("env".into(), pairs(env).into());
            }
            if let Some(cwd) = cwd {
                entry.insert("cwd".into(), cwd.display().to_string().into());
            }
        }
        TransportSpec::Http { url, headers } => {
            entry.insert("type".into(), "http".into());
            entry.insert("url".into(), url.as_str().into());
            if !headers.is_empty() {
                entry.insert(headers_key.into(), pairs(headers).into());
            }
        }
    }
    entry.into()
}

/// The agent whose own configuration `source` is.
pub fn owner(source: Source) -> Option<Agent> {
    adapters()
        .into_iter()
        .find(|adapter| adapter.owns(source))
        .map(|adapter| adapter.agent())
}

#[cfg(test)]
mod tests;
