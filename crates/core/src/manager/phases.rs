//! A request's phases, as the delegator runs them: the orchestrator splits a big request into
//! phases only when they must run one after another, each phase gets one lead, and a lead whose
//! work is big writes an outline first. The orchestrator gets the outline at once and gives the
//! go-ahead (the user gives it under "Ask for approval"); a plan review by the other vendor runs
//! in the background meanwhile, and its findings may arrive after the go-ahead. A verifier is
//! the orchestrator's call, for big or risky work. Nothing here rejects, re-reviews or counts
//! rounds.

use tokio::sync::oneshot;

use super::SessionManager;
use super::cards::CardAnswer;
use super::conversation::Envelope;
use super::workers::TaskExtra;
use crate::model::{ConversationId, PermissionLevel};
use crate::tools::{ApproveOutline, PlanPhases, TaskRef};
use crate::work::{
    ApprovalSubject, CardId, InjectionKind, PhaseStage, Plan, PlanApprover, PlanState, PlanStep,
    Task, TaskId, TaskKind, TaskState, WorkerRole,
};
use crate::{Error, Result, now_ms};

/// What a lead waiting for its go-ahead is blocked on (the turn-end check reads it too).
pub(crate) const WAITING_FOR_GO_AHEAD: &str = "Waiting for the go-ahead on its outline";
/// What an implement worker hears in plan mode, before its outline's go-ahead.
pub(crate) const PLAN_MODE_HOLD: &str = "Plan mode is on: change nothing yet. Read the code, send your outline with submit_outline (even for small work) and stop; you build only after the go-ahead.";

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

    /// `submit_outline`: a lead's outline. It waits for the go-ahead, which the orchestrator
    /// gives once it has read the outline; outside plan mode (where the user reads it first)
    /// a plan review by the other vendor runs in the background meanwhile.
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
        let written = outline.clone();
        self.change_plan(conversation_id, &plan.id, |plan| {
            let step = &mut plan.steps[index];
            step.outline = Some(written);
            step.task_id.get_or_insert(task.id.clone());
            step.stage = PhaseStage::AwaitingGoAhead;
            step.started_at_ms.get_or_insert(now_ms());
            Ok(())
        })
        .await?;
        self.set_task_blocked(task_id, Some(WAITING_FOR_GO_AHEAD.into()))
            .await;
        let task = self.task_by_id(conversation_id, task_id).await?;
        let next = if plan_mode {
            format!(
                "Plan mode is on: show the user this outline in plain words and wait. Once they turn plan mode off or tell you to go, call approve_outline (task-{}) with any changes they asked for.",
                task.number
            )
        } else {
            format!(
                "Judge the outline against the brief and call approve_outline (task-{}) now, with corrections for anything it gets wrong (the brief wins any conflict; never reopen what is settled). A plan review by the other vendor runs in the background: its findings arrive as a [plan review …] message, maybe after the go-ahead. There are no review rounds.",
                task.number
            )
        };
        self.deliver(
            conversation_id,
            Envelope {
                kind: InjectionKind::Report,
                label: format!("outline task-{}", task.number),
                task_id: Some(task.id.clone()),
                text: format!("{}\n{next}", outline_block(&task, &outline)),
            },
        )
        .await;
        if !plan_mode {
            let manager = self.arc();
            self.spawn(async move { manager.review_plan(&task, &outline).await });
        }
        Ok("Outline received. End your turn now: the go-ahead, with any corrections, arrives as your next message. Build nothing before it.".into())
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
        if step.stage != PhaseStage::AwaitingGoAhead || lead.state != TaskState::Blocked {
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
        // Kept with the lead: a verifier and reviews check the work against them too.
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
            "Its outline fits the brief.".into(),
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

    /// `start_verifier`: the orchestrator's call for big or risky work. Starts a fresh verifier
    /// of a reported lead's work, which the orchestrator then lands instead of the lead.
    pub(crate) async fn verify_task(&self, id: &ConversationId, args: TaskRef) -> Result<String> {
        let lead = self.find_task(id, &args.task).await?;
        if lead.kind != TaskKind::Implement || lead.role == Some(WorkerRole::Verifier) {
            return Err(Error::Invalid(format!(
                "task-{} gets no verifier: only implement work does, and not a verifier's own",
                lead.number
            )));
        }
        let report = match (&lead.state, &lead.report) {
            (TaskState::Reported | TaskState::ReadyToLand, Some(report)) => report.clone(),
            _ => {
                return Err(Error::Invalid(format!(
                    "task-{} is {:?}: a verifier starts once its report is in",
                    lead.number, lead.state
                )));
            }
        };
        if let Some(verifier) = self.verifier_of(&lead).await {
            return Err(Error::Invalid(format!(
                "task-{} already has its verifier, task-{}.",
                lead.number, verifier.number
            )));
        }
        // A run that is ending verifies nothing more: its report says what is unverified.
        if lead.run.is_some()
            && self.overnight.active.get(id).is_none_or(|active| {
                active.winding_down || active.wind_down_at_ms.is_some_and(|at| now_ms() >= at)
            })
        {
            return Err(Error::Invalid(
                "The overnight run is ending: no verifier starts now. Land what is ready; what is left goes into the morning report.".into(),
            ));
        }
        let verifier = self.start_verifier(&lead, &report).await?;
        self.orchestrator_step(
            id,
            crate::work::OrchestratorStepKind::Created {
                task_id: verifier.id.clone(),
            },
        )
        .await;
        if let Some(wait) = &verifier.quota_wait {
            return Ok(format!(
                "Created task-{v}, a verifier of task-{n}'s work, but no model it may use can take it now: {}. It starts on its own when one can; land task-{v}, not task-{n}, once it reports.",
                wait.reason,
                v = verifier.number,
                n = lead.number,
            ));
        }
        Ok(format!(
            "Started task-{v}, a fresh verifier of task-{n}'s work, on top of its commits. Its report arrives later as a message; land task-{v}, not task-{n}, once it reports.",
            v = verifier.number,
            n = lead.number,
        ))
    }

    /// Starts a fresh verifier of `lead`'s phase: its own worktree on a branch from the lead's
    /// last commit, the phase's brief, outline and the lead's report. It asks for one review
    /// by the other vendor (`review_code`), checks every "done when" for real while it runs,
    /// triages the findings, fixes and commits what fails, and reports; the orchestrator then
    /// lands the phase with it.
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
            "You verify this phase (\u{201c}{phase_title}\u{201d}) before it lands. Its lead, task-{number}, built it and reported; your worktree is a new branch at the lead's last commit, so its commits are yours to finish.\n1. Call review_code once, first: it starts a review of the whole phase by the other vendor and returns at once.\n2. Meanwhile check every \"done when\" of the brief for real (run it, read it), and run the project's checks: typecheck, lint, build and the tests of what changed.\n3. Fix and commit whatever fails, in small steps. Don't redo the lead's work or widen the scope.\n4. The review's findings arrive as a message from Brigadier (if your checks finish first, end your turn: the findings start your next one). Triage them: fix and commit each one you agree with; for one you don't, say why.\n5. Then submit_report: pass or fail per \"done when\" with its evidence, what the review found and what you fixed, and what is left. needs_user: only what the user alone can do.\n\nThe brief the lead worked from:\n{brief}\n\nThe outline it followed:\n{outline}\n\nThe lead's report:\n{report}",
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

fn outline_block(lead: &Task, outline: &str) -> String {
    format!(
        "[outline task-{} \u{201c}{}\u{201d}]\n{outline}\n[/outline]",
        lead.number, lead.title
    )
}
