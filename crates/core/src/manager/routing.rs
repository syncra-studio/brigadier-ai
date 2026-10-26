//! What the runtime knows of models and providers now (logins, the quota monitor, the registry,
//! each CLI's model list), for whether a conversation's model can run. Brigadier routes no work
//! by it (G18).

use std::sync::Arc;

use brigadier_router::{MergedModel, ProviderState, Registry};

use super::SessionManager;
use crate::model::ModelChoice;

/// The registry and the merged catalog, with every provider's state.
pub(crate) struct Inputs {
    pub registry: Arc<Registry>,
    pub models: Vec<MergedModel>,
    pub providers: Vec<ProviderState>,
}

impl SessionManager {
    /// Every provider as the runtime sees it now.
    pub(crate) fn provider_states(&self, now: i64) -> Vec<ProviderState> {
        brigadier_providers::ProviderKind::ALL
            .into_iter()
            .map(|provider| ProviderState {
                provider,
                logged_in: self.runtime.provider_signed_in(provider),
                quota: self.runtime.provider_usage(provider, now),
            })
            .collect()
    }

    /// The registry, the merged catalog and the providers' states.
    pub(crate) async fn routing_inputs(&self, now: i64) -> Inputs {
        let (registry, patches) = self.runtime.registry().rating_snapshot();
        let catalogs: Vec<(
            brigadier_providers::ProviderKind,
            Vec<brigadier_providers::ModelInfo>,
        )> = brigadier_providers::ProviderKind::ALL
            .into_iter()
            .map(|provider| {
                let models = self
                    .runtime
                    .overview(provider)
                    .and_then(|overview| overview.models)
                    .map(|catalog| catalog.models)
                    .unwrap_or_default();
                (provider, models)
            })
            .collect();
        let catalogs: Vec<(
            brigadier_providers::ProviderKind,
            &[brigadier_providers::ModelInfo],
        )> = catalogs
            .iter()
            .map(|(provider, models)| (*provider, models.as_slice()))
            .collect();
        let research = match self.runtime.routing_store() {
            Some(store) => store.research().await.unwrap_or_default(),
            None => Vec::new(),
        };
        let mut models = brigadier_router::merge(&registry, &catalogs, &research, &[]);
        for model in &mut models {
            for patch in &patches {
                patch.apply(model);
            }
        }
        Inputs {
            registry,
            models,
            providers: self.provider_states(now),
        }
    }

    /// Whether the model `choice` names can run now: its provider ready, and no window that
    /// limits that model used up (a model's own weekly window included). A model the catalog
    /// doesn't list is judged by its provider. Whether the user switched it off is not asked:
    /// this is about a conversation already running on it.
    pub(crate) async fn choice_available(&self, choice: &ModelChoice) -> bool {
        let candidates: Vec<_> = self
            .runtime
            .account_candidates(choice.provider)
            .into_iter()
            .filter(|candidate| {
                choice
                    .account
                    .as_ref()
                    .is_none_or(|account| account == &candidate.account.choice_id())
            })
            .collect();
        crate::accounts::ready(
            &self.core.settings(),
            choice.provider,
            choice.model.as_deref(),
            &candidates,
            &[],
            crate::now_ms(),
        )
        .is_some()
    }

    /// Whether `account` itself could run `choice` now by its own quota, whichever account
    /// leads its provider: no limit, and no window that limits that model used up (#277).
    pub(crate) async fn account_frees_choice(
        &self,
        choice: &ModelChoice,
        account: &crate::accounts::AccountRef,
    ) -> bool {
        let now = crate::now_ms();
        let limited = self
            .runtime
            .monitor()
            .current_for(account, now)
            .is_some_and(|quota| quota.limit.is_some());
        !limited && self.model_free(choice, Some(account)).await
    }

    /// Whether no window that limits `choice`'s model is used up: its provider's lead
    /// account's, or `account`'s when given. A model the catalog doesn't list is free.
    async fn model_free(
        &self,
        choice: &ModelChoice,
        account: Option<&crate::accounts::AccountRef>,
    ) -> bool {
        let Some(id) = choice.model.as_deref() else {
            return true;
        };
        let now = crate::now_ms();
        let mut inputs = self.routing_inputs(now).await;
        if let Some(account) = account
            && let Some(state) = inputs
                .providers
                .iter_mut()
                .find(|state| state.provider == account.provider)
        {
            state.quota = self.runtime.account_usage(account, now);
        }
        inputs
            .models
            .iter()
            .find(|model| {
                model.provider == choice.provider
                    && brigadier_router::is_model(model, &inputs.registry, id)
            })
            .is_none_or(|model| {
                brigadier_router::available(model, &inputs.providers, &inputs.registry, now)
            })
    }
}
