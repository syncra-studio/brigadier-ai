//! The model registry and quota estimates Brigadier shares across its crates (STRIP-DOWN-PLAN
//! S16b): what each CLI's models are and how hot each provider runs. Brigadier routes no work
//! by them: a session picks its workers' models itself, through `brigadier models` (G18).
//!
//! # Registry
//!
//! `registry/models.json` ([`Registry`]) records per model its tier, strengths per task category
//! (0–10: how well suited it is, weighing quality against cost), area modifiers (±2), efforts,
//! context window and modalities. The app ships a copy ([`Registry::bundled`]) and the daemon
//! downloads newer revisions ([`Registry::parse`], [`Registry::is_newer_than`]). Every copy
//! passes the same bounds (see [`load`]'s docs): recognized effort names,
//! only CLIs Brigadier drives, only capabilities their adapters implement.
//!
//! # Merge
//!
//! [`merge()`] joins each CLI's live list with the registry: a model is **curated** (its own
//! entry), **inherited** (a newer member of a family entry, `gpt-6.1-sol` from `sol`),
//! **researched** (placed by a research note, within ±2 of unrated) or **unknown** (tier
//! unrated, strength 5). Model family names impose no runtime restriction.
//!
//! # Quota heat
//!
//! Each window's rate of use gives a forecast at its reset; a provider is Warm at a projected
//! 70%, Hot at 90% (or 85% used), Limited at a limit or 97% used ([`forecast`]). A model is
//! [`available`] while its provider is logged in and no window that limits it is used up.

mod explain;
pub mod forecast;
pub mod load;
pub mod merge;
mod outcome;
mod overrides;
mod quota;
mod rankings;
mod ratings;
mod registry;
pub use ratings::{RatingPatch, validate_patches};

/// Family metadata for the settings discovery view; never a runtime refusal.
pub fn names_expensive(name: &str) -> bool {
    table::names_expensive(name)
}
mod table;

use brigadier_providers::ProviderKind;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use explain::{Alternative, Explanation, Factor, Rerun};
pub use forecast::{active_limit, available, heat, provider_quota, window_applies, window_state};
pub use load::{MAX_REGISTRY_BYTES, Parsed, RegistryError};
pub use merge::{OutcomeCount, TRIAL_OUTCOMES, is_model, merge};
pub use outcome::{Outcome, OutcomeResult};
pub use overrides::{OverrideEffect, OverrideRule, OverrideTarget};
pub use quota::{Forecast, Heat, ProviderQuota, ProviderState, QuotaSample, WindowState};
pub use rankings::{RankedPlace, RankingUse};
pub use registry::{
    Capability, MergedModel, Modalities, Modality, ModelMatch, ModelStatus, QualityTier,
    RatingProvenance, Registry, RegistryInfo, RegistryModel, RegistrySource, ResearchNote,
    TrialState,
};

/// What a piece of work is, for the registry's strengths and the user's rules. Mirrors the
/// core's task kinds plus plain Chats and the orchestrator itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TaskCategory {
    /// Looks around the repository and answers a question.
    Scout,
    /// Reads docs and the web.
    Research,
    /// Changes code; lands as one commit.
    Implement,
    /// Reviews another task's candidate commit or a plan.
    Review,
    /// Resolves conflicts between a task and its target branch.
    Merge,
    /// Runs the project's checks.
    Verify,
    /// Operates apps on the user's Mac; writes nothing that lands.
    Operate,
    /// Makes designs and images: design files (Pencil's `.pen`), mockups, generated images.
    Design,
    /// A plain conversation with the model (no orchestrator, no workers).
    Chat,
    /// A session's orchestrator: plans, delegates and reviews, never does the work itself.
    Orchestrate,
    /// Short git text Brigadier writes itself (a commit message): a fast, cheap model's job.
    GitText,
}

impl TaskCategory {
    pub const ALL: [TaskCategory; 11] = [
        TaskCategory::Scout,
        TaskCategory::Research,
        TaskCategory::Implement,
        TaskCategory::Review,
        TaskCategory::Merge,
        TaskCategory::Verify,
        TaskCategory::Operate,
        TaskCategory::Design,
        TaskCategory::Chat,
        TaskCategory::Orchestrate,
        TaskCategory::GitText,
    ];
}

/// The part of a codebase a task touches, for user rules ("never use X for frontend") and
/// per-area strengths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Area {
    /// UI code, styles and markup.
    Frontend,
    /// Services, libraries, application logic.
    Backend,
    /// Build, CI, containers, deployment.
    Infra,
    /// Documentation.
    Docs,
    /// Tests.
    Tests,
}

impl Area {
    pub const ALL: [Area; 5] = [
        Area::Frontend,
        Area::Backend,
        Area::Infra,
        Area::Docs,
        Area::Tests,
    ];
}

impl std::fmt::Display for TaskCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(table::row(*self).purpose)
    }
}

/// A provider, model or effort asked for by the orchestrator or the user, as tasks recorded
/// before S16b keep it. Every part is optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Pin {
    pub provider: Option<ProviderKind>,
    /// A model id from the provider's list, an alias (`opus`) or a family name (`sol`). A model
    /// alone also picks its provider.
    pub model: Option<String>,
    pub effort: Option<String>,
    /// The user picked this model by hand. Never kept: a stored pin is the orchestrator's.
    #[serde(skip)]
    #[ts(skip)]
    pub by_user: bool,
}
