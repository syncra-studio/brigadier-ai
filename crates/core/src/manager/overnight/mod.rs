//! Overnight runs (PLAN.md §10): the user's commands on a run and its records.
//!
//! A run is proposed from the user's words (and the plan the orchestrator read from them or
//! their files), shown with one Start, and from Start on owned by Brigadier until its report.
//! Every command carries an id, so a repeated one changes nothing, and Start names the
//! revision the user saw, so a proposal that changed meanwhile isn't started unseen. Only the
//! app sends these commands; worker and orchestrator grants can't.

pub(crate) mod admission;
mod conductor;
pub mod directives;
pub(crate) mod git_guard;
mod messages;
mod phase_gates;
pub(crate) mod policy;
mod recovery;
mod report;
mod wind_down;
mod workspace;

use std::path::{Path, PathBuf};

use super::{SessionManager, blocking};
use crate::board::Board;
use crate::model::{ConversationId, DomainEvent, OvernightRunId, Setup};
use crate::overnight::{
    AppliedCommand, Deadline, Directives, OvernightPhase, OvernightRun, OvernightState, PhaseState,
    ProposedPlan, SourceSnapshot, StopReason,
};
use crate::{Error, Result, now_ms};
use directives::{Clock, PhaseInfo};

/// Commands remembered per run, to recognize a repeated one.
const COMMANDS_KEPT: usize = 32;
/// A source file larger than this is named but not kept.
const MAX_SOURCE_BYTES: u64 = 4 * 1024 * 1024;
/// The longest wind-down: handoffs, landing what passed and the report.
const WIND_DOWN_MAX_MS: i64 = 20 * 60_000;

/// What the manager keeps for overnight runs.
#[derive(Default)]
pub(crate) struct Runs {
    /// Held while a run is read, changed and recorded, so its transitions happen one at a
    /// time. Never held across model work. Explicit run Merge keeps it during its bounded
    /// git effect, so Start/Continue cannot race that approval.
    pub(crate) changes: tokio::sync::Mutex<()>,
    /// Serializes report reconciliation across retries.
    reporting: tokio::sync::Mutex<()>,
    /// Each session's active run, for task code that can't read the board.
    pub(crate) active: policy::ActiveRuns,
    /// Each run's executing tasks under its worker cap, and the build lease.
    pub(crate) admission: admission::Admission,
    /// Runs whose clean ending is under way (it runs once).
    winding: std::sync::Mutex<std::collections::HashSet<OvernightRunId>>,
    /// The generic recovery is ending the old daemon's tasks: their missing results are not
    /// verdicts on a run's checks (the round starts again afterwards).
    pub(crate) recovering: std::sync::atomic::AtomicBool,
}

impl SessionManager {
    /// Proposes a run from the user's `words` and the plan read from them (`None`: a bare
    /// goal, whose plan Phase 0 writes). Replaces an earlier proposal of the session that
    /// wasn't started. Nothing runs until Start.
    pub async fn propose_overnight(
        &self,
        conversation_id: ConversationId,
        command_id: String,
        words: String,
        plan: Option<ProposedPlan>,
    ) -> Result<OvernightRun> {
        let conversation = self.core.conversation(&conversation_id)?;
        let Some(Setup::Session { repo, .. }) = &conversation.setup else {
            return Err(Error::Invalid(
                "Overnight runs work in a session with a repository.".into(),
            ));
        };
        let repo = PathBuf::from(repo);
        if words.trim().is_empty() {
            return Err(Error::Invalid("Say what to work on.".into()));
        }
        let plan = plan.unwrap_or_default();
        let sources = self.snapshot_sources(&repo, &plan).await;
        let _held = self.overnight.changes.lock().await;
        let board = self.core.board(&conversation_id).await?;
        if let Some(run) = board
            .runs
            .values()
            .find(|run| run.commands.iter().any(|command| command.id == command_id))
        {
            return Ok(run.clone());
        }
        if board.active_run().is_some() {
            return Err(Error::Invalid(
                "This session already has an overnight run going. Stop it first, or tell it what to change.".into(),
            ));
        }
        let clock = Clock::system();
        let phases = phases_of(&plan);
        let (directives, mut problems) =
            directives::steer(&Directives::default(), &words, &clock, None);
        let numbered = (!phases.is_empty()).then(|| infos(&phases));
        problems.extend(directives::check(
            &directives,
            numbered.as_deref(),
            &[],
            &clock,
            true,
        ));
        let now = now_ms();
        let run = OvernightRun {
            id: OvernightRunId::generate(),
            conversation_id: conversation_id.clone(),
            segment: 1,
            predecessor: None,
            plan_id: None,
            name: name_of(&plan, &words),
            goal: plan.goal.clone().unwrap_or_else(|| words.clone()),
            rules: plan.rules.clone().unwrap_or_default(),
            words,
            sources,
            phases,
            directives,
            problems,
            revision: 1,
            generation: 0,
            state: OvernightState::Proposed,
            wind_down_at_ms: None,
            workspace: None,
            planning: None,
            verified_commit: None,
            gaps: Vec::new(),
            obstacles: Vec::new(),
            report_message_id: None,
            report_outcome: None,
            merged: None,
            notification: None,
            stop: None,
            commands: vec![AppliedCommand {
                id: command_id,
                at_ms: now,
            }],
            created_at_ms: now,
            started_at_ms: None,
            finished_at_ms: None,
        };
        let mut events: Vec<DomainEvent> = board
            .runs
            .values()
            .filter(|old| old.state == OvernightState::Proposed)
            .map(|old| {
                let mut old = old.clone();
                old.state = OvernightState::Superseded;
                old.revision += 1;
                DomainEvent::OvernightUpdated { run: Box::new(old) }
            })
            .collect();
        events.push(DomainEvent::OvernightUpdated {
            run: Box::new(run.clone()),
        });
        self.record_runs(&conversation_id, events).await?;
        Ok(run)
    }

    /// The user's Start on a proposal: the single point where a run begins.
    pub async fn start_overnight(
        &self,
        conversation_id: ConversationId,
        run_id: OvernightRunId,
        command_id: String,
        revision: u32,
    ) -> Result<OvernightRun> {
        let applied = command_id.clone();
        let run = self
            .change_run(&conversation_id, &run_id, command_id, |run, board| {
                if run.state != OvernightState::Proposed {
                    return Err(Error::Invalid(match run.state {
                        OvernightState::Superseded => {
                            "A newer proposal replaced this one; start that one.".into()
                        }
                        _ => "This run has already started.".into(),
                    }));
                }
                if run.revision != revision {
                    return Err(Error::Invalid(
                        "The plan changed since you looked at it. Check it and press Start again."
                            .into(),
                    ));
                }
                if board.active_run().is_some() {
                    return Err(Error::Invalid(
                        "This session already has an overnight run going.".into(),
                    ));
                }
                let clock = Clock::system();
                let verified = verified_numbers(board, run);
                let numbered = (!run.phases.is_empty()).then(|| infos(&run.phases));
                let problems = directives::check(
                    &run.directives,
                    numbered.as_deref(),
                    &verified,
                    &clock,
                    true,
                );
                if let Some(problem) = problems.first().or(run.problems.first()) {
                    return Err(Error::Invalid(problem.message.clone()));
                }
                let now = clock.now.as_millisecond();
                if let Deadline::For { minutes } = run.directives.deadline {
                    let time = directives::resolve_duration(minutes, &clock)
                        .ok_or_else(|| Error::Invalid("That duration is out of range.".into()))?;
                    run.directives.deadline = Deadline::At { time };
                }
                run.wind_down_at_ms = match &run.directives.deadline {
                    Deadline::At { time } => Some(wind_down_at(now, time.at_ms)),
                    Deadline::UntilDone | Deadline::For { .. } => None,
                };
                let chosen = run.directives.clone();
                for phase in &mut run.phases {
                    if phase.state == PhaseState::Pending && !selects(&chosen, phase.number) {
                        phase.state = PhaseState::Skipped;
                    }
                }
                run.state = OvernightState::Preparing;
                run.generation += 1;
                run.started_at_ms = Some(now);
                Ok(())
            })
            .await?;
        // Only the Start that started it prepares it (a repeated Start changes nothing).
        let started = run
            .commands
            .last()
            .is_some_and(|command| command.id == applied);
        if run.state == OvernightState::Preparing && started {
            self.title_session_after(&run).await;
            let manager = self.arc();
            let preparing = run.clone();
            self.spawn(async move { manager.prepare_run(preparing).await });
        }
        Ok(run)
    }

    /// Names the session after the run it starts, unless the user named it themselves: a
    /// title Brigadier took from the user's words ("/overnight Make overnight runs…") gives
    /// way to the plan's name.
    async fn title_session_after(&self, run: &OvernightRun) {
        let Ok(conversation) = self.core.conversation(&run.conversation_id) else {
            return;
        };
        if conversation.title == run.name || !words_title(&conversation.title, &run.words) {
            return;
        }
        if let Err(err) = self
            .core
            .rename_conversation(run.conversation_id.clone(), run.name.clone())
            .await
        {
            tracing::warn!(run = %run.id, error = %err, "could not name the session after its run");
        }
    }

    /// The user's Stop: a proposal is dropped; a started run winds down now, the same clean
    /// ending as its deadline. Stopping again changes nothing.
    pub async fn stop_overnight(
        &self,
        conversation_id: ConversationId,
        run_id: OvernightRunId,
        command_id: String,
    ) -> Result<OvernightRun> {
        let stopped = self.stop_run(&conversation_id, &run_id, command_id).await?;
        self.advance_soon(&conversation_id, &run_id);
        Ok(stopped)
    }

    async fn stop_run(
        &self,
        conversation_id: &ConversationId,
        run_id: &OvernightRunId,
        command_id: String,
    ) -> Result<OvernightRun> {
        self.change_run(conversation_id, run_id, command_id, |run, _| {
            match run.state {
                OvernightState::Proposed => {
                    run.state = OvernightState::Superseded;
                    run.revision += 1;
                }
                OvernightState::Preparing
                | OvernightState::Planning
                | OvernightState::Running
                | OvernightState::PhaseGate
                | OvernightState::WaitingQuota => {
                    run.state = OvernightState::WindingDown;
                    run.stop = Some(StopReason::Stopped);
                }
                // Already ending: Stop is settled.
                OvernightState::WindingDown
                | OvernightState::Reporting
                | OvernightState::Finished
                | OvernightState::Superseded => {}
            }
            Ok(())
        })
        .await
    }

    /// The user's words change a run's restrictions ("until 09:00 instead", "skip phase 4",
    /// "stop after this phase", "max 1 worker"). Before Start they revise the proposal;
    /// during a run they apply at the next boundary. A conflicting change leaves the earlier
    /// restriction in place and lists the problem.
    pub async fn steer_overnight(
        &self,
        conversation_id: ConversationId,
        run_id: OvernightRunId,
        command_id: String,
        words: String,
    ) -> Result<OvernightRun> {
        let steered = self
            .steer_run(&conversation_id, &run_id, command_id, words)
            .await?;
        // A new restriction applies at the next boundary (a stop directive already reached).
        if steered.state.is_active() {
            self.advance_soon(&conversation_id, &run_id);
        }
        Ok(steered)
    }

    async fn steer_run(
        &self,
        conversation_id: &ConversationId,
        run_id: &OvernightRunId,
        command_id: String,
        words: String,
    ) -> Result<OvernightRun> {
        self.change_run(conversation_id, run_id, command_id, |run, board| {
            if run.state.is_final() || run.state == OvernightState::Reporting {
                return Err(Error::Invalid(
                    "This run has ended. Say \"continue\" to plan the rest.".into(),
                ));
            }
            let clock = Clock::system();
            let running = run
                .phases
                .iter()
                .find(|phase| matches!(phase.state, PhaseState::Running | PhaseState::Checking))
                .map(|phase| phase.id.clone());
            let (next, mut problems) =
                directives::steer(&run.directives, &words, &clock, running.as_deref());
            let verified = verified_numbers(board, run);
            let numbered = (!run.phases.is_empty()).then(|| infos(&run.phases));
            problems.extend(directives::check(
                &next,
                numbered.as_deref(),
                &verified,
                &clock,
                run.state == OvernightState::Proposed,
            ));
            let changed = next != run.directives;
            let new_words = !words.trim().is_empty();
            if new_words {
                run.words.push_str("\n\n");
                run.words.push_str(&words);
            }
            if run.state == OvernightState::Proposed {
                // Before Start, a problem blocks Start until reworded; the words still apply.
                run.directives = next;
                run.problems = problems;
                if changed || new_words {
                    run.revision += 1;
                }
            } else if problems.is_empty() {
                if let (Deadline::At { time }, Some(started)) = (&next.deadline, run.started_at_ms)
                    && next.deadline != run.directives.deadline
                {
                    run.wind_down_at_ms = Some(wind_down_at(started, time.at_ms));
                }
                if let Deadline::For { minutes } = next.deadline {
                    let time = directives::resolve_duration(minutes, &clock)
                        .ok_or_else(|| Error::Invalid("That duration is out of range.".into()))?;
                    run.wind_down_at_ms =
                        Some(wind_down_at(clock.now.as_millisecond(), time.at_ms));
                    run.directives = Directives {
                        deadline: Deadline::At { time },
                        ..next
                    };
                } else {
                    run.directives = next;
                }
                // "until done instead" drops the earlier cutoff.
                if run.directives.deadline == Deadline::UntilDone {
                    run.wind_down_at_ms = None;
                }
                // A changed selection: phases not started yet follow it, both ways.
                let chosen = run.directives.clone();
                for phase in &mut run.phases {
                    match phase.state {
                        PhaseState::Pending if !selects(&chosen, phase.number) => {
                            phase.state = PhaseState::Skipped;
                        }
                        PhaseState::Skipped
                            if phase.start_commit.is_none() && selects(&chosen, phase.number) =>
                        {
                            phase.state = PhaseState::Pending;
                        }
                        _ => {}
                    }
                }
                run.problems.clear();
                if changed {
                    run.revision += 1;
                }
            } else {
                // During a run an unclear change waits for the user; the earlier, stricter
                // restriction stays meanwhile.
                run.problems = problems;
            }
            Ok(())
        })
        .await
    }

    /// Continue: proposes the next segment of a finished run with the phases it didn't
    /// verify, on the same branch, with a new deadline from `words` (or until done).
    pub async fn continue_overnight(
        &self,
        conversation_id: ConversationId,
        run_id: OvernightRunId,
        command_id: String,
        words: String,
    ) -> Result<OvernightRun> {
        let _held = self.overnight.changes.lock().await;
        let board = self.core.board(&conversation_id).await?;
        if let Some(run) = board
            .runs
            .values()
            .find(|run| run.commands.iter().any(|command| command.id == command_id))
        {
            return Ok(run.clone());
        }
        let previous = board
            .runs
            .get(&run_id)
            .ok_or_else(|| Error::NotFound(format!("overnight run {run_id}")))?;
        if previous.state != OvernightState::Finished {
            return Err(Error::Invalid("This run hasn't finished yet.".into()));
        }
        if board.active_run().is_some() {
            return Err(Error::Invalid(
                "This session already has an overnight run going.".into(),
            ));
        }
        if board
            .runs
            .values()
            .any(|run| run.predecessor.as_ref() == Some(&run_id) && !run.state.is_final())
        {
            return Err(Error::Invalid(
                "This run is already being continued.".into(),
            ));
        }
        let phases: Vec<OvernightPhase> = previous
            .phases
            .iter()
            .cloned()
            .map(|phase| {
                if phase.state == PhaseState::Verified {
                    return phase;
                }
                // Worked again from the start, with the same criteria; what it lacked stays
                // in view for its next lead.
                OvernightPhase {
                    done_when: phase.done_when.clone(),
                    gaps: phase.gaps.clone(),
                    summary: phase.summary.clone(),
                    ..OvernightPhase::new(
                        phase.number,
                        &phase.name,
                        &phase.scope,
                        &[],
                        &phase.depends_on,
                    )
                }
            })
            .collect();
        if !phases.is_empty()
            && phases
                .iter()
                .all(|phase| phase.state == PhaseState::Verified)
        {
            return Err(Error::Invalid(
                "Every phase is verified; there is nothing left to continue.".into(),
            ));
        }
        let clock = Clock::system();
        // The worker cap carries over; the deadline and phase choices are said anew.
        let carried = Directives {
            max_workers: previous.directives.max_workers,
            ..Directives::default()
        };
        let (directives, mut problems) = directives::steer(&carried, &words, &clock, None);
        let verified = verified_numbers(&board, previous);
        let numbered = (!phases.is_empty()).then(|| infos(&phases));
        problems.extend(directives::check(
            &directives,
            numbered.as_deref(),
            &verified,
            &clock,
            true,
        ));
        let now = now_ms();
        let run = OvernightRun {
            id: OvernightRunId::generate(),
            segment: previous.segment + 1,
            predecessor: Some(previous.id.clone()),
            words: if words.trim().is_empty() {
                previous.words.clone()
            } else {
                format!("{}\n\n{words}", previous.words)
            },
            phases,
            directives,
            problems,
            revision: 1,
            generation: 0,
            state: OvernightState::Proposed,
            wind_down_at_ms: None,
            planning: None,
            gaps: Vec::new(),
            obstacles: Vec::new(),
            report_message_id: None,
            report_outcome: None,
            notification: None,
            stop: None,
            commands: vec![AppliedCommand {
                id: command_id,
                at_ms: now,
            }],
            created_at_ms: now,
            started_at_ms: None,
            finished_at_ms: None,
            ..previous.clone()
        };
        self.record_runs(
            &conversation_id,
            vec![DomainEvent::OvernightUpdated {
                run: Box::new(run.clone()),
            }],
        )
        .await?;
        Ok(run)
    }

    /// Reads, changes and records a run under the run lock. A command already applied
    /// returns the run as it is.
    async fn change_run(
        &self,
        conversation_id: &ConversationId,
        run_id: &OvernightRunId,
        command_id: String,
        change: impl FnOnce(&mut OvernightRun, &Board) -> Result<()>,
    ) -> Result<OvernightRun> {
        let _held = self.overnight.changes.lock().await;
        let board = self.core.board(conversation_id).await?;
        let before = board
            .runs
            .get(run_id)
            .ok_or_else(|| Error::NotFound(format!("overnight run {run_id}")))?;
        if before
            .commands
            .iter()
            .any(|command| command.id == command_id)
        {
            return Ok(before.clone());
        }
        let mut run = before.clone();
        change(&mut run, &board)?;
        run.commands.push(AppliedCommand {
            id: command_id,
            at_ms: now_ms(),
        });
        let excess = run.commands.len().saturating_sub(COMMANDS_KEPT);
        run.commands.drain(..excess);
        self.record_runs(
            conversation_id,
            vec![DomainEvent::OvernightUpdated {
                run: Box::new(run.clone()),
            }],
        )
        .await?;
        Ok(run)
    }

    /// Records run events and keeps the active-run view in step with them.
    pub(crate) async fn record_runs(
        &self,
        conversation_id: &ConversationId,
        events: Vec<DomainEvent>,
    ) -> Result<()> {
        self.core
            .record_conversation(conversation_id, events.clone())
            .await?;
        for event in &events {
            if let DomainEvent::OvernightUpdated { run } = event {
                self.overnight.active.note(run);
            }
        }
        Ok(())
    }

    /// After a restart: each session's active run, from its board.
    pub(crate) async fn recover_active_runs(&self) {
        self.overnight
            .recovering
            .store(true, std::sync::atomic::Ordering::Release);
        for conversation in self.core.catalog().conversations {
            if !matches!(conversation.setup, Some(Setup::Session { .. })) {
                continue;
            }
            if let Ok(board) = self.core.board(&conversation.id).await
                && let Some(run) = board.active_run()
            {
                self.overnight.active.note(run);
                // Interrupted while its branch and worktree were being made.
                if run.state == OvernightState::Preparing {
                    let manager = self.arc();
                    let run = run.clone();
                    self.spawn(async move { manager.prepare_run(run).await });
                }
            }
        }
    }

    /// Start's first effect: the run's branch and worktree. A run that can't have them ends
    /// before any work starts.
    async fn prepare_run(&self, run: OvernightRun) {
        let prepared = self.prepare_run_workspace(&run).await;
        let _held = self.overnight.changes.lock().await;
        let Ok(board) = self.core.board(&run.conversation_id).await else {
            return;
        };
        let Some(now) = board.runs.get(&run.id) else {
            return;
        };
        // Stopped or restarted meanwhile: this result is history.
        if now.generation != run.generation || now.state != OvernightState::Preparing {
            return;
        }
        let mut now = now.clone();
        match prepared {
            Ok(workspace) => now.workspace = Some(workspace),
            Err(err) => {
                tracing::warn!(run = %run.id, error = %err, "could not prepare an overnight run");
                now.state = OvernightState::Finished;
                now.stop = Some(StopReason::Failed {
                    message: format!("Its branch and worktree couldn't be made: {err}"),
                });
                now.finished_at_ms = Some(now_ms());
            }
        }
        let (conversation_id, run_id) = (now.conversation_id.clone(), now.id.clone());
        if let Err(err) = self
            .record_runs(
                &run.conversation_id,
                vec![DomainEvent::OvernightUpdated { run: Box::new(now) }],
            )
            .await
        {
            tracing::warn!(run = %run.id, error = %err, "could not record an overnight run");
            return;
        }
        // Its first phase (or Phase 0) starts once the lock is free.
        self.advance_soon(&conversation_id, &run_id);
    }

    /// Reads the plan's source files as they are now, into the blob store. A file that can't
    /// be read is named without contents; the run asks for it when it needs it.
    async fn snapshot_sources(&self, repo: &Path, plan: &ProposedPlan) -> Vec<SourceSnapshot> {
        let mut snapshots = Vec::new();
        for source in &plan.sources {
            let path = Path::new(&source.path);
            let full = if path.is_absolute() {
                path.to_owned()
            } else {
                repo.join(path)
            };
            let bytes = blocking(move || {
                let size = std::fs::metadata(&full)
                    .map_err(|err| Error::Invalid(err.to_string()))?
                    .len();
                if size > MAX_SOURCE_BYTES {
                    return Err(Error::Invalid("too large to keep".into()));
                }
                std::fs::read(&full).map_err(|err| Error::Invalid(err.to_string()))
            })
            .await;
            let (blob, size) = match bytes {
                Ok(bytes) => {
                    let size = bytes.len() as u64;
                    match self.core.store().blobs().put(bytes).await {
                        Ok(hash) => (Some(hash.to_string()), Some(size)),
                        Err(_) => (None, Some(size)),
                    }
                }
                Err(_) => (None, None),
            };
            snapshots.push(SourceSnapshot {
                path: source.path.clone(),
                sections: source.sections.clone(),
                blob,
                bytes: size,
            });
        }
        snapshots
    }
}

/// The plan's phases with stable ids: `phase-<number>`, criteria `p<number>-c<n>`.
fn phases_of(plan: &ProposedPlan) -> Vec<OvernightPhase> {
    plan.phases
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
        .collect()
}

fn infos(phases: &[OvernightPhase]) -> Vec<PhaseInfo> {
    phases
        .iter()
        .map(|phase| PhaseInfo {
            number: phase.number,
            depends_on: phase.depends_on.clone(),
        })
        .collect()
}

/// Phases this run's earlier segments verified.
fn verified_numbers(board: &Board, run: &OvernightRun) -> Vec<u32> {
    let mut numbers: Vec<u32> = run
        .phases
        .iter()
        .filter(|phase| phase.state == PhaseState::Verified)
        .map(|phase| phase.number)
        .collect();
    let mut previous = run.predecessor.as_ref().and_then(|id| board.runs.get(id));
    while let Some(segment) = previous {
        numbers.extend(
            segment
                .phases
                .iter()
                .filter(|phase| phase.state == PhaseState::Verified)
                .map(|phase| phase.number),
        );
        previous = segment
            .predecessor
            .as_ref()
            .and_then(|id| board.runs.get(id));
    }
    numbers
}

fn selects(directives: &Directives, number: u32) -> bool {
    directives
        .only
        .is_none_or(|range| (range.from..=range.to).contains(&number))
        && !directives.skip.contains(&number)
}

/// When wind-down starts for a run started at `started` with its report due at `due`: a
/// reserve of a third of the run, at most 20 minutes.
fn wind_down_at(started: i64, due: i64) -> i64 {
    let reserve = ((due - started) / 3).clamp(0, WIND_DOWN_MAX_MS);
    due - reserve
}

/// The plan's name, or the first words of the user's message.
fn name_of(plan: &ProposedPlan, words: &str) -> String {
    let name = plan.name.trim();
    if !name.is_empty() {
        return name.to_owned();
    }
    let line = words
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("/overnight"))
        .or_else(|| {
            words
                .trim()
                .strip_prefix("/overnight")
                .map(str::trim)
                .filter(|rest| !rest.is_empty())
        })
        .unwrap_or("Overnight run");
    let line = line.trim_start_matches("/overnight").trim();
    let end = line.char_indices().nth(48).map_or(line.len(), |(at, _)| at);
    let cut = &line[..end];
    let cut = match cut.find(['.', '!', '?', ':']) {
        Some(at) if at > 0 => &cut[..at],
        _ => cut,
    };
    if cut.is_empty() {
        "Overnight run".into()
    } else {
        cut.to_owned()
    }
}

/// Whether `title` is one Brigadier gave the session, from the user's `words` or before any.
fn words_title(title: &str, words: &str) -> bool {
    let start = title.trim_end_matches('…').trim();
    let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
    matches!(title, "New session" | "New chat") || (!start.is_empty() && words.starts_with(start))
}

#[cfg(test)]
mod title_tests {
    use super::words_title;

    #[test]
    fn only_a_title_taken_from_the_users_words_gives_way_to_the_run_name() {
        let words = "/overnight Make overnight runs faster.\n\nRules: never Fable.";
        assert!(words_title("/overnight Make overnight runs…", words));
        assert!(words_title("New session", words));
        assert!(!words_title("Speed work", words));
    }
}
