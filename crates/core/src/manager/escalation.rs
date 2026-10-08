//! A Claude thread's `run_unsandboxed` under Approve for me (THREAD-PLAN.md Q4, Q6).
//!
//! Claude's own auto mode declines every command that asks to leave its sandbox, and Codex's
//! auto-reviewer only sees Codex's own shell. So for a Claude thread at Approve for me,
//! Brigadier decides: a one-shot reviewer on the thread's vendor (read-only, no tools, nobody to
//! ask) reads the command and answers in JSON. It allows the command unless it pushes,
//! publishes or deploys, uses credentials, spends money, or deletes or writes outside the
//! work; an allowed command runs once and is logged as decided for the user. Anything else (an
//! unsafe verdict, no verdict, a reviewer that couldn't run) becomes a card the call waits on,
//! as under Ask for approval. The reviewer's tokens are metered as their own step.

use std::path::Path;
use std::time::Duration;

use brigadier_providers::{
    Access, AllowedModels, ApprovalDecision, ApprovalKind, ApprovalRequest, Origin, ProviderEvent,
    ProviderKind, Role, SessionSpec, Started, ToolSet, TurnInput, TurnStatus,
};
use serde::Deserialize;

use super::SessionManager;
use super::cards::CardAnswer;
use super::usage::TokenOwner;
use crate::model::ConversationId;
use crate::routing::TokenMeter;
use crate::work::{ApprovalSubject, DecisionSource, TaskId};
use crate::{Error, Result, now_ms};

/// The reviewer's model and effort (the user's ruling, 2026-10-07).
const REVIEWER_MODEL: &str = "claude-opus-5-5";
const REVIEWER_EFFORT: &str = "low";
/// How long the reviewer may take before the user decides instead.
const REVIEW_TIME: Duration = Duration::from_secs(120);
/// The id of an approval Brigadier asks itself (no CLI waits on it): its answer goes to the
/// `run_unsandboxed` call waiting on the card.
pub(crate) const OWN_APPROVAL: &str = "brigadier-escalation:";
/// Under which owner the reviewer's tokens are counted ([`super::usage`] reads the prefix).
pub(crate) const ESCALATION_OWNER: &str = "escalation:";

const ROLE: &str = "You review one shell command a coding agent wants to run outside its sandbox, on the user's behalf, while the user is away. You have no tools: decide from the command alone.\n\nAllow it unless it does any of these: pushes to a remote, publishes or releases anything, deploys, uses or reads credentials, tokens or keys, spends money, or deletes or writes outside the project's folder and its own temporary folders. Network reads (downloads, package installs), builds, tests and starting local servers are fine.\n\nReply with one line of JSON and nothing else: {\"allow\": true or false, \"reason\": \"one short sentence\"}.";

#[derive(Deserialize)]
struct Verdict {
    allow: bool,
    #[serde(default)]
    reason: String,
}

impl SessionManager {
    /// Whether a Claude thread at Approve for me may run `command` in `workdir` outside its
    /// sandbox: Brigadier's reviewer allows it, or else the user does on a card. `Err` is the
    /// refusal the model gets.
    pub(crate) async fn decide_escalation(
        &self,
        id: &ConversationId,
        command: &str,
        workdir: &Path,
        workspace: Option<&Path>,
    ) -> Result<()> {
        let verdict = self
            .review_escalation(id, command, workdir, workspace)
            .await;
        let why = match verdict {
            Ok(verdict) if verdict.allow => {
                let request = self.request_for(id, None).await;
                self.decided_for_you(
                    id,
                    request,
                    DecisionSource::Orchestrator,
                    format!("Ran outside the sandbox: {command}"),
                    format!("Brigadier's reviewer allowed it: {}", verdict.reason),
                )
                .await;
                return Ok(());
            }
            Ok(verdict) => format!(
                "Brigadier's reviewer would not allow it: {}",
                verdict.reason
            ),
            Err(err) => {
                tracing::warn!(conversation = %id, error = %err, "the escalation reviewer gave no verdict");
                format!("Brigadier's reviewer gave no verdict ({err})")
            }
        };
        let request = ApprovalRequest {
            id: format!("{OWN_APPROVAL}{}", uuid::Uuid::now_v7()),
            kind: ApprovalKind::Command,
            tool: super::run::RUN_UNSANDBOXED.into(),
            command: Some(command.to_owned()),
            cwd: Some(workdir.display().to_string()),
            paths: Vec::new(),
            reason: Some(why),
            escalation: true,
            input: None,
            grant: None,
        };
        let (_, answer) = self
            .open_approval(id, None, ApprovalSubject::Cli { request })
            .await?;
        match answer.await {
            Ok(CardAnswer::Decision(ApprovalDecision::Allow | ApprovalDecision::AllowSimilar)) => {
                Ok(())
            }
            Ok(CardAnswer::Decision(ApprovalDecision::Deny { message })) if !message.is_empty() => {
                Err(Error::Invalid(format!(
                    "The user declined running it outside the sandbox: {message}"
                )))
            }
            _ => Err(Error::Invalid(
                "The user declined running it outside the sandbox.".into(),
            )),
        }
    }

    /// The reviewer's verdict on `command`.
    async fn review_escalation(
        &self,
        id: &ConversationId,
        command: &str,
        workdir: &Path,
        workspace: Option<&Path>,
    ) -> std::result::Result<Verdict, String> {
        let counted_as = TaskId(format!("{ESCALATION_OWNER}{}", uuid::Uuid::now_v7()));
        let owner = counted_as.0.clone();
        let home = self.owned_dir("escalation", &counted_as.0[ESCALATION_OWNER.len()..]);
        self.prepare_owned_dir(&owner, &home)
            .await
            .map_err(|err| err.to_string())?;
        let spec = SessionSpec {
            cwd: home,
            model: Some(REVIEWER_MODEL.into()),
            effort: Some(REVIEWER_EFFORT.into()),
            fast: false,
            origin: Origin::New,
            access: Access::ReadOnly,
            append_system_prompt: Some(ROLE.into()),
            mcp_servers: Vec::new(),
            tools: ToolSet::None,
            add_dirs: Vec::new(),
            env: Vec::new(),
            unset_env: Vec::new(),
            low_priority: false,
            record_to: None,
            redactor: None,
            owned_cwd: true,
            auto_compact: false,
            allowed_models: Some(AllowedModels::default()),
            auto_review: false,
            omit_ai_coauthors: false,
            output_hook: None,
        };
        let account = self
            .runtime
            .launch_account(ProviderKind::Claude, Some(REVIEWER_MODEL));
        let meter = TokenMeter::default().on_account(account.account.clone());
        meter.turn_started(now_ms());
        let started = self.runtime.start_hosted(&owner, &account, spec).await;
        let reply = match started {
            Ok(Started {
                session,
                mut events,
            }) => {
                let prompt = format!(
                    "The project's folder: {}\nThe command runs in: {}\nThe command:\n```\n{command}\n```",
                    workspace.map_or_else(|| "(none)".into(), |path| path.display().to_string()),
                    workdir.display()
                );
                let turn = async {
                    session
                        .send(TurnInput::text(prompt))
                        .await
                        .map_err(|err| err.to_string())?;
                    let mut last = String::new();
                    while let Some(event) = events.recv().await {
                        match event {
                            ProviderEvent::Message {
                                role: Role::Assistant,
                                text,
                                ..
                            } => last = text,
                            ProviderEvent::Usage {
                                total,
                                last: latest,
                            } => {
                                self.note_tokens(
                                    &meter,
                                    ProviderKind::Claude,
                                    Some(REVIEWER_MODEL),
                                    TokenOwner::Task(id, &counted_as),
                                    &total,
                                    latest.as_ref(),
                                )
                                .await;
                            }
                            ProviderEvent::ApprovalRequested { request } => {
                                let _ = session
                                    .answer(
                                        request.id,
                                        ApprovalDecision::Deny {
                                            message: "The reviewer has no tools.".into(),
                                        },
                                    )
                                    .await;
                            }
                            ProviderEvent::TurnCompleted { status, .. } => {
                                return match status {
                                    TurnStatus::Completed => Ok(last),
                                    _ => Err("its turn did not finish".to_owned()),
                                };
                            }
                            ProviderEvent::Exited { .. } => {
                                return Err("its CLI exited".to_owned());
                            }
                            _ => {}
                        }
                    }
                    Err("its CLI went away".to_owned())
                };
                let reply = tokio::time::timeout(REVIEW_TIME, turn)
                    .await
                    .unwrap_or_else(|_| Err("it ran out of time".to_owned()));
                session.close().await;
                reply
            }
            Err(err) => Err(format!("its CLI didn't start: {err}")),
        };
        self.runtime.ledger().dispose(&owner).await;
        verdict(&reply?)
    }
}

/// The JSON verdict in the reviewer's reply (the first `{` to the last `}`).
fn verdict(reply: &str) -> std::result::Result<Verdict, String> {
    let (Some(start), Some(end)) = (reply.find('{'), reply.rfind('}')) else {
        return Err(format!("no JSON in its reply: {reply}"));
    };
    if end < start {
        return Err(format!("no JSON in its reply: {reply}"));
    }
    serde_json::from_str(&reply[start..=end]).map_err(|err| format!("{err}: {reply}"))
}

#[cfg(test)]
mod tests {
    use super::verdict;

    #[test]
    fn a_verdict_is_read_from_the_reply_s_json() {
        let allowed = verdict("{\"allow\": true, \"reason\": \"A build.\"}").unwrap();
        assert!(allowed.allow);
        assert_eq!(allowed.reason, "A build.");
        let fenced =
            verdict("```json\n{\"allow\": false, \"reason\": \"It pushes.\"}\n```").unwrap();
        assert!(!fenced.allow);
        assert!(verdict("Sure, allowed.").is_err());
        assert!(verdict("} {").is_err());
        assert!(verdict("{\"reason\": \"no verdict\"}").is_err());
    }
}
