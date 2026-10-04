//! Overnight runs (PLAN.md §10): a plan the user hands over with an optional deadline, worked
//! through phase by phase while they are away. These records are the run's durable truth and
//! the wire contract the plan card shows; the conductor (`manager::overnight`) changes them.
//!
//! A run is stored as full snapshots (`DomainEvent::OvernightUpdated`) on its conversation's
//! stream, like plans and tasks. Its phases, criteria and the user's words are fixed once
//! proposed; only the user's later words change its restrictions, each change a new revision.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::{CardId, ConversationId, OvernightRunId, TaskId};
use crate::work::Gate;

/// The run's own branch and worktree, made at Start from the base's committed tip. Its work
/// lands there; only the user's Merge brings verified work into the base.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RunWorkspace {
    /// The branch the run started from and its verified work merges into.
    pub base: String,
    /// The base's tip at Start.
    pub base_commit: String,
    /// `overnight/<date>-<slug>-<short id>`.
    pub branch: String,
    /// The run's worktree, in Brigadier's data directory.
    pub path: String,
}

/// What a task does for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RunRole {
    /// Work the phase lead delegated (and the checks of a plan it proposed).
    Worker,
    /// A reviewer or verifier of a run task's change.
    Check,
    /// Verifies every "done when" criterion of a whole phase, fresh.
    PhaseVerifier,
    /// Reviews a whole phase's diff, from another vendor than its authors.
    PhaseReviewer,
    /// Judges a whole phase (or Phase 0's plan) from the evidence, in a fresh context.
    Judge,
}

impl RunRole {
    /// Checks a whole phase: its candidate stays as it is while they work.
    pub fn checks_phase(self) -> bool {
        matches!(
            self,
            Self::PhaseVerifier | Self::PhaseReviewer | Self::Judge
        )
    }
}

/// Which run a task works for, fixed when the task is made: a late event of the task keeps
/// it, whatever the run or the user's newest message is doing by then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RunTaskContext {
    pub run_id: OvernightRunId,
    pub segment: u32,
    /// The phase it works on; absent before the conductor admits phases (Phase 0, setup).
    #[serde(default)]
    pub phase_id: Option<String>,
    /// The run's generation when the task was made; a result from an older one is history.
    pub generation: u32,
    pub role: RunRole,
    /// The run's Rules as the task was briefed (a hash of the text).
    pub rules_hash: String,
    /// The phase candidate a whole-phase check works on (its checkout is at this commit).
    #[serde(default)]
    pub candidate: Option<String>,
}

/// Where a run is. `Proposed` waits for the user's Start; everything after it is the run's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum OvernightState {
    /// Shown on the card with one Start. Nothing runs yet.
    Proposed,
    /// A newer proposal replaced it before it started.
    Superseded,
    /// Started: its branch and worktree are being made, earlier work is settling.
    Preparing,
    /// Phase 0: writing and reviewing the plan for a bare goal.
    Planning,
    Running,
    /// A phase's work settled and its whole result is being checked.
    PhaseGate,
    /// No eligible model until a usage window resets.
    WaitingQuota,
    /// Stop, the deadline or a block: no new work, live work hands off.
    WindingDown,
    Reporting,
    Finished,
}

impl OvernightState {
    /// Started and not yet finished: the run owns its session and keeps the daemon up.
    pub fn is_active(self) -> bool {
        !matches!(self, Self::Proposed | Self::Superseded | Self::Finished)
    }

    /// No further change of state can happen.
    pub fn is_final(self) -> bool {
        matches!(self, Self::Superseded | Self::Finished)
    }
}

/// How a phase ended up, or where it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PhaseState {
    Pending,
    Running,
    /// Its whole result is being verified, reviewed and judged.
    Checking,
    Verified,
    /// Unfinished: some criteria may be met, the rest wait on the user, or the run ended first.
    Partial,
    Blocked,
    /// Not selected by the user's restrictions, or left out by a stop.
    Skipped,
}

/// One "done when" criterion. Its id never changes, so evidence and verdicts name it exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Criterion {
    /// `p<phase number>-c<n>`.
    pub id: String,
    pub text: String,
}

/// A phase of the plan, as the user's plan numbers it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OvernightPhase {
    /// `phase-<number>`, the same in every segment of the run.
    pub id: String,
    /// The source plan's own number, which the user's restrictions refer to.
    pub number: u32,
    pub name: String,
    /// What the phase covers, exactly as the plan says.
    pub scope: String,
    pub done_when: Vec<Criterion>,
    /// Numbers of the phases it builds on.
    pub depends_on: Vec<u32>,
    pub state: PhaseState,
    /// The request its lead's turns, tasks and reports belong to; set when it starts.
    #[serde(default)]
    pub request_id: Option<String>,
    /// The run branch's tip when the phase started: the whole phase is checked against it.
    #[serde(default)]
    pub start_commit: Option<String>,
    /// The commit its checks verified, once the phase is verified.
    #[serde(default)]
    pub verified_commit: Option<String>,
    /// The latest round of whole-phase checks: its commit is the candidate they check.
    #[serde(default)]
    pub gate: Option<Gate>,
    /// Fix rounds after its checks found gaps (at most two).
    #[serde(default)]
    pub fix_rounds: u32,
    /// What each criterion came to, from the judge and the fresh verifier's evidence.
    #[serde(default)]
    pub criteria: Vec<CriterionResult>,
    /// What the phase still lacks after its last checks, each in one line.
    #[serde(default)]
    pub gaps: Vec<String>,
    /// The lead's own summary when it said the phase's work was done.
    #[serde(default)]
    pub summary: Option<String>,
    /// The lead's answers to the findings of the checks before its last fix round.
    #[serde(default)]
    pub responses: Vec<String>,
    /// The lead's model, the vendor whose work its reviewers must not be.
    #[serde(default)]
    pub lead: Option<crate::model::ModelChoice>,
    /// Times the lead was reminded to say whether the phase's work is done.
    #[serde(default)]
    pub nudges: u32,
    #[serde(default)]
    pub started_at_ms: Option<i64>,
    #[serde(default)]
    pub settled_at_ms: Option<i64>,
}

impl OvernightPhase {
    /// Settled: verified, partial, blocked or skipped.
    pub fn is_settled(&self) -> bool {
        matches!(
            self.state,
            PhaseState::Verified | PhaseState::Partial | PhaseState::Blocked | PhaseState::Skipped
        )
    }

    /// A new phase from a plan, with stable ids: `phase-<number>`, criteria `p<number>-c<n>`.
    pub fn new(
        number: u32,
        name: &str,
        scope: &str,
        done_when: &[String],
        depends_on: &[u32],
    ) -> Self {
        Self {
            id: format!("phase-{number}"),
            number,
            name: name.trim().to_owned(),
            scope: scope.to_owned(),
            done_when: done_when
                .iter()
                .enumerate()
                .map(|(at, text)| Criterion {
                    id: format!("p{number}-c{}", at + 1),
                    text: text.clone(),
                })
                .collect(),
            depends_on: depends_on.to_vec(),
            state: PhaseState::Pending,
            request_id: None,
            start_commit: None,
            verified_commit: None,
            gate: None,
            fix_rounds: 0,
            criteria: Vec::new(),
            gaps: Vec::new(),
            summary: None,
            responses: Vec::new(),
            lead: None,
            nudges: 0,
            started_at_ms: None,
            settled_at_ms: None,
        }
    }
}

/// What a criterion came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum CriterionStatus {
    Met,
    NotMet,
    /// Its check couldn't run, or nobody checked it before the run ended.
    NotRun,
    /// It needs something only the user can give.
    Blocked,
}

/// One criterion's result, checked on one candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CriterionResult {
    pub id: String,
    pub status: CriterionStatus,
    /// The evidence, or what is missing, in the checker's words.
    pub evidence: String,
    /// The commit it was checked on.
    pub candidate: Option<String>,
    /// The task whose report gave the evidence.
    pub by: Option<TaskId>,
}

/// A run's notification, queued with its report and shown by the app as Brigadier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RunNotification {
    /// Stable per run segment, so a repeated delivery replaces rather than adds.
    pub id: String,
    pub title: String,
    pub body: String,
    pub created_at_ms: i64,
    /// When the app showed it; absent while it waits.
    pub delivered_at_ms: Option<i64>,
    /// The last OS refusal/submission error; the outbox stays pending.
    #[serde(default)]
    pub delivery_error: Option<String>,
}

/// Verified work explicitly merged by the user; continuation keeps the run branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RunMerge {
    pub verified_commit: String,
    pub commit: String,
    pub into: String,
    pub at_ms: i64,
}

/// A time Brigadier wasn't running during a run, and why, as far as it can tell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RunGap {
    pub from_ms: i64,
    pub to_ms: i64,
    /// "The Mac slept 01:10–03:40", or "Brigadier was unavailable …" without a sleep record.
    pub cause: String,
}

/// Something that got in the way of a run's work, for the report's "What got in the way"
/// (PLAN.md §10.9). The same thing happening again counts up instead of adding a line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RunObstacle {
    pub kind: ObstacleKind,
    /// One plain line: "`git push` was declined by the overnight rules: it acts outside this
    /// machine".
    pub text: String,
    /// The tasks it happened to, by number, each once.
    pub tasks: Vec<u32>,
    pub count: u32,
    pub first_at_ms: i64,
    pub last_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ObstacleKind {
    /// Something on the never-list a worker tried, refused at once (PLAN.md §10.8).
    Declined,
    /// A worker's session couldn't be resumed, so its task started over.
    ResumeFailed,
}

impl OvernightRun {
    /// Adds an obstacle, or counts it again when the same one is listed.
    pub fn note_obstacle(&mut self, kind: ObstacleKind, text: &str, task: Option<u32>, at_ms: i64) {
        let found = self
            .obstacles
            .iter_mut()
            .find(|obstacle| obstacle.kind == kind && obstacle.text == text);
        let obstacle = match found {
            Some(obstacle) => {
                obstacle.count += 1;
                obstacle.last_at_ms = at_ms;
                obstacle
            }
            None => {
                self.obstacles.push(RunObstacle {
                    kind,
                    text: text.to_owned(),
                    tasks: Vec::new(),
                    count: 1,
                    first_at_ms: at_ms,
                    last_at_ms: at_ms,
                });
                self.obstacles.last_mut().expect("just pushed")
            }
        };
        if let Some(task) = task
            && !obstacle.tasks.contains(&task)
        {
            obstacle.tasks.push(task);
        }
    }
}

/// Phase 0 of a bare goal: the lead writes the plan's phases, another vendor reviews them,
/// and a fresh judge checks they follow the goal without invented scope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlanningPhase {
    pub request_id: String,
    pub state: PhaseState,
    /// The plan card the phases were proposed on (reviewed like any plan).
    pub plan_id: Option<CardId>,
    /// The phases as proposed, kept until the judge accepts them.
    pub proposed: Vec<ProposedPhase>,
    /// The judge of the latest proposal.
    pub judge: Option<TaskId>,
    /// Judge rounds so far (at most two).
    pub rounds: u32,
    /// What the judge found missing or invented.
    pub gaps: Vec<String>,
    pub lead: Option<crate::model::ModelChoice>,
    pub nudges: u32,
    pub started_at_ms: i64,
    pub settled_at_ms: Option<i64>,
}

/// A file the plan was read from, as it was when proposed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SourceSnapshot {
    /// As the user named it (relative to the repository, or absolute).
    pub path: String,
    /// The parts of it the plan uses ("phases 3–5"), as given.
    pub sections: Option<String>,
    /// Its contents in the blob store; absent when it couldn't be read (the run asks for it).
    pub blob: Option<String>,
    pub bytes: Option<u64>,
}

/// A wall-clock time the report is due, resolved once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedTime {
    /// The instant, in ms since the Unix epoch. Later clock or time-zone changes don't move it.
    pub at_ms: i64,
    /// The local date it falls on, `2026-10-03`.
    pub local_date: String,
    /// The local time, `07:30`.
    pub local_time: String,
    /// The local date in words, `Sat 3 Oct`, so a misread day shows.
    pub day: String,
    /// The UTC offset at that time, `+03:00`.
    pub offset: String,
    /// The time zone it was resolved in (an IANA name where known).
    pub time_zone: String,
}

/// When the report should be ready.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Deadline {
    /// Until done: the run ends when its phases are done, it is blocked, or the user stops it.
    UntilDone,
    /// A time of day or a date and time.
    At { time: ResolvedTime },
    /// A duration from Start (`for 3 hours`); Start turns it into `At`.
    For { minutes: u32 },
}

/// "Stop after …".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum StopAfter {
    /// After the phase with this number settles.
    Phase { number: u32 },
    /// After the phase that was running when the user said "this phase" (its id).
    Current { phase_id: String },
}

/// Which kind of restriction a piece of the user's words set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum DirectiveKind {
    Deadline,
    StopAfter,
    Only,
    Skip,
    MaxWorkers,
    /// A quality setting Brigadier decides itself (effort, a model family, hand-off size).
    Ignored,
}

/// Where in the user's words a restriction came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DirectiveSpan {
    pub kind: DirectiveKind,
    /// The words, as typed.
    pub text: String,
    /// Byte offsets in the text they were read from.
    pub start: u32,
    pub end: u32,
}

/// The restrictions Brigadier enforces in code. Everything else the user wrote is Rules,
/// passed word for word to every phase lead, verifier and judge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Directives {
    pub deadline: Deadline,
    pub stop_after: Option<StopAfter>,
    /// Only phases `from..=to`.
    pub only: Option<PhaseRange>,
    pub skip: Vec<u32>,
    /// Brigadier workers of this run executing at once.
    pub max_workers: Option<u32>,
    /// One line per quality setting the user asked for and Brigadier won't apply.
    pub ignored: Vec<String>,
    pub spans: Vec<DirectiveSpan>,
}

impl Default for Directives {
    fn default() -> Self {
        Self {
            deadline: Deadline::UntilDone,
            stop_after: None,
            only: None,
            skip: Vec::new(),
            max_workers: None,
            ignored: Vec::new(),
            spans: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PhaseRange {
    pub from: u32,
    pub to: u32,
}

/// Something in the restrictions that stops Start until the user says it differently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DirectiveProblem {
    pub kind: DirectiveKind,
    /// What's wrong and how to say it instead, in plain words.
    pub message: String,
}

/// Why a run ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum StopReason {
    /// Every selected phase settled.
    Done,
    /// The user's Stop.
    Stopped,
    Deadline,
    /// A later phase needs what a blocked one couldn't finish.
    Blocked {
        phase_id: String,
    },
    /// "Stop after phase N" was reached.
    StopDirective,
    /// It couldn't be prepared (its branch or worktree couldn't be made).
    Failed {
        message: String,
    },
}

/// A user command the run applied, kept so a repeated one changes nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AppliedCommand {
    pub id: String,
    pub at_ms: i64,
}

/// One segment of an overnight run: Start to its report. Continue proposes the next segment
/// on the same branch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OvernightRun {
    pub id: OvernightRunId,
    pub conversation_id: ConversationId,
    /// 1 for the first segment, one more for each Continue.
    pub segment: u32,
    /// The segment this one continues.
    pub predecessor: Option<OvernightRunId>,
    /// The plan card it is shown with, when the plan came from one.
    pub plan_id: Option<CardId>,
    /// The plan's own name ("Windows support"), shown on the card.
    pub name: String,
    /// The user's words that asked for the run, verbatim.
    pub words: String,
    /// The goal: the plan's own statement of it, or the user's words.
    pub goal: String,
    /// Rules and settled decisions every lead, verifier and judge gets verbatim.
    pub rules: String,
    pub sources: Vec<SourceSnapshot>,
    /// Empty for a bare goal until Phase 0 writes the plan.
    pub phases: Vec<OvernightPhase>,
    pub directives: Directives,
    /// What stops Start until the user words it differently.
    pub problems: Vec<DirectiveProblem>,
    /// Bumped by every change of what the user agreed to (the proposal, its restrictions).
    /// Start must name the revision the user saw.
    pub revision: u32,
    /// Bumped each time the run is (re)claimed: results from an older generation are history.
    pub generation: u32,
    pub state: OvernightState,
    /// When wind-down starts; set at Start for a deadline.
    pub wind_down_at_ms: Option<i64>,
    /// Its branch and worktree, once Start made them.
    #[serde(default)]
    pub workspace: Option<RunWorkspace>,
    /// Phase 0, for a bare goal.
    #[serde(default)]
    pub planning: Option<PlanningPhase>,
    /// The newest commit of the run branch whose every phase up to it is verified: the
    /// card's Merge takes this, never the branch's head.
    #[serde(default)]
    pub verified_commit: Option<String>,
    /// Times Brigadier wasn't running during the run (the Mac slept, the daemon was down).
    #[serde(default)]
    pub gaps: Vec<RunGap>,
    /// What got in the way of the work, for the report.
    #[serde(default)]
    pub obstacles: Vec<RunObstacle>,
    /// The report's message, once written (its id is stable per segment).
    #[serde(default)]
    pub report_message_id: Option<String>,
    /// The report's three opening paragraphs, for a restored card whose message is off-page.
    #[serde(default)]
    pub report_outcome: Option<[String; 3]>,
    #[serde(default)]
    pub merged: Option<RunMerge>,
    /// The notification the report comes with, until the app shows it.
    #[serde(default)]
    pub notification: Option<RunNotification>,
    pub stop: Option<StopReason>,
    /// The last commands applied, newest last.
    pub commands: Vec<AppliedCommand>,
    pub created_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub finished_at_ms: Option<i64>,
}

impl OvernightRun {
    /// Whether the user's restrictions select the phase with this number.
    pub fn selects(&self, number: u32) -> bool {
        self.directives
            .only
            .is_none_or(|range| (range.from..=range.to).contains(&number))
            && !self.directives.skip.contains(&number)
    }

    pub fn phase(&self, id: &str) -> Option<&OvernightPhase> {
        self.phases.iter().find(|phase| phase.id == id)
    }

    /// A running run named `name` with `phases`, for tests.
    #[cfg(test)]
    pub(crate) fn for_test(
        conversation_id: ConversationId,
        name: &str,
        phases: Vec<OvernightPhase>,
    ) -> Self {
        Self {
            id: OvernightRunId::generate(),
            conversation_id,
            segment: 1,
            predecessor: None,
            plan_id: None,
            name: name.into(),
            words: format!("/overnight {name}"),
            goal: name.into(),
            rules: String::new(),
            sources: Vec::new(),
            phases,
            directives: Directives::default(),
            problems: Vec::new(),
            revision: 1,
            generation: 1,
            state: OvernightState::Running,
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
            commands: Vec::new(),
            created_at_ms: 0,
            started_at_ms: Some(0),
            finished_at_ms: None,
        }
    }
}

/// A plan for an overnight run, as the orchestrator read it from the user's words and files
/// (or an IPC script gives it). Phase numbers are the source plan's own; criteria get their
/// ids when the run is proposed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
pub struct ProposedPlan {
    pub name: String,
    /// The plan's statement of the goal; the user's words when absent.
    pub goal: Option<String>,
    /// Rules and settled decisions from the plan's sources, verbatim.
    pub rules: Option<String>,
    pub phases: Vec<ProposedPhase>,
    pub sources: Vec<ProposedSource>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
pub struct ProposedPhase {
    /// Its number in the source plan; its place in the list when absent.
    pub number: Option<u32>,
    pub name: String,
    pub scope: String,
    pub done_when: Vec<String>,
    pub depends_on: Vec<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
pub struct ProposedSource {
    pub path: String,
    pub sections: Option<String>,
}
