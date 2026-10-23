//! Which agents and models the user lets Brigadier use (Settings → Providers). Explicit session
//! and worker choices use only the user's model rules file.
//!
//! - An agent switched off ([`Settings::disabled_providers`]) and a model made unavailable
//!   ([`Settings::hidden_models`]) are out of every picker and the commit message writer.
//!   Conversations already running keep their model.
//! - An older model ([`brigadier_providers::ModelInfo::legacy`]) starts unavailable, once
//!   ([`Settings::older_models_off`]): the user turns on the ones they want, and that choice
//!   stands. One the user's own choices already name stays as it is.

use brigadier_providers::ProviderKind;
use brigadier_router::{OverrideEffect, OverrideTarget};

use crate::model::{ModelChoice, ModelRef, SETTINGS_VERSION, Settings};

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

/// The settings with the models of `provider`'s list (`ids`) recorded as known and its older
/// ones (`older`: each its id and the id it resolves to) turned off the first time they are
/// seen, or `None` when nothing is new.
pub fn note_models(
    settings: &Settings,
    provider: ProviderKind,
    ids: &[String],
    older: &[(String, Option<String>)],
) -> Option<Settings> {
    let mut next = settings.clone();
    for id in ids {
        let model = ModelRef {
            provider,
            id: id.clone(),
        };
        if !next.known_models.contains(&model) {
            next.known_models.push(model);
        }
    }
    for (id, resolved) in older {
        let model = ModelRef {
            provider,
            id: id.clone(),
        };
        if next.older_models_off.contains(&model) {
            continue;
        }
        next.older_models_off.push(model.clone());
        let named = |name: &str| name == id || resolved.as_deref() == Some(name);
        if !next.hidden_models.contains(&model) && !named_by_user(&next, provider, named) {
            next.hidden_models.push(model);
        }
    }
    (next != *settings).then_some(next)
}

/// Whether the user's own choices name this model (`named` matches its id or the id it
/// resolves to): the default chat or orchestrator model, or the commit message writer. It is
/// in use by choice.
fn named_by_user(
    settings: &Settings,
    provider: ProviderKind,
    named: impl Fn(&str) -> bool,
) -> bool {
    let chosen = |choice: &Option<ModelChoice>| {
        choice.as_ref().is_some_and(|choice| {
            choice.provider == provider && choice.model.as_deref().is_some_and(&named)
        })
    };
    [
        &settings.default_chat_model,
        &settings.default_orchestrator,
        &settings.git_writer,
    ]
    .into_iter()
    .any(chosen)
}

/// Settings a client sent, written over `current`: a model recorded as known since the client
/// read its copy stays known, and an older model turned off meanwhile stays off, so a page open
/// from before can't turn it back on.
pub fn rebase(current: &Settings, mut incoming: Settings) -> Settings {
    for model in &current.known_models {
        if !incoming.known_models.contains(model) {
            incoming.known_models.push(model.clone());
        }
    }
    for model in &current.older_models_off {
        if incoming.older_models_off.contains(model) {
            continue;
        }
        incoming.older_models_off.push(model.clone());
        if current.hidden_models.contains(model) && !incoming.hidden_models.contains(model) {
            incoming.hidden_models.push(model.clone());
        }
    }
    incoming.settings_version = incoming.settings_version.max(current.settings_version);
    incoming
}

/// Whether a settings change may let work waiting for quota run now: an agent or model turned
/// back on.
pub fn wakes_waiting_work(before: &Settings, after: &Settings) -> bool {
    before.disabled_providers != after.disabled_providers
        || before.hidden_models != after.hidden_models
}

/// Saved settings brought up to [`SETTINGS_VERSION`], or `None` when they already are.
///
/// Version 1: an agent switched off before used to be a `never` rule for the whole agent,
/// everywhere, for all work; such a rule becomes the agent switched off.
///
/// Versions 2 and 5 widened routing rules, which Brigadier no longer keeps (S16b).
///
/// Version 3: merging on its own used to default on; settings saved before this version read
/// it as off, once. A user's own later choice (on or off) after that is kept from then on.
///
/// Version 4: account switching is set per agent, and the one switch saved before covers
/// every agent: off, every agent's work stays on its default account.
pub fn migrate(settings: &Settings) -> Option<Settings> {
    if settings.settings_version >= SETTINGS_VERSION {
        return None;
    }
    let mut next = settings.clone();
    if settings.settings_version < 1 {
        for rule in &settings.legacy_routing_overrides {
            let OverrideTarget::Vendor { provider } = rule.target else {
                continue;
            };
            let off = rule.effect == OverrideEffect::Never
                && rule.categories.is_empty()
                && rule.areas.is_empty()
                && rule.project_id.is_none();
            if off && !next.disabled_providers.contains(&provider) {
                next.disabled_providers.push(provider);
            }
        }
    }
    if settings.settings_version < 4 && !settings.legacy_switch_accounts {
        for provider in ProviderKind::ALL {
            if !next.manual_accounts.contains(&provider) {
                next.manual_accounts.push(provider);
            }
        }
    }
    next.legacy_routing_overrides.clear();
    next.settings_version = SETTINGS_VERSION;
    Some(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn an_older_model_starts_off_once_and_the_users_choice_stands() {
        let old = ModelRef {
            provider: ProviderKind::Claude,
            id: "opus-4-8".into(),
        };
        let older = [("opus-4-8".to_owned(), Some("claude-opus-4-8".to_owned()))];
        let list = ids(&["opus", "opus-4-8"]);
        let next = note_models(&Settings::default(), ProviderKind::Claude, &list, &older)
            .expect("the models are new");
        assert_eq!(next.known_models.len(), 2);
        assert_eq!(next.hidden_models, vec![old.clone()]);
        assert!(!model_available(&next, ProviderKind::Claude, "opus-4-8"));
        assert!(model_available(&next, ProviderKind::Claude, "opus"));
        // The user turns it on: the next list leaves it on.
        let on = Settings {
            hidden_models: Vec::new(),
            ..next.clone()
        };
        assert!(note_models(&on, ProviderKind::Claude, &list, &older).is_none());
        // One the default chat model names stays on.
        let chat = Settings {
            default_chat_model: Some(ModelChoice {
                provider: ProviderKind::Claude,
                model: Some("opus-4-8".into()),
                effort: None,
                fast: None,
                account: None,
            }),
            ..Settings::default()
        };
        let kept = note_models(&chat, ProviderKind::Claude, &list, &older).expect("noted");
        assert!(kept.hidden_models.is_empty());
        assert_eq!(kept.older_models_off, vec![old.clone()]);
        // A page open from before can't turn it back on, or forget the models, by writing its
        // old copy.
        let written = rebase(&next, Settings::default());
        assert!(!model_available(&written, ProviderKind::Claude, "opus-4-8"));
        assert_eq!(written.known_models, next.known_models);
    }

    #[test]
    fn the_old_agent_switch_becomes_the_agent_off() {
        let settings: Settings = serde_json::from_str(
            r#"{"routingOverrides":[
                {"id":"off","effect":"never","target":{"type":"vendor","provider":"codex"},"categories":[],"areas":[],"projectId":null,"createdAtMs":0},
                {"id":"reviews","effect":"never","target":{"type":"vendor","provider":"claude"},"categories":["review"],"createdAtMs":0}
            ],"settingsVersion":0}"#,
        )
        .unwrap();
        let next = migrate(&settings).expect("version 0 converts");
        assert_eq!(next.disabled_providers, vec![ProviderKind::Codex]);
        assert_eq!(next.settings_version, SETTINGS_VERSION);
        assert!(migrate(&next).is_none());
        // The old rules are read once, never saved again.
        let json = serde_json::to_value(&next).unwrap();
        assert!(json.get("routingOverrides").is_none());
    }

    #[test]
    fn settings_saved_with_routing_still_load() {
        let saved: Settings = serde_json::from_str(
            r#"{"routingOverrides":[],"routingRankings":[{"id":"r","category":"review","manual":true,"updatedAtMs":0}],"knownModels":[{"provider":"claude","id":"opus"}],"switchProviders":false,"userName":"Ann","settingsVersion":5}"#,
        )
        .unwrap();
        assert_eq!(saved.user_name, "Ann");
        assert!(migrate(&saved).is_none());
    }

    #[test]
    fn the_one_account_switch_saved_before_becomes_each_agents_own() {
        let saved = |json: &str| -> Settings { serde_json::from_str(json).unwrap() };
        let off = migrate(&saved(r#"{"switchAccounts":false,"settingsVersion":3}"#))
            .expect("version 3 converts");
        assert_eq!(off.manual_accounts, ProviderKind::ALL.to_vec());
        let on = migrate(&saved(r#"{"switchAccounts":true,"settingsVersion":3}"#))
            .expect("version 3 converts");
        assert!(on.manual_accounts.is_empty());
        let unset = migrate(&saved(r#"{"settingsVersion":3}"#)).expect("version 3 converts");
        assert!(unset.manual_accounts.is_empty());
        // The old switch is read once, never saved again.
        let json = serde_json::to_value(&off).unwrap();
        assert!(json.get("switchAccounts").is_none());
        assert!(migrate(&off).is_none());
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
        let unrelated = Settings {
            keep_awake_lid_closed: !off.keep_awake_lid_closed,
            ..off.clone()
        };
        assert!(!wakes_waiting_work(&off, &unrelated));
    }

    #[test]
    fn unavailable_models_and_agents_off_are_out() {
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
        assert!(provider_on(&settings, ProviderKind::Claude));
        assert!(!provider_on(&settings, ProviderKind::Codex));
    }
}
