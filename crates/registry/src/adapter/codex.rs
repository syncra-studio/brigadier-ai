//! Codex MCP sign-in, listing, project-file discovery, and original restoration.
use std::path::Path;

use super::{AgentAdapter, ApplyError, CliCall, Listed, Place, Step, TomlEdit, Unsupported};
use crate::item::ServerSpec;
use crate::{Agent, Catalog, Roots, Source};

const PROGRAM: &str = "codex";

pub struct Codex;

/// The file Codex reads for `project`.
pub fn project_file(project: &Path) -> std::path::PathBuf {
    project.join(".codex").join("config.toml")
}

/// `enabled` of `[<table>."<name>"]`.
/// The edit of `key` at `place`: the user's config, or the project's file.
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
    fn trusts(&self, catalog: &Catalog, project: &Path) -> bool {
        catalog.codex_trusted.iter().any(|path| path == project)
    }

    fn agent(&self) -> Agent {
        Agent::Codex
    }

    fn label(&self) -> &'static str {
        "Codex"
    }

    fn program(&self) -> &'static str {
        PROGRAM
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
            if let Some(oauth) = &spec.oauth {
                entry.insert(
                    "oauth".to_owned(),
                    serde_json::json!({ "client_id": oauth.client_id }),
                );
            }
        }
        let value: toml::Value = serde_json::from_value(entry)
            .map_err(|err| Unsupported(format!("Codex can't take this server: {err}")))?;
        Ok(vec![table(
            place,
            vec!["mcp_servers".to_owned(), name.to_owned()],
            Some(value),
        )])
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
                    super::migration::removal_key(config.as_deref(), &edit.key),
                    serde_json::Value::Null,
                )
            }
        };
        super::migration::codex_writer::write_value(
            program,
            env,
            &super::migration::key_path(&key),
            value,
        )
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
