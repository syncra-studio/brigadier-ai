//! Why a model was chosen: the short line on the worker card and the breakdown behind it.

use brigadier_providers::ProviderKind;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::quota::Heat;
use crate::rankings::RankingUse;
use crate::registry::QualityTier;

/// One part of a model's score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Factor {
    /// What it is ("registry strength for implementation", "5 of 6 reviews passed first time
    /// here", "Codex's weekly window projected at 94%").
    pub label: String,
    /// What it added to (or took from) the score.
    pub delta: f64,
}

/// A model that was considered and not chosen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Alternative {
    pub provider: ProviderKind,
    pub model: String,
    /// Its score, when it was eligible.
    pub score: Option<f64>,
    /// Why it lost or was not eligible ("scored lower", "below the task's quality floor",
    /// "your rule: never for frontend", "Claude's 5-hour window is used up until 21:10").
    pub why_not: String,
}

/// The reasoning behind a routing choice, for the worker card's details.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Explanation {
    /// The chosen model's score.
    pub score: Option<f64>,
    pub factors: Vec<Factor>,
    /// The runners-up, best first (a few).
    pub alternatives: Vec<Alternative>,
    /// A user rule decided or constrained the choice (its text).
    pub rule: Option<String>,
    /// The model runs as a trial (an unrated model on a low-risk task).
    pub trial: bool,
    /// Quota balancing moved the work away from a hot provider.
    pub balancing: bool,
    /// The user's manual ranking decided the choice, or had none of its models available.
    #[serde(default)]
    pub ranking: Option<RankingUse>,
    /// It reruns a cheap task one tier up, because that task's answer fell short.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rerun: Option<Box<Rerun>>,
}

/// A scout or research task run again once, one tier up, because its answer fell short: what
/// ran it first and why it reruns. Kept in the rerun's routing explanation and its outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Rerun {
    /// The task it reruns (its id).
    pub task_id: String,
    /// The cheaper model that ran it first.
    pub provider: ProviderKind,
    pub model: String,
    pub tier: QualityTier,
    /// Why ("its check \"[not met] …\" failed", or the thread's words).
    pub reason: String,
}

/// One model routing weighed, for the Routing page's live order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RouteCandidate {
    pub provider: ProviderKind,
    pub model: String,
    pub tier: QualityTier,
    /// Its score, when it may take the task (under a manual ranking, what scoring gives it).
    pub score: Option<f64>,
    pub factors: Vec<Factor>,
    /// The effort it would run at, when it may take the task.
    pub effort: Option<String>,
    /// The hottest usage window that limits it, when its provider's usage is known.
    pub heat: Option<Heat>,
    /// Its place in the manual ranking in force (1-based).
    pub listed: Option<u32>,
    /// Routing would give it the task.
    pub chosen: bool,
    /// It would run as a trial of a new model.
    pub trial: bool,
    /// A new model not rated here yet (it runs only as a trial until it has outcomes).
    pub new: bool,
    /// Why it can't take the task now.
    pub blocked: Option<String>,
    /// When the limit keeping it out resets, if a limit does.
    pub resets_at_ms: Option<i64>,
}
