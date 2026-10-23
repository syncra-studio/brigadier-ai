//! How a manual ranking decided a choice, as task explanations recorded before S16b keep it.
//! Brigadier no longer keeps rankings or routes work by them.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::overrides::OverrideTarget;

/// How a manual ranking decided (or failed to decide) a choice, kept with the routing
/// explanation so a worker card shows it as it was.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RankingUse {
    /// The ranking as the user would name it ("your implementation ranking for frontend").
    pub text: String,
    /// The place that ran the task (1-based); absent when no ranked model could take it.
    pub position: Option<u32>,
    /// Places in the list.
    pub places: u32,
    /// Only these: the task waits rather than go to other models.
    pub only: bool,
    /// The places above the one that ran (all of them when none did), each with why it was
    /// passed over.
    pub skipped: Vec<RankedPlace>,
}

/// One place of a ranking as routing found it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RankedPlace {
    /// 1-based.
    pub position: u32,
    pub target: OverrideTarget,
    /// The model it comes to, when one can run (a family's newest, a vendor's best).
    pub model: Option<String>,
    /// It took the task.
    pub chosen: bool,
    /// Why it can't take the task, when it can't.
    pub why: Option<String>,
    /// When the limit that keeps it out resets, if a limit does.
    pub resets_at_ms: Option<i64>,
}
