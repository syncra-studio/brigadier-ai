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
pub use model::{CatalogDefinition, CatalogRow, CatalogSignIn, CatalogView, RemovedPlugin};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use ts_rs::TS;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginKind {
    Server,
    Skill,
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
