//! A cheap scout or research task run again once, one tier up (WORKSPACES-PLAN.md phase 6,
//! token doc item 4). Routing starts scouting and research on cheaper models; when such a
//! task's answer falls short, Brigadier reruns it once on a model a tier above the one that
//! ran it and tells the thread plainly: the cheaper model, the new one and why.
//!
//! - **What reruns it.** A "done when" line its report marks [not met], or the thread's
//!   `rerun_task` when it judges that the answer lacks support (a checker's word reaches the
//!   thread, which decides). Never a missing or broken citation (a line marked [unevidenced]):
//!   the same worker fixes that when the thread sends it back. Never the evidence reader's
//!   verdict, nor unverified memory lines.
//! - **Limits.** Scouting and research only ([`brigadier_router::reruns`]); never downward
//!   ([`brigadier_router::rerun_floor`]); never twice: a task reruns once, and a rerun never
//!   reruns. A task the thread put on a model by name reruns only when the thread asks.
//! - **Plumbing, not a guard.** The first report reaches the thread as it is. The rerun is a
//!   new task with the same spec, which the thread may use, ignore or stop, and the thread may
//!   still ask for more itself. Its routing explanation and its outcome name the cheaper model
//!   and why ([`brigadier_router::Rerun`]).

use brigadier_router::{QualityTier, Rerun};

use super::SessionManager;
use super::workers::{TaskExtra, category, route_label};
use crate::model::ConversationId;
use crate::work::{OrchestratorStepKind, Task};
use crate::{Error, Result};

/// The most bytes of a failed check or the thread's reason a rerun's note quotes.
const REASON_MAX: usize = 200;
/// The most bytes of the thread's `why` kept.
const WHY_MAX: usize = 160;

/// The first "done when" line a report marks [not met]: its acceptance check failed. A line
/// marked [met] (with or without its evidence) or [not checked] is none.
pub(crate) fn failed_check(done_when: &[String]) -> Option<&str> {
    done_when.iter().map(|line| line.trim()).find(|line| {
        super::evidence::met(line).is_none() && line.to_ascii_lowercase().contains("[not met]")
    })
}

/// What the thread is told of `rerun`, the rerun of `task`.
pub(crate) fn rerun_note(task: &Task, rerun: &Task) -> String {
    let Some(of) = &rerun.rerun else {
        return String::new();
    };
    let runs = match &rerun.quota_wait {
        Some(wait) => format!(
            "; no model it may use is free now ({}), so it starts when one is",
            wait.reason
        ),
        None => format!(" on {}", route_label(rerun)),
    };
    format!(
        "[rerun task-{n}] task-{n} ran on {} {} ({}) and {}, so Brigadier reruns it once, one tier up, as task-{m}{runs}. Its report arrives later as task-{m}'s: use it, ignore it or stop it, as you judge.",
        of.provider.label(),
        of.model,
        brigadier_router::tier_name(of.tier),
        of.reason,
        n = task.number,
        m = rerun.number,
    )
}

impl SessionManager {
    /// `rerun_task`: the thread judged a task's answer unsupported. What it is told: the
    /// rerun's note, or why there is none.
    pub(crate) async fn rerun_by_thread(
        &self,
        id: &ConversationId,
        args: crate::tools::RerunTask,
    ) -> Result<String> {
        let why = super::brains::one_line(&args.why, WHY_MAX);
        if why.is_empty() {
            return Err(Error::Invalid(
                "rerun_task needs `why`: one line on what its answer lacks".into(),
            ));
        }
        let task = self.find_task(id, &args.task).await?;
        // A rerun hears first that it doesn't rerun.
        if task.report.is_none() && task.rerun.is_none() {
            return Ok(format!(
                "task-{} hasn't reported yet, so there is no answer to rerun: its report arrives as a message.",
                task.number
            ));
        }
        let reason = format!("you judged that its answer lacks support: {why}");
        Ok(match self.rerun(&task, &reason).await {
            Ok(rerun) => rerun_note(&task, &rerun),
            Err(why_not) => why_not,
        })
    }

    /// Reruns `task` once, one tier up, because `reason` (its failed check, or the thread's
    /// words): a new task with its spec, kind, areas and needs, on a model a tier above the
    /// one that ran it. The thread pinned no model on it then (its vendor and effort stay). The
    /// rerun, or what the thread is told instead: why it doesn't rerun.
    pub(crate) async fn rerun(
        &self,
        task: &Task,
        reason: &str,
    ) -> std::result::Result<Task, String> {
        let kind = category(task.kind);
        if !brigadier_router::reruns(kind) {
            return Err(format!(
                "task-{} is {kind} work: only scout and research tasks rerun one tier up. Delegate a new task if you want more.",
                task.number
            ));
        }
        if task.rerun.is_some() {
            return Err(format!(
                "task-{} is already a rerun one tier up; it doesn't rerun again. Delegate a new task if you want more.",
                task.number
            ));
        }
        let board = self
            .core
            .board(&task.conversation_id)
            .await
            .map_err(|err| err.to_string())?;
        let id = task.id.to_string();
        if let Some(done) = board.tasks.values().find(|other| {
            other
                .rerun
                .as_ref()
                .is_some_and(|rerun| rerun.task_id == id)
        }) {
            return Err(format!(
                "task-{} already reran once, as task-{}; it doesn't rerun again. Delegate a new task if you want more.",
                task.number, done.number
            ));
        }
        let choice = &task.route.choice;
        let ran_on = self.ran_tier(task).await.unwrap_or(task.floor);
        let Some(floor) = brigadier_router::rerun_floor(kind, ran_on, task.floor) else {
            return Err(format!(
                "task-{} ran on {}, a model of the top tier: there is none above it to rerun on.",
                task.number,
                route_label(task)
            ));
        };
        let rerun = Rerun {
            task_id: id,
            provider: choice.provider,
            model: choice.model.clone().unwrap_or_else(|| "default".into()),
            tier: ran_on,
            reason: super::brains::one_line(reason, REASON_MAX),
        };
        // The thread's vendor and effort hold; a model it named would be the one that ran.
        let pin = task
            .pin
            .clone()
            .map(|pin| brigadier_router::Pin { model: None, ..pin });
        let created = crate::heap(|| {
            self.create_task_as(
                &task.conversation_id,
                task.title.clone(),
                task.kind,
                task.spec.clone(),
                pin,
                None,
                Vec::new(),
                None,
                None,
                task.attachments.clone(),
                Some(task.areas.clone()),
                Some(floor),
                task.needs.clone(),
                TaskExtra {
                    request: task.request_id.clone(),
                    role: task.role,
                    phase: task.phase,
                    rerun: Some(rerun),
                    ..TaskExtra::default()
                },
            )
        })
        .await
        .map_err(|err| format!("task-{} could not rerun one tier up: {err}", task.number))?;
        tracing::info!(task = %task.id, rerun = %created.id, ?floor, "a cheap task reruns one tier up");
        self.orchestrator_step(
            &task.conversation_id,
            OrchestratorStepKind::Created {
                task_id: created.id.clone(),
            },
        )
        .await;
        Ok(created)
    }

    /// Whether `task`'s report reruns it by itself: a scout or research task that is no rerun
    /// and that the thread didn't put on a model by name.
    pub(crate) fn reruns_by_itself(task: &Task) -> bool {
        brigadier_router::reruns(category(task.kind))
            && task.rerun.is_none()
            && task.pin.as_ref().is_none_or(|pin| pin.model.is_none())
    }

    /// The tier the model that ran `task` counts as for its work, when routing knows it.
    async fn ran_tier(&self, task: &Task) -> Option<QualityTier> {
        let choice = &task.route.choice;
        let id = choice.model.as_deref()?;
        let inputs = self.routing_inputs(None, crate::now_ms()).await;
        inputs
            .models
            .iter()
            .find(|model| {
                model.provider == choice.provider
                    && brigadier_router::is_model(model, &inputs.registry, id)
            })?
            .tier_for(category(task.kind))
    }
}

#[cfg(test)]
mod tests {
    use super::failed_check;

    fn lines(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| (*line).to_owned()).collect()
    }

    /// Only a [not met] line is a failed check: a [met] line without its evidence, or with a
    /// broken one (marked [unevidenced]), and a [not checked] line are not.
    #[test]
    fn only_a_not_met_line_is_a_failed_check() {
        assert_eq!(
            failed_check(&lines(&[
                "[met] the parser is found: src/parse.rs:3-4 `fn parse`",
                "- [Not met] the caller is named: none found",
            ])),
            Some("- [Not met] the caller is named: none found")
        );
        assert_eq!(
            failed_check(&lines(&["the caller is named: [not met], none found"])),
            Some("the caller is named: [not met], none found")
        );
        assert_eq!(
            failed_check(&lines(&[
                "[met] the parser is found: it is there [unevidenced]",
                "[not checked] the tests pass",
                "[met] nothing is [not met] here: out-1a2b3c4d",
            ])),
            None
        );
        assert_eq!(failed_check(&[]), None);
    }
}
