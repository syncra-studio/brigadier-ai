//! Which agents and models the user lets Brigadier use, as the Providers and Routing pages
//! set them:
//!
//! - An agent switched off ([`Settings::disabled_providers`]) and a model made unavailable
//!   ([`Settings::hidden_models`]) are out of every picker and get no work at all, routed or
//!   in the background. Conversations already running keep their model.
//! - Whether the orchestrator may hand a model worker tasks is a `never` rule for the worker
//!   kinds of work ([`worker_rule`]): a Chat's stand-in and an orchestrator are left alone.
//! - A model seen after its agent's first list ([`Settings::known_models`]) starts with that
//!   rule, so it gets no work until the user allows it; its agent's first list is taken as
//!   known, so turning this on (or signing in) changes nothing for the models already there.

use brigadier_providers::{ModelInfo, ProviderKind};
use brigadier_router::{
    MergedModel, OverrideEffect, OverrideRule, OverrideTarget, ProviderState, TaskCategory,
};

use crate::model::{ModelChoice, ModelRef, SETTINGS_VERSION, Settings};

/// The kinds of work the orchestrator hands to workers: what the Routing page's switch is
/// about. Not `chat` (a Chat's stand-in) or `orchestrate`.
pub const WORKER_CATEGORIES: [TaskCategory; 7] = [
    TaskCategory::Scout,
    TaskCategory::Research,
    TaskCategory::Implement,
    TaskCategory::Review,
    TaskCategory::Merge,
    TaskCategory::Verify,
    TaskCategory::Operate,
];

/// The agent isn't switched off.
pub fn provider_on(settings: &Settings, provider: ProviderKind) -> bool {
    !settings.disabled_providers.contains(&provider)
}

/// The model may be used at all: its agent is on and the model isn't made unavailable.
pub fn model_available(settings: &Settings, provider: ProviderKind, id: &str) -> bool {
    provider_on(settings, provider)
        && !settings
            .hidden_models
            .iter()
            .any(|model| model.provider == provider && model.id == id)
}

/// The listed model an id names, as the pickers read it: the model with that id, else the
/// alias that resolves to it (`claude-opus-5-5` names `opus`); no id, or `default`, names the
/// model marked default.
fn listed<'a>(catalog: &'a [ModelInfo], id: Option<&str>) -> Option<&'a ModelInfo> {
    let named = id.and_then(|id| {
        catalog.iter().find(|model| model.id == id).or_else(|| {
            catalog
                .iter()
                .find(|model| model.resolved.as_deref() == Some(id))
        })
    });
    named.or_else(|| {
        matches!(id, None | Some("default"))
            .then(|| catalog.iter().find(|model| model.is_default))
            .flatten()
    })
}

/// Whether a new conversation may start on `choice` (or switch to it), with why not in plain
/// words. `catalog` is its agent's model list: a concrete id or an alias is judged as the
/// listed model it names, and a choice without a model as the agent's default one.
pub fn check_choice(
    settings: &Settings,
    choice: &ModelChoice,
    catalog: &[ModelInfo],
) -> Result<(), String> {
    let agent = choice.provider.label();
    if !provider_on(settings, choice.provider) {
        return Err(format!(
            "{agent} is switched off. Turn it on in Settings › Providers."
        ));
    }
    let id = choice.model.as_deref();
    let model = listed(catalog, id);
    let hidden = |id: &str| !model_available(settings, choice.provider, id);
    if id.is_some_and(hidden) || model.is_some_and(|model| hidden(&model.id)) {
        let name = model.map_or_else(
            || id.unwrap_or("Its default model").to_owned(),
            |model| model.display_name.clone(),
        );
        return Err(format!(
            "{name} isn't available. Turn it on in Settings › Providers."
        ));
    }
    Ok(())
}

/// How the id of a [`worker_rule`] Brigadier added for a model it just saw starts: the Routing
/// page tags that model "New" until the user turns its switch.
pub const NEW_MODEL_RULE: &str = "new-model-";

/// The rule keeping a model it just saw from worker tasks everywhere, the rule the Routing
/// page's switch turns off and on (the switch writes its own id).
pub fn worker_rule(provider: ProviderKind, id: &str, now_ms: i64) -> OverrideRule {
    OverrideRule {
        id: format!("{NEW_MODEL_RULE}{}", uuid::Uuid::now_v7()),
        effect: OverrideEffect::Never,
        target: OverrideTarget::Model {
            provider,
            id: id.to_owned(),
        },
        categories: WORKER_CATEGORIES.to_vec(),
        areas: Vec::new(),
        project_id: None,
        created_at_ms: now_ms,
    }
}

/// Whether `rule` is [`worker_rule`] for this model.
pub fn is_worker_rule(rule: &OverrideRule, provider: ProviderKind, id: &str) -> bool {
    rule.effect == OverrideEffect::Never
        && matches!(&rule.target, OverrideTarget::Model { provider: p, id: m } if *p == provider && m == id)
        && rule.categories.len() == WORKER_CATEGORIES.len()
        && WORKER_CATEGORIES
            .iter()
            .all(|category| rule.categories.contains(category))
        && rule.areas.is_empty()
        && rule.project_id.is_none()
}

/// The settings with the models of `provider`'s list recorded as known, or `None` when all
/// of them already are. Each model seen after the agent's first list gets [`worker_rule`].
pub fn note_models(
    settings: &Settings,
    provider: ProviderKind,
    ids: &[String],
    now_ms: i64,
) -> Option<Settings> {
    let first_list = !settings
        .known_models
        .iter()
        .any(|model| model.provider == provider);
    let mut next = settings.clone();
    for id in ids {
        if next
            .known_models
            .iter()
            .any(|model| model.provider == provider && &model.id == id)
        {
            continue;
        }
        next.known_models.push(ModelRef {
            provider,
            id: id.clone(),
        });
        if !first_list
            && !next
                .routing_overrides
                .iter()
                .any(|rule| is_worker_rule(rule, provider, id))
        {
            next.routing_overrides
                .push(worker_rule(provider, id, now_ms));
        }
    }
    (next != *settings).then_some(next)
}

/// Settings a client sent, written over `current`: a model the core recorded as known since
/// the client read its copy stays known, with the rule keeping it from worker tasks, so a
/// change made on a page open from before can't give that model work.
pub fn rebase(current: &Settings, mut incoming: Settings) -> Settings {
    for model in &current.known_models {
        if incoming.known_models.contains(model) {
            continue;
        }
        incoming.known_models.push(model.clone());
        for rule in current
            .routing_overrides
            .iter()
            .filter(|rule| is_worker_rule(rule, model.provider, &model.id))
        {
            if !incoming
                .routing_overrides
                .iter()
                .any(|kept| kept.id == rule.id)
            {
                incoming.routing_overrides.push(rule.clone());
            }
        }
    }
    incoming.settings_version = incoming.settings_version.max(current.settings_version);
    incoming
}

/// Whether a settings change may let work waiting for quota run now: new rules or rankings,
/// or an agent or model turned back on.
pub fn wakes_waiting_work(before: &Settings, after: &Settings) -> bool {
    before.routing_overrides != after.routing_overrides
        || before.routing_rankings != after.routing_rankings
        || before.disabled_providers != after.disabled_providers
        || before.hidden_models != after.hidden_models
}

/// Saved settings brought up to [`SETTINGS_VERSION`], or `None` when they already are.
///
/// Version 1: an agent switched off before used to be a `never` rule for the whole agent,
/// everywhere, for all work; such a rule becomes the agent switched off. Every other rule
/// stays as it is.
///
/// Version 2: a model's worker switch saved before operate work existed covers it too, so a
/// model kept from worker tasks stays kept from all of them.
pub fn migrate(settings: &Settings) -> Option<Settings> {
    if settings.settings_version >= SETTINGS_VERSION {
        return None;
    }
    let mut next = settings.clone();
    if settings.settings_version < 1 {
        next.routing_overrides.retain(|rule| {
            let OverrideTarget::Vendor { provider } = rule.target else {
                return true;
            };
            let off = rule.effect == OverrideEffect::Never
                && rule.categories.is_empty()
                && rule.areas.is_empty()
                && rule.project_id.is_none();
            if off && !next.disabled_providers.contains(&provider) {
                next.disabled_providers.push(provider);
            }
            !off
        });
    }
    if settings.settings_version < 2 {
        let before = &WORKER_CATEGORIES[..WORKER_CATEGORIES.len() - 1];
        for rule in &mut next.routing_overrides {
            let switch = rule.effect == OverrideEffect::Never
                && matches!(rule.target, OverrideTarget::Model { .. })
                && rule.categories.len() == before.len()
                && before
                    .iter()
                    .all(|category| rule.categories.contains(category))
                && rule.areas.is_empty()
                && rule.project_id.is_none();
            if switch {
                rule.categories.push(TaskCategory::Operate);
            }
        }
    }
    next.settings_version = SETTINGS_VERSION;
    Some(next)
}

/// What routing may consider: the models of agents switched on that aren't made unavailable.
/// An agent switched off, or one whose listed models are all unavailable, counts as signed
/// out: routing would otherwise take its missing list as not read yet and offer its registry
/// models instead.
pub fn routable(
    settings: &Settings,
    models: &[MergedModel],
    providers: &[ProviderState],
) -> (Vec<MergedModel>, Vec<ProviderState>) {
    let kept: Vec<MergedModel> = models
        .iter()
        .filter(|model| model_available(settings, model.provider, &model.id))
        .cloned()
        .collect();
    let providers = providers
        .iter()
        .map(|state| {
            let listed = models.iter().any(|model| model.provider == state.provider);
            let left = kept.iter().any(|model| model.provider == state.provider);
            ProviderState {
                logged_in: state.logged_in
                    && provider_on(settings, state.provider)
                    && (left || !listed),
                ..state.clone()
            }
        })
        .collect();
    (kept, providers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn an_agents_first_list_is_known_without_rules() {
        let settings = Settings::default();
        let next = note_models(&settings, ProviderKind::Claude, &ids(&["opus", "haiku"]), 1)
            .expect("the models are new");
        assert_eq!(next.known_models.len(), 2);
        assert!(next.routing_overrides.is_empty());
    }

    #[test]
    fn a_model_seen_later_gets_no_worker_tasks() {
        let settings = note_models(
            &Settings::default(),
            ProviderKind::Claude,
            &ids(&["opus"]),
            1,
        )
        .expect("the models are new");
        let next = note_models(&settings, ProviderKind::Claude, &ids(&["opus", "fresh"]), 2)
            .expect("fresh is new");
        assert_eq!(next.routing_overrides.len(), 1);
        let rule = &next.routing_overrides[0];
        assert!(is_worker_rule(rule, ProviderKind::Claude, "fresh"));
        assert!(rule.id.starts_with(NEW_MODEL_RULE));
        assert!(!rule.categories.contains(&TaskCategory::Chat));
        assert!(!rule.categories.contains(&TaskCategory::Orchestrate));
        // Seen again: nothing changes, and a rule the user removed stays removed.
        let mut allowed = next.clone();
        allowed.routing_overrides.clear();
        assert!(note_models(&allowed, ProviderKind::Claude, &ids(&["opus", "fresh"]), 3).is_none());
    }

    #[test]
    fn another_agents_first_list_is_known_too() {
        let settings = note_models(
            &Settings::default(),
            ProviderKind::Claude,
            &ids(&["opus"]),
            1,
        )
        .expect("the models are new");
        let next =
            note_models(&settings, ProviderKind::Codex, &ids(&["sol"]), 2).expect("sol is new");
        assert!(next.routing_overrides.is_empty());
    }

    #[test]
    fn the_old_agent_switch_becomes_the_agent_off() {
        let off = OverrideRule {
            id: "off".into(),
            effect: OverrideEffect::Never,
            target: OverrideTarget::Vendor {
                provider: ProviderKind::Codex,
            },
            categories: Vec::new(),
            areas: Vec::new(),
            project_id: None,
            created_at_ms: 0,
        };
        let scoped = OverrideRule {
            id: "reviews".into(),
            categories: vec![TaskCategory::Review],
            ..off.clone()
        };
        let settings = Settings {
            routing_overrides: vec![off, scoped.clone()],
            settings_version: 0,
            ..Settings::default()
        };
        let next = migrate(&settings).expect("version 0 converts");
        assert_eq!(next.disabled_providers, vec![ProviderKind::Codex]);
        assert_eq!(next.routing_overrides, vec![scoped]);
        assert_eq!(next.settings_version, SETTINGS_VERSION);
        assert!(migrate(&next).is_none());
    }

    #[test]
    fn an_old_worker_switch_covers_operate_work() {
        let old = OverrideRule {
            categories: WORKER_CATEGORIES[..6].to_vec(),
            ..worker_rule(ProviderKind::Claude, "haiku", 0)
        };
        let settings = Settings {
            routing_overrides: vec![old],
            settings_version: 1,
            ..Settings::default()
        };
        let next = migrate(&settings).expect("version 1 converts");
        assert!(is_worker_rule(
            &next.routing_overrides[0],
            ProviderKind::Claude,
            "haiku"
        ));
        assert!(migrate(&next).is_none());
    }

    #[test]
    fn a_new_conversation_needs_an_available_model() {
        let settings = Settings {
            disabled_providers: vec![ProviderKind::Codex],
            hidden_models: vec![ModelRef {
                provider: ProviderKind::Claude,
                id: "haiku".into(),
            }],
            ..Settings::default()
        };
        let choice = |provider, model: Option<&str>| ModelChoice {
            provider,
            model: model.map(str::to_owned),
            effort: None,
            fast: None,
            account: None,
        };
        let check = |provider, model| check_choice(&settings, &choice(provider, model), &[]);
        assert!(check(ProviderKind::Claude, Some("opus")).is_ok());
        assert!(check(ProviderKind::Claude, None).is_ok());
        let hidden = check(ProviderKind::Claude, Some("haiku"));
        assert!(hidden.is_err_and(|why| why.contains("haiku")));
        let off = check(ProviderKind::Codex, None);
        assert!(off.is_err_and(|why| why.contains("Codex is switched off")));
    }

    fn listed_model(id: &str, resolved: &str, is_default: bool) -> ModelInfo {
        ModelInfo {
            id: id.into(),
            display_name: format!("Model {id}"),
            description: String::new(),
            resolved: Some(resolved.into()),
            efforts: Vec::new(),
            default_effort: None,
            is_default,
            input_modalities: Vec::new(),
            fast: None,
            legacy: false,
        }
    }

    #[test]
    fn a_choice_is_judged_as_the_listed_model_it_names() {
        let settings = Settings {
            hidden_models: vec![ModelRef {
                provider: ProviderKind::Claude,
                id: "opus".into(),
            }],
            ..Settings::default()
        };
        let catalog = [
            listed_model("opus", "claude-opus-5-5", true),
            listed_model("sonnet", "claude-sonnet-5-5", false),
        ];
        let check = |model: Option<&str>| {
            check_choice(
                &settings,
                &ModelChoice {
                    provider: ProviderKind::Claude,
                    model: model.map(str::to_owned),
                    effort: None,
                    fast: None,
                    account: None,
                },
                &catalog,
            )
        };
        // The concrete id, `default` and no model at all all name the hidden `opus`.
        let resolved = check(Some("claude-opus-5-5"));
        assert!(resolved.is_err_and(|why| why.contains("Model opus")));
        assert!(check(Some("default")).is_err());
        assert!(check(None).is_err());
        assert!(check(Some("claude-sonnet-5-5")).is_ok());
        assert!(check(Some("sonnet")).is_ok());
    }

    #[test]
    fn an_agent_with_every_listed_model_unavailable_gets_no_work() {
        let settings = Settings {
            hidden_models: vec![ModelRef {
                provider: ProviderKind::Claude,
                id: "opus".into(),
            }],
            ..Settings::default()
        };
        let listed = [listed_model("opus", "claude-opus-5-5", true)];
        let merged = brigadier_router::merge(
            &brigadier_router::Registry::bundled(),
            &[(ProviderKind::Claude, &listed)],
            &[],
            &[],
        );
        let states = [ProviderKind::Claude, ProviderKind::Codex].map(|provider| ProviderState {
            provider,
            logged_in: true,
            quota: None,
        });
        let (models, providers) = routable(&settings, &merged, &states);
        assert!(models.is_empty());
        // Signed out as routing sees it, so its registry models aren't offered instead.
        assert!(!providers[0].logged_in);
        // Codex's list isn't read yet: that is routing's to judge, as before.
        assert!(providers[1].logged_in);
    }

    #[test]
    fn a_client_write_keeps_models_seen_meanwhile() {
        let read = note_models(
            &Settings::default(),
            ProviderKind::Claude,
            &ids(&["opus"]),
            1,
        )
        .expect("the models are new");
        // The core sees `fresh` while a page still holds `read`, then the page writes.
        let current = note_models(&read, ProviderKind::Claude, &ids(&["opus", "fresh"]), 2)
            .expect("fresh is new");
        let sent = Settings {
            disabled_providers: vec![ProviderKind::Codex],
            ..read.clone()
        };
        let written = rebase(&current, sent);
        assert_eq!(written.disabled_providers, vec![ProviderKind::Codex]);
        assert_eq!(written.known_models, current.known_models);
        assert_eq!(written.routing_overrides, current.routing_overrides);
        // A page that saw `fresh` and allowed it keeps it allowed.
        let allowed = Settings {
            routing_overrides: Vec::new(),
            ..current.clone()
        };
        assert!(rebase(&current, allowed).routing_overrides.is_empty());
    }

    #[test]
    fn turning_an_agent_or_model_back_on_wakes_waiting_work() {
        let off = Settings {
            disabled_providers: vec![ProviderKind::Codex],
            hidden_models: vec![ModelRef {
                provider: ProviderKind::Claude,
                id: "haiku".into(),
            }],
            ..Settings::default()
        };
        let agent_on = Settings {
            disabled_providers: Vec::new(),
            ..off.clone()
        };
        let model_on = Settings {
            hidden_models: Vec::new(),
            ..off.clone()
        };
        assert!(wakes_waiting_work(&off, &agent_on));
        assert!(wakes_waiting_work(&off, &model_on));
        let worker_allowed = note_models(
            &note_models(&off, ProviderKind::Claude, &ids(&["opus"]), 1).expect("new"),
            ProviderKind::Claude,
            &ids(&["opus", "fresh"]),
            2,
        )
        .expect("fresh is new");
        let allowed = Settings {
            routing_overrides: Vec::new(),
            ..worker_allowed.clone()
        };
        assert!(wakes_waiting_work(&worker_allowed, &allowed));
        let unrelated = Settings {
            keep_awake_lid_closed: !off.keep_awake_lid_closed,
            ..off.clone()
        };
        assert!(!wakes_waiting_work(&off, &unrelated));
    }

    #[test]
    fn unavailable_models_and_agents_off_are_not_routable() {
        let settings = Settings {
            disabled_providers: vec![ProviderKind::Codex],
            hidden_models: vec![ModelRef {
                provider: ProviderKind::Claude,
                id: "haiku".into(),
            }],
            ..Settings::default()
        };
        assert!(model_available(&settings, ProviderKind::Claude, "opus"));
        assert!(!model_available(&settings, ProviderKind::Claude, "haiku"));
        assert!(!model_available(&settings, ProviderKind::Codex, "sol"));
        let states = [
            ProviderState {
                provider: ProviderKind::Claude,
                logged_in: true,
                quota: None,
            },
            ProviderState {
                provider: ProviderKind::Codex,
                logged_in: true,
                quota: None,
            },
        ];
        let (_, providers) = routable(&settings, &[], &states);
        assert!(providers[0].logged_in);
        assert!(!providers[1].logged_in);
    }
}
