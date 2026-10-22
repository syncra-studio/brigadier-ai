//! Routing v2 in the manager (PLAN.md §6 Phase 5): what [`brigadier_router::decide`] needs,
//! gathered from the runtime (logins, the quota monitor, the registry, each CLI's model list)
//! and `routing.sqlite` (outcomes and research), and what its decision becomes on a task.

use std::sync::Arc;

use brigadier_providers::{ErrorKind, ProviderKind};
use brigadier_router::{
    Area, Author, Decision, Exclusion, Learned, MergedModel, Needs, Pin, Preview, ProviderState,
    QualityTier, Query, Registry, Routed, TaskCategory,
};

use super::SessionManager;
use crate::model::{ModelChoice, ProjectId};
use crate::work::{AttemptEnd, Route, Task};

/// One in this many low-risk tasks may go to a model on trial.
const TRIAL_EVERY: u64 = 5;
/// `routing.sqlite` meta key counting low-risk tasks, for trial slots.
const TRIAL_COUNTER: &str = "trial_counter";

/// A routing question, as the manager asks it.
pub(crate) struct Ask<'a> {
    pub category: TaskCategory,
    pub areas: &'a [Area],
    pub floor: QualityTier,
    /// The floor holds for a ranked or named model too (a rerun one tier up).
    pub floor_binds: bool,
    pub needs: Needs,
    pub pin: Option<Pin>,
    /// The pin binds: a hand-off or a resume stays with the pinned vendor, or waits.
    pub hold_pin: bool,
    pub avoid: Option<Author>,
    /// The models already checking the change (a second reviewer, a verifier).
    pub distinct_from: Vec<Author>,
    pub exclude: &'a [Exclusion],
    pub project_id: Option<&'a ProjectId>,
    /// Whether it may go to a model on trial.
    pub trial: Trial,
}

/// A routing question's claim on the trial slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trial {
    /// A task being started: it takes the next slot (the counter moves on).
    Take,
    /// A preview of the next task: it looks at the next slot without taking it.
    Peek,
    /// A task that took a slot when created and waited to start: it still holds it.
    Held,
    /// A fallback or a stand-in: no trial.
    Never,
}

/// What routing is asked about a task that already exists, beyond its own fields: the models
/// it must differ from, those that just failed on it, and what it needs.
pub(crate) struct TaskQuestion {
    avoid: Option<Author>,
    distinct_from: Vec<Author>,
    exclude: Vec<Exclusion>,
    needs: Needs,
    project: Option<ProjectId>,
}

impl TaskQuestion {
    /// The question for `task`. `hold_pin`: its pin binds.
    pub(crate) fn ask<'a>(&'a self, task: &'a Task, hold_pin: bool, trial: Trial) -> Ask<'a> {
        Ask {
            category: super::workers::task_category(task),
            areas: &task.areas,
            floor: task.floor,
            floor_binds: task.rerun.is_some(),
            needs: self.needs,
            pin: task.pin.clone(),
            hold_pin,
            avoid: self.avoid.clone(),
            distinct_from: self.distinct_from.clone(),
            exclude: &self.exclude,
            project_id: self.project.as_ref(),
            trial,
        }
    }
}

/// What routing knows now, beyond the question.
pub(crate) struct Inputs {
    pub registry: Arc<Registry>,
    pub models: Vec<MergedModel>,
    pub learned: Vec<Learned>,
    pub providers: Vec<ProviderState>,
}

impl SessionManager {
    /// Every provider as routing sees it now.
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

    /// The registry, the merged catalog, what `project`'s outcomes taught and the providers'
    /// states.
    pub(crate) async fn routing_inputs(&self, project: Option<&ProjectId>, now: i64) -> Inputs {
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
        let (outcomes, research) = match self.runtime.routing_store() {
            Some(store) => (
                store.outcomes(None).await.unwrap_or_default(),
                store.research().await.unwrap_or_default(),
            ),
            None => (Vec::new(), Vec::new()),
        };
        let counts = brigadier_router::outcome_counts(&outcomes);
        let mut models = brigadier_router::merge(&registry, &catalogs, &research, &counts);
        for model in &mut models {
            for patch in &patches {
                patch.apply(model);
            }
        }
        let learned = match project {
            Some(project) => {
                let own: Vec<_> = outcomes
                    .into_iter()
                    .filter(|outcome| outcome.project_id == project.0)
                    .collect();
                brigadier_router::learn(&own, &models, &registry)
            }
            None => Vec::new(),
        };
        Inputs {
            registry,
            models,
            learned,
            providers: self.provider_states(now),
        }
    }

    /// Whether the model `choice` names can run now: its provider ready, and no window that
    /// limits that model used up (a model's own weekly window included). A model routing
    /// doesn't list is judged by its provider. Whether the user switched it off is not asked:
    /// this is about a conversation already running on it.
    pub(crate) async fn choice_available(&self, choice: &ModelChoice) -> bool {
        if !self.provider_ready(choice.provider) {
            return false;
        }
        self.model_free(choice, None).await
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
    /// account's, or `account`'s when given. A model routing doesn't list is free.
    async fn model_free(
        &self,
        choice: &ModelChoice,
        account: Option<&crate::accounts::AccountRef>,
    ) -> bool {
        let Some(id) = choice.model.as_deref() else {
            return true;
        };
        let now = crate::now_ms();
        let mut inputs = self.routing_inputs(None, now).await;
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

    /// Asks the router.
    pub(crate) async fn decide(&self, ask: &Ask<'_>) -> Decision {
        self.preview(ask).await.0.decision
    }

    /// Asks the router, keeping every model it weighed (the Routing page's live order), and
    /// says whether the question held a trial slot.
    pub(crate) async fn preview(&self, ask: &Ask<'_>) -> (Preview, bool) {
        let ((mut preview, models), trial_slot) = self
            .ask_router(ask, |query, _| {
                (brigadier_router::preview(query), query.models.to_vec())
            })
            .await;
        if let Decision::Run(routed) = preview.decision {
            preview.decision = Decision::Run(cheap_for_development(routed, &models));
        }
        (preview, trial_slot)
    }

    /// Puts `ask` to the router through `answer`, which gets the query and what routing knows
    /// now (its catalog still holds the agents switched off and the models made unavailable,
    /// which the query leaves out), and says whether the question held a trial slot.
    async fn ask_router<T>(
        &self,
        ask: &Ask<'_>,
        answer: impl FnOnce(&Query, &Inputs) -> T,
    ) -> (T, bool) {
        let now = crate::now_ms();
        let inputs = self.routing_inputs(ask.project_id, now).await;
        let settings = self.core.settings();
        let running = self.running_workers();
        let trial_slot = match ask.trial {
            Trial::Take => self.trial_slot(ask.category, true).await,
            Trial::Peek => self.trial_slot(ask.category, false).await,
            Trial::Held => true,
            Trial::Never => false,
        };
        let project_id = ask.project_id.map(|id| id.0.as_str());
        // Agents switched off and models made unavailable aren't considered at all.
        let (models, providers) =
            crate::routing::availability::routable(&settings, &inputs.models, &inputs.providers);
        // With switching to another agent off, work a limit would move stays on its agent.
        let held: Vec<Exclusion>;
        let mut query = Query {
            category: ask.category,
            areas: ask.areas,
            floor: ask.floor,
            floor_binds: ask.floor_binds,
            needs: ask.needs,
            pin: ask.pin.clone(),
            hold_pin: ask.hold_pin,
            avoid: ask.avoid.clone(),
            distinct_from: ask.distinct_from.clone(),
            exclude: ask.exclude,
            overrides: &settings.routing_overrides,
            rankings: &settings.routing_rankings,
            project_id,
            running: &running,
            trial_slot,
            providers: &providers,
            models: &models,
            registry: &inputs.registry,
            learned: &inputs.learned,
            now_ms: now,
        };
        if !settings.switch_providers
            && let Some(agent) = brigadier_router::moved_by_limit(&query)
        {
            held = ask
                .exclude
                .iter()
                .cloned()
                .chain(
                    ProviderKind::ALL
                        .into_iter()
                        .filter(|provider| *provider != agent)
                        .map(|provider| Exclusion {
                            provider,
                            model: None,
                        }),
                )
                .collect();
            query.exclude = &held;
        }
        (answer(&query, &inputs), trial_slot)
    }

    /// The question a task routed again asks (a hand-off, a retry after waiting): a gate
    /// member stays independent (never the author's model, nor another member's), the models
    /// that stopped on an error are left out right after it (a task that waited since tries
    /// them again: the cause may be gone, and the hand-off cap still ends a model that keeps
    /// failing), and a context window too small asks for a bigger one.
    pub(crate) async fn task_question(&self, task: &Task) -> TaskQuestion {
        let (avoid, distinct_from) = (None, Vec::new());
        let exclude: Vec<Exclusion> = task
            .attempts
            .iter()
            .filter(|_| task.quota_wait.is_none())
            .filter(|attempt| matches!(attempt.end, Some(AttemptEnd::Error { .. })))
            .map(|attempt| Exclusion {
                provider: attempt.route.choice.provider,
                model: attempt.route.choice.model.clone(),
            })
            .collect();
        let mut needs = super::workers::needs_of(task.kind, &task.attachments, &task.needs);
        if let Some(attempt) = task.attempts.last()
            && matches!(
                attempt.end,
                Some(AttemptEnd::Error {
                    kind: ErrorKind::ContextWindow,
                    ..
                })
            )
            && let Some(window) = self
                .routing_inputs(None, crate::now_ms())
                .await
                .models
                .iter()
                .find(|model| {
                    model.provider == attempt.route.choice.provider
                        && Some(&model.id) == attempt.route.choice.model.as_ref()
                })
                .and_then(|model| model.context_window)
        {
            needs.context_tokens = Some(window + 1);
        }
        let project = self
            .core
            .conversation(&task.conversation_id)
            .ok()
            .and_then(|conversation| conversation.project_id);
        TaskQuestion {
            avoid,
            distinct_from,
            exclude,
            needs,
            project,
        }
    }

    /// Whether this low-risk task may go to a model on trial: one in [`TRIAL_EVERY`], counted
    /// across launches. `take`: the task takes the slot (the count moves on); otherwise this
    /// only looks at what the next task would get.
    async fn trial_slot(&self, category: TaskCategory, take: bool) -> bool {
        if !brigadier_router::allows_trials(category) {
            return false;
        }
        let Some(store) = self.runtime.routing_store() else {
            return false;
        };
        let count = store
            .meta(TRIAL_COUNTER)
            .await
            .ok()
            .flatten()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        if take && let Err(err) = store.set_meta(TRIAL_COUNTER, (count + 1).to_string()).await {
            tracing::debug!(error = %err, "could not count the trial slot");
        }
        count % TRIAL_EVERY == 0
    }
}

/// A task's route from the router's choice.
pub(crate) fn route_from(routed: Routed) -> Route {
    Route {
        choice: ModelChoice {
            provider: routed.provider,
            model: Some(routed.model),
            effort: routed.effort,
            fast: None,
            account: None,
        },
        reason: routed.reason,
        explanation: Some(routed.explanation),
    }
}

/// `BRIGADIER_ROUTE_CHEAP=1`, in development builds only (verification runs): after routing
/// picks the vendor, use its cheapest model at low effort. The routing reason is kept and says
/// so.
#[cfg(debug_assertions)]
fn cheap_for_development(mut routed: Routed, models: &[MergedModel]) -> Routed {
    use brigadier_providers::ProviderKind;
    if std::env::var_os("BRIGADIER_ROUTE_CHEAP").is_none_or(|value| value != "1") {
        return routed;
    }
    let family = match routed.provider {
        ProviderKind::Claude => "haiku",
        ProviderKind::Codex => "luna",
    };
    routed.model = models
        .iter()
        .filter(|model| model.provider == routed.provider)
        .find(|model| model.id.contains(family))
        .map_or_else(|| family.to_owned(), |model| model.id.clone());
    routed.effort = (routed.provider == ProviderKind::Codex).then(|| "low".to_owned());
    routed.reason = format!("{} (dev: cheapest model substituted)", routed.reason);
    routed
}

/// Release builds route as the router says.
#[cfg(not(debug_assertions))]
fn cheap_for_development(routed: Routed, _models: &[MergedModel]) -> Routed {
    routed
}
