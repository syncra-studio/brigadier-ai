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
//! - The skills and MCP servers of the Claude Code and Codex plugins the user switched on
//!   ([`plugins`]), which are their agent's own.
//!
//! A [`Catalog`] holds the user's entries and, asked for a project, that project's. Each server
//! has a stable, source-qualified id (`claude:user:railway`, `codex:project:<path>:x`,
//! `desktop:pencil`). Which agents have an entry natively depends on the session (its agent, its
//! working folder and the project whose setup it loads): [`Catalog::server_native`],
//! [`Catalog::skill_native`]. Values of a server's environment and headers are [`Secret`]s: only
//! their names leave Brigadier's process, never their values.
//!
//! A remote server with no credentials in its config signs in with OAuth ([`Server::signs_in`]),
//! through each CLI's own `mcp login`, which keeps it in the CLI's own store.
//!
//! [`Registry`] keeps the user's part current with a file watcher; a project's part is read when
//! asked for. Variables (`${VAR}` in Claude's configs, `${env:VAR}` in Cursor's) expand from
//! [`Roots::vars`], the environment the CLIs themselves run with.

pub mod adapter;
pub mod add_parse;
pub mod claude_config;
pub mod item;
pub mod library;
mod plugins;
mod servers;
mod skills;
mod watch;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

pub use plugins::Plugin;
pub use skills::{Skill, SkillRoot};

/// The agents that can be given what another has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
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
    /// Claude Code's config file: `.claude.json` in `CLAUDE_CONFIG_DIR` when that is set, else
    /// in the home; an old install's `.config.json` in Claude's folder wins when it is there
    /// ([`claude_config::config_file`]).
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
        let claude_json = claude_config::config_file(&home, claude_config.as_deref());
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
    /// A Claude Code plugin's servers ([`Server::plugin`]).
    ClaudePlugin,
    /// A Codex plugin's servers ([`Server::plugin`]).
    CodexPlugin,
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
            Self::ClaudePlugin => "Claude Code (plugin)",
            Self::CodexPlugin => "Codex (plugin)",
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
            Self::ClaudePlugin => "claude:plugin",
            Self::CodexPlugin => "codex:plugin",
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
    /// which each agent does by itself, with its own `mcp login`.
    pub signs_in: bool,
    /// The plugin it comes with ([`Source::ClaudePlugin`], [`Source::CodexPlugin`]).
    pub plugin: Option<Plugin>,
    /// The file it is configured in (a plugin's `.mcp.json` or manifest).
    pub file: PathBuf,
    /// The OAuth client its config names (Codex's `[mcp_servers.<name>.oauth]`, Claude's
    /// `oauth.clientId`), which renewing its sign-in needs: no CLI stores a configured secret
    /// with the token.
    pub oauth: Option<OAuthClient>,
    /// Its config switches it off (Claude's `disabled: true`): kept in
    /// [`SwitchedOff::servers`], never loaded.
    pub disabled: bool,
    /// Its settings as the config writes them, before any variable is expanded: all that may
    /// be shown or copied, so no expanded value leaves the config.
    pub written: AsWritten,
    /// Some value of it comes from the environment (`${VAR}`, Codex's `env_vars`,
    /// `env_http_headers`, `bearer_token_env_var`).
    pub templated: bool,
    /// A project's Codex entry that only switches the user's server of its name (no command or
    /// url of its own): the user's server, switched in that project.
    pub overrides_user: bool,
}

/// A server's command, arguments, address and working folder as its config writes them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AsWritten {
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub cwd: Option<String>,
}

impl Server {
    /// The name its source's agent knows it by: Claude's `plugin_<plugin>_<name>` for one of
    /// its plugins' (what its tools are prefixed with after `mcp__`), else its own.
    pub fn native_name(&self) -> String {
        match &self.plugin {
            Some(plugin) => plugin.server_name(&self.name),
            None => self.name.clone(),
        }
    }
}

/// A pre-registered OAuth client of a server's config.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OAuthClient {
    pub client_id: Option<String>,
    pub client_secret: Option<Secret>,
}

/// A server, or a whole config file, that couldn't be read: no session gets it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The server's id, or for a file that couldn't be parsed, its source and path.
    pub id: String,
    /// The server's name, or the file's.
    pub name: String,
    pub source: Source,
    pub project: Option<PathBuf>,
    /// The config file.
    pub file: PathBuf,
    /// Its 1-based line in [`Self::file`], when known.
    pub line: Option<usize>,
    /// What was read of it (an address or command), if anything.
    pub read: Option<String>,
    /// Why, never quoting the config's text (it may hold a secret).
    pub error: String,
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
    /// The servers and config files that couldn't be read.
    pub problems: Vec<Problem>,
    pub skills: Vec<Skill>,
    /// The user's own Codex `skills.config` entries (as JSON), which a session that switches
    /// skills off must keep: Codex's `skills.config` override replaces the whole list.
    pub codex_skill_config: Vec<serde_json::Value>,
    /// The projects the user trusts in Codex, whose `.codex` layers Codex loads.
    pub codex_trusted: Vec<PathBuf>,
    /// The plugins of Claude Code and Codex that are on: the user's and, read for a project,
    /// the project's.
    pub plugins: Vec<Plugin>,
    /// The user's Codex servers the read project's own `.codex/config.toml` switches off
    /// (`[mcp_servers.<name>] enabled = false`), when Codex trusts the project.
    pub codex_off_here: Vec<String>,
    /// What the user's own setup has but switches off for every project.
    pub off: SwitchedOff,
    /// The servers Claude Code switches off in the read project (`disabledMcpServers` of its
    /// entry in the config file, what `/mcp` writes), by the name Claude Code lists them under.
    pub claude_off_here: Vec<String>,
    /// Why the read project's entry in Claude Code's config file couldn't be read: its off
    /// flags are unknown then, never taken as none.
    pub claude_off_error: Option<String>,
    /// The approval of each of the read project's team servers (`.mcp.json`), by name.
    pub claude_approval: Vec<(String, claude_config::Approval)>,
}

/// Items an agent has but switches off for every project: none of a catalog's other lists hold
/// them, as no session loads them, yet they can be switched on again.
#[derive(Debug, Clone, Default)]
pub struct SwitchedOff {
    /// Codex's own servers set `enabled = false`, and the servers of the plugins below.
    pub servers: Vec<Server>,
    /// The skills of the plugins below.
    pub skills: Vec<Skill>,
    /// Installed plugins switched off in the user's scope.
    pub plugins: Vec<Plugin>,
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

    /// The user's catalog `self` with `project`'s entries: [`Self::read`] for `project`, the
    /// user's part not read again.
    pub fn for_project(&self, roots: &Roots, project: &Path) -> Self {
        let mut catalog = self.clone();
        catalog.add_project(roots, project);
        catalog
    }

    /// The plugins [`Self::read`] lists (`plugins`), read without anything else: what loads a
    /// plugin is a few settings files, while a whole catalog reads every skill too.
    pub fn plugins(roots: &Roots, project: Option<&Path>) -> Vec<Plugin> {
        let codex = servers::codex_config(roots, &mut Vec::new());
        let mut plugins = plugins::claude(roots, None);
        plugins.extend(plugins::codex(roots, codex.as_ref()));
        let Some(project) = project else {
            return plugins;
        };
        // As `add_project`: the project's settings switch Claude's, its installs count there.
        let here = plugins::claude(roots, Some(project));
        plugins.retain(|plugin| {
            plugin.agent != Agent::Claude
                || here
                    .iter()
                    .any(|other| other.id == plugin.id && other.root == plugin.root)
        });
        plugins.extend(here.into_iter().filter(|plugin| plugin.project.is_some()));
        // As `add_codex_layer`.
        let trusted = codex
            .as_ref()
            .map(servers::codex_trusted)
            .unwrap_or_default();
        if !trusted.iter().any(|path| path == project) {
            return plugins;
        }
        let Some(layer) = servers::read_toml(
            &adapter::codex::project_file(project),
            Source::CodexProject,
            Some(project),
            &mut Vec::new(),
        ) else {
            return plugins;
        };
        for (plugin, on) in plugins::codex_switched(roots, Some(&layer), Some(project)) {
            let known = plugins
                .iter()
                .position(|known| known.agent == Agent::Codex && known.id == plugin.id);
            match (known, on) {
                (Some(at), false) => {
                    plugins.remove(at);
                }
                (None, true) => plugins.push(plugin),
                _ => {}
            }
        }
        plugins
    }

    fn user(roots: &Roots) -> Self {
        let mut problems = Vec::new();
        let codex = servers::codex_config(roots, &mut problems);
        let (servers, disabled): (Vec<Server>, Vec<Server>) =
            servers::user(roots, codex.as_ref(), &mut problems)
                .into_iter()
                .partition(|server| !server.disabled);
        let mut catalog = Self {
            servers,
            problems,
            skills: Vec::new(),
            codex_skill_config: codex
                .as_ref()
                .map(servers::codex_skill_config)
                .unwrap_or_default(),
            codex_trusted: codex
                .as_ref()
                .map(servers::codex_trusted)
                .unwrap_or_default(),
            plugins: Vec::new(),
            codex_off_here: Vec::new(),
            off: SwitchedOff::default(),
            claude_off_here: Vec::new(),
            claude_off_error: None,
            claude_approval: Vec::new(),
        };
        skills::add_user(&mut catalog.skills, roots);
        let mut plugins = plugins::claude(roots, None);
        plugins.extend(plugins::codex(roots, codex.as_ref()));
        for plugin in plugins {
            catalog.add_plugin(roots, plugin, codex.as_ref());
        }
        catalog.off = Self::switched_off(roots, codex.as_ref());
        catalog.off.servers.extend(disabled);
        catalog
    }

    fn switched_off(roots: &Roots, codex: Option<&toml::Table>) -> SwitchedOff {
        let mut off = SwitchedOff::default();
        // Its problems are the page's once it is on.
        let mut problems = Vec::new();
        if let Some(codex) = codex {
            off.servers = servers::toml_servers_off(codex, roots);
        }
        let mut plugins = plugins::claude_off(roots);
        plugins.extend(
            plugins::codex_switched(roots, codex, None)
                .into_iter()
                .filter_map(|(plugin, on)| (!on).then_some(plugin)),
        );
        for plugin in plugins {
            skills::add_plugin(&mut off.skills, &plugin);
            off.servers
                .extend(servers::plugin(roots, &plugin, codex, &mut problems));
            off.plugins.push(plugin);
        }
        off
    }

    fn add_project(&mut self, roots: &Roots, project: &Path) {
        for server in servers::project(roots, project, &mut self.problems) {
            if server.disabled {
                self.off.servers.push(server);
            } else {
                self.servers.push(server);
            }
        }
        skills::add_project(&mut self.skills, project);
        // A project's settings switch Claude's plugins on and off for its sessions, and its
        // installs count there.
        let here = plugins::claude(roots, Some(project));
        let gone: Vec<Plugin> = self
            .plugins
            .iter()
            .filter(|plugin| {
                plugin.agent == Agent::Claude
                    && !here
                        .iter()
                        .any(|other| other.id == plugin.id && other.root == plugin.root)
            })
            .cloned()
            .collect();
        for plugin in &gone {
            skills::remove_plugin(&mut self.skills, plugin);
            self.servers
                .retain(|server| server.plugin.as_ref() != Some(plugin));
            self.problems
                .retain(|problem| !problem.id.starts_with(&format!("{}:", plugin.prefix())));
            self.plugins.retain(|known| known != plugin);
        }
        for plugin in here {
            if plugin.project.is_some() {
                self.add_plugin(roots, plugin, None);
            }
        }
        self.add_codex_layer(roots, project);
        self.add_claude_entry(roots, project);
    }

    /// What the project's entry in Claude Code's config file switches: its servers off there,
    /// its team servers approved or not.
    fn add_claude_entry(&mut self, roots: &Roots, project: &Path) {
        let settings = match claude_config::project_settings(&roots.claude_json, project) {
            Ok(settings) => settings,
            Err(err) => {
                self.claude_off_error = Some(err);
                return;
            }
        };
        let mut team: Vec<&str> = self
            .servers
            .iter()
            .chain(&self.off.servers)
            .filter(|server| {
                server.source == Source::ClaudeProject && server.project.as_deref() == Some(project)
            })
            .map(|server| server.name.as_str())
            .collect();
        team.sort_unstable();
        team.dedup();
        self.claude_approval = team
            .into_iter()
            .map(|name| {
                (
                    name.to_owned(),
                    claude_config::team_approval(&settings, &roots.claude_dir, project, name),
                )
            })
            .collect();
        self.claude_off_here = settings.off;
    }

    /// The approval of the read project's team server `name`; `None` for another.
    pub fn claude_approval(&self, name: &str) -> Option<claude_config::Approval> {
        self.claude_approval
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, approval)| *approval)
    }

    /// What a trusted project's own `.codex/config.toml` switches: the user's plugins and
    /// servers off there, plugins on only there.
    fn add_codex_layer(&mut self, roots: &Roots, project: &Path) {
        if !self.codex_trusted.iter().any(|path| path == project) {
            return;
        }
        let file = adapter::codex::project_file(project);
        let Some(layer) = servers::read_toml(
            &file,
            Source::CodexProject,
            Some(project),
            &mut self.problems,
        ) else {
            return;
        };
        self.codex_off_here = layer
            .get("mcp_servers")
            .and_then(toml::Value::as_table)
            .into_iter()
            .flatten()
            .filter(|(_, server)| {
                server.get("enabled").and_then(toml::Value::as_bool) == Some(false)
            })
            .map(|(name, _)| name.clone())
            .collect();
        for (plugin, on) in plugins::codex_switched(roots, Some(&layer), Some(project)) {
            let known = self
                .plugins
                .iter()
                .find(|known| known.agent == Agent::Codex && known.id == plugin.id)
                .cloned();
            match (known, on) {
                (Some(known), false) => {
                    skills::remove_plugin(&mut self.skills, &known);
                    self.servers
                        .retain(|server| server.plugin.as_ref() != Some(&known));
                    self.plugins.retain(|other| *other != known);
                }
                (None, true) => {
                    let user = servers::codex_config(roots, &mut Vec::new());
                    self.add_plugin(roots, plugin, user.as_ref());
                }
                _ => {}
            }
        }
    }

    fn add_plugin(&mut self, roots: &Roots, plugin: Plugin, codex: Option<&toml::Table>) {
        skills::add_plugin(&mut self.skills, &plugin);
        self.servers
            .extend(servers::plugin(roots, &plugin, codex, &mut self.problems));
        self.plugins.push(plugin);
    }

    /// Whether `session`'s agent has `server` by itself: it loads it, and no entry of the same
    /// name that it loads too overrides it (Claude: local over project over user; Codex: a
    /// project's layer over the user's config).
    pub fn server_native(&self, server: &Server, session: &Session<'_>) -> bool {
        self.loads(server, session)
            && (server.source == Source::ClaudePlugin
                || !self.servers.iter().any(|other| {
                    other.name == server.name
                        && other.source != Source::ClaudePlugin
                        && precedence(other.source) > precedence(server.source)
                        && self.loads(other, session)
                }))
    }

    /// Whether `session`'s agent loads `server`'s entry, overridden or not.
    fn loads(&self, server: &Server, session: &Session<'_>) -> bool {
        let project = server.project.as_deref();
        // Switched off in the session's project (`/mcp`), under the name Claude Code lists.
        let claude_off = |name: &str| {
            session.project.is_some() && self.claude_off_here.iter().any(|off| off == name)
        };
        match (server.source, session.agent) {
            (Source::ClaudeUser, Agent::Claude) => !claude_off(&server.name),
            (Source::CodexUser, Agent::Codex) => {
                !(session.project.is_some() && self.codex_off_here.contains(&server.name))
            }
            // Claude keys its local scope on the repository's main folder: a session in a
            // subfolder or a linked worktree of it loads it too.
            (Source::ClaudeLocal, Agent::Claude) => {
                !claude_off(&server.name)
                    && project.is_some_and(|project| {
                        claude_config::project_key(project)
                            == claude_config::project_key(session.cwd)
                    })
            }
            // Team approval is separate from /mcp off flags: never bypass the approval gate.
            (Source::ClaudeProject, Agent::Claude) => {
                project == session.project
                    && !claude_off(&server.name)
                    && self.claude_approval(&server.name) == Some(claude_config::Approval::Approved)
            }
            // Codex loads a project's layers only where the user trusts it in Codex.
            (Source::CodexProject, Agent::Codex) => {
                project == session.project
                    && project
                        .is_some_and(|project| self.codex_trusted.iter().any(|p| p == project))
            }
            (Source::ClaudePlugin, Agent::Claude) => server.plugin.as_ref().is_some_and(|plugin| {
                plugin.counts_in(session.project)
                    && !claude_off(&format!("plugin:{}:{}", plugin.name, server.name))
            }),
            (Source::CodexPlugin, Agent::Codex) => server
                .plugin
                .as_ref()
                .is_some_and(|plugin| plugin.counts_in(session.project)),
            _ => false,
        }
    }

    /// Whether `session`'s agent finds `skill` by itself.
    pub fn skill_native(&self, skill: &Skill, session: &Session<'_>) -> bool {
        skill.roots.iter().any(|root| root.native(session))
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
        self.find_among(entry, |_| true)
    }

    /// As [`Self::find`], a name counting only the servers `keep` keeps (an id names any).
    pub fn find_among(
        &self,
        entry: &str,
        keep: impl Fn(&Server) -> bool,
    ) -> Result<&Server, FindError> {
        if let Some(server) = self.server(entry) {
            return Ok(server);
        }
        let named: Vec<&Server> = self
            .servers
            .iter()
            .filter(|server| server.name == entry && keep(server))
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

/// Which of an agent's own entries of one name wins: the higher.
fn precedence(source: Source) -> u8 {
    match source {
        Source::ClaudeLocal => 3,
        Source::ClaudeProject | Source::CodexProject => 2,
        Source::ClaudeUser | Source::CodexUser => 1,
        Source::Desktop
        | Source::CursorUser
        | Source::CursorProject
        | Source::ClaudePlugin
        | Source::CodexPlugin => 0,
    }
}

/// A server's id: its source, its project for a project's, and its name.
fn server_id(source: Source, project: Option<&Path>, name: &str) -> String {
    match project {
        Some(project) => format!("{}:{}:{name}", source.prefix(), project.display()),
        None => format!("{}:{name}", source.prefix()),
    }
}

/// Told when the watcher sees a config or skill change.
type Listener = Box<dyn Fn() + Send + Sync>;

/// The user's catalog, kept current by a watcher, and a project's read when asked.
pub struct Registry {
    roots: Roots,
    user: RwLock<Arc<Catalog>>,
    watcher: Mutex<Option<watch::Watcher>>,
    /// The projects whose own configs and skills the watcher looks at too.
    projects: Mutex<Vec<PathBuf>>,
    listeners: Mutex<Vec<Listener>>,
}

impl Registry {
    /// Reads the user's configs once; [`Self::watch`] keeps them current.
    pub fn new(roots: Roots) -> Arc<Self> {
        let user = Catalog::user(&roots);
        Arc::new(Self {
            roots,
            user: RwLock::new(Arc::new(user)),
            watcher: Mutex::new(None),
            projects: Mutex::new(Vec::new()),
            listeners: Mutex::new(Vec::new()),
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

    /// Adds to `catalog` the servers and skills of `project`'s own config whose ids
    /// `keep_server` and `keep_skill` take, as found there: items of another project the user
    /// uses in every project. Its unreadable entries are left out.
    pub fn adopt(
        &self,
        catalog: &mut Catalog,
        project: &Path,
        keep_server: impl Fn(&str) -> bool,
        keep_skill: impl Fn(&str) -> bool,
    ) {
        let servers = servers::project(&self.roots, project, &mut Vec::new());
        for server in servers {
            if !server.disabled && keep_server(&server.id) && catalog.server(&server.id).is_none() {
                catalog.servers.push(server);
            }
        }
        let mut skills = Vec::new();
        skills::add_project(&mut skills, project);
        for skill in skills {
            if keep_skill(&skill.id) && !catalog.skills.iter().any(|known| known.id == skill.id) {
                skills::merge(&mut catalog.skills, skill);
            }
        }
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
            // A folder made since (a first skill root) may have filled before it was watched.
            if watcher.rewatch(&self.watched()) {
                let user = Arc::new(Catalog::user(&self.roots));
                *self.user.write().unwrap_or_else(|p| p.into_inner()) = user;
            }
        }
    }

    /// Calls `listener` after each change the watcher sees (the user's catalog read again
    /// first), from the watcher's thread.
    pub fn on_change(&self, listener: impl Fn() + Send + Sync + 'static) {
        self.listeners
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(Box::new(listener));
    }

    /// Watches `projects`' own configs and skill folders too (their `.mcp.json`,
    /// `.codex/config.toml`, Claude's settings and `.claude/skills`, `.agents/skills`).
    pub fn watch_projects(&self, projects: &[PathBuf]) {
        {
            let mut known = self.projects.lock().unwrap_or_else(|p| p.into_inner());
            if known.as_slice() == projects {
                return;
            }
            *known = projects.to_vec();
        }
        if let Some(watcher) = self
            .watcher
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_mut()
        {
            watcher.rewatch(&self.watched());
        }
    }

    /// What the watcher does on a change: reads the user's configs again, then tells each
    /// listener.
    pub(crate) fn changed(&self) {
        self.refresh();
        for listener in self
            .listeners
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
        {
            listener();
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
                    registry.changed();
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
            roots
                .claude_dir
                .join("plugins")
                .join("installed_plugins.json"),
            roots.claude_dir.join("settings.json"),
        ];
        files.extend(roots.desktop_config.clone());
        let mut trees = vec![
            roots.claude_dir.join("skills"),
            roots.agents_skills.clone(),
            roots.codex_home.join("skills"),
        ];
        // A plugin's manifest, servers and skills folder; a Codex plugin's versions, of which
        // the newest is read.
        let mut dirs = Vec::new();
        for plugin in &user.plugins {
            for folder in [".claude-plugin", ".codex-plugin"] {
                files.push(plugin.root.join(folder).join("plugin.json"));
            }
            files.push(plugin.root.join(".mcp.json"));
            trees.push(plugin.root.join("skills"));
            if plugin.agent == Agent::Codex {
                dirs.extend(plugin.root.parent().map(Path::to_owned));
            }
        }
        files.extend(
            user.servers
                .iter()
                .filter(|server| server.plugin.is_some())
                .map(|server| server.file.clone()),
        );
        for project in self
            .projects
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
        {
            files.push(project.join(".mcp.json"));
            files.push(adapter::codex::project_file(project));
            files.push(project.join(".claude").join("settings.json"));
            files.push(project.join(".claude").join("settings.local.json"));
            trees.push(project.join(".claude").join("skills"));
            trees.push(project.join(".agents").join("skills"));
        }
        let mut project_skills = Vec::new();
        for project in self
            .projects
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
        {
            skills::add_project(&mut project_skills, project);
        }
        // A skill reached through a symlink: its own folder, wherever it is.
        for skill in user.skills.iter().chain(&project_skills) {
            if !trees.iter().any(|tree| skill.dir.starts_with(tree)) {
                trees.push(skill.dir.clone());
            }
        }
        watch::Watched { files, trees, dirs }
    }
}

#[cfg(test)]
mod tests;
