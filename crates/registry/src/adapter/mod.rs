//! Each agent CLI's own skills, MCP and plugin handling behind one interface, so a new CLI is
//! one adapter and nothing above it names an agent (proposed in the plugins redesign plan).
//!
//! An adapter never changes a config by itself: it *plans* the CLI's own commands (and, where a
//! CLI has none but reads a per-project file, that file's edit), and [`apply()`] carries a plan
//! out. Reading stays the [`Catalog`]'s: an adapter says which of its sources are its own.
//!
//! Where an item lives:
//! - [`Place::Everywhere`]: the user's own scope, for every project.
//! - [`Place::Project`]: one project only, "just me" (kept out of the repository) unless
//!   `shared`, which writes the form the team commits.

mod app_server;
mod apply;
mod claude;
pub mod codex;

use std::path::{Path, PathBuf};

use crate::item::{ServerSpec, TransportSpec};
use crate::{Agent, Catalog, Roots, Source};

pub use apply::{Applied, ApplyError, apply};
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

/// How an agent can keep an item to one project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectScope {
    /// Its own commands take a project scope.
    Native,
    /// It has no command for it but reads a project file, which Brigadier writes (D8). It
    /// reads that file only in projects it trusts.
    ProjectFile,
    /// It can't.
    Unsupported,
}

/// What an agent's CLI can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// It installs plugins from a local marketplace (the Library).
    pub plugins: bool,
    pub project_scope: ProjectScope,
    /// Its `mcp login` needs a terminal: Brigadier runs it in one.
    pub login_needs_terminal: bool,
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

    /// The product name shown to the user.
    fn label(&self) -> &'static str;

    /// The binary name looked up on the login shell's PATH.
    fn program(&self) -> &'static str;

    fn capabilities(&self) -> Capabilities;

    /// Whether `source` is this agent's own configuration (its plugins' too).
    fn owns(&self, source: Source) -> bool;

    /// The environment that points the CLI at `roots`' home for it (`CLAUDE_CONFIG_DIR`,
    /// `CODEX_HOME`).
    fn home_env(&self, roots: &Roots) -> Vec<(String, String)>;

    /// Whether it reads `project`'s own files: it trusts the project.
    fn trusts(&self, catalog: &Catalog, project: &Path) -> bool;

    /// Adds a local marketplace (a folder) to the CLI.
    fn add_marketplace(&self, dir: &Path) -> CliCall;

    /// Whether plugin `id` is installed for every project (on or off there).
    fn installed(&self, roots: &Roots, id: &str) -> bool;

    /// Where plugin `id` has an install of its own for one project only, which
    /// [`Self::remove_plugin`] at that place takes away. None by default: the CLI installs for
    /// every project and only switches it per project.
    fn installed_in_projects(&self, _roots: &Roots, _id: &str) -> Vec<Place> {
        Vec::new()
    }

    /// Reads local marketplace `name` again, when the CLI keeps a copy of it.
    fn refresh_marketplace(&self, name: &str) -> Option<CliCall>;

    /// Installs plugin `id` (`<plugin>@<marketplace>`) at `place`.
    fn install_plugin(&self, id: &str, place: &Place) -> Result<Vec<Step>, Unsupported>;

    /// Brings installed plugin `id` at `place` to its marketplace's version.
    fn update_plugin(&self, id: &str, place: &Place) -> Result<Vec<Step>, Unsupported>;

    /// Uninstalls plugin `id` from `place`.
    fn remove_plugin(&self, id: &str, place: &Place) -> Result<Vec<Step>, Unsupported>;

    /// Adds MCP server `name` to the user's own config (`Everywhere`) or the project's,
    /// as `spec` describes it: what Undo puts back.
    fn add_server(
        &self,
        name: &str,
        spec: &ServerSpec,
        place: &Place,
    ) -> Result<Vec<Step>, Unsupported>;

    /// Removes the user's own MCP server `name` (not a plugin's) from `place`.
    fn remove_server(&self, name: &str, place: &Place) -> Result<Vec<Step>, Unsupported>;

    /// Turns installed plugin `id` on or off at `place`: off in one project for one that is on
    /// everywhere, on in one project for one that is off everywhere.
    fn set_plugin(&self, id: &str, place: &Place, on: bool) -> Result<Vec<Step>, Unsupported>;

    /// Turns the user's own MCP server `name` (not a plugin's) on or off at `place`.
    fn set_server(&self, name: &str, place: &Place, on: bool) -> Result<Vec<Step>, Unsupported>;

    /// Signs the CLI in to the server it lists as `name` (the name its own list reports) in
    /// `project` (or anywhere), where [`Self::list_servers`] found it. It prints the sign-in
    /// page's address rather than opening a browser: Brigadier opens it.
    fn login(&self, name: &str, project: Option<&Path>) -> CliCall;

    /// Forgets the CLI's own sign-in to the server it lists as `name` in `project` (or
    /// anywhere).
    fn logout(&self, name: &str, project: Option<&Path>) -> CliCall;

    /// Lists the MCP servers the CLI loads in `project` (or anywhere), as it reports them.
    fn list_servers(&self, project: Option<&Path>) -> CliCall;

    /// Reads [`Self::list_servers`]' output.
    fn parse_servers(&self, output: &str) -> Vec<Listed>;

    /// Whether the name its list reports, `listed`, is server `server`, of plugin `plugin`
    /// (its name, without the marketplace) when it comes with one.
    fn is_server(&self, listed: &str, server: &str, plugin: Option<&str>) -> bool;

    /// Writes `edit` in the user's own config through the CLI's own config writer, run as
    /// `program` with `env`. Only an agent whose plans hold [`Step::UserConfig`] has one.
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
