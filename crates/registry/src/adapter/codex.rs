//! Codex: its own `plugin` and `mcp` commands, which know only the user's config. For one
//! project it reads `<project>/.codex/config.toml` in projects it trusts, which Brigadier writes
//! (D8): an item only in some projects is off in the user's config (through Codex's own config
//! writer) and on in each of those projects' files.

use std::path::Path;

use super::{
    AgentAdapter, ApplyError, Capabilities, CliCall, Listed, Place, ProjectScope, Step, TomlEdit,
    Unsupported, app_server, apply,
};
use crate::item::ServerSpec;
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
    table(place, key, value.map(toml::Value::Boolean))
}

/// `key` set to `value` (removed for `None`) at `place`.
fn table(place: &Place, key: Vec<String>, value: Option<toml::Value>) -> Step {
    let edit = TomlEdit { key, value };
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

    fn switched_on_in(&self, project: &Path, id: &str) -> bool {
        crate::plugins::codex_file_switch(&project_file(project), id) == Some(true)
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

    fn add_server(
        &self,
        name: &str,
        spec: &ServerSpec,
        place: &Place,
    ) -> Result<Vec<Step>, Unsupported> {
        let mut entry = super::server_json(spec, "http_headers");
        if let Some(entry) = entry.as_object_mut() {
            // It tells the two apart by `command` or `url`.
            entry.remove("type");
        }
        let value: toml::Value = serde_json::from_value(entry)
            .map_err(|err| Unsupported(format!("Codex can't take this server: {err}")))?;
        Ok(vec![table(
            place,
            vec!["mcp_servers".to_owned(), name.to_owned()],
            Some(value),
        )])
    }

    fn remove_server(&self, name: &str, place: &Place) -> Result<Vec<Step>, Unsupported> {
        Ok(match place {
            Place::Everywhere => vec![Step::Run(
                CliCall::new(PROGRAM, &["mcp", "remove"]).arg(name),
            )],
            Place::Project { .. } => vec![table(
                place,
                vec!["mcp_servers".to_owned(), name.to_owned()],
                None,
            )],
        })
    }

    fn set_plugin(&self, id: &str, place: &Place, on: bool) -> Result<Vec<Step>, Unsupported> {
        Ok(vec![edit(place, enabled("plugins", id), Some(on))])
    }

    fn set_server(&self, name: &str, place: &Place, on: bool) -> Result<Vec<Step>, Unsupported> {
        Ok(vec![edit(place, enabled("mcp_servers", name), Some(on))])
    }

    fn login(&self, name: &str, project: Option<&Path>) -> CliCall {
        // It waits for the browser's callback while its input stays open.
        CliCall::new(PROGRAM, &["mcp", "login"])
            .arg(name)
            .arg("--no-browser")
            .in_project(project)
    }

    fn logout(&self, name: &str, project: Option<&Path>) -> CliCall {
        CliCall::new(PROGRAM, &["mcp", "logout"])
            .arg(name)
            .in_project(project)
    }

    fn list_servers(&self, project: Option<&Path>) -> CliCall {
        CliCall::new(PROGRAM, &["mcp", "list", "--json"]).in_project(project)
    }

    /// Through `codex app-server`'s `config/value/write`; a removal takes the tables it would
    /// leave empty with it.
    fn write_user_config(
        &self,
        roots: &Roots,
        env: &[(String, String)],
        program: &Path,
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
        app_server::write_value(program, env, &apply::key_path(&key), value)
    }

    /// A JSON array of servers, `auth_status` `not_logged_in` for one that needs a sign-in
    /// (0.164.0).
    fn parse_servers(&self, output: &str) -> Vec<Listed> {
        let Ok(serde_json::Value::Array(servers)) = serde_json::from_str(output.trim()) else {
            return Vec::new();
        };
        servers
            .iter()
            .filter_map(|server| {
                Some(Listed {
                    name: server.get("name")?.as_str()?.to_owned(),
                    needs_sign_in: server.get("auth_status").and_then(|s| s.as_str())
                        == Some("not_logged_in"),
                    // It lists without connecting.
                    failed: None,
                })
            })
            .collect()
    }

    fn is_server(&self, listed: &str, server: &str, _: Option<&str>) -> bool {
        // A plugin's server keeps its own name.
        listed == server
    }
}
