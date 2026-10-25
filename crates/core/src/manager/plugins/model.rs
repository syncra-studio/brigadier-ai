//! What Settings › Plugins and the sharing pass read: every item the agents have, as rows, with
//! each agent's own entries of it (its originals) kept apart from Brigadier's Library copy.
//!
//! An item is one MCP server or skill by name, or one plugin of a CLI's own marketplace. Its
//! originals are the agents' own entries (their configs, their skill folders), switched on or
//! off; Library plugins never count as originals.

use std::path::{Component, Path, PathBuf};

use brigadier_registry::adapter::{adapters, owner};
use brigadier_registry::library::{Entry, MARKETPLACE};
use brigadier_registry::{Agent, Catalog, Server, Session, Skill, SkillRoot, Source, Transport};

use super::share::Sharing;
use super::{PluginElsewhereView, PluginKind};

/// One project's catalog, as its sessions get it.
pub(crate) struct ProjectCatalog {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub catalog: Catalog,
}

/// A session of `agent` in `project` (or none: what every project gets).
pub(crate) fn session(agent: Agent, project: Option<&Path>) -> Session<'_> {
    Session {
        agent,
        cwd: project.unwrap_or(Path::new("")),
        project,
        in_checkout: true,
    }
}

/// What one catalog entry is part of.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Key {
    pub kind: KeyKind,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum KeyKind {
    /// A Library plugin, by its name.
    Library,
    Server,
    Skill,
    /// Another marketplace's plugin, by its name (`<plugin>/<server>` for one of its servers).
    Plugin,
}

impl Key {
    pub fn text(&self) -> String {
        let prefix = match self.kind {
            KeyKind::Library => "lib",
            KeyKind::Server => "server",
            KeyKind::Skill => "skill",
            KeyKind::Plugin => "plugin",
        };
        format!("{prefix}:{}", self.name)
    }
}

/// The Library plugin `id` names (`<name>@brigadier`), if it is one.
pub(crate) fn library_plugin(id: &str) -> Option<&str> {
    id.strip_suffix(&format!("@{MARKETPLACE}"))
}

/// The item `server` belongs to; `None` for another app's.
fn server_key(server: &Server) -> Option<Key> {
    if let Some(plugin) = &server.plugin {
        return Some(match library_plugin(&plugin.id) {
            Some(name) => Key {
                kind: KeyKind::Library,
                name: name.to_owned(),
            },
            None => Key {
                kind: KeyKind::Plugin,
                name: format!("{}/{}", plugin.name, server.name),
            },
        });
    }
    match server.source {
        Source::Desktop | Source::CursorUser | Source::CursorProject => None,
        _ => Some(Key {
            kind: KeyKind::Server,
            name: server.name.clone(),
        }),
    }
}

/// The item `skill` belongs to, after its provider-supplied entries have been removed.
fn skill_key(skill: &Skill) -> Key {
    match skill.plugin() {
        Some(plugin) => match library_plugin(&plugin.id) {
            Some(name) => Key {
                kind: KeyKind::Library,
                name: name.to_owned(),
            },
            // A plugin's skills are one item: its agent switches the plugin.
            None => Key {
                kind: KeyKind::Plugin,
                name: plugin.name.clone(),
            },
        },
        None => Key {
            kind: KeyKind::Skill,
            name: skill.name.clone(),
        },
    }
}

/// One of an agent's own entries of an item.
#[derive(Debug, Clone)]
pub(crate) enum Origin {
    Server { agent: Agent, server: Box<Server> },
    Skill { agent: Agent, skill: Skill },
}

impl Origin {
    pub fn agent(&self) -> Agent {
        match self {
            Self::Server { agent, .. } | Self::Skill { agent, .. } => *agent,
        }
    }

    /// The file (a server's config) or folder (a skill's) it lives in.
    pub fn file(&self) -> PathBuf {
        match self {
            Self::Server { server, .. } => server.file.clone(),
            Self::Skill { skill, .. } => skill.dir.clone(),
        }
    }

    /// Kept to the user's own scope (every project), rather than one project or folder.
    fn user_scope(&self) -> bool {
        match self {
            Self::Server { server, .. } => server.project.is_none(),
            Self::Skill { skill, .. } => skill.roots.iter().any(|root| {
                matches!(
                    root,
                    SkillRoot::ClaudeUser | SkillRoot::AgentsUser | SkillRoot::CodexUser
                )
            }),
        }
    }

    /// What it is, in a form two equal definitions share.
    fn definition(&self) -> String {
        match self {
            Self::Server { server, .. } => endpoint(server),
            Self::Skill { skill, .. } => skill.dir.display().to_string(),
        }
    }
}

/// What a server reaches, as written: its command, arguments and folder, or its address. Two
/// servers of one endpoint are the same server under two names.
pub(crate) fn endpoint(server: &Server) -> String {
    let written = &server.written;
    match &server.transport {
        Transport::Http { url, .. } => {
            format!("url {}", written.url.as_deref().unwrap_or(url))
        }
        Transport::Stdio { command, args, .. } => {
            let command = written.command.as_deref().unwrap_or(command);
            let args = if written.command.is_some() {
                &written.args
            } else {
                args
            };
            let folder = server
                .transport
                .working_dir()
                .map(|dir| dir.display().to_string())
                .unwrap_or_default();
            format!("run {command}\0{}\0{folder}", args.join("\0"))
        }
    }
}

/// Where an item is, for each agent.
#[derive(Debug, Clone, Default)]
pub(crate) struct Presence {
    pub kind: Option<PluginKind>,
    pub description: String,
    /// Its command, address or folder, as written.
    pub detail: String,
    /// A remote server's address, as written.
    pub url: Option<String>,
    /// Agents that have it in every project.
    pub everywhere: Vec<Agent>,
    /// (project or folder, agent) pairs that have it.
    pub here: Vec<(PathBuf, Agent)>,
    /// Agents that have it anywhere, on or off.
    pub anywhere: Vec<Agent>,
    pub files: Vec<String>,
    /// Each agent's server: its name, and its plugin's name when it comes with one.
    pub native: Vec<(Agent, String, Option<String>)>,
    /// The server to sign in to, for one that signs in, and the project whose catalog has it.
    pub server_id: Option<(String, Option<PathBuf>)>,
    /// The skills it holds, by name.
    pub skills: Vec<String>,
    pub problem: Option<String>,
    /// The agents' own entries of it.
    pub sources: Vec<Origin>,
    /// Why the switch and the scope can't change it.
    pub lock: Option<String>,
}

impl Presence {
    fn note(&mut self, agent: Agent, project: Option<&Path>) {
        match project {
            None => push(&mut self.everywhere, agent),
            Some(path) => {
                if !self.here.iter().any(|(p, a)| p == path && *a == agent) {
                    self.here.push((path.to_owned(), agent));
                }
            }
        }
        push(&mut self.anywhere, agent);
    }

    fn origin(&mut self, origin: Origin) {
        let same = |known: &Origin| match (known, &origin) {
            (
                Origin::Server {
                    agent: a,
                    server: x,
                },
                Origin::Server {
                    agent: b,
                    server: y,
                },
            ) => a == b && x.id == y.id,
            (Origin::Skill { agent: a, skill: x }, Origin::Skill { agent: b, skill: y }) => {
                a == b && x.dir == y.dir
            }
            _ => false,
        };
        if !self.sources.iter().any(same) {
            push(&mut self.anywhere, origin.agent());
            self.sources.push(origin);
        }
    }

    fn lock(&mut self, reason: Option<String>) {
        if self.lock.is_none() {
            self.lock = reason;
        }
    }

    /// The agents with an entry of their own.
    pub fn own_agents(&self) -> Vec<Agent> {
        let mut agents = Vec::new();
        for source in &self.sources {
            push(&mut agents, source.agent());
        }
        agents
    }
}

fn push<T: PartialEq>(list: &mut Vec<T>, item: T) {
    if !list.contains(&item) {
        list.push(item);
    }
}

/// A path as the user knows it: `~/…` under `home`.
pub(crate) fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if !home.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

/// `brigadier mcp run <id> -- <command> <args…>` (a Library copy that carries values) as the
/// command it runs.
fn unwrapped<'a>(command: &'a str, args: &'a [String]) -> (&'a str, &'a [String]) {
    let brigadier = Path::new(command)
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with("brigadier"));
    if brigadier
        && args.first().map(String::as_str) == Some("mcp")
        && args.get(1).map(String::as_str) == Some("run")
        && let Some(at) = args.iter().position(|arg| arg == "--")
        && let Some((inner, rest)) = args[at + 1..].split_first()
    {
        return (inner, rest);
    }
    (command, args)
}

/// A server's command line or address, as written (no variable expanded), and its address
/// when it is remote.
pub(crate) fn server_detail(server: &Server) -> (String, Option<String>) {
    let written = &server.written;
    match &server.transport {
        Transport::Http { url, .. } => {
            let url = written.url.clone().unwrap_or_else(|| url.clone());
            (url.clone(), Some(url))
        }
        Transport::Stdio { command, args, .. } => {
            let (command, args) = match &written.command {
                Some(command) => (command.as_str(), written.args.as_slice()),
                None => (command.as_str(), args.as_slice()),
            };
            let (command, args) = unwrapped(command, args);
            let mut text = command.to_owned();
            for arg in args {
                text.push(' ');
                text.push_str(arg);
            }
            (text, None)
        }
    }
}

/// Why the switch and the scope can't change an agent's own server.
fn server_lock(server: &Server, home: &Path) -> Option<String> {
    match server.source {
        Source::ClaudeUser | Source::ClaudeLocal => Some(
            "Claude Code keeps it on: it's in Claude Code's own settings, which Brigadier leaves as they are."
                .to_owned(),
        ),
        Source::ClaudeProject => Some(format!(
            "Shared with your team in {}: change it there.",
            tilde(&server.file, home)
        )),
        Source::CodexProject => Some(format!(
            "It's set up in {}: change it there.",
            tilde(&server.file, home)
        )),
        _ => None,
    }
}

/// The agent that finds skills in `root`.
fn root_agent(root: &SkillRoot) -> Agent {
    match root {
        SkillRoot::ClaudeUser | SkillRoot::ClaudeSynced(_) | SkillRoot::ClaudeProject(_) => {
            Agent::Claude
        }
        SkillRoot::AgentsUser | SkillRoot::CodexUser | SkillRoot::AgentsProject(_) => Agent::Codex,
        SkillRoot::Plugin(plugin) => plugin.agent,
    }
}

/// Why the switch and the scope can't change a skill found in `root` at `path`.
fn skill_lock(root: &SkillRoot, path: &Path, home: &Path) -> Option<String> {
    let folder = path
        .parent()
        .map(|dir| tilde(dir, home))
        .unwrap_or_default();
    let label = brigadier_registry::adapter::adapter(root_agent(root)).label();
    match root {
        SkillRoot::ClaudeUser | SkillRoot::AgentsUser | SkillRoot::CodexUser => Some(format!(
            "{label} loads every skill in {folder}: it can't be switched off there."
        )),
        SkillRoot::ClaudeSynced(_) => Some(
            "Claude Code syncs it from your Claude account: it can't be switched off here."
                .to_owned(),
        ),
        SkillRoot::ClaudeProject(_) | SkillRoot::AgentsProject(_) => Some(format!(
            "Shared with your team in {folder}: change it there."
        )),
        SkillRoot::Plugin(_) => None,
    }
}

/// Commands inside known provider app bundles are supplied by those apps. Match path
/// components, not server names, and don't mistake a path escaping the bundle for one in it.
pub(super) fn provider_app_command(command: &str) -> bool {
    let path = Path::new(command);
    if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return false;
    }
    let parts: Vec<_> = path.components().collect();
    let apps = match parts.as_slice() {
        [
            Component::RootDir,
            Component::Normal(applications),
            rest @ ..,
        ] if *applications == "Applications" => rest,
        [
            Component::RootDir,
            Component::Normal(users),
            Component::Normal(_),
            Component::Normal(applications),
            rest @ ..,
        ] if *users == "Users" && *applications == "Applications" => rest,
        _ => return false,
    };
    matches!(apps,
        [Component::Normal(app), Component::Normal(contents), _, ..]
            if (*app == "ChatGPT.app" || *app == "Claude.app") && *contents == "Contents")
}

fn provider_server(server: &Server) -> bool {
    server
        .plugin
        .as_ref()
        .is_some_and(|plugin| plugin.provider_supplied())
        || matches!(&server.transport, Transport::Stdio { command, .. } if provider_app_command(command))
}

/// `skill` in two parts: the entries that are the agents' own, and those in Library plugins
/// (Brigadier's copy), either empty when it has none.
fn split_library(skill: &Skill) -> (Option<Skill>, Option<Skill>) {
    let library = |root: &SkillRoot| matches!(root, SkillRoot::Plugin(plugin) if library_plugin(&plugin.id).is_some());
    let part = |keep: bool| {
        let entries: Vec<_> = skill
            .entries
            .iter()
            .filter(|(root, _)| library(root) == keep)
            .cloned()
            .collect();
        let (_, first) = entries.first()?;
        let mut part = skill.clone();
        part.roots.retain(|root| library(root) == keep);
        if skill.entries.first() != entries.first() {
            part.dir = std::fs::canonicalize(first).unwrap_or_else(|_| first.clone());
        }
        part.entries = entries;
        Some(part)
    };
    (part(false), part(true))
}

/// Reads `catalog` (for `project`, or the user's) into `items`.
fn gather(
    items: &mut Vec<(Key, Presence)>,
    elsewhere: &mut Vec<PluginElsewhereView>,
    catalog: &Catalog,
    project: Option<&Path>,
    home: &Path,
) {
    for server in &catalog.servers {
        if provider_server(server) {
            continue;
        }
        let Some(key) = server_key(server) else {
            if project.is_none()
                && !catalog.servers.iter().any(|other| {
                    !provider_server(other)
                        && other.name == server.name
                        && server_key(other).is_some()
                })
                && !elsewhere.iter().any(|known| known.name == server.name)
            {
                let (detail, url) = server_detail(server);
                elsewhere.push(PluginElsewhereView {
                    key: format!("other:{}", server.id),
                    name: server.name.clone(),
                    detail,
                    url,
                    found_in: server.source.label().to_owned(),
                    file: server.file.display().to_string(),
                });
            }
            continue;
        };
        let presence = row(items, key);
        presence.kind = Some(PluginKind::Server);
        (presence.detail, presence.url) = server_detail(server);
        push(&mut presence.files, server.file.display().to_string());
        if server.plugin.is_none()
            && let Some(agent) = owner(server.source)
        {
            presence.lock(server_lock(server, home));
            presence.origin(Origin::Server {
                agent,
                server: Box::new(server.clone()),
            });
        }
        for adapter in adapters() {
            let agent = adapter.agent();
            if catalog.server_native(server, &session(agent, project)) {
                presence.note(agent, project);
                push(
                    &mut presence.native,
                    (
                        agent,
                        server.name.clone(),
                        server.plugin.as_ref().map(|plugin| plugin.name.clone()),
                    ),
                );
                if server.signs_in && presence.server_id.is_none() {
                    presence.server_id = Some((server.id.clone(), project.map(Path::to_owned)));
                }
            }
        }
    }
    for problem in &catalog.problems {
        if problem.read.as_deref().is_some_and(provider_app_command)
            || catalog
                .plugins
                .iter()
                .chain(&catalog.off.plugins)
                .any(|plugin| {
                    plugin.provider_supplied()
                        && problem.source == Source::CodexPlugin
                        && problem.file.starts_with(&plugin.root)
                })
        {
            continue;
        }
        let presence = row(
            items,
            Key {
                kind: KeyKind::Server,
                name: problem.name.clone(),
            },
        );
        presence.kind = Some(PluginKind::Server);
        presence.problem = Some(problem.error.clone());
        push(&mut presence.files, problem.file.display().to_string());
        if let Some(read) = &problem.read
            && presence.detail.is_empty()
        {
            presence.detail = read.clone();
        }
    }
    for skill in &catalog.skills {
        let Some(skill) = skill.user_installed() else {
            continue;
        };
        let (own, copy) = split_library(&skill);
        for part in [own, copy].into_iter().flatten() {
            let presence = skill_row(items, &part);
            if part.plugin().is_none() {
                for (root, path) in &part.entries {
                    presence.lock(skill_lock(root, path, home));
                }
                let mut agents = Vec::new();
                for root in &part.roots {
                    push(&mut agents, root_agent(root));
                }
                for agent in agents {
                    presence.origin(Origin::Skill {
                        agent,
                        skill: part.clone(),
                    });
                }
            }
            for adapter in adapters() {
                let agent = adapter.agent();
                if catalog.skill_native(&part, &session(agent, project)) {
                    presence.note(agent, project);
                }
            }
        }
    }
    // What an agent has but switches off (a Codex server set `enabled = false`, a Claude one
    // set `disabled`, a switched-off plugin's): still its own, and an off row it can be
    // switched on from again.
    for server in &catalog.off.servers {
        if provider_server(server) {
            continue;
        }
        let (Some(key), Some(agent)) = (server_key(server), owner(server.source)) else {
            continue;
        };
        let presence = row(items, key);
        presence.kind = Some(PluginKind::Server);
        if presence.detail.is_empty() {
            (presence.detail, presence.url) = server_detail(server);
        }
        push(&mut presence.files, server.file.display().to_string());
        push(&mut presence.anywhere, agent);
        if server.plugin.is_none() {
            presence.lock(server_lock(server, home));
            presence.origin(Origin::Server {
                agent,
                server: Box::new(server.clone()),
            });
        }
    }
    if project.is_some() {
        return;
    }
    for skill in &catalog.off.skills {
        let Some(skill) = skill.user_installed() else {
            continue;
        };
        if let Some(plugin) = skill.plugin() {
            let presence = skill_row(items, &skill);
            push(&mut presence.anywhere, plugin.agent);
        }
    }
}

/// The row `key` names in `items`, added when new.
fn row(items: &mut Vec<(Key, Presence)>, key: Key) -> &mut Presence {
    let at = match items.iter().position(|(known, _)| *known == key) {
        Some(at) => at,
        None => {
            items.push((key, Presence::default()));
            items.len() - 1
        }
    };
    &mut items[at].1
}

/// The row `skill` is part of, with what the skill tells of it.
fn skill_row<'a>(items: &'a mut Vec<(Key, Presence)>, skill: &Skill) -> &'a mut Presence {
    let key = skill_key(skill);
    let library = key.kind == KeyKind::Library;
    let plugin = (key.kind == KeyKind::Plugin)
        .then(|| skill.plugin())
        .flatten();
    let presence = row(items, key);
    if presence.kind.is_none() || !library {
        presence.kind = Some(PluginKind::Skill);
    }
    push(&mut presence.skills, skill.name.clone());
    if presence.description.is_empty() {
        presence.description = plugin
            .and_then(|plugin| plugin.description())
            .unwrap_or_else(|| skill.description.clone());
    }
    if presence.detail.is_empty() {
        presence.detail = plugin
            .map_or(&skill.dir, |plugin| &plugin.root)
            .display()
            .to_string();
    }
    push(&mut presence.files, skill.skill_md().display().to_string());
    presence
}

/// One row of the page: an item, its agents' own entries and Brigadier's copy.
#[derive(Debug, Clone)]
pub(crate) struct Row {
    pub key: Key,
    pub kind: PluginKind,
    /// The agents' own entries (for a Library item no agent has by itself: none).
    pub own: Presence,
    /// The Library plugin that is its copy, or the item itself for a Library item.
    pub lib: Option<String>,
    /// Where the Library plugin is.
    pub copy: Presence,
    /// Agents with the same server under another name.
    pub equivalent: Vec<Agent>,
    /// Two different setups go by its name.
    pub conflict: bool,
}

impl Row {
    /// Agents that have it: their own entry, the same server by another name, or (for another
    /// marketplace's plugin) the plugin.
    pub fn holders(&self) -> Vec<Agent> {
        let mut agents = self.own.own_agents();
        for agent in &self.equivalent {
            push(&mut agents, *agent);
        }
        if self.key.kind == KeyKind::Plugin {
            for agent in &self.own.anywhere {
                push(&mut agents, *agent);
            }
        }
        agents
    }

    /// Agents that have its Library copy, installed or on somewhere.
    pub fn copy_agents(&self) -> &[Agent] {
        &self.copy.anywhere
    }

    /// Agents with it on in every project, own entry or copy.
    pub fn everywhere(&self) -> Vec<Agent> {
        let mut agents = self.own.everywhere.clone();
        for agent in &self.copy.everywhere {
            push(&mut agents, *agent);
        }
        agents
    }

    /// (project or folder, agent) pairs with it on, own entry or copy.
    pub fn here(&self) -> Vec<(PathBuf, Agent)> {
        let mut here = self.own.here.clone();
        for pair in &self.copy.here {
            push(&mut here, pair.clone());
        }
        here
    }

    pub fn on(&self) -> bool {
        !self.own.everywhere.is_empty()
            || !self.own.here.is_empty()
            || !self.copy.everywhere.is_empty()
            || !self.copy.here.is_empty()
    }

    /// The endpoint of its first own server.
    pub fn endpoint(&self) -> Option<String> {
        self.own.sources.iter().find_map(|source| match source {
            Origin::Server { server, .. } => Some(endpoint(server)),
            Origin::Skill { .. } => None,
        })
    }

    /// Which own entry the copies are made from: the first agent's in the page's order, its
    /// user scope before a project's, on before off.
    pub fn original(&self) -> Option<&Origin> {
        let order = |agent: Agent| {
            adapters()
                .iter()
                .position(|adapter| adapter.agent() == agent)
                .unwrap_or(usize::MAX)
        };
        self.own.sources.iter().min_by_key(|source| {
            let off = matches!(source, Origin::Server { server, .. } if server.disabled);
            (order(source.agent()), !source.user_scope(), off)
        })
    }
}

/// Every row, and other apps' servers.
pub(crate) struct Model {
    pub rows: Vec<Row>,
    pub elsewhere: Vec<PluginElsewhereView>,
}

impl Model {
    /// The row page key `key` names: its own key, or `lib:<name>` of its Library copy.
    pub fn find(&self, key: &str) -> Option<&Row> {
        self.rows
            .iter()
            .find(|row| row.key.text() == key)
            .or_else(|| {
                let name = key.strip_prefix("lib:")?;
                self.rows
                    .iter()
                    .find(|row| row.lib.as_deref() == Some(name))
            })
    }
}

/// The rows of the user's catalog and each project's, with the Library and what sharing
/// remembers.
pub(crate) fn model(
    user: &Catalog,
    projects: &[ProjectCatalog],
    library: &[Entry],
    sharing: &Sharing,
    home: &Path,
) -> Model {
    let mut items: Vec<(Key, Presence)> = Vec::new();
    let mut elsewhere = Vec::new();
    gather(&mut items, &mut elsewhere, user, None, home);
    for project in projects {
        gather(
            &mut items,
            &mut elsewhere,
            &project.catalog,
            Some(&project.path),
            home,
        );
    }
    // An agent's own server kept to a folder that isn't one of the user's projects (Claude's
    // local scope): on there, though no project's sessions get it.
    for server in &user.servers {
        let Some(folder) = &server.project else {
            continue;
        };
        if server.plugin.is_some()
            || provider_server(server)
            || projects.iter().any(|project| project.path == *folder)
        {
            continue;
        }
        let (Some(key), Some(agent)) = (server_key(server), owner(server.source)) else {
            continue;
        };
        let presence = row(&mut items, key);
        presence.note(agent, Some(folder));
        push(&mut presence.native, (agent, server.name.clone(), None));
    }
    // A Library item none of the agents has yet.
    for entry in library {
        let key = Key {
            kind: KeyKind::Library,
            name: entry.name.clone(),
        };
        let presence = row(&mut items, key);
        if presence.kind.is_none() {
            presence.kind = Some(if entry.dir.join("skills").is_dir() {
                PluginKind::Skill
            } else {
                PluginKind::Server
            });
        }
        if presence.description.is_empty() {
            presence.description.clone_from(&entry.description);
        }
    }
    let (libraries, others): (Vec<_>, Vec<_>) = items
        .into_iter()
        .filter(|(_, presence)| presence.kind.is_some())
        .partition(|(key, _)| key.kind == KeyKind::Library);
    let mut rows: Vec<Row> = others
        .into_iter()
        .map(|(key, own)| Row {
            kind: own.kind.unwrap_or(PluginKind::Server),
            key,
            own,
            lib: None,
            copy: Presence::default(),
            equivalent: Vec::new(),
            conflict: false,
        })
        .collect();
    // Each Library plugin joins the item sharing made it from; any other stands alone.
    for (key, copy) in libraries {
        let kind = copy.kind.unwrap_or(PluginKind::Server);
        let joined = sharing.items.get(&key.name).and_then(|record| {
            rows.iter_mut().find(|row| {
                row.kind == record.kind
                    && row.key.name == record.name
                    && matches!(row.key.kind, KeyKind::Server | KeyKind::Skill)
            })
        });
        match joined {
            Some(row) => {
                row.lib = Some(key.name.clone());
                row.copy = copy;
            }
            None => rows.push(Row {
                lib: Some(key.name.clone()),
                kind,
                key,
                own: Presence {
                    kind: Some(kind),
                    description: copy.description.clone(),
                    detail: copy.detail.clone(),
                    url: copy.url.clone(),
                    files: copy.files.clone(),
                    skills: copy.skills.clone(),
                    ..Presence::default()
                },
                copy,
                equivalent: Vec::new(),
                conflict: false,
            }),
        }
    }
    // The same server under another name, per agent; two different setups of one name; an
    // unrecorded Library item of the same kind and name.
    let endpoints: Vec<(Agent, String, String)> = rows
        .iter()
        .flat_map(|row| row.own.sources.iter())
        .filter_map(|source| match source {
            Origin::Server { agent, server } => {
                Some((*agent, server.name.clone(), endpoint(server)))
            }
            Origin::Skill { .. } => None,
        })
        .collect();
    let standalone: Vec<(PluginKind, String)> = rows
        .iter()
        .filter(|row| row.key.kind == KeyKind::Library)
        .map(|row| (row.kind, row.key.name.clone()))
        .collect();
    for row in &mut rows {
        if let Some(mine) = row.endpoint() {
            for (agent, name, other) in &endpoints {
                if *other == mine && *name != row.key.name {
                    push(&mut row.equivalent, *agent);
                }
            }
        }
        let mut definitions: Vec<String> = Vec::new();
        for source in &row.own.sources {
            push(&mut definitions, source.definition());
        }
        row.conflict = definitions.len() > 1
            || (row.lib.is_none()
                && !row.own.sources.is_empty()
                && standalone.contains(&(row.kind, row.key.name.clone())));
    }
    Model { rows, elsewhere }
}
