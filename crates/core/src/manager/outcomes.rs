//! Outcome recording (PLAN.md §6 Phase 5): how each model did on each task, per project, kept
//! in `routing.sqlite` for the router's outcome learning (`brigadier_router::learn`, which does
//! the math and documents how much each measure moves a model's score).
//!
//! One outcome per model that ran a task, recorded when its part ends: at a hand-off for the
//! model that stopped, and when the task ends for the last one. What each measure is, and
//! which way it moves the learned adjustment:
//! - **result**: landed or done lifts the model's success rate, rejected or failed lowers it;
//!   stopped says nothing about the model, and a hand-off only about its quota.
//! - **review pass rate**: whether the first review of its change approved it; a first review
//!   asking for changes lowers the sample.
//! - **rework rounds**: how often the task was sent back after reporting (a review asking for
//!   changes, or the orchestrator's `message_worker`); each round lowers the sample.
//! - **verification**: the checks a verify task ran on its work (its `subject`); failed checks
//!   lower the sample. A verify task's own result is about the code, not about its model.
//! - **time** and **quota used**: the attempt's duration, its tokens, and its estimated share
//!   of the provider's shortest window; cheaper than the category's median on the project
//!   lifts the model a little, dearer lowers it.
//!
//! Reviews and verifications can come after the task ended; the subject's outcome is recorded
//! again when they do.

use std::collections::HashMap;
use std::sync::Arc;

use brigadier_router::{Outcome, OutcomeResult};

use super::SessionManager;
use crate::now_ms;
use crate::routing::RoutingStore;
use crate::work::{Attempt, ChecksResult, ReviewVerdict, Task, TaskKind, TaskState};

/// Samples this long before an attempt started can be its quota baseline (they are taken on
/// every change, and at least every half hour).
const BASELINE_LOOKBACK_MS: i64 = 35 * 60 * 1000;

impl SessionManager {
    /// Records the outcome of `task`'s last model when the task ends as `state`.
    pub(crate) async fn record_task_outcome(&self, task: &Task, state: TaskState) {
        let result = match state {
            TaskState::Landed => OutcomeResult::Landed,
            TaskState::Done => OutcomeResult::Done,
            TaskState::Rejected => OutcomeResult::Rejected,
            TaskState::Failed => OutcomeResult::Failed,
            TaskState::Stopped => OutcomeResult::Stopped,
            _ => return,
        };
        // A task that waited from the start and never began taught nothing.
        if task.attempts.is_empty() && task.workspace.is_none() {
            return;
        }
        // The task's end closed its last attempt (see `dispose_task`), so a review or check
        // recorded later keeps its time.
        let attempt = task.attempts.last().cloned().unwrap_or_else(|| Attempt {
            route: task.route.clone(),
            started_at_ms: task.created_at_ms,
            ended_at_ms: Some(task.updated_at_ms),
            end: None,
        });
        self.record_outcome(task, &attempt, result).await;
        // A review or a verification says something about the task it checked.
        if matches!(task.kind, TaskKind::Review | TaskKind::Verify)
            && let Some(subject) = &task.subject
            && let Ok(subject) = self.task_by_id(&task.conversation_id, subject).await
            && subject.state.is_final()
        {
            Box::pin(self.record_task_outcome(&subject, subject.state)).await;
        }
    }

    /// Records one model's part of `task`.
    pub(crate) async fn record_outcome(
        &self,
        task: &Task,
        attempt: &Attempt,
        result: OutcomeResult,
    ) {
        let Some(store) = self.runtime.routing_store().cloned() else {
            return;
        };
        let Some(project_id) = self
            .core
            .conversation(&task.conversation_id)
            .ok()
            .and_then(|conversation| conversation.project_id)
        else {
            return;
        };
        let started = attempt.started_at_ms;
        let ended = attempt.ended_at_ms.unwrap_or_else(now_ms);
        let provider = attempt.route.choice.provider;
        let (tokens, model) = attempt_tokens(&store, task, provider, started, ended).await;
        let model = model
            .or_else(|| attempt.route.choice.model.clone())
            .unwrap_or_else(|| "default".into());
        let quota_percent = self
            .quota_share(&store, task, provider, started, ended)
            .await;
        let checks = self.checked(task).await;
        let outcome = Outcome {
            project_id: project_id.to_string(),
            task_id: task.id.to_string(),
            provider,
            model,
            category: super::workers::category(task.kind),
            areas: task.areas.clone(),
            result,
            review_passed_first: checks.review_passed_first,
            reviews: checks.reviews,
            rework_rounds: task.rework_rounds,
            verification: checks.verification,
            duration_ms: (ended - started).max(0),
            tokens,
            quota_percent,
            at_ms: ended,
        };
        if let Err(err) = store.put_outcome(outcome).await {
            tracing::debug!(task = %task.id, error = %err, "could not record the outcome");
        }
    }

    /// The reviews and verify tasks that checked `task`'s work.
    async fn checked(&self, task: &Task) -> Checked {
        let mut checked = Checked::default();
        if !task.kind.writes() {
            return checked;
        }
        let Ok(tasks) = self.core.tasks(&task.conversation_id).await else {
            return checked;
        };
        let mut reviews: Vec<(i64, ReviewVerdict)> = Vec::new();
        let mut verified: Option<(i64, ChecksResult)> = None;
        for other in tasks
            .iter()
            .filter(|other| other.subject.as_ref() == Some(&task.id))
        {
            let Some(report) = &other.report else {
                continue;
            };
            match other.kind {
                TaskKind::Review => {
                    if let Some(verdict) = report.verdict {
                        reviews.push((report.submitted_at_ms, verdict));
                    }
                }
                TaskKind::Verify => {
                    if let Some(result) = report.checks
                        && verified.is_none_or(|(at, _)| at < report.submitted_at_ms)
                    {
                        verified = Some((report.submitted_at_ms, result));
                    }
                }
                _ => {}
            }
        }
        reviews.sort_by_key(|(at, _)| *at);
        checked.reviews = u32::try_from(reviews.len()).unwrap_or(u32::MAX);
        checked.review_passed_first = reviews
            .first()
            .map(|(_, verdict)| *verdict == ReviewVerdict::Approve);
        checked.verification = verified.and_then(|(_, result)| match result {
            ChecksResult::Passed => Some(true),
            ChecksResult::Failed => Some(false),
            ChecksResult::NotRun | ChecksResult::NoChecks => None,
        });
        checked
    }

    /// The attempt's estimated share of its provider's shortest window, in percentage points:
    /// how far that window moved while it ran, times the attempt's share of every token
    /// Brigadier spent on that provider meanwhile. Unknown when the window reset in between or
    /// was not sampled.
    async fn quota_share(
        &self,
        store: &Arc<RoutingStore>,
        task: &Task,
        provider: brigadier_providers::ProviderKind,
        started: i64,
        ended: i64,
    ) -> Option<f64> {
        let turns = store.turns_since(provider, started).await.ok()?;
        // The account the attempt ran on (most of its use): only its quota, and only the use
        // charged to it, say what the attempt cost.
        let mut by_account: HashMap<Option<&str>, i64> = HashMap::new();
        for turn in turns.iter().filter(|turn| {
            turn.at_ms <= ended && turn.task_id.as_deref() == Some(task.id.0.as_str())
        }) {
            *by_account.entry(turn.account.as_deref()).or_default() +=
                turn.input + turn.cached_input + turn.output;
        }
        let account = by_account
            .into_iter()
            .max_by_key(|(_, tokens)| *tokens)
            .and_then(|(account, _)| account.map(str::to_owned));
        let account = crate::accounts::AccountRef::new(provider, account);
        let quota = self.runtime.monitor().current_for(&account, ended)?;
        let window = quota
            .windows
            .iter()
            // Provider-wide windows (Codex's main bucket has a name; a model's own has a model).
            .filter(|window| window.model.is_none())
            .min_by_key(|window| window.window_minutes.unwrap_or(i64::MAX))?;
        let samples =
            self.runtime
                .monitor()
                .history(&account, &window.id, started - BASELINE_LOOKBACK_MS);
        let before = samples
            .iter()
            .rev()
            .find(|sample| sample.at_ms <= started)?;
        let after = samples.iter().rev().find(|sample| sample.at_ms <= ended)?;
        let moved = after.used_percent - before.used_percent;
        if moved < 0.0 {
            return None;
        }
        let (mut own, mut all) = (0_i64, 0_i64);
        for turn in turns
            .iter()
            .filter(|turn| turn.at_ms <= ended && turn.account == account.account)
        {
            let tokens = turn.input + turn.cached_input + turn.output;
            all += tokens;
            if turn.task_id.as_deref() == Some(task.id.0.as_str()) {
                own += tokens;
            }
        }
        if all == 0 {
            return None;
        }
        #[allow(clippy::cast_precision_loss)]
        Some(moved * own as f64 / all as f64)
    }
}

/// What checked a task's work.
#[derive(Default)]
struct Checked {
    reviews: u32,
    review_passed_first: Option<bool>,
    verification: Option<bool>,
}

/// The tokens `task`'s turns on `provider` used between `started` and `ended`, and the model
/// most of them ran on (what the CLI reported, so an alias is resolved).
async fn attempt_tokens(
    store: &Arc<RoutingStore>,
    task: &Task,
    provider: brigadier_providers::ProviderKind,
    started: i64,
    ended: i64,
) -> (i64, Option<String>) {
    let Ok(turns) = store.turns_since(provider, started).await else {
        return (0, None);
    };
    let mut tokens = 0;
    let mut models: HashMap<String, i64> = HashMap::new();
    for turn in turns
        .iter()
        .filter(|turn| turn.at_ms <= ended && turn.task_id.as_deref() == Some(task.id.0.as_str()))
    {
        let used = turn.input + turn.cached_input + turn.output;
        tokens += used;
        *models.entry(turn.model.clone()).or_default() += used;
    }
    let model = models
        .into_iter()
        .filter(|(model, _)| !model.is_empty())
        .max_by_key(|(_, used)| *used)
        .map(|(model, _)| model);
    (tokens, model)
}
