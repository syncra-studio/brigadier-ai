//! An overnight run's state as task code needs it (PLAN.md §10.8).
//!
//! A run's workers follow the session's permission level, like any other worker: nothing is
//! refused for being part of a run. They are told never to push, publish or deploy, and list
//! such steps for the user instead.

use std::collections::HashMap;

use crate::model::{ConversationId, OvernightRunId};
use crate::overnight::{ObstacleKind, OvernightRun, RunRole, RunTaskContext, RunWorkspace};
use crate::work::Task;

/// A session's active run, as task code needs it without reading the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveRun {
    pub id: OvernightRunId,
    pub segment: u32,
    pub generation: u32,
    pub rules_hash: String,
    pub workspace: Option<RunWorkspace>,
    /// "max N workers": its tasks executing at once.
    pub max_workers: Option<u32>,
    /// Ending (Stop, the deadline, a block): no new work starts.
    pub winding_down: bool,
    /// When wind-down starts, for a deadline run (wall clock, ms).
    pub wind_down_at_ms: Option<i64>,
}

/// The active run of each session that has one. Kept in step with every recorded run, and
/// rebuilt from the boards at startup.
#[derive(Default)]
pub(crate) struct ActiveRuns(std::sync::Mutex<HashMap<ConversationId, ActiveRun>>);

impl ActiveRuns {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ConversationId, ActiveRun>> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn get(&self, id: &ConversationId) -> Option<ActiveRun> {
        self.lock().get(id).cloned()
    }

    /// Makes `run` the session's active run, as a test sets it up.
    #[cfg(test)]
    pub fn insert(&self, id: ConversationId, run: ActiveRun) {
        self.lock().insert(id, run);
    }

    /// Every session's active run.
    pub fn all(&self) -> Vec<(ConversationId, ActiveRun)> {
        self.lock()
            .iter()
            .map(|(id, run)| (id.clone(), run.clone()))
            .collect()
    }

    /// Takes in a run as just recorded: an active one is the session's run, one that ended
    /// no longer is.
    pub fn note(&self, run: &OvernightRun) {
        let mut active = self.lock();
        if run.state.is_active() {
            active.insert(
                run.conversation_id.clone(),
                ActiveRun {
                    id: run.id.clone(),
                    segment: run.segment,
                    generation: run.generation,
                    rules_hash: rules_hash(&run.rules),
                    workspace: run.workspace.clone(),
                    max_workers: run.directives.max_workers,
                    winding_down: matches!(
                        run.state,
                        crate::overnight::OvernightState::WindingDown
                            | crate::overnight::OvernightState::Reporting
                    ),
                    wind_down_at_ms: run.wind_down_at_ms,
                },
            );
        } else if active
            .get(&run.conversation_id)
            .is_some_and(|now| now.id == run.id)
        {
            active.remove(&run.conversation_id);
        }
    }
}

impl ActiveRun {
    /// The context a task made now gets: a check of another task's change inherits that
    /// task's run (or none: checks of work from before the run stay the session's), anything
    /// else works for the run's current phase.
    pub fn context_for(active: Option<&Self>, subject: Option<&Task>) -> Option<RunTaskContext> {
        match subject {
            Some(subject) => subject.run.clone().map(|run| RunTaskContext {
                role: RunRole::Check,
                ..run
            }),
            None => active.map(|active| active.context(RunRole::Worker)),
        }
    }

    /// A task of the run's current phase in `role`.
    pub fn context(&self, role: RunRole) -> RunTaskContext {
        RunTaskContext {
            run_id: self.id.clone(),
            segment: self.segment,
            generation: self.generation,
            role,
            rules_hash: self.rules_hash.clone(),
        }
    }
}

/// A short, stable hash of the run's Rules, so a task's briefing can be matched to them.
pub(crate) fn rules_hash(rules: &str) -> String {
    blake3::hash(rules.as_bytes()).to_hex()[..16].to_owned()
}

impl super::super::SessionManager {
    /// Lists an obstacle on an active run, or counts it again.
    pub(crate) async fn note_obstacle(
        &self,
        conversation_id: &ConversationId,
        run_id: &OvernightRunId,
        kind: ObstacleKind,
        text: &str,
        task: Option<u32>,
    ) {
        let _held = self.overnight.changes.lock().await;
        let Ok(board) = self.core.board(conversation_id).await else {
            return;
        };
        let Some(mut run) = board.runs.get(run_id).cloned() else {
            return;
        };
        if !run.state.is_active() {
            return;
        }
        run.note_obstacle(kind, text, task, crate::now_ms());
        if let Err(err) = self.record_run(&run).await {
            tracing::warn!(run = %run_id, error = %err, "could not list what got in the way");
        }
    }
}
