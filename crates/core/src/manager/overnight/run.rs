//! An overnight run on the session's own thread (THREAD-PLAN.md Q10): the same thread, its
//! workspace switched to the run's worktree, with a deadline and the user's restrictions.
//!
//! At Start the run's phases become an ordinary plan of the run's request, each step keeping
//! the source plan's own number, and the thread hears of the run in a `[run]` note: the goal,
//! the plan, the Rules, the deadline and the restrictions. From then on it works through the
//! plan with its normal tools. Landing records progress; the thread settles each step itself
//! (`settle_step`) once it has judged the step's whole scope and every "done when", and a step
//! settled done moves the run's accepted tip, which Merge takes, while every step before it
//! is done too. It ends the run with `end_run` when nothing is left that it can do.
//!
//! Code still holds what the user decided: the deadline and the worker cap, the steps "only"
//! and "skip" leave out (no work starts for them, and work under way hands off), and "stop
//! after": no work starts past the step it names, and the run winds down once that step is
//! settled.

use super::super::conversation::Envelope;
use super::super::{SessionManager, blocking, git_error};
use crate::board::Board;
use crate::model::{ConversationId, DomainEvent, Setup};
use crate::overnight::{Deadline, OvernightRun, OvernightState, StopAfter, StopReason};
use crate::tools::{EndRun, EndRunOutcome, SettleStep};
use crate::work::{
    InjectionKind, PhaseStage, Plan, PlanState, PlanStep, RequestState, StepOutcome,
    StepSettlement, TaskState, UserRequest,
};
use crate::{Error, Result, now_ms};

/// The user's words since Start carried into the `[run]` note, at most.
const WORDS_BYTES: usize = 12_000;

/// What a live worker of a step the user left out is told.
const LEFT_OUT: &str = "[Brigadier] The user left this phase out of the overnight run. Start nothing new. Finish only the step you are in the middle of, leave the worktree coherent and commit the finished steps (not broken work). Then call submit_report at once with a handoff a fresh worker could carry on from: Goal and where it stands; Done (with commit hashes); In progress; Next steps; How to verify.";

/// The request a run segment's own work belongs to (the thread's turns for it, its tasks).
pub(crate) fn run_request(run: &OvernightRun) -> String {
    format!("run-{}-g{}", run.id.short(), run.generation)
}

/// The run's plan: the one Start recorded, else one the thread made for a bare goal.
pub(crate) fn run_plan<'a>(board: &'a Board, run: &OvernightRun) -> Option<&'a Plan> {
    if let Some(plan) = run.plan_id.as_ref().and_then(|id| board.plans.get(id)) {
        return Some(plan);
    }
    let prefix = format!("run-{}-", run.id.short());
    board
        .plans
        .values()
        .filter(|plan| {
            plan.request_id
                .as_deref()
                .is_some_and(|request| request.starts_with(&prefix))
                && matches!(plan.state, PlanState::Approved { .. })
        })
        .max_by_key(|plan| plan.created_at_ms)
}

/// The step numbered `number` in `plan`, with its index.
pub(crate) fn step_numbered(plan: &Plan, number: u32) -> Option<(usize, &PlanStep)> {
    plan.steps
        .iter()
        .enumerate()
        .find(|(index, step)| step.number_at(*index) == number)
}

/// The steps the run's lineage settled done, by number: they are carried forward, and "only"
/// or "skip" may leave them out.
pub(crate) fn done_numbers(board: &Board, run: &OvernightRun) -> Vec<u32> {
    let mut numbers = Vec::new();
    let mut at = Some(run);
    let mut seen = 0;
    while let Some(segment) = at {
        if let Some(plan) = run_plan(board, segment) {
            for (index, step) in plan.steps.iter().enumerate() {
                if step
                    .settled
                    .as_ref()
                    .is_some_and(|settled| settled.outcome == StepOutcome::Done)
                {
                    numbers.push(step.number_at(index));
                }
            }
        }
        seen += 1;
        at = segment
            .predecessor
            .as_ref()
            .and_then(|id| board.runs.get(id))
            .filter(|_| seen < 64);
    }
    numbers.sort_unstable();
    numbers.dedup();
    numbers
}

/// The id a step's restriction binds to ("stop after this phase").
pub(crate) fn step_id(number: u32) -> String {
    format!("phase-{number}")
}

/// The step at work now: the first selected step not settled that has started, else the
/// first selected step not settled.
pub(crate) fn current_step(plan: &Plan) -> Option<u32> {
    let open = |step: &&PlanStep| step.settled.is_none() && step.stage != PhaseStage::Skipped;
    let numbered = || plan.steps.iter().enumerate();
    numbered()
        .find(|(_, step)| open(step) && step.stage != PhaseStage::Pending)
        .or_else(|| numbered().find(|(_, step)| open(step)))
        .map(|(index, step)| step.number_at(index))
}

/// The step "stop after" names, by its number.
fn stop_number(run: &OvernightRun) -> Option<u32> {
    match &run.directives.stop_after {
        Some(StopAfter::Phase { number }) => Some(*number),
        Some(StopAfter::Current { phase_id }) => phase_id
            .strip_prefix("phase-")
            .and_then(|number| number.parse().ok()),
        None => None,
    }
}

/// Whether the step numbered `number` comes after the one "stop after" names: no work starts
/// for it.
pub(crate) fn past_stop(run: &OvernightRun, plan: &Plan, number: u32) -> bool {
    let Some(stop) = stop_number(run) else {
        return false;
    };
    let position = |n: u32| step_numbered(plan, n).map(|(index, _)| index);
    match (position(stop), position(number)) {
        (Some(stop), Some(at)) => at > stop,
        _ => number > stop,
    }
}

/// Whether the run's "stop after" step is settled: the run winds down.
pub(crate) fn stop_reached(run: &OvernightRun, plan: Option<&Plan>) -> bool {
    let (Some(stop), Some(plan)) = (stop_number(run), plan) else {
        return false;
    };
    step_numbered(plan, stop).is_some_and(|(_, step)| step.settled.is_some())
}

impl SessionManager {
    pub(crate) async fn record_run(&self, run: &OvernightRun) -> Result<()> {
        self.record_runs(
            &run.conversation_id,
            vec![DomainEvent::OvernightUpdated {
                run: Box::new(run.clone()),
            }],
        )
        .await
    }

    /// Re-reads the run under its lock and applies `change` if the run is still the same
    /// generation and active, recording it. `None` when it moved on or `change` declined.
    pub(crate) async fn change_run_if(
        &self,
        run: &OvernightRun,
        change: impl FnOnce(&mut OvernightRun) -> Option<()>,
    ) -> Option<OvernightRun> {
        let _held = self.overnight.changes.lock().await;
        let board = self.core.board(&run.conversation_id).await.ok()?;
        let mut now = board.runs.get(&run.id)?.clone();
        if now.generation != run.generation || !now.state.is_active() {
            return None;
        }
        change(&mut now)?;
        if let Err(err) = self.record_run(&now).await {
            tracing::warn!(run = %run.id, error = %err, "could not record an overnight run");
            return None;
        }
        Some(now)
    }

    /// Start's second effect, once the run has its worktree: its phases become the plan of
    /// its request, it runs, and the thread hears of it. The thread's next turn resumes its
    /// own session with the run's worktree as its workspace.
    pub(crate) async fn begin_run(&self, run: OvernightRun) {
        let request = run_request(&run);
        let plan = match self.run_steps(&run).await {
            Some(steps) => {
                let title = run.name.clone();
                match self
                    .record_plan_for(&run.conversation_id, Some(request.clone()), title, steps)
                    .await
                {
                    Ok(plan) => Some(plan.id),
                    Err(err) => {
                        tracing::warn!(run = %run.id, error = %err, "could not record the run's plan");
                        None
                    }
                }
            }
            None => None,
        };
        let now = now_ms();
        if let Err(err) = self
            .core
            .record_conversation(
                &run.conversation_id,
                vec![DomainEvent::RequestUpdated {
                    request: UserRequest {
                        id: request.clone(),
                        conversation_id: run.conversation_id.clone(),
                        preview: format!("Overnight · {}", run.name),
                        state: RequestState::Working,
                        started_at_ms: now,
                        ended_at_ms: None,
                        steered_into: None,
                        steered_after: None,
                        undo: None,
                        worked: vec![crate::work::WorkSpan {
                            from_ms: now,
                            to_ms: None,
                        }],
                        quota_wait: false,
                    },
                }],
            )
            .await
        {
            tracing::warn!(run = %run.id, error = %err, "could not file the run's request");
        }
        let Some(run) = self
            .change_run_if(&run, |now| {
                if now.state != OvernightState::Preparing {
                    return None;
                }
                if plan.is_some() {
                    now.plan_id.clone_from(&plan);
                }
                now.state = OvernightState::Running;
                Some(())
            })
            .await
        else {
            return;
        };
        let text = if run.phases.is_empty() {
            format!(
                "[overnight] The run \u{201c}{}\u{201d} has started. Its goal, Rules, deadline and restrictions are in the [run] note. The user gave a goal without a plan: plan it first with plan_phases (only what the goal asks for), then work through it.",
                run.name
            )
        } else {
            format!(
                "[overnight] The run \u{201c}{}\u{201d} has started. Its plan, Rules, deadline and restrictions are in the [run] note. Work through the plan now.",
                run.name
            )
        };
        self.tell_thread(&run, "run started", text).await;
    }

    /// The steps of the run's plan: every source phase with its own number, those the user's
    /// restrictions leave out skipped, and those an earlier segment settled done kept done.
    /// `None` for a bare goal.
    async fn run_steps(&self, run: &OvernightRun) -> Option<Vec<PlanStep>> {
        if run.phases.is_empty() {
            return None;
        }
        let board = self.core.board(&run.conversation_id).await.ok();
        // What earlier segments settled done, by number, newest first.
        let mut carried: Vec<PlanStep> = Vec::new();
        if let Some(board) = &board {
            let mut at = run.predecessor.as_ref().and_then(|id| board.runs.get(id));
            while let Some(segment) = at {
                if let Some(plan) = run_plan(board, segment) {
                    for (index, step) in plan.steps.iter().enumerate() {
                        let done = step
                            .settled
                            .as_ref()
                            .is_some_and(|settled| settled.outcome == StepOutcome::Done);
                        let number = step.number_at(index);
                        if done && !carried.iter().any(|kept| kept.number == Some(number)) {
                            carried.push(PlanStep {
                                number: Some(number),
                                ..step.clone()
                            });
                        }
                    }
                }
                at = segment
                    .predecessor
                    .as_ref()
                    .and_then(|id| board.runs.get(id));
            }
        }
        Some(
            run.phases
                .iter()
                .map(|phase| {
                    if let Some(done) = carried
                        .iter()
                        .find(|step| step.number == Some(phase.number))
                    {
                        return done.clone();
                    }
                    PlanStep {
                        title: phase.name.clone(),
                        detail: Some(step_detail(phase)),
                        number: Some(phase.number),
                        stage: if run.selects(phase.number) {
                            PhaseStage::Pending
                        } else {
                            PhaseStage::Skipped
                        },
                        ..Default::default()
                    }
                })
                .collect(),
        )
    }

    /// Tells the thread something about its run, as a message of the run's request.
    pub(crate) async fn tell_thread(&self, run: &OvernightRun, label: &str, text: String) {
        self.deliver_for(
            &run.conversation_id,
            Envelope {
                kind: InjectionKind::Run,
                label: label.to_owned(),
                task_id: None,
                text,
            },
            Some(run_request(run)),
        )
        .await;
    }

    /// While an overnight run is active in the conversation: its worktree, and what the
    /// `[run]` note tells the thread about it (the plan, the Rules, the restrictions).
    pub(crate) async fn run_setting(
        &self,
        conversation_id: &ConversationId,
    ) -> Option<(crate::overnight::RunWorkspace, String)> {
        let active = self.overnight.active.get(conversation_id)?;
        let workspace = active.workspace?;
        let board = self.core.board(conversation_id).await.ok()?;
        let run = board.runs.get(&active.id)?;
        let words = self.words_since_start(run).await;
        let done = run
            .predecessor
            .as_ref()
            .and_then(|id| board.runs.get(id))
            .map(|previous| done_numbers(&board, previous))
            .unwrap_or_default();
        Some((workspace, run_brief(run, &words, &done)))
    }

    /// The user's messages since the run started, verbatim and bounded.
    async fn words_since_start(&self, run: &OvernightRun) -> String {
        let Some(started) = run.started_at_ms else {
            return String::new();
        };
        let id = &run.conversation_id;
        let branch = match self.core.head(id).await {
            Ok(Some(head)) => self.core.branch(id, &head).await.unwrap_or_default(),
            _ => Vec::new(),
        };
        let mut lines = Vec::new();
        let mut bytes = 0;
        for message in branch.iter().filter(|message| {
            message.role == crate::model::MessageRole::User && message.created_at_ms >= started
        }) {
            let text = self.full_words(message).await;
            bytes += text.len();
            if bytes > WORDS_BYTES {
                lines.push("(more in the transcript: search_transcript)".to_owned());
                break;
            }
            lines.push(format!("- {}", text.replace('\n', "\n  ")));
        }
        lines.join("\n")
    }

    /// The run branch's tip now.
    pub(crate) async fn run_tip(&self, run: &OvernightRun) -> Result<String> {
        let workspace = run
            .workspace
            .clone()
            .ok_or_else(|| Error::Invalid("the overnight run has no branch yet".into()))?;
        let repo = match self.core.conversation(&run.conversation_id)?.setup {
            Some(Setup::Session { repo, .. }) => repo,
            _ => return Err(Error::Invalid("overnight runs belong to a session".into())),
        };
        let git = self.git.clone();
        blocking(move || {
            git.open(std::path::Path::new(&repo))
                .map_err(git_error)?
                .branch_tip(&workspace.branch)
                .map_err(git_error)?
                .map(|oid| oid.0)
                .ok_or_else(|| Error::Invalid(format!("branch {} is gone", workspace.branch)))
        })
        .await
    }

    /// `settle_step`: the thread settles a step of its run's plan. A step settled done moves
    /// the accepted tip while every step before it is done too; a blocked one lists what it
    /// needs under "Waiting on you". The step "stop after" names ends the run once settled.
    pub(crate) async fn settle_step(
        &self,
        id: &ConversationId,
        args: SettleStep,
    ) -> Result<String> {
        let active = self.overnight.active.get(id).ok_or_else(|| {
            Error::Invalid(
                "No overnight run is going in this session: settle_step is only for a run's plan."
                    .into(),
            )
        })?;
        let board = self.core.board(id).await?;
        let run = board
            .runs
            .get(&active.id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("overnight run {}", active.id)))?;
        if run.state != OvernightState::Running && run.state != OvernightState::WindingDown {
            return Err(Error::Invalid(
                "The run isn't working on its plan now.".into(),
            ));
        }
        let plan = run_plan(&board, &run).cloned().ok_or_else(|| {
            Error::Invalid("The run has no plan yet: record it with plan_phases first.".into())
        })?;
        let (index, step) = step_numbered(&plan, args.phase).ok_or_else(|| {
            Error::Invalid(format!(
                "The run's plan has no phase {}; its phases are {}.",
                args.phase,
                plan.steps
                    .iter()
                    .enumerate()
                    .map(|(index, step)| step.number_at(index).to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        if step.stage == PhaseStage::Skipped {
            return Err(Error::Invalid(format!(
                "Phase {} is left out by the user's restrictions; there is nothing to settle.",
                args.phase
            )));
        }
        let open: Vec<String> = board
            .tasks
            .values()
            .filter(|task| {
                task.phase == Some(args.phase)
                    && task
                        .run
                        .as_ref()
                        .is_some_and(|context| context.run_id == run.id)
                    && !task.state.is_final()
            })
            .map(|task| {
                let what = match task.state {
                    TaskState::Reported if task.kind.writes() => "reported, not landed yet",
                    TaskState::Reported => "reported",
                    TaskState::Landing => "landing",
                    TaskState::ReadyToLand => "held back from landing",
                    TaskState::Paused => "paused",
                    TaskState::Blocked => "blocked",
                    _ => "running",
                };
                format!("task-{} ({what})", task.number)
            })
            .collect();
        if !open.is_empty() {
            return Err(Error::Invalid(format!(
                "Phase {} still has work going: {}. Land it with land_phase, or stop what is no longer needed, then settle the phase.",
                args.phase,
                open.join(", ")
            )));
        }
        let summary = args.summary.trim().to_owned();
        if summary.is_empty() {
            return Err(Error::Invalid(
                "Say in `summary` what the phase changed, how each \"done when\" was checked and what the review found.".into(),
            ));
        }
        let left: Vec<String> = args
            .left
            .iter()
            .map(|line| line.trim().to_owned())
            .filter(|line| !line.is_empty())
            .collect();
        if args.outcome != StepOutcome::Done && left.is_empty() {
            return Err(Error::Invalid(
                "Say in `left` what is left of the phase, one line each.".into(),
            ));
        }
        let tip = self.run_tip(&run).await?;
        let settlement = StepSettlement {
            outcome: args.outcome,
            summary: summary.clone(),
            left: left.clone(),
            tip: Some(tip.clone()),
            at_ms: now_ms(),
        };
        let plan = self
            .change_plan(id, &plan.id, |plan| {
                let step = &mut plan.steps[index];
                step.settled = Some(settlement.clone());
                step.stage = match args.outcome {
                    StepOutcome::Done => PhaseStage::Done,
                    _ => PhaseStage::Failed,
                };
                step.started_at_ms.get_or_insert(settlement.at_ms);
                step.ended_at_ms = Some(settlement.at_ms);
                Ok(())
            })
            .await?;
        // The accepted tip moves only while every step before this one is done.
        let accepted = args.outcome == StepOutcome::Done
            && plan.steps[..index].iter().all(|step| {
                step.stage == PhaseStage::Skipped
                    || step
                        .settled
                        .as_ref()
                        .is_some_and(|settled| settled.outcome == StepOutcome::Done)
            });
        let stop = stop_reached(&run, Some(&plan));
        let Some(now) = self
            .change_run_if(&run, |now| {
                if accepted {
                    now.verified_commit = Some(tip.clone());
                }
                if stop && now.state == OvernightState::Running {
                    now.state = OvernightState::WindingDown;
                    now.stop = Some(StopReason::StopDirective);
                }
                Some(())
            })
            .await
        else {
            return Err(Error::Invalid(
                "The run moved on meanwhile; nothing changed.".into(),
            ));
        };
        let request = Some(run_request(&now));
        let title = step.title.clone();
        if args.outcome == StepOutcome::Blocked {
            for gap in &left {
                self.run_waits(
                    &now,
                    request.clone(),
                    None,
                    &format!("Phase {}: {gap}", args.phase),
                )
                .await;
            }
        }
        let (what, why) = match args.outcome {
            StepOutcome::Done => (
                format!("Settled \u{201c}{title}\u{201d} as done"),
                "Its whole scope and every \"done when\" were checked and its work landed."
                    .to_owned(),
            ),
            outcome => (
                format!(
                    "Settled \u{201c}{title}\u{201d} as {}",
                    if outcome == StepOutcome::Blocked {
                        "blocked"
                    } else {
                        "partial"
                    }
                ),
                left.first().map_or_else(
                    || "Part of it is left.".to_owned(),
                    |gap| format!("Left: {gap}"),
                ),
            ),
        };
        self.phase_outcome_decided(&now, &step_id(args.phase), request, what, why)
            .await;
        if args.outcome == StepOutcome::Done {
            self.phase_verified_waiting(id, &now.id, args.phase).await;
        }
        if now.state == OvernightState::WindingDown {
            self.wind_down_soon(&now);
            return Ok(format!(
                "Settled phase {}. It was the last the user wanted: the run is ending now. Start nothing new; write the user's morning answer (what got done, what is left, what waits on them).",
                args.phase
            ));
        }
        Ok(format!(
            "Settled phase {}{}.",
            args.phase,
            if accepted {
                format!(
                    " at {}: the user's Merge takes up to here",
                    &tip[..tip.len().min(10)]
                )
            } else {
                String::new()
            }
        ))
    }

    /// `end_run`: the thread ends its run, its plan done or what is left needing the user.
    pub(crate) async fn end_run_now(&self, id: &ConversationId, args: EndRun) -> Result<String> {
        let active =
            self.overnight.active.get(id).ok_or_else(|| {
                Error::Invalid("No overnight run is going in this session.".into())
            })?;
        let board = self.core.board(id).await?;
        let run = board
            .runs
            .get(&active.id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("overnight run {}", active.id)))?;
        let why = args.why.trim().to_owned();
        let blocked_on = run_plan(&board, &run).and_then(|plan| {
            plan.steps
                .iter()
                .enumerate()
                .find(|(_, step)| {
                    step.stage != PhaseStage::Skipped
                        && !step
                            .settled
                            .as_ref()
                            .is_some_and(|settled| settled.outcome == StepOutcome::Done)
                })
                .map(|(index, step)| step_id(step.number_at(index)))
        });
        let ended = self
            .change_run_if(&run, |now| {
                if !matches!(
                    now.state,
                    OvernightState::Preparing | OvernightState::Running
                ) {
                    return None;
                }
                now.state = OvernightState::WindingDown;
                now.stop = Some(match args.outcome {
                    EndRunOutcome::Done => StopReason::Done,
                    EndRunOutcome::NeedsUser => StopReason::Blocked {
                        phase_id: blocked_on.clone().unwrap_or_else(|| "the plan".into()),
                    },
                });
                Some(())
            })
            .await;
        let Some(now) = ended else {
            return Ok("The run is already ending.".into());
        };
        if !why.is_empty() {
            self.run_decided(
                &now,
                None,
                Some(run_request(&now)),
                "Ended the run".into(),
                why,
            )
            .await;
        }
        self.wind_down_soon(&now);
        Ok("The run is ending. Start nothing new. Write the user's morning answer now: what got done, what is left and what waits on them, briefly. Brigadier adds the run's report (commits, reviews, waiting items, usage) after it.".into())
    }

    /// Lists what only the user can do for the run, for the morning.
    pub(crate) async fn run_waits(
        &self,
        run: &OvernightRun,
        request: Option<String>,
        task: Option<crate::model::TaskId>,
        what: &str,
    ) {
        if let Err(err) = self
            .wait_on_user(
                &run.conversation_id,
                request,
                crate::work::WaitingSource::Run {
                    run_id: run.id.clone(),
                    task_id: task,
                },
                what,
            )
            .await
        {
            tracing::warn!(run = %run.id, error = %err, "could not list what the run needs");
        }
    }

    /// The run's decision, under "Decided for you".
    pub(crate) async fn run_decided(
        &self,
        run: &OvernightRun,
        phase_id: Option<&str>,
        request: Option<String>,
        what: String,
        why: String,
    ) {
        self.run_decided_as(
            run,
            phase_id,
            request,
            crate::work::DecisionKind::Routine,
            what,
            why,
        )
        .await;
    }

    /// [`Self::run_decided`] for a step that settled: the run's card shows it.
    pub(crate) async fn phase_outcome_decided(
        &self,
        run: &OvernightRun,
        phase_id: &str,
        request: Option<String>,
        what: String,
        why: String,
    ) {
        self.run_decided_as(
            run,
            Some(phase_id),
            request,
            crate::work::DecisionKind::PhaseOutcome,
            what,
            why,
        )
        .await;
    }

    async fn run_decided_as(
        &self,
        run: &OvernightRun,
        phase_id: Option<&str>,
        request: Option<String>,
        kind: crate::work::DecisionKind,
        what: String,
        why: String,
    ) {
        self.record_decision(
            &run.conversation_id,
            request,
            crate::work::DecisionSource::Run {
                run_id: run.id.clone(),
                phase_id: phase_id.map(str::to_owned),
            },
            kind,
            what,
            why,
        )
        .await;
    }

    /// The user's restrictions changed during the run: steps they leave out are skipped and
    /// their work under way hands off; steps they bring back are open again. A "stop after"
    /// whose step is already settled ends the run.
    pub(crate) async fn apply_run_selection(&self, run: &OvernightRun) {
        let Ok(board) = self.core.board(&run.conversation_id).await else {
            return;
        };
        let Some(plan) = run_plan(&board, run).cloned() else {
            return;
        };
        let mut dropped = Vec::new();
        let changed = self
            .change_plan(&run.conversation_id, &plan.id, |plan| {
                for (index, step) in plan.steps.iter_mut().enumerate() {
                    let number = step.number_at(index);
                    if step.settled.is_some() {
                        continue;
                    }
                    let selected = run.selects(number);
                    if !selected && step.stage != PhaseStage::Skipped {
                        step.stage = PhaseStage::Skipped;
                        dropped.push(number);
                    } else if selected && step.stage == PhaseStage::Skipped {
                        step.stage = PhaseStage::Pending;
                    }
                }
                Ok(())
            })
            .await;
        let plan = match changed {
            Ok(plan) => plan,
            Err(err) => {
                tracing::warn!(run = %run.id, error = %err, "could not apply the run's new selection");
                plan
            }
        };
        // Work of a step left out (or past the new stopping point) hands off cleanly; work that
        // waits to start (for the task it builds on, or a worker slot) never starts, its
        // unfinished changes kept on its branch.
        for task in board.tasks.values().filter(|task| {
            task.run
                .as_ref()
                .is_some_and(|context| context.run_id == run.id)
                && task
                    .phase
                    .is_some_and(|n| dropped.contains(&n) || past_stop(run, &plan, n))
        }) {
            match task.state {
                _ if task.state.is_final() => {}
                state if state == TaskState::Queued || self.waits_for_slot(task) => {
                    if let Err(err) = self.stop_task(task.id.clone()).await {
                        tracing::warn!(task = %task.id, error = %err, "could not stop a task of a phase the user left out");
                    }
                }
                TaskState::Running
                | TaskState::Starting
                | TaskState::Blocked
                | TaskState::Paused
                    if task.kind.writes() =>
                {
                    let words = LEFT_OUT.to_owned();
                    let _ = self
                        .message_worker(&run.conversation_id, task, words, "Brigadier")
                        .await;
                }
                _ => {}
            }
        }
        if stop_reached(run, Some(&plan))
            && let Some(now) = self
                .change_run_if(run, |now| {
                    (now.state == OvernightState::Running).then(|| {
                        now.state = OvernightState::WindingDown;
                        now.stop = Some(StopReason::StopDirective);
                    })
                })
                .await
        {
            let words = super::wind_down::ENDING
                .replace("{reason}", "the user's stop after a phase is reached");
            self.tell_thread(&now, "run ending", words).await;
            self.wind_down_soon(&now);
        }
    }

    /// The run ended: what was owed to its requests is over. The thread's next turn resumes
    /// its own session in the session's checkout, told so by its notes.
    pub(crate) async fn run_finished(&self, run: &OvernightRun) {
        tracing::info!(run = %run.id, stop = ?run.stop, "overnight run finished");
        // The report answers the run's requests: they settle for good, and nothing owed to
        // them (a reminder, a late message) starts a turn after the run.
        if let Ok(conv) = self.conv(&run.conversation_id) {
            conv.forget_run_requests(&format!("run-{}-", run.id.short()))
                .await;
        }
        self.settle_requests(&run.conversation_id).await;
    }
}

/// The step's detail on the plan: its scope, its "done when" and what it builds on.
fn step_detail(phase: &crate::overnight::OvernightPhase) -> String {
    let mut detail = phase.scope.trim().to_owned();
    detail.push_str("\nDone when:");
    for criterion in &phase.done_when {
        detail.push_str(&format!("\n- {}: {}", criterion.id, criterion.text));
    }
    if !phase.depends_on.is_empty() {
        detail.push_str(&format!(
            "\nBuilds on phase{} {}.",
            if phase.depends_on.len() == 1 { "" } else { "s" },
            numbers(&phase.depends_on)
        ));
    }
    detail
}

fn numbers(numbers: &[u32]) -> String {
    numbers
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// What the `[run]` note tells the thread: the run's goal and plan, the Rules and the user's
/// words verbatim, the restrictions Brigadier holds, and how the thread works through it.
/// Only what changes with the user's words: settling a step doesn't change it.
pub(crate) fn run_brief(run: &OvernightRun, words: &str, done: &[u32]) -> String {
    let mut text = format!(
        "Run \u{201c}{}\u{201d}. Its goal, in the user's words:\n{}\n",
        run.name, run.goal
    );
    if run.phases.is_empty() {
        text.push_str("\nNo plan was given: plan the goal with plan_phases first (only what it asks for), then work through it.\n");
    } else {
        text.push_str("\nThe plan (each phase by its own number; work only on the phases the user selected):\n");
        for phase in &run.phases {
            text.push_str(&format!(
                "\nPhase {} · {}{}\n{}\nDone when:\n",
                phase.number,
                phase.name,
                if done.contains(&phase.number) {
                    " (settled done by an earlier segment of this run: don't redo it)"
                } else if run.selects(phase.number) {
                    ""
                } else {
                    " (left out by the user: don't work on it)"
                },
                phase.scope.trim()
            ));
            for criterion in &phase.done_when {
                text.push_str(&format!("- {}: {}\n", criterion.id, criterion.text));
            }
            if !phase.depends_on.is_empty() {
                text.push_str(&format!(
                    "It builds on phase{} {}.\n",
                    if phase.depends_on.len() == 1 { "" } else { "s" },
                    numbers(&phase.depends_on)
                ));
            }
        }
    }
    text.push_str(&format!(
        "\nRules and settled decisions, verbatim (they bind you and every worker; pass the parts that matter into task specs):\n{}\n",
        if run.rules.trim().is_empty() {
            "(none given)"
        } else {
            run.rules.as_str()
        }
    ));
    text.push_str(&format!(
        "\nThe user's words when they started the run, verbatim:\n{}\n",
        run.words
    ));
    if !words.is_empty() {
        text.push_str(&format!(
            "\nWhat the user wrote since Start, verbatim, oldest first (it supersedes earlier choices; it can't change a phase's scope or settled decisions):\n{words}\n"
        ));
    }
    text.push_str(&format!("\n{}\n", directives_text(run)));
    text.push_str("\nHow the run goes: give each phase's work to a lead with delegate_task and `phase: <its number>` (tiny edits you make yourself, committed on the run's branch). Land finished work with land_phase. Once a phase's whole scope and every \"done when\" are met and landed, settle it with settle_step (partial or blocked with what is left, when it can't be finished); the user's Merge takes the work up to the last phase settled done with every phase before it done. Phases that build on one left unfinished wait. When every selected phase is settled, or all that is left needs the user, call end_run. Nobody can answer before the morning: decide what the plan and the Rules settle (note_for_user, kind decided) and list what only the user can do (note_for_user, kind waiting).");
    text
}

/// The restrictions Brigadier holds, as the thread should know them.
pub(crate) fn directives_text(run: &OvernightRun) -> String {
    let mut lines = vec!["Restrictions Brigadier holds for you:".to_owned()];
    match &run.directives.deadline {
        Deadline::At { time } => lines.push(format!(
            "- The report is due at {} {} ({}); Brigadier stops new work in time to end cleanly and tells you when.",
            time.day, time.local_time, time.offset
        )),
        Deadline::For { minutes } => lines.push(format!("- The run lasts {minutes} minutes.")),
        Deadline::UntilDone => lines.push("- No deadline: until the plan is done.".into()),
    }
    if let Some(cap) = run.directives.max_workers {
        lines.push(format!(
            "- At most {cap} worker{} of this run at once; more wait for a free one.",
            if cap == 1 { "" } else { "s" }
        ));
    }
    match &run.directives.stop_after {
        Some(StopAfter::Phase { number }) => lines.push(format!(
            "- The run stops once phase {number} is settled; nothing past it starts."
        )),
        Some(StopAfter::Current { phase_id }) => lines.push(format!(
            "- The run stops once {} is settled; nothing past it starts.",
            phase_id.replace('-', " ")
        )),
        None => {}
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ConversationId;
    use crate::overnight::OvernightPhase;

    fn plan(numbers: &[u32]) -> Plan {
        Plan {
            id: crate::work::CardId::generate(),
            conversation_id: ConversationId("c".into()),
            request_id: None,
            position: 0,
            title: "Plan".into(),
            body: None,
            steps: numbers
                .iter()
                .map(|n| PlanStep {
                    title: format!("Phase {n}"),
                    number: Some(*n),
                    ..Default::default()
                })
                .collect(),
            state: PlanState::Approved {
                by: crate::work::PlanApprover::Orchestrator,
            },
            created_at_ms: 0,
            decided_at_ms: None,
        }
    }

    #[test]
    fn steps_keep_their_source_numbers_and_stop_after_binds_by_them() {
        let mut run = OvernightRun::for_test(
            ConversationId("c".into()),
            "Run",
            vec![
                OvernightPhase::new(3, "Three", "s", &["x".into()], &[]),
                OvernightPhase::new(4, "Four", "s", &["y".into()], &[]),
            ],
        );
        let plan = plan(&[3, 4]);
        assert_eq!(step_numbered(&plan, 4).map(|(index, _)| index), Some(1));
        assert!(step_numbered(&plan, 1).is_none());
        run.directives.stop_after = Some(StopAfter::Phase { number: 3 });
        assert!(past_stop(&run, &plan, 4));
        assert!(!past_stop(&run, &plan, 3));
        assert!(!stop_reached(&run, Some(&plan)));
        let mut settled = plan.clone();
        settled.steps[0].settled = Some(StepSettlement {
            outcome: StepOutcome::Partial,
            summary: "s".into(),
            left: vec!["l".into()],
            tip: None,
            at_ms: 0,
        });
        assert!(stop_reached(&run, Some(&settled)));
        assert_eq!(current_step(&settled), Some(4));
    }
}
