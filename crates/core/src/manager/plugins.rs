//! Settings › Plugins: every skill and MCP server the agents have, as one list, each shared
//! with every agent ([`share`]): an agent that lacks an item gets Brigadier's Library copy,
//! while the agents that have it keep their own, untouched.
//!
//! Everything here is read from the agents' own setup ([`Catalog`]), read for the user and
//! for each of the user's trusted projects, so the page shows what a plain `claude` or `codex`
//! started in that project gets. Items Brigadier adds live in the Library
//! ([`brigadier_registry::library`]) and reach each agent through its own plugin commands.

mod jobs;
mod library_cli;
mod model;
pub mod ops;
mod share;

use std::path::{Path, PathBuf};

use brigadier_providers::ProviderKind;
use brigadier_registry::Agent;
use brigadier_registry::adapter::{Listed, adapter, adapters};
use brigadier_registry::library::{Entry, Library};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use model::{KeyKind, Model, ProjectCatalog, Row};
use share::Sharing;

/// The page.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginsView {
    /// The projects the scope picker offers: the user's trusted ones.
    pub projects: Vec<PluginProjectView>,
    /// In the order shown: items with a problem first (any of [`PluginProblem`]'s), then
    /// A–Z.
    pub items: Vec<PluginItemView>,
    /// Servers only other apps have (Claude Desktop, Cursor): no agent uses them until brought
    /// in.
    pub elsewhere: Vec<PluginElsewhereView>,
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

/// One skill or MCP server, for every agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginItemView {
    /// Stable: `server:<name>`, `skill:<name>` for an agent's own item (shared or not),
    /// `lib:<name>` for one added through Brigadier, `plugin:<plugin>` (`plugin:<plugin>/<server>`)
    /// for one of another marketplace's plugins.
    pub key: String,
    pub kind: PluginKind,
    pub name: String,
    /// Empty when it has none: the subtitle is then [`Self::detail`].
    pub description: String,
    /// Its command line, address or folder, as its config writes it (no variable expanded).
    pub detail: String,
    /// A remote server's address, as written.
    pub url: Option<String>,
    /// The remote server's site icon, as a `data:` URL, when the daemon could fetch it.
    pub icon: Option<String>,
    /// On for the agents (somewhere).
    pub on: bool,
    /// Where it is on now, or, switched off, where it goes back on.
    pub scope: PluginScope,
    pub problem: Option<PluginProblem>,
    /// Present: the switch and the scope can't change it, and this is why (one line, the
    /// subtitle). Absent: they can.
    pub locked: Option<String>,
    pub removal: PluginRemoval,
    /// Share with every agent is offered: the user removed it from the others, or sharing it
    /// failed.
    pub can_share: bool,
    /// How many skills it holds (another marketplace's plugin may hold several); 0 for a
    /// server.
    pub skill_count: u32,
    /// "Saved in": the files it is set up in (an agent's own first, then the Library copy's).
    pub files: Vec<String>,
}

/// Where an item is on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PluginScope {
    AllProjects,
    /// The user's projects (`projectIds`), and folders that aren't projects (`folders`, as
    /// paths: Claude's local scope).
    SomeProjects {
        project_ids: Vec<String>,
        folders: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PluginProblem {
    /// An agent needs the user to sign in: `serverId` and `projectId` go to
    /// `Request::SignInMcpServer`.
    NeedsSignIn {
        message: String,
        server_id: String,
        project_id: Option<String>,
    },
    /// It won't start: "Won't start: unknown type "websocket"."
    WontStart { message: String },
    /// Two different setups go by its name: neither is shared.
    Conflict { message: String },
    /// A newer version is waiting (an item added from Git or skills.sh).
    UpdateReady { message: String },
    /// Only some agents have it: "Only in Claude Code: came in Claude Code's own plugin."
    OnlyIn {
        message: String,
        agents: Vec<ProviderKind>,
        reason: OnlyInReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum OnlyInReason {
    /// It came in an agent's own plugin.
    OwnPlugin,
    /// The user removed it from the other agents.
    Removed,
    /// It can't be shared (a name or a value Brigadier won't copy).
    CantShare,
    /// Sharing it failed: Share with every agent tries again.
    Failed,
}

/// What Remove does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginRemoval {
    /// Added through Brigadier: Remove takes it from every agent.
    Everything,
    /// Remove takes Brigadier's copies and the off flags it added; the original stays.
    Copies,
    /// Brigadier made nothing for it: no Remove.
    None,
}

/// A server only another app has.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginElsewhereView {
    /// `other:<id>`.
    pub key: String,
    pub name: String,
    pub detail: String,
    pub url: Option<String>,
    /// The app: "Claude Desktop".
    pub found_in: String,
    pub file: String,
}

/// What an item is set up with: `Request::GetPluginItem`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginItemDetails {
    pub key: String,
    pub kind: PluginKind,
    /// The skill files to open: one for a skill, each one for a plugin holding several.
    pub skills: Vec<PluginSkillFile>,
    /// A server's settings, read-only.
    pub server: Option<PluginServerSettings>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginSkillFile {
    pub name: String,
    /// Its `SKILL.md`, absolute.
    pub path: String,
    pub contents: String,
    /// Why it can't be saved; absent when it can.
    pub read_only: Option<String>,
}

/// A server's settings as its config writes them: the names of its values, never a value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PluginServerSettings {
    Stdio {
        command: String,
        args: Vec<String>,
        cwd: Option<String>,
        env_names: Vec<String>,
    },
    Http {
        url: String,
        header_names: Vec<String>,
    },
}

/// How saving a skill file went: `Response::SavePluginSkill`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PluginSkillSave {
    /// Saved; the job (`Request::GetPluginJob`) brings its copies up to date.
    Saved { job: String },
    /// The file changed since it was opened (`expected` no longer matches): nothing was
    /// written. `current` is its text now.
    ChangedSinceOpened { current: String },
}

/// How one agent stands with an item: `brigadier library list`'s states.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum AgentItemState {
    Ready,
    NeedsSignIn,
    WontStart {
        reason: String,
    },
    /// Switched off where the call looks.
    Off,
    /// The agent doesn't have it.
    Missing,
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
/// project (`None`) and each project.
pub(crate) struct Lists(pub Vec<(Agent, Option<PathBuf>, Vec<Listed>)>);

impl Health for Lists {
    fn state(
        &self,
        agent: Agent,
        server: &str,
        plugin: Option<&str>,
        project: Option<&Path>,
    ) -> Option<AgentItemState> {
        let adapter = adapter(agent);
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

/// "Claude Code", "Claude Code and Codex".
fn labels(agents: &[Agent]) -> String {
    let names: Vec<&str> = agents.iter().map(|agent| adapter(*agent).label()).collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// A Library item as `brigadier library list` shows it.
#[derive(Debug, Clone)]
pub(crate) struct LibraryRow {
    pub name: String,
    pub kind: PluginKind,
    pub detail: String,
    pub everywhere: bool,
    pub on_in: Vec<String>,
    pub off_in: Vec<String>,
    pub agents: Vec<(Agent, AgentItemState)>,
    pub attention: Option<String>,
    /// The server to sign in to, and the project whose catalog has it.
    pub server_id: Option<(String, Option<String>)>,
}

/// The page and the CLI's Library rows.
pub(crate) struct Built {
    pub view: PluginsView,
    pub library: Vec<LibraryRow>,
}

/// Each of `ids` once, in order.
fn distinct(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out = Vec::new();
    for id in ids {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// Every repository folder of `projects` that the user trusts: id, name and folder. A project
/// of several repositories has a line for each (review #12).
pub(crate) fn trusted_repos(projects: &[crate::model::Project]) -> Vec<(String, String, PathBuf)> {
    projects
        .iter()
        .flat_map(|project| {
            project
                .repos
                .iter()
                .filter(|repo| project.counts_trusted(&repo.path))
                .map(|repo| {
                    (
                        project.id.to_string(),
                        project.name.clone(),
                        PathBuf::from(&repo.path),
                    )
                })
        })
        .collect()
}

/// The project id of folder `path`, when it is one of the user's projects.
fn project_id(projects: &[ProjectCatalog], path: &Path) -> Option<String> {
    projects
        .iter()
        .find(|project| project.path == path)
        .map(|project| project.id.clone())
}

/// The page from the model; `updates`: the Library items with a newer version waiting;
/// `focus`: where the CLI's agent states look (else every project).
pub(crate) fn build(
    model: &Model,
    projects: &[ProjectCatalog],
    sharing: &Sharing,
    health: &dyn Health,
    updates: &[String],
    focus: Option<&Path>,
) -> Built {
    let every: Vec<Agent> = adapters().iter().map(|adapter| adapter.agent()).collect();
    let mut items: Vec<(bool, PluginItemView)> = Vec::new();
    let mut library = Vec::new();
    for row in &model.rows {
        let key = row.key.text();
        let everywhere = row.everywhere();
        let here = row.here();
        let on = row.on() && sharing.chosen.get(&key).is_none_or(|c| c.on);
        let in_project = |path: &Path| {
            here.iter()
                .filter(|(at, _)| at == path)
                .map(|(_, agent)| *agent)
                .collect::<Vec<Agent>>()
        };
        let on_in: Vec<&ProjectCatalog> = projects
            .iter()
            .filter(|project| !in_project(&project.path).is_empty())
            .collect();
        let scope = if let Some(choice) = sharing.chosen.get(&key) {
            match &choice.scope {
                None => PluginScope::AllProjects,
                Some(paths) => PluginScope::SomeProjects {
                    project_ids: distinct(paths.iter().filter_map(|p| project_id(projects, p))),
                    folders: vec![],
                },
            }
        } else if !on {
            match sharing.off_scopes.get(&key) {
                Some(Some(paths)) => PluginScope::SomeProjects {
                    project_ids: distinct(
                        paths.iter().filter_map(|path| project_id(projects, path)),
                    ),
                    folders: Vec::new(),
                },
                _ => PluginScope::AllProjects,
            }
        } else if !everywhere.is_empty() && on_in.len() == projects.len() {
            PluginScope::AllProjects
        } else {
            let mut folders = Vec::new();
            for (path, _) in &here {
                if !projects.iter().any(|project| project.path == *path) {
                    let path = path.display().to_string();
                    if !folders.contains(&path) {
                        folders.push(path);
                    }
                }
            }
            PluginScope::SomeProjects {
                project_ids: distinct(on_in.iter().map(|project| project.id.clone())),
                folders,
            }
        };
        // Each agent's server, and where it is on: what its own list says there.
        let mut native = row.own.native.clone();
        for entry in &row.copy.native {
            if !native.contains(entry) {
                native.push(entry.clone());
            }
        }
        let places = |agent: Agent| -> Vec<Option<PathBuf>> {
            everywhere
                .contains(&agent)
                .then_some(None)
                .into_iter()
                .chain(
                    here.iter()
                        .filter(|(_, a)| *a == agent)
                        .map(|(path, _)| Some(path.clone())),
                )
                .collect()
        };
        let states: Vec<AgentItemState> = native
            .iter()
            .flat_map(|(agent, name, plugin)| {
                places(*agent)
                    .into_iter()
                    .filter_map(|place| {
                        health.state(*agent, name, plugin.as_deref(), place.as_deref())
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        let mut needs: Vec<Agent> = Vec::new();
        for (agent, name, plugin) in &native {
            if places(*agent).into_iter().any(|place| {
                health.state(*agent, name, plugin.as_deref(), place.as_deref())
                    == Some(AgentItemState::NeedsSignIn)
            }) && !needs.contains(agent)
            {
                needs.push(*agent);
            }
        }
        let server_id = row
            .own
            .server_id
            .clone()
            .or_else(|| row.copy.server_id.clone());
        let mut holders = row.holders();
        for agent in row.copy_agents() {
            if !holders.contains(agent) {
                holders.push(*agent);
            }
        }
        holders.sort_by_key(|agent| every.iter().position(|known| known == agent));
        let missing: Vec<Agent> = every
            .iter()
            .copied()
            .filter(|agent| !holders.contains(agent))
            .collect();
        let declined = sharing.is_declined(row);
        let failure = sharing.failed.get(&key);
        let only_in = |reason: OnlyInReason, why: String| PluginProblem::OnlyIn {
            message: format!("Only in {}: {why}", labels(&holders)),
            agents: holders.iter().map(|agent| provider(*agent)).collect(),
            reason,
        };
        let problem = if let Some(problem) = row.own.problem.as_ref().or(row.copy.problem.as_ref())
        {
            Some(PluginProblem::WontStart {
                message: format!("Won't start: {problem}"),
            })
        } else if row.conflict {
            Some(PluginProblem::Conflict {
                message: format!(
                    "Two different setups are called {}: Brigadier shares neither until one goes.",
                    row.key.name
                ),
            })
        } else if let Some(reason) = states.iter().find_map(|state| match state {
            AgentItemState::WontStart { reason } => Some(reason),
            _ => None,
        }) {
            Some(PluginProblem::WontStart {
                message: format!("Won't start: {reason}"),
            })
        } else if let (false, Some((id, project))) = (needs.is_empty(), &server_id) {
            let verb = if needs.len() == 1 { "needs" } else { "need" };
            Some(PluginProblem::NeedsSignIn {
                message: format!("{} {verb} you to sign in.", labels(&needs)),
                server_id: id.clone(),
                project_id: project
                    .as_deref()
                    .and_then(|path| project_id(projects, path)),
            })
        } else if row.lib.as_ref().is_some_and(|lib| updates.contains(lib)) {
            Some(PluginProblem::UpdateReady {
                message: "A newer version is ready.".to_owned(),
            })
        } else if holders.is_empty() || missing.is_empty() {
            None
        } else if row.key.kind == KeyKind::Plugin {
            let label = labels(&holders);
            Some(only_in(
                OnlyInReason::OwnPlugin,
                format!("came in {label}'s own plugin"),
            ))
        } else if declined {
            Some(only_in(
                OnlyInReason::Removed,
                "you removed it from the other agents".to_owned(),
            ))
        } else if let Some(why) = share::cant_share(row) {
            Some(only_in(OnlyInReason::CantShare, why))
        } else {
            failure.map(|failure| {
                only_in(
                    OnlyInReason::Failed,
                    format!(
                        "couldn't share it with {}: {}",
                        labels(&missing),
                        failure.reason
                    ),
                )
            })
        };
        // Every row with a problem comes first (L5 Q6).
        let urgent = problem.is_some();
        let own = !row.own.sources.is_empty();
        let removal = match (&row.lib, own) {
            (Some(_), true) => PluginRemoval::Copies,
            (Some(_), false) => PluginRemoval::Everything,
            (None, _) if sharing.owned.iter().any(|f| f.owner == key) => PluginRemoval::Copies,
            (None, _) => PluginRemoval::None,
        };
        let name = row
            .key
            .name
            .rsplit('/')
            .next()
            .unwrap_or(&row.key.name)
            .to_owned();
        let mut files = row.own.files.clone();
        for file in &row.copy.files {
            if !files.contains(file) {
                files.push(file.clone());
            }
        }
        let detail = if row.own.detail.is_empty() {
            row.copy.detail.clone()
        } else {
            row.own.detail.clone()
        };
        let skills = if row.own.skills.is_empty() {
            row.copy.skills.len()
        } else {
            row.own.skills.len()
        };
        let view = PluginItemView {
            key: key.clone(),
            kind: row.kind,
            name: name.clone(),
            description: if row.own.description.is_empty() {
                row.copy.description.clone()
            } else {
                row.own.description.clone()
            },
            detail: detail.clone(),
            url: row.own.url.clone().or_else(|| row.copy.url.clone()),
            icon: None,
            on,
            scope: scope.clone(),
            locked: row.own.lock.clone(),
            removal,
            can_share: row.key.kind != KeyKind::Plugin && own && (declined || failure.is_some()),
            skill_count: if row.kind == PluginKind::Skill {
                u32::try_from(skills.max(1)).unwrap_or(u32::MAX)
            } else {
                0
            },
            files,
            problem,
        };
        if let Some(lib) = &row.lib {
            library.push(library_row(
                row, lib, &view, projects, health, focus, server_id,
            ));
        }
        items.push((urgent, view));
    }
    items.sort_by(|(a, x), (b, y)| {
        b.cmp(a)
            .then_with(|| x.name.to_lowercase().cmp(&y.name.to_lowercase()))
            .then_with(|| x.key.cmp(&y.key))
    });
    Built {
        view: PluginsView {
            // One line a project: its first repository's.
            projects: projects
                .iter()
                .enumerate()
                .filter(|(at, project)| !projects[..*at].iter().any(|known| known.id == project.id))
                .map(|(_, project)| PluginProjectView {
                    id: project.id.clone(),
                    name: project.name.clone(),
                    path: project.path.display().to_string(),
                    notes: adapters()
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
            items: items.into_iter().map(|(_, view)| view).collect(),
            elsewhere: model.elsewhere.clone(),
        },
        library,
    }
}

/// `row`'s Library item as `brigadier library list` shows it, each agent's state where
/// `focus` looks.
fn library_row(
    row: &Row,
    lib: &str,
    view: &PluginItemView,
    projects: &[ProjectCatalog],
    health: &dyn Health,
    focus: Option<&Path>,
    server_id: Option<(String, Option<PathBuf>)>,
) -> LibraryRow {
    let everywhere = row.everywhere();
    let here = row.here();
    let in_project = |path: &Path| here.iter().any(|(at, _)| at == path);
    let global = view.on && matches!(view.scope, PluginScope::AllProjects);
    let agents = adapters()
        .iter()
        .map(|adapter| {
            let agent = adapter.agent();
            let has = match focus {
                None => everywhere.contains(&agent),
                Some(path) => here.iter().any(|(at, a)| at == path && *a == agent),
            };
            let state = if has {
                row.own
                    .native
                    .iter()
                    .chain(&row.copy.native)
                    .find(|(a, _, _)| *a == agent)
                    .and_then(|(_, name, plugin)| {
                        health.state(agent, name, plugin.as_deref(), focus)
                    })
                    .unwrap_or(AgentItemState::Ready)
            } else if row.own.anywhere.contains(&agent) || row.copy.anywhere.contains(&agent) {
                AgentItemState::Off
            } else {
                AgentItemState::Missing
            };
            (agent, state)
        })
        .collect();
    LibraryRow {
        name: lib.to_owned(),
        kind: row.kind,
        detail: view.detail.clone(),
        everywhere: global,
        on_in: if global {
            Vec::new()
        } else {
            projects
                .iter()
                .filter(|project| in_project(&project.path))
                .map(|project| project.path.display().to_string())
                .collect()
        },
        off_in: if global {
            projects
                .iter()
                .filter(|project| !in_project(&project.path))
                .map(|project| project.path.display().to_string())
                .collect()
        } else {
            Vec::new()
        },
        agents,
        attention: view.problem.as_ref().map(|problem| match problem {
            PluginProblem::NeedsSignIn { message, .. }
            | PluginProblem::WontStart { message }
            | PluginProblem::Conflict { message }
            | PluginProblem::UpdateReady { message }
            | PluginProblem::OnlyIn { message, .. } => message.clone(),
        }),
        server_id: server_id.map(|(id, project)| {
            (
                id,
                project
                    .as_deref()
                    .and_then(|path| project_id(projects, path)),
            )
        }),
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

    /// The user's trusted repositories: project id, name and folder, a line for each of a
    /// project's repositories the user trusts.
    pub(crate) fn plugin_projects(&self) -> Vec<(String, String, PathBuf)> {
        trusted_repos(&self.core.catalog().projects)
    }

    /// Every trusted repository folder: what sharing reads and the watcher looks at.
    fn plugin_repos(&self) -> Vec<PathBuf> {
        let mut repos: Vec<PathBuf> = self
            .plugin_projects()
            .into_iter()
            .map(|(_, _, path)| path)
            .collect();
        repos.sort();
        repos.dedup();
        repos
    }

    /// The user's projects changed (added, removed, trusted): the watcher and sharing follow.
    pub(crate) fn plugin_projects_changed(&self) {
        self.request_plugin_sync(false);
    }

    /// The page and the CLI's Library rows, with each agent's state where `focus` (a project
    /// id) looks.
    async fn plugins_built(&self, focus: Option<String>) -> crate::Result<Built> {
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
        let home = registry.roots().home.clone();
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
            let entries: Vec<Entry> = library.entries();
            let sharing = Sharing::load(&library).unwrap_or_default();
            let model = model::model(&user, &projects, &entries, &sharing, &home);
            let updates = jobs::updates_waiting(library, &entries);
            Ok(build(
                &model,
                &projects,
                &sharing,
                health.as_ref(),
                &updates,
                focus_path.as_deref(),
            ))
        })
        .await
    }

    /// Settings › Plugins.
    pub async fn plugins_view(&self) -> crate::Result<PluginsView> {
        self.plugins_built(None).await.map(|built| built.view)
    }

    /// The Library's items as `brigadier library` shows them, each agent's state in project
    /// `focus` (an id) or every project.
    pub(crate) async fn library_rows(
        &self,
        focus: Option<String>,
    ) -> crate::Result<Vec<LibraryRow>> {
        self.plugins_built(focus).await.map(|built| built.library)
    }

    /// The folders of the user's projects `ids`: `None` stays every project.
    fn plugin_scope(&self, ids: Option<Vec<String>>) -> crate::Result<Option<Vec<PathBuf>>> {
        let Some(ids) = ids else {
            return Ok(None);
        };
        if ids.is_empty() {
            return Err(crate::Error::Invalid("no projects picked".to_owned()));
        }
        let projects = self.plugin_projects();
        let mut paths = Vec::new();
        for id in ids {
            let found: Vec<PathBuf> = projects
                .iter()
                .filter(|(known, _, _)| *known == id)
                .map(|(_, _, path)| path.clone())
                .collect();
            if found.is_empty() {
                return Err(crate::Error::Invalid(format!("no project {id}")));
            }
            // Every repository of the project.
            paths.extend(found);
        }
        Ok(Some(paths))
    }

    fn plugin_setup(&self) -> jobs::Setup {
        jobs::Setup {
            registry: self.registry(),
            library: self.library(),
            cli: self.brigadier_cli(),
            claude_configs: self
                .runtime
                .account_homes(ProviderKind::Claude)
                .into_iter()
                .map(|home| home.join(".claude.json"))
                .collect(),
            binary: self.clis().binary,
            projects: self
                .plugin_projects()
                .into_iter()
                .map(|(_, _, path)| path)
                .collect(),
        }
    }

    fn plugin_job(&self, action: jobs::Action) -> String {
        jobs::start(self.plugin_setup(), action)
    }

    /// Turns item `key` on or off for every agent, keeping its projects for when it is on
    /// again. Its job's id.
    pub fn switch_plugin(&self, key: String, on: bool) -> String {
        self.plugin_job(jobs::Action::Switch { key, on })
    }

    /// Puts item `key` on in every project (`project_ids` `None`) or in exactly the user's
    /// projects `project_ids`, for every agent. Its job's id.
    pub fn set_plugin_scope(
        &self,
        key: String,
        project_ids: Option<Vec<String>>,
    ) -> crate::Result<String> {
        let scope = self.plugin_scope(project_ids)?;
        Ok(self.plugin_job(jobs::Action::SetScope { key, scope }))
    }

    /// Share with every agent: item `key`, removed earlier or failed, is shared now. Its job's
    /// id.
    pub fn share_plugin(&self, key: String) -> String {
        self.plugin_job(jobs::Action::Share { key })
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

    /// Adds what `text` holds for every agent: in every project (`project_ids` `None`) or in
    /// exactly those. `values` sets or adds the one pasted server's env or headers. Its job's
    /// id.
    pub fn add_plugin(
        &self,
        text: String,
        name: Option<String>,
        project_ids: Option<Vec<String>>,
        values: Option<std::collections::BTreeMap<String, String>>,
    ) -> crate::Result<String> {
        self.add_plugin_as(text, name, project_ids, false, values)
    }

    /// [`Self::add_plugin`]; `shared` (the CLI's `--shared`, one project) in the form the
    /// project commits.
    pub(crate) fn add_plugin_as(
        &self,
        text: String,
        name: Option<String>,
        project_ids: Option<Vec<String>>,
        shared: bool,
        values: Option<std::collections::BTreeMap<String, String>>,
    ) -> crate::Result<String> {
        let scope = self.plugin_scope(project_ids)?;
        Ok(self.plugin_job(jobs::Action::Add {
            text,
            name,
            scope,
            shared,
            options: ops::AddOptions {
                values: values.unwrap_or_default(),
            },
        }))
    }

    /// Brings another app's server (`key`: an `elsewhere` entry's) to every agent, as Add does
    /// a pasted one: in every project (`project_ids` `None`) or in exactly those; `name`
    /// replaces its own on a clash. Its job's id.
    pub fn import_plugin(
        &self,
        key: String,
        name: Option<String>,
        project_ids: Option<Vec<String>>,
    ) -> crate::Result<String> {
        let scope = self.plugin_scope(project_ids)?;
        Ok(self.plugin_job(jobs::Action::Import { key, name, scope }))
    }

    /// Removes what Brigadier made for item `key`: its copies (the agents' own entry stays,
    /// and it isn't shared again), or an item added through Brigadier whole. Its job's id.
    pub fn remove_plugin(&self, key: String) -> String {
        self.plugin_job(jobs::Action::Remove { key })
    }

    /// Brings Library item `key` to its source's newest version. Its job's id.
    pub fn update_plugin(&self, key: String) -> String {
        self.plugin_job(jobs::Action::Update { key })
    }

    /// What item `key` is set up with: its skill files, or its server's settings.
    pub async fn plugin_item(&self, key: String) -> crate::Result<PluginItemDetails> {
        let setup = self.plugin_setup();
        super::blocking(move || {
            jobs::with_ctx(&setup, &|_, _| {}, |ctx| share::details(ctx, &key))
                .map_err(crate::Error::Invalid)
        })
        .await
    }

    /// Saves `contents` as item `key`'s skill file `path`, which must still hold `expected`
    /// (the text as opened); then its copies follow, in a job.
    pub async fn save_plugin_skill(
        &self,
        key: String,
        path: String,
        contents: String,
        expected: String,
    ) -> crate::Result<PluginSkillSave> {
        let setup = self.plugin_setup();
        let written = {
            let (key, path) = (key.clone(), path.clone());
            super::blocking(move || {
                jobs::with_ctx(&setup, &|_, _| {}, |ctx| {
                    share::write_skill(ctx, &key, &path, &contents, &expected)
                })
                .map_err(crate::Error::Invalid)
            })
            .await?
        };
        Ok(match written {
            share::Saved::ChangedSinceOpened { current } => {
                PluginSkillSave::ChangedSinceOpened { current }
            }
            share::Saved::Saved => PluginSkillSave::Saved {
                job: self.plugin_job(jobs::Action::FollowSkill { key, path }),
            },
        })
    }

    /// How plugin job `id` goes.
    pub fn plugin_job_view(&self, id: &str) -> crate::Result<ops::PluginJobView> {
        jobs::job(id).ok_or_else(|| crate::Error::Invalid(format!("no job {id}")))
    }

    /// Starts sharing: a pass now in the background, then one after each change the watcher
    /// sees (the user's projects watched too). Shows nothing.
    pub(crate) fn start_plugin_sharing(&self) {
        // Tests share nothing behind their back.
        if cfg!(test) {
            return;
        }
        let registry = self.registry();
        registry.watch_projects(&self.plugin_repos());
        let me = self.me.clone();
        registry.on_change(move || {
            if let Some(manager) = me.upgrade() {
                manager.request_plugin_sync(false);
            }
        });
        self.request_plugin_sync(true);
    }

    /// Asks for a sharing pass (one more after the running one, if one runs); `retry`: try
    /// again what failed with the same inputs.
    fn request_plugin_sync(&self, retry: bool) {
        // Tests share nothing behind their back.
        if cfg!(test) {
            return;
        }
        self.registry().watch_projects(&self.plugin_repos());
        jobs::request_sync(self.plugin_setup(), retry);
    }

    /// What each agent's own list said of its servers, last time it was read, for every
    /// project and for `focus`; a stale or missing list is read again in the background.
    pub(crate) fn plugin_health(
        &self,
        focus: Option<&Path>,
    ) -> std::sync::Arc<dyn Health + Send + Sync> {
        let clis = self.clis();
        let mut lists = Vec::new();
        // Every project's too: a row shows how it stands wherever it is on.
        let projects: Vec<PathBuf> = self
            .plugin_projects()
            .into_iter()
            .map(|(_, _, path)| path)
            .filter(|path| Some(path.as_path()) != focus)
            .collect();
        for adapter in adapters() {
            let agent = adapter.agent();
            for project in [None, focus]
                .into_iter()
                .chain(projects.iter().map(|path| Some(path.as_path())))
            {
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
