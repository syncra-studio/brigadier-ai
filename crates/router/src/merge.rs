//! Merging the CLIs' live model lists with the registry, research notes and trial counts: the
//! catalog routing chooses from.
//!
//! A listed model is:
//! - **Curated** when its id, or the concrete model its alias resolves to, is one of an entry's
//!   `match.ids`;
//! - **Inherited** when it has an entry's `match.family` word in its id (or resolved id) and is
//!   not older than that entry's own models: `gpt-6.1-sol` inherits the `sol` entry, while an old
//!   `gpt-5-sol` the registry never listed does not. Several entries with the family: the one
//!   released last;
//! - **Researched** when a research note places it (tier at most `standard`, strengths within
//!   ±2 of the unrated 5);
//! - **Unknown** otherwise: tier `unrated`, strength 5 everywhere.
//!
//! Researched and unknown models are on trial until they
//! have [`TRIAL_OUTCOMES`] outcomes.

use std::collections::BTreeMap;

use brigadier_providers::{ModelInfo, ProviderKind};

use crate::load::{MAX_CONTEXT_WINDOW, MIN_CONTEXT_WINDOW};
use crate::registry::{
    MergedModel, Modalities, Modality, ModelStatus, QualityTier, RatingProvenance, Registry,
    RegistryModel, ResearchNote, TrialState,
};
use crate::{TaskCategory, table};

/// Outcomes a new model needs before it is scored like the others (its trial ends).
pub const TRIAL_OUTCOMES: u32 = 3;
/// The strength of a model nobody has rated, in every category.
pub const UNRATED_STRENGTH: f64 = 5.0;
/// How far research may move a strength from [`UNRATED_STRENGTH`], either way.
pub const RESEARCH_REACH: f64 = 2.0;
/// The highest tier research alone can place a model at (two steps above unrated).
pub const RESEARCH_MAX_TIER: QualityTier = QualityTier::Standard;

/// How many outcomes a model has (across projects), for its trial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeCount {
    pub provider: ProviderKind,
    /// The model as outcomes record it (the concrete model), or its listed id.
    pub model: String,
    pub outcomes: u32,
}

/// Merges each provider's live model list (in the CLI's order) with the registry, research
/// notes and outcome counts. See the module docs.
pub fn merge(
    registry: &Registry,
    catalogs: &[(ProviderKind, &[ModelInfo])],
    research: &[ResearchNote],
    counts: &[OutcomeCount],
) -> Vec<MergedModel> {
    let mut merged = Vec::new();
    for (provider, models) in catalogs {
        for info in models.iter() {
            merged.push(merge_one(registry, *provider, info, research, counts));
        }
    }
    merged
}

fn merge_one(
    registry: &Registry,
    provider: ProviderKind,
    info: &ModelInfo,
    research: &[ResearchNote],
    counts: &[OutcomeCount],
) -> MergedModel {
    let efforts = bounded_efforts(&info.efforts);
    let input = cli_modalities(&info.input_modalities);
    let mut model = MergedModel {
        provider,
        id: info.id.clone(),
        resolved: info.resolved.clone(),
        display_name: info.display_name.clone(),
        status: ModelStatus::Unknown,
        rating_provenance: RatingProvenance::Unrated,
        default_effort: BTreeMap::new(),
        registry_key: None,
        family: None,
        tier: QualityTier::Unrated,
        only: None,
        strengths: unrated_strengths(),
        area_strengths: BTreeMap::new(),
        efforts,
        context_window: None,
        knowledge_cutoff: None,
        modalities: Modalities {
            input,
            output: vec![Modality::Text],
            tools: Vec::new(),
        },
        legacy: info.legacy,
        excluded: false,
        trial: None,
        research: None,
    };
    let resolved = info.resolved.as_deref();
    if let Some((entry, status)) = match_entry(registry, provider, &info.id, resolved) {
        apply_entry(&mut model, entry, status, &info.input_modalities);
        model.family = family_of(registry, provider, entry, &info.id, resolved);
        return model;
    }
    model.family = registry
        .families(provider)
        .find(|family| names_word(&info.id, resolved, family))
        .map(str::to_owned);
    let outcomes = count_for(counts, provider, &info.id, resolved);
    model.trial = Some(TrialState {
        outcomes,
        needed: TRIAL_OUTCOMES,
    });
    if let Some(note) = research
        .iter()
        .filter(|note| {
            note.provider == provider
                && (note.model.eq_ignore_ascii_case(&info.id)
                    || resolved.is_some_and(|r| note.model.eq_ignore_ascii_case(r)))
        })
        .max_by_key(|note| note.at_ms)
    {
        model.status = ModelStatus::Researched;
        model.rating_provenance = RatingProvenance::ResearchNote;
        model.tier = note.tier.min(RESEARCH_MAX_TIER);
        for (category, strength) in &note.strengths {
            model.strengths.insert(
                *category,
                strength.clamp(
                    UNRATED_STRENGTH - RESEARCH_REACH,
                    UNRATED_STRENGTH + RESEARCH_REACH,
                ),
            );
        }
        model.context_window = note
            .context_window
            .filter(|window| (MIN_CONTEXT_WINDOW..=MAX_CONTEXT_WINDOW).contains(window));
        model.knowledge_cutoff = note.knowledge_cutoff.clone();
        model.research = Some(note.clone());
    }
    model
}

fn apply_entry(
    model: &mut MergedModel,
    entry: &RegistryModel,
    status: ModelStatus,
    cli_input: &[String],
) {
    model.status = status;
    model.rating_provenance = RatingProvenance::Curated;
    model.default_effort = entry.default_effort.clone();
    model.registry_key = Some(entry.key.clone());
    model.tier = entry.tier;
    model.only = entry.only.clone();
    for (category, strength) in &entry.strengths {
        model.strengths.insert(*category, *strength);
    }
    model.area_strengths = entry.area_strengths.clone();
    if !entry.efforts.is_empty() {
        model
            .efforts
            .retain(|effort| entry.efforts.contains(effort));
    }
    model.context_window = entry.context_window;
    model.knowledge_cutoff = entry.knowledge_cutoff.clone();
    let mut input = entry.modalities.input.clone();
    if input.is_empty() {
        input = model.modalities.input.clone();
    } else if !cli_input.is_empty() {
        // The CLI's own list is the live truth about what it will send the model.
        input.retain(|modality| model.modalities.input.contains(modality));
    }
    model.modalities = Modalities {
        input,
        output: if entry.modalities.output.is_empty() {
            vec![Modality::Text]
        } else {
            entry.modalities.output.clone()
        },
        tools: entry.modalities.tools.clone(),
    };
}

/// The registry entry for a listed model, and how it matched.
pub(crate) fn match_entry<'r>(
    registry: &'r Registry,
    provider: ProviderKind,
    id: &str,
    resolved: Option<&str>,
) -> Option<(&'r RegistryModel, ModelStatus)> {
    let claims = |entry: &&RegistryModel, name: &str| {
        entry
            .matches
            .ids
            .iter()
            .any(|claimed| claimed.eq_ignore_ascii_case(name))
    };
    if let Some(entry) = registry.entries(provider).find(|entry| claims(entry, id)) {
        // An alias (`opus`) that now resolves to a newer model than the entry knows is that
        // newer model: it inherits the family entry below instead of passing for the old one.
        let moved_on = resolved.is_some_and(|resolved| {
            !claims(&entry, resolved)
                && entry
                    .matches
                    .ids
                    .iter()
                    .map(|claimed| version_of(claimed))
                    .max()
                    .is_some_and(|known| version_of(resolved) > known)
        });
        if !moved_on {
            return Some((entry, ModelStatus::Curated));
        }
    }
    if let Some(resolved) = resolved
        && let Some(entry) = registry
            .entries(provider)
            .find(|entry| claims(entry, resolved))
    {
        return Some((entry, ModelStatus::Curated));
    }
    let own = version_of(resolved.unwrap_or(id)).max(version_of(id));
    registry
        .entries(provider)
        .filter(|entry| {
            entry
                .matches
                .family
                .as_deref()
                .is_some_and(|family| names_word(id, resolved, family))
        })
        // Newest entry of the family: released last, then first in the document.
        .rev()
        .max_by(|a, b| a.released.cmp(&b.released))
        .filter(|entry| {
            let known = entry.matches.ids.iter().map(|id| version_of(id)).max();
            known.is_none_or(|known| own >= known)
        })
        .map(|entry| (entry, ModelStatus::Inherited))
}

/// A model's family word: its entry's, else any registry family its names contain.
fn family_of(
    registry: &Registry,
    provider: ProviderKind,
    entry: &RegistryModel,
    id: &str,
    resolved: Option<&str>,
) -> Option<String> {
    if let Some(family) = &entry.matches.family {
        return Some(family.clone());
    }
    registry
        .families(provider)
        .find(|family| {
            names_word(id, resolved, family)
                || entry
                    .matches
                    .ids
                    .iter()
                    .any(|claimed| table::has_word(claimed, family))
        })
        .map(str::to_owned)
}

fn names_word(id: &str, resolved: Option<&str>, word: &str) -> bool {
    table::has_word(id, word) || resolved.is_some_and(|resolved| table::has_word(resolved, word))
}

/// The version numbers in a model id: `gpt-6.1-sol` → [6, 1], `claude-opus-4-5` → [4, 5]
/// (a date suffix is not a version).
pub(crate) fn version_of(id: &str) -> Vec<u32> {
    table::words(id)
        .flat_map(|word| word.split('.'))
        .filter_map(|part| part.parse::<u32>().ok())
        .filter(|number| *number < 1000)
        .collect()
}

fn count_for(
    counts: &[OutcomeCount],
    provider: ProviderKind,
    id: &str,
    resolved: Option<&str>,
) -> u32 {
    counts
        .iter()
        .filter(|count| {
            count.provider == provider
                && (count.model.eq_ignore_ascii_case(id)
                    || resolved.is_some_and(|r| count.model.eq_ignore_ascii_case(r)))
        })
        .map(|count| count.outcomes)
        .sum()
}

/// The CLI's recognized effort levels, in its own order.
fn bounded_efforts(efforts: &[String]) -> Vec<String> {
    efforts
        .iter()
        .filter(|effort| table::rank(effort).is_some())
        .cloned()
        .collect()
}

fn cli_modalities(names: &[String]) -> Vec<Modality> {
    let mut modalities = vec![Modality::Text];
    if names.iter().any(|name| name.eq_ignore_ascii_case("image")) {
        modalities.push(Modality::Image);
    }
    modalities
}

fn unrated_strengths() -> BTreeMap<TaskCategory, f64> {
    TaskCategory::ALL
        .into_iter()
        .map(|category| (category, UNRATED_STRENGTH))
        .collect()
}

/// Whether `name` (an id or the concrete model an alias resolves to) is this model.
pub fn is_model(model: &MergedModel, registry: &Registry, name: &str) -> bool {
    model.id.eq_ignore_ascii_case(name)
        || model
            .resolved
            .as_deref()
            .is_some_and(|resolved| resolved.eq_ignore_ascii_case(name))
        || model
            .registry_key
            .as_deref()
            .filter(|_| model.status == crate::ModelStatus::Curated)
            .and_then(|key| registry.entry(key))
            .is_some_and(|entry| {
                entry
                    .matches
                    .ids
                    .iter()
                    .any(|id| id.eq_ignore_ascii_case(name))
            })
}
