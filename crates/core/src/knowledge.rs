//! What the core records about the Project Brain: the overview the Inspector reads.

use brigadier_brain::{BrainStats, EmbedderStatus};
use brigadier_index::IndexStatus;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::ProjectId;

/// A project's Brain at a glance, for the Inspector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrainOverview {
    pub project_id: ProjectId,
    pub stats: BrainStats,
    /// The project's code index (none for a project without a repo).
    pub index: Option<IndexStatus>,
    pub embedder: EmbedderStatus,
}

/// What an AGENTS.md export wrote.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConventionsExport {
    pub path: String,
    /// Conventions written.
    pub conventions: u32,
    /// The file did not exist before.
    pub created: bool,
}
