//! Outcome learning: how each model did on each kind of task in a project, and what that does
//! to its score there.

use brigadier_providers::ProviderKind;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{Area, TaskCategory};

/// How a task ended for the model that ran it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum OutcomeResult {
    /// Its change landed.
    Landed,
    /// A read-only task finished.
    Done,
    /// Turned down by the orchestrator or a review.
    Rejected,
    /// Failed (an error, or its CLI exited before reporting).
    Failed,
    /// Stopped by the user or the orchestrator: says nothing about the model.
    Stopped,
    /// Its usage window ran out mid-task and another model took over: says nothing about its
    /// quality, only about its quota.
    HandedOff,
}

/// One model's run of one task, recorded when the task (or the model's part of it) ends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub project_id: String,
    pub task_id: String,
    pub provider: ProviderKind,
    /// The concrete model (what an alias resolves to).
    pub model: String,
    pub category: TaskCategory,
    pub areas: Vec<Area>,
    pub result: OutcomeResult,
    /// Whether the first review of its change approved it (write tasks that were reviewed).
    pub review_passed_first: Option<bool>,
    /// Reviews it went through.
    pub reviews: u32,
    /// Times it was sent back to fix something (a review asking for changes, or the
    /// orchestrator's `message_worker` after a report).
    pub rework_rounds: u32,
    /// Whether the project's checks passed on its work (a verify task's result).
    pub verification: Option<bool>,
    pub duration_ms: i64,
    /// Tokens its CLI reported for the task (input, cached input and output).
    pub tokens: i64,
    /// Its estimated share of the provider's shortest window, in percentage points.
    pub quota_percent: Option<f64>,
    pub at_ms: i64,
    /// It reran a cheaper model's task one tier up: that model and why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rerun: Option<crate::Rerun>,
}
