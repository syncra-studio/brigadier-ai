//! Brigadier's own reviewer of a request to go beyond a sandbox, under Approve for me
//! (THREAD-PLAN.md Q4, Q6).
//!
//! - **A Claude thread's `run_unsandboxed`.** Claude's own auto mode declines every command
//!   that asks to leave its sandbox, so Brigadier decides. An allowed command runs once and is
//!   logged as decided for the user. Anything else (an unsafe verdict, no verdict, a reviewer
//!   that couldn't run) becomes a card the call waits on, as under Ask for approval.
//! - **A Codex session's escalations** (its thread's and its workers'). Codex's auto-reviewer
//!   doesn't get the rule to keep off other sessions' work (codex-cli 0.162.1 ignores
//!   `auto_review.extra_policy`, SESSION-FIXES-PLAN bug 6, "Gap left"), so Codex sends them to
//!   Brigadier (`approvalsReviewer: user`). What would touch the protected set is declined to
//!   the model with the reason; any other refusal, or no verdict, is the user's card, or
//!   declined too when nobody is there to ask (an overnight run).
//!
//! The reviewer is a one-shot session (read-only, no tools, nobody to ask) that reads the
//! request, with the session's protected set as a rule, and answers in JSON. It allows the
//! request unless it handles credentials itself, spends money, or deletes or writes outside the
//! work: pushes, pull requests, publishing and deploying on the user's own git and gh sign-in
//! are the agent's call (Q61), though that sign-in lives in the keychain. Its tokens are
//! metered as their own step.

use std::path::Path;
use std::time::Duration;

use brigadier_providers::{
    Access, AllowedModels, ApprovalDecision, ApprovalKind, ApprovalRequest, Origin, ProviderEvent,
    ProviderKind, Role, SessionSpec, Started, ToolSet, TurnInput, TurnStatus,
};
use brigadier_sandbox::Protection;
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

const ROLE: &str = "You review one request of a coding agent to go beyond its sandbox (a shell command to run outside it, files to write, or wider permissions), on the user's behalf, while the user is away. You have no tools: decide from the request alone.\n\nAllow it unless it does any of these: reads, prints, copies or stores credentials, tokens or keys itself (a token, a password, the keychain directly, a key file), signs in, spends money, or deletes or writes outside the project's folder and its own temporary folders. Pushing, opening pull requests, publishing and deploying are the agent's own call, with the git and gh sign-in the user set up (it lives in the keychain; that is fine): allow them. Network reads (downloads, package installs), builds, tests and starting local servers are fine.\n\nReply with one line of JSON and nothing else: {\"allow\": true or false, \"protected\": true only when you refuse it because a rule below forbids it, \"reason\": \"one short sentence\"}.";

#[derive(Deserialize)]
struct Verdict {
    allow: bool,
    /// A refusal because the request would touch what the session's rule protects (other
    /// sessions' work, Brigadier's own).
    #[serde(default)]
    protected: bool,
    #[serde(default)]
    reason: String,
}

/// What Brigadier's reviewer made of a Codex session's request to go beyond its sandbox.
#[derive(Debug)]
pub(crate) enum Settled {
    /// The CLI's answer.
    Answer(ApprovalDecision),
    /// The user's to answer on a card, with why the reviewer didn't settle it.
    Ask(String),
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
        protection: &Protection,
    ) -> Result<()> {
        let request = format!(
            "The command runs in: {}\nThe command:\n```\n{command}\n```",
            workdir.display()
        );
        let verdict = self
            .review_escalation(id, &request, workspace, protection)
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

    /// How a Codex session at Approve for me answers `request`, a request to go beyond its
    /// sandbox, with Brigadier's reviewer and the session's `protection` as its rule: what it
    /// allows is allowed (logged as decided for the user), and what would touch the protected
    /// set is declined to the model with the reason. Any other refusal, or no verdict (a
    /// reviewer that couldn't run, as for a user with Codex alone), is declined too when
    /// nobody is there to ask (`unattended`: an overnight run), else it is the user's card.
    /// `workspace`: the session's own folder.
    pub(crate) async fn settle_codex_escalation(
        &self,
        id: &ConversationId,
        request: &ApprovalRequest,
        workspace: Option<&Path>,
        protection: &Protection,
        unattended: bool,
    ) -> Settled {
        let what = describe(request);
        let why = match self
            .review_escalation(id, &what, workspace, protection)
            .await
        {
            Ok(verdict) if verdict.allow => {
                let asked = self.request_for(id, None).await;
                self.decided_for_you(
                    id,
                    asked,
                    DecisionSource::Orchestrator,
                    format!("Went beyond the sandbox: {}", summary(request)),
                    format!("Brigadier's reviewer allowed it: {}", verdict.reason),
                )
                .await;
                return Settled::Answer(ApprovalDecision::Allow);
            }
            Ok(verdict) if verdict.protected => {
                return Settled::Answer(ApprovalDecision::Deny {
                    message: format!(
                        "Brigadier's reviewer would not allow it: {}",
                        verdict.reason
                    ),
                });
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
        if unattended {
            Settled::Answer(ApprovalDecision::Deny {
                // The reviewer's reason is a sentence of its own, often with its full stop.
                message: format!(
                    "{}, and nobody is here to ask. Work around it, or report it.",
                    why.trim_end_matches('.')
                ),
            })
        } else {
            Settled::Ask(why)
        }
    }

    /// The reviewer's verdict on a sandbox escalation under the session's protection rules.
    async fn review_escalation(
        &self,
        id: &ConversationId,
        request: &str,
        workspace: Option<&Path>,
        protection: &Protection,
    ) -> std::result::Result<Verdict, String> {
        let counted_as = TaskId(format!("{ESCALATION_OWNER}{}", uuid::Uuid::now_v7()));
        let owner = counted_as.0.clone();
        let home = self.owned_dir("escalation", &counted_as.0[ESCALATION_OWNER.len()..]);
        self.prepare_owned_dir(&owner, &home)
            .await
            .map_err(|err| err.to_string())?;
        let spec = SessionSpec {
            cwd: home.clone(),
            model: Some(REVIEWER_MODEL.into()),
            effort: Some(REVIEWER_EFFORT.into()),
            fast: false,
            origin: Origin::New,
            ephemeral: true,
            access: Access::ReadOnly,
            append_system_prompt: Some(ROLE.into()),
            mcp_servers: Vec::new(),
            tools: ToolSet::None,
            skill_plugins: Vec::new(),
            add_dirs: Vec::new(),
            project: None,
            env: Vec::new(),
            unset_env: Vec::new(),
            low_priority: false,
            record_to: None,
            redactor: None,
            owned_cwd: true,
            sweep_dir: Some(home),
            auto_compact: false,
            allowed_models: Some(AllowedModels::default()),
            auto_review: false,
            omit_ai_coauthors: false,
            output_hook: None,
            stop_hook: None,
            deny_tools: None,
            suggestions: false,
            protection: Default::default(),
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
                let mut prompt = format!(
                    "The project's folder: {}\n{request}",
                    workspace.map_or_else(|| "(none)".into(), |path| path.display().to_string()),
                );
                // What the session's sandbox keeps it from (PLAN bug 6), which a command
                // outside it would reach.
                if let Some(rule) = brigadier_providers::approver_rule(protection) {
                    prompt.push_str(&format!("\n\n{rule}"));
                }
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
                                calls,
                            } => {
                                self.note_tokens(
                                    &meter,
                                    ProviderKind::Claude,
                                    Some(REVIEWER_MODEL),
                                    TokenOwner::Task(id, &counted_as),
                                    &total,
                                    latest.as_ref(),
                                    &calls,
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

/// A CLI's request to go beyond its sandbox as the reviewer reads it.
fn describe(request: &ApprovalRequest) -> String {
    let mut text = String::new();
    if let Some(cwd) = &request.cwd {
        text.push_str(&format!("It runs in: {cwd}\n"));
    }
    match request.kind {
        ApprovalKind::Command => text.push_str("It asks to run this command outside its sandbox"),
        ApprovalKind::FileChange => text.push_str("It asks to write files outside its sandbox"),
        ApprovalKind::Permissions => text.push_str("It asks for wider permissions"),
        ApprovalKind::Tool => text.push_str(&format!("It asks to use {}", request.tool)),
    }
    if let Some(reason) = &request.reason {
        text.push_str(&format!(" (its reason: {reason})"));
    }
    text.push(':');
    if let Some(command) = &request.command {
        text.push_str(&format!("\n```\n{command}\n```"));
        // A command run with `with_additional_permissions` (Codex): what it would reach.
        if !request.paths.is_empty() {
            text.push_str("\nWith these permissions beyond its sandbox:");
        }
    }
    for path in &request.paths {
        text.push_str(&format!("\n- {path}"));
    }
    if request.command.is_none()
        && let Some(input) = &request.input
    {
        text.push_str(&format!("\n```\n{input}\n```"));
    }
    text
}

/// One line on what was allowed, for the decisions log.
fn summary(request: &ApprovalRequest) -> String {
    match (&request.command, request.paths.first(), &request.input) {
        (Some(command), _, _) => command.clone(),
        (None, Some(path), _) => format!("write {path}"),
        (None, None, Some(input)) => input.clone(),
        (None, None, None) => request.tool.clone(),
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
