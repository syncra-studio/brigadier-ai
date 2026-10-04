//! The tools each role sees, their descriptions (the models' manual), and the mapping from an
//! MCP `tools/call` onto [`ToolCall`].

use std::sync::{Arc, OnceLock};

use brigadier_core::tools::{
    AcceptTask, AskOrchestrator, AskUser, ChatCall, CodeRefs, CodeSearch, DelegateTask,
    FinishSession, JobCall, MessageWorker, NoteForUser, OrchestratorCall, PhaseDone,
    ProposeOvernight, ProposePhases, ProposePlan, QueryBrain, ReadArtifact, RecordNodes, Remember,
    ReportRef, RequestApproval, Role, RouteFollowUp, SaveMemory, SearchTranscript, SubmitReport,
    TaskRef, ToolCall, WorkerCall,
};
use rmcp::model::{JsonObject, Tool};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::schema::{input_schema, no_arguments};

const DELEGATE_TASK: &str = "Start a worker on one task. Returns at once with the task id \
(e.g. \"task-3\"); the worker runs in the background and its final report arrives later as a \
message in this conversation. Never wait, sleep or poll for it: end your turn, and carry on \
when the report or the user's next message arrives. You can delegate several independent tasks \
at once. The worker sees nothing of this conversation but `spec` (and the listed attachments), \
so make the spec self-contained. `implement` and `merge` tasks change code in their own git \
worktree and land only through accept_task; the other kinds only read and report.";

const MESSAGE_WORKER: &str = "Send text to a running worker: the answer to the question it \
asked you (it is waiting for it), or an instruction that steers its current work. Returns once \
delivered.";

const ROUTE_FOLLOW_UP: &str = "Sort a [follow-up …] the user sent while you work on their \
request. joins=true: it belongs to this work; it reaches you at once as the user's message, and \
your final answer covers it too. joins=false: it is a request of its own; it waits in the user's \
queue and reaches you once this work is done.";

const STOP_WORKER: &str = "Stop a running worker, e.g. when its task is no longer needed or \
went wrong. Nothing of it lands.";

const ASK_USER: &str = "Ask the user a question only they can answer (a product choice, an \
unclear requirement). Returns at once; the answer arrives later as a message. Name the task that \
waits for the answer in `task` so other work continues; `options` become numbered answers (the \
user can always type their own answer), and `recommended` marks the one you recommend.";

const READ_REPORT: &str = "Read a task's final report again: summary, changes, decisions, \
verification, open questions and artifact ids. A report from another session of this project \
is read by the task id a Brain answer names.";

const READ_ARTIFACT: &str = "Read part of an artifact named in a report or a Brain answer (full \
transcript, diff, command output, note), from any session of this project, by its id. At most 16000 bytes per call, starting at `offset`; the \
reply gives the total size so you can page. Read only what you need: everything you read enters \
your context.";

const QUERY_BRAIN: &str = "Ask the Project Brain: what Brigadier knows about this project \
(its modules and services, earlier scout and research findings, decisions, conventions, \
contracts) and the user's preferences, each with where it came from. Fast and cheap: ask it \
first, and delegate a scout only when it has no answer or marks its answer stale. It answers with \
what holds now; set `history` to also see earlier versions and why they changed. When it says \
there are more results, ask again with `page`.";

const REMEMBER: &str = "Keep something that later work must respect: a decision the user \
settled or you made, a convention of this project, a contract between parts, or (personal) a \
preference of the user's that holds in every project. One clear line in `title`, the why in \
`detail`. Do it silently: don't tell the user you remembered it. Returns the node id; pass it \
as `replaces` when a later decision changes it.";

const SEARCH_TRANSCRIPT: &str = "Search this conversation's full transcript (the user's \
messages, your answers, reports and decisions), including what is no longer in your context. \
Returns the best-matching passages with their dates.";

const PROPOSE_PLAN: &str = "Show the user a plan card for multi-step work: a title and the \
steps. A first non-risky plan for a request with at most {small_plan_steps} steps skips \
independent review in an interactive session outside plan mode. Under \"Approve for me\" \
or \"Full access\" Brigadier approves it without review; under \"Ask for approval\" the user \
decides, and no write task may start before approval. Larger non-risky plans get one \
reviewer; set `risky` for big, risky or architectural plans, which get two. The small-plan \
exception never applies to overnight runs, plan mode or revisions. Other non-risky multi-step \
plans also get one reviewer unless their steps were already approved; changed steps and \
plans after exhausted review rounds are reviewed even with one step. Under automatic approval, \
when a review asks for changes, propose the revised plan with `revises` (the plan's id) and \
one response per finding (\"F1 accepted: …\", \"F2 declined: why\"). Every reviewer checks \
the revision again, focusing on prior findings, changed steps and their interactions with \
the rest of the plan; responses alone never resolve findings. The decision is returned \
with the tool result or arrives later as a message.";

const PHASE_DONE: &str = "Only while you lead a phase of an overnight run: say the phase's \
work is done, or as done as it can get without the user. Call it once every task of the phase \
has landed or ended (nothing of it may still run, wait for checks or wait to be accepted). \
Brigadier then checks the whole phase with a fresh verifier, a reviewer from another vendor and \
a fresh judge; the outcome arrives later as a message. After its checks sent you findings, \
call it again once they are fixed, with one response per finding.";

const PROPOSE_PHASES: &str = "Only in Phase 0 of an overnight run (the user gave a goal \
without a plan): propose the plan's phases, each with its exact scope, \"done when\" criteria \
anyone can check, and the phases it builds on. It is reviewed by another vendor and judged \
against the user's goal before any phase starts; nothing beyond the goal belongs in it. When \
the review asks for changes, propose the revision with `revises` and one response per \
finding.";

const REQUEST_APPROVAL: &str = "Ask the user to approve an action Brigadier cannot see on its \
own. Returns at once; the decision arrives later as a message.";

const ACCEPT_TASK: &str = "Land a finished `implement` or `merge` task as one commit on the \
session's branch, with your commit message. Call it after reading the task's report. Brigadier \
first has the change reviewed by another vendor and checks that it can land safely; what happens \
arrives as a message. Meanwhile the next plan step may start if it edits none of this task's \
changed files and doesn't depend on its code or decisions. Set override only when the user \
explicitly told you to land it despite the checks' findings, after those findings reached you: \
the change they found problems in then lands as it is, without being checked again. Brigadier \
refuses it unless the user wrote since.";

const FINISH_SESSION: &str = "New-worktree sessions only: when all the work has landed, ask \
the user to merge the session branch into its base branch (one click on a card). Returns at \
once; the outcome arrives later as a message.";

const NOTE_FOR_USER: &str = "Keep the user's session summary current. kind \"decided\": a \
judgement call you made on the user's behalf that they would want to know (a product or scope \
choice they didn't settle), with why; it shows under \"Decided for you\". kind \"waiting\": \
something only the user can do (a key, a sign-in, an account, a push), in one line; it shows \
under \"Waiting on you\" until they mark it done, and you hear when they do. Brigadier lists its \
own decisions and the workers' needs_user items itself: don't repeat them. Returns at once.";

const LIST_TASKS: &str = "List this session's tasks: id, title, kind, status and model.";

const CODE_SEARCH: &str = "Search the repository's code index (instant; it is kept current \
as files change): symbol definitions by name (functions, types, classes, methods) and files \
by path, best matches first. Use it before grepping to find where things are.";

const CODE_REFS: &str = "Where a symbol is defined and where it is used (calls, type uses), \
from the code index. References are matched by name, without type information.";

const PROJECT_MAP: &str = "The repository at a glance, from the code index: top folders, \
modules and their dependencies, package manifests, scripts (build, test, lint, run), \
services and their ports, and the most used definitions per module.";

const RECORD_NODES: &str = "Record what you found in the Project Brain: one node per module, \
service, important file, convention or contract, each with a short title and a body of a few \
plain sentences (what it is for, what matters about it). Give modules and file summaries their \
repository-relative `path`, and list in `files` the files each was learned from. Call it as \
often as you like; a node with the same kind and path replaces the earlier one. A node that \
replaces one under another title or kind (a stale convention, reworded) names its key in \
`replaces`; the old one is kept as history.";

const SAVE_MEMORY: &str = "Save something about the user that will help in later \
conversations (a preference, their role, what they work on), as one short sentence. Only when \
they state it or clearly imply it holds beyond this chat; never secrets or passing details. The \
user sees it saved and can remove it.";

const ASK_ORCHESTRATOR: &str = "Ask the orchestrator (who gave you this task) a question you \
cannot settle yourself, such as an unclear requirement or a choice outside your task. The call \
blocks until the answer comes back, which can take minutes. Ask only when you cannot sensibly \
go on without the answer.";

const SUBMIT_REPORT: &str = "Submit your final structured report. Call it exactly once, as \
your last action, when the task is done or cannot be done. The orchestrator reads only this \
report, never your messages: the summary must hold your findings (or name the artifact that \
does), never point to text below or in a message. It is final and capped at about 800 tokens: \
keep every field short, and put long material (full findings, logs, command output, notes) in \
files in your outputs folder, listed under `artifacts`. List every file you changed, created or \
deleted under `changes`: new files that are not listed are not kept.";

/// The tools `role` may call, in a stable order. The gate role gets none, and a worker that
/// checks a change or plan no `ask_orchestrator`.
pub fn tools_for(role: &Role) -> &'static [Tool] {
    static ORCHESTRATOR: OnceLock<Vec<Tool>> = OnceLock::new();
    static WORKER: OnceLock<Vec<Tool>> = OnceLock::new();
    static CHECKER: OnceLock<Vec<Tool>> = OnceLock::new();
    static JOB: OnceLock<Vec<Tool>> = OnceLock::new();
    static CHAT: OnceLock<Vec<Tool>> = OnceLock::new();
    match role {
        Role::Orchestrator { .. } => ORCHESTRATOR.get_or_init(orchestrator_tools),
        Role::Worker { checks: false, .. } => WORKER.get_or_init(worker_tools),
        Role::Worker { checks: true, .. } => CHECKER.get_or_init(|| {
            worker_tools()
                .into_iter()
                .filter(|tool| tool.name != "ask_orchestrator")
                .collect()
        }),
        Role::BrainJob { .. } => JOB.get_or_init(job_tools),
        Role::Chat { .. } => CHAT.get_or_init(chat_tools),
        Role::Gate { .. } => &[],
    }
}

fn orchestrator_tools() -> Vec<Tool> {
    vec![
        tool(
            "delegate_task",
            DELEGATE_TASK,
            input_schema::<DelegateTask>(),
        ),
        tool(
            "message_worker",
            MESSAGE_WORKER,
            input_schema::<MessageWorker>(),
        ),
        tool(
            "route_follow_up",
            ROUTE_FOLLOW_UP,
            input_schema::<RouteFollowUp>(),
        ),
        tool("stop_worker", STOP_WORKER, input_schema::<TaskRef>()),
        tool("ask_user", ASK_USER, input_schema::<AskUser>()),
        tool("read_report", READ_REPORT, input_schema::<ReportRef>()),
        tool(
            "read_artifact",
            READ_ARTIFACT,
            input_schema::<ReadArtifact>(),
        ),
        tool("query_brain", QUERY_BRAIN, input_schema::<QueryBrain>()),
        tool("remember", REMEMBER, input_schema::<Remember>()),
        tool(
            "search_transcript",
            SEARCH_TRANSCRIPT,
            input_schema::<SearchTranscript>(),
        ),
        Tool::new(
            "propose_plan",
            PROPOSE_PLAN.replace(
                "{small_plan_steps}",
                &brigadier_core::tools::SMALL_PLAN_STEPS.to_string(),
            ),
            Arc::new(input_schema::<ProposePlan>()),
        ),
        tool(
            "propose_overnight",
            "Fill the user's unstarted overnight proposal from their brief or source files. Keep source phase numbers, dependencies, done-when and Rules verbatim; no invented scope. Include every phase the user's words select, also those after a \"stop after\" or a skip: Brigadier enforces those itself and keeps the rest for Continue. A bare goal keeps empty phases for Phase 0. Does not start, review or implement anything: only the user's Start does that. Use the run_id and revision from the proposal briefing.",
            input_schema::<ProposeOvernight>(),
        ),
        tool(
            "request_approval",
            REQUEST_APPROVAL,
            input_schema::<RequestApproval>(),
        ),
        tool("accept_task", ACCEPT_TASK, input_schema::<AcceptTask>()),
        tool(
            "finish_session",
            FINISH_SESSION,
            input_schema::<FinishSession>(),
        ),
        tool(
            "note_for_user",
            NOTE_FOR_USER,
            input_schema::<NoteForUser>(),
        ),
        tool("list_tasks", LIST_TASKS, no_arguments()),
        tool("phase_done", PHASE_DONE, input_schema::<PhaseDone>()),
        tool(
            "propose_phases",
            PROPOSE_PHASES,
            input_schema::<ProposePhases>(),
        ),
    ]
}

fn worker_tools() -> Vec<Tool> {
    vec![
        tool(
            "ask_orchestrator",
            ASK_ORCHESTRATOR,
            input_schema::<AskOrchestrator>(),
        ),
        tool(
            "submit_report",
            SUBMIT_REPORT,
            input_schema::<SubmitReport>(),
        ),
        tool("code_search", CODE_SEARCH, input_schema::<CodeSearch>()),
        tool("code_refs", CODE_REFS, input_schema::<CodeRefs>()),
        tool("project_map", PROJECT_MAP, no_arguments()),
    ]
}

fn job_tools() -> Vec<Tool> {
    vec![
        tool("record_nodes", RECORD_NODES, input_schema::<RecordNodes>()),
        tool("code_search", CODE_SEARCH, input_schema::<CodeSearch>()),
        tool("code_refs", CODE_REFS, input_schema::<CodeRefs>()),
        tool("project_map", PROJECT_MAP, no_arguments()),
    ]
}

fn chat_tools() -> Vec<Tool> {
    vec![tool(
        "save_memory",
        SAVE_MEMORY,
        input_schema::<SaveMemory>(),
    )]
}

fn tool(name: &'static str, description: &'static str, schema: JsonObject) -> Tool {
    Tool::new(name, description, Arc::new(schema))
}

/// Why a `tools/call` could not become a [`ToolCall`]; shown to the model as a tool error.
#[derive(Debug)]
pub enum ParseError {
    /// Not one of this role's tools.
    UnknownTool(String),
    /// The arguments do not match the tool's schema.
    BadArguments { tool: String, reason: String },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownTool(name) => write!(f, "There is no tool named {name:?} for you."),
            Self::BadArguments { tool, reason } => {
                write!(f, "Invalid arguments for {tool}: {reason}")
            }
        }
    }
}

/// Maps a call of tool `name` with `arguments` onto the core's [`ToolCall`], for `role`.
pub fn parse_call(
    role: &Role,
    name: &str,
    arguments: Option<JsonObject>,
) -> Result<ToolCall, ParseError> {
    let mut arguments = arguments.unwrap_or_default();
    // Models sometimes send `null` for an optional argument; it means "left out".
    arguments.retain(|_, value| !value.is_null());
    let arguments = Value::Object(arguments);
    let unknown = || ParseError::UnknownTool(name.to_owned());
    match role {
        Role::Orchestrator { .. } => {
            let call = match name {
                "delegate_task" => OrchestratorCall::DelegateTask(args(name, arguments)?),
                "message_worker" => OrchestratorCall::MessageWorker(args(name, arguments)?),
                "route_follow_up" => OrchestratorCall::RouteFollowUp(args(name, arguments)?),
                "stop_worker" => OrchestratorCall::StopWorker(args(name, arguments)?),
                "ask_user" => OrchestratorCall::AskUser(args(name, arguments)?),
                "read_report" => OrchestratorCall::ReadReport(args(name, arguments)?),
                "read_artifact" => OrchestratorCall::ReadArtifact(args(name, arguments)?),
                "query_brain" => OrchestratorCall::QueryBrain(args(name, arguments)?),
                "remember" => OrchestratorCall::Remember(args(name, arguments)?),
                "search_transcript" => {
                    OrchestratorCall::SearchTranscript(args::<SearchTranscript>(name, arguments)?)
                }
                "propose_plan" => OrchestratorCall::ProposePlan(args(name, arguments)?),
                "request_approval" => OrchestratorCall::RequestApproval(args(name, arguments)?),
                "accept_task" => OrchestratorCall::AcceptTask(args(name, arguments)?),
                "finish_session" => OrchestratorCall::FinishSession(args(name, arguments)?),
                "note_for_user" => OrchestratorCall::NoteForUser(args(name, arguments)?),
                "list_tasks" => OrchestratorCall::ListTasks,
                "phase_done" => OrchestratorCall::PhaseDone(args(name, arguments)?),
                "propose_phases" => OrchestratorCall::ProposePhases(args(name, arguments)?),
                "propose_overnight" => OrchestratorCall::ProposeOvernight(args(name, arguments)?),
                _ => return Err(unknown()),
            };
            Ok(ToolCall::Orchestrator(call))
        }
        Role::Worker { checks, .. } => {
            let call = match name {
                "ask_orchestrator" if !checks => {
                    WorkerCall::AskOrchestrator(args::<AskOrchestrator>(name, arguments)?)
                }
                "submit_report" => WorkerCall::SubmitReport(args::<SubmitReport>(name, arguments)?),
                "code_search" => WorkerCall::CodeSearch(args::<CodeSearch>(name, arguments)?),
                "code_refs" => WorkerCall::CodeRefs(args::<CodeRefs>(name, arguments)?),
                "project_map" => WorkerCall::ProjectMap,
                _ => return Err(unknown()),
            };
            Ok(ToolCall::Worker(call))
        }
        Role::BrainJob { .. } => {
            let call = match name {
                "record_nodes" => JobCall::RecordNodes(args::<RecordNodes>(name, arguments)?),
                "code_search" => JobCall::CodeSearch(args::<CodeSearch>(name, arguments)?),
                "code_refs" => JobCall::CodeRefs(args::<CodeRefs>(name, arguments)?),
                "project_map" => JobCall::ProjectMap,
                _ => return Err(unknown()),
            };
            Ok(ToolCall::Job(call))
        }
        Role::Chat { .. } => match name {
            "save_memory" => Ok(ToolCall::Chat(ChatCall::SaveMemory(args::<SaveMemory>(
                name, arguments,
            )?))),
            _ => Err(unknown()),
        },
        Role::Gate { .. } => Err(unknown()),
    }
}

fn args<T: DeserializeOwned>(tool: &str, arguments: Value) -> Result<T, ParseError> {
    serde_json::from_value(arguments).map_err(|err| ParseError::BadArguments {
        tool: tool.to_owned(),
        reason: err.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker(checks: bool) -> Role {
        Role::Worker {
            conversation_id: brigadier_core::model::ConversationId("c1".into()),
            task_id: brigadier_core::model::TaskId("t1".into()),
            checks,
        }
    }

    #[test]
    fn plan_description_explains_small_plans_and_revision_reviews() {
        let tools = orchestrator_tools();
        let plan = tools
            .iter()
            .find(|tool| tool.name == "propose_plan")
            .unwrap();
        let description = plan.description.as_deref().unwrap();
        assert!(!description.contains("two or more steps"));
        assert!(description.contains(&format!(
            "at most {} steps",
            brigadier_core::tools::SMALL_PLAN_STEPS
        )));
        assert!(description.contains("approves it without review"));
        assert!(description.contains("Larger non-risky plans get one reviewer"));
        assert!(description.contains("which get two"));
        assert!(description.contains("prior findings, changed steps and their interactions"));
        assert!(description.contains("responses alone never resolve findings"));
    }

    #[test]
    fn a_gate_member_cannot_ask_the_orchestrator() {
        let names = |role: &Role| -> Vec<String> {
            tools_for(role)
                .iter()
                .map(|tool| tool.name.to_string())
                .collect()
        };
        assert!(names(&worker(false)).contains(&"ask_orchestrator".to_owned()));
        let checker = names(&worker(true));
        assert!(!checker.contains(&"ask_orchestrator".to_owned()));
        assert!(checker.contains(&"submit_report".to_owned()));
        let question = || {
            let mut arguments = JsonObject::new();
            arguments.insert("question".into(), Value::String("Which file?".into()));
            Some(arguments)
        };
        assert!(parse_call(&worker(false), "ask_orchestrator", question()).is_ok());
        assert!(matches!(
            parse_call(&worker(true), "ask_orchestrator", question()),
            Err(ParseError::UnknownTool(_))
        ));
    }
}
