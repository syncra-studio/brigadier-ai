//! The Brigadier MCP tools as the core sees them: who may call them, their arguments, and the
//! host that answers them.
//!
//! Every CLI session Brigadier starts gets its own random **grant**, scoped to one role in one
//! conversation. The grant is the only credential a session holds: it lives in daemon memory,
//! is checked on every call (so revoking it also cuts off connections that are already open),
//! and is revoked when the session ends. A grant never reaches UI-only requests such as
//! answering approval cards.
//!
//! - [`Role::Orchestrator`] may call the orchestrator tools ([`OrchestratorCall`]).
//! - [`Role::Worker`] may call the worker tools ([`WorkerCall`]) for its own task.
//!
//! The MCP server (`crates/mcp-server`) maps MCP tool calls onto these types; the session
//! manager implements [`ToolHost`].

use std::collections::HashMap;
use std::sync::Mutex;

use brigadier_providers::BoxFuture;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer};

use crate::model::{ConversationId, ProjectId, TaskId};
use crate::work::{ChecksResult, ReviewVerdict, TaskKind, WorkerRole};

/// What a grant allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    /// The orchestrator of a session (its thread).
    Orchestrator {
        conversation_id: ConversationId,
        /// Its command tools, which depend on its vendor and level.
        run: RunTools,
    },
    /// The worker running one task.
    Worker {
        conversation_id: ConversationId,
        task_id: TaskId,
        /// It checks a change or plan in a gate: it decides from what it was given and its
        /// own evidence alone, so it has no `ask_orchestrator`.
        checks: bool,
    },
    /// A Brain job (skeleton pass, enrichment) of a project: it reads and records nodes.
    BrainJob {
        project_id: ProjectId,
        job_id: String,
    },
    /// A Chat's model: it may save memories to the Personal Brain.
    Chat { conversation_id: ConversationId },
    /// A Claude thread's output hook (`brigadierd hook post-tool-use`): it may only store the
    /// thread's command output, and calls no tools.
    OutputHook { conversation_id: ConversationId },
}

/// The command tools a thread has besides the orchestrator tools (THREAD-PLAN.md Q4): a Codex
/// thread runs long commands through Brigadier, which keeps their whole output and returns a
/// digest. A Claude thread's own `Bash` output is trimmed by its hook instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunTools {
    #[default]
    None,
    /// `run`.
    Run,
    /// `run`, and `run_unsandboxed` to leave the sandbox with the level's approval.
    WithEscalation,
}

/// Live grants, keyed by their secret value. Each belongs to a cleanup-ledger owner
/// (`orch:<id>`, `task:<id>`, …) so ending the owner revokes all of its grants.
#[derive(Default)]
pub struct Grants {
    inner: Mutex<HashMap<String, (String, Role)>>,
}

impl Grants {
    /// Issues a fresh grant for `role`, owned by `owner`.
    pub fn issue(&self, owner: &str, role: Role) -> String {
        let secret = format!(
            "brg_{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        self.lock().insert(secret.clone(), (owner.to_owned(), role));
        secret
    }

    /// The role of a live grant.
    pub fn resolve(&self, grant: &str) -> Option<Role> {
        self.lock().get(grant).map(|(_, role)| role.clone())
    }

    /// Revokes every grant `owner` holds.
    pub fn revoke_owner(&self, owner: &str) {
        self.lock().retain(|_, (held_by, _)| held_by != owner);
    }

    /// Every live grant value, for scrubbing them out of logs and recordings.
    pub fn secrets(&self) -> Vec<String> {
        self.lock().keys().cloned().collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (String, Role)>> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}

// ----- orchestrator tools ----------------------------------------------------------------

/// `delegate_task`: start a worker. Returns at once with the task's number; the report arrives
/// later as a message.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DelegateTask {
    /// A plain 2–4 word job name, unique among this chat's workers, e.g. "Add JSON output".
    /// Describe the job, without roles, phases or internal task ids.
    pub title: String,
    /// What the task does. `implement` and `merge` tasks change code and land as one commit
    /// each; the others only read and report.
    pub kind: TaskKind,
    /// The full task spec for the worker: goal, context, constraints, what "done" means and how
    /// to verify it. The worker sees nothing else of the conversation.
    pub spec: String,
    /// Optional provider override: "claude" or "codex". Leave it out to let Brigadier route.
    #[serde(default)]
    pub provider: Option<String>,
    /// Optional model id override (from that provider's model list).
    #[serde(default)]
    pub model: Option<String>,
    /// Optional reasoning effort override: "low", "medium" or "high".
    #[serde(default)]
    pub effort: Option<String>,
    /// For `review` tasks: the task whose candidate commit is reviewed, e.g. "task-2". For
    /// `merge` tasks: the task whose work conflicts with the branch it lands on. For a `fix`:
    /// the task whose work it fixes (the phase's verifier); it continues from that work and
    /// lands it with its own.
    #[serde(default)]
    pub subject: Option<String>,
    /// Ids of the user's attachments the worker should get as files.
    #[serde(default)]
    pub attachments: Vec<String>,
    /// Optional: the parts of the codebase it touches ("frontend", "backend", "infra",
    /// "docs", "tests"), when the spec's paths don't make it plain. Routing and the user's
    /// routing rules go by them.
    #[serde(default)]
    pub areas: Vec<String>,
    /// Optional: "high" when the task needs the vendors' best models (hard or risky work);
    /// leave it out otherwise.
    #[serde(default)]
    pub quality: Option<String>,
    /// True when the task must generate images: only some models can, and Brigadier keeps
    /// it on one that can, hand-offs included.
    #[serde(default)]
    pub image_generation: bool,
    /// For a task that works on a phase of the request's plan (`plan_phases`): that phase's
    /// number (1 is the first). The user follows the plan's progress by it.
    #[serde(default, alias = "step")]
    pub phase: Option<u32>,
    /// Its part in the request: "lead" (one per phase, the default for implement tasks),
    /// "parallel" (a stream of its own files next to the lead) or "fix" (what a phase's
    /// verifier left).
    #[serde(default)]
    pub role: Option<WorkerRole>,
}

/// `message_worker`: answer a worker's blocking question, or steer a running worker.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MessageWorker {
    /// The task, e.g. "task-3".
    pub task: String,
    /// The answer or instruction.
    pub text: String,
}

/// `answer_worker`: answer the question a worker waits on (`ask_orchestrator`).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnswerWorker {
    /// The task, e.g. "task-3".
    pub task: String,
    /// The answer: what the worker should do.
    pub answer: String,
    /// Why, in a few words (the brief says so, the worker's recommendation fits, a decision
    /// settled it).
    pub why: String,
}

/// `route_follow_up`: sorts a message the user sent while the orchestrator works on their
/// request.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RouteFollowUp {
    /// The follow-up's id, from its [follow-up …] block.
    pub follow_up: String,
    /// True if it belongs to the work in progress (it joins it now); false if it is a request
    /// of its own (it waits until this work is done).
    pub joins: bool,
}

/// A task reference only (`stop_worker`, `read_report`).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskRef {
    /// The task, e.g. "task-3".
    pub task: String,
}

/// `read_report`: a task of this session, or a report from another session of the project.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReportRef {
    /// This session's task, e.g. "task-3"; or another session's task by the id a Brain
    /// answer names ("from report <id>").
    pub task: String,
}

/// `ask_user`: a question only the user can answer (a product choice, an unclear requirement).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskUser {
    /// The question, self-contained: the user may read it later, out of context.
    pub question: String,
    /// Suggested answers shown as buttons; the user can always type their own.
    #[serde(default)]
    pub options: Vec<String>,
    /// The option you recommend, by its 0-based index in `options` (shown as "Recommended").
    #[serde(default)]
    pub recommended: Option<u32>,
    /// The task that waits for the answer, if any (other tasks continue).
    #[serde(default)]
    pub task: Option<String>,
}

/// `read_artifact`: page through a stored artifact (transcript, diff, command output, note).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadArtifact {
    /// The artifact id from a report.
    pub id: String,
    /// Byte offset to start at.
    #[serde(default)]
    pub offset: Option<u64>,
    /// Bytes to read (at most 16000).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `run`: a shell command run for the thread, its whole output kept.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunCommand {
    /// The command, as you would type it in a shell (`/bin/sh -c`).
    pub command: String,
    /// The folder it runs in: the workspace (the default) or a folder inside it, or your own
    /// scratch folder.
    #[serde(default)]
    pub workdir: Option<String>,
    /// How long it may run, in seconds (default 600, at most 1800); it is stopped then.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// `run_check`: a check (tests, lint, typecheck, build) run in the caller's tree, its result
/// kept per tree; without a command, the checks the tree's changes affect (THREAD-PLAN.md Q8
/// lever 3). For workers and the thread alike.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunCheck {
    /// The check, as you would type it in a shell (`/bin/sh -c`). Leave it out to get the
    /// checks your changes affect (nothing runs then).
    #[serde(default)]
    pub command: Option<String>,
    /// The folder it runs in, relative to your checkout's root (the root by default).
    #[serde(default)]
    pub workdir: Option<String>,
    /// How long it may run, in seconds (default 600, at most 1800); it is stopped then.
    #[serde(default, alias = "timeout")]
    pub timeout_secs: Option<u64>,
    /// Run it even when it already ran on the same files, and keep the new result.
    #[serde(default)]
    pub rerun: bool,
}

/// `run_unsandboxed`: the same, outside the sandbox, once approved.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunUnsandboxed {
    /// The command, as you would type it in a shell (`/bin/sh -c`).
    pub command: String,
    /// The folder it runs in: the workspace (the default) or a folder inside it, or your own
    /// scratch folder.
    #[serde(default)]
    pub workdir: Option<String>,
    /// How long it may run, in seconds (default 600, at most 1800); it is stopped then.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Why it must run outside the sandbox (what the sandbox blocked), in a sentence: the
    /// approval is decided on it.
    pub justification: String,
}

/// `start_preview`: a long-running process (a dev server, the app) the user can look at.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartPreview {
    /// The command, as you would type it in a shell (`/bin/sh -c`), run in the foreground
    /// (no trailing `&`): it runs until it is stopped.
    pub command: String,
    /// A short name the user sees ("web app", "docs server"); the command by default.
    #[serde(default)]
    pub name: Option<String>,
    /// Environment variables to set for it.
    #[serde(default)]
    pub env: Option<std::collections::BTreeMap<String, String>>,
    /// The folder it runs in, relative to the workspace (the workspace by default); it must be
    /// inside the workspace.
    #[serde(default)]
    pub workdir: Option<String>,
}

/// `stop_preview`: stop one preview, or every running one.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StopPreview {
    /// The preview's id ("preview-1"); every running preview when left out.
    #[serde(default)]
    pub id: Option<String>,
}

/// `preview_log`: the end of a preview's output.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewLog {
    /// The preview's id ("preview-1"); the latest one when left out.
    #[serde(default)]
    pub id: Option<String>,
    /// How many of its last lines to show (default 40, at most 400).
    #[serde(default)]
    pub tail_lines: Option<u32>,
}

/// `query_brain`: ask the Project Brain.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryBrain {
    /// What you want to know about the project.
    pub query: String,
    /// Also show earlier versions of decisions, conventions and contracts (what held before
    /// and why it changed). Off by default: answers give what holds now.
    #[serde(default)]
    pub history: Option<bool>,
    /// The next page of results, when an answer says there are more (from 1).
    #[serde(default)]
    pub page: Option<u32>,
}

/// What `remember` records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum MemoryKind {
    /// Something the user settled, or you decided, that later work must respect, including a
    /// rule the user sets for this session only.
    Decision,
    /// How this project always does things (naming, structure, style, commit messages),
    /// shared with its other sessions; not a rule for this session only.
    Convention,
    /// What the user likes in general, across projects.
    Preference,
    /// An interface between parts or services that both sides rely on.
    Contract,
}

/// `remember`: record a decision, convention, preference or contract in the Brain.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Remember {
    pub kind: MemoryKind,
    /// One line that states it, e.g. "Commit subjects are imperative and under 60 characters".
    pub title: String,
    /// Why, and any detail that matters later.
    #[serde(default)]
    pub detail: Option<String>,
    /// True for a preference of the user's that holds in every project (it goes to their
    /// Personal Brain); false for this project only.
    #[serde(default)]
    pub personal: bool,
    /// Repository-relative files or folders it is about, if any.
    #[serde(default)]
    pub files: Vec<String>,
    /// The id of an earlier decision this one replaces.
    #[serde(default)]
    pub replaces: Option<String>,
}

/// `search_transcript`: search this conversation's full transcript.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchTranscript {
    /// Words to look for.
    pub query: String,
    /// At most this many passages (default 8).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `code_search`: find symbols and files in the repository's code index.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodeSearch {
    /// A name or path fragment.
    pub query: String,
    /// "symbol", "file" or "any" (default).
    #[serde(default)]
    pub kind: Option<String>,
    /// Only this language, e.g. "rust", "typescript", "python".
    #[serde(default)]
    pub language: Option<String>,
    /// Only under this repository-relative folder.
    #[serde(default)]
    pub path: Option<String>,
    /// At most this many hits (default 30).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `code_refs`: where a symbol is defined and used.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodeRefs {
    /// The symbol's name.
    pub symbol: String,
    /// At most this many references (default 50).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// A node a Brain job records.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeInput {
    /// "module", "service", "fileSummary", "convention", "contract" or "decision".
    pub kind: String,
    /// For modules and file summaries: the repository-relative folder or file it describes.
    #[serde(default)]
    pub path: Option<String>,
    /// A short title ("crates/store: the event store").
    pub title: String,
    /// What it is for and what matters about it, in a few sentences.
    pub body: String,
    /// Repository-relative files it was learned from.
    #[serde(default)]
    pub files: Vec<String>,
    /// The key (or id) of a node this one replaces, when the replacement has another title
    /// or kind (a stale convention, renamed). The old one is kept as history.
    #[serde(default)]
    pub replaces: Option<String>,
}

/// `record_nodes`: a Brain job's findings.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordNodes {
    pub nodes: Vec<NodeInput>,
}

/// One phase of a request's plan.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanStepInput {
    /// The phase, in a few words.
    pub title: String,
    /// Its scope and "done when", if the title is not enough.
    #[serde(default)]
    pub detail: Option<String>,
}

/// `plan_phases`: split a big request into phases that must run one after another. Each
/// phase gets one lead; nothing is reviewed or approved here.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanPhases {
    /// What the request achieves.
    pub title: String,
    /// The phases, in order.
    pub phases: Vec<PlanStepInput>,
}

/// `approve_outline`: let a lead build from its outline, with your corrections.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApproveOutline {
    /// The lead's task, e.g. "task-3".
    pub task: String,
    /// What the lead must change in its outline: the review findings you agree with and
    /// anything the brief implies. Leave it out when the outline is right as it is.
    #[serde(default)]
    pub corrections: Option<String>,
}

/// `request_approval`: ask the user to approve what only they may decide (money, credentials,
/// destroying something outside the session's own work).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequestApproval {
    /// What would happen, in one line.
    pub action: String,
    /// Why, and what exactly.
    pub details: String,
}

/// `land_phase`: land a reported write task's commits (with the work they build on) on the
/// session's branch.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LandPhase {
    /// The task whose commits land, e.g. "task-3": a phase's verifier, or the lead of a
    /// small request.
    pub task: String,
}

/// `review_plan`: one review of the thread's own plan by the other vendor, in the background.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewPlan {
    /// The plan: the steps in order with the files each touches, how each "done when" will
    /// be checked, and the risks.
    pub plan: String,
    /// What the plan must achieve: the user's request in their words, the constraints and
    /// the settled decisions. The reviewer judges the plan against it.
    pub brief: String,
}

/// `finish_session`: merge the session branch into its base (new-worktree sessions), behind the
/// user's one-click approval.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FinishSession {
    /// The merge commit message, when a merge commit is needed.
    #[serde(default)]
    pub message: Option<String>,
}

/// What `note_for_user` records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum NoteKind {
    /// A judgement call you made on the user's behalf: it shows under "Decided for you".
    Decided,
    /// Something only the user can do: it shows under "Waiting on you" until they mark it
    /// done.
    Waiting,
}

/// `note_for_user`: keep the user's session summary current.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoteForUser {
    pub kind: NoteKind,
    /// One line: what you decided ("Kept the v1 endpoint for old clients"), or what the user
    /// must do ("Add STRIPE_KEY to .env").
    pub what: String,
    /// For a decision: why, in a sentence.
    #[serde(default)]
    pub why: Option<String>,
}

/// `settle_step`: during an overnight run, the thread settles a step of the run's plan once it
/// has judged the step's whole scope and every "done when" (its own edits too).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SettleStep {
    /// The step's number in the run's plan (the source plan's phase number).
    pub phase: u32,
    pub outcome: crate::work::StepOutcome,
    /// What the step changed, how each "done when" was checked and what the review found and
    /// what was done about it, in a few sentences.
    pub summary: String,
    /// For a partial or blocked step: what is left, one line each (for blocked, exactly what
    /// only the user can do).
    #[serde(default, deserialize_with = "lines")]
    #[schemars(with = "String", extend("default" = ""))]
    pub left: Vec<String>,
}

/// `end_run`: the thread ends its overnight run early: the plan is done, or all that is left
/// needs the user.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EndRun {
    pub outcome: EndRunOutcome,
    /// Why, in a sentence.
    pub why: String,
}

/// Why the thread ends its run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum EndRunOutcome {
    /// Every selected step is settled.
    Done,
    /// What is left needs the user.
    NeedsUser,
}

/// `propose_overnight`: interpret a user's unstarted proposal; it cannot Start a run.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposeOvernight {
    pub run_id: String,
    pub revision: u32,
    pub name: String,
    pub goal: String,
    /// Rules and settled decisions from the source, verbatim.
    pub rules: String,
    /// Source phase numbers, scope, dependencies and done-when, without invented work.
    pub phases: Vec<OvernightPhaseInput>,
    #[serde(default)]
    pub sources: Vec<OvernightSourceInput>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OvernightPhaseInput {
    pub number: Option<u32>,
    pub name: String,
    pub scope: String,
    pub done_when: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<u32>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OvernightSourceInput {
    pub path: String,
    pub sections: Option<String>,
}

/// A tool call from an orchestrator.
#[derive(Debug, Clone)]
pub enum OrchestratorCall {
    DelegateTask(DelegateTask),
    MessageWorker(MessageWorker),
    AnswerWorker(AnswerWorker),
    RouteFollowUp(RouteFollowUp),
    StopWorker(TaskRef),
    AskUser(AskUser),
    ReadReport(ReportRef),
    ReadArtifact(ReadArtifact),
    QueryBrain(QueryBrain),
    Remember(Remember),
    SearchTranscript(SearchTranscript),
    PlanPhases(PlanPhases),
    ApproveOutline(ApproveOutline),
    StartVerifier(TaskRef),
    RequestApproval(RequestApproval),
    LandPhase(LandPhase),
    FinishSession(FinishSession),
    NoteForUser(NoteForUser),
    ListTasks,
    SettleStep(SettleStep),
    EndRun(EndRun),
    ProposeOvernight(ProposeOvernight),
    CodeSearch(CodeSearch),
    CodeRefs(CodeRefs),
    ProjectMap,
    ReviewPlan(ReviewPlan),
    Run(RunCommand),
    RunUnsandboxed(RunUnsandboxed),
    RunCheck(RunCheck),
    StartPreview(StartPreview),
    StopPreview(StopPreview),
    PreviewLog(PreviewLog),
}

impl OrchestratorCall {
    /// The MCP tool name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::DelegateTask(_) => "delegate_task",
            Self::MessageWorker(_) => "message_worker",
            Self::AnswerWorker(_) => "answer_worker",
            Self::RouteFollowUp(_) => "route_follow_up",
            Self::StopWorker(_) => "stop_worker",
            Self::AskUser(_) => "ask_user",
            Self::ReadReport(_) => "read_report",
            Self::ReadArtifact(_) => "read_artifact",
            Self::QueryBrain(_) => "query_brain",
            Self::Remember(_) => "remember",
            Self::SearchTranscript(_) => "search_transcript",
            Self::PlanPhases(_) => "plan_phases",
            Self::ApproveOutline(_) => "approve_outline",
            Self::StartVerifier(_) => "start_verifier",
            Self::RequestApproval(_) => "request_approval",
            Self::LandPhase(_) => "land_phase",
            Self::FinishSession(_) => "finish_session",
            Self::NoteForUser(_) => "note_for_user",
            Self::ListTasks => "list_tasks",
            Self::SettleStep(_) => "settle_step",
            Self::EndRun(_) => "end_run",
            Self::ProposeOvernight(_) => "propose_overnight",
            Self::CodeSearch(_) => "code_search",
            Self::CodeRefs(_) => "code_refs",
            Self::ProjectMap => "project_map",
            Self::ReviewPlan(_) => "review_plan",
            Self::Run(_) => "run",
            Self::RunUnsandboxed(_) => "run_unsandboxed",
            Self::RunCheck(_) => "run_check",
            Self::StartPreview(_) => "start_preview",
            Self::StopPreview(_) => "stop_preview",
            Self::PreviewLog(_) => "preview_log",
        }
    }
}

// ----- worker tools ----------------------------------------------------------------------

/// `ask_orchestrator`: a blocking question. The call returns the orchestrator's answer.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskOrchestrator {
    /// The question, with the context the orchestrator needs to answer it.
    pub question: String,
}

/// `submit_outline`: a lead's plan for its phase, written after reading the code. The lead then
/// waits for the go-ahead.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmitOutline {
    /// The outline: the steps in order with the files each touches, how each "done when"
    /// will be checked, risks, and any question for the orchestrator with your
    /// recommendation.
    pub outline: String,
}

/// A file the worker saved in its scratch folder, attached to its report.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactInput {
    /// Path of the file, inside the worker's scratch folder (absolute, or relative to it).
    pub path: String,
    /// What the file holds, in a few words.
    pub title: String,
}

/// `submit_report`: the worker's final structured report (about 800 tokens at most; details go
/// into artifacts). Call it exactly once, at the end: the orchestrator reads only this report,
/// never your messages.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmitReport {
    /// What was done and what you found, in a few sentences. It must hold your findings (or
    /// name the artifact that does): never say they are below or in a message.
    pub summary: String,
    /// Repo-relative paths changed, created or deleted (every new file you want kept must be
    /// listed).
    #[serde(default, deserialize_with = "paths")]
    pub changes: Vec<String>,
    /// Decisions made and why, one per line.
    #[serde(default, deserialize_with = "lines")]
    #[schemars(with = "String", extend("default" = ""))]
    pub decisions: Vec<String>,
    /// Exactly what was verified and how (commands run and their results), one item per line.
    #[serde(default, deserialize_with = "lines")]
    #[schemars(with = "String", extend("default" = ""))]
    pub verification: Vec<String>,
    /// Each "done when" criterion of the task, one per line: its status in brackets, the
    /// criterion, then the evidence, e.g. "[met] `pnpm test` passes: 41 passed, 0 failed".
    /// The status is [met], [not met] or [not checked].
    #[serde(default, deserialize_with = "lines")]
    #[schemars(with = "String", extend("default" = ""))]
    pub done_when: Vec<String>,
    /// Questions left open, or (for reviews) the exact issues to fix, one per line.
    #[serde(default, deserialize_with = "lines")]
    #[schemars(with = "String", extend("default" = ""))]
    pub open_questions: Vec<String>,
    /// Risks and assumptions the work rests on, and what you skipped and why, one per line.
    #[serde(default, deserialize_with = "lines")]
    #[schemars(with = "String", extend("default" = ""))]
    pub risks: Vec<String>,
    /// What only the user can do (a credential, a sign-in, a push, a paid signup, an account
    /// id), one per line. Finish everything else around it.
    #[serde(default, deserialize_with = "lines")]
    #[schemars(with = "String", extend("default" = ""))]
    pub needs_user: Vec<String>,
    /// Review tasks only: the verdict on the reviewed change.
    #[serde(default)]
    pub verdict: Option<ReviewVerdict>,
    /// Verify tasks only: whether every check you ran passed, one failed, or you could not
    /// run them.
    #[serde(default)]
    pub checks: Option<ChecksResult>,
    /// Files from your scratch folder with details the report leaves out.
    #[serde(default)]
    pub artifacts: Vec<ArtifactInput>,
}

/// A report list given as one text, one item per line, or as a list. The schema asks for text:
/// a model writing long items full of quotes and backticks sometimes emits a list as bare text,
/// which breaks the call's JSON, while it writes a text field reliably.
fn lines<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Lines {
        Text(String),
        List(Vec<String>),
    }
    Ok(match Option::<Lines>::deserialize(deserializer)? {
        None => Vec::new(),
        Some(Lines::List(items)) => items.into_iter().filter(|item| !empty_item(item)).collect(),
        Some(Lines::Text(text)) => text
            .lines()
            .map(|line| {
                let line = line.trim();
                line.strip_prefix("- ")
                    .or_else(|| line.strip_prefix("* "))
                    .unwrap_or(line)
            })
            .filter(|line| !empty_item(line))
            .map(str::to_owned)
            .collect(),
    })
}

/// The changed paths, without a placeholder that says there are none.
fn paths<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    Ok(Option::<Vec<String>>::deserialize(deserializer)?
        .unwrap_or_default()
        .into_iter()
        .filter(|path| !empty_item(path))
        .collect())
}

/// Whether a report line only says the list is empty ("None.", "N/A", "-", "None for
/// implementation."): some models fill every section, and a placeholder under needs_user must
/// not read as something for the user.
fn empty_item(item: &str) -> bool {
    const PLACEHOLDERS: [&str; 6] = ["none", "n/a", "na", "nothing", "no", "none needed"];
    // A placeholder qualified by what it is about ("None for this task", "Nothing needed for
    // the build"), as long as no second sentence or clause follows that could hold a real ask.
    const QUALIFIED: [&str; 7] = [
        "none for ",
        "none needed ",
        "nothing needed",
        "none required",
        "nothing required",
        "no action needed",
        "no action required",
    ];
    let item = item
        .trim()
        .trim_matches(|c: char| {
            c.is_whitespace() || c.is_ascii_punctuation() || matches!(c, '—' | '–')
        })
        .to_lowercase();
    let one_clause = !item.contains([';', ':', '—', '–']) && !item.contains(". ");
    item.is_empty()
        || PLACEHOLDERS.contains(&item.as_str())
        || (one_clause && QUALIFIED.iter().any(|start| item.starts_with(start)))
}

/// A tool call from a worker.
#[derive(Debug, Clone)]
pub enum WorkerCall {
    AskOrchestrator(AskOrchestrator),
    SubmitOutline(SubmitOutline),
    /// `review_code`: one review of the worker's committed work by the other vendor; returns
    /// at once, and the findings arrive as a message.
    ReviewCode,
    SubmitReport(SubmitReport),
    QueryBrain(QueryBrain),
    CodeSearch(CodeSearch),
    CodeRefs(CodeRefs),
    ProjectMap,
    RunCheck(RunCheck),
}

impl WorkerCall {
    /// The MCP tool name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::AskOrchestrator(_) => "ask_orchestrator",
            Self::SubmitOutline(_) => "submit_outline",
            Self::ReviewCode => "review_code",
            Self::SubmitReport(_) => "submit_report",
            Self::QueryBrain(_) => "query_brain",
            Self::CodeSearch(_) => "code_search",
            Self::CodeRefs(_) => "code_refs",
            Self::ProjectMap => "project_map",
            Self::RunCheck(_) => "run_check",
        }
    }
}

/// `save_memory`: a Chat keeps something about the user for later conversations.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveMemory {
    /// What to remember, as one short sentence about the user ("Prefers answers in British
    /// English").
    pub memory: String,
}

/// A tool call from a Brain job.
#[derive(Debug, Clone)]
pub enum JobCall {
    RecordNodes(RecordNodes),
    CodeSearch(CodeSearch),
    CodeRefs(CodeRefs),
    ProjectMap,
}

impl JobCall {
    /// The MCP tool name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::RecordNodes(_) => "record_nodes",
            Self::CodeSearch(_) => "code_search",
            Self::CodeRefs(_) => "code_refs",
            Self::ProjectMap => "project_map",
        }
    }
}

/// A tool call from a Chat's model.
#[derive(Debug, Clone)]
pub enum ChatCall {
    SaveMemory(SaveMemory),
}

/// A tool call, for any role.
#[derive(Debug, Clone)]
pub enum ToolCall {
    Orchestrator(OrchestratorCall),
    Worker(WorkerCall),
    Job(JobCall),
    Chat(ChatCall),
}

/// What a tool call returns to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolReply {
    pub text: String,
    /// The call failed or was refused; `text` says why.
    pub is_error: bool,
}

impl ToolReply {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

/// Answers tool calls. Implemented by the session manager.
pub trait ToolHost: Send + Sync {
    /// The role of a live grant; `None` for an unknown or revoked grant.
    fn role(&self, grant: &str) -> Option<Role>;

    /// Runs a tool call. The host re-checks the grant and that its role may make this call.
    fn call(&self, grant: &str, call: ToolCall) -> BoxFuture<'_, ToolReply>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(json: serde_json::Value) -> SubmitReport {
        serde_json::from_value(json).expect("a valid report")
    }

    #[test]
    fn a_report_list_may_be_text_one_item_per_line() {
        let report = report(serde_json::json!({
            "summary": "Done.",
            "verification": "- Ran `pnpm test -- \"src/api\"`: 12 passed.\n\n* Read C:\\repo\\a.ts\nNo bullet",
        }));
        assert_eq!(
            report.verification,
            [
                "Ran `pnpm test -- \"src/api\"`: 12 passed.",
                "Read C:\\repo\\a.ts",
                "No bullet"
            ]
        );
        assert!(report.decisions.is_empty());
    }

    #[test]
    fn a_report_list_may_still_be_a_list_or_null() {
        let report = report(serde_json::json!({
            "summary": "Done.",
            "decisions": ["Kept the old name."],
            "open_questions": null,
        }));
        assert_eq!(report.decisions, ["Kept the old name."]);
        assert!(report.open_questions.is_empty());
    }

    #[test]
    fn done_when_risks_and_needs_user_are_lines_too() {
        let report = report(serde_json::json!({
            "summary": "Done.",
            "done_when": "- [met] `pnpm test` passes: 41 passed\n- [not checked] the smoke run",
            "risks": ["Assumes Node 22."],
            "needs_user": "Set STRIPE_KEY in .env",
        }));
        assert_eq!(
            report.done_when,
            [
                "[met] `pnpm test` passes: 41 passed",
                "[not checked] the smoke run"
            ]
        );
        assert_eq!(report.risks, ["Assumes Node 22."]);
        assert_eq!(report.needs_user, ["Set STRIPE_KEY in .env"]);
    }

    #[test]
    fn placeholder_lines_leave_a_report_list_empty() {
        let report = report(serde_json::json!({
            "summary": "Done.",
            "changes": ["src/a.ts", "None."],
            "decisions": "- None\n- N/A.",
            "done_when": ["[met] tests pass: 41 passed", "none needed"],
            "open_questions": ["None."],
            "risks": "-\nNothing.\n(none)",
            "needs_user": ["  no  ", "n/a", "None for implementation.", "Nothing needed for the build"],
        }));
        assert_eq!(report.changes, ["src/a.ts"]);
        assert!(report.decisions.is_empty());
        assert_eq!(report.done_when, ["[met] tests pass: 41 passed"]);
        assert!(report.open_questions.is_empty());
        assert!(report.risks.is_empty());
        assert!(report.needs_user.is_empty());
        // A real item that starts like a placeholder stays.
        let kept = self::report(serde_json::json!({
            "summary": "Done.",
            "needs_user": "None of the keys are set: add STRIPE_KEY to .env",
        }));
        assert_eq!(
            kept.needs_user,
            ["None of the keys are set: add STRIPE_KEY to .env"]
        );
        // So does a qualified placeholder that goes on to ask for something.
        let asks = self::report(serde_json::json!({
            "summary": "Done.",
            "needs_user": "None for the code; paste one real screenshot in the app",
        }));
        assert_eq!(asks.needs_user.len(), 1);
    }

    #[test]
    fn the_schema_asks_for_text() {
        let schema = serde_json::to_value(schemars::schema_for!(SubmitReport)).expect("schema");
        let verification = &schema["properties"]["verification"];
        assert!(
            verification.to_string().contains("\"string\""),
            "{verification}"
        );
        assert!(
            !verification.to_string().contains("array"),
            "{verification}"
        );
    }
}
