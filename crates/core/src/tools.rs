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
//! - [`Role::Gate`] may only ask whether an outward command may run ([`ToolHost::ask_outward`]).
//!
//! The MCP server (`crates/mcp-server`) maps MCP tool calls onto these types; the session
//! manager implements [`ToolHost`].

use std::collections::HashMap;
use std::sync::Mutex;

use brigadier_providers::BoxFuture;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer};

use crate::model::{ConversationId, ProjectId, TaskId};
use crate::work::{ChecksResult, ReviewVerdict, TaskKind};

/// What a grant allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    /// The orchestrator of a session.
    Orchestrator { conversation_id: ConversationId },
    /// The worker running one task.
    Worker {
        conversation_id: ConversationId,
        task_id: TaskId,
        /// It checks a change or plan in a gate: it decides from what it was given and its
        /// own evidence alone, so it has no `ask_orchestrator`.
        checks: bool,
    },
    /// The outward-command gate of a CLI session: it can only ask.
    Gate {
        conversation_id: ConversationId,
        task_id: Option<TaskId>,
    },
    /// A Brain job (skeleton pass, enrichment) of a project: it reads and records nodes.
    BrainJob {
        project_id: ProjectId,
        job_id: String,
    },
    /// A Chat's model: it may save memories to the Personal Brain.
    Chat { conversation_id: ConversationId },
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
    /// A short title for the task card, e.g. "Add the --json flag to `list`".
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
    /// `merge` tasks: the task whose work conflicts with the branch it lands on.
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
    /// For a task that carries out a step of the approved plan: that step's number (1 is the
    /// first step). The user follows the plan's progress by it.
    #[serde(default)]
    pub step: Option<u32>,
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

/// One step of a proposed plan.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanStepInput {
    /// The step, in a few words.
    pub title: String,
    /// What it involves, if the title is not enough.
    #[serde(default)]
    pub detail: Option<String>,
}

/// First, non-risky interactive proposals up to this size need no independent review.
pub const SMALL_PLAN_STEPS: usize = 3;

/// `propose_plan`: show a plan card. Under "Ask for approval" the user approves it before any
/// write task starts.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposePlan {
    /// What the plan achieves.
    pub title: String,
    /// The steps, in order.
    pub steps: Vec<PlanStepInput>,
    /// True for big, risky or architectural plans: they get two independent reviewers.
    /// Non-risky plans of two or more steps normally get one; eligible first small
    /// interactive plans skip review.
    #[serde(default)]
    pub risky: bool,
    /// The id of the plan this one revises after its review asked for changes.
    #[serde(default)]
    pub revises: Option<String>,
    /// With `revises`: one line per finding of that review, "F1 accepted: what changed" or
    /// "F2 declined: why".
    #[serde(default)]
    pub responses: Vec<String>,
}

/// `request_approval`: ask the user to approve an action Brigadier cannot see otherwise.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequestApproval {
    /// What would happen, in one line.
    pub action: String,
    /// Why, and what exactly.
    pub details: String,
}

/// `accept_task`: land a reported write task as one reviewed commit.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AcceptTask {
    /// The task, e.g. "task-3".
    pub task: String,
    /// The commit message: a short subject line, a blank line, then the body.
    pub commit_message: String,
    /// Only when the user explicitly told you to land it despite the checks' findings: it
    /// lands as it is, without being checked again.
    #[serde(default, rename = "override")]
    pub override_checks: bool,
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

/// `phase_done`: the lead of an overnight phase says its work is done (or as done as it can
/// get without the user), so Brigadier checks the whole phase.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhaseDone {
    /// What the phase changed and how it was verified, in a few sentences, and anything left
    /// for the user.
    pub summary: String,
    /// After the phase's checks found gaps: one line per finding, "F1 fixed: how" or "F2
    /// declined: why".
    #[serde(default, deserialize_with = "lines")]
    #[schemars(with = "String", extend("default" = ""))]
    pub responses: Vec<String>,
}

/// One phase of a plan Phase 0 writes.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhaseInput {
    /// The phase, in a few words.
    pub name: String,
    /// Exactly what it covers.
    pub scope: String,
    /// Its "done when" criteria, one per line, each checkable by running something or reading
    /// the code.
    #[serde(deserialize_with = "lines")]
    #[schemars(with = "String")]
    pub done_when: Vec<String>,
    /// Numbers (1-based, in this list) of the phases it builds on.
    #[serde(default)]
    pub depends_on: Vec<u32>,
}

/// `propose_phases`: Phase 0 of an overnight run with a bare goal writes the plan's phases.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposePhases {
    /// The plan's name, a few words ("Windows support").
    pub name: String,
    pub phases: Vec<PhaseInput>,
    /// The id of the plan this one revises after its review asked for changes.
    #[serde(default)]
    pub revises: Option<String>,
    /// With `revises`: one line per finding of that review, "F1 accepted: what changed" or
    /// "F2 declined: why".
    #[serde(default)]
    pub responses: Vec<String>,
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
    RouteFollowUp(RouteFollowUp),
    StopWorker(TaskRef),
    AskUser(AskUser),
    ReadReport(ReportRef),
    ReadArtifact(ReadArtifact),
    QueryBrain(QueryBrain),
    Remember(Remember),
    SearchTranscript(SearchTranscript),
    ProposePlan(ProposePlan),
    RequestApproval(RequestApproval),
    AcceptTask(AcceptTask),
    FinishSession(FinishSession),
    NoteForUser(NoteForUser),
    ListTasks,
    PhaseDone(PhaseDone),
    ProposePhases(ProposePhases),
    ProposeOvernight(ProposeOvernight),
}

impl OrchestratorCall {
    /// The MCP tool name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::DelegateTask(_) => "delegate_task",
            Self::MessageWorker(_) => "message_worker",
            Self::RouteFollowUp(_) => "route_follow_up",
            Self::StopWorker(_) => "stop_worker",
            Self::AskUser(_) => "ask_user",
            Self::ReadReport(_) => "read_report",
            Self::ReadArtifact(_) => "read_artifact",
            Self::QueryBrain(_) => "query_brain",
            Self::Remember(_) => "remember",
            Self::SearchTranscript(_) => "search_transcript",
            Self::ProposePlan(_) => "propose_plan",
            Self::RequestApproval(_) => "request_approval",
            Self::AcceptTask(_) => "accept_task",
            Self::FinishSession(_) => "finish_session",
            Self::NoteForUser(_) => "note_for_user",
            Self::ListTasks => "list_tasks",
            Self::PhaseDone(_) => "phase_done",
            Self::ProposePhases(_) => "propose_phases",
            Self::ProposeOvernight(_) => "propose_overnight",
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

/// Whether a report line only says the list is empty ("None.", "N/A", "-"): some models fill
/// every section, and a placeholder under needs_user must not read as something for the user.
fn empty_item(item: &str) -> bool {
    const PLACEHOLDERS: [&str; 6] = ["none", "n/a", "na", "nothing", "no", "none needed"];
    let item = item
        .trim()
        .trim_matches(|c: char| {
            c.is_whitespace() || c.is_ascii_punctuation() || matches!(c, '—' | '–')
        })
        .to_lowercase();
    item.is_empty() || PLACEHOLDERS.contains(&item.as_str())
}

/// A tool call from a worker.
#[derive(Debug, Clone)]
pub enum WorkerCall {
    AskOrchestrator(AskOrchestrator),
    SubmitReport(SubmitReport),
    CodeSearch(CodeSearch),
    CodeRefs(CodeRefs),
    ProjectMap,
}

impl WorkerCall {
    /// The MCP tool name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::AskOrchestrator(_) => "ask_orchestrator",
            Self::SubmitReport(_) => "submit_report",
            Self::CodeSearch(_) => "code_search",
            Self::CodeRefs(_) => "code_refs",
            Self::ProjectMap => "project_map",
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

/// The user's decision on an outward command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateAnswer {
    Allow,
    Deny { message: String },
}

/// Answers tool calls and gate questions. Implemented by the session manager.
pub trait ToolHost: Send + Sync {
    /// The role of a live grant; `None` for an unknown or revoked grant.
    fn role(&self, grant: &str) -> Option<Role>;

    /// Runs a tool call. The host re-checks the grant and that its role may make this call.
    fn call(&self, grant: &str, call: ToolCall) -> BoxFuture<'_, ToolReply>;

    /// Asks the user whether an outward command (PLAN §5 always-ask list) may run, and waits
    /// for the answer. `argv` is the full command line as the program received it, `cwd` the
    /// directory it runs in; an approval is bound to exactly these.
    fn ask_outward(&self, grant: &str, argv: Vec<String>, cwd: String)
    -> BoxFuture<'_, GateAnswer>;
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
            "needs_user": ["  no  ", "n/a"],
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
