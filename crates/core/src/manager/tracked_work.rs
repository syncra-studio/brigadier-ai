//! One append-only stream per owner: replay, forgetting and completion survive restarts.
use super::SessionManager;
use crate::{ConversationId, PrHistoryEvent, Result, TrackedPullRequest, TrackedWork, WaitingOn};
use brigadier_store::{NewEvent, StreamPage};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(super) enum Change {
    Pr {
        pr: TrackedPullRequest,
        event: Option<PrHistoryEvent>,
    },
    Wait {
        wait: WaitingOn,
    },
}
impl TrackedWork {
    pub(super) fn apply(&mut self, change: Change) {
        match change {
            Change::Pr { pr, event } => {
                if let Some(old) = self
                    .pull_requests
                    .iter_mut()
                    .find(|p| p.identity == pr.identity)
                {
                    *old = pr;
                } else {
                    self.pull_requests.push(pr);
                }
                if let Some(event) = event
                    && !self.pr_history.iter().any(|e| e.id == event.id)
                {
                    self.pr_history.push(event);
                }
            }
            Change::Wait { wait } => {
                if let Some(old) = self.waiting_on.iter_mut().find(|w| w.id == wait.id) {
                    *old = wait;
                } else {
                    self.waiting_on.push(wait);
                }
            }
        }
    }
}
impl SessionManager {
    pub async fn tracked_work(&self, id: &ConversationId) -> Result<TrackedWork> {
        self.core.conversation(id)?;
        let mut before = None;
        let mut records = Vec::new();
        loop {
            let page = self
                .core
                .store()
                .read_stream(
                    format!("tracked-work:{id}"),
                    StreamPage {
                        before,
                        kinds: vec![],
                        limit: 500,
                    },
                )
                .await?;
            if page.is_empty() {
                break;
            }
            before = page.last().map(|e| e.stream_seq);
            records.extend(page);
        }
        let mut work = TrackedWork::default();
        for record in records.into_iter().rev() {
            let change = serde_json::from_str(record.payload.get()).map_err(|source| {
                crate::Error::Corrupt {
                    seq: record.stream_seq,
                    source,
                }
            })?;
            work.apply(change);
        }
        work.pr_history.sort_by(|a, b| b.at_ms.cmp(&a.at_ms));
        Ok(work)
    }
    pub(super) async fn keep_tracked(&self, id: &ConversationId, change: &Change) -> Result<()> {
        let event = NewEvent::new(
            format!("tracked-work:{id}"),
            "tracked.changed",
            crate::now_ms(),
            change,
        )?;
        self.core.store().append(vec![event]).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PrIdentity;
    fn pr() -> TrackedPullRequest {
        TrackedPullRequest {
            identity: PrIdentity::parse("https://github.com/org/repo/pull/7").unwrap(),
            account: Some("alice".into()),
            viewer: None,
            pull_request: None,
            touched_at_ms: 1,
            observed_at_ms: None,
            actor: "worker".into(),
            action: "opened".into(),
            status: "Checking status".into(),
            complete: false,
            forgotten: false,
        }
    }
    #[test]
    fn replay_is_deduped_and_forget_survives() {
        let mut work = TrackedWork::default();
        let mut p = pr();
        work.apply(Change::Pr {
            pr: p.clone(),
            event: None,
        });
        work.apply(Change::Pr {
            pr: p.clone(),
            event: None,
        });
        p.forgotten = true;
        work.apply(Change::Pr { pr: p, event: None });
        assert_eq!(work.pull_requests.len(), 1);
        assert!(work.pull_requests[0].forgotten);
    }
    #[test]
    fn canonical_identity_rejects_credentials_and_non_prs() {
        for url in [
            "https://token@github.com/org/repo/pull/7",
            "https://github.com/org/repo/issues/7",
            "https://github.com/org/repo/pull/7?token=x",
            "http://github.com/org/repo/pull/7",
        ] {
            assert!(PrIdentity::parse(url).is_none());
        }
        assert_eq!(
            PrIdentity::parse("https://GitHub.com/Org/Repo/pull/7/")
                .unwrap()
                .key(),
            "github.com/org/repo/7"
        );
    }
}

impl SessionManager {
    pub(super) async fn cli_tracked_work(
        &self,
        caller: &super::CliCaller,
        tool: brigadier_cli::protocol::Tool,
        action: &str,
        given: serde_json::Value,
    ) -> brigadier_cli::protocol::CliReply {
        use crate::tools::Role;
        use brigadier_cli::protocol::{CliReply, ErrorCode, Tool};
        // Scope always comes from the authenticated caller, never CLI arguments.
        let allowed = matches!(
            self.grants.resolve(&caller.grant),
            Some(Role::Orchestrator { .. } | Role::Worker { .. } | Role::Chat { .. })
        );
        if !allowed {
            return CliReply::error(ErrorCode::Refused, "not available in this caller scope");
        }
        let id = &caller.conversation_id;
        let actor = caller
            .task_id
            .as_ref()
            .map(|t| format!("worker {}", t.0))
            .unwrap_or_else(|| "session".into());
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct PrArgs {
            value: String,
            account: Option<String>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Done {
            id: String,
            #[serde(default)]
            note: Option<String>,
        }
        let result: crate::Result<(String, serde_json::Value)> = async {
            match (tool, action) {
                (Tool::Pr, "watch") => {
                    let a: PrArgs = serde_json::from_value(given).map_err(|_| {
                        crate::Error::Invalid("watch needs a PR URL and optional account".into())
                    })?;
                    let p = self
                        .watch_pull_request(id, &a.value, a.account, &actor)
                        .await?;
                    Ok((
                        format!("Watching {}", p.identity.url()),
                        serde_json::to_value(p).unwrap(),
                    ))
                }
                (Tool::Pr, "forget") => {
                    let a: PrArgs = serde_json::from_value(given).map_err(|_| {
                        crate::Error::Invalid("forget needs a PR key or URL".into())
                    })?;
                    if a.account.is_some() {
                        return Err(crate::Error::Invalid("forget takes no account".into()));
                    }
                    self.forget_pull_request(id, &a.value).await?;
                    Ok((
                        "Pull request forgotten; history retained".into(),
                        serde_json::json!({}),
                    ))
                }
                (Tool::Pr, "list") => {
                    if given.as_object().is_none_or(|o| !o.is_empty()) {
                        return Err(crate::Error::Invalid("list takes no arguments".into()));
                    }
                    self.discover_pull_requests(id).await?;
                    let work = self.tracked_work(id).await?;
                    let text = work
                        .pull_requests
                        .iter()
                        .filter(|p| !p.forgotten)
                        .map(|p| format!("{} · {}", p.identity.key(), p.status))
                        .collect::<Vec<_>>()
                        .join("\n");
                    Ok((text, serde_json::to_value(work).unwrap()))
                }
                (Tool::Waiting, "add") => {
                    let a = serde_json::from_value(given)
                        .map_err(|_| crate::Error::Invalid("invalid wait arguments".into()))?;
                    let w = self.add_waiting_on(id, &actor, a).await?;
                    Ok((
                        format!("{} · {}", w.id, w.text),
                        serde_json::to_value(w).unwrap(),
                    ))
                }
                (Tool::Waiting, "done") => {
                    let a: Done = serde_json::from_value(given)
                        .map_err(|_| crate::Error::Invalid("done needs one wait ID".into()))?;
                    let w = self
                        .complete_waiting_on_with_note(id, &a.id, a.note)
                        .await?;
                    Ok((
                        format!("Done · {}", w.text),
                        serde_json::to_value(w).unwrap(),
                    ))
                }
                (Tool::Waiting, "list") => {
                    if given.as_object().is_none_or(|o| !o.is_empty()) {
                        return Err(crate::Error::Invalid("list takes no arguments".into()));
                    }
                    let work = self.tracked_work(id).await?;
                    let text = work
                        .waiting_on
                        .iter()
                        .filter(|w| w.done_at_ms.is_none())
                        .map(|w| {
                            format!(
                                "{} · {}{}",
                                w.id,
                                w.text,
                                w.by.as_ref()
                                    .map(|d| format!(" · by {d} ({})", w.timezone))
                                    .unwrap_or_default()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    Ok((text, serde_json::json!({"waitingOn":work.waiting_on})))
                }
                _ => Err(crate::Error::Invalid("unknown tracking action".into())),
            }
        }
        .await;
        match result {
            Ok((text, data)) => {
                CliReply::ok(&serde_json::json!({"action":action,"text":text,"data":data}))
            }
            Err(err) => CliReply::error(
                if matches!(err, crate::Error::Invalid(_)) {
                    ErrorCode::Usage
                } else if matches!(err, crate::Error::NotFound(_)) {
                    ErrorCode::NotFound
                } else {
                    ErrorCode::Failed
                },
                err.to_string(),
            ),
        }
    }
}

#[cfg(test)]
mod durable_tests {
    use super::super::flow::{Flow, Options, Reply};
    use std::sync::Arc;
    #[tokio::test]
    async fn waits_and_forget_survive_restart_and_done_is_idempotent() {
        let mut flow = Flow::start(
            "tracked-work-durable",
            Options::default(),
            Arc::new(|_| Box::pin(async { Reply::text("Done") })),
        )
        .await;
        let id = flow.conversation.clone();
        let wait = flow
            .manager
            .add_waiting_on(
                &id,
                "worker 1",
                crate::AddWaitingOn {
                    text: "Michael checks ticket 2552".into(),
                    by: Some("2026-10-28".into()),
                    timezone: Some("Europe/Chisinau".into()),
                    then: None,
                    link: Some("https://jira.example.com/browse/T-2552".into()),
                },
            )
            .await
            .unwrap();
        let p = flow
            .manager
            .watch_pull_request(
                &id,
                "https://github.com/org/repo/pull/7",
                Some("org-user".into()),
                "worker 1",
            )
            .await
            .unwrap();
        flow.manager
            .forget_pull_request(&id, &p.identity.key())
            .await
            .unwrap();
        let first = flow
            .manager
            .complete_waiting_on(&id, &wait.id)
            .await
            .unwrap();
        let again = flow
            .manager
            .complete_waiting_on(&id, &wait.id)
            .await
            .unwrap();
        assert_eq!(first, again);
        flow.restart().await;
        let work = flow.manager.tracked_work(&id).await.unwrap();
        assert_eq!(work.waiting_on, vec![first]);
        assert!(work.pull_requests[0].forgotten);
        assert_eq!(work.pr_history.len(), 2);
        assert!(
            flow.manager
                .pending_waiting_wakes(&id, i64::MAX)
                .await
                .unwrap()
                .is_empty()
        );
        let other = flow
            .manager
            .create_conversation(
                crate::ConversationKind::Chat,
                None,
                Some("Other".into()),
                None,
            )
            .await
            .unwrap();
        assert!(
            flow.manager
                .complete_waiting_on(&other.id, &wait.id)
                .await
                .is_err()
        );
        assert!(
            flow.manager
                .forget_pull_request(&other.id, &p.identity.key())
                .await
                .is_err()
        );
        flow.stop().await;
    }
}
