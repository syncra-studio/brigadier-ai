//! Settings › Plugins: every skill and MCP server the agents have, as one list, with where
//! each is on (every project, only some, off in some) and how each agent stands with it.
//!
//! Everything here is read from the agents' own setup ([`Catalog`]), read for the user and
//! for each of the user's trusted projects, so the page shows what a plain `claude` or `codex`
//! started in that project gets. Items Brigadier adds live in the Library
//! ([`brigadier_registry::library`]) and reach each agent through its own plugin commands.

mod jobs;
pub mod ops;

use std::path::{Path, PathBuf};

use brigadier_providers::ProviderKind;
use brigadier_registry::adapter::{AgentAdapter, Listed, adapters};
use brigadier_registry::library::{Entry, Library, MARKETPLACE};
use brigadier_registry::{Agent, Catalog, Server, Session, Skill, SkillRoot, Source, Transport};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The page.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginsView {
    /// Every agent Brigadier drives, in the order its marks show.
    pub agents: Vec<PluginAgentView>,
    /// The projects the filter offers: the user's trusted ones.
    pub projects: Vec<PluginProjectView>,
    pub items: Vec<PluginItemView>,
    /// Servers only Claude Desktop or Cursor have: no agent uses them until brought in.
    pub elsewhere: Vec<PluginItemView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginAgentView {
    pub agent: ProviderKind,
    /// "Claude Code".
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginProjectView {
    pub id: String,
    pub name: String,
    /// Its folder.
    pub path: String,
    /// Agents that don't read this project's own settings, with why, in plain words: Codex
    /// before the user trusts the project.
    pub notes: Vec<PluginAgentNote>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginAgentNote {
    pub agent: ProviderKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginKind {
    Server,
    Skill,
}

/// One skill or MCP server, across agents and projects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginItemView {
    /// Stable: `lib:<name>` for a Library item, `server:<name>`, `skill:<name>`,
    /// `plugin:<plugin>/<name>` for another plugin's, `other:<id>` for another app's.
    pub key: String,
    pub kind: PluginKind,
    pub name: String,
    pub description: String,
    /// How it runs: its command, or its address.
    pub detail: String,
    /// Reached over the network (an address), not started as a process.
    pub remote: bool,
    /// It is in the Library: every agent can have it, everywhere or per project.
    pub library: bool,
    /// On in every project (for at least one agent).
    pub everywhere: bool,
    /// When not [`Self::everywhere`]: the projects (folders) it is on in.
    pub on_in: Vec<String>,
    /// When [`Self::everywhere`]: the projects (folders) it is off in.
    pub off_in: Vec<String>,
    /// Each agent's state, in [`PluginsView::agents`]' order, where the page looks: the picked
    /// project, or every project.
    pub agents: Vec<PluginAgentState>,
    /// What needs the user, if anything.
    pub attention: Option<PluginAttention>,
    /// Its switch works as it is; else it goes into the Library first (Use everywhere): one
    /// of the agents that has it can't keep it to a project by itself.
    pub switchable: bool,
    /// A newer version is waiting (a Library item from Git or skills.sh).
    pub update_available: bool,
    /// It came into the Library by Use everywhere, which can be undone.
    pub can_undo: bool,
    /// The server to sign in to (`Request::SignInMcpServer`'s id), for one that signs in.
    pub server_id: Option<String>,
    /// The files it is set up in.
    pub files: Vec<String>,
    /// For another app's: the app ("Claude Desktop").
    pub found_in: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginAgentState {
    pub agent: ProviderKind,
    pub state: AgentItemState,
}

/// How one agent stands with an item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AgentItemState {
    Ready,
    NeedsSignIn,
    WontStart {
        reason: String,
    },
    /// Switched off where the page looks.
    Off,
    /// The agent doesn't have it.
    Missing,
    /// The agent can't use it, with why.
    NotSupported {
        reason: String,
    },
    /// Being added, removed or switched.
    Working,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PluginAttention {
    /// An agent needs the user to sign in: "Codex needs you to sign in."
    SignIn { message: String },
    /// It won't start: "Won't start: unknown type "websocket"."
    WontStart { message: String },
    /// Some agents don't have it: "Only Claude Code has it."
    NotEverywhere { message: String },
}

/// One project's catalog, as its sessions get it.
pub(crate) struct ProjectCatalog {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub catalog: Catalog,
}

/// What each agent's own list says of a server, where known.
pub(crate) trait Health {
    /// `agent`'s state of server `server` (of plugin `plugin`, by its name) in `project`.
    fn state(
        &self,
        agent: Agent,
        server: &str,
        plugin: Option<&str>,
        project: Option<&Path>,
    ) -> Option<AgentItemState>;
}

/// No list read yet.
#[cfg(test)]
pub(crate) struct Unknown;

#[cfg(test)]
impl Health for Unknown {
    fn state(
        &self,
        _: Agent,
        _: &str,
        _: Option<&str>,
        _: Option<&Path>,
    ) -> Option<AgentItemState> {
        None
    }
}

/// Each agent's own list (`claude mcp list`, `codex mcp list --json`), as last read: for every
/// project (`None`) and the project the page looks at.
pub(crate) struct Lists(pub Vec<(Agent, Option<PathBuf>, Vec<Listed>)>);

impl Health for Lists {
    fn state(
        &self,
        agent: Agent,
        server: &str,
        plugin: Option<&str>,
        project: Option<&Path>,
    ) -> Option<AgentItemState> {
        let adapter = brigadier_registry::adapter::adapter(agent);
        let (_, _, listed) = self
            .0
            .iter()
            .find(|(a, p, _)| *a == agent && p.as_deref() == project)?;
        let entry = listed
            .iter()
            .find(|entry| adapter.is_server(&entry.name, server, plugin))?;
        Some(if entry.needs_sign_in {
            AgentItemState::NeedsSignIn
        } else if let Some(reason) = &entry.failed {
            AgentItemState::WontStart {
                reason: reason.clone(),
            }
        } else {
            AgentItemState::Ready
        })
    }
}

pub(crate) fn provider(agent: Agent) -> ProviderKind {
    match agent {
        Agent::Claude => ProviderKind::Claude,
        Agent::Codex => ProviderKind::Codex,
    }
}

/// A session of `agent` in `project` (or none: what every project gets).
fn session(agent: Agent, project: Option<&Path>) -> Session<'_> {
    Session {
        agent,
        cwd: project.unwrap_or(Path::new("")),
        project,
        in_checkout: true,
    }
}

/// What one catalog entry is part of.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    kind: KeyKind,
    name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum KeyKind {
    Library,
    Server,
    Skill,
    /// Another marketplace's plugin, by its name.
    Plugin,
}

impl Key {
    fn text(&self) -> String {
        let prefix = match self.kind {
            KeyKind::Library => "lib",
            KeyKind::Server => "server",
            KeyKind::Skill => "skill",
            KeyKind::Plugin => "plugin",
        };
        format!("{prefix}:{}", self.name)
    }
}

fn library_plugin(id: &str) -> Option<&str> {
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

/// The item `skill` belongs to; `None` for one synced from a Claude account (never touched).
fn skill_key(skill: &Skill) -> Option<Key> {
    if skill
        .roots
        .iter()
        .all(|root| matches!(root, SkillRoot::ClaudeSynced(_)))
    {
        return None;
    }
    Some(match skill.plugin() {
        Some(plugin) => match library_plugin(&plugin.id) {
            Some(name) => Key {
                kind: KeyKind::Library,
                name: name.to_owned(),
            },
            None => Key {
                kind: KeyKind::Plugin,
                name: format!("{}/{}", plugin.name, skill.name),
            },
        },
        None => Key {
            kind: KeyKind::Skill,
            name: skill.name.clone(),
        },
    })
}

/// Where an item is, for each agent.
#[derive(Debug, Default)]
struct Presence {
    kind: Option<PluginKind>,
    description: String,
    detail: String,
    remote: bool,
    /// Agents that have it in every project.
    everywhere: Vec<Agent>,
    /// (project, agent) pairs that have it.
    here: Vec<(PathBuf, Agent)>,
    /// Agents that have it anywhere.
    anywhere: Vec<Agent>,
    files: Vec<String>,
    /// Each agent's server: its name, and its plugin's name when it comes with one.
    native: Vec<(Agent, String, Option<String>)>,
    /// The server to sign in to, for one that signs in.
    server_id: Option<String>,
    /// Sources that hold it: whether every agent with it can switch it per project.
    switchable: bool,
    problem: Option<String>,
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
}

fn push<T: PartialEq>(list: &mut Vec<T>, item: T) {
    if !list.contains(&item) {
        list.push(item);
    }
}

fn detail(transport: &Transport) -> (String, bool) {
    match transport {
        Transport::Stdio { command, args, .. } => {
            let mut text = command.clone();
            for arg in args {
                text.push(' ');
                text.push_str(arg);
            }
            (text, false)
        }
        Transport::Http { url, .. } => (url.clone(), true),
    }
}

/// Whether the agent of a server from `source` can switch it per project by itself.
fn switchable_source(source: Source) -> bool {
    // A plugin of any marketplace is switched by its agent's own commands; Codex's own
    // servers by its project file (D8). Claude's own servers have no per-project off (D9).
    matches!(
        source,
        Source::ClaudePlugin | Source::CodexPlugin | Source::CodexUser | Source::CodexProject
    )
}

/// Reads `catalog` (for `project`, or the user's) into `items`.
fn gather(
    items: &mut Vec<(Key, Presence)>,
    elsewhere: &mut Vec<PluginItemView>,
    catalog: &Catalog,
    project: Option<&Path>,
    agents: &[&'static dyn AgentAdapter],
) {
    let entry = |key: Key, items: &mut Vec<(Key, Presence)>| -> usize {
        match items.iter().position(|(known, _)| *known == key) {
            Some(at) => at,
            None => {
                items.push((
                    key,
                    Presence {
                        switchable: true,
                        ..Presence::default()
                    },
                ));
                items.len() - 1
            }
        }
    };
    for server in &catalog.servers {
        let Some(key) = server_key(server) else {
            if project.is_none() {
                let (text, remote) = detail(&server.transport);
                if !catalog
                    .servers
                    .iter()
                    .any(|other| other.name == server.name && server_key(other).is_some())
                    && !elsewhere.iter().any(|known| known.name == server.name)
                {
                    elsewhere.push(PluginItemView {
                        key: format!("other:{}", server.id),
                        kind: PluginKind::Server,
                        name: server.name.clone(),
                        description: String::new(),
                        detail: text,
                        remote,
                        library: false,
                        everywhere: false,
                        on_in: Vec::new(),
                        off_in: Vec::new(),
                        agents: agents
                            .iter()
                            .map(|adapter| PluginAgentState {
                                agent: provider(adapter.agent()),
                                state: AgentItemState::Missing,
                            })
                            .collect(),
                        attention: None,
                        switchable: false,
                        update_available: false,
                        can_undo: false,
                        server_id: None,
                        files: vec![server.file.display().to_string()],
                        found_in: Some(server.source.label().to_owned()),
                    });
                }
            }
            continue;
        };
        let at = entry(key, items);
        let presence = &mut items[at].1;
        presence.kind = Some(PluginKind::Server);
        let (text, remote) = detail(&server.transport);
        presence.detail = text;
        presence.remote = remote;
        push(&mut presence.files, server.file.display().to_string());
        let owned_by_switchless = !switchable_source(server.source);
        for adapter in agents {
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
                    presence.server_id = Some(server.id.clone());
                }
                if owned_by_switchless {
                    presence.switchable = false;
                }
            }
        }
    }
    for problem in &catalog.problems {
        let key = Key {
            kind: KeyKind::Server,
            name: problem.name.clone(),
        };
        let at = entry(key, items);
        let presence = &mut items[at].1;
        presence.kind = Some(PluginKind::Server);
        presence.problem = Some(problem.error.clone());
        push(&mut presence.files, problem.file.display().to_string());
        if let Some(read) = &problem.read {
            presence.detail = read.clone();
        }
    }
    for skill in &catalog.skills {
        let Some(key) = skill_key(skill) else {
            continue;
        };
        let library = key.kind == KeyKind::Library;
        let at = entry(key, items);
        let presence = &mut items[at].1;
        if presence.kind.is_none() || !library {
            presence.kind = Some(PluginKind::Skill);
        }
        if presence.description.is_empty() {
            presence.description = skill.description.clone();
        }
        if presence.detail.is_empty() {
            presence.detail = skill.dir.display().to_string();
        }
        push(&mut presence.files, skill.skill_md().display().to_string());
        let plugin_only = skill.plugin().is_some();
        for adapter in agents {
            let agent = adapter.agent();
            if catalog.skill_native(skill, &session(agent, project)) {
                presence.note(agent, project);
                if !plugin_only {
                    presence.switchable = false;
                }
            }
        }
    }
}

/// The page, from the user's catalog and each trusted project's; `focus` is the project the
/// filter picks (its folder), else every project.
pub(crate) fn build(
    user: &Catalog,
    projects: &[ProjectCatalog],
    library: &[Entry],
    health: &dyn Health,
    focus: Option<&Path>,
) -> PluginsView {
    let agents = adapters();
    let mut items: Vec<(Key, Presence)> = Vec::new();
    let mut elsewhere = Vec::new();
    gather(&mut items, &mut elsewhere, user, None, &agents);
    for project in projects {
        gather(
            &mut items,
            &mut elsewhere,
            &project.catalog,
            Some(&project.path),
            &agents,
        );
    }
    // A Library item none of the agents has yet.
    for entry in library {
        let key = Key {
            kind: KeyKind::Library,
            name: entry.name.clone(),
        };
        if !items.iter().any(|(known, _)| *known == key) {
            items.push((
                key,
                Presence {
                    description: entry.description.clone(),
                    switchable: true,
                    kind: Some(if entry.dir.join("skills").is_dir() {
                        PluginKind::Skill
                    } else {
                        PluginKind::Server
                    }),
                    ..Presence::default()
                },
            ));
        }
    }
    items.sort_by(|(a, _), (b, _)| a.name.cmp(&b.name).then(a.kind.cmp(&b.kind)));
    let label = |agents: &[Agent]| -> String {
        let names: Vec<&str> = agents
            .iter()
            .map(|agent| brigadier_registry::adapter::adapter(*agent).label())
            .collect();
        match names.as_slice() {
            [] => String::new(),
            [one] => (*one).to_owned(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        }
    };
    let items = items
        .into_iter()
        .filter_map(|(key, presence)| {
            let kind = presence.kind?;
            let library = key.kind == KeyKind::Library;
            let everywhere = !presence.everywhere.is_empty();
            let in_project = |path: &Path| {
                presence
                    .here
                    .iter()
                    .filter(|(p, _)| p == path)
                    .map(|(_, a)| *a)
                    .collect::<Vec<Agent>>()
            };
            let on_in: Vec<String> = if everywhere {
                Vec::new()
            } else {
                projects
                    .iter()
                    .filter(|project| !in_project(&project.path).is_empty())
                    .map(|project| project.path.display().to_string())
                    .collect()
            };
            let off_in: Vec<String> = if everywhere {
                projects
                    .iter()
                    .filter(|project| {
                        let here = in_project(&project.path);
                        presence
                            .everywhere
                            .iter()
                            .all(|agent| !here.contains(agent))
                    })
                    .map(|project| project.path.display().to_string())
                    .collect()
            } else {
                Vec::new()
            };
            let has = |agent: Agent| match focus {
                None => presence.everywhere.contains(&agent),
                Some(path) => in_project(path).contains(&agent),
            };
            let states: Vec<(Agent, AgentItemState)> = agents
                .iter()
                .map(|adapter| {
                    let agent = adapter.agent();
                    let state = if has(agent) {
                        presence
                            .native
                            .iter()
                            .find(|(a, _, _)| *a == agent)
                            .and_then(|(_, name, plugin)| {
                                health.state(agent, name, plugin.as_deref(), focus)
                            })
                            .unwrap_or(AgentItemState::Ready)
                    } else if presence.anywhere.contains(&agent) {
                        AgentItemState::Off
                    } else {
                        AgentItemState::Missing
                    };
                    (agent, state)
                })
                .collect();
            let needs: Vec<Agent> = states
                .iter()
                .filter(|(_, state)| *state == AgentItemState::NeedsSignIn)
                .map(|(agent, _)| *agent)
                .collect();
            let missing: Vec<Agent> = agents
                .iter()
                .map(|adapter| adapter.agent())
                .filter(|agent| !presence.anywhere.contains(agent))
                .collect();
            let attention = if let Some(problem) = &presence.problem {
                Some(PluginAttention::WontStart {
                    message: format!("Won't start: {problem}"),
                })
            } else if let Some(reason) = states.iter().find_map(|(_, state)| match state {
                AgentItemState::WontStart { reason } => Some(reason),
                _ => None,
            }) {
                Some(PluginAttention::WontStart {
                    message: format!("Won't start: {reason}"),
                })
            } else if !needs.is_empty() {
                let verb = if needs.len() == 1 { "needs" } else { "need" };
                Some(PluginAttention::SignIn {
                    message: format!("{} {verb} you to sign in.", label(&needs)),
                })
            } else if !missing.is_empty() && !presence.anywhere.is_empty() {
                Some(PluginAttention::NotEverywhere {
                    message: format!("Only {} has it.", label(&presence.anywhere)),
                })
            } else {
                None
            };
            Some(PluginItemView {
                key: key.text(),
                kind,
                name: key.name.rsplit('/').next().unwrap_or(&key.name).to_owned(),
                description: presence.description,
                detail: presence.detail,
                remote: presence.remote,
                library,
                everywhere,
                on_in,
                off_in,
                agents: states
                    .into_iter()
                    .map(|(agent, state)| PluginAgentState {
                        agent: provider(agent),
                        state,
                    })
                    .collect(),
                attention,
                switchable: library || presence.switchable,
                update_available: false,
                can_undo: false,
                server_id: presence.server_id,
                files: presence.files,
                found_in: None,
            })
        })
        .collect();
    PluginsView {
        agents: agents
            .iter()
            .map(|adapter| PluginAgentView {
                agent: provider(adapter.agent()),
                label: adapter.label().to_owned(),
            })
            .collect(),
        projects: projects
            .iter()
            .map(|project| PluginProjectView {
                id: project.id.clone(),
                name: project.name.clone(),
                path: project.path.display().to_string(),
                notes: agents
                    .iter()
                    .filter(|adapter| !adapter.trusts(&project.catalog, &project.path))
                    .map(|adapter| PluginAgentNote {
                        agent: provider(adapter.agent()),
                        message: format!(
                            "{} only reads this project's settings once you trust the project.",
                            adapter.label()
                        ),
                    })
                    .collect(),
            })
            .collect(),
        items,
        elsewhere,
    }
}

impl super::SessionManager {
    /// The `brigadier` CLI that ships beside the daemon: what Library servers run through.
    pub(crate) fn brigadier_cli(&self) -> PathBuf {
        let name = if cfg!(windows) {
            "brigadier.exe"
        } else {
            "brigadier"
        };
        self.config
            .daemon_exe
            .parent()
            .map_or_else(|| PathBuf::from(name), |dir| dir.join(name))
    }

    /// The Library in the CLIs' home.
    pub(crate) fn library(&self) -> Library {
        Library::in_home(&self.registry().roots().home, self.brigadier_cli())
    }

    /// The user's trusted projects: id, name and folder (its first repository).
    pub(crate) fn plugin_projects(&self) -> Vec<(String, String, PathBuf)> {
        self.core
            .catalog()
            .projects
            .into_iter()
            .filter_map(|project| {
                let path = project.repos.first()?.path.clone();
                project.counts_trusted(&path).then(|| {
                    (
                        project.id.to_string(),
                        project.name.clone(),
                        PathBuf::from(path),
                    )
                })
            })
            .collect()
    }

    /// Settings › Plugins, looking at project `focus` (its id), else at every project.
    pub async fn plugins_view(&self, focus: Option<String>) -> crate::Result<PluginsView> {
        let registry = self.registry();
        let projects = self.plugin_projects();
        let library = self.library();
        let focus_path = focus.as_ref().and_then(|id| {
            projects
                .iter()
                .find(|(known, _, _)| known == id)
                .map(|(_, _, path)| path.clone())
        });
        let health = self.plugin_health(focus_path.as_deref());
        super::blocking(move || {
            let user = registry.catalog(None);
            let projects: Vec<ProjectCatalog> = projects
                .into_iter()
                .map(|(id, name, path)| ProjectCatalog {
                    catalog: registry.catalog(Some(&path)),
                    id,
                    name,
                    path,
                })
                .collect();
            let focus = focus.and_then(|id| {
                projects
                    .iter()
                    .find(|project| project.id == id)
                    .map(|project| project.path.clone())
            });
            Ok(build(
                &user,
                &projects,
                &library.entries(),
                health.as_ref(),
                focus.as_deref(),
            ))
        })
        .await
        .map(|mut view| {
            jobs::mark_library(&mut view, self.library());
            view
        })
    }

    /// The folder of the user's trusted project `id`.
    fn plugin_project(&self, id: Option<String>) -> crate::Result<Option<PathBuf>> {
        let Some(id) = id else {
            return Ok(None);
        };
        self.plugin_projects()
            .into_iter()
            .find(|(known, _, _)| *known == id)
            .map(|(_, _, path)| Some(path))
            .ok_or_else(|| crate::Error::Invalid(format!("no project {id}")))
    }

    fn plugin_job(&self, action: jobs::Action) -> String {
        jobs::start(
            jobs::Setup {
                registry: self.registry(),
                library: self.library(),
                cli: self.brigadier_cli(),
                projects: self
                    .plugin_projects()
                    .into_iter()
                    .map(|(_, _, path)| path)
                    .collect(),
            },
            action,
        )
    }

    /// Turns item `key` on or off: in project `project_id`, else in every project; for one
    /// agent only when `agent`. Its job's id.
    pub fn switch_plugin(
        &self,
        key: String,
        project_id: Option<String>,
        on: bool,
        agent: Option<ProviderKind>,
    ) -> crate::Result<String> {
        let project = self.plugin_project(project_id)?;
        Ok(self.plugin_job(jobs::Action::Switch {
            key,
            project,
            on,
            agent: agent.and_then(ops::agent_of),
        }))
    }

    /// What pasted `text` would add, before anything is added.
    pub async fn check_plugin_add(
        &self,
        text: String,
        name: Option<String>,
    ) -> crate::Result<ops::PluginAddCheck> {
        let home = self.registry().roots().home.clone();
        super::blocking(move || Ok(jobs::check_add(&home, &text, name.as_deref()))).await
    }

    /// Adds what `text` holds for every agent: in project `project_id` only ("just me" unless
    /// `shared`), else in every project. Its job's id.
    pub fn add_plugin(
        &self,
        text: String,
        name: Option<String>,
        project_id: Option<String>,
        shared: bool,
    ) -> crate::Result<String> {
        let project = self.plugin_project(project_id)?;
        Ok(self.plugin_job(jobs::Action::Add {
            text,
            name,
            project,
            shared,
        }))
    }

    /// Moves an agent's own item `key` into the Library for every agent. Its job's id.
    pub fn use_everywhere(&self, key: String) -> String {
        self.plugin_job(jobs::Action::UseEverywhere { key })
    }

    /// Undoes Use everywhere of Library item `key`. Its job's id.
    pub fn undo_use_everywhere(&self, key: String) -> String {
        self.plugin_job(jobs::Action::Undo { key })
    }

    /// Removes item `key` from every agent and project. Its job's id.
    pub fn remove_plugin(&self, key: String) -> String {
        self.plugin_job(jobs::Action::Remove { key })
    }

    /// Brings Library item `key` to its source's newest version. Its job's id.
    pub fn update_plugin(&self, key: String) -> String {
        self.plugin_job(jobs::Action::Update { key })
    }

    /// How plugin job `id` goes.
    pub fn plugin_job_view(&self, id: &str) -> crate::Result<ops::PluginJobView> {
        jobs::job(id).ok_or_else(|| crate::Error::Invalid(format!("no job {id}")))
    }

    /// What each agent's own list said of its servers, last time it was read, for every
    /// project and for `focus`; a stale or missing list is read again in the background.
    pub(crate) fn plugin_health(
        &self,
        focus: Option<&Path>,
    ) -> std::sync::Arc<dyn Health + Send + Sync> {
        let clis = self.clis();
        let mut lists = Vec::new();
        for adapter in adapters() {
            let agent = adapter.agent();
            for project in [None, focus] {
                if let Some(listed) = self.mcp_logins.listed(&clis, agent, project) {
                    lists.push((agent, project.map(Path::to_owned), listed));
                }
            }
        }
        std::sync::Arc::new(Lists(lists))
    }
}

#[cfg(test)]
mod tests;
