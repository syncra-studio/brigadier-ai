//! The MCP servers and skills on this machine, from every agent's own configuration (PLAN.md §6
//! Phase 7), so a worker of one agent can be given what another agent has.
//!
//! Brigadier only reads these configs, never edits them:
//!
//! - Claude Code: `~/.claude.json` (`mcpServers`, and `projects[<path>].mcpServers`, the local
//!   scope), a project's `.mcp.json`, `~/.claude/skills` and a project's `.claude/skills`
//!   (`CLAUDE_CONFIG_DIR` moves the first and the user's skills).
//! - Codex: `$CODEX_HOME/config.toml` `mcp_servers`, a project's `.codex/config.toml` layers,
//!   `$CODEX_HOME/skills`, `~/.agents/skills` and a project's `.agents/skills`.
//! - Claude Desktop's `claude_desktop_config.json` and Cursor's `~/.cursor/mcp.json` and a
//!   project's `.cursor/mcp.json`.
//!
//! A [`Catalog`] holds the user's entries and, asked for a project, that project's. Each server
//! has a stable, source-qualified id (`claude:user:railway`, `codex:project:<path>:x`,
//! `desktop:pencil`). Which agents have an entry natively depends on the session (its agent, its
//! working folder and the project whose setup it loads): [`Catalog::server_native`],
//! [`Catalog::skill_native`]. Values of a server's environment and headers are [`Secret`]s: only
//! their names leave Brigadier's process, never their values.
//!
//! [`Registry`] keeps the user's part current with a file watcher; a project's part is read when
//! asked for. Variables (`${VAR}` in Claude's configs, `${env:VAR}` in Cursor's) expand from
//! [`Roots::vars`], the environment the CLIs themselves run with.

mod servers;
mod skills;
mod watch;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

pub use skills::{Skill, SkillRoot};

/// The agents that can be given what another has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Agent {
    Claude,
    Codex,
}

/// A value from a config (an env value, a header) that must not leave Brigadier: its `Debug`
/// shows nothing of it.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// Where each config lives, and the environment its variables expand from.
#[derive(Debug, Clone)]
pub struct Roots {
    pub home: PathBuf,
    /// Claude Code's configuration folder (`CLAUDE_CONFIG_DIR`, else `~/.claude`).
    pub claude_dir: PathBuf,
    /// Claude Code's `.claude.json`: in `CLAUDE_CONFIG_DIR` when that is set, else in the home.
    pub claude_json: PathBuf,
    /// `CODEX_HOME`, else `~/.codex`.
    pub codex_home: PathBuf,
    /// Claude Desktop's `claude_desktop_config.json`, where this platform keeps it.
    pub desktop_config: Option<PathBuf>,
    /// `~/.cursor`.
    pub cursor_dir: PathBuf,
    /// `~/.agents/skills`.
    pub agents_skills: PathBuf,
    /// The environment the CLIs run with (the user's login shell's), which `${VAR}` reads.
    pub vars: BTreeMap<String, String>,
}

impl Roots {
    /// The roots for the environment `vars`; `None` without a home folder.
    pub fn from_vars(vars: impl IntoIterator<Item = (String, String)>) -> Option<Self> {
        let vars: BTreeMap<String, String> = vars.into_iter().collect();
        let var = |name: &str| {
            vars.get(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        let home = if cfg!(windows) {
            var("USERPROFILE").or_else(|| var("HOME"))
        } else {
            var("HOME")
        }?;
        let claude_config = var("CLAUDE_CONFIG_DIR");
        let claude_dir = claude_config
            .clone()
            .unwrap_or_else(|| home.join(".claude"));
        let claude_json =
            claude_config.map_or_else(|| home.join(".claude.json"), |dir| dir.join(".claude.json"));
        let desktop_config = if cfg!(target_os = "macos") {
            Some(home.join("Library/Application Support/Claude"))
        } else if cfg!(windows) {
            var("APPDATA").map(|dir| dir.join("Claude"))
        } else {
            Some(
                var("XDG_CONFIG_HOME")
                    .unwrap_or_else(|| home.join(".config"))
                    .join("Claude"),
            )
        }
        .map(|dir| dir.join("claude_desktop_config.json"));
        Some(Self {
            claude_dir,
            claude_json,
            codex_home: var("CODEX_HOME").unwrap_or_else(|| home.join(".codex")),
            desktop_config,
            cursor_dir: home.join(".cursor"),
            agents_skills: home.join(".agents").join("skills"),
            home,
            vars,
        })
    }
}

/// Where a server is configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    /// `~/.claude.json` `mcpServers`.
    ClaudeUser,
    /// `~/.claude.json` `projects[<path>].mcpServers`: Claude's local scope, for sessions
    /// started in exactly that folder.
    ClaudeLocal,
    /// A project's `.mcp.json`.
    ClaudeProject,
    /// `$CODEX_HOME/config.toml`.
    CodexUser,
    /// A project's `.codex/config.toml`.
    CodexProject,
    /// Claude Desktop's config.
    Desktop,
    /// `~/.cursor/mcp.json`.
    CursorUser,
    /// A project's `.cursor/mcp.json`.
    CursorProject,
}

impl Source {
    /// How a person names it.
    pub fn label(self) -> &'static str {
        match self {
            Self::ClaudeUser => "Claude Code (user)",
            Self::ClaudeLocal => "Claude Code (local)",
            Self::ClaudeProject => "Claude Code (project)",
            Self::CodexUser => "Codex (user)",
            Self::CodexProject => "Codex (project)",
            Self::Desktop => "Claude Desktop",
            Self::CursorUser => "Cursor (user)",
            Self::CursorProject => "Cursor (project)",
        }
    }

    /// The id's start, before the project and name.
    fn prefix(self) -> &'static str {
        match self {
            Self::ClaudeUser => "claude:user",
            Self::ClaudeLocal => "claude:local",
            Self::ClaudeProject => "claude:project",
            Self::CodexUser => "codex:user",
            Self::CodexProject => "codex:project",
            Self::Desktop => "desktop",
            Self::CursorUser => "cursor:user",
            Self::CursorProject => "cursor:project",
        }
    }
}

/// How a server is reached.
#[derive(Debug, Clone, PartialEq)]
pub enum Transport {
    /// A process the CLI starts, speaking over its stdio.
    Stdio {
        /// Absolute when the config named a path relative to [`Self::Stdio::base`].
        command: String,
        args: Vec<String>,
        env: Vec<(String, Secret)>,
        /// The working folder the config names (absolute).
        cwd: Option<PathBuf>,
        /// The folder its relative paths resolve from in its source (a project's root for a
        /// project's config); `None`: wherever the session runs.
        base: Option<PathBuf>,
    },
    /// Streamable HTTP.
    Http {
        url: String,
        headers: Vec<(String, Secret)>,
    },
}

impl Transport {
    /// The folder the server runs in: the one its config names, else its base.
    pub fn working_dir(&self) -> Option<&Path> {
        match self {
            Self::Stdio { cwd, base, .. } => cwd.as_deref().or(base.as_deref()),
            Self::Http { .. } => None,
        }
    }
}

/// One MCP server, as one source configures it.
#[derive(Debug, Clone, PartialEq)]
pub struct Server {
    /// Stable and source-qualified: `claude:user:railway`, `codex:project:<path>:x`.
    pub id: String,
    /// The name its source gives it, which its tools are called by.
    pub name: String,
    pub source: Source,
    /// The project whose config holds it (the path a local-scope entry is keyed on).
    pub project: Option<PathBuf>,
    pub transport: Transport,
    /// A remote server with no credentials of its own in the config: it signs in with OAuth,
    /// which each agent does by itself (Codex: `codex mcp login`), so another agent's sign-in
    /// doesn't serve it as such (docs/research/phase7-probes.md (f)): [`Catalog::sign_in`].
    pub signs_in: bool,
}

/// How a session's agent is signed in to a server that [`Server::signs_in`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignIn {
    /// The agent has the server by itself, and its own sign-in serves it.
    Native,
    /// The sign-in another agent stored can serve it (read, and written back when renewed).
    ReusableFrom(Agent),
    /// No agent's stored sign-in can: one sign-in in Brigadier serves it to every agent.
    InBrigadier,
    /// Not known: nothing reads the agents' stored sign-ins yet.
    Unknown,
}

/// A session, as far as what it finds by itself goes.
#[derive(Debug, Clone, Copy)]
pub struct Session<'a> {
    pub agent: Agent,
    /// Its working folder.
    pub cwd: &'a Path,
    /// The project whose setup it loads (a trusted one): Claude's `.mcp.json` and `.claude`
    /// are linked into its folder, and a Codex session gets the project's Codex servers.
    pub project: Option<&'a Path>,
    /// It works in a checkout of [`Self::project`] (its worktree), where Codex finds the
    /// project's `.agents/skills` by itself.
    pub in_checkout: bool,
}

/// Why a name given for a server can't be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindError {
    Unknown,
    /// Several different servers have the name: their ids.
    Ambiguous(Vec<String>),
}

/// The servers and skills of the user and, when asked for one, of a project.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub servers: Vec<Server>,
    pub skills: Vec<Skill>,
    /// The user's own Codex `skills.config` entries (as JSON), which a session that switches
    /// skills off must keep: Codex's `skills.config` override replaces the whole list.
    pub codex_skill_config: Vec<serde_json::Value>,
    /// The projects the user trusts in Codex, whose `.codex` layers Codex loads.
    pub codex_trusted: Vec<PathBuf>,
}

impl Catalog {
    /// The user's entries and, for `project`, the project's.
    pub fn read(roots: &Roots, project: Option<&Path>) -> Self {
        let mut catalog = Self::user(roots);
        if let Some(project) = project {
            catalog.add_project(roots, project);
        }
        catalog
    }

    fn user(roots: &Roots) -> Self {
        let codex = servers::codex_config(roots);
        let mut catalog = Self {
            servers: servers::user(roots, codex.as_ref()),
            skills: Vec::new(),
            codex_skill_config: codex
                .as_ref()
                .map(servers::codex_skill_config)
                .unwrap_or_default(),
            codex_trusted: codex
                .as_ref()
                .map(servers::codex_trusted)
                .unwrap_or_default(),
        };
        skills::add_user(&mut catalog.skills, roots);
        catalog
    }

    fn add_project(&mut self, roots: &Roots, project: &Path) {
        self.servers.extend(servers::project(roots, project));
        skills::add_project(&mut self.skills, project);
    }

    /// Whether `session`'s agent has `server` by itself.
    pub fn server_native(&self, server: &Server, session: &Session<'_>) -> bool {
        let project = server.project.as_deref();
        match (server.source, session.agent) {
            (Source::ClaudeUser, Agent::Claude) | (Source::CodexUser, Agent::Codex) => true,
            // Claude keys its local scope on the exact folder a session starts in.
            (Source::ClaudeLocal, Agent::Claude) => project == Some(session.cwd),
            (Source::ClaudeProject, Agent::Claude) => project == session.project,
            // Codex loads a project's layers only where the user trusts it in Codex.
            (Source::CodexProject, Agent::Codex) => {
                project == session.project
                    && project
                        .is_some_and(|project| self.codex_trusted.iter().any(|p| p == project))
            }
            _ => false,
        }
    }

    /// Whether `session`'s agent finds `skill` by itself.
    pub fn skill_native(&self, skill: &Skill, session: &Session<'_>) -> bool {
        skill.roots.iter().any(|root| match (root, session.agent) {
            (SkillRoot::ClaudeUser, Agent::Claude)
            | (SkillRoot::AgentsUser | SkillRoot::CodexUser, Agent::Codex) => true,
            (SkillRoot::ClaudeProject(project), Agent::Claude) => {
                Some(project.as_path()) == session.project
            }
            (SkillRoot::AgentsProject(project), Agent::Codex) => {
                Some(project.as_path()) == session.project && session.in_checkout
            }
            _ => false,
        })
    }

    /// How `session`'s agent is signed in to `server`; `None` for one that doesn't sign in.
    pub fn sign_in(&self, server: &Server, session: &Session<'_>) -> Option<SignIn> {
        server.signs_in.then(|| {
            if self.server_native(server, session) {
                SignIn::Native
            } else {
                SignIn::Unknown
            }
        })
    }

    pub fn server(&self, id: &str) -> Option<&Server> {
        self.servers.iter().find(|server| server.id == id)
    }

    pub fn skill(&self, id: &str) -> Option<&Skill> {
        self.skills.iter().find(|skill| skill.id == id)
    }

    /// The server `entry` names: an id, or a name only one server has (servers that are the
    /// same under several sources, such as one server set up in both Claude apps, count once).
    pub fn find(&self, entry: &str) -> Result<&Server, FindError> {
        if let Some(server) = self.server(entry) {
            return Ok(server);
        }
        let named: Vec<&Server> = self
            .servers
            .iter()
            .filter(|server| server.name == entry)
            .collect();
        let Some(first) = named.first() else {
            return Err(FindError::Unknown);
        };
        if named
            .iter()
            .all(|server| server.transport == first.transport)
        {
            Ok(first)
        } else {
            Err(FindError::Ambiguous(
                named.iter().map(|server| server.id.clone()).collect(),
            ))
        }
    }
}

/// A server's id: its source, its project for a project's, and its name.
fn server_id(source: Source, project: Option<&Path>, name: &str) -> String {
    match project {
        Some(project) => format!("{}:{}:{name}", source.prefix(), project.display()),
        None => format!("{}:{name}", source.prefix()),
    }
}

/// The user's catalog, kept current by a watcher, and a project's read when asked.
pub struct Registry {
    roots: Roots,
    user: RwLock<Arc<Catalog>>,
    watcher: Mutex<Option<watch::Watcher>>,
}

impl Registry {
    /// Reads the user's configs once; [`Self::watch`] keeps them current.
    pub fn new(roots: Roots) -> Arc<Self> {
        let user = Catalog::user(&roots);
        Arc::new(Self {
            roots,
            user: RwLock::new(Arc::new(user)),
            watcher: Mutex::new(None),
        })
    }

    pub fn roots(&self) -> &Roots {
        &self.roots
    }

    /// The user's entries and, for `project`, the project's (read now).
    pub fn catalog(&self, project: Option<&Path>) -> Catalog {
        let mut catalog = Catalog::clone(&self.user.read().unwrap_or_else(|p| p.into_inner()));
        if let Some(project) = project {
            catalog.add_project(&self.roots, project);
        }
        catalog
    }

    /// Reads the user's configs again.
    pub fn refresh(&self) {
        let user = Arc::new(Catalog::user(&self.roots));
        *self.user.write().unwrap_or_else(|p| p.into_inner()) = user;
        if let Some(watcher) = self
            .watcher
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_mut()
        {
            watcher.rewatch(&self.watched());
        }
    }

    /// Keeps the user's catalog current: a change to any config or skill (a skill's
    /// `SKILL.md` through a symlink too) reads them again, half a second after the last one.
    /// Idle, it costs nothing. Ends with the registry.
    pub fn watch(self: &Arc<Self>) -> std::io::Result<()> {
        let registry = Arc::downgrade(self);
        let watcher = watch::Watcher::start(
            &self.watched(),
            Box::new(move || {
                if let Some(registry) = registry.upgrade() {
                    registry.refresh();
                }
            }),
        )?;
        *self.watcher.lock().unwrap_or_else(|p| p.into_inner()) = Some(watcher);
        Ok(())
    }

    /// What the watcher looks at.
    fn watched(&self) -> watch::Watched {
        let roots = &self.roots;
        let user = self.user.read().unwrap_or_else(|p| p.into_inner());
        let mut files = vec![
            roots.claude_json.clone(),
            roots.codex_home.join("config.toml"),
            roots.cursor_dir.join("mcp.json"),
        ];
        files.extend(roots.desktop_config.clone());
        let mut trees = vec![
            roots.claude_dir.join("skills"),
            roots.agents_skills.clone(),
            roots.codex_home.join("skills"),
        ];
        // A skill reached through a symlink: its own folder, wherever it is.
        for skill in &user.skills {
            if !trees.iter().any(|tree| skill.dir.starts_with(tree)) {
                trees.push(skill.dir.clone());
            }
        }
        watch::Watched { files, trees }
    }
}

#[cfg(test)]
mod tests;
