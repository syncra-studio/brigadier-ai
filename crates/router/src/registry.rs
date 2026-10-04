//! The curated capability registry (`registry/models.json`, PLAN.md §6 Phase 5): what each
//! model is good at, as a document the app ships with and updates from the repository.
//!
//! The registry is data, not policy. It may set a model's tier, strengths, context window,
//! efforts and modalities, within bounds enforced when it is read; it never beats the user's
//! overrides or the hard rules (no Fable models, efforts at most `high`, only CLIs Brigadier
//! drives, only capabilities their adapters implement).

use std::collections::BTreeMap;

use brigadier_providers::ProviderKind;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{Area, TaskCategory};

/// How capable a model is overall. A task's quality floor is a tier too.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize, TS,
)]
#[serde(rename_all = "camelCase")]
pub enum QualityTier {
    /// Not rated: a model neither the registry nor research has placed yet. Below every
    /// floor; such a model runs only as a trial (low-risk tasks).
    Unrated,
    /// Small and fast: scouting, running checks.
    #[default]
    Light,
    /// Solid everyday model: research, simple changes.
    Standard,
    /// Strong coding model: implementation, review, merges.
    Strong,
    /// The vendor's best.
    Frontier,
}

/// Kinds of input and output a model handles. A registry naming one this app doesn't know has
/// it dropped when read, not the whole document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Modality {
    Text,
    Image,
}

/// Built-in capabilities of a model through its CLI. Unknown names are dropped when read, and a
/// capability the CLI's adapter doesn't implement is never enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Capability {
    WebSearch,
    /// Makes images (Codex only).
    ImageGeneration,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
pub struct Modalities {
    pub input: Vec<Modality>,
    pub output: Vec<Modality>,
    pub tools: Vec<Capability>,
}

/// How a registry entry is matched against the models a CLI lists.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelMatch {
    /// Exact model ids or aliases (`opus`, `claude-opus-5-5`, `gpt-6-sol`).
    pub ids: Vec<String>,
    /// A family word (`opus`, `sol`): a listed model with this word in its id and no entry of
    /// its own inherits this entry (the newest version of the family wins).
    pub family: Option<String>,
}

/// One model in the registry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RegistryModel {
    /// Stable key (`claude-opus`, `codex-sol`).
    pub key: String,
    /// Who makes it (`anthropic`, `openai`).
    pub vendor: String,
    /// The CLI that runs it (`claude`, `codex`). Entries for a CLI Brigadier doesn't drive are
    /// skipped when the registry is read.
    pub cli: String,
    #[serde(rename = "match")]
    pub matches: ModelMatch,
    pub tier: QualityTier,
    /// Effort levels it accepts, lowest first. Clamped to `low`..`high` when read.
    #[serde(default)]
    pub efforts: Vec<String>,
    #[serde(default)]
    pub context_window: Option<i64>,
    /// `YYYY-MM`.
    #[serde(default)]
    pub knowledge_cutoff: Option<String>,
    #[serde(default)]
    pub modalities: Modalities,
    /// 0–10 per task category; a missing category counts as 5.
    #[serde(default)]
    pub strengths: BTreeMap<TaskCategory, f64>,
    /// −2..+2 per area, added to the category strength.
    #[serde(default)]
    pub area_strengths: BTreeMap<Area, f64>,
    /// The effort to run it at per category (clamped like `efforts`).
    #[serde(default)]
    pub default_effort: BTreeMap<TaskCategory, String>,
    /// `YYYY-MM-DD`.
    #[serde(default)]
    pub released: Option<String>,
    /// Release notes and benchmarks the entry is based on.
    #[serde(default)]
    pub sources: Vec<String>,
}

/// The registry document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Registry {
    /// The document's format. A major the app doesn't know is not used.
    pub schema_version: u32,
    /// Grows with every published change; a copy with a revision not above the one in use is
    /// never taken (no rollback).
    pub revision: u64,
    /// `YYYY-MM-DD`.
    pub updated: String,
    pub models: Vec<RegistryModel>,
}

/// Where the registry in use came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RegistrySource {
    /// The copy built into the app.
    Bundled,
    /// A copy downloaded from the repository and cached in the data directory.
    Downloaded,
}

/// The registry in use and its last update check, for the Usage page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RegistryInfo {
    pub revision: u64,
    pub updated: String,
    pub source: RegistrySource,
    pub models: u32,
    /// When the copy in use was downloaded.
    pub fetched_at_ms: Option<i64>,
    /// When the repository was last asked for a newer copy.
    pub checked_at_ms: Option<i64>,
    /// Why the last check did not update it (network, a malformed or older copy).
    pub error: Option<String>,
}

/// How far the live catalog's model is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ModelStatus {
    /// Has its own registry entry.
    Curated,
    /// A newer member of a registry family: the family's entry, until outcomes confirm it.
    Inherited,
    /// Not in the registry; placed by a research note.
    Researched,
    /// Not in the registry and not researched yet.
    Unknown,
}

/// What a research run found out about a model the registry doesn't know (release notes,
/// benchmarks). Dated; replaced as soon as the registry curates the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ResearchNote {
    pub provider: ProviderKind,
    /// The model's id as its CLI lists it.
    pub model: String,
    pub at_ms: i64,
    /// A few sentences for the Usage page.
    pub summary: String,
    pub tier: QualityTier,
    pub strengths: BTreeMap<TaskCategory, f64>,
    pub context_window: Option<i64>,
    pub knowledge_cutoff: Option<String>,
    pub sources: Vec<String>,
}

/// A model under trial: an unknown or researched model that gets a share of low-risk tasks
/// (scout, research, verify) until it has enough outcomes to be scored like the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TrialState {
    pub outcomes: u32,
    pub needed: u32,
}

/// A model a CLI offers, merged with what the registry, research and trials say about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MergedModel {
    pub provider: ProviderKind,
    /// The id passed to the CLI.
    pub id: String,
    /// The concrete model an alias resolves to, when the CLI says.
    pub resolved: Option<String>,
    pub display_name: String,
    pub status: ModelStatus,
    pub rating_provenance: RatingProvenance,
    /// Effective category defaults for this concrete model.
    pub default_effort: BTreeMap<TaskCategory, String>,
    /// The registry entry it is (or inherits), if any.
    pub registry_key: Option<String>,
    /// Its family word (`opus`, `sol`), the registry's `match.family`: what a user rule about
    /// a family names. Absent for a model of no known family.
    pub family: Option<String>,
    pub tier: QualityTier,
    pub strengths: BTreeMap<TaskCategory, f64>,
    pub area_strengths: BTreeMap<Area, f64>,
    pub efforts: Vec<String>,
    pub context_window: Option<i64>,
    pub knowledge_cutoff: Option<String>,
    pub modalities: Modalities,
    /// An older model the CLI still lists beside a newer one.
    pub legacy: bool,
    /// Never routed to (Fable).
    pub excluded: bool,
    pub trial: Option<TrialState>,
    pub research: Option<ResearchNote>,
}

/// Where the effective ratings came from; independent of identity and trial status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RatingProvenance {
    Curated,
    ResearchedOverlay,
    ResearchNote,
    Unrated,
}

impl MergedModel {
    /// Only the provider's verified resolution groups aliases. Family keys never identify
    /// a concrete model, and a later alias resolution cannot move an existing patch.
    pub fn rating_identity(&self) -> String {
        self.resolved
            .as_deref()
            .unwrap_or(&self.id)
            .to_ascii_lowercase()
    }
}
