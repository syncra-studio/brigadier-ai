//! A CLI's own subagent asks for a permission (D69): a short-lived fork of the session that
//! started it, with all of that session's context, answers ALLOW or DENY with a reason. The user
//! never sees it; Brigadier only carries the question and the answer, and deletes the fork.
//! Each CLI's adapter marks such a request ([`ApprovalRequest::agent`]); nothing here is per CLI.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use brigadier_providers::{
    Access, ApprovalDecision, ApprovalRequest, Origin, ProviderEvent, Role, Started, TurnInput,
};

use super::SessionManager;
use super::conversation::Cli;
use crate::model::ConversationId;
use crate::work::{CardId, TaskId};
use crate::{Error, Result};

/// The requests no fork could answer, by card: when it last failed, and why. They stay pending,
/// their subagent waits, and its row says so (D69).
pub(super) type ForkFailures = std::sync::Mutex<HashMap<CardId, (i64, String)>>;

/// How long a fork may take to answer before it counts as failed: a failure only re-asks or
/// leaves the request pending, it never answers it.
const FORK_PATIENCE: Duration = Duration::from_secs(180);

/// What the fork answers with when its own CLI asks for something: it only answers.
const FORK_ONLY_ANSWERS: &str = "You only answer the question; don't act.";

impl SessionManager {
    /// The subagent's request `request`, from the CLI `cli` runs (the thread's, or worker
    /// `task_id`'s): kept as a card only its orchestrator answers, then answered by a fork of
    /// that CLI's session. A fork that fails is asked once more; then the request stays pending.
    pub(crate) async fn ask_fork(
        &self,
        cli: &Arc<Cli>,
        conversation_id: &ConversationId,
        task_id: Option<TaskId>,
        request: ApprovalRequest,
    ) {
        let card = match self
            .open_cli_card(cli, conversation_id, task_id, request.clone())
            .await
        {
            Ok(Some(card)) => card,
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(conversation = %conversation_id, error = %err, "could not keep a subagent's request");
                return;
            }
        };
        self.judge(card.id, cli.clone(), conversation_id.clone(), request);
    }

    /// Puts card `card`'s `request` to a fork, once more if that fails, and passes on its
    /// answer. Not `async`: answering goes back through the routing that calls [`Self::ask_fork`].
    fn judge(
        &self,
        card: CardId,
        cli: Arc<Cli>,
        conversation_id: ConversationId,
        request: ApprovalRequest,
    ) {
        let manager = self.arc();
        self.spawn(async move {
            let mut failed = String::new();
            for _ in 0..2 {
                if cli.ended.is_cancelled() {
                    return;
                }
                match manager.fork_decision(&cli, &request).await {
                    Ok(decision) => {
                        manager.fork_failures.lock().unwrap().remove(&card);
                        if let Err(err) = manager
                            .answer_card(conversation_id.clone(), card.clone(), decision)
                            .await
                        {
                            tracing::warn!(conversation = %conversation_id, error = %err, "could not pass on the fork's answer");
                        }
                        return;
                    }
                    Err(err) => failed = err.to_string(),
                }
            }
            tracing::warn!(conversation = %conversation_id, error = %failed, "no fork could answer a subagent's request; it waits");
            manager
                .fork_failures
                .lock()
                .unwrap()
                .insert(card, (crate::now_ms(), failed));
        });
    }

    /// One fork of `cli`'s session, asked about `request`: its answer. The fork runs read-only
    /// and is deleted, from its CLI and Brigadier, before this returns.
    async fn fork_decision(
        &self,
        cli: &Cli,
        request: &ApprovalRequest,
    ) -> Result<ApprovalDecision> {
        let owner = format!("fork:{}", uuid::Uuid::now_v7());
        let mut spec = cli.spec.clone();
        spec.origin = Origin::Fork {
            native_id: cli.session.native_id(),
        };
        // Never saved where it can be (Codex), read-only, and nothing of the parent's that
        // its cleanup could take: folders, hooks, servers, recordings.
        spec.ephemeral = true;
        spec.access = Access::ReadOnly;
        spec.mcp_servers.clear();
        spec.skill_plugins.clear();
        spec.project = None;
        spec.output_hook = None;
        spec.stop_hook = None;
        spec.native_compaction = None;
        spec.record_to = None;
        spec.owned_cwd = false;
        spec.sweep_dir = None;
        spec.keeps_detached = false;
        spec.temp_tag = None;
        let mut running = None;
        let answered = tokio::time::timeout(FORK_PATIENCE, async {
            let Started {
                session,
                mut events,
            } = self
                .runtime
                .start_hosted(&owner, &cli.account, spec)
                .await?;
            running = Some(session.clone());
            session
                .send(TurnInput::text(fork_question(request)))
                .await
                .map_err(|err| Error::Provider(format!("the fork didn't start: {err}")))?;
            let mut reply = None;
            while let Some(event) = events.recv().await {
                match event {
                    ProviderEvent::Message {
                        role: Role::Assistant,
                        text,
                        ..
                    } => reply = Some(text),
                    ProviderEvent::ApprovalRequested { request } => {
                        let deny = ApprovalDecision::Deny {
                            message: FORK_ONLY_ANSWERS.into(),
                        };
                        let _ = session.answer(request.id, deny).await;
                    }
                    ProviderEvent::Error { error } if !error.will_retry => {
                        return Err(Error::Provider(error.message));
                    }
                    ProviderEvent::TurnCompleted { .. } | ProviderEvent::Exited { .. } => break,
                    _ => {}
                }
            }
            let reply = reply.unwrap_or_default();
            verdict(&reply).ok_or_else(|| {
                Error::Provider(format!("the fork's answer wasn't ALLOW or DENY: {reply}"))
            })
        })
        .await
        .unwrap_or_else(|_| Err(Error::Provider("the fork took too long to answer".into())));
        if let Some(session) = running {
            session.close().await;
        }
        let left = self.runtime.ledger().dispose(&owner).await;
        tracing::info!(
            owner,
            answered = answered.is_ok(),
            ?left,
            "a subagent's request was put to a fork"
        );
        answered
    }
}

/// What the fork is asked: the request, which subagent asks it, and the answer's form.
fn fork_question(request: &ApprovalRequest) -> String {
    let what = request
        .command
        .clone()
        .or_else(|| (!request.paths.is_empty()).then(|| request.paths.join(", ")))
        .or_else(|| request.input.clone())
        .unwrap_or_default();
    let place = request
        .cwd
        .as_deref()
        .map(|cwd| format!(" (in {cwd})"))
        .unwrap_or_default();
    let why = request
        .reason
        .as_deref()
        .map(|reason| format!("\nIt says: {reason}"))
        .unwrap_or_default();
    format!(
        "Brigadier here, not the user. One of your own subagents ({agent}) is waiting for \
         permission to use {tool}: {what}{place}{why}\n\n\
         You are a short-lived copy of yourself, made only to answer this; you can't act. \
         Judge it as its orchestrator, against what you are working on. Reply with one line \
         only: ALLOW, or DENY: <the reason, which the subagent reads>.",
        agent = request.agent.as_deref().unwrap_or("subagent"),
        tool = request.tool,
    )
}

/// The fork's answer: its last line that says ALLOW or DENY (markdown aside), or `None`.
fn verdict(reply: &str) -> Option<ApprovalDecision> {
    reply.lines().rev().find_map(|line| {
        let line = line
            .trim()
            .trim_matches(|c: char| matches!(c, '*' | '`' | '_' | '"'));
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("ALLOW") {
            Some(ApprovalDecision::Allow)
        } else if upper.starts_with("DENY") {
            let reason = line[4..]
                .trim_start_matches(|c: char| c == ':' || c.is_whitespace() || "*`_".contains(c));
            Some(ApprovalDecision::Deny {
                message: reason.trim().to_owned(),
            })
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_forks_answer_is_its_allow_or_its_deny_with_the_reason() {
        assert_eq!(verdict("ALLOW"), Some(ApprovalDecision::Allow));
        assert_eq!(
            verdict("Looks fine.\n**ALLOW**"),
            Some(ApprovalDecision::Allow)
        );
        assert_eq!(
            verdict("DENY: a force-push to main is off-goal"),
            Some(ApprovalDecision::Deny {
                message: "a force-push to main is off-goal".into()
            })
        );
        assert_eq!(
            verdict("`DENY` not needed"),
            Some(ApprovalDecision::Deny {
                message: "not needed".into()
            })
        );
        assert_eq!(verdict("I think so"), None);
        assert_eq!(verdict(""), None);
    }
}
