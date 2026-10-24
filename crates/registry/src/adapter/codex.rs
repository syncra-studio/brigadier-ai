//! Codex: its own `plugin` and `mcp` commands, which know only the user's config. For one
//! project it reads `<project>/.codex/config.toml` in projects it trusts, which Brigadier writes
//! (D8): an item only in some projects is off in the user's config (through Codex's own config
//! writer) and on in each of those projects' files.

use std::path::Path;

use super::{
    AgentAdapter, ApplyError, Capabilities, CliCall, Place, ProjectScope, Step, TomlEdit,
    Unsupported, app_server, apply,
};
use crate::{Agent, Catalog, Roots, Source};

const PROGRAM: &str = "codex";

pub struct Codex;

/// The file Codex reads for `project`.
pub fn project_file(project: &Path) -> std::path::PathBuf {
    project.join(".codex").join("config.toml")
}

/// `enabled` of `[<table>."<name>"]`.
fn enabled(table: &str, name: &str) -> Vec<String> {
    vec![table.to_owned(), name.to_owned(), "enabled".to_owned()]
}

/// The edit of `key` at `place`: the user's config, or the project's file.
fn edit(place: &Place, key: Vec<String>, value: Option<bool>) -> Step {
    let edit = TomlEdit {
        key,
        value: value.map(toml::Value::Boolean),
    };
    match place {
        Place::Everywhere => Step::UserConfig(edit),
        Place::Project { path, shared } => Step::ProjectFile {
            file: project_file(path),
            edit,
            shared: *shared,
        },
    }
}

impl AgentAdapter for Codex {
    fn agent(&self) -> Agent {
        Agent::Codex
    }

    fn label(&self) -> &'static str {
        "Codex"
    }

    fn program(&self) -> &'static str {
        PROGRAM
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            plugins: true,
            project_scope: ProjectScope::ProjectFile,
            login_needs_terminal: false,
        }
    }

    fn owns(&self, source: Source) -> bool {
        matches!(
            source,
            Source::CodexUser | Source::CodexProject | Source::CodexPlugin
        )
    }

    fn home_env(&self, roots: &Roots) -> Vec<(String, String)> {
        if roots.codex_home == roots.home.join(".codex") {
            Vec::new()
        } else {
            vec![(
                "CODEX_HOME".to_owned(),
                roots.codex_home.display().to_string(),
            )]
        }
    }

    fn trusts(&self, catalog: &Catalog, project: &Path) -> bool {
        catalog.codex_trusted.iter().any(|path| path == project)
    }

    fn add_marketplace(&self, dir: &Path) -> CliCall {
        CliCall::new(PROGRAM, &["plugin", "marketplace", "add"]).arg(dir.display().to_string())
    }

    fn installed(&self, roots: &Roots, id: &str) -> bool {
        crate::plugins::codex_installed(roots, id)
    }

    fn refresh_marketplace(&self, _: &str) -> Option<CliCall> {
        // It reads a local marketplace as it is.
        None
    }

    fn update_plugin(&self, id: &str, _: &Place) -> Result<Vec<Step>, Unsupported> {
        // Adding it again copies the new version into its cache; where it is on is unchanged.
        Ok(vec![Step::Run(
            CliCall::new(PROGRAM, &["plugin", "add"]).arg(id),
        )])
    }

    fn install_plugin(&self, id: &str, place: &Place) -> Result<Vec<Step>, Unsupported> {
        let add = Step::Run(CliCall::new(PROGRAM, &["plugin", "add"]).arg(id));
        Ok(match place {
            Place::Everywhere => vec![add],
            // Its install is the user's: off there, on in the project.
            Place::Project { .. } => vec![
                add,
                edit(&Place::Everywhere, enabled("plugins", id), Some(false)),
                edit(place, enabled("plugins", id), Some(true)),
            ],
        })
    }

    fn remove_plugin(&self, id: &str, place: &Place) -> Result<Vec<Step>, Unsupported> {
        Ok(match place {
            Place::Everywhere => vec![Step::Run(
                CliCall::new(PROGRAM, &["plugin", "remove"]).arg(id),
            )],
            Place::Project { .. } => vec![edit(place, enabled("plugins", id), None)],
        })
    }

    fn set_plugin(&self, id: &str, place: &Place, on: bool) -> Result<Vec<Step>, Unsupported> {
        Ok(vec![edit(place, enabled("plugins", id), Some(on))])
    }

    fn set_server(&self, name: &str, place: &Place, on: bool) -> Result<Vec<Step>, Unsupported> {
        Ok(vec![edit(place, enabled("mcp_servers", name), Some(on))])
    }

    fn login(&self, name: &str) -> CliCall {
        CliCall::new(PROGRAM, &["mcp", "login"]).arg(name)
    }

    fn list_servers(&self, project: Option<&Path>) -> CliCall {
        let call = CliCall::new(PROGRAM, &["mcp", "list", "--json"]);
        match project {
            Some(dir) => call.in_dir(dir),
            None => call,
        }
    }

    /// Through `codex app-server`'s `config/value/write`; a removal takes the tables it would
    /// leave empty with it.
    fn write_user_config(
        &self,
        roots: &Roots,
        env: &[(String, String)],
        edit: &TomlEdit,
    ) -> Result<(), ApplyError> {
        let (key, value) = match &edit.value {
            Some(value) => (
                edit.key.clone(),
                serde_json::to_value(value).map_err(|err| {
                    ApplyError::Failed(format!("Brigadier couldn't send the setting: {err}"))
                })?,
            ),
            None => {
                let config = std::fs::read_to_string(roots.codex_home.join("config.toml")).ok();
                (
                    apply::removal_key(config.as_deref(), &edit.key),
                    serde_json::Value::Null,
                )
            }
        };
        app_server::write_value(PROGRAM, env, &apply::key_path(&key), value)
    }
}
