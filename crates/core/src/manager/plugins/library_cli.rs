//! `brigadier library list|add|remove|login`: data-dir library content, never CLI installs.
//! Workers can list; user sessions can change the library.

use std::path::Path;
use std::time::Duration;

use brigadier_cli::protocol::{CliReply, ErrorCode, LibraryItem, LibraryOutput};
use serde::Deserialize;
use serde_json::Value;

use super::CatalogSignIn;
use crate::manager::SessionManager;
use crate::manager::cli::CliCaller;

/// How long `login` waits for the user to sign in, every agent at once.
const SIGN_IN_WAIT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Args {
    text: Option<String>,
    name: Option<String>,
    /// A project folder, absolute.
    project: Option<String>,
    shared: bool,
}

fn failed(message: impl Into<String>) -> CliReply {
    CliReply::error(ErrorCode::Failed, message)
}

/// A serialized enum's tag (`ready`, `needsSignIn`, `claude`, `server`): the names the page
/// and the schema share.
fn tag(value: impl serde::Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(name)) => name,
        Ok(Value::Object(object)) => object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        _ => String::new(),
    }
}

impl SessionManager {
    /// `brigadier library <action>`, for `caller`.
    pub(crate) async fn cli_library(
        &self,
        caller: &CliCaller,
        action: &str,
        given: Value,
    ) -> CliReply {
        if action != "list" && caller.task_id.is_some() {
            return CliReply::error(
                ErrorCode::Refused,
                "refused: a worker can list the Library but not change it; say what it needs \
                 in your last message, and your session adds it",
            );
        }
        let args: Args = match serde_json::from_value(given) {
            Ok(args) => args,
            Err(err) => return CliReply::error(ErrorCode::Usage, format!("bad arguments: {err}")),
        };
        let project = match &args.project {
            None => None,
            Some(path) => match self
                .plugin_projects()
                .into_iter()
                .find(|(_, _, folder)| folder == Path::new(path))
            {
                Some((id, _, _)) => Some(id),
                None => {
                    return failed(format!(
                        "{path} isn't one of your projects in Brigadier, or you haven't \
                         trusted it yet"
                    ));
                }
            },
        };
        let answer = match action {
            "list" => Ok(()),
            "add" => {
                if args.shared {
                    return failed(
                        "--shared is no longer supported: library items are used by Brigadier sessions.",
                    );
                }
                self.add_library_item(
                    args.text.unwrap_or_default(),
                    args.name,
                    args.project.map(|path| vec![path]),
                    None,
                )
                .await
            }
            "remove" => {
                self.remove_library_item(format!("lib:{}", args.name.unwrap_or_default()))
                    .await
            }
            "login" => {
                return self
                    .cli_library_login(project, args.name.unwrap_or_default())
                    .await;
            }
            other => return CliReply::error(ErrorCode::Usage, format!("no action {other:?}")),
        };
        if let Err(error) = answer {
            return failed(error.to_string());
        }
        self.cli_library_output(action)
    }

    fn cli_library_output(&self, action: &str) -> CliReply {
        let catalog = match self.plugin_catalog() {
            Ok(catalog) => catalog,
            Err(error) => return failed(error.to_string()),
        };
        let root = self.data_dir.join("library");
        let items = catalog
            .rows
            .into_iter()
            .filter(|row| {
                row.definitions
                    .iter()
                    .any(|definition| Path::new(&definition.file).starts_with(&root))
            })
            .map(|row| {
                let (everywhere, on_in) = match &row.choice.scope {
                    super::choices::Scope::Global(_) => (row.choice.on, Vec::new()),
                    super::choices::Scope::Projects(paths) => (
                        false,
                        if row.choice.on {
                            paths
                                .iter()
                                .map(|path| path.to_string_lossy().into_owned())
                                .collect()
                        } else {
                            Vec::new()
                        },
                    ),
                };
                LibraryItem {
                    name: row.name,
                    kind: tag(row.kind),
                    detail: row
                        .definitions
                        .first()
                        .map(|definition| {
                            definition
                                .endpoint
                                .clone()
                                .unwrap_or_else(|| definition.file.clone())
                        })
                        .unwrap_or_default(),
                    everywhere,
                    on_in,
                    off_in: Vec::new(),
                    agents: Vec::new(),
                    attention: None,
                }
            })
            .collect();
        CliReply::ok(&LibraryOutput {
            action: action.to_owned(),
            items,
            agents: Vec::new(),
            warning: None,
        })
    }

    /// `login <name>`: every agent that needs it signs in to the Library server `name`, all at
    /// once, in the user's browser; the answer comes the moment the last one did.
    async fn cli_library_login(&self, _project: Option<String>, name: String) -> CliReply {
        let catalog = match self.plugin_catalog() {
            Ok(catalog) => catalog,
            Err(error) => return failed(error.to_string()),
        };
        let root = self.data_dir.join("library");
        let Some(row) = catalog.rows.iter().find(|row| {
            row.name == name
                && row
                    .definitions
                    .iter()
                    .any(|definition| Path::new(&definition.file).starts_with(&root))
        }) else {
            return failed(format!("{name} isn't in your Library"));
        };
        let mut state = match self.start_plugin_sign_in(&row.id).await {
            Ok(state) => state,
            Err(error) => return failed(error.to_string()),
        };
        let deadline = tokio::time::Instant::now() + SIGN_IN_WAIT;
        loop {
            match state {
                CatalogSignIn::SignedIn | CatalogSignIn::NotRequired => {
                    return self.cli_library_output("login");
                }
                CatalogSignIn::CliFallback { ref agents, .. } if agents.is_empty() => {
                    return self.cli_library_output("login");
                }
                CatalogSignIn::CliSetupRequired { .. } => {
                    return failed(
                        "This server only accepts sign-in from Claude Code or Codex: add it in that CLI",
                    );
                }
                CatalogSignIn::Failed { message } => return failed(message),
                _ => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return failed("the sign-in waited 15 minutes for the browser and stopped");
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
            state = match self.get_plugin_sign_in_status(&row.id).await {
                Ok(state) => state,
                Err(error) => return failed(error.to_string()),
            };
        }
    }
}
