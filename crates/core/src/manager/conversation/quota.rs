//! Durable, launch-confirmed recovery notices. Intent survives a crash; the board owns
//! confirmed history, so reconnects cannot turn a queued selection into a completed move.
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{Cli, ConvLive};
use crate::manager::SessionManager;
use crate::model::{ConversationId, ModelChoice};
use crate::work::{OrchestratorStepKind, ProviderTransitionCause};

#[derive(Serialize, Deserialize)]
struct TransitionIntent {
    id: String,
    cause: ProviderTransitionCause,
    from: ModelChoice,
    to: ModelChoice,
    request: Option<String>,
}

impl SessionManager {
    // A cancellation tombstone is separate from the database write. If clearing the wait
    // fails, restart recovery still knows that this particular waiting request was stopped.
    pub(in crate::manager) async fn cancel_quota_recovery(
        &self,
        id: &ConversationId,
    ) -> crate::Result<()> {
        let conv = self.conv(id)?;
        let _transitioning = conv.transition.lock().await;
        if let Some(wait) = self.core.conversation(id)?.quota_wait {
            let path = self.session_dir(id).record().join("quota-cancelled.json");
            crate::manager::blocking(move || {
                let bytes = serde_json::to_vec(&(wait.since_ms, wait.request))
                    .map_err(|err| crate::Error::Invalid(err.to_string()))?;
                super::super::session_dir::write_atomic(&path, &bytes)
            })
            .await?;
            self.core.set_conversation_wait(id, None).await?;
        }
        self.clear_provider_transition(id).await
    }

    pub(super) async fn quota_was_cancelled(
        &self,
        id: &ConversationId,
        wait: &crate::work::QuotaWait,
    ) -> bool {
        let path = self.session_dir(id).record().join("quota-cancelled.json");
        let token = (wait.since_ms, wait.request.clone());
        crate::manager::blocking(move || {
            Ok(std::fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<(i64, Option<String>)>(&bytes).ok())
                .is_some_and(|cancelled| cancelled == token))
        })
        .await
        .unwrap_or(true)
    }
    pub(super) async fn fail_quota_handover(&self, conv: &Arc<ConvLive>, message: &str) {
        let request = {
            let mut state = conv.state.lock().await;
            let request = state.hand_over.take().and_then(|over| over.request);
            if let Some(request) = &request {
                state.outcomes.insert(
                    request.clone(),
                    crate::work::RequestState::Failed {
                        error: message.into(),
                    },
                );
            }
            state.busy = false;
            state.awaiting_start = false;
            request
        };
        let _ = self.clear_provider_transition(&conv.id).await;
        self.set_run_for(
            &conv.id,
            crate::work::RunState::Failed,
            Some(message.into()),
            request,
        )
        .await;
        self.settle_requests(&conv.id).await;
    }

    pub(super) fn next_quota_reset(&self, conv: &ConvLive, from: &ModelChoice) -> Option<i64> {
        let settings = self.core.settings();
        let mut choices = vec![from.clone()];
        if settings.switch_providers && self.seam_on(conv) {
            choices.extend(self.fallback_rows().into_iter().filter(|choice| {
                choice.provider != from.provider
                    && crate::routing::availability::provider_on(&settings, choice.provider)
                    && self
                        .model_refusal(
                            choice.provider,
                            choice.model.as_deref(),
                            choice.effort.as_deref(),
                            Some(crate::manager::models::FALLBACK_WORK),
                        )
                        .is_ok_and(|refusal| refusal.is_none())
            }));
        }
        choices
            .into_iter()
            .flat_map(|choice| {
                self.runtime
                    .account_candidates(choice.provider)
                    .into_iter()
                    .filter(|candidate| candidate.logged_in)
                    .filter_map(move |candidate| {
                        candidate.quota.as_ref().and_then(|quota| {
                            crate::accounts::ready_again_at(quota, choice.model.as_deref())
                        })
                    })
            })
            .filter(|at| *at > crate::now_ms())
            .min()
    }

    pub(in crate::manager) async fn model_selected(
        &self,
        id: &ConversationId,
        from: ModelChoice,
        to: ModelChoice,
    ) -> crate::Result<()> {
        let conv = self.conv(id)?;
        let _retrying = conv.retry.lock().await;
        let (waiting, stopped, request) = {
            let state = conv.state.lock().await;
            (
                state.waiting,
                state.interrupted,
                state
                    .waited_request
                    .clone()
                    .or_else(|| state.request.clone()),
            )
        };
        if stopped {
            return Ok(());
        }
        self.prepare_provider_transition(
            &conv,
            from,
            to.clone(),
            ProviderTransitionCause::ManualSwitch,
            request,
        )
        .await;
        if waiting && self.choice_available(&to).await {
            self.stop_waiting(&conv).await?;
            // A limited lead's native jobs cannot migrate to the new CLI. Independent
            // workers are owned separately and keep running.
            conv.close_cli().await;
        }
        self.kick(&conv);
        Ok(())
    }

    pub(in crate::manager) async fn prepare_provider_transition(
        &self,
        conv: &ConvLive,
        from: ModelChoice,
        to: ModelChoice,
        cause: ProviderTransitionCause,
        request: Option<String>,
    ) {
        let _transitioning = conv.transition.lock().await;
        let path = self
            .session_dir(&conv.id)
            .record()
            .join("quota-transition.json");
        let mut from = from;
        {
            let state = conv.state.lock().await;
            let account = state
                .cli
                .as_ref()
                .map(|cli| &cli.account)
                .or(state.waited_account.as_ref());
            if let Some(account) = account.filter(|account| account.provider == from.provider) {
                from.account = Some(account.choice_id());
            }
        }
        let intent = TransitionIntent {
            id: uuid::Uuid::new_v4().to_string(),
            cause,
            from,
            to,
            request,
        };
        let result = crate::manager::blocking(move || {
            let bytes = serde_json::to_vec(&intent)
                .map_err(|err| crate::Error::Invalid(err.to_string()))?;
            super::super::session_dir::write_atomic(&path, &bytes)
        })
        .await;
        if let Err(err) = result {
            tracing::warn!(conversation = %conv.id, error = %err, "could not keep provider transition intent");
        }
    }

    pub(in crate::manager) async fn clear_provider_transition(
        &self,
        id: &ConversationId,
    ) -> crate::Result<()> {
        let path = self.session_dir(id).record().join("quota-transition.json");
        crate::manager::blocking(move || match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(crate::Error::Invalid(format!(
                "could not cancel provider transition: {err}"
            ))),
        })
        .await
    }

    pub(in crate::manager) async fn confirm_provider_transition(
        &self,
        conv: &Arc<ConvLive>,
        cli: &Cli,
    ) {
        let _transitioning = conv.transition.lock().await;
        let current = {
            let state = conv.state.lock().await;
            !state.interrupted
                && state.transition_send_accepted
                && state.transition_turn_started
                && state
                    .cli
                    .as_ref()
                    .is_some_and(|now| std::ptr::eq(now.as_ref(), cli))
        };
        if !current {
            return;
        }
        let path = self
            .session_dir(&conv.id)
            .record()
            .join("quota-transition.json");
        let intent = crate::manager::blocking(move || {
            Ok(std::fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<TransitionIntent>(&bytes).ok()))
        })
        .await
        .ok()
        .flatten();
        let Some(intent) = intent else {
            return;
        };
        if intent.to.provider != cli.provider
            || intent.to.model != cli.model.model
            || intent
                .to
                .account
                .as_ref()
                .is_some_and(|account| account != &cli.account.choice_id())
        {
            return;
        }
        let already = self.core.board(&conv.id).await.is_ok_and(|board| board.orchestrator_steps.iter()
            .any(|step| matches!(&step.kind, OrchestratorStepKind::ProviderTransition { id, .. } if id == &intent.id)));
        if !already {
            let mut to = cli.model.clone();
            to.account = Some(cli.account.choice_id());
            self.orchestrator_step_in(
                &conv.id,
                intent.request,
                OrchestratorStepKind::ProviderTransition {
                    task_id: None,
                    id: intent.id,
                    cause: intent.cause,
                    from: intent.from,
                    to,
                    account: cli.account.clone(),
                    account_label: self.runtime.account_label(&cli.account),
                    started_at_ms: crate::now_ms(),
                },
            )
            .await;
        }
        if let Err(err) = self.clear_provider_transition(&conv.id).await {
            tracing::error!(conversation = %conv.id, error = %err, "could not clear confirmed transition");
        }
    }
}
