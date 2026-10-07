//! The conductor (PLAN.md §10.6): it takes a started run through its phases, each the way a
//! request's phase goes. Each phase gets a fresh lead, the session's orchestrator started over
//! from the phase's own briefing (its scope, criteria, the user's Rules and words, what earlier
//! phases settled) without the conversation's recent messages. It briefs one worker lead, which
//! may outline first (one advisory review); the lead builds and reports, a fresh verifier
//! checks every "done when" with one review from the other vendor and fixes what fails, and
//! `land_phase` lands the work on the run branch. The orchestrator then settles the phase
//! (`phase_done`): done, partial or blocked, judged from the verifier's report.
//!
//! The conductor decides in code what comes next: the next selected phase whose dependencies
//! are verified, a clean ending when the plan is done, a stop directive was reached or a later
//! phase depends on work a blocked phase couldn't finish. Every step re-reads the run under
//! its lock, records the decision first and checks the run's generation again before acting
//! on a result that arrives later.

use std::time::Duration;

use super::super::conversation::Envelope;
use super::super::{SessionManager, blocking, git_error};
use super::policy::PLANNING_PHASE;
use crate::board::Board;
use crate::model::{ConversationId, DomainEvent, MessageRole, OvernightRunId, Setup};
use crate::overnight::{
    Deadline, OvernightPhase, OvernightRun, OvernightState, PhaseState, PlanningPhase, StopAfter,
    StopReason,
};
use crate::tools::{PhaseDone, PhaseOutcome, ProposePhases};
use crate::work::{InjectionKind, RequestState, TaskState, UserRequest};
use crate::{Error, Result, now_ms};

/// How long a phase waits for the orchestrator's turn (the user's own question, say) to end
/// before it takes over the orchestrator, per try.
const LEAD_WAIT: Duration = Duration::from_secs(10 * 60);
/// How long a lead's turn may end with the phase still open before it is reminded.
const NUDGE_GRACE: Duration = if cfg!(test) {
    Duration::from_secs(2)
} else {
    Duration::from_secs(60)
};
/// Reminders a lead gets to finish or hand over its phase; after them its phase is checked
/// as it is.
const NUDGES: u32 = 2;
/// The user's words since Start carried into a phase briefing, at most.
const WORDS_BYTES: usize = 12_000;

/// What the conductor does next for a run.
enum Next {
    Nothing,
    Plan(OvernightRun),
    Phase(OvernightRun, String),
    WindDown(OvernightRun),
}

/// Where the plan goes after the phases settled so far.
#[derive(Debug, PartialEq, Eq)]
enum Pick {
    Phase(String),
    /// Nothing can start now (a phase still runs or waits on one that does).
    Wait,
    End(StopReason),
}

impl SessionManager {
    /// Takes the run one step further, if there is a step to take now. Safe to call any time
    /// and repeatedly: it re-reads the run and does nothing when nothing is due.
    pub(crate) fn advance_soon(&self, conversation_id: &ConversationId, run_id: &OvernightRunId) {
        let (manager, conversation_id, run_id) =
            (self.arc(), conversation_id.clone(), run_id.clone());
        self.spawn(async move { manager.advance_run(&conversation_id, &run_id).await });
    }

    pub(crate) async fn advance_run(
        &self,
        conversation_id: &ConversationId,
        run_id: &OvernightRunId,
    ) {
        let next = {
            let _held = self.overnight.changes.lock().await;
            match self.next_step(conversation_id, run_id).await {
                Ok(next) => next,
                Err(err) => {
                    tracing::warn!(run = %run_id, error = %err, "could not advance an overnight run");
                    return;
                }
            }
        };
        match next {
            Next::Nothing => {}
            Next::Plan(run) => self.lead_planning(run).await,
            Next::Phase(run, phase_id) => self.lead_phase_of(run, phase_id).await,
            Next::WindDown(run) => self.end_run(run).await,
        }
    }

    /// Decides and records the run's next step. Called under the run lock.
    async fn next_step(
        &self,
        conversation_id: &ConversationId,
        run_id: &OvernightRunId,
    ) -> Result<Next> {
        let board = self.core.board(conversation_id).await?;
        let Some(run) = board.runs.get(run_id) else {
            return Ok(Next::Nothing);
        };
        let mut run = run.clone();
        match run.state {
            OvernightState::Preparing if run.workspace.is_some() => {
                if run.phases.is_empty() {
                    let request_id = format!("run-{}-phase-0", run.id.short());
                    run.state = OvernightState::Planning;
                    run.planning = Some(PlanningPhase {
                        request_id: request_id.clone(),
                        state: PhaseState::Running,
                        plan_id: None,
                        proposed: Vec::new(),
                        gaps: Vec::new(),
                        lead: None,
                        nudges: 0,
                        started_at_ms: now_ms(),
                        settled_at_ms: None,
                    });
                    self.record_run_with_request(&run, &request_id, "Phase 0 · Write the plan")
                        .await?;
                    return Ok(Next::Plan(run));
                }
                run.state = OvernightState::Running;
                self.pick_and_record(run, &board).await
            }
            OvernightState::Running => self.pick_and_record(run, &board).await,
            OvernightState::WindingDown => Ok(Next::WindDown(run)),
            _ => Ok(Next::Nothing),
        }
    }

    /// Starts the next phase, or ends the run when nothing is left to start.
    async fn pick_and_record(&self, mut run: OvernightRun, board: &Board) -> Result<Next> {
        let verified = super::verified_numbers(board, &run);
        match pick_next(&run, &verified) {
            Pick::Wait => {
                // The state may have moved from Preparing to Running.
                if board
                    .runs
                    .get(&run.id)
                    .is_some_and(|before| before.state != run.state)
                {
                    self.record_run(&run).await?;
                }
                Ok(Next::Nothing)
            }
            Pick::Phase(phase_id) => {
                let request_id = format!("run-{}-{phase_id}-g{}", run.id.short(), run.generation);
                let preview = {
                    let phase = run
                        .phases
                        .iter_mut()
                        .find(|phase| phase.id == phase_id)
                        .ok_or_else(|| Error::NotFound(phase_id.clone()))?;
                    phase.state = PhaseState::Running;
                    phase.request_id = Some(request_id.clone());
                    phase.started_at_ms = Some(now_ms());
                    format!("Phase {} · {}", phase.number, phase.name)
                };
                run.state = OvernightState::Running;
                self.record_run_with_request(&run, &request_id, &preview)
                    .await?;
                // Moving past a phase that needs the user: say why that's sound.
                let unfinished: Vec<String> = run
                    .phases
                    .iter()
                    .filter(|phase| {
                        matches!(phase.state, PhaseState::Partial | PhaseState::Blocked)
                    })
                    .map(|phase| format!("phase {}", phase.number))
                    .collect();
                if !unfinished.is_empty() {
                    self.run_decided(
                        &run,
                        Some(&phase_id),
                        Some(request_id.clone()),
                        format!("Went on with {}", preview),
                        format!(
                            "It doesn't build on {}, which {} unfinished.",
                            unfinished.join(" or "),
                            if unfinished.len() == 1 { "is" } else { "are" }
                        ),
                    )
                    .await;
                }
                Ok(Next::Phase(run, phase_id))
            }
            Pick::End(reason) => {
                run.state = OvernightState::WindingDown;
                run.stop = Some(reason);
                self.record_run(&run).await?;
                Ok(Next::WindDown(run))
            }
        }
    }

    /// Records a run that starts a phase, with the phase's own request: the lead's turns,
    /// tasks and reports belong to it, not to whatever the user wrote last.
    async fn record_run_with_request(
        &self,
        run: &OvernightRun,
        request_id: &str,
        preview: &str,
    ) -> Result<()> {
        let now = now_ms();
        self.core
            .record_conversation(
                &run.conversation_id,
                vec![DomainEvent::RequestUpdated {
                    request: UserRequest {
                        id: request_id.to_owned(),
                        conversation_id: run.conversation_id.clone(),
                        preview: preview.to_owned(),
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
            .await?;
        self.record_run(run).await
    }

    pub(crate) async fn record_run(&self, run: &OvernightRun) -> Result<()> {
        self.record_runs(
            &run.conversation_id,
            vec![DomainEvent::OvernightUpdated {
                run: Box::new(run.clone()),
            }],
        )
        .await
    }

    /// Hands the phase to a fresh lead: the orchestrator starts over from the phase's
    /// briefing once its current turn (if any) is over.
    async fn lead_phase_of(&self, run: OvernightRun, phase_id: String) {
        let tip = match self.run_tip(&run).await {
            Ok(tip) => tip,
            Err(err) => {
                tracing::warn!(run = %run.id, error = %err, "could not read the run branch");
                return;
            }
        };
        let lead = self.lead_choice(&run.conversation_id);
        // The phase starts from the branch as it is now; a stale call changes nothing.
        let started = self
            .change_run_if(&run, |now| {
                let phase = now.phases.iter_mut().find(|phase| phase.id == phase_id)?;
                if phase.state != PhaseState::Running || phase.start_commit.is_some() {
                    return None;
                }
                phase.start_commit = Some(tip.clone());
                phase.lead = lead.clone();
                Some(())
            })
            .await;
        let Some(run) = started else {
            return;
        };
        let Some(phase) = run.phase(&phase_id).cloned() else {
            return;
        };
        let Some(request) = phase.request_id.clone() else {
            return;
        };
        let briefing = self.phase_brief(&run, &phase).await;
        let kickoff = format!(
            "[overnight · phase {n}] Lead phase {n} (\u{201c}{name}\u{201d}) now, as your briefing describes. Give it one lead (delegate_task, kind implement) with a complete brief: the scope, every \"done when\" with its id, and code pointers. A lead of big work sends an outline for your go-ahead (approve_outline). When the lead reports, Brigadier starts the phase's verifier; land the phase with land_phase on the verifier once it reports. Then settle the phase with phase_done (done, partial or blocked), judging the verifier's report. Don't write to the user: reply with exactly {quiet}.",
            n = phase.number,
            name = phase.name,
            quiet = super::super::prompts::QUIET,
        );
        self.hand_to_lead(
            &run,
            briefing,
            kickoff,
            request,
            format!("phase {}", phase.number),
        )
        .await;
    }

    /// Phase 0 of a bare goal: a fresh lead writes the plan's phases.
    async fn lead_planning(&self, run: OvernightRun) {
        let Some(planning) = run.planning.clone() else {
            return;
        };
        let lead = self.lead_choice(&run.conversation_id);
        let Some(run) = self
            .change_run_if(&run, |now| {
                let planning = now.planning.as_mut()?;
                if planning.lead.is_some() {
                    return None;
                }
                planning.lead = lead.clone();
                Some(())
            })
            .await
        else {
            return;
        };
        let briefing = planning_brief(&run);
        let kickoff = format!(
            "[overnight · phase 0] Write this run's plan now, as your briefing describes, and propose it with propose_phases. Don't write to the user: reply with exactly {} until the plan is proposed.",
            super::super::prompts::QUIET
        );
        self.hand_to_lead(
            &run,
            briefing,
            kickoff,
            planning.request_id,
            "phase 0".into(),
        )
        .await;
    }

    /// Starts the lead over from `briefing`, waiting as long as it takes for the orchestrator's
    /// running turn to end (unless the run moves on meanwhile).
    async fn hand_to_lead(
        &self,
        run: &OvernightRun,
        briefing: String,
        kickoff: String,
        request: String,
        label: String,
    ) {
        loop {
            let envelope = Envelope {
                kind: InjectionKind::Phase,
                label: label.clone(),
                task_id: None,
                text: kickoff.clone(),
            };
            match self
                .lead_phase(
                    &run.conversation_id,
                    briefing.clone(),
                    envelope,
                    request.clone(),
                    LEAD_WAIT,
                )
                .await
            {
                Ok(true) => return,
                Ok(false) => {
                    // Still the same run, generation and phase? Then wait on.
                    let current =
                        self.overnight
                            .active
                            .get(&run.conversation_id)
                            .is_some_and(|active| {
                                active.id == run.id && active.generation == run.generation
                            });
                    if !current {
                        return;
                    }
                    tracing::info!(run = %run.id, "the orchestrator is still busy; the phase waits for its turn to end");
                }
                Err(err) => {
                    tracing::warn!(run = %run.id, error = %err, "could not hand a phase to its lead");
                    return;
                }
            }
        }
    }

    /// The model a phase lead starts on: the session's own orchestrator choice (the user's
    /// saved setup stays as it is).
    fn lead_choice(&self, conversation_id: &ConversationId) -> Option<crate::model::ModelChoice> {
        match self.core.conversation(conversation_id).ok()?.setup {
            Some(Setup::Session { orchestrator, .. }) => Some(orchestrator),
            _ => None,
        }
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

    /// While an overnight run is active in the conversation: its branch, and the restrictions
    /// Brigadier enforces for it as a lead knows them.
    pub(crate) async fn run_setting(
        &self,
        conversation_id: &ConversationId,
    ) -> Option<(crate::overnight::RunWorkspace, String)> {
        let active = self.overnight.active.get(conversation_id)?;
        let workspace = active.workspace?;
        let board = self.core.board(conversation_id).await.ok()?;
        let run = board.runs.get(&active.id)?;
        Some((workspace, directives_text(run)))
    }

    /// While a phase of the conversation's active run (Phase 0 too) is being led: its
    /// briefing as it is now, and when the phase started.
    pub(crate) async fn led_phase(
        &self,
        conversation_id: &ConversationId,
    ) -> Option<(String, i64)> {
        let active = self.overnight.active.get(conversation_id)?;
        let board = self.core.board(conversation_id).await.ok()?;
        let run = board.runs.get(&active.id)?;
        match run.state {
            OvernightState::Planning => {
                let planning = run.planning.as_ref()?;
                (planning.state == PhaseState::Running && planning.lead.is_some())
                    .then(|| (planning_brief(run), planning.started_at_ms))
            }
            OvernightState::Running => {
                let phase = run.phases.iter().find(|phase| {
                    phase.state == PhaseState::Running && phase.start_commit.is_some()
                })?;
                Some((
                    self.phase_brief(run, phase).await,
                    phase.started_at_ms.unwrap_or(0),
                ))
            }
            _ => None,
        }
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

    /// Everything a phase's lead starts from: the phase as the reviewed plan has it, the
    /// user's Rules and words verbatim, and what the run already settled.
    async fn phase_brief(&self, run: &OvernightRun, phase: &OvernightPhase) -> String {
        let selected: Vec<&OvernightPhase> = run
            .phases
            .iter()
            .filter(|phase| phase.state != PhaseState::Skipped)
            .collect();
        let mut text = format!(
            "[Overnight run \u{201c}{}\u{201d} · Phase {} of {}: {}]\n",
            run.name,
            phase.number,
            selected.len(),
            phase.name
        );
        text.push_str(&format!(
            "The run's goal, in the user's words:\n{}\n\n",
            run.goal
        ));
        text.push_str(&format!(
            "This phase's scope, exactly as the plan says:\n{}\n\n",
            phase.scope
        ));
        text.push_str("Done when (the phase's verifier checks each, by its id, for real):\n");
        for criterion in &phase.done_when {
            text.push_str(&format!("- {}: {}\n", criterion.id, criterion.text));
        }
        if !phase.depends_on.is_empty() {
            text.push_str(&format!(
                "It builds on phase{} {}.\n",
                if phase.depends_on.len() == 1 { "" } else { "s" },
                phase
                    .depends_on
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
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
        let earlier: Vec<String> = run
            .phases
            .iter()
            .filter(|other| other.id != phase.id && other.state != PhaseState::Pending)
            .map(|other| {
                let outcome = match other.state {
                    PhaseState::Verified => format!(
                        "verified at {}",
                        other
                            .verified_commit
                            .as_deref()
                            .map_or("?", |commit| &commit[..commit.len().min(10)])
                    ),
                    PhaseState::Partial => {
                        format!("partial; still missing: {}", other.gaps.join("; "))
                    }
                    PhaseState::Blocked => format!("blocked; it needs: {}", other.gaps.join("; ")),
                    PhaseState::Skipped => "skipped (not selected)".into(),
                    PhaseState::Running | PhaseState::Checking | PhaseState::Pending => {
                        "in progress".into()
                    }
                };
                let summary = other
                    .summary
                    .as_deref()
                    .map(|summary| format!(" Its lead's summary: {summary}"))
                    .unwrap_or_default();
                format!(
                    "- Phase {} · {}: {outcome}.{summary}",
                    other.number, other.name
                )
            })
            .collect();
        if !earlier.is_empty() {
            text.push_str("\nThe other phases so far:\n");
            text.push_str(&earlier.join("\n"));
            text.push('\n');
        }
        if !phase.gaps.is_empty() {
            text.push_str(&format!(
                "\nAn earlier run segment left this phase incomplete; what it still lacked:\n- {}\n",
                phase.gaps.join("\n- ")
            ));
        }
        if let Some(workspace) = &run.workspace {
            text.push_str(&format!(
                "\nThe run's branch is `{}` (from `{}` at {}); this phase starts at {}. Verified so far: {}.\n",
                workspace.branch,
                workspace.base,
                &workspace.base_commit[..workspace.base_commit.len().min(10)],
                phase
                    .start_commit
                    .as_deref()
                    .map_or("its tip", |commit| &commit[..commit.len().min(10)]),
                run.verified_commit
                    .as_deref()
                    .map_or("nothing yet".to_owned(), |commit| commit[..commit.len().min(10)].to_owned())
            ));
        }
        if let Ok(board) = self.core.board(&run.conversation_id).await {
            let waiting: Vec<String> = board
                .waiting
                .values()
                .map(|item| format!("- {}", item.what))
                .collect();
            if !waiting.is_empty() {
                text.push_str(&format!(
                    "\nWaiting on the user (they answer in the morning; don't ask again, work around them):\n{}\n",
                    waiting.join("\n")
                ));
            }
        }
        let words = self.words_since_start(run).await;
        if !words.is_empty() {
            text.push_str(&format!(
                "\nWhat the user wrote since Start, verbatim, oldest first (it supersedes earlier choices; it can't change this phase's scope or settled decisions):\n{words}\n"
            ));
        }
        text.push_str(&format!("\n{}\n", directives_text(run)));
        text
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
        for message in branch
            .iter()
            .filter(|message| message.role == MessageRole::User && message.created_at_ms >= started)
        {
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

    /// `phase_done`: the lead settles its phase, judging the verifier's report. Refused while
    /// any of the phase's work still runs or waits to land.
    pub(crate) async fn phase_done(&self, id: &ConversationId, args: PhaseDone) -> Result<String> {
        let active = self.overnight.active.get(id).ok_or_else(|| {
            Error::Invalid(
                "No overnight run is going in this session: phase_done is only for leading one of its phases.".into(),
            )
        })?;
        let board = self.core.board(id).await?;
        let run = board
            .runs
            .get(&active.id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("overnight run {}", active.id)))?;
        if run.state == OvernightState::Planning {
            return Err(Error::Invalid(
                "This is Phase 0: it writes the plan only. Propose the phases with propose_phases."
                    .into(),
            ));
        }
        let phase = run
            .phases
            .iter()
            .find(|phase| phase.state == PhaseState::Running)
            .cloned()
            .ok_or_else(|| {
                Error::Invalid(
                    "No phase is being led now: the run is ending. Reply with exactly [quiet]."
                        .into(),
                )
            })?;
        let open = unsettled(&board, &run, &phase);
        if !open.is_empty() {
            return Err(Error::Invalid(format!(
                "Phase {} still has work going: {}. Wait for it, land the verifier's work (land_phase) or stop what is no longer needed, then call phase_done again.",
                phase.number,
                open.join(", ")
            )));
        }
        let summary = args.summary.trim().to_owned();
        if summary.is_empty() {
            return Err(Error::Invalid(
                "Say in `summary` what the phase changed, how its verifier checked it and what the review found.".into(),
            ));
        }
        let left: Vec<String> = args
            .left
            .iter()
            .map(|line| line.trim().to_owned())
            .filter(|line| !line.is_empty())
            .collect();
        let state = match args.outcome {
            PhaseOutcome::Done => PhaseState::Verified,
            PhaseOutcome::Partial => PhaseState::Partial,
            PhaseOutcome::Blocked => PhaseState::Blocked,
        };
        if state != PhaseState::Verified && left.is_empty() {
            return Err(Error::Invalid(
                "Say in `left` what is left of the phase, one line each.".into(),
            ));
        }
        let tip = self.run_tip(&run).await?;
        if self
            .settle_phase(&run, &phase.id, state, Some(summary), left, Some(tip))
            .await
            .is_none()
        {
            return Err(Error::Invalid(
                "The run moved on meanwhile; nothing changed. Reply with exactly [quiet].".into(),
            ));
        }
        Ok(format!(
            "Settled phase {} as {}. Brigadier starts the next phase or ends the run itself. Reply with exactly {} now.",
            phase.number,
            match state {
                PhaseState::Verified => "done",
                PhaseState::Blocked => "blocked",
                _ => "partial",
            },
            super::super::prompts::QUIET
        ))
    }

    /// Settles a running phase as `state` (the run branch at `tip` for one done), records the
    /// outcome under "Decided for you" and what a blocked phase needs under "Waiting on you",
    /// and moves the run on. `None` when the run or the phase moved on meanwhile.
    async fn settle_phase(
        &self,
        run: &OvernightRun,
        phase_id: &str,
        state: PhaseState,
        summary: Option<String>,
        gaps: Vec<String>,
        tip: Option<String>,
    ) -> Option<OvernightRun> {
        let now = self
            .change_run_if(run, |now| {
                // The merge point moves only while everything before it is verified.
                let all_verified = now
                    .phases
                    .iter()
                    .filter(|p| p.id != phase_id && p.start_commit.is_some())
                    .all(|p| p.state == PhaseState::Verified);
                let phase = now.phases.iter_mut().find(|p| p.id == phase_id)?;
                if phase.state != PhaseState::Running {
                    return None;
                }
                phase.state = state;
                phase.summary.clone_from(&summary);
                phase.gaps.clone_from(&gaps);
                phase.settled_at_ms = Some(now_ms());
                if state == PhaseState::Verified {
                    phase.verified_commit.clone_from(&tip);
                    if all_verified {
                        now.verified_commit.clone_from(&tip);
                    }
                }
                Some(())
            })
            .await?;
        let phase = now.phase(phase_id)?.clone();
        if state == PhaseState::Blocked {
            for gap in &gaps {
                self.run_waits(
                    &now,
                    phase.request_id.clone(),
                    None,
                    &format!("Phase {}: {gap}", phase.number),
                )
                .await;
            }
        }
        let (what, why) = match state {
            PhaseState::Verified => (
                format!(
                    "Verified phase {} \u{201c}{}\u{201d}",
                    phase.number, phase.name
                ),
                "Its verifier checked every \"done when\" and its work landed.".to_owned(),
            ),
            _ => (
                format!(
                    "Settled phase {} \u{201c}{}\u{201d} as {}",
                    phase.number,
                    phase.name,
                    if state == PhaseState::Blocked {
                        "blocked"
                    } else {
                        "partial"
                    }
                ),
                gaps.first().map_or_else(
                    || "Part of it is left.".to_owned(),
                    |gap| format!("Left: {gap}"),
                ),
            ),
        };
        self.phase_outcome_decided(&now, phase_id, phase.request_id.clone(), what, why)
            .await;
        if state == PhaseState::Verified {
            self.phase_verified_waiting(&now.conversation_id, &now.id, phase.number)
                .await;
        }
        self.advance_soon(&now.conversation_id, &now.id);
        Some(now)
    }

    /// The lead stopped answering with its phase open: the phase settles as partial, and what
    /// of it still runs stops (its work is kept with its task).
    async fn settle_unanswered(&self, run: &OvernightRun, phase: &OvernightPhase, board: &Board) {
        let open: Vec<_> = board
            .tasks
            .values()
            .filter(|task| of_phase(task, run, phase) && !task.state.is_final())
            .map(|task| task.id.clone())
            .collect();
        for task in open {
            if let Err(err) = Box::pin(self.stop_task(task.clone())).await {
                tracing::warn!(task = %task, error = %err, "could not stop a task of an unsettled phase");
            }
            self.release_run_task(&task);
        }
        let tip = self.run_tip(run).await.ok();
        self.settle_phase(
            run,
            &phase.id,
            PhaseState::Partial,
            None,
            vec!["Its lead stopped before it settled the phase.".into()],
            tip,
        )
        .await;
    }

    /// `propose_phases`: Phase 0's plan, judged against the goal.
    pub(crate) async fn propose_phases(
        &self,
        id: &ConversationId,
        args: ProposePhases,
    ) -> Result<String> {
        let active = self.overnight.active.get(id).filter(|active| active.planning).ok_or_else(|| {
            Error::Invalid(
                "propose_phases is only for Phase 0 of an overnight run (a goal without a plan).".into(),
            )
        })?;
        if args.phases.is_empty() {
            return Err(Error::Invalid("a plan needs at least one phase".into()));
        }
        for (index, phase) in args.phases.iter().enumerate() {
            if phase.name.trim().is_empty() || phase.scope.trim().is_empty() {
                return Err(Error::Invalid(format!(
                    "phase {} needs a name and its scope",
                    index + 1
                )));
            }
            if phase.done_when.is_empty() {
                return Err(Error::Invalid(format!(
                    "phase {} (\u{201c}{}\u{201d}) needs its \"done when\" criteria",
                    index + 1,
                    phase.name
                )));
            }
            if let Some(bad) = phase
                .depends_on
                .iter()
                .find(|dep| **dep == 0 || **dep as usize > index)
            {
                return Err(Error::Invalid(format!(
                    "phase {} builds on phase {bad}, which doesn't come before it",
                    index + 1
                )));
            }
        }
        let steps = args
            .phases
            .iter()
            .enumerate()
            .map(|(index, phase)| crate::work::PlanStep {
                title: format!("Phase {} · {}", index + 1, phase.name.trim()),
                detail: Some(phase_detail(phase)),
                ..Default::default()
            })
            .collect();
        let plan = self.record_phases(id, args.name.clone(), steps).await?;
        let plan_id = Some(plan.id.clone());
        let proposed: Vec<crate::overnight::ProposedPhase> = args
            .phases
            .iter()
            .enumerate()
            .map(|(index, phase)| crate::overnight::ProposedPhase {
                number: Some(index as u32 + 1),
                name: phase.name.trim().to_owned(),
                scope: phase.scope.clone(),
                done_when: phase.done_when.clone(),
                depends_on: phase.depends_on.clone(),
            })
            .collect();
        let board = self.core.board(id).await?;
        let run = board
            .runs
            .get(&active.id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("overnight run {}", active.id)))?;
        if self
            .adopt_plan(&run, args.name.trim(), proposed, plan_id)
            .await
            .is_none()
        {
            return Err(Error::Invalid(
                "The run moved on meanwhile; nothing changed. Reply with exactly [quiet].".into(),
            ));
        }
        Ok(format!(
            "Recorded the plan. Phase 1 starts now, with a fresh briefing. Reply with exactly {}.",
            super::super::prompts::QUIET
        ))
    }

    /// Phase 0's phases become the run's plan, and the run goes on with its first phase.
    async fn adopt_plan(
        &self,
        run: &OvernightRun,
        name: &str,
        proposed: Vec<crate::overnight::ProposedPhase>,
        plan_id: Option<crate::model::CardId>,
    ) -> Option<OvernightRun> {
        let phases: Vec<OvernightPhase> = proposed
            .iter()
            .enumerate()
            .map(|(index, phase)| {
                OvernightPhase::new(
                    phase.number.unwrap_or(index as u32 + 1),
                    &phase.name,
                    &phase.scope,
                    &phase.done_when,
                    &phase.depends_on,
                )
            })
            .collect();
        let now = self
            .change_run_if(run, |now| {
                if now.state != OvernightState::Planning {
                    return None;
                }
                let planning = now.planning.as_mut()?;
                if plan_id.is_some() {
                    planning.plan_id.clone_from(&plan_id);
                }
                planning.proposed.clone_from(&proposed);
                planning.state = PhaseState::Verified;
                planning.settled_at_ms = Some(now_ms());
                if !name.is_empty() {
                    now.name = name.to_owned();
                }
                now.phases = phases.clone();
                let chosen = now.directives.clone();
                for phase in &mut now.phases {
                    if !super::selects(&chosen, phase.number) {
                        phase.state = PhaseState::Skipped;
                    }
                }
                now.state = OvernightState::Running;
                Some(())
            })
            .await?;
        self.run_decided(
            &now,
            Some(PLANNING_PHASE),
            Some(planning_request(&now)),
            format!(
                "Planned \u{201c}{}\u{201d} in {}",
                now.name,
                plural_phases(now.phases.len())
            ),
            "Phase 0 read the goal and the code; the phases follow the goal.".into(),
        )
        .await;
        self.advance_soon(&now.conversation_id, &now.id);
        Some(now)
    }

    /// An older run caught while Phase 0's plan was being judged: the plan it proposed is the
    /// run's plan now (or, with none, Phase 0 goes on).
    pub(crate) async fn resume_judged_plan(&self, run: &OvernightRun) {
        let Some(planning) = run.planning.clone() else {
            return;
        };
        if planning.proposed.is_empty() {
            let resumed = self
                .change_run_if(run, |now| {
                    let planning = now.planning.as_mut()?;
                    planning.state = PhaseState::Running;
                    planning.nudges = 0;
                    Some(())
                })
                .await;
            if resumed.is_some() {
                self.lead_turn_ended(&run.conversation_id);
            }
            return;
        }
        self.adopt_plan(run, "", planning.proposed, None).await;
    }

    /// Phase 0 couldn't produce a plan the run may follow: the run ends, saying why.
    pub(crate) async fn planning_blocked(&self, run: &OvernightRun, asks: Vec<String>) {
        let Some(now) = self
            .change_run_if(run, |now| {
                let planning = now.planning.as_mut()?;
                planning.state = PhaseState::Blocked;
                planning.settled_at_ms = Some(now_ms());
                planning.gaps = asks.clone();
                now.state = OvernightState::WindingDown;
                now.stop = Some(StopReason::Blocked {
                    phase_id: PLANNING_PHASE.into(),
                });
                Some(())
            })
            .await
        else {
            return;
        };
        let request = now.planning.as_ref().map(|p| p.request_id.clone());
        for ask in &asks {
            self.run_waits(&now, request.clone(), None, ask).await;
        }
        self.advance_soon(&now.conversation_id, &now.id);
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

    /// [`Self::run_decided`] for a phase that verified or settled: the run's card shows it.
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

    /// After an orchestrator turn: a lead whose turn ended with its phase still open and
    /// nothing of it running is reminded to finish it, twice; then its phase is checked as it
    /// is (or Phase 0, which proposed nothing, ends blocked).
    pub(crate) fn lead_turn_ended(&self, conversation_id: &ConversationId) {
        if self.overnight.active.get(conversation_id).is_none() {
            return;
        }
        let (manager, id) = (self.arc(), conversation_id.clone());
        self.spawn(async move {
            tokio::time::sleep(NUDGE_GRACE).await;
            manager.nudge_lead(&id).await;
        });
    }

    async fn nudge_lead(&self, id: &ConversationId) {
        let Some(active) = self.overnight.active.get(id) else {
            return;
        };
        if let Ok(conv) = self.conv(id)
            && conv.turn_running().await
        {
            return;
        }
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        let Some(run) = board.runs.get(&active.id).cloned() else {
            return;
        };
        // The phase being led and its request, unless something of it still works.
        let (request, label, nudges, open) = match run.state {
            OvernightState::Planning => {
                let Some(planning) = &run.planning else {
                    return;
                };
                if planning.state != PhaseState::Running {
                    return;
                }
                (
                    planning.request_id.clone(),
                    "phase 0".to_owned(),
                    planning.nudges,
                    Vec::new(),
                )
            }
            OvernightState::Running => {
                let Some(phase) = run.phases.iter().find(|p| p.state == PhaseState::Running) else {
                    return;
                };
                let Some(request) = phase.request_id.clone() else {
                    return;
                };
                (
                    request,
                    format!("phase {}", phase.number),
                    phase.nudges,
                    unsettled(&board, &run, phase),
                )
            }
            _ => return,
        };
        let working = board
            .requests
            .get(&request)
            .is_some_and(|r| r.state == RequestState::Working);
        // Tasks running or a turn queued: the lead hears from them, no reminder needed.
        if working
            && board.tasks.values().any(|task| {
                task.request_id.as_deref() == Some(request.as_str())
                    && !task.state.is_final()
                    && task.state != TaskState::Reported
            })
        {
            return;
        }
        if nudges >= NUDGES {
            if run.state == OvernightState::Planning {
                self.planning_blocked(&run, vec![
                    "Phase 0 ended without proposing a plan. Give the run a plan (or a narrower goal), then Continue.".into(),
                ])
                .await;
            } else if let Some(phase) = run.phases.iter().find(|p| p.state == PhaseState::Running) {
                self.settle_unanswered(&run, phase, &board).await;
            }
            return;
        }
        let bumped = self
            .change_run_if(&run, |now| {
                match now.state {
                    OvernightState::Planning => now.planning.as_mut()?.nudges += 1,
                    _ => {
                        now.phases
                            .iter_mut()
                            .find(|p| p.state == PhaseState::Running)?
                            .nudges += 1
                    }
                }
                Some(())
            })
            .await;
        if bumped.is_none() {
            return;
        }
        let text = if run.state == OvernightState::Planning {
            "[overnight · phase 0] The plan isn't proposed yet. Propose the phases with propose_phases now (nobody can answer questions before the morning: decide what the goal and Rules settle, note what only the user can decide with note_for_user, and plan around it).".to_owned()
        } else if open.is_empty() {
            format!(
                "[overnight · {label}] Nothing of this phase runs now. Delegate what remains of it, or settle it with phase_done (done, partial or blocked) from its verifier's report. Nobody can answer questions before the morning: note what only the user can do with note_for_user (kind waiting) and finish the rest."
            )
        } else {
            format!(
                "[overnight · {label}] This phase waits on you: {}. Land the verifier's work (land_phase), send back or stop the rest, then go on; settle the phase with phase_done once all of it has landed or ended.",
                open.join(", ")
            )
        };
        self.deliver_for(
            id,
            Envelope {
                kind: InjectionKind::Reminder,
                label: format!("{label} reminder"),
                task_id: None,
                text,
            },
            Some(request),
        )
        .await;
    }

    /// The run ended: the orchestrator's next turn resumes without the run's instructions.
    pub(crate) async fn run_finished(&self, run: &OvernightRun) {
        tracing::info!(run = %run.id, stop = ?run.stop, "overnight run finished");
        self.retire_orchestrator(&run.conversation_id).await;
        // The report answers the run's requests: they settle for good, and nothing owed to
        // them (a reminder, a late message) starts a turn after the run.
        if let Ok(conv) = self.conv(&run.conversation_id) {
            conv.forget_run_requests(&format!("run-{}-", run.id.short()))
                .await;
        }
        self.settle_requests(&run.conversation_id).await;
    }
}

/// Whether `task` works on `phase` of `run`.
fn of_phase(task: &crate::work::Task, run: &OvernightRun, phase: &OvernightPhase) -> bool {
    task.run.as_ref().is_some_and(|context| {
        context.run_id == run.id && context.phase_id.as_deref() == Some(phase.id.as_str())
    })
}

/// What of `phase` still runs or waits to be decided: its tasks that haven't landed or ended.
fn unsettled(board: &Board, run: &OvernightRun, phase: &OvernightPhase) -> Vec<String> {
    let mut open: Vec<_> = board
        .tasks
        .values()
        .filter(|task| of_phase(task, run, phase) && !task.state.is_final())
        .collect();
    open.sort_by_key(|task| task.number);
    open.iter()
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
        .collect()
}

/// The next step of the plan: the first selected pending phase whose dependencies are
/// verified, an ending, or nothing yet.
fn pick_next(run: &OvernightRun, verified_before: &[u32]) -> Pick {
    let settled = |phase: &OvernightPhase| phase.is_settled() && phase.state != PhaseState::Skipped;
    match &run.directives.stop_after {
        Some(StopAfter::Phase { number })
            if run.phases.iter().any(|p| p.number == *number && settled(p)) =>
        {
            return Pick::End(StopReason::StopDirective);
        }
        Some(StopAfter::Current { phase_id })
            if run.phases.iter().any(|p| &p.id == phase_id && settled(p)) =>
        {
            return Pick::End(StopReason::StopDirective);
        }
        _ => {}
    }
    if run
        .phases
        .iter()
        .any(|phase| matches!(phase.state, PhaseState::Running | PhaseState::Checking))
    {
        return Pick::Wait;
    }
    for phase in &run.phases {
        if phase.state != PhaseState::Pending || !run.selects(phase.number) {
            continue;
        }
        for dep in &phase.depends_on {
            if verified_before.contains(dep) {
                continue;
            }
            match run.phases.iter().find(|p| p.number == *dep) {
                Some(dep) if dep.state == PhaseState::Verified => {}
                // Work it needs wasn't finished: the run stops here, and says so.
                Some(dep) => {
                    return Pick::End(StopReason::Blocked {
                        phase_id: dep.id.clone(),
                    });
                }
                None => {}
            }
        }
        return Pick::Phase(phase.id.clone());
    }
    Pick::End(StopReason::Done)
}

/// Everything Phase 0's lead starts from: the goal, the Rules and the restrictions.
fn planning_brief(run: &OvernightRun) -> String {
    format!(
        "[Overnight run \u{201c}{name}\u{201d} · Phase 0: write the plan]\nThe user gave a goal without a plan. Your job in this phase is the plan only: its phases, each with its exact scope, \"done when\" criteria anyone can check by running something or reading the code, and the phases it builds on. Split into phases only what must happen one after another; one phase is fine. Scouts and research may look around the repository first; nothing is changed in Phase 0. Propose the phases with propose_phases; the run follows them, so put in only what the goal asks for: no invented scope, nothing the Rules exclude.\n\nThe goal, in the user's words:\n{goal}\n\nRules and settled decisions, verbatim:\n{rules}\n\n{directives}",
        name = run.name,
        goal = run.words,
        rules = if run.rules.trim().is_empty() {
            "(none given)".into()
        } else {
            run.rules.clone()
        },
        directives = directives_text(run),
    )
}

/// The restrictions Brigadier enforces, as the lead should know them.
fn directives_text(run: &OvernightRun) -> String {
    let mut lines = vec!["Restrictions Brigadier enforces (you don't need to):".to_owned()];
    match &run.directives.deadline {
        Deadline::At { time } => lines.push(format!(
            "- The report is due at {} {} ({}); Brigadier stops new work in time to finish cleanly.",
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
        Some(StopAfter::Phase { number }) => {
            lines.push(format!("- The run stops after phase {number}."))
        }
        Some(StopAfter::Current { phase_id }) => {
            lines.push(format!("- The run stops after {phase_id}."))
        }
        None => {}
    }
    lines.join("\n")
}

/// A proposed phase as its plan card step shows it.
fn phase_detail(phase: &crate::tools::PhaseInput) -> String {
    let mut detail = phase.scope.trim().to_owned();
    detail.push_str("\nDone when:");
    for criterion in &phase.done_when {
        detail.push_str(&format!("\n- {criterion}"));
    }
    if !phase.depends_on.is_empty() {
        detail.push_str(&format!(
            "\nBuilds on phase{} {}.",
            if phase.depends_on.len() == 1 { "" } else { "s" },
            phase
                .depends_on
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    detail
}

/// Phase 0's request.
fn planning_request(run: &OvernightRun) -> String {
    run.planning.as_ref().map_or_else(
        || format!("run-{}-phase-0", run.id.short()),
        |planning| planning.request_id.clone(),
    )
}

/// "1 phase", "3 phases".
fn plural_phases(count: usize) -> String {
    format!("{count} phase{}", if count == 1 { "" } else { "s" })
}
