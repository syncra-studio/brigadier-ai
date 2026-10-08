//! Routing v2 in the manager (PLAN.md §6 Phase 5): what [`brigadier_router::decide`] needs,
//! gathered from the runtime (logins, the quota monitor, the registry, each CLI's model list)
//! and `routing.sqlite` (outcomes and research), and what its decision becomes on a task.

use std::sync::Arc;

use brigadier_providers::{AllowedModels, ErrorKind, ProviderKind};
use brigadier_router::{
    Area, Author, Decision, Exclusion, Learned, MergedModel, ModelStatus, Needs, Pin, Preview,
    ProviderState, QualityTier, Query, Registry, Routed, TaskCategory,
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
            category: super::workers::category(task.kind),
            areas: &task.areas,
            floor: task.floor,
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
                logged_in: self.runtime.overview(provider).is_some_and(|overview| {
                    overview
                        .status
                        .as_ref()
                        .is_some_and(|status| status.logged_in)
                }),
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
        let Some(id) = choice.model.as_deref() else {
            return true;
        };
        let now = crate::now_ms();
        let inputs = self.routing_inputs(None, now).await;
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
        let query = Query {
            category: ask.category,
            areas: ask.areas,
            floor: ask.floor,
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
        (answer(&query, &inputs), trial_slot)
    }

    /// The question a task routed again asks (a hand-off, a retry after waiting): a gate
    /// member stays independent (never the author's model, nor another member's), the models
    /// that stopped on an error are left out right after it (a task that waited since tries
    /// them again: the cause may be gone, and the hand-off cap still ends a model that keeps
    /// failing), and a context window too small asks for a bigger one.
    pub(crate) async fn task_question(&self, task: &Task) -> TaskQuestion {
        let (avoid, distinct_from) = self.checker_avoid(task).await;
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
        let mut needs = super::workers::needs_of(&task.attachments, &task.needs);
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

    /// The models `task`'s worker may hand work to through its CLI's own sub-agents (PLAN.md
    /// §7): every model of its provider the router finds eligible for the task now
    /// ([`brigadier_router::eligible`]), asked as a hand-off asks but with no trial (a trial
    /// measures the task's own model) and its pin only a preference (other vendors are left
    /// out anyway), and always the model it runs on. In the CLI's exact ids, with every id of
    /// the CLI Brigadier knows that the task may not use (Fable and models made unavailable
    /// included), for Claude's prefix check.
    pub(crate) async fn allowed_models(&self, task: &Task) -> AllowedModels {
        let provider = task.route.choice.provider;
        let own = task.route.choice.model.clone();
        let question = self.task_question(task).await;
        let ask = question.ask(task, false, Trial::Never);
        self.ask_router(&ask, |query, inputs| {
            let eligible = brigadier_router::eligible(query);
            let own = own.as_deref().map(|id| {
                (
                    id,
                    own_model(provider, id, &inputs.models, &inputs.registry),
                )
            });
            AllowedModels {
                ids: allowed_ids(provider, own, &eligible, &inputs.registry),
                outside: outside_ids(provider, own, &eligible, &inputs.models, &inputs.registry),
            }
        })
        .await
        .0
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

/// A model's exact id, as an allowlist names it: Codex lists exact ids; a Claude alias (`opus`)
/// stands for the model it resolves to (`claude-opus-5-5`), else for its curated registry
/// entry's full id. `None` when no exact id is known.
fn exact_id(model: &MergedModel, registry: &Registry) -> Option<String> {
    model
        .resolved
        .as_deref()
        .and_then(|resolved| exact_named(model.provider, resolved))
        .or_else(|| exact_named(model.provider, &model.id))
        .or_else(|| {
            // Only a curated model is its entry; an inherited one is a newer member of it.
            let key = model
                .registry_key
                .as_deref()
                .filter(|_| model.status == ModelStatus::Curated)?;
            registry
                .entry(key)?
                .matches
                .ids
                .iter()
                .find_map(|id| exact_named(model.provider, id))
        })
}

/// `id` when it names one model exactly, without a context suffix (`[1m]`).
fn exact_named(provider: ProviderKind, id: &str) -> Option<String> {
    let id = id.split('[').next().unwrap_or(id).trim();
    match provider {
        ProviderKind::Codex => (!id.is_empty()).then(|| id.to_owned()),
        ProviderKind::Claude => id.starts_with("claude-").then(|| id.to_owned()),
    }
}

/// The session's own model in `provider`'s list: the one it is named by, else the one whose
/// alias, resolved id or curated entry names it.
fn own_model<'m>(
    provider: ProviderKind,
    own: &str,
    models: &'m [MergedModel],
    registry: &Registry,
) -> Option<&'m MergedModel> {
    let listed = models.iter().filter(|model| model.provider == provider);
    listed
        .clone()
        .find(|model| model.id.eq_ignore_ascii_case(own))
        .or_else(|| {
            listed
                .clone()
                .find(|model| brigadier_router::is_model(model, registry, own))
        })
}

/// The exact ids of `provider`'s models in `eligible`, the session's own model first (`own`:
/// its id, and its entry in the list). Empty when its own model has no exact id: a list
/// without it would stop the session itself.
fn allowed_ids(
    provider: ProviderKind,
    own: Option<(&str, Option<&MergedModel>)>,
    eligible: &[MergedModel],
    registry: &Registry,
) -> Vec<String> {
    let Some(own) = own.and_then(|(id, model)| {
        model
            .and_then(|model| exact_id(model, registry))
            .or_else(|| exact_named(provider, id))
    }) else {
        return Vec::new();
    };
    let mut ids = vec![own];
    for id in eligible
        .iter()
        .filter(|model| model.provider == provider)
        .filter_map(|model| exact_id(model, registry))
    {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

/// Every exact id of `provider`'s models Brigadier knows that the task may not use: the ids
/// of each model in its CLI's list (Fable and models made unavailable included) that is
/// neither eligible nor the session's own, a context variant kept apart (`opus[1m]` left out
/// while `opus` may run is `claude-opus-5-5[1m]`), and the registry's ids that name none of
/// the models it may use (a dated or `-fast` id of an eligible model is that model, and an
/// entry's id names a model listed only with its context suffix).
fn outside_ids(
    provider: ProviderKind,
    own: Option<(&str, Option<&MergedModel>)>,
    eligible: &[MergedModel],
    models: &[MergedModel],
    registry: &Registry,
) -> Vec<String> {
    let own = own.and_then(|(_, model)| model);
    let same = |a: &MergedModel, b: &MergedModel| a.provider == b.provider && a.id == b.id;
    let usable: Vec<&MergedModel> = models
        .iter()
        .filter(|model| model.provider == provider)
        .filter(|model| {
            own.is_some_and(|own| same(own, model))
                || eligible.iter().any(|eligible| same(eligible, model))
        })
        .collect();
    let mut outside: Vec<String> = Vec::new();
    let mut add = |id: &str| {
        let id = id.trim();
        if exact_named(provider, id).is_some() && !outside.iter().any(|known| known == id) {
            outside.push(id.to_owned());
        }
    };
    for model in models
        .iter()
        .filter(|model| model.provider == provider)
        .filter(|model| !usable.iter().any(|usable| same(usable, model)))
    {
        add(&model.id);
        if let Some(resolved) = &model.resolved {
            add(resolved);
        }
    }
    for id in registry
        .entries(provider)
        .flat_map(|entry| entry.matches.ids.iter())
        .filter(|id| {
            !usable.iter().any(|model| {
                brigadier_router::is_model(model, registry, id)
                    || [Some(&model.id), model.resolved.as_ref()]
                        .into_iter()
                        .flatten()
                        .any(|name| {
                            name.split('[')
                                .next()
                                .unwrap_or(name)
                                .eq_ignore_ascii_case(id)
                        })
            })
        })
    {
        add(id);
    }
    outside
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

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str, resolved: Option<&str>) -> brigadier_providers::ModelInfo {
        brigadier_providers::ModelInfo {
            id: id.to_owned(),
            display_name: id.to_owned(),
            description: String::new(),
            resolved: resolved.map(str::to_owned),
            efforts: Vec::new(),
            default_effort: None,
            is_default: false,
            input_modalities: vec!["text".into()],
            fast: None,
            legacy: false,
        }
    }

    fn catalog(registry: &Registry) -> Vec<MergedModel> {
        let claude = [
            info("opus[1m]", Some("claude-opus-5-5[1m]")),
            info("claude-fable-5-1[1m]", Some("claude-fable-5-1")),
            info("sonnet", Some("claude-sonnet-5")),
            // Not resolved by the CLI: its curated entry names it.
            info("haiku", None),
            info("claude-opus-5", None),
        ];
        let codex = [info("gpt-6.1-sol", None), info("gpt-6-luna", None)];
        brigadier_router::merge(
            registry,
            &[
                (ProviderKind::Claude, claude.as_slice()),
                (ProviderKind::Codex, codex.as_slice()),
            ],
            &[],
            &[],
        )
    }

    /// The allowed ids for a session on `own`, with `eligible` what routing allows.
    fn allowed(
        provider: ProviderKind,
        own: Option<&str>,
        eligible: &[MergedModel],
        models: &[MergedModel],
        registry: &Registry,
    ) -> Vec<String> {
        let own = own.map(|id| (id, own_model(provider, id, models, registry)));
        allowed_ids(provider, own, eligible, registry)
    }

    /// The ids left out for a Claude session on `own`, with `eligible` what routing allows.
    fn outside(
        own: &str,
        eligible: &[MergedModel],
        models: &[MergedModel],
        registry: &Registry,
    ) -> Vec<String> {
        let own = Some((own, own_model(ProviderKind::Claude, own, models, registry)));
        outside_ids(ProviderKind::Claude, own, eligible, models, registry)
    }

    /// `models` without the ones named.
    fn without(models: &[MergedModel], ids: &[&str]) -> Vec<MergedModel> {
        models
            .iter()
            .filter(|model| !model.excluded && !ids.contains(&model.id.as_str()))
            .cloned()
            .collect()
    }

    #[test]
    fn allowed_ids_are_exact_and_start_with_the_sessions_own_model() {
        let registry = Registry::bundled();
        let models = catalog(&registry);
        // What the router finds eligible never holds Fable.
        let eligible = without(&models, &[]);
        let ids = allowed(
            ProviderKind::Claude,
            Some("opus[1m]"),
            &eligible,
            &models,
            &registry,
        );
        assert_eq!(
            ids,
            [
                "claude-opus-5-5",
                "claude-sonnet-5",
                "claude-haiku-4-5-20251001",
                "claude-opus-5",
            ]
        );
        let codex = allowed(
            ProviderKind::Codex,
            Some("gpt-6-luna"),
            &eligible,
            &models,
            &registry,
        );
        assert_eq!(codex, ["gpt-6-luna", "gpt-6.1-sol"]);
        // Its own model always, even when routing would not pick it now.
        let alone = allowed(
            ProviderKind::Claude,
            Some("sonnet"),
            &[],
            &models,
            &registry,
        );
        assert_eq!(alone, ["claude-sonnet-5"]);
        // The CLI's default model has no exact id: none at all.
        assert!(allowed(ProviderKind::Claude, None, &models, &models, &registry).is_empty());
        assert!(
            allowed(
                ProviderKind::Claude,
                Some("best"),
                &models,
                &models,
                &registry
            )
            .is_empty()
        );
    }

    #[test]
    fn outside_ids_are_fable_and_the_models_nothing_allowed_is() {
        let registry = Registry::bundled();
        let models = catalog(&registry);
        let left_out = outside("opus[1m]", &without(&models, &[]), &models, &registry);
        for id in [
            "claude-fable-5-1[1m]",
            "claude-fable-5-1",
            "claude-opus-4-8",
        ] {
            assert!(left_out.iter().any(|known| known == id), "{id}");
        }
        // Aliases are never listed, nor any id of a model it may use.
        for id in [
            "opus",
            "claude-opus-5-5",
            "claude-opus-5-5[1m]",
            "claude-opus-5",
            "claude-haiku-4-5",
            "claude-haiku-4-5-20251001",
        ] {
            assert!(!left_out.iter().any(|known| known == id), "{id}");
        }
        // Left out by a rule, its ids are outside; the session's own model never is.
        let left_out = outside(
            "opus[1m]",
            &without(&models, &["opus[1m]", "haiku"]),
            &models,
            &registry,
        );
        assert!(left_out.iter().any(|id| id == "claude-haiku-4-5-20251001"));
        assert!(left_out.iter().any(|id| id == "claude-haiku-4-5"));
        assert!(!left_out.iter().any(|id| id.starts_with("claude-opus-5-5")));
    }

    #[test]
    fn a_context_variant_left_out_is_outside_beside_the_one_allowed() {
        let registry = Registry::bundled();
        let models = brigadier_router::merge(
            &registry,
            &[(
                ProviderKind::Claude,
                [
                    info("opus", Some("claude-opus-5-5")),
                    info("opus[1m]", Some("claude-opus-5-5[1m]")),
                ]
                .as_slice(),
            )],
            &[],
            &[],
        );
        // `opus[1m]` hidden: its id stays apart from the allowed `claude-opus-5-5`.
        let eligible = without(&models, &["opus[1m]"]);
        assert_eq!(
            allowed(
                ProviderKind::Claude,
                Some("opus"),
                &eligible,
                &models,
                &registry
            ),
            ["claude-opus-5-5"]
        );
        assert_eq!(
            outside("opus", &eligible, &models, &registry)
                .iter()
                .filter(|id| id.starts_with("claude-opus-5-5"))
                .collect::<Vec<_>>(),
            ["claude-opus-5-5[1m]"]
        );
        // Both allowed: nothing of Opus 5.5 is outside.
        assert!(
            !outside("opus", &without(&models, &[]), &models, &registry)
                .iter()
                .any(|id| id.starts_with("claude-opus-5-5"))
        );
    }

    #[test]
    fn a_dated_or_fast_id_is_outside_unless_routing_allows_its_model() {
        let registry = Registry::bundled();
        let models = brigadier_router::merge(
            &registry,
            &[(
                ProviderKind::Claude,
                [
                    info("claude-opus-5-5", None),
                    info("claude-opus-5-5-fast", None),
                    info("claude-haiku-4-5", None),
                    info("claude-sonnet-5", None),
                    info("claude-sonnet-5-20260101", None),
                ]
                .as_slice(),
            )],
            &[],
            &[],
        );
        // A Never rule on the fast model and on the dated Sonnet: both outside.
        let eligible = without(
            &models,
            &["claude-opus-5-5-fast", "claude-sonnet-5-20260101"],
        );
        let left_out = outside("claude-opus-5-5", &eligible, &models, &registry);
        assert!(left_out.iter().any(|id| id == "claude-opus-5-5-fast"));
        assert!(left_out.iter().any(|id| id == "claude-sonnet-5-20260101"));
        // Haiku's dated id is Haiku itself, which routing allows: not outside.
        assert!(!left_out.iter().any(|id| id.starts_with("claude-haiku-4-5")));
        // Eligible, the fast model is not outside either.
        let all = outside(
            "claude-opus-5-5",
            &without(&models, &[]),
            &models,
            &registry,
        );
        assert!(!all.iter().any(|id| id.starts_with("claude-opus-5-5")));
        assert!(!all.iter().any(|id| id == "claude-sonnet-5-20260101"));
    }
}
