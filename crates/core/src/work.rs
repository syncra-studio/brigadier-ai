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
use crate::model::{ConversationId, ModelChoice, ProjectId};

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
    /// An image pasted at `[Image #n]`. Zero reads legacy `[image:<id>]` messages.
    #[serde(default, deserialize_with = "read_inline_image")]
    pub inline: Option<u32>,
}

// Read both installed-main booleans and the AB branch's numbered references.
fn read_inline_image<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u32>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Saved {
        Number(u32),
        Legacy(bool),
    }
    Ok(
        Option::<Saved>::deserialize(deserializer)?.and_then(|saved| match saved {
            Saved::Number(n) => Some(n),
            Saved::Legacy(inline) => inline.then_some(0),
        }),
    )
}

// ----- tasks and workers ------------------------------------------------------------------

/// What a task does (PLAN.md §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
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
    /// Operates apps on the user's Mac; writes nothing that lands.
    Operate,
}

impl TaskKind {
    /// Whether the task's result is a change that lands as a commit.
    pub fn writes(self) -> bool {
        matches!(self, Self::Implement | Self::Merge)
    }
}

/// A task's part in a mockup first (SESSION-FIXES-PLAN.md Q10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MockupPart {
    /// Builds a static preview of the screen from the real components with fake data; its
    /// outputs' images are the mockup.
    Builds,
    /// Builds the real change to match the mockup `task` built; it was given that mockup's
    /// images, and its outputs' images are the live captures to compare with them.
    Matches { task: TaskId },
}

/// What a worker does in the request's flow, for its name in the thread ("Lead · Phase 1").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum WorkerRole {
    /// Leads a phase: outlines it when it is big, builds it and checks its own work.
    Lead,
    /// Works next to the lead on files of its own.
    Parallel,
    /// Checks a finished phase with fresh eyes, fixes what it finds and reports.
    Verifier,
    /// Fixes what a phase's verifier could not.
    Fix,
    /// Resolves conflicts between a phase's branch and where it lands.
    Merge,
    /// Reads a lead's outline or a phase's change for the other vendor (advisory).
    Reviewer,
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
/// the repository.
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
    /// The model a conversation's wait is for (it hit its limit), so only that model running
    /// again counts as its limit reset (#128), after a restart too. A task's wait has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub model: Option<ModelChoice>,
    /// The request whose turn waited, where its reset shows (#128). A task's wait has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub request: Option<String>,
    /// The reset first named for a conversation's wait, kept when a later look names another
    /// or none (another window still blocks it): once it passed, the model running again is
    /// its limit reset, not another account taking over first (#128). A task's wait has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub first_reset_ms: Option<i64>,
    /// The account that hit the limit a conversation's wait is for: its reset counts only
    /// once that account is free again, not when another one leads (#277). A task's wait has
    /// none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub account: Option<crate::accounts::AccountRef>,
    /// What kind of limit a conversation's wait is for: a spend control or credits never
    /// resumes on its own, only on the user's Continue now (MA5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit_kind: Option<brigadier_providers::LimitKind>,
    /// How often in a row the conversation went on by itself after a usage limit reset (MA5).
    #[serde(default, skip_serializing_if = "is_zero")]
    #[ts(skip)]
    pub tries: u32,
    /// The waiting messages the CLI had begun: the turn that goes on says to carry on with
    /// them instead of sending them again (MA5).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(skip)]
    pub landed: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
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
    /// The user continues the worker's own session in a terminal ("Open in terminal"): no
    /// headless CLI runs it until the terminal ends, then it reports what was done there.
    TakenOver,
    /// Its commits are being moved onto the target branch (`land_phase`), or its merge worker
    /// was sent back to finish its merge first.
    Landing,
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

/// A task state as stored, older names included: `reviewing` and `awaitingApproval` (the
/// per-change checks and landing card, gone) read as `landing`, which restart recovery treats
/// as an interrupted landing.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum StoredTaskState {
    Queued,
    Starting,
    Running,
    Blocked,
    Paused,
    Reported,
    TakenOver,
    Landing,
    Reviewing,
    AwaitingApproval,
    ReadyToLand,
    Landed,
    Done,
    Rejected,
    Stopped,
    Failed,
}

impl<'de> Deserialize<'de> for TaskState {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match StoredTaskState::deserialize(deserializer)? {
            StoredTaskState::Queued => Self::Queued,
            StoredTaskState::Starting => Self::Starting,
            StoredTaskState::Running => Self::Running,
            StoredTaskState::Blocked => Self::Blocked,
            StoredTaskState::Paused => Self::Paused,
            StoredTaskState::Reported => Self::Reported,
            StoredTaskState::TakenOver => Self::TakenOver,
            StoredTaskState::Landing
            | StoredTaskState::Reviewing
            | StoredTaskState::AwaitingApproval => Self::Landing,
            StoredTaskState::ReadyToLand => Self::ReadyToLand,
            StoredTaskState::Landed => Self::Landed,
            StoredTaskState::Done => Self::Done,
            StoredTaskState::Rejected => Self::Rejected,
            StoredTaskState::Stopped => Self::Stopped,
            StoredTaskState::Failed => Self::Failed,
        })
    }
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

/// A worker's session open in the user's terminal: what is handed back when it ends, and
/// what a restart finds (the terminal's process, ended if it survived).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Takeover {
    /// The worker's CLI session the terminal continues.
    #[ts(skip)]
    pub native_id: String,
    pub since_ms: i64,
    /// The task's state before; it goes back to it if the terminal never opened.
    pub from: TaskState,
    /// The terminal's process, once it runs.
    #[serde(default)]
    #[ts(skip)]
    pub pid: Option<u32>,
    #[serde(default)]
    #[ts(skip)]
    pub started_at_ms: Option<f64>,
    /// Recorded before a deliberate end closes the terminal (Stop, archive, delete): the task
    /// ends instead of being handed back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub ending: Option<String>,
    /// The orchestrator's messages while it is open, for the report request (kept here too,
    /// so a restart delivers them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(skip)]
    pub held: Vec<String>,
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
    /// A merge task: what it merges into its target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub merge: Option<MergeInputs>,
}

/// What a merge task merges into its target (SESSION-FIXES-PLAN.md Q2): its worker runs the
/// merge with git and commits it on its own branch; the landing checks that the target's tip
/// and `incoming` are both merged and nothing of either vanished, then fast-forwards the
/// target to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeInputs {
    /// The commit merged in: a conflicting task's work, or the session base's tip.
    pub incoming: String,
    /// Its name for the worker and the thread: "`main`", "task-3's work".
    pub label: String,
    /// The conflicting task's commits that `incoming` rebuilt without their litter, each with
    /// its replacement: a task branch at one of them counts as landed once its replacement has.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rebuilt: Vec<Rebuilt>,
}

/// A commit a merge task's start rewrote, and the commit that replaced it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rebuilt {
    pub from: String,
    pub to: String,
}

/// A structured report, the only part of a worker's work that enters the orchestrator's
/// context. About 800 tokens at most; details go to artifacts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub summary: String,
    pub changes: Vec<String>,
    pub decisions: Vec<String>,
    /// What the next worker in this area should know, each kept as a Brain fact
    /// (SESSION-FIXES-PLAN.md Q6.6).
    #[serde(default)]
    pub learned: Vec<String>,
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
    /// For merge tasks: each file removed on purpose, with why ("path: why").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(skip)]
    pub deleted: Vec<String>,
    /// For review tasks: the verdict.
    pub verdict: Option<ReviewVerdict>,
    /// For verify tasks: whether the project's checks passed.
    #[serde(default)]
    pub checks: Option<ChecksResult>,
    pub artifacts: Vec<ArtifactRef>,
    pub submitted_at_ms: i64,
}

/// What a report to Brigadier's developers is: something wrong, or an idea. Each opens its own
/// form in Brigadier's repository (the bug report, the proposal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FeedbackKind {
    Bug,
    Idea,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ReviewVerdict {
    Approve,
    RequestChanges,
}

/// What a verify task found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
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

/// The stored copy a file link resolves to once the folder it named is gone: a task's
/// output or report file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct StoredCopy {
    pub task_id: TaskId,
    pub artifact: ArtifactRef,
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

/// A command's output the thread's model got as a digest, kept whole in the blob store
/// (THREAD-PLAN.md Q4). The digest names its alias and the blob's file, which the model reads
/// with its own tools; it goes with the conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct StoredOutput {
    /// `out-<id>`, unique in the conversation.
    pub alias: String,
    /// Content hash in the blob store.
    pub blob: String,
    pub source: OutputSource,
    /// How the command ended ("exit 0", "exit 1", "timed out after 600 s").
    pub status: String,
    pub bytes: u64,
    pub lines: u64,
    /// What the model got instead, when it got a digest.
    #[serde(default)]
    pub shown_bytes: Option<u64>,
    pub at_ms: i64,
}

/// A file the session's thread read with its own tools (THREAD-PLAN.md Q8 lever 1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRead {
    /// Absolute, symlinks resolved (as written when the file is gone).
    pub path: String,
    /// The lines it got; `None` for the whole file, or a part the tool didn't tell.
    pub lines: Option<brigadier_providers::LineRange>,
    /// Outside the thread's workspace when it read it.
    pub outside: bool,
}

/// A search the session's thread made with its own tools.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSearch {
    pub kind: brigadier_providers::SearchKind,
    /// The text or file name pattern; `None` for a listing.
    pub pattern: Option<String>,
    /// The folder or file searched, absolute (the CLI's working directory when the tool
    /// didn't say).
    pub scope: String,
    /// The file filter (`Grep`'s `glob` or `type:<name>`).
    pub glob: Option<String>,
    /// The files it found that exist, absolute, at most
    /// [`crate::manager::reads::HITS_STORED`].
    pub hits: Vec<String>,
    /// Files found beyond those.
    #[serde(default)]
    pub more_hits: u32,
    /// Its scope is outside the thread's workspace.
    pub outside: bool,
}

/// Where a [`StoredOutput`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum OutputSource {
    /// A Claude thread's successful `Bash` call: its whole output.
    Bash,
    /// A Claude thread's failing `Bash` call: only the excerpt the CLI gives a failure's hook
    /// (at most about 10,000 characters of it), not the whole output.
    BashExcerpt,
    /// A Codex thread's `run`: its whole output, stdout and stderr together.
    Run,
    /// A preview's log (stdout and stderr together) as it stood when it was read or ended.
    Preview,
    /// A `run_check` command's whole output, stdout and stderr together (THREAD-PLAN.md Q8
    /// lever 3).
    Check,
    /// A Codex thread's own shell command's whole output, as it streamed, when it was longer
    /// than the CLI's event kept.
    Shell,
    /// One of the session's terminals, as `read_terminal` read it (Q90).
    Terminal,
}

// ----- previews -----------------------------------------------------------------------------

/// A process the thread or a worker started to show its work (a dev server, the app), owned by
/// the daemon and kept across the thread's turns, hibernation, seams and account switches
/// (THREAD-PLAN.md Q6). The thread's runs in the session's workspace and stops with the
/// session, its merge, a workspace change, `stop_preview`, the user's Stop and Brigadier's
/// quit; a worker's runs in its own folder and stops with that folder too.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    /// `preview-<n>`, unique in the conversation.
    pub id: String,
    pub conversation_id: ConversationId,
    /// What the thread called it (its command when it gave no name).
    pub name: String,
    pub command: String,
    /// The folder it runs in, inside `workspace`.
    pub workdir: String,
    /// The workspace it belongs to: a thread's preview stops when the thread's workspace
    /// changes; a worker's runs in the worker's worktree (or scratch folder).
    pub workspace: String,
    /// Its process, which leads its own process group.
    #[serde(default)]
    pub pid: Option<u32>,
    pub state: PreviewState,
    pub started_at_ms: i64,
    #[serde(default)]
    pub ended_at_ms: Option<i64>,
    /// The latest snapshot of its log (`out-<id>`, a stored output).
    #[serde(default)]
    pub log: Option<String>,
    /// A local HTTP URL printed by this preview; never for an app preview, whose window is
    /// what the user looks at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub url: Option<String>,
    #[serde(default)]
    pub kind: PreviewKind,
    /// What it shows when that isn't a URL: a simulator's device ("iPhone 16"), a PDF's file,
    /// a test run's command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target: Option<String>,
    /// The worker that started it, in its own worktree or scratch folder; none for the
    /// thread's. A worker's preview stops with its task's folder, not with the thread's
    /// workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_id: Option<TaskId>,
    /// While it is starting, its log shows a build under way (compiling, bundling) that
    /// hasn't finished: a dev app built and launched in one command reads Building, then
    /// Starting.
    #[serde(default)]
    pub building: bool,
    /// The variables the agent gave it, for Start again (Brigadier's own are added again).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    #[ts(skip)]
    pub env: std::collections::BTreeMap<String, String>,
    /// The request whose turn started it (a worker's: its task's), kept when it starts again,
    /// so its card stays on that turn; none when no turn ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub request_id: Option<String>,
    /// A rendered PDF's page count, read from its file when it was ready and again when its
    /// render ended; none when it couldn't be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pages: Option<u32>,
}

/// What a proof is, which decides how it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ProofKind {
    /// A picture of a page, a window or a device's screen.
    Screenshot,
    /// A short video of the change used.
    Recording,
    /// A program's output (a preview's log, a command's).
    Log,
    /// A test run's output.
    Test,
}

/// What a preview shows the user, which decides when it is ready and whether it has a URL.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PreviewKind {
    /// A web page (a dev server, a docs site): ready once its URL or a port it listens on
    /// answers, or once it opens a window.
    #[default]
    Web,
    /// A desktop app that opens its own window (`tauri dev`, Electron): ready once it does.
    App,
    /// An app run in an iOS simulator (`xcrun simctl`, `xcodebuild` for a simulator): running
    /// once started; its device is its `target`.
    Simulator,
    /// A rendered PDF: ready once its file (its `target`) is there.
    Pdf,
    /// A test run: running once started, it ends with its result; its log is its output.
    Test,
}

/// Where a preview stands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PreviewState {
    /// Its process runs but what it shows isn't ready yet (still building, no window yet).
    Starting,
    Running,
    /// The user suspended its process group; it still belongs to the session.
    Paused,
    /// It ended on its own: `status` as "exit 1" or "killed by signal 9".
    Exited {
        code: Option<i32>,
        status: String,
    },
    /// Brigadier stopped it, for `reason` ("stopped by the thread", "the session closed").
    Stopped {
        reason: String,
    },
}

impl PreviewState {
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Paused)
    }

    /// Its process runs (starting or ready), not paused or ended.
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }
}

/// Lines added and removed, per file and in total.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FileStat {
    pub path: String,
    pub insertions: u32,
    pub deletions: u32,
    /// Binary files have no line counts.
    pub binary: bool,
    /// Whether the change created, changed or deleted the file, where a thread's own file
    /// change says (its row reads Created, Edited or Deleted); absent elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub change: Option<brigadier_providers::FileChangeKind>,
    /// A renamed file's old path, where a thread's own file change moved it (its row reads
    /// Renamed); `path` is where it went.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub from: Option<String>,
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

/// What a checking task did in its gate round, as stores from before the phase flow keep it.
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
    /// A whole phase of an older overnight run: its candidate was the run branch's tip.
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
    /// Set while it is paused waiting for quota. Boxed: it is rarely set, and engine futures
    /// hold several tasks (`run_orchestrator_call` stays under 16 KB).
    #[serde(default)]
    pub quota_wait: Option<Box<QuotaWait>>,
    /// The task this one reviews or merges.
    pub subject: Option<TaskId>,
    /// An operate task's app, dev build or URL, and the window if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target: Option<String>,
    /// What an operate task must leave true, checkable on screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub end_state: Option<String>,
    /// Its part in a mockup first (SESSION-FIXES-PLAN.md Q10): it builds the mockup, or the
    /// real work that must match one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub mockup: Option<MockupPart>,
    /// It makes designs or images (design files, a mockup, generated images): routing files
    /// it as design work (see `manager::workers::category`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(skip)]
    pub design: bool,
    /// The plan this one reviews.
    pub plan: Option<CardId>,
    pub attachments: Vec<AttachmentRef>,
    pub workspace: Option<TaskWorkspace>,
    pub report: Option<Report>,
    /// Older tasks: the single commit the per-change checks were built on.
    pub candidate: Option<Candidate>,
    /// A reviewer or verifier of an overnight phase's checks: the round it belongs to.
    #[serde(default)]
    pub gate_link: Option<GateLink>,
    /// Set while its work is being landed, or its merge worker was sent back to finish it:
    /// its next report lands on its own (fast-forward only, never reviewed again). Older
    /// tasks: the commit message it was accepted with.
    #[serde(default)]
    pub landing: Option<String>,
    /// The target's tip its commits were put on top of, recorded right before the target
    /// moves: a restart that cuts the landing off finds them landed from it (BUILD-PLAN D1.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub landing_from: Option<String>,
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
    /// Its part in the request's flow; absent for scouts, research and older tasks.
    #[serde(default)]
    pub role: Option<WorkerRole>,
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
    /// The latest version a report of it was given for its Brain claims (BUILD-PLAN A11): a
    /// report after a restart goes on from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub report_version: Option<u64>,
    /// How its CLI ended without a report since it last reported ("signal 9: …"): the first
    /// time it resumes, the second time the task stops (SESSION-FIXES-PLAN.md Q4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(skip)]
    pub deaths: Vec<String>,
    /// It looks at the integrated tree's own dev build, live, for the visible changes that
    /// landed: the one live check of the app, instead of one per worker (§11).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub live_look: bool,
    /// A fresh verifier's candidate commit: its worktree starts there, and it gets no
    /// outline, report, Brain section or context pack of the build, only its spec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub fresh_commit: Option<String>,
    /// The catalog ids of the MCP servers it gets besides its agent's own (`delegate_task`'s
    /// `integrations`; `manager::integrations`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(skip)]
    pub integrations: Vec<String>,
    /// What it took from and gave to the project's Brain (SESSION-FIXES-PLAN.md Q6.6).
    #[serde(default)]
    pub brain: BrainUse,
    /// While its CLI session starts again after it was interrupted.
    #[serde(default)]
    pub resuming: bool,
    /// While the user has the worker's session open in a terminal ([`TaskState::TakenOver`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub takeover: Option<Takeover>,
    /// It took a trial slot when created and waits to start: it keeps the slot until then.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(skip)]
    pub trial_slot: bool,
    /// It reruns a cheaper model's scout or research task once, one tier up: that task, the
    /// model that ran it and why. A rerun never reruns again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rerun: Option<brigadier_router::Rerun>,
    /// A plain CLI worker its session started with `brigadier start` (STRIP-DOWN-PLAN S13):
    /// how it was asked to run. Brigadier starts, messages, waits on and closes it, and
    /// nothing more: no brief, no report, no hand-off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub plain: Option<PlainWorker>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// How a `brigadier start` worker runs ([`Task::plain`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlainWorker {
    /// Its CLI's permission mode: its session's unless `--permission` said (Q8).
    pub permission: crate::model::PermissionLevel,
    /// It works in its session's checkout (`--local`; a Chat's worker: `repo`'s), not a
    /// worktree of its own.
    #[serde(default)]
    pub local: bool,
    /// A Chat's worker: the repository it works in (`--repo`, S28). A Chat's worker without
    /// one works in a folder of its own, with no git; a session's works in the session's
    /// repository.
    #[serde(default)]
    pub repo: Option<String>,
    /// The branch its worktree starts from and merges back into (G21); none with `--local`.
    #[serde(default)]
    pub base: Option<String>,
    /// The MCP servers it was granted (`--mcp`, D18), each by the name its CLI knows it by;
    /// it has none of the user's others, nor their skills (D13). Its successors and resumes
    /// keep them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcps: Vec<String>,
    /// It has the computer tools while Settings › Computer use is on (`--with computer-use`,
    /// S35); no other worker has them.
    #[serde(default)]
    pub computer_use: bool,
    /// `brigadier close` ended it: `brigadier workers` lists it no more.
    #[serde(default)]
    pub closed: bool,
}

/// A worker's use of the project's Brain, shown on its row and in the Inspector
/// (SESSION-FIXES-PLAN.md Q6.6).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrainUse {
    /// Its `query_brain` calls.
    pub queries: u32,
    /// The Brain facts its brief carried.
    pub facts_in: u32,
    /// The facts it wrote back (`learned` and decisions), counted once the Brain kept them.
    pub facts_out: u32,
    /// Its tool calls and commands: a long task with no query and no write-back shows "low
    /// Brain use".
    pub calls: u32,
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
        /// Brigadier keeps allowing the request's scope for this conversation.
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
    /// An outward command stopped by the command gate of earlier versions. Only in recorded
    /// conversations: nothing asks this way any more.
    OutwardCommand { argv: Vec<String>, cwd: String },
    /// Ask: land a reviewed task on its branch.
    Landing {
        task_id: TaskId,
        branch: String,
        diff_stat: DiffStat,
    },
    /// Merge the session branch into its base. Only in recorded conversations: the user asks
    /// for the merge in words now (`finish_session`).
    FinishSession {
        branch: String,
        base: String,
        commits: u32,
        diff_stat: DiffStat,
    },
    /// An action the orchestrator asked the user to approve.
    Action { action: String, details: String },
    /// Ask: start a phase from its lead's outline. Only in recorded conversations: leads post no
    /// outlines any more.
    Outline {
        task_id: TaskId,
        title: String,
        outline: String,
    },
    /// Ask: send the thread's message to another session ("Send a message to
    /// ‹title› in ‹project›?"), exactly as written.
    PeerMessage {
        message_id: String,
        to: ConversationId,
        title: String,
        project: String,
        text: String,
    },
    /// Ask first (Q67): an agent's action in one of its Browser tabs pays, signs in, enters a
    /// password, card or one-time code, or sends something in the user's name.
    Browser {
        tab: String,
        url: String,
        /// The page it was asked on: a yes holds there only.
        generation: u64,
        sensitive: crate::browser::BrowserSensitive,
        /// The question, in plain words ("Pay on shop.example?").
        question: String,
        /// What the agent would do there ("Clicks button “Pay $5”.").
        details: String,
        /// The very call asked about: a yes holds for it alone.
        fingerprint: String,
    },
    /// The plan the session's CLI proposed from its own plan mode (harness D7): "Build it"
    /// takes the session out of plan mode and tells it to go ahead; the user may reply instead.
    Plan { plan: String },
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
    /// Local checkout with uncommitted changes: should workers start from them? `files` are
    /// the user's own; `thread_files` those the thread's own edits left uncommitted (#88).
    UncommittedChanges {
        files: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[ts(as = "Option<Vec<String>>", optional)]
        thread_files: Vec<String>,
    },
    /// A merge card from before BUILD-PLAN A4 (`propose_merge`, retired): none is made any
    /// more, and no answer on one is consent. Kept so stored cards still load.
    Merge {
        branch: String,
        base: String,
        /// The branch conflicted with the base when the card was made.
        #[serde(default)]
        conflicted: bool,
        /// The files it conflicted in, the first few (none when git couldn't pin it to one).
        #[serde(default)]
        conflicts: Vec<String>,
    },
}

/// One answer a question suggests: a short label, and a line on what it means.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct QuestionOption {
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// One question of a round the user answers at once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct QuestionItem {
    pub text: String,
    #[serde(default)]
    pub options: Vec<QuestionOption>,
    /// The option the asker recommends, by its index in `options`.
    #[serde(default)]
    pub recommended: Option<u32>,
    /// The user may pick several options: its answer names each, joined with ", " (a CLI's
    /// own multi-select question, harness D7).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub multi_select: bool,
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
    /// The question; for a round, its questions one per line.
    pub text: String,
    /// Suggested answers; the user may also type one. A round's are in its `items`.
    pub options: Vec<String>,
    /// The suggested answer the asker recommends, by its index in `options`.
    #[serde(default)]
    pub recommended: Option<u32>,
    /// A round's questions, answered together. Empty for a single question (`text` and
    /// `options`), as every card stored before rounds is.
    #[serde(default)]
    pub items: Vec<QuestionItem>,
    /// The answer as the asker reads it; for a round, each question with its answer.
    pub answer: Option<String>,
    /// A round's answers, one per item, once answered.
    #[serde(default)]
    pub answers: Vec<String>,
    /// A few words over the card, for a round of a series ("Round 2 · Budget").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
    pub created_at_ms: i64,
    pub answered_at_ms: Option<i64>,
    /// A merge card the user answered in their own words: the user message they went as (a
    /// new request), its queued message while it waits, or the card's own id while they are on
    /// their way. Its answer is "Not yet", and that message is never consent to merge nor the user
    /// writing since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub words_id: Option<String>,
    /// Where its answer came from: the client that sent it and how the user gave it. Cards
    /// answered before this was kept have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub answered_by: Option<AnswerOrigin>,
}

/// How the user gave a card's answer, as the app saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum AnswerInput {
    /// A click on an answer or a button.
    Click,
    /// Enter (or Space) on the focused card or one of its buttons.
    Enter,
    /// An answer's digit, on the focused card.
    Digit,
    /// Words typed in the card's own field.
    Typed,
}

/// Where a card's answer came from: what to read when an answer nobody remembers giving turns
/// up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AnswerOrigin {
    /// How the user gave it, when the client said.
    pub input: Option<AnswerInput>,
    /// The client that sent it, by the name it connected with ("Brigadier" for the app).
    pub client: String,
    /// That client's process.
    pub pid: u32,
}

impl Question {
    /// Its questions: a round's items, or the single question it asks.
    pub fn round(&self) -> Vec<QuestionItem> {
        if !self.items.is_empty() {
            return self.items.clone();
        }
        vec![QuestionItem {
            multi_select: false,
            text: self.text.clone(),
            options: self
                .options
                .iter()
                .map(|label| QuestionOption {
                    label: label.clone(),
                    description: None,
                })
                .collect(),
            recommended: self.recommended,
        }]
    }

    /// Whether it still waits for the user.
    pub fn is_open(&self) -> bool {
        self.answer.is_none() && self.answered_at_ms.is_none()
    }
}

/// The base's upstream at a session's merge (#224): the merge isn't pushed to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MergedUpstream {
    /// Its name (`origin/main`).
    pub name: String,
    /// Commits it had that the merged base lacks: the base had diverged from it.
    pub behind: u32,
}

/// What the user's consent to the session's merge waits for (SESSION-FIXES-PLAN.md Q3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum MergeCondition {
    /// Nothing: the merge goes as soon as no worker writes.
    Now,
    /// "merge when done": the running work ending, said in words.
    WorkersFinish,
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
    /// Sent while the session's turn works: the session has it and judges it with `brigadier
    /// queue` (run2-D40); it waits here, Deciding, until it is judged or joins by itself.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub deciding: bool,
    /// The session already has its words (given to it to judge): it can't be edited any more,
    /// and the turn it goes on later names it (run2-D46).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub delivered: bool,
    /// The thread's command it waits behind while Deciding: the session reads it once that
    /// command ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub held_by: Option<HeldBy>,
    /// The session judged it could run apart (`brigadier queue <id> new-session`): it waits for
    /// the user's pick, "Open as a new session" or "Run here", and is never sent until then.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub offer_session: bool,
}

/// A queued message the session sent on to one of its workers (`brigadier queue <id> worker
/// <worker-id>`, run2-D40/D38): the one-line notice the rail keeps until the user dismisses it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SentToWorker {
    /// The queued message's id.
    pub id: String,
    pub text: String,
    pub worker: TaskId,
    /// The worker's number and title then.
    pub worker_number: u32,
    pub worker_title: String,
    pub sent_at_ms: i64,
}

/// The thread's command that a message sent into its running turn waits behind: the thread
/// can't read it before the command ends, so the app says when it will be read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HeldBy {
    /// The command's own words.
    pub command: String,
    pub since_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MessageQueue {
    /// In the order they will be sent.
    pub items: Vec<QueuedMessage>,
    /// The user interrupted the turn; nothing is sent until they resume.
    pub paused: bool,
    /// What the session sent on to its workers: notices, never sent from here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<SentToWorker>>", optional)]
    pub sent: Vec<SentToWorker>,
}

impl MessageQueue {
    /// The next message in queue order, past those waiting for the user's pick on a new
    /// session offer.
    pub fn next_to_send(&self) -> Option<&QueuedMessage> {
        self.items.iter().find(|item| !item.offer_session)
    }

    /// The next message to send now: none while the next one is still Deciding.
    pub fn ready_to_send(&self) -> Option<&QueuedMessage> {
        self.next_to_send().filter(|item| !item.deciding)
    }
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

/// What the sidebar shows about a conversation: whether its CLI works, and what waits for the
/// user. The UI keeps it current from the same events as the board, and the CLI's turns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConversationActivity {
    pub conversation_id: ConversationId,
    pub run: RunState,
    /// Its CLI's turn is open.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub turn_open: bool,
    /// One of its own workers has an open CLI turn, as `WorkerRow::working` reads it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub workers_working: bool,
    /// Its CLI's background commands hold its turn, or one's end owes it a turn
    /// (`thread_view::CliJobs`, D21).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub commands_working: bool,
    /// When its last recorded CLI turn ended; absent before any turn ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub turn_ended_at_ms: Option<i64>,
    /// Its tasks that are not over, with their state.
    pub tasks: Vec<(TaskId, TaskState)>,
    /// The titles of those tasks, for their rows under the session's row.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<(TaskId, String)>>", optional)]
    pub titles: Vec<(TaskId, String)>,
    /// Its CLI's permission prompts and plans waiting for the user's decision.
    pub approvals: Vec<CardId>,
    /// Questions not answered yet.
    pub questions: Vec<CardId>,
    /// Its CLI's MCP servers that want the user to sign in, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<String>>", optional)]
    pub sign_ins: Vec<String>,
    /// Nothing works, and its CLI did not take the turn or its last turn failed or was cut
    /// off: it needs the user.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub failed: bool,
    /// Nothing works, and its CLI's last turn was stopped by the user.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub stopped: bool,
}

/// Where a session stands, as its chat tab and its sidebar row show it; the first
/// that holds wins ([`crate::board::Board::tab_state`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TabState {
    /// An open approval, plan, question or MCP sign-in, or its last turn failed.
    NeedsYou,
    /// Its CLI's turn is open or starting, one of its workers works, or its messages wait for a usage limit to reset.
    Working,
    /// Idle, with commits its base doesn't have since its last merge.
    Ready,
    /// Its work merged, and nothing newer is unmerged.
    Merged,
    /// Its latest merge was reverted on the base, and nothing newer is unmerged.
    Reverted,
}

/// What became of a merge's work on its base since it merged, as git shows it now: when it was
/// reverted, and when it was put back after that (Redo), if it was. Asked of git each time,
/// so a revert or redo made anywhere (the thread, a terminal, GitHub) counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MergeUndo {
    /// The commit on the base that reverted the merge's work last ("Undone on main · 9:41").
    pub reverted_by: String,
    /// That commit's commit time.
    pub reverted_at_ms: i64,
    /// The commit that reverted that revert (Redo), putting the work back; none while the
    /// work stays reverted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub redone_by: Option<String>,
    /// That commit's commit time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub redone_at_ms: Option<i64>,
}

/// A tab that is ready or merged, whatever runs in it now: the part of its state the app can't
/// fold from events alone (it needs git), read once at connect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SettledTab {
    pub conversation_id: ConversationId,
    pub state: TabState,
    /// Its branch's pull request as last seen on GitHub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pull_request: Option<PullRequestState>,
    /// The thread attached proof (`attach_proof`) since its last merge.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub proof: bool,
    /// How many commits its session branch has that its base lacks ("On ‹branch› · 2 commits
    /// ahead"); none when it has no branch of its own or git couldn't tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ahead: Option<u32>,
}

impl TabState {
    /// The state in the thread's words (the board, a [follow-up …] block).
    pub fn words(self) -> &'static str {
        match self {
            Self::NeedsYou => "needs the user",
            Self::Working => "working",
            Self::Ready => "ready to merge",
            Self::Merged => "merged",
            Self::Reverted => "merged, then reverted",
        }
    }
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
        let waiting = |state: S| matches!(state, S::ReadyToLand);
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
            S::Reported | S::Done if !matches!(was, S::Reported | S::Done | S::Landing) => {
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

/// What a thread's tool call is, as the transcript item a worker's would fold into: a
/// command (its output, exit and duration), a file change (its files' diffs and lines in and
/// out), an image it looked at (its path), a web search or page read (its query or URL), or
/// any other tool (its input and output).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ThreadItemKind {
    Command,
    Files,
    Image,
    Web,
    Tool,
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

/// The most bytes of a message an orchestrator step keeps.
pub const STEP_TEXT_MAX: usize = 2000;

/// What the orchestrator (or a Chat's model) did that the thread tells as a grey row, in
/// its own words. What already shows by itself (a worker's own row, a card) has none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum OrchestratorStepKind {
    /// A provider tool call, visible as soon as its input starts streaming.
    Tool {
        item_id: String,
        name: String,
        detail: Option<String>,
        status: brigadier_providers::ItemStatus,
        /// It failed only because the user's Stop ended it (its process killed with the
        /// turn): its row reads stopped, not failed.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        #[ts(as = "Option<bool>", optional)]
        stopped: bool,
        // Latest folded update: concurrent view replay must not regress this call.
        #[serde(default)]
        through_position: i64,
        /// When the call finished (any status but in progress); the step's `at_ms` is when it
        /// started.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        ended_at_ms: Option<i64>,
        /// How a command ended, when it says: its exit code.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        exit: Option<i32>,
        /// What the call is as a transcript item (`listThreadItems` returns it in full).
        /// `None` in steps from before it was kept.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        item: Option<ThreadItemKind>,
        /// A file change's files with their lines in and out (none counted: 0 and 0).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        files: Option<Vec<FileStat>>,
        /// How much output a command has printed so far, while it streams (Codex; Claude's
        /// command output arrives at completion), updated at most once a second. The output
        /// itself is read with `listThreadItems`, not kept here.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        output_bytes: Option<u64>,
        /// The `out-…` alias its whole output is stored under, when its event holds less of it
        /// (a digest, or the CLI's clip).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        full_output: Option<String>,
        /// The orchestrator log entry (its stream sequence in `orch:<id>`) with the call's
        /// newest event, which `listThreadItems` reads.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        log_seq: Option<i64>,
        /// The turn it ran in: the orchestrator log entry of the turn's start.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        turn: Option<i64>,
    },
    /// "Sent message to {worker}".
    Messaged {
        task_id: TaskId,
        /// What it sent (`message_worker`), cut to 2000 bytes; none in
        /// steps from before it was kept.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        text: Option<String>,
    },
    /// "Stopped {worker}": the orchestrator's `stop_worker`, or the user's Stop all.
    Stopped { task_id: TaskId, reason: String },
    /// "Reviewed {worker}'s change": an older Brigadier's one-shot code review of the tasks'
    /// work ended with a verdict (clean, or `findings` problems found). Nothing records it now.
    Reviewed {
        task_ids: Vec<TaskId>,
        findings: u32,
    },
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
    /// "Created {worker} with the instructions: …" (the task holds its spec).
    Created { task_id: TaskId },
    /// "Answered {worker}: {answer} — {why}".
    Answered {
        task_id: TaskId,
        question: String,
        answer: String,
        why: String,
    },
    /// "Landed {commits} commits on {branch}".
    Landed {
        task_ids: Vec<TaskId>,
        commits: u32,
        branch: String,
        head: String,
    },
    /// A screenshot, recording, log or test output the thread attached to its reply
    /// (`attach_proof`): the merge message's proof, read with `readThreadImage` by its `id`.
    Proof {
        id: String,
        /// Its file name as the thread gave it.
        name: String,
        mime: String,
        /// What it shows, in the thread's words ("Added a todo with a due date").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        caption: Option<String>,
        /// Brigadier's copy.
        #[ts(skip)]
        file: String,
        /// What it is; none on proofs from before it was kept (read it from `mime` then).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        kind: Option<ProofKind>,
        /// Where it was taken: the page's URL, the device, the file or the command.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        taken_on: Option<String>,
        /// The preview it was taken of (`preview-2`), when it was one of the session's.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        preview: Option<String>,
        /// When it was taken: the screenshot's own time, else the file's last change.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        taken_at_ms: Option<i64>,
    },
    /// "Merged {branch} into {base}": the user asked for it in the thread.
    Merged {
        branch: String,
        base: String,
        #[serde(default)]
        commits: u32,
        /// The user message whose words asked for it: it asks for no other merge.
        #[serde(default)]
        asked_in: Option<String>,
        /// The base's upstream when it merged (#224): the merge is on this Mac only, not pushed
        /// to it. None: the base has no upstream.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        upstream: Option<MergedUpstream>,
        /// The session branch's head that merged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        head: Option<String>,
        /// The commit the merge made on the base (a squash or rebase merge's too).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        merge_commit: Option<String>,
        /// The pull request it merged on GitHub; none for a merge on this Mac.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        pr: Option<u32>,
        /// The requests whose work it merged.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[ts(as = "Option<Vec<String>>", optional)]
        requests: Vec<String>,
    },
    /// A notice an earlier version showed when something the user asked for needed more than
    /// its sandbox allowed (`suggest_full_access`, gone since S6): kept so old records load;
    /// nothing writes it now.
    FullAccessSuggested { reason: String },
    /// A card with a change of one of the few settings the thread may suggest
    /// (`suggest_setting`) and one button that makes it; nothing changes until it is pressed.
    SettingSuggested {
        change: SettingChange,
        reason: String,
    },
    /// A report to Brigadier's developers the thread drafted when the user seemed annoyed with
    /// Brigadier (`offer_feedback`), for them to send, edit first or dismiss. Open until a
    /// `FeedbackAnswered` with its `id`.
    FeedbackOffered {
        id: String,
        kind: FeedbackKind,
        title: String,
        body: String,
    },
    /// The user's answer to the feedback offer `id`: opened on GitHub (`sent`) or dismissed.
    /// Sent, what became of its reports (a Try again that files the rest answers again, in
    /// this one's place).
    FeedbackAnswered {
        id: String,
        sent: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[ts(as = "Option<Vec<crate::feedback::FeedbackSent>>", optional)]
        results: Vec<crate::feedback::FeedbackSent>,
    },
    /// Work outside the request the thread offered as a session of its own
    /// (`offer_session`): a card with one "Start as session" button. Open until a
    /// `SessionOfferTaken` with its `id`.
    SessionOffered {
        id: String,
        title: String,
        task: String,
    },
    /// The user started the offer `id` as the session `session`.
    SessionOfferTaken { id: String, session: ConversationId },
    /// An older Brigadier's merge, held until no worker wrote ("Merging after N workers
    /// finish"). Nothing holds a merge now; stored ones are read.
    MergeHeld {
        branch: String,
        base: String,
        /// The user message, or "Merge into main" click (a merge card's, before BUILD-PLAN
        /// A4), whose consent it holds.
        asked_in: String,
        /// When the user consented.
        consented_at_ms: i64,
        condition: MergeCondition,
        /// The write tasks that ran when it was held.
        waiting: Vec<TaskId>,
        /// The branch's tip the consent covers: the tip when it was held, moved on by each of
        /// the session's own landings on it. Other work on the branch isn't covered.
        #[serde(default)]
        tip: String,
    },
    /// The user clicked "Merge into main" on the side panel (BUILD-PLAN A4): the thread was
    /// told to merge the session branch into its base. `id` is what a merge on it names as
    /// `asked_in`.
    MergeRequested {
        id: String,
        branch: String,
        base: String,
        /// Who sent the click: the client and how.
        origin: AnswerOrigin,
    },
    /// The user cancelled an older Brigadier's held merge. Nothing records it now.
    MergeCancelled { asked_in: String },
    /// An older Brigadier's held merge stopped without merging. Nothing records it now.
    MergeStopped { asked_in: String, reason: String },
    /// The CI watch of a push (`watch_ci`, SESSION-FIXES-PLAN.md Q1), as it goes: one row per
    /// push, updated in place, "Checking CI…" while `result` is none.
    CiWatch {
        /// `owner/name` on GitHub.
        repo: String,
        branch: String,
        sha: String,
        /// No run has started yet: "Waiting for CI to start…".
        waiting: bool,
        runs: Vec<CiRun>,
        /// How it ended: "passed", "failed", "no runs" or "stopped" (gh went away, or the
        /// session closed).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        result: Option<String>,
    },
    /// "Couldn't message ‹title›": a `message_session` the engine refused before anything was
    /// recorded (an archived, unknown or own target, the hop or rate rule, a bad `reply_to`,
    /// an empty text). A send that was recorded shows from its record instead
    /// (SESSION-MESSAGING-PLAN.md §6.1).
    MessageRefused {
        /// The session it was for, when the id named one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        to: Option<ConversationId>,
        /// Its title then, when the id named one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        title: Option<String>,
        /// Why, in the words the thread heard.
        reason: String,
    },
    /// The usage limit a turn waited for reset and the turn went on (#128): a divider across
    /// the thread where its work went on, "Claude usage limit reset · 9:39 PM".
    QuotaResumed { provider: ProviderKind },
    /// A moment of the conversation that is no work of its own (#506): a usage limit, a move
    /// to another model, provider or account, the chosen model back. The thread says it across
    /// the column as a divider where it happened.
    Moment { text: String },
}

impl OrchestratorStepKind {
    /// Whether this step ends a held merge on the consent given in `asked`: merged on it,
    /// cancelled or stopped.
    pub fn ends_merge(&self, asked: &str) -> bool {
        match self {
            Self::Merged {
                asked_in: Some(done),
                ..
            } => done == asked,
            Self::MergeCancelled { asked_in } | Self::MergeStopped { asked_in, .. } => {
                asked_in == asked
            }
            _ => false,
        }
    }
}

/// One GitHub Actions run of a watched push.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CiRun {
    pub workflow: String,
    /// `queued`, `in_progress`, `completed`.
    pub status: String,
    /// Once completed: `success`, `failure`, …
    pub conclusion: String,
    pub url: String,
    pub jobs_done: u32,
    pub jobs_total: u32,
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
    /// An overnight run: a phase the thread settled, or the run it ended.
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
    /// The orchestrator answered a worker's question: the thread shows it as an "Answered"
    /// row, the morning report lists it.
    Answer,
}

/// Something decided on the user's behalf (under "Accept edits" and "Full access"), and
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
    /// When it worked, as recorded before S16b; no longer kept.
    #[serde(default)]
    pub worked: Vec<WorkSpan>,
    /// When it waited for its own question card, as recorded before S16b; no longer kept.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<WorkSpan>>", optional)]
    pub waited: Vec<WorkSpan>,
    /// When it stood interrupted after a quit, as recorded before S16b; no longer kept.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<WorkSpan>>", optional)]
    pub closed: Vec<WorkSpan>,
    /// It waited only for quota, as recorded before S16b; no longer kept.
    #[serde(default)]
    pub quota_wait: bool,
    /// The sessions whose replies it waited for, as recorded before S16b; no longer kept.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<ConversationId>>", optional)]
    pub waiting_for: Vec<ConversationId>,
    /// The session whose message started it (not the user).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub from_peer: Option<ConversationId>,
    /// Steered into a turn while the thread's command runs: the thread reads it once that
    /// command ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub held_by: Option<HeldBy>,
    /// While it works: how many of the orchestrator's own background jobs (a command it runs
    /// in the background) still run for it. Their end starts the turn that answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub background_jobs: Option<u32>,
    /// What it changed in its session's checkout (the turn diff card, the files-changed pill):
    /// the thread's own edits and commands and its workers' landings, never what the user
    /// changed meanwhile. Absent on requests from before it was kept, and while it changed
    /// nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub changes: Option<RequestChanges>,
    /// Where the user's review left each file it changed, once it settled.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    #[ts(
        as = "Option<std::collections::BTreeMap<String, FileReview>>",
        optional
    )]
    pub review: std::collections::BTreeMap<String, FileReview>,
}

/// A stretch of time a request worked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkSpan {
    pub from_ms: i64,
    /// Absent while it still works.
    pub to_ms: Option<i64>,
}

/// A turn's exact git snapshot diff, with files introduced by merges grouped separately.
/// The captured trees stay under the session's refs for Review and confirmed Revert.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RequestChanges {
    /// Files brought in by merges, grouped by the source branch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<MergedTurnChanges>>", optional)]
    pub merged: Vec<MergedTurnChanges>,
    /// The files and their lines in and out.
    pub stat: DiffStat,
    /// File-review refusals. A full Revert names later files in its confirmation instead.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<HeldPath>>", optional)]
    pub held: Vec<HeldPath>,
    /// The CLI's turn-end snapshot is retained.
    #[serde(default)]
    pub settled: bool,
}

/// A branch whose merge introduced files during a turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MergedTurnChanges {
    pub branch: String,
    pub tip: String,
    pub merge_commit: String,
    pub stat: DiffStat,
}

/// The files named before confirming a turn's start-tree restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TurnRevert {
    pub expected_tree: String,
    pub changed_after: Vec<String>,
}

/// A file Undo or Reapply leaves as it is, with why in plain words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HeldPath {
    pub path: String,
    pub reason: String,
}

/// What an Undo or Reapply did: the files it put back, and those it left as they are.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UndoOutcome {
    pub restored: Vec<String>,
    pub held: Vec<HeldPath>,
}

/// Where the user's review left one file a request changed (a turn diff card's row).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FileReview {
    /// Its changes wait for the user's Keep or Undo.
    Pending,
    /// Still waiting, but a later request changed it too: that request's card has it now.
    Later,
    /// The user kept its changes (some of them, at least).
    Kept,
    /// Its changes are gone: the user undid them, or the file is back as it was.
    Undone,
    /// A merge took its changes: the merge is their record now, not the review.
    Merged,
}

/// The changes agents made that the user hasn't kept or undone yet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PendingChanges {
    pub files: Vec<PendingFile>,
}

/// One file's changes waiting for review: from its review base (the file as the user last
/// accepted it) to the file in the checkout now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PendingFile {
    pub path: String,
    /// The request whose card lists it: the latest that changed it.
    pub request_id: String,
    /// Its text at the review base and now (`None`: no file), when both are text small enough
    /// to show. Without them only the whole file can be kept or undone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub current: Option<String>,
    /// Whether its text is shown, whatever `base` and `current` hold.
    pub text: bool,
    /// Each run of changed lines, numbered in `base` (old) and `current` (new).
    pub hunks: Vec<PendingHunk>,
    pub insertions: u32,
    pub deletions: u32,
    /// Why Undo leaves it as it is: someone else changed it too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub held: Option<String>,
}

/// One run of changed lines in a pending file, as git numbers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PendingHunk {
    /// Taken from where it is and its lines: Keep or Undo of an id the file no longer has is
    /// refused.
    pub id: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
}

/// Whether the user restored the turn's start snapshot in the checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RequestUndo {
    /// The start snapshot was restored.
    pub reverted: bool,
    /// No commits are created by a snapshot restore.
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

/// A session checkout's files for the Source panel: what is staged, what is not, and where
/// the branch pushes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SourceState {
    /// The branch checked out; absent on a detached HEAD.
    pub branch: Option<String>,
    /// The remote the branch pushes to, if any.
    pub remote: Option<String>,
    pub upstream: Option<String>,
    /// Commits not pushed yet.
    pub ahead: u32,
    /// The index against HEAD, sorted by path.
    pub staged: Vec<SourceFile>,
    /// The files against the index, untracked and conflicted files included, sorted by path.
    /// A partly staged file is in both lists.
    pub changes: Vec<SourceFile>,
    /// The merge in progress, if any.
    #[serde(default)]
    pub merging: Option<SourceMerge>,
    /// The rebase in progress, if any; `branch` is absent meanwhile (HEAD is detached).
    #[serde(default)]
    pub rebasing: Option<SourceRebase>,
}

/// A rebase in progress in a checkout, stopped on a commit it replays, for the Source panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SourceRebase {
    /// The branch being rebased; absent for a detached HEAD's.
    pub branch: Option<String>,
    /// What it is rebased onto: a branch's name, else its commit's short id.
    pub onto: String,
    /// The commit it stopped on, counted from 1, of `total`.
    pub step: u32,
    pub total: u32,
    /// The commit it stopped on: its short id and message (empty when git didn't say).
    pub commit: String,
    pub message: String,
}

/// A merge in progress in a checkout, for the Source panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SourceMerge {
    /// What is being merged in: a branch's name, else its commit's short id.
    pub incoming: String,
    /// The message git prepared for the merge commit.
    pub message: String,
}

/// A branch the Source panel can merge from or switch to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SourceBranch {
    /// `main`, or `origin/main` for a remote branch.
    pub name: String,
    pub remote: bool,
    /// When its tip was committed, in ms since the Unix epoch.
    pub at_ms: i64,
    /// Its tip's author and subject.
    pub author: String,
    pub subject: String,
}

/// A conflicted file's versions, for its conflict and merge editor tabs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConflictFile {
    /// The file as it is now, conflict markers and all; absent when it was deleted.
    pub text: Option<String>,
    /// The checkout's own version; absent when its side deleted it.
    pub current: Option<String>,
    /// The incoming version; absent when that side deleted it.
    pub incoming: Option<String>,
    /// Both versions merged again with every conflict marked `<<<<<<< Current`,
    /// `||||||| Base`, `=======`, `>>>>>>> Incoming`; absent when a side deleted the file.
    pub merged: Option<String>,
    /// The two sides: the checkout's commit and branch, the incoming commit and name. In a
    /// rebase, what it is rebased onto and the branch's commit being replayed.
    pub current_label: String,
    pub incoming_label: String,
    /// The rebase in progress, if any, naming the commit being replayed.
    #[serde(default)]
    pub rebasing: Option<SourceRebase>,
}

/// One of the Source panel's git actions on a checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SourceAction {
    /// Merges a branch (local, or remote as `origin/main`) into the checked-out one.
    Merge {
        branch: String,
    },
    /// Aborts the merge in progress.
    AbortMerge,
    /// Switches to a branch; a remote one to the local branch of its name.
    Checkout {
        branch: String,
    },
    Pull,
    Push,
    /// Commit & Sync's pull, then its push unless the pull stopped on conflicts. A session
    /// branch starting from its pull request's squash or rebase merge, whose kept remote
    /// branch is still the merged head, isn't pulled: its push replaces that (#331).
    Sync,
    Fetch,
    /// Rebases the checked-out branch onto a branch (local, or remote as `origin/main`); one
    /// that stops on conflicts stays in progress for them to be resolved.
    Rebase {
        branch: String,
    },
    /// Continues the rebase in progress once its conflicts are resolved and staged; it stops
    /// again on the next commit's.
    ContinueRebase,
    /// Aborts the rebase in progress.
    AbortRebase,
    /// Creates a branch and switches to it, from what is checked out or from `from`.
    CreateBranch {
        name: String,
        from: Option<String>,
    },
    /// Renames the checked-out branch.
    RenameBranch {
        name: String,
    },
    /// Deletes a local branch; with `force` also one not merged.
    DeleteBranch {
        name: String,
        force: bool,
    },
    /// Stashes the changes; with `untracked` the untracked files too.
    Stash {
        untracked: bool,
    },
    /// Applies the latest stash; with `pop` drops it once applied.
    ApplyStash {
        pop: bool,
    },
    /// Drops the latest stash.
    DropStash,
    /// Writes a conflicted file as the user resolved it, only while it is still `expected`
    /// (absent: deleted); with `stage`, marks it resolved.
    Resolve {
        path: String,
        expected: Option<String>,
        text: String,
        stage: bool,
    },
}

/// A git command the Source panel ran on a checkout, for its Git Output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SourceGitRun {
    /// When it ended, in ms since the Unix epoch.
    pub at_ms: i64,
    /// `git` and its arguments.
    pub command: String,
    /// What it printed, out and error together.
    pub output: String,
    pub ok: bool,
}

/// One file of the Source panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SourceFile {
    /// Repo-relative path.
    pub path: String,
    /// Where a rename or copy came from.
    pub old_path: Option<String>,
    pub status: SourceStatus,
}

/// How a Source panel file differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SourceStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    /// Not tracked by git; only among the changes.
    Untracked,
    /// Unmerged; only among the changes.
    Conflicted,
}

/// Which side of the Source panel a Discard throws away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SourceScope {
    /// The index and files go back to HEAD's.
    Staged,
    /// The files go back to the index's; untracked files are deleted.
    Unstaged,
}

/// One of the Source pane's changes to a checkout: stage, unstage, or discard one side. A
/// list of paths acts on those files; none, on every one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SourceChange {
    Stage {
        paths: Option<Vec<String>>,
    },
    Unstage {
        paths: Option<Vec<String>>,
    },
    Discard {
        scope: SourceScope,
        paths: Option<Vec<String>>,
    },
}

/// The GitHub pull request of a session's branch (the pinned card's row).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    pub number: u32,
    pub title: String,
    pub url: String,
    pub state: PullRequestState,
    /// The branch it targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub base: Option<String>,
    /// The commit its merge made on the base, once merged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub merge_commit: Option<String>,
    /// Its checks on GitHub, when it has any (the card's lookup only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ci: Option<PullRequestCi>,
    /// GitHub explicitly reports an approved review decision.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub approved: bool,
    /// GitHub explicitly reports MERGEABLE; unknown is not ready.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub mergeable: bool,
    /// GitHub can't merge it: its branch conflicts with the base.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub conflicts: bool,
    /// A reviewer asked for changes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub changes_requested: bool,
    /// Review threads not resolved yet, whatever the review decision says.
    #[serde(default, skip_serializing_if = "is_zero")]
    #[ts(as = "Option<u32>", optional)]
    pub unresolved: u32,
}

fn is_zero(count: &u32) -> bool {
    *count == 0
}

/// A pull request's checks on GitHub, all of them together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PullRequestCi {
    Passing,
    Failing,
    Pending,
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
    /// Why the message wasn't written by the model the user chose, when it wasn't.
    #[serde(default)]
    pub note: Option<String>,
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
    /// One commit. A fast-forward merge's `commits`, when it names them: all of them, its
    /// head being the merge's commit.
    Commit {
        commit: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        commits: Option<u32>,
    },
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

/// Whose checkout the right sidebar shows: a session's (its worktree, or the repository while
/// that doesn't exist yet), or a project's folder before its session starts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WorkspaceTarget {
    Conversation { conversation_id: ConversationId },
    Project { project_id: ProjectId },
}

/// A checkout's files as the right sidebar's tree lists them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTree {
    /// Tracked and untracked files, relative to the checkout's root, sorted.
    pub files: Vec<String>,
    /// Set when the checkout has more files than were listed.
    pub truncated: bool,
    /// What it ignores, sorted: an ignored folder once, ending in `/`, its contents read
    /// only when it is opened.
    pub ignored: Vec<String>,
    /// Set when it ignores more than were listed.
    pub ignored_truncated: bool,
}

/// One entry of a folder read on its own (an ignored one, opened in the tree).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceEntry {
    pub name: String,
    pub folder: bool,
}

/// A folder of a checkout, read on its own: folders first, then files, each by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFolder {
    pub entries: Vec<WorkspaceEntry>,
    /// Set when it holds more than were listed.
    pub truncated: bool,
}

/// What the right sidebar's Search looks for in a checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    /// The text, or with `regex` a regular expression.
    pub text: String,
    pub match_case: bool,
    /// Only matches standing as whole words.
    pub whole_word: bool,
    pub regex: bool,
    /// Comma-separated globs a file must match (none: every file), such as `src/**, *.ts`.
    #[serde(default)]
    pub include: String,
    /// Comma-separated globs of files and folders left out.
    #[serde(default)]
    pub exclude: String,
}

/// Where a match is in a result line's preview, in UTF-16 code units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchRange {
    pub start: u32,
    pub end: u32,
}

/// One line of a file holding matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchLine {
    /// From 1.
    pub line: u32,
    /// The line's text, or the part of a long line around its first match (cut ends marked
    /// with `…`).
    pub preview: String,
    pub ranges: Vec<SearchRange>,
}

/// A file holding matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SearchFile {
    /// Relative to the checkout's root.
    pub path: String,
    /// Every match in the file, also those past the lines listed.
    pub matches: u32,
    /// The lines with matches, the first ones when it has very many.
    pub lines: Vec<SearchLine>,
    /// What the file held when searched: a replace only rewrites it unchanged.
    pub hash: String,
}

/// What a running search sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SearchUpdate {
    /// More files with matches, in path order.
    Files { files: Vec<SearchFile> },
    /// The search ended: how many it found, and whether it stopped at its limits first.
    Done {
        files: u32,
        matches: u32,
        capped: bool,
    },
}

/// A file a replace rewrites, as its search found it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceFile {
    pub path: String,
    /// The search's hash of it.
    pub hash: String,
    /// How many matches the search found in it.
    pub matches: u32,
}

/// A file that kept a replace from happening, or that it couldn't put back, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceSkip {
    pub path: String,
    pub reason: String,
}

/// What a replace did. It is all or nothing: every file listed is rewritten, or none is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceOutcome {
    /// Files rewritten: all of those listed, or none.
    pub files: u32,
    pub matches: u32,
    /// Why nothing was rewritten: each file that changed since the search, can't be written,
    /// or changed while the others were written (those were put back).
    pub skipped: Vec<ReplaceSkip>,
    /// Files the replace rewrote and couldn't put back after another one failed (something
    /// else changed them meanwhile): they stay replaced, and the reason says where their text
    /// from before is kept.
    pub unrestored: Vec<ReplaceSkip>,
}

/// Why something entered the orchestrator's context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, TS)]
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
    /// Brigadier asks for an answer the orchestrator left out.
    Reminder,
    /// An overnight run starts, restarts or ends.
    Run,
    /// Another session's message (its label is `peer <message id>`).
    PeerMessage,
    /// The result of a job the thread's CLI ran in the background, or what became of it.
    BackgroundRun,
    /// The one message at a seam (G12): write the hand-off note; or the hand-off a successor
    /// starts from.
    Seam,
    /// What its workers did that no `brigadier wait` of its took, once its turn ended (S29;
    /// its label is `worker events <message id>`).
    WorkerEvents,
}

/// An injection's kind as stored, older names included: an earlier run's per-phase `phase`
/// reads as `run`, so the thread engine's first start can still delete it (THREAD-PLAN.md Q14),
/// and any other kind an earlier version wrote (the first message its fresh thread started
/// from) reads as a hand-off.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum StoredInjectionKind {
    Instructions,
    UserMessage,
    Report,
    WorkerQuestion,
    Decision,
    TaskFailed,
    ToolResult,
    Artifact,
    Reseed,
    Resume,
    FollowUp,
    Reminder,
    Run,
    Phase,
    PeerMessage,
    BackgroundRun,
    Seam,
    WorkerEvents,
    #[serde(other)]
    Retired,
}

impl<'de> Deserialize<'de> for InjectionKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match StoredInjectionKind::deserialize(deserializer)? {
            StoredInjectionKind::Instructions => Self::Instructions,
            StoredInjectionKind::UserMessage => Self::UserMessage,
            StoredInjectionKind::Report => Self::Report,
            StoredInjectionKind::WorkerQuestion => Self::WorkerQuestion,
            StoredInjectionKind::Decision => Self::Decision,
            StoredInjectionKind::TaskFailed => Self::TaskFailed,
            StoredInjectionKind::ToolResult => Self::ToolResult,
            StoredInjectionKind::Artifact => Self::Artifact,
            StoredInjectionKind::Reseed => Self::Reseed,
            StoredInjectionKind::Resume => Self::Resume,
            StoredInjectionKind::FollowUp => Self::FollowUp,
            StoredInjectionKind::Reminder => Self::Reminder,
            StoredInjectionKind::Run | StoredInjectionKind::Phase => Self::Run,
            StoredInjectionKind::PeerMessage => Self::PeerMessage,
            StoredInjectionKind::BackgroundRun => Self::BackgroundRun,
            StoredInjectionKind::Seam | StoredInjectionKind::Retired => Self::Seam,
            StoredInjectionKind::WorkerEvents => Self::WorkerEvents,
        })
    }
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
    /// Something broke the orchestrator's contract, such as its CLI compacting the context
    /// (PLAN.md §2: the orchestrator never compacts).
    ContractBreach {
        message: String,
    },
    /// An entry of a kind an earlier version wrote that nothing reads any more.
    #[serde(other)]
    Retired,
}

/// A settings change the thread suggests on a card (Q90): this session's permission level,
/// the built-in browser, or merging on its own. Nothing else is the agent's to suggest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "setting",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SettingChange {
    Permission {
        level: crate::model::PermissionLevel,
    },
    /// Settings › Browser's switch.
    Browser { on: bool },
    /// Settings › Git's "Merge on its own when the work is ready", from an older Brigadier:
    /// that setting is gone, and its card's button changes nothing.
    MergeOnOwn { on: bool },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The retired routing's offer and move notices in stored events are ignored without a
    /// migration; Deciding and its held note read again (run2-D40), and a restart settles
    /// them.
    #[test]
    fn old_queue_events_read_without_retired_routing_state() {
        let item = serde_json::json!({
            "id": "first", "text": "Later.", "attachments": [], "mentions": [],
            "queuedAtMs": 1, "editedAtMs": null, "deciding": true, "offerTab": true,
            "heldBy": { "command": "sleep 30", "sinceMs": 1 }
        });
        let queue: MessageQueue = serde_json::from_value(serde_json::json!({
            "items": [item], "paused": false,
            "moved": [{
                "id": "old", "text": "Moved.", "attachments": [], "mentions": [],
                "to": "other", "toTitle": "Other", "movedAtMs": 2,
                "sent": { "type": "queued", "itemId": "there" }
            }]
        }))
        .expect("an old stored queue");
        let serialized = serde_json::to_value(&queue).unwrap();
        assert_eq!(serialized["items"][0]["deciding"], true);
        assert!(serialized["items"][0].get("offerTab").is_none());
        assert!(serialized["items"][0].get("offerSession").is_none());
        assert!(serialized.get("moved").is_none());
        assert!(serialized.get("sent").is_none());
        assert_eq!(queue.next_to_send().unwrap().id, "first");
        assert!(queue.ready_to_send().is_none(), "Deciding holds the queue");
    }

    /// Log entries and injection kinds an earlier version wrote still read: as retired, or as
    /// a hand-off.
    #[test]
    fn old_log_entries_still_read() {
        let entry: OrchestratorEntry = serde_json::from_value(serde_json::json!({
            "type": "checkpoint",
            "anything": 1,
        }))
        .unwrap();
        assert_eq!(entry, OrchestratorEntry::Retired);
        let entry: OrchestratorEntry = serde_json::from_value(serde_json::json!({
            "type": "injection",
            "injection": {
                "kind": "anOlderKind",
                "bytes": 10,
                "tokensEstimate": 3,
                "label": "first message",
                "taskId": null,
            },
        }))
        .unwrap();
        let OrchestratorEntry::Injection { injection } = entry else {
            panic!("an injection: {entry:?}");
        };
        assert_eq!(injection.kind, InjectionKind::Seam);
    }

    #[test]
    fn a_question_card_stored_before_rounds_reads_as_a_round_of_one() {
        let stored = serde_json::json!({
            "id": "q1",
            "conversationId": "c1",
            "taskId": null,
            "requestId": "r1",
            "position": 0,
            "kind": { "type": "orchestrator" },
            "text": "Which region?",
            "options": ["eu-west", "us-east"],
            "recommended": 1,
            "answer": null,
            "createdAtMs": 0,
            "answeredAtMs": null
        });
        let question: Question = serde_json::from_value(stored).unwrap();
        assert!(question.items.is_empty() && question.answers.is_empty());
        assert!(question.is_open());
        let round = question.round();
        assert_eq!(round.len(), 1);
        assert_eq!(round[0].text, "Which region?");
        let labels: Vec<_> = round[0].options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels, ["eu-west", "us-east"]);
        assert!(round[0].options.iter().all(|o| o.description.is_none()));
        assert_eq!(round[0].recommended, Some(1));
    }

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
}

#[cfg(test)]
mod attachment_tests {
    use super::*;

    #[test]
    fn attachments_read_both_saved_inline_formats() {
        for (saved, expected) in [
            ("true", Some(0)),
            ("false", None),
            ("1", Some(1)),
            ("null", None),
        ] {
            let json = format!(
                r#"{{"id":"hash","name":"photo.png","mime":"image/png","bytes":12,"inline":{saved}}}"#
            );
            assert_eq!(
                serde_json::from_str::<AttachmentRef>(&json).unwrap().inline,
                expected
            );
        }
    }

    #[test]
    fn old_attachment_defaults_to_non_inline() {
        let attachment: AttachmentRef = serde_json::from_str(
            r#"{"id":"hash","name":"photo.png","mime":"image/png","bytes":12}"#,
        )
        .unwrap();
        assert!(attachment.inline.is_none());
        assert!(!attachment.pasted);
        let mut inline = attachment;
        inline.inline = Some(0);
        let saved = serde_json::to_string(&inline).unwrap();
        assert_eq!(
            serde_json::from_str::<AttachmentRef>(&saved).unwrap(),
            inline
        );
    }
}
