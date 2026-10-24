//! Each agent CLI's own skills, MCP and plugin handling behind one interface, so a new CLI is
//! one adapter and nothing above it names an agent (proposed in the plugins redesign plan).
//!
//! An adapter never changes a config by itself: it *plans* the CLI's own commands (and, where a
//! CLI has none but reads a per-project file, that file's edit), and [`run`] carries a plan
//! out. Reading stays the [`Catalog`]'s: an adapter says which of its sources are its own.
//!
//! Where an item lives:
//! - [`Place::Everywhere`]: the user's own scope, for every project.
//! - [`Place::Project`]: one project only, "just me" (kept out of the repository) unless
//!   `shared`, which writes the form the team commits.

mod claude;
mod codex;

use std::path::{Path, PathBuf};

use crate::{Agent, Catalog, Roots, Source};

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

    /// Installs plugin `id` (`<plugin>@<marketplace>`) at `place`.
    fn install_plugin(&self, id: &str, place: &Place) -> Result<Vec<Step>, Unsupported>;

    /// Uninstalls plugin `id` from `place`.
    fn remove_plugin(&self, id: &str, place: &Place) -> Result<Vec<Step>, Unsupported>;

    /// Turns installed plugin `id` on or off at `place`: off in one project for one that is on
    /// everywhere, on in one project for one that is off everywhere.
    fn set_plugin(&self, id: &str, place: &Place, on: bool) -> Result<Vec<Step>, Unsupported>;

    /// Turns the user's own MCP server `name` (not a plugin's) on or off at `place`.
    fn set_server(&self, name: &str, place: &Place, on: bool) -> Result<Vec<Step>, Unsupported>;

    /// Signs the CLI in to the server it lists as `name` (the name its own list reports).
    fn login(&self, name: &str) -> CliCall;

    /// Lists the MCP servers the CLI loads in `project` (or anywhere), as it reports them.
    fn list_servers(&self, project: Option<&Path>) -> CliCall;
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

/// The agent whose own configuration `source` is.
pub fn owner(source: Source) -> Option<Agent> {
    adapters()
        .into_iter()
        .find(|adapter| adapter.owns(source))
        .map(|adapter| adapter.agent())
}

#[cfg(test)]
mod tests;
