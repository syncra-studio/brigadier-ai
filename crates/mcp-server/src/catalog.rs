//! The tools each role sees, their descriptions (the models' manual), and the mapping from an
//! MCP `tools/call` onto [`ToolCall`].

use std::sync::{Arc, OnceLock};

use brigadier_core::tools::{
    AnswerWorker, ApproveOutline, AskOrchestrator, AskUser, ChatCall, CodeRefs, CodeSearch,
    DelegateTask, EndRun, FinishSession, JobCall, LandPhase, MessageWorker, NoteForUser,
    OrchestratorCall, PlanPhases, PreviewLog, ProposeOvernight, QueryBrain, ReadArtifact,
    RecordNodes, Remember, ReportRef, RequestApproval, ReviewPlan, Role, RouteFollowUp, RunCheck,
    RunCommand, RunTools, RunUnsandboxed, SaveMemory, SearchTranscript, SettleStep, StartPreview,
    StopPreview, StopWorker, SubmitOutline, SubmitReport, TaskRef, ToolCall, WorkerCall,
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
worktree, commit their own steps and land only through land_phase; the other kinds only read \
and report.";

const MESSAGE_WORKER: &str = "Send text to a worker: an instruction that steers its current \
work, or sends a reported worker back to work. Answer a worker's question with answer_worker. \
Returns once delivered.";

const ANSWER_WORKER: &str = "Answer the question a worker asked you (it waits for the answer). \
Answer at once and yourself: take the worker's recommendation if it fits, else what the brief, \
the outline, the user's words or the Brain imply; never reopen a decision already settled. Ask \
the user only what truly only they can decide. `why` is a few words the user reads next to the \
answer.";

const ROUTE_FOLLOW_UP: &str = "Sort a [follow-up …] the user sent while you work on their \
request. joins=true: it belongs to this work; it reaches you at once as the user's message, and \
your final answer covers it too. joins=false: it is a request of its own; it waits in the user's \
queue and reaches you once this work is done.";

const STOP_WORKER: &str = "Stop a running worker, e.g. when its task is no longer needed or \
went wrong. Nothing of it lands. `reason` is required: one plain line on why, which the user \
reads on the thread's \"Stopped\" row (e.g. \"No longer needed: the user dropped the export\").";

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

const PLAN_PHASES: &str = "Split a big request into phases that must run one after another \
(each builds on the one before). Most requests are one phase: then skip this and delegate the \
lead. Records the phases for the user's progress pill; nothing is reviewed or approved here. \
Then delegate phase 1's lead (delegate_task, kind implement, phase 1), and each next phase once \
the one before it has landed.";

const APPROVE_OUTLINE: &str = "Give a lead the go-ahead on its outline as soon as you have \
judged it: don't wait for its plan review, which runs in the background and may arrive after the \
go-ahead (then send the lead the findings you agree with by message_worker). Put what the \
outline gets wrong, and anything the brief implies, in `corrections`; the brief wins any \
conflict. Under \"Ask for approval\" this shows the user a \"Start this plan?\" card and the \
go-ahead goes once they start it. Returns at once.";

const START_VERIFIER: &str = "Your call, for big or risky work only: start a fresh verifier \
on top of a reported implement task's commits. It asks for a review of the whole work by the \
other vendor, checks every \"done when\" for real, fixes and commits what fails, triages the \
review's findings and reports. Land the verifier's task then (its commits hold the lead's), not \
the lead's. Small work needs none: land the lead. Returns at once.";

const SETTLE_STEP: &str = "Only during an overnight run: settle a step of the run's plan, by \
its number, once you have judged its whole scope and every \"done when\" (your own edits \
too) and nothing of it still runs or waits to land. Done: all of it is met and landed; partial: \
some is left for a later run; blocked: what is left needs the user. A step settled done moves \
the tip the user's Merge takes, once every step before it is done too.";

const END_RUN: &str = "Only during an overnight run: end it now, because every selected step \
is settled (done) or all that is left needs the user (needsUser). Brigadier then ends the run \
cleanly and writes its report; you write the user's morning answer.";

const REQUEST_APPROVAL: &str = "Ask the user to approve what only they may decide: spending \
money, using credentials or the keychain, or destroying something outside this session's own \
work. Not for pushes, pull requests or deploys the user asked for (just do those) nor for work \
inside the session. Returns at once; the decision arrives later as a message.";

const REVIEW_CODE: &str = "Start one review of your committed work by a model from the other \
vendor, from where your work started to your last commit (uncommitted changes are committed for \
you first). Returns at once: keep working, and the findings arrive later as a message from \
Brigadier (if you have nothing left to do before them, end your turn; they start your next \
one). Fix what you agree with and commit; for a finding you don't, say why in your report. It is \
advisory and there are no rounds: call it once.";

const REVIEW_PLAN: &str = "Your call, for a plan of your own that is big or risky: start one \
review of it by a model from the other vendor, against the brief you give. Returns at once: \
carry on (delegate, or wait for nothing), and the findings arrive later as a [plan review …] \
message. Weigh them against the brief; there are no rounds.";

const LAND_PHASE: &str = "Land a finished `implement` or `merge` task's commits on the \
session's branch: the lead's task once its report is in, or a verifier you started for big or \
risky work (its commits hold the lead's). Call it after reading the report. Brigadier commits \
what was left uncommitted, leaves litter out, and fast-forwards the branch; no card, no further \
checks. Each landing gets one background review by the other vendor, unless the same commits \
were reviewed already; its findings arrive later as a [review …] message. If the branch moved meanwhile, the commits are rebased and the worker runs a \
quick self-check first; then they land on their own and you hear when. Conflicts come back to \
you: delegate a merge task.";

const FINISH_SESSION: &str = "New-worktree sessions only: merge the session branch into its \
base branch, once the user's latest message asks for it (\"merge it\", also together with the \
work: then merge as soon as it has landed, without asking again) or plainly agrees to the merge \
your reply right before proposed, as a question naming the base (\"yes\"). Pass their \
words in user_words, quoted exactly from that message. Brigadier checks them against it and \
refuses on a question, a condition, a \"no\" or a \"wait\", or words already used for a merge; \
then propose it and wait for their answer. Never merge on silence. Returns when merged.";

const NOTE_FOR_USER: &str = "Keep the user's session summary current. kind \"decided\": a \
judgement call you made on the user's behalf that they would want to know (a product or scope \
choice they didn't settle), with why; it shows under \"Decided for you\". kind \"waiting\": \
something only the user can do (a key, a sign-in, an account, a push), in one line; it shows \
under \"Waiting on you\" until they mark it done, and you hear when they do. Brigadier lists its \
own decisions and the workers' needs_user items itself: don't repeat them. Returns at once.";

const RUN: &str = "Run a shell command and get its result: use it for builds, tests, logs and \
long listings, anything that prints a lot. It runs with this session's access, in the same \
sandbox as your own shell, in `workdir` (the workspace by default; it must be inside the \
workspace or your scratch folder). The whole output is kept: up to 8 KB comes back as it is, \
longer output as a digest (the exit status, the error and warning lines, the first and last \
lines) with an out-… id; page through the rest with read_artifact. `timeout_secs` defaults to \
600 (at most 1800); the command is stopped then.";

const RUN_CHECK: &str = "Run a check (tests, lint, typecheck, build, formatting) in your \
checkout and get its result: use it for every check rather than your shell. A check that \
already ran on the same files (committed or not) answers at once from the cache, and says so; \
`rerun: true` runs it again. Call it first with no `command`: it answers with the checks your \
changes call for (the changed packages and those that depend on them) and runs nothing. It runs \
with your access in `workdir` (relative to your checkout's root, the root by default). Up to 8 KB \
of output comes back as it is, longer output as a digest (the exit status, the error and warning \
lines, the first and last lines) with where to read the whole of it. `timeout_secs` defaults to \
600 (at most 1800); the command is stopped then.";

const RUN_UNSANDBOXED: &str = "Like run, but outside the sandbox, for a command the sandbox \
blocked (the network, files outside the workspace): it waits for approval first, decided on \
your `justification`. Try run first; use this only when the sandbox is what stopped it.";

const START_PREVIEW: &str = "Start something the user wants to see running (a dev server, \
the app, a docs site) and keep it running: it lives across your turns until you stop it, and \
answers at once with its first output. Run the command in the foreground (no trailing `&`). It \
runs in the workspace (or `workdir` inside it) with this session's access, in its own process \
group. Tell the user where to look (the URL and port). It stops when you call stop_preview, \
when the user presses Stop, and when the session is merged, archived or deleted; for one-off \
commands use your shell instead.";

const STOP_PREVIEW: &str = "Stop a preview (`id`, e.g. \"preview-1\") or, without an id, \
every running one: SIGTERM, then a kill after a few seconds. Returns how each ended.";

const PREVIEW_LOG: &str = "The last lines of a preview's output (stdout and stderr together; \
the latest preview's by default), with its state and an out-… id to page through its whole log \
with read_artifact.";

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
cannot settle yourself, such as an unclear requirement or a choice outside your task. One \
question per call, with the options you see and the one you recommend. The call blocks until \
the answer comes back. Ask only when you cannot sensibly go on without the answer; look things \
up with query_brain and the code tools first.";

const WORKER_QUERY_BRAIN: &str = "Ask the Project Brain (read-only): what Brigadier knows \
about this project (its modules, earlier findings, decisions, conventions, contracts) and the \
user's preferences, each with where it came from. Fast and cheap: ask it before reading widely. \
It answers with what holds now; set `history` to also see earlier versions. When it says there \
are more results, ask again with `page`.";

const SUBMIT_OUTLINE: &str = "Leads only, when the work is multi-step or risky: after \
reading the code, send your outline (the steps in order with the files each touches, how you \
will check each \"done when\", risks, and any question with your recommendation), then end your \
turn. The go-ahead, with any corrections, arrives as your next message; build nothing before \
it. Small work needs no outline: just do it.";

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
    static RUNNER: OnceLock<Vec<Tool>> = OnceLock::new();
    static ESCALATING: OnceLock<Vec<Tool>> = OnceLock::new();
    static WORKER: OnceLock<Vec<Tool>> = OnceLock::new();
    static CHECKER: OnceLock<Vec<Tool>> = OnceLock::new();
    static JOB: OnceLock<Vec<Tool>> = OnceLock::new();
    static CHAT: OnceLock<Vec<Tool>> = OnceLock::new();
    static COMPUTER: OnceLock<Vec<Tool>> = OnceLock::new();
    match role {
        Role::Orchestrator { run, .. } => match run {
            RunTools::None => ORCHESTRATOR.get_or_init(orchestrator_tools),
            RunTools::Run => RUNNER.get_or_init(|| {
                let mut tools = orchestrator_tools();
                tools.push(tool("run", RUN, input_schema::<RunCommand>()));
                tools
            }),
            RunTools::WithEscalation => ESCALATING.get_or_init(|| {
                let mut tools = orchestrator_tools();
                tools.push(tool("run", RUN, input_schema::<RunCommand>()));
                tools.push(tool(
                    "run_unsandboxed",
                    RUN_UNSANDBOXED,
                    input_schema::<RunUnsandboxed>(),
                ));
                tools
            }),
        },
        Role::Worker { checks: false, .. } => WORKER.get_or_init(worker_tools),
        Role::Worker { checks: true, .. } => CHECKER.get_or_init(|| {
            worker_tools()
                .into_iter()
                .filter(|tool| {
                    !matches!(
                        tool.name.as_ref(),
                        "ask_orchestrator" | "submit_outline" | "review_code"
                    )
                })
                .collect()
        }),
        Role::BrainJob { .. } => JOB.get_or_init(job_tools),
        Role::Chat { .. } => CHAT.get_or_init(chat_tools),
        Role::OutputHook { .. } => &[],
        Role::Computer { .. } => COMPUTER.get_or_init(crate::computer::tools),
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
            "answer_worker",
            ANSWER_WORKER,
            input_schema::<AnswerWorker>(),
        ),
        tool(
            "route_follow_up",
            ROUTE_FOLLOW_UP,
            input_schema::<RouteFollowUp>(),
        ),
        tool("stop_worker", STOP_WORKER, input_schema::<StopWorker>()),
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
        tool("plan_phases", PLAN_PHASES, input_schema::<PlanPhases>()),
        tool(
            "approve_outline",
            APPROVE_OUTLINE,
            input_schema::<ApproveOutline>(),
        ),
        tool("start_verifier", START_VERIFIER, input_schema::<TaskRef>()),
        tool(
            "propose_overnight",
            "Fill the user's unstarted overnight proposal from their brief or source files. Keep source phase numbers, dependencies, done-when and Rules verbatim; no invented scope. Include every phase the user's words select, also those after a \"stop after\" or a skip: Brigadier enforces those itself and keeps the rest for Continue. A bare goal keeps empty phases: the thread plans it once the run starts. Does not start, review or implement anything: only the user's Start does that. Use the run_id and revision from the proposal briefing.",
            input_schema::<ProposeOvernight>(),
        ),
        tool(
            "request_approval",
            REQUEST_APPROVAL,
            input_schema::<RequestApproval>(),
        ),
        tool("land_phase", LAND_PHASE, input_schema::<LandPhase>()),
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
        tool("settle_step", SETTLE_STEP, input_schema::<SettleStep>()),
        tool("end_run", END_RUN, input_schema::<EndRun>()),
        tool("code_search", CODE_SEARCH, input_schema::<CodeSearch>()),
        tool("code_refs", CODE_REFS, input_schema::<CodeRefs>()),
        tool("project_map", PROJECT_MAP, no_arguments()),
        tool("review_plan", REVIEW_PLAN, input_schema::<ReviewPlan>()),
        tool(
            "start_preview",
            START_PREVIEW,
            input_schema::<StartPreview>(),
        ),
        tool("stop_preview", STOP_PREVIEW, input_schema::<StopPreview>()),
        tool("preview_log", PREVIEW_LOG, input_schema::<PreviewLog>()),
        tool("run_check", RUN_CHECK, input_schema::<RunCheck>()),
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
            "submit_outline",
            SUBMIT_OUTLINE,
            input_schema::<SubmitOutline>(),
        ),
        tool("review_code", REVIEW_CODE, no_arguments()),
        tool(
            "submit_report",
            SUBMIT_REPORT,
            input_schema::<SubmitReport>(),
        ),
        tool(
            "query_brain",
            WORKER_QUERY_BRAIN,
            input_schema::<QueryBrain>(),
        ),
        tool("code_search", CODE_SEARCH, input_schema::<CodeSearch>()),
        tool("code_refs", CODE_REFS, input_schema::<CodeRefs>()),
        tool("project_map", PROJECT_MAP, no_arguments()),
        tool("run_check", RUN_CHECK, input_schema::<RunCheck>()),
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
        Role::Orchestrator { run, .. } => {
            let call = match name {
                "delegate_task" => OrchestratorCall::DelegateTask(args(name, arguments)?),
                "message_worker" => OrchestratorCall::MessageWorker(args(name, arguments)?),
                "answer_worker" => OrchestratorCall::AnswerWorker(args(name, arguments)?),
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
                "plan_phases" => OrchestratorCall::PlanPhases(args(name, arguments)?),
                "approve_outline" => OrchestratorCall::ApproveOutline(args(name, arguments)?),
                "start_verifier" => OrchestratorCall::StartVerifier(args(name, arguments)?),
                "request_approval" => OrchestratorCall::RequestApproval(args(name, arguments)?),
                "land_phase" => OrchestratorCall::LandPhase(args(name, arguments)?),
                "finish_session" => OrchestratorCall::FinishSession(args(name, arguments)?),
                "note_for_user" => OrchestratorCall::NoteForUser(args(name, arguments)?),
                "list_tasks" => OrchestratorCall::ListTasks,
                "settle_step" => OrchestratorCall::SettleStep(args(name, arguments)?),
                "end_run" => OrchestratorCall::EndRun(args(name, arguments)?),
                "propose_overnight" => OrchestratorCall::ProposeOvernight(args(name, arguments)?),
                "code_search" => OrchestratorCall::CodeSearch(args(name, arguments)?),
                "code_refs" => OrchestratorCall::CodeRefs(args(name, arguments)?),
                "project_map" => OrchestratorCall::ProjectMap,
                "review_plan" => OrchestratorCall::ReviewPlan(args(name, arguments)?),
                "start_preview" => OrchestratorCall::StartPreview(args(name, arguments)?),
                "stop_preview" => OrchestratorCall::StopPreview(args(name, arguments)?),
                "preview_log" => OrchestratorCall::PreviewLog(args(name, arguments)?),
                "run_check" => OrchestratorCall::RunCheck(args(name, arguments)?),
                "run" if *run != RunTools::None => OrchestratorCall::Run(args(name, arguments)?),
                "run_unsandboxed" if *run == RunTools::WithEscalation => {
                    OrchestratorCall::RunUnsandboxed(args(name, arguments)?)
                }
                _ => return Err(unknown()),
            };
            Ok(ToolCall::Orchestrator(call))
        }
        Role::Worker { checks, .. } => {
            let call = match name {
                "ask_orchestrator" if !checks => {
                    WorkerCall::AskOrchestrator(args::<AskOrchestrator>(name, arguments)?)
                }
                "submit_outline" if !checks => {
                    WorkerCall::SubmitOutline(args::<SubmitOutline>(name, arguments)?)
                }
                "review_code" if !checks => WorkerCall::ReviewCode,
                "submit_report" => WorkerCall::SubmitReport(args::<SubmitReport>(name, arguments)?),
                "query_brain" => WorkerCall::QueryBrain(args::<QueryBrain>(name, arguments)?),
                "code_search" => WorkerCall::CodeSearch(args::<CodeSearch>(name, arguments)?),
                "code_refs" => WorkerCall::CodeRefs(args::<CodeRefs>(name, arguments)?),
                "project_map" => WorkerCall::ProjectMap,
                "run_check" => WorkerCall::RunCheck(args::<RunCheck>(name, arguments)?),
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
        Role::OutputHook { .. } => Err(unknown()),
        Role::Computer { .. } => crate::computer::parse(name, arguments),
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
    fn phases_are_planned_and_outlines_approved_without_review_rounds() {
        let tools = orchestrator_tools();
        let names: Vec<_> = tools.iter().map(|tool| tool.name.to_string()).collect();
        assert!(names.contains(&"plan_phases".to_owned()));
        assert!(names.contains(&"approve_outline".to_owned()));
        assert!(!names.contains(&"propose_plan".to_owned()));
        let approve = tools
            .iter()
            .find(|tool| tool.name == "approve_outline")
            .unwrap();
        let description = approve.description.as_deref().unwrap();
        assert!(description.contains("don't wait for its plan review"));
        assert!(description.contains("Start this plan?"));
        assert!(names.contains(&"start_verifier".to_owned()));
    }

    #[test]
    fn the_thread_searches_the_code_index_and_asks_for_plan_reviews() {
        let thread = Role::Orchestrator {
            conversation_id: brigadier_core::model::ConversationId("c1".into()),
            run: RunTools::None,
        };
        let names: Vec<String> = tools_for(&thread)
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        for name in [
            "code_search",
            "code_refs",
            "project_map",
            "review_plan",
            "delegate_task",
            "land_phase",
            "finish_session",
        ] {
            assert!(names.contains(&name.to_owned()), "{name}");
        }
        let mut arguments = JsonObject::new();
        arguments.insert("query".into(), Value::String("ConvLive".into()));
        assert!(matches!(
            parse_call(&thread, "code_search", Some(arguments)),
            Ok(ToolCall::Orchestrator(OrchestratorCall::CodeSearch(_)))
        ));
        assert!(matches!(
            parse_call(&thread, "project_map", None),
            Ok(ToolCall::Orchestrator(OrchestratorCall::ProjectMap))
        ));
        let mut arguments = JsonObject::new();
        arguments.insert("plan".into(), Value::String("1. Edit a.rs".into()));
        arguments.insert("brief".into(), Value::String("Fix the bug".into()));
        assert!(matches!(
            parse_call(&thread, "review_plan", Some(arguments)),
            Ok(ToolCall::Orchestrator(OrchestratorCall::ReviewPlan(_)))
        ));
    }

    /// The lead names a worker's effort for every task (THREAD-UX-PLAN.md §4.1 b): the schema
    /// requires it, and a call without it is refused.
    #[test]
    fn delegate_task_needs_an_effort() {
        let thread = Role::Orchestrator {
            conversation_id: brigadier_core::model::ConversationId("c1".into()),
            run: RunTools::None,
        };
        let delegate = tools_for(&thread)
            .iter()
            .find(|tool| tool.name == "delegate_task")
            .unwrap();
        let required = delegate.input_schema.get("required").unwrap();
        assert!(
            required
                .as_array()
                .unwrap()
                .contains(&Value::from("effort"))
        );
        let call = |effort: Option<&str>| {
            let mut arguments = JsonObject::new();
            arguments.insert("title".into(), Value::from("Add a flag"));
            arguments.insert("kind".into(), Value::from("implement"));
            arguments.insert("spec".into(), Value::from("Add the flag."));
            if let Some(effort) = effort {
                arguments.insert("effort".into(), Value::from(effort));
            }
            parse_call(&thread, "delegate_task", Some(arguments))
        };
        assert!(call(None).is_err());
        assert!(matches!(
            call(Some("medium")),
            Ok(ToolCall::Orchestrator(OrchestratorCall::DelegateTask(_)))
        ));
    }

    /// `run` is a Codex thread's, and `run_unsandboxed` only a sandboxed one's; a Claude
    /// thread (its Bash output is trimmed by its hook) and the hook's own grant have neither.
    #[test]
    fn only_a_codex_thread_runs_commands_through_brigadier() {
        let thread = |run| Role::Orchestrator {
            conversation_id: brigadier_core::model::ConversationId("c1".into()),
            run,
        };
        let names = |role: &Role| -> Vec<String> {
            tools_for(role)
                .iter()
                .map(|tool| tool.name.to_string())
                .collect()
        };
        let command = || {
            let mut arguments = JsonObject::new();
            arguments.insert("command".into(), Value::String("cargo test".into()));
            arguments.insert("timeout_secs".into(), Value::from(60));
            Some(arguments)
        };
        let claude = names(&thread(RunTools::None));
        assert!(
            !claude
                .iter()
                .any(|name| name == "run" || name == "run_unsandboxed")
        );
        assert!(matches!(
            parse_call(&thread(RunTools::None), "run", command()),
            Err(ParseError::UnknownTool(_))
        ));
        let full = names(&thread(RunTools::Run));
        assert!(full.contains(&"run".to_owned()));
        assert!(!full.contains(&"run_unsandboxed".to_owned()));
        assert!(matches!(
            parse_call(&thread(RunTools::Run), "run", command()),
            Ok(ToolCall::Orchestrator(OrchestratorCall::Run(RunCommand {
                timeout_secs: Some(60),
                ..
            })))
        ));
        assert!(matches!(
            parse_call(&thread(RunTools::Run), "run_unsandboxed", command()),
            Err(ParseError::UnknownTool(_))
        ));
        let sandboxed = names(&thread(RunTools::WithEscalation));
        assert!(sandboxed.contains(&"run".to_owned()));
        assert!(sandboxed.contains(&"run_unsandboxed".to_owned()));
        let mut arguments = command().unwrap();
        arguments.insert(
            "justification".into(),
            Value::String("It needs the network.".into()),
        );
        assert!(matches!(
            parse_call(
                &thread(RunTools::WithEscalation),
                "run_unsandboxed",
                Some(arguments)
            ),
            Ok(ToolCall::Orchestrator(OrchestratorCall::RunUnsandboxed(_)))
        ));
        let hook = Role::OutputHook {
            conversation_id: brigadier_core::model::ConversationId("c1".into()),
        };
        assert!(tools_for(&hook).is_empty());
        assert!(parse_call(&hook, "run", command()).is_err());
    }

    #[test]
    fn every_thread_starts_reads_and_stops_previews() {
        for run in [RunTools::None, RunTools::Run, RunTools::WithEscalation] {
            let thread = Role::Orchestrator {
                conversation_id: brigadier_core::model::ConversationId("c1".into()),
                run,
            };
            let names: Vec<String> = tools_for(&thread)
                .iter()
                .map(|tool| tool.name.to_string())
                .collect();
            for name in ["start_preview", "stop_preview", "preview_log"] {
                assert!(names.contains(&name.to_owned()), "{name} {run:?}");
            }
            let arguments = serde_json::json!({
                "command": "python3 -m http.server 8123",
                "name": "site",
                "env": { "PORT": "8123" },
                "workdir": "web",
            });
            let Ok(ToolCall::Orchestrator(OrchestratorCall::StartPreview(start))) =
                parse_call(&thread, "start_preview", arguments.as_object().cloned())
            else {
                panic!("start_preview parses");
            };
            assert_eq!(start.env.unwrap()["PORT"], "8123");
            assert!(matches!(
                parse_call(&thread, "stop_preview", None),
                Ok(ToolCall::Orchestrator(OrchestratorCall::StopPreview(stop))) if stop.id.is_none()
            ));
            let arguments = serde_json::json!({ "id": "preview-1", "tail_lines": 10 });
            assert!(matches!(
                parse_call(&thread, "preview_log", arguments.as_object().cloned()),
                Ok(ToolCall::Orchestrator(OrchestratorCall::PreviewLog(log)))
                    if log.tail_lines == Some(10)
            ));
        }
        // Workers don't: a preview is the thread's.
        let names: Vec<String> = tools_for(&worker(false))
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert!(!names.iter().any(|name| name.contains("preview")));
    }

    #[test]
    fn every_thread_and_worker_runs_checks() {
        let check = serde_json::json!({
            "command": "cargo test -p core",
            "workdir": "crates/core",
            "timeout": 120,
            "rerun": true,
        });
        for run in [RunTools::None, RunTools::Run, RunTools::WithEscalation] {
            let thread = Role::Orchestrator {
                conversation_id: brigadier_core::model::ConversationId("c1".into()),
                run,
            };
            assert!(
                tools_for(&thread)
                    .iter()
                    .any(|tool| tool.name == "run_check")
            );
            let Ok(ToolCall::Orchestrator(OrchestratorCall::RunCheck(args))) =
                parse_call(&thread, "run_check", check.as_object().cloned())
            else {
                panic!("run_check parses for {run:?}");
            };
            assert_eq!(args.timeout_secs, Some(120));
            assert!(args.rerun);
        }
        for checks in [false, true] {
            assert!(
                tools_for(&worker(checks))
                    .iter()
                    .any(|tool| tool.name == "run_check")
            );
            assert!(matches!(
                parse_call(&worker(checks), "run_check", None),
                Ok(ToolCall::Worker(WorkerCall::RunCheck(RunCheck {
                    command: None,
                    rerun: false,
                    ..
                })))
            ));
        }
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
        assert!(names(&worker(false)).contains(&"review_code".to_owned()));
        let checker = names(&worker(true));
        assert!(!checker.contains(&"ask_orchestrator".to_owned()));
        assert!(!checker.contains(&"review_code".to_owned()));
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

    /// `stop_worker` takes a required one-line reason, which the thread's "Stopped" row shows.
    #[test]
    fn stop_worker_needs_a_reason() {
        let thread = Role::Orchestrator {
            conversation_id: brigadier_core::model::ConversationId("c1".into()),
            run: RunTools::None,
        };
        let tool = tools_for(&thread)
            .iter()
            .find(|tool| tool.name == "stop_worker")
            .unwrap();
        let required = tool.input_schema.get("required").unwrap();
        assert!(
            required
                .as_array()
                .unwrap()
                .contains(&Value::String("reason".into())),
            "{required}"
        );
        let mut arguments = JsonObject::new();
        arguments.insert("task".into(), Value::String("task-2".into()));
        let err = parse_call(&thread, "stop_worker", Some(arguments.clone())).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Invalid arguments for stop_worker: missing field `reason`"
        );
        arguments.insert("reason".into(), Value::String("No longer needed".into()));
        assert!(matches!(
            parse_call(&thread, "stop_worker", Some(arguments)),
            Ok(ToolCall::Orchestrator(OrchestratorCall::StopWorker(args)))
                if args.task == "task-2" && args.reason == "No longer needed"
        ));
    }
}
