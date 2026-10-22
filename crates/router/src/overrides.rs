//! The legacy task router's user preferences, retained until its last consumer leaves.
//! Explicit worker starts use only the user's model rules file.

use brigadier_providers::ProviderKind;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{Area, TaskCategory};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum OverrideEffect {
    /// The target is never used where the rule applies.
    Never,
    /// The target is chosen where the rule applies whenever it is eligible.
    Prefer,
    /// Only the target is used where the rule applies. When it is unavailable the task waits
    /// for it; it never falls through to other models.
    Only,
}

/// What a rule is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum OverrideTarget {
    /// Every model of a CLI.
    Vendor { provider: ProviderKind },
    /// Every model of a family (`opus`, `sol`), whatever its version.
    Family {
        provider: ProviderKind,
        family: String,
    },
    /// One model, by the id its CLI lists.
    Model { provider: ProviderKind, id: String },
}

/// One routing rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OverrideRule {
    pub id: String,
    pub effect: OverrideEffect,
    pub target: OverrideTarget,
    /// Where it applies: these categories (empty: all of them)…
    #[serde(default)]
    pub categories: Vec<TaskCategory>,
    /// …and tasks touching these areas (empty: any task).
    #[serde(default)]
    pub areas: Vec<Area>,
    /// Only in this project; absent: everywhere.
    #[serde(default)]
    pub project_id: Option<String>,
    pub created_at_ms: i64,
}
