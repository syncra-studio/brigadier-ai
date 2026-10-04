//! Choosing the provider, model and reasoning effort for each piece of work (docs/PLAN.md §6
//! Phase 5). The orchestrator's own vendor is never an input (§2 principle 3): a query carries
//! the task, what each provider can offer now, the user's rules and what outcomes taught.
//!
//! # Registry
//!
//! `registry/models.json` ([`Registry`]) records per model its tier, strengths per task category
//! (0–10: how well suited it is, weighing quality against cost), area modifiers (±2), efforts,
//! context window and modalities. The app ships a copy ([`Registry::bundled`]) and the daemon
//! downloads newer revisions ([`Registry::parse`], [`Registry::is_newer_than`]). Every copy
//! passes the same bounds (see [`load`]'s docs): no Fable, efforts only up to `high`, only CLIs
//! Brigadier drives, only capabilities their adapters implement.
//!
//! # Merge
//!
//! [`merge()`] joins each CLI's live list with the registry: a model is **curated** (its own
//! entry), **inherited** (a newer member of a family entry, `gpt-6.1-sol` from `sol`),
//! **researched** (placed by a research note, within ±2 of unrated) or **unknown** (tier
//! unrated, strength 5). Fable models are listed but excluded.
//!
//! # Eligibility
//!
//! A model may take a task when all of these hold ([`decide`]):
//! - it isn't Fable, and its provider is logged in;
//! - **no confirmed limit**: a provider limit (usage window, spend control, credits), a window
//!   that limits it at 97% used, or its own scoped bucket used up. A limit is a hard exclusion,
//!   never a large penalty;
//! - it isn't excluded by the query (it or its provider just failed on the task);
//! - no applicable `never` rule names it, and every applicable `only` rule allows it;
//! - it meets the task's needs (image input, image generation, context window);
//! - its tier is at or above the task's quality floor ([`default_floor`]);
//! - for reviews, its vendor differs from the author's when another vendor can review; else a
//!   different model of the same vendor reviews, and the reason says so.
//!
//! [`eligible`] lists every model these checks let take the task: the models the task's worker
//! may hand work to through its CLI's own sub-agents.
//!
//! # Score
//!
//! `strength(category) + mean area modifier + learned adjustment − quota penalty − load penalty
//! + trial bonus`, less small penalties for a legacy model (−1) and an inherited one not yet
//! proven here (−0.25). The learned adjustment ([`learn()`]) is a Beta-shrunk success rate
//! against the registry prior, capped at ±2.5. The quota penalty ([`forecast`]) is 0 below a
//! projected 70% and rises smoothly to 6 at a projected 100%; each running worker on the
//! provider costs 0.25 (at most 1.5). A `prefer` rule wins whenever its target is eligible, over
//! a pin too. Effort is the registry's default for the category (else low for scouting and
//! checks, medium for research, implementation and orchestration, high for reviews and merges),
//! fitted to what the model accepts and never above `high`.
//!
//! # Trials
//!
//! An unknown or researched model with fewer than 3 outcomes may run scouting, research and
//! verification — never implementation, reviews, merges or orchestration — when the query holds
//! a trial slot (the caller gives at most 1 in 5 low-risk tasks one) and it meets the task's
//! needs. Until then this is the only way it runs, whatever its tier; the trial path stands in
//! for the floor check, with a +4 bonus. After its trial, a researched model is scored like any
//! other; an unknown one waits for research to place it (it stays unrated, below every floor).
//!
//! # Rules
//!
//! `never`, `prefer` and `only` rules ([`OverrideRule`]) apply by project, category and area.
//! `never` and `only` filter; `prefer` beats scores, balancing and pins where routing scores.
//! They hold during fallback: when every model an `only` rule allows is unavailable, the task
//! waits, naming the rule and the limit.
//!
//! # Manual rankings
//!
//! Per kind of work the user may rank models by hand ([`Ranking`]), everywhere or in one
//! project, with area overrides; the most specific applies ([`ranking_for`]). A Manual ranking
//! is tried top-down instead of scoring: a model place takes that model, a family place its
//! newest model that can run (the CLI lists newest first), a vendor place its best-scored one.
//! In order of strength:
//! 1. The hard rules (no Fable, efforts at most `high`, a login, no confirmed limit, not the
//!    model that just failed, the task's needs, a binding pin's vendor, cross-vendor review).
//! 2. `never` and `only` rules.
//! 3. The floor: a ranked model skips the category's default floor (the Routing page warns);
//!    a floor the orchestrator raised for a task still holds. A ranked model not rated yet
//!    runs without a trial.
//! 4. The ranking: its first place that can run. It beats `prefer` rules, pins that don't
//!    bind (a pin naming a ranked model picks it), balancing, load, learned scores and trials.
//!    Its effort comes first, then the pin's, then the category's.
//! 5. When no place can run: with Only these the task waits for them (the earliest reset among
//!    them); otherwise routing scores the other models, and the reason says the ranking had
//!    none free. [`Explanation::ranking`] keeps every place passed over and why.
//!
//! # Quota heat and balancing
//!
//! Each window's rate of use gives a forecast at its reset; a provider is Warm at a projected
//! 70%, Hot at 90% (or 85% used), Limited at a limit or 97% used ([`forecast`]). The penalty
//! shifts new work to the other provider as a window heats up; when that changes the vendor,
//! the explanation says `balancing` and the reason names the window.
//!
//! # Waiting
//!
//! With nothing eligible, [`decide`] returns [`Decision::Wait`] with the earliest reset among
//! the models kept out only by a limit (none when no reset would help). Fallback is the same
//! call with the failed provider or model excluded and the same floor, areas, needs and rules.
//! There the pin binds ([`Query::hold_pin`]): a task the orchestrator pinned to a vendor (or to
//! a model, and so its vendor) stays with that vendor and waits rather than leave it.
//!
//! # Tie order
//!
//! Scores that tie follow the Phase 3 table's vendor order, then each CLI's own order (newest
//! first). Write work starts on Claude and read-only work on Codex, so in a typical session the
//! two draw on different quotas and every write lands on a vendor whose change the other
//! reviews:
//!
//! | Category       | Ties go to    |
//! |----------------|---------------|
//! | Scout          | Codex, Claude |
//! | Research       | Codex, Claude |
//! | Implement      | Claude, Codex |
//! | Review         | Codex, Claude |
//! | Merge          | Claude, Codex |
//! | Verify         | Codex, Claude |
//! | Chat           | Claude, Codex |
//! | Orchestrate    | Claude, Codex |

mod areas;
pub mod decide;
mod explain;
pub mod forecast;
pub mod learn;
pub mod load;
pub mod merge;
mod outcome;
mod overrides;
mod quota;
mod rankings;
mod ratings;
mod registry;
pub use ratings::{RatingPatch, validate_patches};
mod table;

use brigadier_providers::ProviderKind;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use areas::infer_areas;
pub use decide::{
    Decision, Exclusion, Needs, Preview, ProviderState, Query, Routed, Waiting, allows_trials,
    available, decide, default_floor, eligible, preview, rule_text, targets,
};
pub use explain::{Alternative, Explanation, Factor, RouteCandidate};
pub use forecast::{
    active_limit, heat, provider_quota, quota_penalty, window_applies, window_state,
};
pub use learn::{is_model, learn, learned_for};
pub use load::{MAX_REGISTRY_BYTES, Parsed, RegistryError};
pub use merge::{OutcomeCount, TRIAL_OUTCOMES, merge, outcome_counts};
pub use outcome::{Learned, Outcome, OutcomeResult};
pub use overrides::{OverrideEffect, OverrideRule, OverrideTarget};
pub use quota::{Forecast, Heat, ProviderQuota, QuotaSample, WindowState};
pub use rankings::{
    RankedEntry, RankedPlace, Ranking, RankingUse, ranking_for, ranking_text, target_text,
};
pub use registry::{
    Capability, MergedModel, Modalities, Modality, ModelMatch, ModelStatus, QualityTier,
    RatingProvenance, Registry, RegistryInfo, RegistryModel, RegistrySource, ResearchNote,
    TrialState,
};

/// What a piece of work is, for routing. Mirrors the core's task kinds plus plain Chats and
/// the orchestrator itself.
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
    /// A plain conversation with the model (no orchestrator, no workers).
    Chat,
    /// A session's orchestrator: plans, delegates and reviews, never does the work itself.
    Orchestrate,
}

impl TaskCategory {
    pub const ALL: [TaskCategory; 8] = [
        TaskCategory::Scout,
        TaskCategory::Research,
        TaskCategory::Implement,
        TaskCategory::Review,
        TaskCategory::Merge,
        TaskCategory::Verify,
        TaskCategory::Chat,
        TaskCategory::Orchestrate,
    ];
}

/// The part of a codebase a task touches, for user rules ("never use X for frontend") and
/// per-area strengths. Named by the orchestrator (`delegate_task`), else derived from the
/// paths its spec names.
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

/// A provider, model or effort asked for by the orchestrator (`delegate_task`) or the user.
/// Every part is optional; what is left out is routed. Kept on the task, so a hand-off or a
/// resume keeps it ([`Query::hold_pin`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Pin {
    pub provider: Option<ProviderKind>,
    /// A model id from the provider's list, an alias (`opus`) or a family name (`sol`). A model
    /// alone also picks its provider.
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// The model that wrote the change a review is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Author {
    pub provider: ProviderKind,
    /// Its model id; `None` for the CLI's default model.
    pub model: Option<String>,
}

/// A provider's name as the user sees it.
fn name(provider: ProviderKind) -> &'static str {
    provider.label()
}
