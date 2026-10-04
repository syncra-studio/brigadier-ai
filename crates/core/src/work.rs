//! The work inside a conversation: delegated tasks and their workers, reports, the cards that
//! wait for the user (approvals, questions, plans), the message queue and attachments. Like the
//! catalog, these types are the wire contract and are exported to TypeScript.
//!
//! Each conversation's stream (`conversation:<id>`) holds its messages and snapshots of these
//! objects as they change. `position` fields are the stream sequence of the event that created
//! the object, so messages and cards interleave in one timeline.

use brigadier_providers::{
    ApprovalRequest, Decider, ErrorKind, LimitHit, ProviderEvent, ProviderKind,
};
use brigadier_router::{Area, Capability, Explanation, Pin, QualityTier};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use crate::model::{CardId, Mention, TaskId};
use crate::model::{ConversationId, ModelChoice};

/// A file the user attached, kept in the blob store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentRef {
    /// Content hash in the blob store.
    pub id: String,
    pub name: String,
    pub mime: String,
    pub bytes: u64,
    /// Text the user pasted into the composer, not a file they attached: it is part of what
    /// they wrote, so it goes to the model as their message.
    #[serde(default)]
    pub pasted: bool,
    /// A pasted image placed at its `[image:<id>]` token in the message.
    #[serde(default)]
    pub inline: bool,
}

// ----- tasks and workers ------------------------------------------------------------------

/// What a task does (PLAN.md §5).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum TaskKind {
    /// Looks around the repository and answers a question.
    Scout,
    /// Reads docs and the web.
    Research,
    /// Changes code; its accepted result lands as one commit.
    Implement,
    /// Reviews another task's candidate commit or a plan.
    Review,
    /// Resolves conflicts between a task and its target branch.
    Merge,
    /// Runs the project's checks.
    Verify,
}

impl TaskKind {
    /// Whether the task's result is a change that lands as a commit.
    pub fn writes(self) -> bool {
        matches!(self, Self::Implement | Self::Merge)
    }
}

/// How a worker may touch the repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RepoAccess {
    /// No repository checkout (research).
    None,
    /// A detached worktree it can read but not change.
    Read,
    /// Its own worktree on a task branch.
    Write,
}

/// A worker's sandbox, set per task. Every worker also has a writable scratch folder outside
/// the repository, and the outward-command gate at every permission level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkerAccess {
    pub repo: RepoAccess,
    pub network: bool,
    /// No OS sandbox (Full access).
    pub unsandboxed: bool,
}

/// The model a task runs on and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    pub choice: ModelChoice,
    /// Shown on the worker card ("why this model"): one short sentence.
    pub reason: String,
    /// The breakdown behind `reason`, for the card's details.
    #[serde(default)]
    pub explanation: Option<Explanation>,
}

/// How one model's run of a task ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AttemptEnd {
    /// Its provider refused more work (a usage window ran out, a spend control, credits).
    Limit { limit: LimitHit },
    /// It failed in a way another model may not (overloaded, a server or network error, its
    /// CLI exiting, the context window).
    Error { kind: ErrorKind, message: String },
}

/// One model's run of a task. A task that was handed off mid-way has several; the last one is
/// the model at work now (`Task::route`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    pub route: Route,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    /// Why it ended, when it was cut short; absent while it runs and when it finished the
    /// task.
    pub end: Option<AttemptEnd>,
}

/// A task waiting for quota: no model it may use is available (all at their limits, or the
/// ones a user rule allows). It resumes on its own when one resets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWait {
    /// What it waits for ("Claude's 5-hour window resets at 21:10").
    pub reason: String,
    /// The earliest reset that could let it continue, when known.
    pub resets_at_ms: Option<i64>,
    /// The user rule that keeps it from other models, if one does (its text).
    pub rule: Option<String>,
    /// The user's ranking that keeps it from other models (Only these), if one does (its
    /// text).
    #[serde(default)]
    pub ranking: Option<String>,
    pub since_ms: i64,
    /// A conversation's waiting user messages (their ids), to wait with again after a
    /// restart. A task's wait has none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(skip)]
    pub messages: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TaskState {
    /// Waiting for an approved plan, a free slot or the task it depends on.
    Queued,
    /// Creating its worktree and starting the worker.
    Starting,
    Running,
    /// The worker asked the orchestrator a question and waits for the answer.
    Blocked,
    /// The user interrupted the worker, or it waits for quota (`quotaWait`); it continues on
    /// resume, or when the quota it waits for resets.
    Paused,
    /// The worker submitted its report; the orchestrator decides what happens next.
    Reported,
    /// Its candidate commit is being reviewed by another vendor.
    Reviewing,
    /// Ask for approval: the landing waits for the user.
    AwaitingApproval,
    /// Accepted, but it cannot land safely right now (see `blockedReason`). Nothing changed.
    ReadyToLand,
    /// Its commit is on the target branch.
    Landed,
    /// Finished without landing (read-only tasks end here too).
    Done,
    /// The orchestrator or a review turned it down.
    Rejected,
    /// Stopped by the user or the orchestrator.
    Stopped,
    Failed,
}

impl TaskState {
    /// Whether the task is over: its worker is gone and it will not run again.
    pub fn is_final(self) -> bool {
        matches!(
            self,
            Self::Landed | Self::Done | Self::Rejected | Self::Stopped | Self::Failed
        )
    }
}

/// Where a task works.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskWorkspace {
    /// Its worktree, in Brigadier's data directory. Absent for tasks without a checkout.
    pub worktree: Option<String>,
    /// Its task branch (write tasks).
    pub branch: Option<String>,
    /// The commit it started from.
    pub base: Option<String>,
    /// `base` is a snapshot of the user's uncommitted changes (they let workers see them).
    /// Those changes are never landed or kept as part of the task's work.
    #[serde(default)]
    pub on_snapshot: bool,
    /// The branch its accepted work lands on.
    pub target: Option<String>,
    /// Its scratch folder outside the repository.
    pub scratch: String,
    /// Dependency installs and build caches copied in from the user's checkout
    /// (repo-relative folders), which its worker is told not to reinstall.
    #[serde(default)]
    #[ts(skip)]
    pub warmed: Vec<String>,
}

/// A structured report, the only part of a worker's work that enters the orchestrator's
/// context. About 800 tokens at most; details go to artifacts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub summary: String,
    pub changes: Vec<String>,
    pub decisions: Vec<String>,
    /// What the worker verified and how.
    pub verification: Vec<String>,
    /// Each "done when" criterion with its status and evidence ("[met] … ").
    #[serde(default)]
    pub done_when: Vec<String>,
    pub open_questions: Vec<String>,
    /// Risks, assumptions and what was skipped.
    #[serde(default)]
    pub risks: Vec<String>,
    /// What only the user can do.
    #[serde(default)]
    pub needs_user: Vec<String>,
    /// For review tasks: the verdict.
    pub verdict: Option<ReviewVerdict>,
    /// For verify tasks: whether the project's checks passed.
    #[serde(default)]
    pub checks: Option<ChecksResult>,
    pub artifacts: Vec<ArtifactRef>,
    pub submitted_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ReviewVerdict {
    Approve,
    RequestChanges,
}

/// What a verify task found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ChecksResult {
    /// Every check it ran passed. Any it could not run fail the same way on the parent
    /// commit, each named under risks as "[pre-existing] check: evidence from the parent".
    Passed,
    /// At least one check failed.
    Failed,
    /// The project has checks, but it could not run some that are not such gaps: the change
    /// does not land.
    NotRun,
    /// The project has no checks it could run; the evidence for each criterion comes from
    /// reading the code and running what it could.
    NoChecks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ArtifactKind {
    /// The worker's full transcript.
    Transcript,
    Diff,
    CommandOutput,
    Screenshot,
    /// A research or review note.
    Note,
    File,
}

/// Something stored in the blob store that the orchestrator can read on demand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRef {
    /// Content hash in the blob store.
    pub id: String,
    pub title: String,
    pub kind: ArtifactKind,
    pub mime: String,
    pub bytes: u64,
    /// The name to save it under (a worker's file keeps its own name).
    #[serde(default)]
    pub file_name: Option<String>,
}

/// Lines added and removed, per file and in total.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DiffStat {
    pub files: Vec<FileStat>,
    pub insertions: u32,
    pub deletions: u32,
}

/// What a write task's checkout changed since it started while it is at work: its commits and
/// its uncommitted files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkerDiff {
    pub task_id: TaskId,
    pub stat: DiffStat,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FileStat {
    pub path: String,
    pub insertions: u32,
    pub deletions: u32,
    /// Binary files have no line counts.
    pub binary: bool,
}

/// A write task's result as one commit on the current target tip: what is reviewed, verified
/// and landed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub commit: String,
    /// The target tip it was built on.
    pub onto: String,
    pub message: String,
    pub diff_stat: DiffStat,
    /// Files the litter guard left out, with why.
    pub excluded: Vec<ExcludedFile>,
    /// The full diff, for reviewers and the UI.
    pub diff: Option<ArtifactRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ExcludedFile {
    pub path: String,
    pub reason: String,
}

/// The mandatory cross-vendor review of a write task's candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ReviewRecord {
    /// The review task.
    pub task_id: TaskId,
    /// The candidate commit it reviewed.
    pub commit: String,
    pub verdict: Option<ReviewVerdict>,
    /// False when only one vendor was available and another model of it reviewed.
    pub cross_vendor: bool,
}

/// One round of independent checks of a change before it lands (reviewers and a verifier, on
/// one candidate commit) or of a plan before it is approved (reviewers). A new candidate or
/// a revised plan opens a new round; results of an older round are ignored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Gate {
    /// Checks chosen from this candidate. Missing on events written before scoped checks.
    #[serde(default)]
    pub verification_scope: VerificationScope,
    /// This candidate was rebased during landing; verify retries must keep full checks.
    #[serde(default)]
    pub rebased: bool,
    /// From 1, counted per task or plan.
    pub round: u32,
    /// The candidate commit the round checks; none for a plan.
    #[serde(default)]
    pub commit: Option<String>,
    pub members: Vec<GateMember>,
    /// Set once every member has a result; the round is closed then.
    #[serde(default)]
    pub outcome: Option<GateOutcome>,
    /// The user already approved the change it checks (a clean replay onto a target that
    /// moved): it lands as soon as the round passes.
    #[serde(default)]
    pub relanding: bool,
    /// A second verification after a verifier could not check the change.
    #[serde(default)]
    pub retry: bool,
    /// The user had the change land despite the round's findings (`accept_task` with
    /// `override`): it lands as it is, without another round.
    #[serde(default)]
    pub overridden: bool,
    /// A plan's round: its reviewers' issues as each result arrives, numbered F1, F2, … for
    /// the revision to answer one by one.
    #[serde(default)]
    pub findings: Vec<Finding>,
}

/// The checks required for one candidate, and why they were selected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum VerificationScope {
    Full {
        reason: String,
    },
    Scoped {
        reason: String,
        crates: Vec<String>,
        desktop: bool,
    },
}

impl Default for VerificationScope {
    fn default() -> Self {
        Self::Full {
            reason: "Scope not determined".into(),
        }
    }
}

/// The orchestrator was handed a write task's failed checks (see [`Task::escalated`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Escalated {
    /// The gate round whose findings it was told.
    pub round: u32,
    /// The candidate commit those findings are about.
    pub commit: String,
    pub at_ms: i64,
}

/// A problem a plan's reviewer found, by the id the revision answers it with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// "F1", "F2", … within its round.
    pub id: String,
    pub text: String,
    /// The reviewer that found it.
    pub by: TaskId,
}

/// How a revised plan answers a finding of the plan it revises.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FindingResponse {
    /// The finding's id ("F1").
    pub id: String,
    /// The finding, as the reviewer wrote it.
    pub finding: String,
    pub accepted: bool,
    /// What changed for it, or why it was declined.
    pub note: String,
}

/// A task checking a change or plan in a gate round.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GateMember {
    pub task_id: TaskId,
    pub role: GateRole,
    #[serde(default)]
    pub result: Option<GateResult>,
    /// Models it must not be besides the author and the round's other members, kept for a
    /// hand-off: a second verifier avoids the one that could not check the change.
    #[serde(default)]
    pub avoid: Vec<ModelChoice>,
}

/// What a gate member does. The Phase 6 fusion panel adds an analyst that weighs the
/// reviewers' findings (`SessionManager::panel_size` decides the panel).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum GateRole {
    /// Reviews the change or plan and gives a verdict.
    Review,
    /// Runs the checks and proves each "done when" criterion of the task.
    Verify,
    /// Judges a whole overnight phase from its verification and review, in a fresh context.
    Judge,
}

/// A gate member's result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GateResult {
    Passed,
    /// The change or plan needs fixing; the findings say what.
    Failed {
        findings: Vec<String>,
    },
    /// It could not check the change (checks that couldn't run, criteria left unchecked);
    /// nothing lands unverified.
    Unverified {
        reason: String,
    },
    /// It gave no usable result: it failed or was stopped, or (a verifier) changed what it
    /// checked.
    NoResult {
        reason: String,
    },
}

/// How a gate round ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GateOutcome {
    Passed,
    Failed,
    Unverified,
    NoResult,
    /// A newer candidate or plan replaced what it checked.
    Superseded,
}

/// The gate round a checking task belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GateLink {
    pub owner: GateOwner,
    pub round: u32,
    pub role: GateRole,
}

/// What a gate checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GateOwner {
    /// A write task's candidate commit.
    Task { task_id: TaskId },
    /// A proposed plan.
    Plan { plan_id: CardId },
    /// A whole phase of an overnight run: its candidate is the run branch's tip.
    Phase {
        run_id: crate::model::OvernightRunId,
        phase_id: String,
    },
}

/// A delegated unit of work and its worker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: TaskId,
    pub conversation_id: ConversationId,
    /// Shown as `task-N` and used for @-mentions; unique within the conversation.
    pub number: u32,
    pub position: i64,
    pub title: String,
    pub kind: TaskKind,
    /// The task spec the worker got.
    pub spec: String,
    pub access: WorkerAccess,
    /// The model at work now and why.
    pub route: Route,
    /// Every model that ran it, oldest first (more than one after a hand-off).
    #[serde(default)]
    pub attempts: Vec<Attempt>,
    /// The least capable model it may run on, fallbacks included.
    #[serde(default)]
    pub floor: QualityTier,
    /// The parts of the codebase it touches.
    #[serde(default)]
    pub areas: Vec<Area>,
    /// The provider, model or effort the orchestrator asked for. A hand-off or a resume keeps
    /// the task with that vendor, or it waits.
    #[serde(default)]
    pub pin: Option<Pin>,
    /// What its model must be able to do, fallbacks included (image generation).
    #[serde(default)]
    pub needs: Vec<Capability>,
    pub state: TaskState,
    /// Set while it is paused waiting for quota.
    #[serde(default)]
    pub quota_wait: Option<QuotaWait>,
    /// The task this one reviews or merges.
    pub subject: Option<TaskId>,
    /// The plan this one reviews.
    pub plan: Option<CardId>,
    pub attachments: Vec<AttachmentRef>,
    pub workspace: Option<TaskWorkspace>,
    pub report: Option<Report>,
    pub candidate: Option<Candidate>,
    pub review: Option<ReviewRecord>,
    /// A write task: the current gate round on its candidate (reviewers and a verifier).
    #[serde(default)]
    pub gate: Option<Gate>,
    /// A reviewer or verifier: the gate round it belongs to.
    #[serde(default)]
    pub gate_link: Option<GateLink>,
    /// A write task: the commit message it was accepted with, while Brigadier lands it on
    /// its own (it is re-gated after each fix round).
    #[serde(default)]
    pub landing: Option<String>,
    /// A write task: times Brigadier sent it back with a gate's findings.
    #[serde(default)]
    pub fix_rounds: u32,
    /// A write task: the findings Brigadier sent it back with, one entry per fix round,
    /// oldest first. Later checks read them, and so does the orchestrator when the fixes end
    /// without landing.
    #[serde(default)]
    pub fixes: Vec<String>,
    /// A write task Brigadier lands on its own: what its worker wrote after its last report,
    /// held back from the orchestrator. Its checks read it; the orchestrator gets it only if
    /// the change does not land.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub addendum: Option<String>,
    /// A write task: when the orchestrator was told its checks' findings and that nothing
    /// landed. Only the user's word after that lands the change despite them (`accept_task`
    /// with `override`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub escalated: Option<Escalated>,
    /// The landed commit.
    pub landed: Option<String>,
    /// Why it is blocked, paused or cannot land yet.
    pub blocked_reason: Option<String>,
    pub error: Option<String>,
    /// Unfinished changes kept when the task was stopped or archived.
    pub kept: Option<KeptWork>,
    /// The files the worker left in its outputs folder, stored when it reported and again
    /// when the task ended: deliverables the user saves from the task card.
    #[serde(default)]
    pub outputs: Vec<ArtifactRef>,
    /// The user request it was delegated for.
    #[serde(default)]
    pub request_id: Option<String>,
    /// The overnight run it works for: then it runs under the run's rules (the session's
    /// access, nothing on the never-list, the run's branch), whatever else the session says.
    #[serde(default)]
    pub run: Option<crate::overnight::RunTaskContext>,
    /// What the orchestrator sent the worker after its spec (`message_worker`), oldest first:
    /// changes to the task that its review checks the work against too.
    #[serde(default)]
    pub messages: Vec<String>,
    /// Times it was sent back to work after reporting (a review asking for changes, or a
    /// message from the orchestrator).
    #[serde(default)]
    pub rework_rounds: u32,
    /// The native id of the worker's latest CLI session, recorded when it starts: sending the
    /// task back resumes that session, however long its event stream has grown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub native_session: Option<String>,
    /// It took a trial slot when created and waits to start: it keeps the slot until then.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(skip)]
    pub trial_slot: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// What happened to a task's unfinished changes when its worktree was removed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum KeptWork {
    /// Committed as a work-in-progress commit on the task branch, which is kept.
    Branch { branch: String, commit: String },
    /// Stored as a diff artifact, when it could not be kept as a clean commit on the target
    /// (for example because it overlaps uncommitted changes the user let workers see). The
    /// task branch is gone; the patch can be restored as a new branch.
    Diff {
        artifact: ArtifactRef,
        /// The branch the patch was restored as.
        #[serde(default)]
        restored: Option<String>,
    },
}

/// What restoring a kept patch as a branch did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RestoreOutcome {
    /// A new branch with one commit of the patch on the target branch's current tip.
    Restored { branch: String, commit: String },
    /// The patch conflicts with the target branch now; nothing was created.
    Conflicts { paths: Vec<String> },
    /// The patch no longer applies at all; nothing was created.
    Failed { reason: String },
}

// ----- cards -------------------------------------------------------------------------------

/// Whether a card still waits for an answer, and the answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CardState {
    Pending,
    Allowed {
        by: Decider,
        /// The CLI keeps allowing the request's session grant (similar commands).
        #[serde(default)]
        similar: bool,
    },
    Denied {
        by: Decider,
        message: Option<String>,
    },
    /// Nobody answered in time, or what it asked about went away.
    Expired {
        reason: String,
    },
}

/// What an approval card asks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ApprovalSubject {
    /// A worker's CLI asks for a permission Brigadier may not grant on the user's behalf.
    Cli { request: ApprovalRequest },
    /// An outward command stopped by the command gate, bound to exactly this argv and cwd.
    OutwardCommand { argv: Vec<String>, cwd: String },
    /// Ask for approval: land a reviewed task on its branch.
    Landing {
        task_id: TaskId,
        branch: String,
        diff_stat: DiffStat,
    },
    /// Merge the session branch into its base.
    FinishSession {
        branch: String,
        base: String,
        commits: u32,
        diff_stat: DiffStat,
    },
    /// An action the orchestrator asked the user to approve.
    Action { action: String, details: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub id: CardId,
    pub conversation_id: ConversationId,
    pub task_id: Option<TaskId>,
    /// The user request it belongs to.
    #[serde(default)]
    pub request_id: Option<String>,
    pub position: i64,
    pub subject: ApprovalSubject,
    pub state: CardState,
    pub created_at_ms: i64,
    pub resolved_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum QuestionKind {
    /// The orchestrator asks something only the user can answer (`ask_user`).
    Orchestrator,
    /// Local checkout with uncommitted changes: should workers start from them?
    UncommittedChanges { files: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub id: CardId,
    pub conversation_id: ConversationId,
    /// The task that waits for the answer, if any.
    pub task_id: Option<TaskId>,
    /// The user request it belongs to.
    #[serde(default)]
    pub request_id: Option<String>,
    pub position: i64,
    pub kind: QuestionKind,
    pub text: String,
    /// Suggested answers; the user may also type one.
    pub options: Vec<String>,
    /// The suggested answer the asker recommends, by its index in `options`.
    #[serde(default)]
    pub recommended: Option<u32>,
    pub answer: Option<String>,
    pub created_at_ms: i64,
    pub answered_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlanStep {
    pub title: String,
    pub detail: Option<String>,
    /// The task carrying out this step, once delegated.
    pub task_id: Option<TaskId>,
}

/// Who approved a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PlanApprover {
    User,
    /// Approve for me, a plan the orchestrator did not mark risky: approved without review,
    /// and marked as such on the card.
    Brigadier,
    /// Approve for me, a plan of two or more steps or a risky one: approved after a
    /// cross-vendor plan review.
    Review,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PlanState {
    Proposed,
    /// Reviewers from another vendor are checking it (`task_id`: the first; all are on its
    /// gate).
    InReview {
        task_id: TaskId,
    },
    Approved {
        by: PlanApprover,
    },
    Rejected {
        message: Option<String>,
    },
    /// A newer plan replaced it.
    Superseded,
    /// Its review asked for changes: the orchestrator revises it (`propose_plan` with
    /// `revises`), answering each finding.
    Revising,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub id: CardId,
    pub conversation_id: ConversationId,
    /// The user request it belongs to.
    #[serde(default)]
    pub request_id: Option<String>,
    pub position: i64,
    pub title: String,
    pub steps: Vec<PlanStep>,
    /// Big, risky or architectural, as the orchestrator judged it.
    pub risky: bool,
    pub state: PlanState,
    /// Its current review round (reviewers from other vendors than the orchestrator's).
    #[serde(default)]
    pub gate: Option<Gate>,
    /// The plan it revises after that plan's review asked for changes.
    #[serde(default)]
    pub revises: Option<CardId>,
    /// Its answer to each finding of the plan it revises.
    #[serde(default)]
    pub responses: Vec<FindingResponse>,
    /// What the reviewers noted when they approved it.
    #[serde(default)]
    pub review_notes: Vec<String>,
    pub created_at_ms: i64,
    pub decided_at_ms: Option<i64>,
}

// ----- the message queue --------------------------------------------------------------------

/// A message waiting for the running turn (or a session's working answer) to end.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct QueuedMessage {
    pub id: String,
    pub text: String,
    pub attachments: Vec<AttachmentRef>,
    pub mentions: Vec<Mention>,
    pub queued_at_ms: i64,
    pub edited_at_ms: Option<i64>,
    /// Sent to a session while its answer works: the orchestrator is judging whether it
    /// belongs to that answer (it joins it) or is a request of its own (it waits here).
    #[serde(default)]
    pub deciding: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MessageQueue {
    /// In the order they will be sent.
    pub items: Vec<QueuedMessage>,
    /// The user interrupted the turn; nothing is sent until they resume.
    pub paused: bool,
}

// ----- the conversation's live state --------------------------------------------------------

/// What a conversation's model (the orchestrator, or a Chat's model) is doing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RunState {
    /// Not running (no CLI process, or between turns).
    #[default]
    Idle,
    /// Setting up the environment or starting the CLI.
    Starting,
    /// A turn is running.
    Running,
    /// Its CLI stopped; the next message resumes it.
    Hibernated,
    Failed,
}

/// What the sidebar shows about a conversation: whether it runs, and what waits for the user.
/// The UI keeps it current from the same events as the board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConversationActivity {
    pub conversation_id: ConversationId,
    pub run: RunState,
    /// Its tasks that are not over, with their state.
    pub tasks: Vec<(TaskId, TaskState)>,
    /// Approvals and plans waiting for the user's decision.
    pub approvals: Vec<CardId>,
    /// Questions not answered yet.
    pub questions: Vec<CardId>,
}

// ----- worker steps -----------------------------------------------------------------------

/// A turn in a worker's life, as the thread tells it ("task-2 finished").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum WorkerStepKind {
    Started,
    /// Its change waits for the user's approval, or for a landing the user can unblock.
    Waiting,
    /// The user interrupted it.
    Paused,
    /// Working again after waiting, a pause or its report.
    Resumed,
    /// It reported (or ended without anything to land).
    Finished,
    /// It reported again after it was sent back to work: its report (and change) changed.
    Updated,
    Landed,
    /// The orchestrator or a review turned it down.
    Rejected,
    Stopped,
    Failed,
}

impl WorkerStepKind {
    /// The step a task takes when its state goes from `was` (absent: it was just created) to
    /// `now`, if the thread shows one. `reported` tells whether it had reported before.
    pub fn between(was: Option<TaskState>, now: TaskState, reported: bool) -> Option<Self> {
        use TaskState as S;
        let waiting = |state: S| matches!(state, S::AwaitingApproval | S::ReadyToLand);
        // A review of its commit is not the worker working again.
        let working = |state: S| matches!(state, S::Queued | S::Starting | S::Running | S::Blocked);
        let Some(was) = was else {
            return Some(Self::Started);
        };
        if was == now {
            return None;
        }
        match now {
            S::Landed => Some(Self::Landed),
            S::Rejected => Some(Self::Rejected),
            S::Stopped => Some(Self::Stopped),
            S::Failed => Some(Self::Failed),
            // Back from a review, the reviewer's own row tells it.
            S::Reported | S::Done if !matches!(was, S::Reported | S::Done | S::Reviewing) => {
                Some(if reported {
                    Self::Updated
                } else {
                    Self::Finished
                })
            }
            S::Paused => Some(Self::Paused),
            _ if waiting(now) && !waiting(was) => Some(Self::Waiting),
            _ if working(now) && (waiting(was) || matches!(was, S::Paused | S::Reported)) => {
                Some(Self::Resumed)
            }
            _ => None,
        }
    }
}

/// One step of a worker, where it happened in the conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkerStep {
    pub task_id: TaskId,
    /// The user request the task belongs to.
    #[serde(default)]
    pub request_id: Option<String>,
    pub kind: WorkerStepKind,
    pub at_ms: i64,
    /// Where it happened in the conversation's stream (set when the board reads it).
    #[serde(default)]
    pub position: i64,
}

// ----- orchestrator steps -----------------------------------------------------------------

/// What the orchestrator (or a Chat's model) did that the thread tells as a grey row, in
/// its own words. What already shows by itself (a worker's own row, a card) has none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum OrchestratorStepKind {
    /// "Sent message to {worker}".
    Messaged { task_id: TaskId },
    /// "Read {worker}'s report".
    ReadReport { task_id: TaskId },
    /// "Read {artifact}".
    ReadArtifact { name: String },
    /// "Accepted {worker}'s change".
    Accepted { task_id: TaskId },
    /// "Searched the web for {query}" (a Chat).
    SearchedWeb { query: String },
    /// "Read {page}" (a Chat).
    ReadPage { url: String },
}

/// One step of the orchestrator, where it happened in the conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorStep {
    /// The user request the turn served.
    #[serde(default)]
    pub request_id: Option<String>,
    pub kind: OrchestratorStepKind,
    pub at_ms: i64,
    /// Where it happened in the conversation's stream (set when the board reads it).
    #[serde(default)]
    pub position: i64,
}

// ----- decided for you, waiting on you ----------------------------------------------------

/// What a decision made on the user's behalf is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DecisionSource {
    /// A task: its landing, a fix round, a permission declined, a stalled worker.
    Task { task_id: TaskId },
    /// A plan and its review.
    Plan { plan_id: CardId },
    /// A judgement call the orchestrator noted (`note_for_user`).
    Orchestrator,
    /// An overnight run's conductor: a phase verified, judged partial or moved past.
    Run {
        run_id: crate::model::OvernightRunId,
        phase_id: Option<String>,
    },
}

/// What a decision is, for where the app shows it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum DecisionKind {
    /// A decision of the work: a landing, a fix round, a plan, a judgement call.
    #[default]
    Routine,
    /// An overnight phase verified or settled: the run's card shows it, not the thread.
    PhaseOutcome,
}

/// Something decided on the user's behalf (under "Approve for me" and "Full access"), and
/// why: the summary card's "Decided for you", and a quiet row in the request's thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub id: String,
    /// The user request it was made for.
    #[serde(default)]
    pub request_id: Option<String>,
    pub source: DecisionSource,
    /// What was decided, in one line ("Landed task-3 “Add the flag”").
    pub what: String,
    /// Where the app shows it.
    #[serde(default)]
    pub kind: DecisionKind,
    /// Why, in a sentence or two; empty when the line says it all.
    #[serde(default)]
    pub why: String,
    pub at_ms: i64,
    /// Where it happened in the conversation's stream (set when the board reads it).
    #[serde(default)]
    pub position: i64,
    /// What the app and the morning report say: `what` and `why` in the current short words,
    /// also for a decision recorded in an earlier version's longer ones (set when the board
    /// reads it; a decision recorded now is in them already).
    #[serde(default)]
    pub short: Option<DecisionWords>,
}

/// A decision's line and reason as the user reads them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DecisionWords {
    pub what: String,
    pub why: String,
}

/// Where something only the user can do came from, which also says when it is over without
/// them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WaitingSource {
    /// A card nobody answered for long: it is over when the card is answered or expires.
    Card { card_id: CardId },
    /// A worker's report listed it under `needs_user`: it is over when the task is stopped,
    /// or a later report of it no longer lists it.
    Task { task_id: TaskId },
    /// The checks of a task's change can't run until the user does it (its verifier's
    /// `needs_user`): it is over when the task lands or ends, or a later round of its checks
    /// no longer lists it.
    Landing { task_id: TaskId },
    /// The orchestrator noted it (`note_for_user`).
    Orchestrator,
    /// An overnight run refused something only the user may do (an outward command, leaving
    /// the sandbox, landing despite failed checks): it stays listed for the run's report
    /// until the user marks it done.
    Run {
        run_id: crate::model::OvernightRunId,
        task_id: Option<TaskId>,
    },
}

/// Something only the user can do, listed under "Waiting on you" until they mark it done or
/// it is over without them. Its request waits while it is open; other work goes on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WaitingItem {
    pub id: String,
    /// The user request it holds up.
    #[serde(default)]
    pub request_id: Option<String>,
    pub source: WaitingSource,
    /// Its source and normalized text: an open item with the same key is the same item.
    pub key: String,
    /// What the user must do, in one line.
    pub what: String,
    pub created_at_ms: i64,
}

/// Who marked a "Waiting on you" item done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ResolvedBy {
    /// The user clicked Done.
    User,
    /// It was over without them: its card settled, its task stopped or landed, or the
    /// worker no longer listed it.
    Brigadier,
}

// ----- compactions -----------------------------------------------------------------------

/// Where a compaction stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CompactionState {
    Running,
    Done,
    Failed { error: String },
}

/// A Chat's model compacting its context: a "Compacting context" row, then "Context
/// compacted". A session's orchestrator never compacts (it starts afresh instead).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Compaction {
    pub id: String,
    /// The request whose turn it happened in (the model compacted on its own mid-turn);
    /// absent when the user asked for it between turns.
    #[serde(default)]
    pub request_id: Option<String>,
    /// The last message of the branch shown when it began: it shows after that message's
    /// answer, on that branch only.
    #[serde(default)]
    pub after: Option<String>,
    /// The model compacted on its own as its context filled up.
    pub automatic: bool,
    pub state: CompactionState,
    /// The context before and after, when the CLI says.
    pub tokens_before: Option<i64>,
    pub tokens_after: Option<i64>,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    /// Where it happened in the conversation's stream (set when the board reads it).
    #[serde(default)]
    pub position: i64,
}

// ----- user requests ----------------------------------------------------------------------

/// Where a user's request stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RequestState {
    /// The orchestrator (or a Chat's model) is on it, or a worker it started is.
    Working,
    /// Nothing runs: it waits for the user (a card, a paused worker, a landing on hold).
    Waiting,
    Done,
    /// The user stopped the reply.
    Stopped,
    Failed {
        error: String,
    },
}

/// What one user message set in motion. The message starts it; the model's replies, the tasks
/// delegated and the cards opened while serving it carry its id (`request_id`), so a thread
/// shows one block per request whatever order things finished in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UserRequest {
    /// The id of the user message that started it.
    pub id: String,
    pub conversation_id: ConversationId,
    /// The start of the user's message, on one line.
    pub preview: String,
    pub state: RequestState,
    pub started_at_ms: i64,
    /// When it last stopped working; absent while it works.
    pub ended_at_ms: Option<i64>,
    /// The request whose running turn its message was steered into: the thread shows it
    /// inside that request's block.
    #[serde(default)]
    pub steered_into: Option<String>,
    /// The reply that was streaming when it was steered in: its bubble shows after it.
    #[serde(default)]
    pub steered_after: Option<String>,
    /// The user's Undo of what its workers landed, once they used it.
    #[serde(default)]
    pub undo: Option<RequestUndo>,
}

/// The user's Undo and Reapply of what a request's workers landed (the turn diff card).
/// Each is a new commit, never a history rewrite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RequestUndo {
    /// Its changes are reverted now: true after Undo, false after Reapply.
    pub reverted: bool,
    /// The commits Undo and Reapply landed, oldest first; each reverts the one before it, the
    /// first the request's own landings.
    pub commits: Vec<String>,
}

/// A session checkout's state for the pinned card's Git actions and commit popover.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GitState {
    /// The branch checked out; absent on a detached HEAD.
    pub branch: Option<String>,
    /// What is staged.
    pub staged: DiffStat,
    /// Everything not committed yet, untracked files included.
    pub uncommitted: DiffStat,
    /// The remote the branch pushes to, if any.
    pub remote: Option<String>,
    pub upstream: Option<String>,
    /// Commits not pushed yet.
    pub ahead: u32,
}

/// The GitHub pull request of a session's branch (the pinned card's row).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    pub number: u32,
    pub title: String,
    pub url: String,
    pub state: PullRequestState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PullRequestState {
    Open,
    Draft,
    Merged,
    Closed,
}

/// The user's commit, as made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CommitOutcome {
    pub commit: String,
    pub message: String,
    pub branch: String,
    pub pushed: bool,
}

/// What the side panel's Review tab compares (the scope picker).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ReviewScope {
    /// What one request's workers landed. Absent: the latest request that landed anything.
    LastTurn { request_id: Option<String> },
    /// Everything not committed yet, untracked files included.
    Uncommitted,
    /// What is not staged yet, untracked files included.
    Unstaged,
    /// What is staged.
    Staged,
    /// One commit.
    Commit { commit: String },
    /// The session's branch since it left its base.
    Branch,
}

/// How a file changed, for the Review tab's badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ReviewFileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    TypeChanged,
    /// New and not tracked by git yet.
    Untracked,
}

/// One file of a review.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFile {
    pub path: String,
    /// A renamed file's old path.
    pub from: Option<String>,
    pub status: ReviewFileStatus,
    pub insertions: u32,
    pub deletions: u32,
    pub binary: bool,
}

/// A commit the Review tab offers under "Committed".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ReviewCommit {
    pub commit: String,
    pub subject: String,
    pub at_ms: i64,
}

/// The Review tab's diff: its files and their unified patch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ReviewDiff {
    /// The scope shown; "Last Turn" names the request it found.
    pub scope: ReviewScope,
    pub files: Vec<ReviewFile>,
    pub insertions: u32,
    pub deletions: u32,
    /// The unified text patch of every file, in `files`' order.
    pub patch: String,
    /// The patch carries whole files, so unchanged lines can be expanded.
    pub full_files: bool,
    /// The branch's latest commits, newest first.
    pub commits: Vec<ReviewCommit>,
    /// The branch compared ("Branch") and the one it left.
    pub branch: Option<String>,
    pub base: Option<String>,
}

/// A file of a session's checkout, as the Files tab shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutFile {
    /// Relative to the checkout's root.
    pub path: String,
    /// Its size in bytes.
    pub size: u64,
    /// Its text; absent for a binary file.
    pub text: Option<String>,
    /// Only the file's start was read.
    pub truncated: bool,
}

/// Why something entered the orchestrator's context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum InjectionKind {
    /// Brigadier's role instructions, once per CLI session.
    Instructions,
    UserMessage,
    Report,
    WorkerQuestion,
    /// A card was answered (approval, question, plan).
    Decision,
    TaskFailed,
    /// A tool's result (a task id, a short status).
    ToolResult,
    /// An artifact it asked to read.
    Artifact,
    /// The transcript a new orchestrator CLI session is seeded with.
    Reseed,
    /// The user resumed a request they had stopped.
    Resume,
    /// A follow-up the user sent while the answer worked, for the orchestrator to sort.
    FollowUp,
    /// The briefing a reborn orchestrator starts with.
    Briefing,
    /// Brigadier asks for an answer the orchestrator left out.
    Reminder,
    /// An overnight run hands the orchestrator a phase to lead, or its checks' outcome.
    Phase,
}

/// One thing Brigadier put into the orchestrator's context, for the Inspector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ContextInjection {
    pub kind: InjectionKind,
    pub bytes: u64,
    /// About four bytes per token.
    pub tokens_estimate: u64,
    /// A short description (`report task-3`, the first words of a message).
    pub label: String,
    pub task_id: Option<TaskId>,
    /// What this told the session of the parts of its instructions that can change while its
    /// CLI session lives on (role instructions, or a note about one of them). Logged only once
    /// the CLI took it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub told: Option<Told>,
}

/// What an orchestrator or Chat session was told of the parts of its instructions that can
/// change while its CLI session lives on: a resumed CLI keeps the instructions it started
/// with. `None`: not told (or not known).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Told {
    /// The version of the instructions' contract (whether Brigadier's notes replace them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<u32>,
    /// Today's date, `YYYY-MM-DD` (UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub today: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_replies: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<crate::model::PermissionLevel>,
    /// The overnight run's fingerprint (its branch, sandbox and restrictions); empty: no run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// The user's preferences' fingerprint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferences: Option<String>,
}

/// One entry of the orchestrator log shown in the Inspector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum OrchestratorEntry {
    Injection {
        injection: ContextInjection,
    },
    /// What the orchestrator's CLI reported.
    Provider {
        provider: ProviderKind,
        event: ProviderEvent,
    },
    /// The orchestrator was reborn: a fresh CLI session took over from a briefing.
    Rebirth {
        record: Box<crate::knowledge::RebirthRecord>,
    },
    /// Something broke the orchestrator's contract, such as its CLI compacting the context
    /// (PLAN.md §2: the orchestrator never compacts).
    ContractBreach {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_stored_before_done_when_still_reads() {
        let report: Report = serde_json::from_value(serde_json::json!({
            "summary": "Done.",
            "changes": ["a.rs"],
            "decisions": [],
            "verification": ["cargo test: ok"],
            "openQuestions": [],
            "verdict": null,
            "artifacts": [],
            "submittedAtMs": 1,
        }))
        .expect("an old report");
        assert!(report.done_when.is_empty() && report.risks.is_empty());
        assert!(report.needs_user.is_empty());
    }

    #[test]
    fn a_plan_in_review_stored_before_plan_gates_still_reads() {
        let plan: Plan = serde_json::from_value(serde_json::json!({
            "id": "p1",
            "conversationId": "c1",
            "position": 3,
            "title": "Rework the API",
            "steps": [{ "title": "Change it", "detail": null, "taskId": null }],
            "risky": true,
            "state": { "type": "inReview", "taskId": "t1" },
            "createdAtMs": 1,
            "decidedAtMs": null,
        }))
        .expect("an old plan");
        assert_eq!(
            plan.state,
            PlanState::InReview {
                task_id: TaskId("t1".into())
            }
        );
        assert!(plan.gate.is_none() && plan.revises.is_none());
        assert!(plan.responses.is_empty() && plan.review_notes.is_empty());
        let gate: Gate = serde_json::from_value(serde_json::json!({
            "round": 1,
            "members": [{ "taskId": "t2", "role": "review" }],
        }))
        .expect("an old gate");
        assert!(gate.findings.is_empty());
    }
}

#[cfg(test)]
mod attachment_tests {
    use super::*;

    #[test]
    fn old_attachment_defaults_to_non_inline() {
        let attachment: AttachmentRef = serde_json::from_str(
            r#"{"id":"hash","name":"photo.png","mime":"image/png","bytes":12}"#,
        )
        .unwrap();
        assert!(!attachment.inline);
        assert!(!attachment.pasted);
        let mut inline = attachment;
        inline.inline = true;
        let saved = serde_json::to_string(&inline).unwrap();
        assert_eq!(
            serde_json::from_str::<AttachmentRef>(&saved).unwrap(),
            inline
        );
    }
}
