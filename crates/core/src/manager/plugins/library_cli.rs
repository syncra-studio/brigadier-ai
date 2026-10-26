//! `brigadier library list|add|remove|login` (STRIP-DOWN-PLAN X7): the Library as Settings ›
//! Plugins has it, from a session's own shell. The changes run as the page's jobs, through
//! each agent's own setup; a worker can only list.

use std::path::Path;
use std::time::Duration;

use brigadier_cli::protocol::{
    CliReply, ErrorCode, LibraryAgentResult, LibraryAgentState, LibraryItem, LibraryOutput,
};
use serde::Deserialize;
use serde_json::Value;

use super::LibraryRow;
use super::ops::{PluginJobView, PluginStep};
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

fn item(row: &LibraryRow) -> LibraryItem {
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
            .map(|(agent, state)| LibraryAgentState {
                agent: tag(super::provider(*agent)),
                state: tag(state),
            })
            .collect(),
        attention: row.attention.clone(),
    }
}

fn library_items(rows: &[LibraryRow]) -> Vec<LibraryItem> {
    rows.iter().map(item).collect()
}

fn job_agents(job: &PluginJobView) -> Vec<LibraryAgentResult> {
    job.agents
        .iter()
        .map(|agent| {
            let (outcome, message) = match &agent.step {
                PluginStep::Done => ("done", None),
                PluginStep::Failed { reason } => ("failed", Some(reason.clone())),
                PluginStep::Skipped { reason } => ("skipped", Some(reason.clone())),
                PluginStep::Waiting | PluginStep::Working => ("skipped", None),
            };
            LibraryAgentResult {
                agent: tag(agent.agent),
                outcome: outcome.to_owned(),
                message,
            }
        })
        .collect()
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
            "list" => self.library_rows(project).await.map(|rows| LibraryOutput {
                action: action.to_owned(),
                items: library_items(&rows),
                agents: Vec::new(),
                warning: None,
            }),
            "add" => {
                let text = args.text.unwrap_or_default();
                let job = match self.add_plugin_as(
                    text,
                    args.name,
                    project.clone().map(|id| vec![id]),
                    args.shared,
                    None,
                ) {
                    Ok(id) => self.finished_job(&id).await,
                    Err(err) => return failed(err.to_string()),
                };
                if let Some(error) = &job.error {
                    return failed(error.clone());
                }
                self.library_rows(project).await.map(|rows| LibraryOutput {
                    action: action.to_owned(),
                    items: library_items(&rows)
                        .into_iter()
                        .filter(|item| job.keys.contains(&format!("lib:{}", item.name)))
                        .collect(),
                    agents: job_agents(&job),
                    warning: job.warning.clone(),
                })
            }
            "remove" => {
                let name = args.name.unwrap_or_default();
                if self.library().entry(&name).is_none() {
                    return failed(format!("{name} isn't in your Library"));
                }
                let job = self
                    .finished_job(&self.remove_plugin(format!("lib:{name}")))
                    .await;
                if let Some(error) = &job.error {
                    return failed(error.clone());
                }
                Ok(LibraryOutput {
                    action: action.to_owned(),
                    items: Vec::new(),
                    agents: job_agents(&job),
                    warning: job.warning.clone(),
                })
            }
            "login" => {
                return self
                    .cli_library_login(project, args.name.unwrap_or_default())
                    .await;
            }
            other => return CliReply::error(ErrorCode::Usage, format!("no action {other:?}")),
        };
        match answer {
            Ok(output) => CliReply::ok(&output),
            Err(err) => failed(err.to_string()),
        }
    }

    /// Plugin job `id`, once it finished.
    async fn finished_job(&self, id: &str) -> PluginJobView {
        loop {
            match self.plugin_job_view(id) {
                Ok(job) if job.finished => return job,
                Ok(_) => tokio::time::sleep(Duration::from_millis(250)).await,
                Err(err) => {
                    return PluginJobView {
                        id: id.to_owned(),
                        agents: Vec::new(),
                        finished: true,
                        error: Some(err.to_string()),
                        warning: None,
                        keys: Vec::new(),
                    };
                }
            }
        }
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
            items: Vec::new(),
            agents: sign_in_agents(&progress),
            warning: None,
        })
    }
}
