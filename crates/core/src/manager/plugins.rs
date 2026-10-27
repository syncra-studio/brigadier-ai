//! Read-only plugin catalog, data-dir Library actions, and one-time legacy cleanup.
pub mod choices;
mod cleanup;
mod cleanup_context;
mod cleanup_state;
pub mod launch;
mod library_cli;
mod model;
pub mod ops;
pub mod resolve;
use brigadier_providers::ProviderKind;
use brigadier_registry::Agent;
use brigadier_registry::library::Library;
pub use model::{CatalogDefinition, CatalogRow, CatalogSignIn, CatalogView};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use ts_rs::TS;
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
    /// Saved to the Library. The empty job field is retained for the page display contract.
    Saved { job: String },
    /// The file changed since it was opened (`expected` no longer matches): nothing was
    /// written. `current` is its text now.
    ChangedSinceOpened { current: String },
}

pub(crate) fn provider(agent: Agent) -> ProviderKind {
    match agent {
        Agent::Claude => ProviderKind::Claude,
        Agent::Codex => ProviderKind::Codex,
    }
}

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

impl super::SessionManager {
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

    pub(crate) fn library(&self) -> Library {
        Library::in_home(&self.registry().roots().home, self.brigadier_cli())
    }

    pub(crate) fn plugin_projects(&self) -> Vec<(String, String, PathBuf)> {
        trusted_repos(&self.core.catalog().projects)
    }

    fn plugin_setup(&self) -> cleanup_context::Setup {
        cleanup_context::Setup {
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

    pub async fn check_plugin_add(
        &self,
        text: String,
        name: Option<String>,
    ) -> crate::Result<ops::PluginAddCheck> {
        let home = self.registry().roots().home.clone();
        super::blocking(move || Ok(ops::check_add(&home, &text, name.as_deref()))).await
    }
}
