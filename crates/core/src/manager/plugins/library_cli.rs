//! `brigadier library list|add|remove|login`: data-dir library content, never CLI installs.
//! Workers can list; user sessions can change the library.

use std::path::Path;
use std::time::Duration;

use brigadier_cli::protocol::{
    CliReply, ErrorCode, LibraryAgentResult, LibraryItem, LibraryOutput,
};
use serde::Deserialize;
use serde_json::Value;

use crate::manager::SessionManager;
use crate::manager::cli::CliCaller;
use crate::manager::mcp_login::{AgentSignInState, McpSignInProgress};

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

fn legacy_login_item(row: &super::LibraryRow) -> LibraryItem {
    LibraryItem {
        name: row.name.clone(),
        kind: tag(row.kind),
        detail: row.detail.clone(),
        everywhere: row.everywhere,
        on_in: row.on_in.clone(),
        off_in: row.off_in.clone(),
        agents: row
            .agents
            .iter()
            .map(
                |(agent, state)| brigadier_cli::protocol::LibraryAgentState {
                    agent: tag(super::provider(*agent)),
                    state: tag(state),
                },
            )
            .collect(),
        attention: row.attention.clone(),
    }
}

fn sign_in_agents(progress: &McpSignInProgress) -> Vec<LibraryAgentResult> {
    progress
        .agents
        .iter()
        .map(|agent| {
            let (outcome, message) = match &agent.state {
                AgentSignInState::SignedIn => ("done", None),
                AgentSignInState::AlreadySignedIn => ("done", Some("Already signed in.".into())),
                AgentSignInState::Failed { reason } => ("failed", Some(reason.clone())),
                AgentSignInState::Cancelled => ("failed", Some("Cancelled.".into())),
                _ => ("skipped", None),
            };
            LibraryAgentResult {
                agent: tag(agent.agent),
                outcome: outcome.to_owned(),
                message,
            }
        })
        .collect()
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
    async fn cli_library_login(&self, project: Option<String>, name: String) -> CliReply {
        let rows = match self.library_rows(project.clone()).await {
            Ok(rows) => rows,
            Err(err) => return failed(err.to_string()),
        };
        let Some(entry) = rows.iter().find(|entry| entry.name == name) else {
            return failed(format!("{name} isn't in your Library"));
        };
        let Some((server, found_in)) = entry.server_id.clone() else {
            return failed(format!("{name} doesn't sign in"));
        };
        // The catalog that has the server: the project's, when only it does.
        let project = found_in.or(project);
        let Some(open) = self.sign_in_browser.get().cloned() else {
            return failed("Brigadier can't open a browser here");
        };
        let id = match self
            .sign_in_mcp_server(&server, project.map(crate::model::ProjectId), open)
            .await
        {
            Ok(id) => id,
            Err(err) => return failed(err.to_string()),
        };
        let (logins, waited) = (self.mcp_logins.clone(), id.clone());
        let progress =
            match tokio::task::spawn_blocking(move || logins.finished(&waited, SIGN_IN_WAIT)).await
            {
                Ok(Some(progress)) if progress.finished => progress,
                Ok(Some(_)) => {
                    let _ = self.cancel_mcp_sign_in(&id);
                    return failed("the sign-in waited 15 minutes for the browser and stopped");
                }
                Ok(None) => return failed(format!("no sign-in `{id}`")),
                Err(err) => return failed(format!("the sign-in stopped: {err}")),
            };
        CliReply::ok(&LibraryOutput {
            action: "login".to_owned(),
            items: rows.iter().map(legacy_login_item).collect(),
            agents: sign_in_agents(&progress),
            warning: None,
        })
    }
}
