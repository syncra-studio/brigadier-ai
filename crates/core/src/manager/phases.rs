//! A request's phases, as the delegator runs them: the orchestrator splits a big request into
//! phases only when they must run one after another, each phase gets one lead, and a lead whose
//! work is big writes an outline first. That outline gets one advisory review from the other
//! vendor; the orchestrator merges what it agrees with into the go-ahead (the user gives the
//! go-ahead under "Ask for approval"). Nothing here rejects, re-reviews or counts rounds.

use std::collections::HashMap;

use tokio::sync::oneshot;

use super::SessionManager;
use super::cards::CardAnswer;
use super::conversation::Envelope;
use super::workers::TaskExtra;
use crate::model::{ConversationId, PermissionLevel};
use crate::tools::{ApproveOutline, PlanPhases};
use crate::work::{
    ApprovalSubject, CardId, InjectionKind, PhaseStage, Plan, PlanApprover, PlanState, PlanStep,
    Task, TaskId, TaskKind, TaskState, WorkerRole,
};
use crate::{Error, Result, now_ms};

/// What a lead waiting for its go-ahead is blocked on (the turn-end check reads it too).
pub(crate) const WAITING_FOR_GO_AHEAD: &str = "Waiting for the go-ahead on its outline";
/// What an implement worker hears in plan mode, before its outline's go-ahead.
pub(crate) const PLAN_MODE_HOLD: &str = "Plan mode is on: change nothing yet. Read the code, send your outline with submit_outline (even for small work) and stop; you build only after the go-ahead.";

/// Reviews a worker waits for (`request_review`) or the orchestrator gets (an outline's), by
/// the reviewer's task: its report, or why it gave none.
#[derive(Default)]
pub(crate) struct Reviews {
    waiting:
        std::sync::Mutex<HashMap<TaskId, oneshot::Sender<std::result::Result<String, String>>>>,
}

impl Reviews {
    fn wait(&self, reviewer: &TaskId) -> oneshot::Receiver<std::result::Result<String, String>> {
        let (tx, rx) = oneshot::channel();
        self.lock().insert(reviewer.clone(), tx);
        rx
    }

    /// Hands a reviewer's outcome to whoever waits for it; false when nobody does (after a
    /// restart its report goes to the orchestrator like any other).
    pub(crate) fn settle(
        &self,
        reviewer: &TaskId,
        outcome: std::result::Result<String, String>,
    ) -> bool {
        match self.lock().remove(reviewer) {
            Some(tx) => tx.send(outcome).is_ok(),
            None => false,
        }
    }

    fn lock(
        &self,
    ) -> std::sync::MutexGuard<
        '_,
        HashMap<TaskId, oneshot::Sender<std::result::Result<String, String>>>,
    > {
        self.waiting.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl SessionManager {
    /// `plan_phases`: records the request's phases. Nothing reviews or approves them: each
    /// phase's lead outlines its own work when it is big.
    pub(crate) async fn plan_phases(
        &self,
        id: &ConversationId,
        args: PlanPhases,
    ) -> Result<String> {
        if args.phases.is_empty() {
            return Err(Error::Invalid("a plan needs at least one phase".into()));
        }
        let steps = args
            .phases
            .into_iter()
            .map(|phase| PlanStep {
                title: phase.title,
                detail: phase.detail,
                ..Default::default()
            })
            .collect();
        let plan = self.record_phases(id, args.title, steps).await?;
        Ok(format!(
            "Recorded {} phases. Start phase 1 now: delegate its lead (delegate_task, kind implement, phase 1) with the brief. Start each next phase once the one before it has landed.",
            plan.steps.len()
        ))
    }

    /// Records `steps` as the phases of the request the orchestrator serves, replacing the
    /// request's earlier plan.
    pub(crate) async fn record_phases(
        &self,
        id: &ConversationId,
        title: String,
        steps: Vec<PlanStep>,
    ) -> Result<Plan> {
        let request_id = self.request_for(id, None).await;
        self.record_plan_for(id, request_id, title, steps).await
    }

    async fn record_plan_for(
        &self,
        id: &ConversationId,
        request_id: Option<String>,
        title: String,
        steps: Vec<PlanStep>,
    ) -> Result<Plan> {
        let _held = self.plans.lock().await;
        let board = self.core.board(id).await?;
        for earlier in board.plans.values().filter(|plan| {
            plan.request_id == request_id
                && matches!(plan.state, PlanState::Proposed | PlanState::Approved { .. })
        }) {
            let mut replaced = earlier.clone();
            replaced.state = PlanState::Superseded;
            replaced.decided_at_ms = Some(now_ms());
            self.store_plan(&replaced).await?;
        }
        let plan = Plan {
            id: CardId::generate(),
            conversation_id: id.clone(),
            request_id,
            position: 0,
            title: title.trim().to_owned(),
            steps,
            state: PlanState::Approved {
                by: PlanApprover::Orchestrator,
            },
            created_at_ms: now_ms(),
            decided_at_ms: Some(now_ms()),
        };
        self.store_plan(&plan).await?;
        Ok(plan)
    }

    /// Changes a plan as it is stored now.
    pub(crate) async fn change_plan(
        &self,
        conversation_id: &ConversationId,
        plan_id: &CardId,
        change: impl FnOnce(&mut Plan) -> Result<()>,
    ) -> Result<Plan> {
        let _held = self.plans.lock().await;
        let board = self.core.board(conversation_id).await?;
        let before = board
            .plans
            .get(plan_id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("plan {plan_id}")))?;
        let mut plan = before.clone();
        change(&mut plan)?;
        if plan != before {
            self.store_plan(&plan).await?;
        }
        Ok(plan)
    }

    /// The current plan of `task`'s request and the index of the phase `task` leads (or works
    /// on, by its `phase`).
    pub(crate) async fn phase_of(&self, task: &Task) -> Option<(Plan, usize)> {
        let board = self.core.board(&task.conversation_id).await.ok()?;
        let plan = board
            .plans
            .values()
            .filter(|plan| {
                plan.request_id == task.request_id
                    && matches!(plan.state, PlanState::Approved { .. })
            })
            .max_by_key(|plan| plan.created_at_ms)?;
        let index = plan
            .steps
            .iter()
            .position(|step| step.task_id.as_ref() == Some(&task.id))
            .or_else(|| {
                let index = (task.phase? as usize).checked_sub(1)?;
                (index < plan.steps.len()).then_some(index)
            })?;
        Some((plan.clone(), index))
    }

    /// Moves the phase `task` works on to `stage`.
    pub(crate) async fn set_phase_stage(&self, task: &Task, stage: PhaseStage) {
        let Some((plan, index)) = self.phase_of(task).await else {
            return;
        };
        let result = self
            .change_plan(&task.conversation_id, &plan.id, |plan| {
                let step = &mut plan.steps[index];
                step.stage = stage;
                let now = now_ms();
                if step.started_at_ms.is_none() && stage != PhaseStage::Pending {
                    step.started_at_ms = Some(now);
                }
                if matches!(stage, PhaseStage::Done | PhaseStage::Failed) {
                    step.ended_at_ms = Some(now);
                } else {
                    step.ended_at_ms = None;
                }
                Ok(())
            })
            .await;
        if let Err(err) = result {
            tracing::warn!(task = %task.id, error = %err, "could not move a phase on");
        }
    }

    /// A lead takes on phase `number` of the request's plan.
    pub(crate) async fn assign_phase(&self, lead: &Task, number: u32) -> Result<()> {
        let (plan, index) = self
            .phase_step(&lead.conversation_id, lead.request_id.clone(), number)
            .await?;
        // In plan mode the lead outlines first: it builds only after its go-ahead.
        let stage = if self.plan_mode(&lead.conversation_id) {
            PhaseStage::Outlining
        } else {
            PhaseStage::Building
        };
        self.change_plan(&lead.conversation_id, &plan.id, |plan| {
            let step = &mut plan.steps[index];
            step.task_id = Some(lead.id.clone());
            step.stage = stage;
            step.started_at_ms.get_or_insert(now_ms());
            step.ended_at_ms = None;
            Ok(())
        })
        .await?;
        Ok(())
    }

    /// The plan of `request` and the index of its phase `number` (from 1).
    pub(crate) async fn phase_step(
        &self,
        id: &ConversationId,
        request: Option<String>,
        number: u32,
    ) -> Result<(Plan, usize)> {
        let board = self.core.board(id).await?;
        let plan = board
            .plans
            .values()
            .filter(|plan| {
                plan.request_id == request && matches!(plan.state, PlanState::Approved { .. })
            })
            .max_by_key(|plan| plan.created_at_ms)
            .cloned()
            .ok_or_else(|| {
                Error::Invalid(
                    "`phase` needs the request's phases (plan_phases); there are none. Leave `phase` out for a request of one phase.".into(),
                )
            })?;
        let index = (number as usize)
            .checked_sub(1)
            .filter(|index| *index < plan.steps.len())
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "the plan \"{}\" has phases 1 to {}; there is no phase {number}",
                    plan.title,
                    plan.steps.len()
                ))
            })?;
        Ok((plan, index))
    }

    /// `submit_outline`: a lead's outline. It waits for the go-ahead; meanwhile one reviewer
    /// from the other vendor reads it (not in plan mode, where the user reads it first).
    pub(crate) async fn submit_outline(
        &self,
        conversation_id: &ConversationId,
        task_id: &TaskId,
        outline: String,
    ) -> Result<String> {
        let outline = outline.trim().to_owned();
        if outline.is_empty() {
            return Err(Error::Invalid("the outline is empty".into()));
        }
        let live = self
            .existing_task_live(task_id)
            .ok_or_else(|| Error::Invalid("the task has ended".into()))?;
        let task = self.task_by_id(conversation_id, task_id).await?;
        if !task.kind.writes()
            || !matches!(
                task.role,
                None | Some(WorkerRole::Lead | WorkerRole::Parallel)
            )
        {
            return Err(Error::Invalid(
                "Only a lead writes an outline. Do your task and report.".into(),
            ));
        }
        if !matches!(task.state, TaskState::Starting | TaskState::Running) {
            return Err(Error::Invalid(format!(
                "task-{} is {:?}: an outline is written before the work starts",
                task.number, task.state
            )));
        }
        let outline = self.redact_for(&live, &outline).await;
        // A request of one phase gets its plan now: its outline, stage and pill live there.
        let (plan, index) = match self.phase_of(&task).await {
            Some(found) => found,
            None => {
                let step = PlanStep {
                    title: task.title.clone(),
                    task_id: Some(task.id.clone()),
                    stage: PhaseStage::Building,
                    started_at_ms: Some(task.created_at_ms),
                    ..Default::default()
                };
                let plan = self
                    .record_plan_for(
                        conversation_id,
                        task.request_id.clone(),
                        task.title.clone(),
                        vec![step],
                    )
                    .await?;
                self.update_task(conversation_id, task_id, |t| {
                    t.phase = Some(1);
                    t.role.get_or_insert(WorkerRole::Lead);
                })
                .await?;
                (plan, 0)
            }
        };
        let plan_mode = self.plan_mode(conversation_id);
        let stage = if plan_mode {
            PhaseStage::AwaitingGoAhead
        } else {
            PhaseStage::OutlineReview
        };
        let written = outline.clone();
        self.change_plan(conversation_id, &plan.id, |plan| {
            let step = &mut plan.steps[index];
            step.outline = Some(written);
            step.task_id.get_or_insert(task.id.clone());
            step.stage = stage;
            step.started_at_ms.get_or_insert(now_ms());
            Ok(())
        })
        .await?;
        self.set_task_blocked(task_id, Some(WAITING_FOR_GO_AHEAD.into()))
            .await;
        let task = self.task_by_id(conversation_id, task_id).await?;
        if plan_mode {
            self.deliver(
                conversation_id,
                Envelope {
                    kind: InjectionKind::Report,
                    label: format!("outline task-{}", task.number),
                    task_id: Some(task.id.clone()),
                    text: format!(
                        "{}\nPlan mode is on: show the user this outline in plain words and wait. Once they turn plan mode off or tell you to go, call approve_outline (task-{}) with any changes they asked for.",
                        outline_block(&task, &outline),
                        task.number
                    ),
                },
            )
            .await;
        } else {
            let manager = self.arc();
            self.spawn(async move { manager.review_outline(task, outline).await });
        }
        Ok("Outline received. End your turn now: the go-ahead, with any corrections, arrives as your next message. Build nothing before it.".into())
    }

    /// The one advisory review of an outline, by the other vendor; then the orchestrator
    /// decides on the go-ahead.
    async fn review_outline(&self, lead: Task, outline: String) {
        let spec = format!(
            "Review this outline before any code is written. It is advisory: the orchestrator merges the findings it agrees with into the lead's go-ahead.\n\nThe lead's brief (task-{number} \u{201c}{title}\u{201d}):\n{brief}\n\nThe lead's outline:\n{outline}\n\nRead the code the outline names (precise reads, line ranges). Find what would make the work fail or miss its brief: a wrong assumption about the code, a missed caller or file, a step in the wrong order, a \"done when\" it can't check for real, needless scope. Don't restate the outline or praise it. Report each finding in open_questions as one line: what is wrong, where (file:line), and the fix. No findings: say so in the summary.",
            number = lead.number,
            title = lead.title,
            brief = lead.spec,
        );
        let outcome = self
            .run_review(
                &lead,
                format!("Review the outline of task-{}", lead.number),
                spec,
            )
            .await;
        // The lead may have been stopped or steered meanwhile.
        let Ok(now) = self.task_by_id(&lead.conversation_id, &lead.id).await else {
            return;
        };
        if now.state.is_final() {
            return;
        }
        self.set_phase_stage(&now, PhaseStage::AwaitingGoAhead)
            .await;
        let review = match outcome {
            Ok(findings) => findings,
            Err(why) => format!("The review could not run: {why}. Judge the outline yourself."),
        };
        self.deliver(
            &lead.conversation_id,
            Envelope {
                kind: InjectionKind::Report,
                label: format!("outline task-{}", lead.number),
                task_id: Some(lead.id.clone()),
                text: format!(
                    "{}\n[outline review]\n{review}\n[/outline review]\nJudge the outline against the brief. Merge the findings you agree with into corrections (the brief wins any conflict; never reopen what is settled) and call approve_outline (task-{}). There are no review rounds.",
                    outline_block(&now, &outline),
                    lead.number
                ),
            },
        )
        .await;
    }

    /// Runs one review by the vendor other than `of`'s author and returns its findings as
    /// text, or why there are none.
    pub(crate) async fn run_review(
        &self,
        of: &Task,
        title: String,
        spec: String,
    ) -> std::result::Result<String, String> {
        let author = self.phase_author(of).await;
        let avoid = Some(brigadier_router::Author {
            provider: author.route.choice.provider,
            model: author.route.choice.model.clone(),
        });
        // Waited for before the reviewer starts, so a review that ends at once still arrives.
        let id = TaskId::generate();
        let rx = self.reviews.wait(&id);
        let created = self
            .create_task_as(
                &of.conversation_id,
                title,
                TaskKind::Review,
                spec,
                None,
                avoid,
                Vec::new(),
                None,
                Some(of.clone()),
                Vec::new(),
                None,
                None,
                Vec::new(),
                TaskExtra {
                    id: Some(id.clone()),
                    role: Some(WorkerRole::Reviewer),
                    phase: of.phase,
                    ..TaskExtra::default()
                },
            )
            .await;
        let reviewer = match created {
            Ok(reviewer) => reviewer,
            Err(err) => {
                self.reviews.settle(&id, Err(String::new()));
                return Err(err.to_string());
            }
        };
        if let Some(wait) = &reviewer.quota_wait {
            let why = format!(
                "no model of the other vendor can review now ({})",
                wait.reason
            );
            self.reviews.settle(&reviewer.id, Err(why.clone()));
            let _ = Box::pin(self.stop_task(reviewer.id.clone())).await;
            return Err(why);
        }
        // A reviewer that waits for quota or fails ends without a report; its end settles it.
        match rx.await {
            Ok(outcome) => outcome,
            Err(_) => Err("the reviewer ended without a result".into()),
        }
    }

    /// Whose work a review of `task` checks: the phase's lead (the vendor that wrote the
    /// phase), else the lead a verifier checks (an overnight phase without an outline), else
    /// `task` itself.
    pub(crate) async fn phase_author(&self, task: &Task) -> Task {
        if let Some((plan, index)) = self.phase_of(task).await
            && let Some(lead) = &plan.steps[index].task_id
            && lead != &task.id
            && let Ok(lead) = self.task_by_id(&task.conversation_id, lead).await
        {
            return lead;
        }
        if task.role == Some(WorkerRole::Verifier)
            && let Some(lead) = &task.subject
            && let Ok(lead) = self.task_by_id(&task.conversation_id, lead).await
        {
            return lead;
        }
        task.clone()
    }

    /// `approve_outline`: the go-ahead for a lead's outline, or, under "Ask for approval", a
    /// card that asks the user for it.
    pub(crate) async fn approve_outline(
        &self,
        id: &ConversationId,
        args: ApproveOutline,
    ) -> Result<String> {
        let lead = self.find_task(id, &args.task).await?;
        let Some((plan, index)) = self.phase_of(&lead).await else {
            return Err(Error::Invalid(format!(
                "task-{} has no outline to approve",
                lead.number
            )));
        };
        let step = &plan.steps[index];
        let Some(outline) = step.outline.clone() else {
            return Err(Error::Invalid(format!(
                "task-{} has no outline to approve",
                lead.number
            )));
        };
        if !matches!(
            step.stage,
            PhaseStage::OutlineReview | PhaseStage::AwaitingGoAhead
        ) || lead.state != TaskState::Blocked
        {
            return Err(Error::Invalid(format!(
                "task-{}'s outline is not waiting for a go-ahead",
                lead.number
            )));
        }
        if self.plan_mode(id) {
            return Err(Error::Invalid(
                "Plan mode is on: the user decides. Show them the outline and wait; approve it once they turn plan mode off or tell you to go.".into(),
            ));
        }
        let corrections = args
            .corrections
            .map(|c| c.trim().to_owned())
            .filter(|c| !c.is_empty());
        if self.permission(id) == PermissionLevel::AskForApproval && lead.run.is_none() {
            let shown = match &corrections {
                Some(corrections) => format!("{outline}\n\nCorrections:\n{corrections}"),
                None => outline,
            };
            let (_, rx) = self
                .open_approval(
                    id,
                    Some(lead.id.clone()),
                    ApprovalSubject::Outline {
                        task_id: lead.id.clone(),
                        title: plan.steps[index].title.clone(),
                        outline: shown,
                    },
                )
                .await?;
            let manager = self.arc();
            self.spawn(async move { manager.outline_decided(lead, corrections, rx).await });
            return Ok("The user decides on the outline now (\u{201c}Start this plan?\u{201d}). Their answer arrives as a message; carry on with anything else.".into());
        }
        self.go_ahead(&lead, corrections).await?;
        Ok(format!(
            "Sent task-{} the go-ahead. Its report arrives later as a message; don't wait for it.",
            lead.number
        ))
    }

    /// The user answered the "Start this plan?" card.
    async fn outline_decided(
        &self,
        lead: Task,
        corrections: Option<String>,
        rx: oneshot::Receiver<CardAnswer>,
    ) {
        use brigadier_providers::ApprovalDecision;
        let text = match rx.await {
            Ok(CardAnswer::Decision(ApprovalDecision::Allow | ApprovalDecision::AllowSimilar)) => {
                match self.go_ahead(&lead, corrections).await {
                    Ok(()) => format!(
                        "[decision] The user started task-{}'s outline; it builds now.",
                        lead.number
                    ),
                    Err(err) => format!(
                        "[decision] The user started task-{}'s outline, but the go-ahead could not reach it: {err}",
                        lead.number
                    ),
                }
            }
            Ok(CardAnswer::Decision(ApprovalDecision::Deny { message })) => format!(
                "[decision] The user did not start task-{}'s outline{}. The lead still waits: send it the changes with approve_outline, or stop it.",
                lead.number,
                if message.trim().is_empty() {
                    String::new()
                } else {
                    format!(": {message}")
                }
            ),
            // Withdrawn (the lead ended) or expired: nothing to say.
            _ => return,
        };
        self.deliver(
            &lead.conversation_id,
            Envelope {
                kind: InjectionKind::Decision,
                label: format!("outline task-{}", lead.number),
                task_id: Some(lead.id.clone()),
                text,
            },
        )
        .await;
    }

    /// Sends a lead "Go ahead." with the corrections; its phase builds.
    pub(crate) async fn go_ahead(&self, lead: &Task, corrections: Option<String>) -> Result<()> {
        let text = match &corrections {
            Some(corrections) => format!(
                "Go ahead, with these corrections to your outline (they win over it):\n{corrections}"
            ),
            None => "Go ahead.".to_owned(),
        };
        // Kept with the lead: its verifier and their reviews check the work against them too.
        if corrections.is_some() {
            let kept = text.clone();
            self.update_task(&lead.conversation_id, &lead.id, |task| {
                if !task.messages.contains(&kept) {
                    task.messages.push(kept);
                }
            })
            .await?;
        }
        self.set_task_blocked(&lead.id, None).await;
        let now = self.task_by_id(&lead.conversation_id, &lead.id).await?;
        self.message_worker(&lead.conversation_id, &now, text, "the orchestrator")
            .await?;
        self.set_phase_stage(&now, PhaseStage::Building).await;
        self.decided_for_task(
            &now,
            format!("Started task-{}'s outline", now.number),
            "Its outline fits the brief, after one review from the other vendor.".into(),
        )
        .await;
        Ok(())
    }

    /// In plan mode an implement worker only outlines: it builds after its go-ahead.
    pub(crate) async fn held_by_plan_mode(&self, task: &Task) -> bool {
        if task.kind != TaskKind::Implement || !self.plan_mode(&task.conversation_id) {
            return false;
        }
        match self.phase_of(task).await {
            Some((plan, index)) => !matches!(
                plan.steps[index].stage,
                PhaseStage::Building
                    | PhaseStage::Verifying
                    | PhaseStage::Landing
                    | PhaseStage::Done
            ),
            None => true,
        }
    }

    /// `request_review`: one review of the caller's committed work (from its phase's start)
    /// by the vendor other than the phase's author. Blocks until the findings are in; never
    /// gates anything.
    pub(crate) async fn request_review(
        &self,
        conversation_id: &ConversationId,
        task_id: &TaskId,
        focus: Option<String>,
    ) -> Result<String> {
        let task = self.task_by_id(conversation_id, task_id).await?;
        if !task.kind.writes() || task.kind == TaskKind::Merge {
            return Err(Error::Invalid(
                "Only a worker that builds a change asks for its review.".into(),
            ));
        }
        if self.held_by_plan_mode(&task).await {
            return Err(Error::Invalid(PLAN_MODE_HOLD.into()));
        }
        if !matches!(task.state, TaskState::Starting | TaskState::Running) {
            return Err(Error::Invalid(format!(
                "task-{} is {:?}: ask for the review while you work, before your report",
                task.number, task.state
            )));
        }
        // Every new file but litter: no report names them yet, and the landing checks their
        // provenance over the whole range anyway.
        self.commit_leftovers(
            &task,
            &[".".to_owned()],
            &format!(
                "{}\n\nWork in progress, committed for its review.",
                task.title
            ),
        )
        .await?;
        let focus = focus
            .map(|focus| focus.trim().to_owned())
            .filter(|focus| !focus.is_empty())
            .map(|focus| format!("\n\nThe worker asks you to look hardest at: {focus}"))
            .unwrap_or_default();
        let spec = format!(
            "Review the work of task-{number} \u{201c}{title}\u{201d}: your checkout is at its last commit, and the brief below says where its work starts. Read the whole diff from there, and the code around it (precise reads, line ranges). Find what is wrong: bugs, a \"done when\" not really met, missing or weak verification, stray files, needless scope, slop. This is a code review: don't rebuild or re-run the author's checks; run a command only to confirm a specific defect you suspect. Don't restate the change or praise it. Report each finding in open_questions as one line: what is wrong, where (file:line), and the fix. No findings: say so in the summary. Change nothing.{focus}",
            number = task.number,
            title = task.title,
        );
        // An overnight run's worker gives its slot to its reviewer while it waits (with one
        // worker allowed, the review could not start otherwise), and takes one back after.
        self.release_run_task(&task.id);
        let outcome = self
            .run_review(
                &task,
                format!("Review the work of task-{}", task.number),
                spec,
            )
            .await;
        self.admit_run_task(&task).await?;
        Ok(match outcome {
            Ok(findings) => format!(
                "[review]\n{findings}\n[/review]\nFix each finding you agree with and commit; for one you don't, say why in your report. There are no review rounds."
            ),
            Err(why) => format!(
                "The review could not run: {why}. Review your diff yourself, carefully, and say so in your report."
            ),
        })
    }

    /// Whether `task`'s report ends a phase that had an outline, or a phase of an overnight
    /// run: a fresh verifier then checks the phase before it lands.
    pub(crate) async fn needs_verifier(&self, task: &Task) -> bool {
        if task.kind != TaskKind::Implement
            || !matches!(task.role, Some(WorkerRole::Lead | WorkerRole::Parallel))
        {
            return false;
        }
        if task.run.as_ref().is_some_and(|run| {
            run.phase_id.is_some() && run.role == crate::overnight::RunRole::Worker
        }) {
            // A run that is ending verifies nothing more: its report says what is unverified.
            let ending = self
                .overnight
                .active
                .get(&task.conversation_id)
                .is_none_or(|active| active.winding_down);
            return !ending && self.verifier_of(task).await.is_none();
        }
        let Some((plan, index)) = self.phase_of(task).await else {
            return false;
        };
        plan.steps[index].outline.is_some() && self.verifier_of(task).await.is_none()
    }

    /// Starts the fresh verifier of `lead`'s phase: its own worktree on a branch from the
    /// lead's last commit, the phase's brief, outline and the lead's report. It runs one
    /// review by the other vendor, checks every "done when" for real, fixes and commits what
    /// fails, and reports; the orchestrator then lands the phase with it.
    pub(crate) async fn start_verifier(
        &self,
        lead: &Task,
        report: &crate::work::Report,
    ) -> Result<Task> {
        let (outline, phase_title) = match self.phase_of(lead).await {
            Some((plan, index)) => (
                plan.steps[index].outline.clone().unwrap_or_default(),
                plan.steps[index].title.clone(),
            ),
            None => (String::new(), lead.title.clone()),
        };
        let mut brief = lead.spec.clone();
        for message in &lead.messages {
            brief.push_str(&format!("\n---\n{message}"));
        }
        let mut summary = report.summary.clone();
        for line in report.done_when.iter().chain(&report.verification) {
            summary.push_str(&format!("\n- {line}"));
        }
        let report = summary;
        let spec = format!(
            "You verify this phase (\u{201c}{phase_title}\u{201d}) before it lands. Its lead, task-{number}, built it and reported; your worktree is a new branch at the lead's last commit, so its commits are yours to finish.\n1. Call request_review once: a reviewer from the other vendor reads the whole phase. Fix each finding you agree with; for one you don't, say why.\n2. Check every \"done when\" of the brief for real (run it, read it), and run the project's checks: typecheck, lint, build and the tests of what changed.\n3. Fix and commit whatever fails, in small steps. Don't redo the lead's work or widen the scope.\n4. submit_report: pass or fail per \"done when\" with its evidence, what the review found and what you fixed, and what is left. needs_user: only what the user alone can do.\n\nThe brief the lead worked from:\n{brief}\n\nThe outline it followed:\n{outline}\n\nThe lead's report:\n{report}",
            number = lead.number,
        );
        let verifier = self
            .create_task_as(
                &lead.conversation_id,
                format!("Verify {}", phase_title),
                TaskKind::Implement,
                spec,
                None,
                None,
                Vec::new(),
                None,
                Some(lead.clone()),
                Vec::new(),
                None,
                None,
                Vec::new(),
                TaskExtra {
                    role: Some(WorkerRole::Verifier),
                    phase: lead.phase,
                    request: lead.request_id.clone(),
                    ..TaskExtra::default()
                },
            )
            .await?;
        self.set_phase_stage(lead, PhaseStage::Verifying).await;
        Ok(verifier)
    }

    /// Who a checking task must not be, for a hand-off to another model: a review or a
    /// verification of a phase's work, the phase's author.
    pub(crate) async fn checker_avoid(
        &self,
        member: &Task,
    ) -> (
        Option<brigadier_router::Author>,
        Vec<brigadier_router::Author>,
    ) {
        if !matches!(member.kind, TaskKind::Review | TaskKind::Verify)
            && member.role != Some(WorkerRole::Verifier)
        {
            return (None, Vec::new());
        }
        let Some(subject) = &member.subject else {
            return (None, Vec::new());
        };
        let Ok(subject) = self.task_by_id(&member.conversation_id, subject).await else {
            return (None, Vec::new());
        };
        let author = self.phase_author(&subject).await;
        let choice = &author.route.choice;
        (
            Some(brigadier_router::Author {
                provider: choice.provider,
                model: choice.model.clone(),
            }),
            Vec::new(),
        )
    }

    /// Whether `task` is a lead whose outline waits for its go-ahead: its turn ending is
    /// expected, not a missing report.
    pub(crate) fn waits_for_go_ahead(task: &Task) -> bool {
        task.state == TaskState::Blocked
            && task.blocked_reason.as_deref() == Some(WAITING_FOR_GO_AHEAD)
    }
}

/// What a reviewer found, as the one waiting for it reads it.
pub(crate) fn review_text(report: &crate::work::Report) -> String {
    let mut text = report.summary.trim().to_owned();
    if !report.open_questions.is_empty() {
        text.push_str("\nFindings:");
        for finding in &report.open_questions {
            text.push_str(&format!("\n- {finding}"));
        }
    }
    if !report.risks.is_empty() {
        text.push_str("\nRisks:");
        for risk in &report.risks {
            text.push_str(&format!("\n- {risk}"));
        }
    }
    text
}

fn outline_block(lead: &Task, outline: &str) -> String {
    format!(
        "[outline task-{} \u{201c}{}\u{201d}]\n{outline}\n[/outline]",
        lead.number, lead.title
    )
}
