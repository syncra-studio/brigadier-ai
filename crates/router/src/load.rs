//! Reading the registry document: the copy built into the app and copies downloaded from the
//! repository go through the same checks, so a downloaded copy can never do more than the
//! bundled one could.
//!
//! - **Size:** at most [`MAX_REGISTRY_BYTES`].
//! - **Format:** a `schemaVersion` other than [`SCHEMA_VERSION`] is refused (the app keeps what
//!   it has). Within a version, names this app doesn't know (a modality, a capability, a task
//!   category or area) are dropped where they appear; everything else must parse.
//! - **Bounds**, enforced here whatever the data says: entries for a CLI Brigadier doesn't drive
//!   and entries naming an expensive model (Fable, Mythos) are dropped; efforts keep only `low`, `medium` and `high`
//!   (a default above `high` becomes `high`, one below `low` becomes `low`); strengths are
//!   clamped to 0–10 and area modifiers to ±2; a context window outside
//!   [`MIN_CONTEXT_WINDOW`]..=[`MAX_CONTEXT_WINDOW`] counts as unknown; a capability is kept only
//!   if the CLI's adapter implements it (Claude: web search; Codex: web search and image
//!   generation), and image output only with image generation.
//! - **Roles:** an entry's `only` limits it to the categories it lists, at the tier given for
//!   each. A category this app doesn't know is dropped from it, and an `only` left empty takes
//!   nothing (never every category).
//! - **Consistency:** entry keys are unique, no id is claimed by two entries, and at least one
//!   entry is left; otherwise the document is refused.

use std::collections::{BTreeMap, HashSet};
use std::sync::OnceLock;

use brigadier_providers::ProviderKind;
use serde_json::{Map, Value};

use crate::registry::{
    Capability, Modality, Registry, RegistryInfo, RegistryModel, RegistrySource,
};
use crate::table;
use crate::{Area, TaskCategory};

/// The registry format this app reads.
pub const SCHEMA_VERSION: u32 = 1;
/// The largest registry document read.
pub const MAX_REGISTRY_BYTES: usize = 1024 * 1024;
/// Context windows outside this range (in tokens) are treated as unknown.
pub const MIN_CONTEXT_WINDOW: i64 = 8_000;
pub const MAX_CONTEXT_WINDOW: i64 = 20_000_000;
/// Strengths are on a 0–10 scale.
pub const MAX_STRENGTH: f64 = 10.0;
/// Area modifiers move a category strength by at most this much either way.
pub const MAX_AREA_MODIFIER: f64 = 2.0;

/// The effort levels a registry may name, weakest first (never above `high`).
const EFFORTS: [&str; 3] = ["low", "medium", "high"];

const BUNDLED: &str = include_str!("../../../registry/models.json");

/// Why a registry document was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    #[error("the registry is {0} bytes, over the {MAX_REGISTRY_BYTES}-byte limit")]
    TooLarge(usize),
    #[error("the registry is not valid: {0}")]
    Malformed(String),
    #[error("the registry's schema version {0} is not one this app reads ({SCHEMA_VERSION})")]
    UnsupportedSchema(u64),
}

/// A registry that passed the checks, with what the bounds changed or dropped (for logs).
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub registry: Registry,
    /// One line per change ("claude-opus: effort max dropped").
    pub adjustments: Vec<String>,
}

impl Registry {
    /// The copy built into the app. It passes the same checks as a downloaded copy; the build
    /// ships a valid one, so a failure here is a packaging bug.
    pub fn bundled() -> Registry {
        static BUNDLED_REGISTRY: OnceLock<Registry> = OnceLock::new();
        BUNDLED_REGISTRY
            .get_or_init(|| match Registry::parse(BUNDLED.as_bytes()) {
                Ok(registry) => registry,
                Err(error) => panic!("the bundled registry/models.json is invalid: {error}"),
            })
            .clone()
    }

    /// Reads a registry document, enforcing the size cap, the schema version and the bounds
    /// (see the module docs).
    pub fn parse(bytes: &[u8]) -> Result<Registry, RegistryError> {
        Registry::parse_with_adjustments(bytes).map(|parsed| parsed.registry)
    }

    /// [`Registry::parse`], also saying what the bounds changed or dropped.
    pub fn parse_with_adjustments(bytes: &[u8]) -> Result<Parsed, RegistryError> {
        if bytes.len() > MAX_REGISTRY_BYTES {
            return Err(RegistryError::TooLarge(bytes.len()));
        }
        let mut document: Value = serde_json::from_slice(bytes)
            .map_err(|error| RegistryError::Malformed(error.to_string()))?;
        let Some(root) = document.as_object_mut() else {
            return Err(malformed("the document is not a JSON object"));
        };
        match root.get("schemaVersion").and_then(Value::as_u64) {
            Some(version) if version == u64::from(SCHEMA_VERSION) => {}
            Some(version) => return Err(RegistryError::UnsupportedSchema(version)),
            None => return Err(malformed("schemaVersion is missing")),
        }
        let mut adjustments = Vec::new();
        let Some(models) = root.get_mut("models").and_then(Value::as_array_mut) else {
            return Err(malformed("models is missing"));
        };
        // An entry for a CLI Brigadier doesn't drive is skipped before the typed read, so its
        // shape (perhaps fields this app doesn't know) can't refuse the whole document.
        models.retain(|model| {
            let cli = model.get("cli").and_then(Value::as_str);
            let driven = cli.is_some_and(|cli| provider_of(cli).is_some());
            if !driven {
                let key = model
                    .get("key")
                    .and_then(Value::as_str)
                    .unwrap_or("an entry");
                adjustments.push(format!(
                    "{key}: skipped, Brigadier doesn't drive the {:?} CLI",
                    cli.unwrap_or_default()
                ));
            }
            driven
        });
        for model in models.iter_mut() {
            drop_unknown_names(model, &mut adjustments);
        }
        let registry: Registry = serde_json::from_value(document)
            .map_err(|error| RegistryError::Malformed(error.to_string()))?;
        let registry = bound(registry, &mut adjustments)?;
        Ok(Parsed {
            registry,
            adjustments,
        })
    }

    /// Whether this copy may replace `current`: only a higher revision does (no rollback).
    pub fn is_newer_than(&self, current: &Registry) -> bool {
        self.revision > current.revision
    }

    /// The registry to use: a cached download when it is newer than the bundled copy, else the
    /// bundled one (so a newer app always beats an older cache).
    pub fn effective(downloaded: Option<Registry>) -> (Registry, RegistrySource) {
        let bundled = Registry::bundled();
        match downloaded {
            Some(downloaded) if downloaded.is_newer_than(&bundled) => {
                (downloaded, RegistrySource::Downloaded)
            }
            _ => (bundled, RegistrySource::Bundled),
        }
    }

    /// The Usage page's summary of this registry.
    pub fn info(
        &self,
        source: RegistrySource,
        fetched_at_ms: Option<i64>,
        checked_at_ms: Option<i64>,
        error: Option<String>,
    ) -> RegistryInfo {
        RegistryInfo {
            revision: self.revision,
            updated: self.updated.clone(),
            source,
            models: u32::try_from(self.models.len()).unwrap_or(u32::MAX),
            fetched_at_ms,
            checked_at_ms,
            error,
        }
    }

    /// The entry with this key.
    pub fn entry(&self, key: &str) -> Option<&RegistryModel> {
        self.models.iter().find(|model| model.key == key)
    }

    /// The entries for one CLI, in document order.
    pub fn entries(
        &self,
        provider: ProviderKind,
    ) -> impl DoubleEndedIterator<Item = &RegistryModel> {
        self.models
            .iter()
            .filter(move |model| provider_of(&model.cli) == Some(provider))
    }

    /// Every family word the registry names for one CLI.
    pub fn families(&self, provider: ProviderKind) -> impl Iterator<Item = &str> {
        self.entries(provider)
            .filter_map(|model| model.matches.family.as_deref())
    }
}

/// The CLI a registry entry names, if Brigadier drives it.
pub(crate) fn provider_of(cli: &str) -> Option<ProviderKind> {
    ProviderKind::ALL
        .into_iter()
        .find(|provider| cli.eq_ignore_ascii_case(provider.binary()))
}

/// The capabilities a CLI's adapter implements; a registry can't enable others.
pub(crate) fn adapter_capabilities(provider: ProviderKind) -> &'static [Capability] {
    match provider {
        ProviderKind::Claude => &[Capability::WebSearch],
        ProviderKind::Codex => &[Capability::WebSearch, Capability::ImageGeneration],
    }
}

fn malformed(what: &str) -> RegistryError {
    RegistryError::Malformed(what.to_owned())
}

// ----- names this app doesn't know ----------------------------------------------------------

/// Drops the modality, capability, category and area names this app doesn't know from one raw
/// entry, so a newer document within the same schema version still reads.
fn drop_unknown_names(model: &mut Value, adjustments: &mut Vec<String>) {
    let key = model
        .get("key")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_owned();
    let Some(model) = model.as_object_mut() else {
        return;
    };
    if let Some(modalities) = model.get_mut("modalities").and_then(Value::as_object_mut) {
        for field in ["input", "output"] {
            retain_names::<Modality>(modalities, field, &key, adjustments);
        }
        retain_names::<Capability>(modalities, "tools", &key, adjustments);
    }
    for field in ["strengths", "defaultEffort", "only"] {
        retain_keys::<TaskCategory>(model, field, &key, adjustments);
    }
    retain_keys::<Area>(model, "areaStrengths", &key, adjustments);
}

fn known<T: serde::de::DeserializeOwned>(name: &str) -> bool {
    serde_json::from_value::<T>(Value::String(name.to_owned())).is_ok()
}

fn retain_names<T: serde::de::DeserializeOwned>(
    object: &mut Map<String, Value>,
    field: &str,
    key: &str,
    adjustments: &mut Vec<String>,
) {
    if let Some(names) = object.get_mut(field).and_then(Value::as_array_mut) {
        names.retain(|name| match name.as_str() {
            Some(text) if !known::<T>(text) => {
                adjustments.push(format!("{key}: unknown {field} name {text:?} dropped"));
                false
            }
            _ => true,
        });
    }
}

fn retain_keys<T: serde::de::DeserializeOwned>(
    object: &mut Map<String, Value>,
    field: &str,
    key: &str,
    adjustments: &mut Vec<String>,
) {
    if let Some(map) = object.get_mut(field).and_then(Value::as_object_mut) {
        map.retain(|name, _| {
            let keep = known::<T>(name);
            if !keep {
                adjustments.push(format!("{key}: unknown {field} key {name:?} dropped"));
            }
            keep
        });
    }
}

// ----- bounds -------------------------------------------------------------------------------

fn bound(mut registry: Registry, adjustments: &mut Vec<String>) -> Result<Registry, RegistryError> {
    if registry.revision == 0 {
        return Err(malformed("revision must be at least 1"));
    }
    if !is_date(&registry.updated) {
        return Err(malformed("updated must be a YYYY-MM-DD date"));
    }
    let mut kept = Vec::with_capacity(registry.models.len());
    for model in registry.models {
        let Some(provider) = provider_of(&model.cli) else {
            adjustments.push(format!(
                "{}: skipped, Brigadier doesn't drive the {:?} CLI",
                model.key, model.cli
            ));
            continue;
        };
        if names_expensive(&model) {
            adjustments.push(format!(
                "{}: skipped, expensive models are never rated or routed to automatically",
                model.key
            ));
            continue;
        }
        kept.push(bound_model(model, provider, adjustments)?);
    }
    if kept.is_empty() {
        return Err(malformed("no usable model entries"));
    }
    let mut keys = HashSet::new();
    let mut ids = HashSet::new();
    for model in &kept {
        if model.key.trim().is_empty() || model.vendor.trim().is_empty() {
            return Err(malformed("an entry has an empty key or vendor"));
        }
        if !keys.insert(model.key.as_str()) {
            return Err(RegistryError::Malformed(format!(
                "the key {:?} appears twice",
                model.key
            )));
        }
        for id in &model.matches.ids {
            if !ids.insert((model.cli.to_ascii_lowercase(), id.to_ascii_lowercase())) {
                return Err(RegistryError::Malformed(format!(
                    "the id {id:?} is claimed by two entries"
                )));
            }
        }
    }
    registry.models = kept;
    Ok(registry)
}

fn names_expensive(model: &RegistryModel) -> bool {
    table::names_expensive(&model.key)
        || model
            .matches
            .ids
            .iter()
            .any(|id| table::names_expensive(id))
        || model
            .matches
            .family
            .as_deref()
            .is_some_and(table::names_expensive)
}

fn bound_model(
    mut model: RegistryModel,
    provider: ProviderKind,
    adjustments: &mut Vec<String>,
) -> Result<RegistryModel, RegistryError> {
    let key = model.key.clone();
    let mut note = |what: String| adjustments.push(format!("{key}: {what}"));

    model.matches.ids.retain(|id| !id.trim().is_empty());
    if model
        .matches
        .family
        .as_deref()
        .is_some_and(|family| family.trim().is_empty() || table::words(family).count() != 1)
    {
        note("family must be one word; dropped".to_owned());
        model.matches.family = None;
    }
    if model.matches.ids.is_empty() && model.matches.family.is_none() {
        return Err(RegistryError::Malformed(format!(
            "{key}: an entry needs an id or a family to match"
        )));
    }

    let mut efforts: Vec<String> = Vec::new();
    for effort in &model.efforts {
        let lower = effort.to_ascii_lowercase();
        if EFFORTS.contains(&lower.as_str()) {
            if !efforts.contains(&lower) {
                efforts.push(lower);
            }
        } else {
            note(format!("effort {effort:?} dropped"));
        }
    }
    efforts.sort_by_key(|effort| table::rank(effort));
    model.efforts = efforts;

    let mut defaults = BTreeMap::new();
    for (category, effort) in std::mem::take(&mut model.default_effort) {
        match clamp_effort(&effort) {
            Some(clamped) => {
                if clamped != effort {
                    note(format!("{category:?} effort {effort:?} → {clamped:?}"));
                }
                defaults.insert(category, clamped.to_owned());
            }
            None => note(format!("{category:?} effort {effort:?} dropped")),
        }
    }
    model.default_effort = defaults;

    for (category, strength) in model.strengths.iter_mut() {
        let clamped = strength.clamp(0.0, MAX_STRENGTH);
        if clamped != *strength {
            note(format!("{category:?} strength {strength} → {clamped}"));
            *strength = clamped;
        }
    }
    for (area, modifier) in model.area_strengths.iter_mut() {
        let clamped = modifier.clamp(-MAX_AREA_MODIFIER, MAX_AREA_MODIFIER);
        if clamped != *modifier {
            note(format!("{area:?} modifier {modifier} → {clamped}"));
            *modifier = clamped;
        }
    }

    if let Some(window) = model.context_window
        && !(MIN_CONTEXT_WINDOW..=MAX_CONTEXT_WINDOW).contains(&window)
    {
        note(format!("context window {window} out of range; unknown"));
        model.context_window = None;
    }
    if let Some(cutoff) = &model.knowledge_cutoff
        && !is_month(cutoff)
    {
        note(format!(
            "knowledge cutoff {cutoff:?} is not YYYY-MM; dropped"
        ));
        model.knowledge_cutoff = None;
    }
    if let Some(released) = &model.released
        && !is_date(released)
    {
        note(format!(
            "release date {released:?} is not YYYY-MM-DD; dropped"
        ));
        model.released = None;
    }

    if model.only.as_ref().is_some_and(|only| only.is_empty()) {
        note("takes only categories this app doesn't know; never routed to".to_owned());
    }

    let implemented = adapter_capabilities(provider);
    let mut tools: Vec<Capability> = Vec::new();
    for tool in &model.modalities.tools {
        if !implemented.contains(tool) {
            note(format!(
                "capability {tool:?} not implemented by its CLI; dropped"
            ));
        } else if !tools.contains(tool) {
            tools.push(*tool);
        }
    }
    model.modalities.tools = tools;
    dedup(&mut model.modalities.input);
    dedup(&mut model.modalities.output);
    if model.modalities.output.contains(&Modality::Image)
        && !model
            .modalities
            .tools
            .contains(&Capability::ImageGeneration)
    {
        note("image output without image generation; dropped".to_owned());
        model.modalities.output.retain(|m| *m != Modality::Image);
    }
    Ok(model)
}

fn dedup<T: PartialEq + Copy>(items: &mut Vec<T>) {
    let mut seen: Vec<T> = Vec::new();
    items.retain(|item| {
        let new = !seen.contains(item);
        if new {
            seen.push(*item);
        }
        new
    });
}

/// An effort a registry names, fitted into `low`..`high`; `None` for a level it doesn't know.
pub(crate) fn clamp_effort(effort: &str) -> Option<&'static str> {
    let rank = table::rank(effort)?;
    let low = table::rank("low")?;
    let high = table::rank("high")?;
    Some(if rank <= low {
        "low"
    } else if rank >= high {
        "high"
    } else {
        "medium"
    })
}

fn is_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10 && bytes[4] == b'-' && bytes[7] == b'-' && is_month(&text[..7]) && {
        let day = &text[8..];
        day.bytes().all(|b| b.is_ascii_digit()) && (1..=31).contains(&day.parse().unwrap_or(0))
    }
}

fn is_month(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 7
        && bytes[4] == b'-'
        && text[..4].bytes().all(|b| b.is_ascii_digit())
        && text[5..].bytes().all(|b| b.is_ascii_digit())
        && (1..=12).contains(&text[5..].parse::<u32>().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::QualityTier;

    fn document(only: Option<serde_json::Value>) -> Vec<u8> {
        let mut model = serde_json::json!({
            "key": "small", "vendor": "anthropic", "cli": "claude",
            "match": {"ids": ["claude-small-1"]}, "tier": "light"
        });
        if let Some(only) = only {
            model["only"] = only;
        }
        serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1, "revision": 1, "updated": "2026-10-13", "models": [model]
        }))
        .unwrap()
    }

    /// `only` keeps a model to the categories it names; a category this app doesn't know is
    /// dropped from it, and one left empty takes nothing rather than everything.
    #[test]
    fn only_limits_categories_and_never_widens_when_its_names_are_unknown() {
        let plain = Registry::parse(&document(None)).unwrap();
        assert_eq!(plain.models[0].only, None);

        let operate = Registry::parse(&document(Some(serde_json::json!({
            "operate": "strong", "painting": "frontier"
        }))))
        .unwrap();
        assert_eq!(
            operate.models[0].only,
            Some(BTreeMap::from([(
                TaskCategory::Operate,
                QualityTier::Strong
            )]))
        );

        let unknown = Registry::parse_with_adjustments(&document(Some(serde_json::json!({
            "painting": "strong"
        }))))
        .unwrap();
        assert_eq!(unknown.registry.models[0].only, Some(BTreeMap::new()));
        assert!(
            unknown
                .adjustments
                .iter()
                .any(|note| note.contains("never routed to"))
        );
    }
}
