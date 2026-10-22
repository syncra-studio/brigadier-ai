//! The tools each role sees, their descriptions (the models' manual), and the mapping from an
//! MCP `tools/call` onto [`ToolCall`].

use std::sync::{Arc, OnceLock};

use brigadier_core::tools::{
    AnswerWorker, ApproveOutline, AskOrchestrator, AttachProof, CHAT_THREAD_TOOLS, ChatCall,
    CiPush, CodeRefs, CodeSearch, DelegateTask, EndRun, FinishSession, JobCall, LandPhase,
    MessageWorker, NoteForUser, NoteProgress, OfferFeedback, OrchestratorCall, PlanPhases,
    ProposeOvernight, ProposePlan, QueryBrain, ReadArtifact, ReadPage, RecordNodes, Remember,
    ReportRef, RequestApproval, RerunTask, Role, RouteFollowUp, SaveMemory, SaveReport,
    SearchTranscript, SetHeadline, SettleStep, StartVerifier, StopWorker, StoreOutput,
    SubmitOutline, SubmitReport, SuggestSetting, THREAD_TOOLS, TabChatCall, TabTools, ToolCall,
    WorkerCall, WritePage, WriteTabFile,
};
use rmcp::model::{JsonObject, MetaObject, Tool};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::schema::{input_schema, no_arguments};

/// `delegate_task`'s description, which carries the rules for briefing and routing workers
/// (BUILD-PLAN C1): the thread does normal work itself, so they are read when it delegates,
/// not on every call. Under [`DESCRIPTION_MAX`] characters, where Claude Code cuts a
/// description it loads up front.
const DELEGATE_TASK: &str = "Start a worker on one task. Returns at once with its id (\"task-3\"); \
its report arrives later as a message. Never wait or poll: end your turn and carry on when the \
report or the user's next message arrives.

When: parts that can run at once, very long jobs, work needing tools you lack (a look at a \
running app), or the user's ask. Independent parts (separate questions, writers on separate \
files) start in one batch, several calls in one message, each one job with its own \"done \
when\"; dependent parts run one after another (plan_phases). A long job across back end and \
UI goes to one worker per layer, split along a data contract (the field names) in both briefs. \
Meanwhile do your own part.

The brief (`spec`) is self-contained: the worker sees nothing of this conversation. Give the \
constraints and settled decisions, what \"done\" means and how to check each part by its own \
checks only (never the full suite or an app build; screenshots only if the user asked), and the \
code pointers you have. With `include_request` Brigadier adds the user's request verbatim, with \
what was added since: don't copy it into the spec. Never tell a worker to write outside its \
worktree and scratch folder, nor name a /tmp path: files for you or the user go in its outputs \
folder.

Kinds: `scout` looks around the repository; `research` checks current docs and the web; \
`implement` changes code in its own worktree, commits its steps and lands only through \
land_phase; `operate` uses apps on this Mac (over about five GUI steps) to reach `end_state` in \
`target`, changing no code, on a fast GUI model; a job that changes code and shows in an app \
stays one implement task. `review`, `merge` and `verify` read and report.

Apps: with computer use on, every worker can open an app on this Mac, look at it and use it; \
you can't. A visible UI change, however small, is looked at live once on the landed work: \
once the full checks pass on it, delegate one task with `live_look`; workers use scripted UI checks, and a \
mockup's final comparison is a live capture. A visible UI change that isn't a small fix: first ask_user \"Want a mockup \
first?\" (Yes / Skip; say if part of it can't be faked). Yes: a task with `mockup`, then ask \
only what its screenshots can't show; the user's marked notes go to that worker (message_worker \
with their attachments). On their yes to the work, land the mockup, then delegate with \
`matches_mockup`; if they drop it, stop the mockup.

`subject`: a review's task; a merge task's conflicting task (none merges the session's base \
into its branch); a fix's task: after a phase's verifier it continues that work and lands it \
with its own.";

const MESSAGE_WORKER: &str = "Send text to a worker: an instruction that steers its current \
work, or sends a reported worker back to work. Answer a worker's question with answer_worker. \
Returns once delivered.";

const ANSWER_WORKER: &str = "Answer the question a worker asked you (it waits for the answer). \
Answer at once and yourself: take the worker's recommendation if it fits, else what the brief, \
the outline, the user's words or the Brain imply; never reopen a decision already settled. Ask \
the user only what truly only they can decide. `why` is a few words the user reads next to the \
answer.";

const ROUTE_FOLLOW_UP: &str = "Sort a [follow-up …] the user sent while you work on their \
request. route \"join\": it belongs to this work; it reaches you at once as the user's message, \
and your final answer covers it too. \"new_session\": separate work, with a different goal that \
leaves the files this request is changing alone (another file, feature or fix), however small; \
it waits offering the user \"Open as a new session\" or \"Run here\" until they pick (you \
never start sessions). \"wait\": a request of its own that has to come after this work (it \
builds on its result or changes the same files); it waits in the user's queue and reaches you \
once this work is done. \"session\" with `session` (another session's id): it belongs to \
that session of this project; it goes there as the user's message and this session shows the \
user a notice with Undo; the result says if that session already merged. Optional: one left unsorted joins this work shortly.";

const STOP_WORKER: &str = "Stop a running worker, e.g. when its task is no longer needed or \
went wrong. Nothing of it lands. `reason` is required: one plain line on why, which the user \
reads on the thread's \"Stopped\" row (e.g. \"No longer needed: the user dropped the export\").";

const RERUN_TASK: &str = "Run a scout or research task again once, one tier up, when you judge \
that its answer lacks support: the same spec on a model a tier above the one that ran it. Its \
report arrives later as a message; use it, ignore it or stop it. Once per task; not for a rerun \
or other kinds of work. A report with a [not met] line reruns by itself. A missing or broken \
citation is no reason: the report marks it [unevidenced] for you to judge. `why` is one line on \
what falls short.";

const READ_REPORT: &str = "Read a task's final report again: summary, changes, decisions, \
verification, open questions and artifact ids. A report from another session of this project \
is read by the task id a Brain answer names.";

const STORE_OUTPUT: &str = "Keep a file you wrote, such as your corrected copy of a worker's \
deliverable, as that task's output, in place of its output with the same file name. Link it as \
its reply says ([name](outputs/name)): it opens and saves from there and from the task card, and \
outlives your workspace and scratch.";

const READ_ARTIFACT: &str = "Read part of an artifact named in a report or a Brain answer (full \
transcript, diff, command output, note), from any session of this project, by its id. At most 16000 bytes per call, starting at `offset`; the \
reply gives the total size so you can page. Read only what you need: everything you read enters \
your context.";

const QUERY_BRAIN: &str = "Ask the Project Brain what Brigadier knows about this project (modules, \
earlier findings, decisions, conventions, contracts) and the user's preferences, each with \
where it came from. Fast and cheap: ask it first; when it has nothing or marks a line stale, \
read the code yourself. It answers with what holds now (`history` adds earlier versions; `page` \
the next results).";

const REMEMBER: &str = "Keep what later work must respect: a decision the user \
settled or you made, a convention of this project, a contract between parts, a standing rule of \
the user's (kind constraint, in their exact words with its `scope`), or (personal) a stable fact \
about them across projects (where they live, their trade, their tools, a preference; never \
credentials, health or money details). What the user settled cites their message (`user_message`, with their \
words in `quote`): Brigadier checks both. Anything else is kept as your claim, unverified, with \
its evidence (`files` or `check`, and the words it shows in `quote`), re-read whenever \
the claim is used. Do it silently. Returns the node id; pass it as `replaces` to correct it, or a \
claim you find wrong.";

const SEARCH_TRANSCRIPT: &str = "Search this conversation's full transcript (the user's \
messages, your answers, reports and decisions, and the commands you ran, files you read and \
tools you called), including what is no longer in your context. Returns the best-matching \
passages with their dates: a message shows its number (#N), a step of your own work shows as \
a thread step, never a message number, with where its whole output is kept.";

const PLAN_PHASES: &str = "Split a big request into phases that must run one after another \
(each builds on the one before). Most requests are one phase: then skip this and delegate the \
lead. Records the phases for the user's progress pill; nothing is reviewed or approved here. \
Then delegate phase 1's lead (delegate_task, kind implement, phase 1), and each next phase once \
the one before it has landed.";

const PROPOSE_PLAN: &str = "Put your plan before the user, as a document on a card they read and \
then accept (\"Yes, implement this plan\") or send back with the changes they want. Use it when \
plan mode is on, or when the user asks for a plan. Look first (read the code yourself, or a scout \
for more), so the plan names the real files and symbols; ask_user before it only what only the user \
can decide. Call it once: the card is your answer. A yes comes back as a \
message: build it then. Changes come back too: propose the revised plan again.

Write the body in markdown, in this shape:
# A short title: what gets built, in a few words
One sentence: what changes, in the user's terms.
## Changes
- One bullet per change: the file and symbol in `code` and what changes there, in one line.
## Checks
- How the work is verified: the tests added or run, the commands, what to look at in the app.
## Assumptions
- Each choice you made that the user might make otherwise: defaults, edge cases, what is left out.

Each bullet is one short line: the reader scans it. Detail the code will show anyway (error \
messages, exit codes, every case a test covers) stays out unless the user must decide it. Plain, \
specific words; no preamble, no restating the request, no filler (\"This plan will…\", \
\"robust\", \"seamless\"). Most plans fit in 8 to 15 lines; bigger work may need more, but never pad.";

const APPROVE_OUTLINE: &str = "Give a lead the go-ahead on its outline as soon as you have \
judged it. Put what the outline gets wrong, and anything the brief implies, in `corrections`; the brief wins any \
conflict. Under \"Ask\" this shows the user an \"Implement this plan?\" card and the \
go-ahead goes once they start it. Returns at once.";

const START_VERIFIER: &str = "Your call, for big or risky work only: start a fresh verifier \
on top of a reported implement task's commits. It checks every \"done when\" for real, fixes \
and commits what fails, and reports. Land the verifier's task then (its commits hold the lead's), not \
the lead's. Small work needs none: land the lead. With fresh: true it gets only the original \
request, the user's constraints, done_when and the candidate commit (a task's, or commit for your \
own work) and tries to break it. Returns at once.";

const ATTACH_PROOF: &str = "Attach a screenshot or short recording you made of the change \
(PNG, JPEG, GIF, WebP, MP4, MOV or WebM, at most 11 MB), or a log or test run's output (.log or \
.txt, at most 1 MB, with `kind`), to your reply: the user sees it under the reply, the merge \
message's proof. Always give a short `caption` of what it shows (\"Added a todo with a due \
date\"), and `preview` when it was taken of one of your previews; a browser_screenshot \
knows its page. Brigadier keeps a copy, so a temporary file is fine.";

const SETTLE_STEP: &str = "Only during an overnight run: settle a step of the run's plan, by \
its number, once you have judged its whole scope and every \"done when\" (your own edits \
too) and nothing of it still runs or waits to land. Done: all of it is met and landed; partial: \
some is left for a later run; blocked: what is left needs the user. Done should rest on \
evidence for every criterion at the tip (its report's, or yours in `done_when`) and the full \
checks passing there; what has none is said in the result and marked in the morning report, \
not refused. A step settled done moves the tip the user's Merge takes, once every \
step before it is done too.";

const END_RUN: &str = "Only during an overnight run: end it now, because every selected step \
is settled (done) or all that is left needs the user (needsUser). Brigadier then ends the run \
cleanly and writes its report; you write the user's morning answer.";

const REQUEST_APPROVAL: &str = "Ask the user to approve what only they may decide: spending \
money, handling credentials yourself (a token, a password, the keychain directly), or destroying \
something outside this session's own work. Not for pushes, pull requests or deploys (just do \
those, on the git and gh sign-in the user set up) nor for work inside the session. Returns at once; the decision arrives later as a message.";

const READ_TAB_PAGE: &str = "Read the page beside this chat: its title, its text in Markdown \
and its revision. Only that page.";

const WRITE_TAB_PAGE: &str = "Change the page beside this chat: its whole new text in Markdown, \
and a new title only if asked. Pass the revision read_page returned: if the page changed since, \
nothing is written and you read it again. The user sees the change beside the chat.";

const READ_TAB_FILE: &str = "Read the file beside this chat: its path, its text and its \
revision.";

const WRITE_TAB_FILE: &str = "Change the file beside this chat: its whole new text. Pass the \
revision read_tab_file returned: if the file changed since, nothing is written and you read it \
again. This is the only file you may change.";

const LAND_PHASE: &str = "Land a finished `implement` or `merge` task's commits on the \
session's branch: the lead's task once its report is in, or a verifier you started for big or \
risky work (its commits hold the lead's). Call it after reading the report. Brigadier commits \
what was left uncommitted, leaves litter out, and fast-forwards the branch; no card, no review \
and no checks start (your checks and reviews are yours to run). If the branch moved \
meanwhile, the commits are rebased onto it and land at once. Conflicts come back to you: \
delegate a merge task.";

const FINISH_SESSION: &str = "New-worktree sessions only: merge the session branch into its \
base branch now, when the user asked for the merge (their words, a card answer, or [merge]: \
their click). It merges what has landed and names what still runs; the session keeps its \
worktree and branch after the merge. You may instead merge with git yourself.";

const NOTE_FOR_USER: &str = "Keep the user's session summary current: kind \"decided\" for a \
judgement call you made on their behalf that they'd want to know, with why (\"Decided for \
you\"); kind \"waiting\", during an overnight run only, for what only the user can do (a key, \
a sign-in), in one line. Brigadier lists its own decisions and the workers' needs itself: don't \
repeat them. Returns at once.";

const CI_STATUS: &str = "After a push of yours (`git push`), say which commit and branch went \
to which remote: it answers at once whether this repository's CI can be watched from here \
(GitHub Actions, gh signed in), can't be (another CI, or gh missing or signed out; with a link \
when there is one), or has no CI. On [ci … failed], fix it on that branch (`main` only \
through the usual merge into it here), push it and call watch_ci again without asking.";

const WATCH_CI: &str = "Watch the GitHub Actions runs of a push: it returns at once, the user sees the runs live, and the result comes later as a \
[ci <sha> · passed|failed|no runs …] message with each failing job and its log lines.";

const CLEAN_UP: &str = "Only when the user asks to clean up: removes what this session's \
finished work left that nothing uses any more (finished tasks' worktrees, folders and merged \
branches), by Brigadier's own records, and says what went and what \
stayed and why. Brigadier already cleans up after you; never delete branches, worktrees or temp \
folders by hand.";

const SET_HEADLINE: &str = "Say what the session is doing now, for the user's Job card: \
`headline` in a few plain words (\"Starting the dev server\", \"The build failed\", \"It's up \
again\"), `detail`, one short line under it, usually what comes next (\"Next: open the \
window\"), and `failed` true when the words tell of a failure that still stands. Call it when \
what you are doing changes, not for every step; it replaces the last one and stays until the \
user's next message. Returns at once.";

const SUGGEST_SETTING: &str = "Suggest changing one setting on a card whose one button makes \
the change, when what the user asked for needs it: this session's permission level \
(`permission`: ask, accept_edits or full_access) or the Browser setting \
(`browser`: on or off). reason: one line on why it helps with what they asked. Nothing changes until the user presses \
the button; every other setting is theirs alone. Refused when the setting is already so, during \
an overnight run, and a second time in one request.";

const OFFER_FEEDBACK: &str = "When the user seems annoyed with Brigadier itself (a bug in \
the app or in how you or its workers behave; never their own project's code), or asks you to \
report something to Brigadier's developers (\"report this\", \"file that as a bug\"), offer \
them a report drafted from this conversation: a card under your answer. Its Send files it on \
GitHub as the user, one issue per bug or idea, a repeat added to the issue it repeats; they may \
edit it first or dismiss it. Nothing is sent without their click, and the card, not your \
answer, says whether it was. title: one line naming the problem. body: what happened and what \
they expected, steps if known, a few plain lines in their words; never invent steps; no secrets, paths or project code. asked: true when the user asked \
for it. One offer per incident: unless asked, refused while an earlier one is unanswered or \
after they dismissed one. Asked to report several separate things, offer one card each, up to 10 \
in a request.";

const LIST_TASKS: &str = "List this session's tasks (id, title, kind, status and model), then \
where the session's full checks stand (passed, running and for how long, failed, due) and which \
of your commands run in the background: call it before you say whether the work is ready.";

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

const SAVE_REPORT: &str = "Keep a research answer's full report as a page of the user's: \
everything the short answer leaves out, in Markdown, with its sources. The reply gives the link \
to put in your answer ([Full report](brigadier-page:<id>)); one click opens it. Never paste the \
report into the chat.";

const SAVE_MEMORY: &str = "Save a stable fact about the user that will help in later \
conversations (where they live, their trade, the tools or devices they use, a preference), as one \
short sentence. Only when they state it or clearly imply it holds beyond this chat; never \
passwords, keys, card or account numbers (refused), health or money details, or passing details. \
The user sees, edits and deletes it in Settings.";

const ASK_THREAD: &str = "Ask the thread (which gave you this task) a question you \
cannot settle yourself, such as an unclear requirement or a choice outside your task. One \
question per call, with the options you see and the one you recommend. The call blocks until \
the answer comes back. Ask only when you cannot sensibly go on without the answer; look things \
up with query_brain and the code tools first. If none comes within the hour, an operational \
question goes on with its `safe_default`; any other keeps waiting.";

const NOTE_PROGRESS: &str = "Say where you are in your plan, in a few words on one line \
(\"Step 3 of 5: wiring the exporter\", \"Scene 4 of 12: pricing section\"): the user sees it under \
your name while you work, until your next note. On work of several steps or over a few minutes, \
post one as you start each step. It returns at once and tells no one else.";

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
your last action, when the task is done or cannot be done. The thread reads only this \
report, never your messages: the summary must hold your findings (or name the artifact that \
does), never point to text below or in a message. It is final, and the thread reads \
about 800 tokens of it inline (a longer one reaches it shortened): keep every field short, and \
put long material (full findings, logs, command output, notes) in \
files in your outputs folder, listed under `artifacts`. List every file you changed, created or \
deleted under `changes`: new files that are not listed are not kept.";

/// The tools `role` may call, in a stable order. The gate role gets none, and a worker that
/// checks a change or plan no `ask_thread`.
pub fn tools_for(role: &Role) -> &'static [Tool] {
    static ORCHESTRATOR: OnceLock<Vec<Tool>> = OnceLock::new();
    static WORKER: OnceLock<Vec<Tool>> = OnceLock::new();
    static CHECKER: OnceLock<Vec<Tool>> = OnceLock::new();
    static JOB: OnceLock<Vec<Tool>> = OnceLock::new();
    static CHAT: OnceLock<Vec<Tool>> = OnceLock::new();
    static TAB_PAGE: OnceLock<Vec<Tool>> = OnceLock::new();
    static TAB_FILE: OnceLock<Vec<Tool>> = OnceLock::new();
    match role {
        // A note fork lists the thread's tools, whose calls the host refuses.
        Role::Orchestrator { .. } | Role::NoteFork { .. } => ORCHESTRATOR.get_or_init(|| {
            loading(
                orchestrator_tools()
                    .into_iter()
                    .filter(|tool| THREAD_TOOLS.contains(&tool.name.as_ref()))
                    .collect(),
            )
        }),
        Role::Worker { checks: false, .. } => WORKER.get_or_init(worker_tools),
        Role::Worker { checks: true, .. } => CHECKER.get_or_init(|| {
            worker_tools()
                .into_iter()
                .filter(|tool| !matches!(tool.name.as_ref(), "ask_thread" | "submit_outline"))
                .collect()
        }),
        Role::BrainJob { .. } => JOB.get_or_init(job_tools),
        Role::Chat { .. } => CHAT.get_or_init(chat_tools),
        Role::TabChat {
            tools: TabTools::Page,
            ..
        } => TAB_PAGE.get_or_init(|| {
            vec![
                tool("read_page", READ_TAB_PAGE, input_schema::<ReadPage>()),
                tool("write_page", WRITE_TAB_PAGE, input_schema::<WritePage>()),
            ]
        }),
        Role::TabChat {
            tools: TabTools::File,
            ..
        } => TAB_FILE.get_or_init(|| {
            vec![
                tool("read_tab_file", READ_TAB_FILE, no_arguments()),
                tool(
                    "write_tab_file",
                    WRITE_TAB_FILE,
                    input_schema::<WriteTabFile>(),
                ),
            ]
        }),
        Role::OutputHook { .. } | Role::StopHook { .. } => &[],
    }
}

fn orchestrator_tools() -> Vec<Tool> {
    vec![
        tool("delegate_task", DELEGATE_TASK, delegate_task_schema()),
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
        tool("rerun_task", RERUN_TASK, input_schema::<RerunTask>()),
        tool("read_report", READ_REPORT, input_schema::<ReportRef>()),
        tool(
            "read_artifact",
            READ_ARTIFACT,
            input_schema::<ReadArtifact>(),
        ),
        tool("store_output", STORE_OUTPUT, input_schema::<StoreOutput>()),
        tool("query_brain", QUERY_BRAIN, input_schema::<QueryBrain>()),
        tool("remember", REMEMBER, input_schema::<Remember>()),
        tool(
            "search_transcript",
            SEARCH_TRANSCRIPT,
            input_schema::<SearchTranscript>(),
        ),
        tool("plan_phases", PLAN_PHASES, input_schema::<PlanPhases>()),
        tool("propose_plan", PROPOSE_PLAN, input_schema::<ProposePlan>()),
        tool(
            "approve_outline",
            APPROVE_OUTLINE,
            input_schema::<ApproveOutline>(),
        ),
        tool(
            "start_verifier",
            START_VERIFIER,
            input_schema::<StartVerifier>(),
        ),
        tool("attach_proof", ATTACH_PROOF, input_schema::<AttachProof>()),
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
        tool("ci_status", CI_STATUS, input_schema::<CiPush>()),
        tool("watch_ci", WATCH_CI, input_schema::<CiPush>()),
        tool(
            "offer_feedback",
            OFFER_FEEDBACK,
            input_schema::<OfferFeedback>(),
        ),
        tool("set_headline", SET_HEADLINE, input_schema::<SetHeadline>()),
        tool("list_tasks", LIST_TASKS, no_arguments()),
        tool("settle_step", SETTLE_STEP, input_schema::<SettleStep>()),
        tool("end_run", END_RUN, input_schema::<EndRun>()),
        tool("code_search", CODE_SEARCH, input_schema::<CodeSearch>()),
        tool("code_refs", CODE_REFS, input_schema::<CodeRefs>()),
        tool("project_map", PROJECT_MAP, no_arguments()),
        tool("clean_up", CLEAN_UP, no_arguments()),
        tool("save_report", SAVE_REPORT, input_schema::<SaveReport>()),
        tool(
            "suggest_setting",
            SUGGEST_SETTING,
            input_schema::<SuggestSetting>(),
        ),
    ]
}

fn worker_tools() -> Vec<Tool> {
    vec![
        tool("ask_thread", ASK_THREAD, input_schema::<AskOrchestrator>()),
        tool(
            "submit_outline",
            SUBMIT_OUTLINE,
            input_schema::<SubmitOutline>(),
        ),
        tool(
            "note_progress",
            NOTE_PROGRESS,
            input_schema::<NoteProgress>(),
        ),
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

/// A Chat's: the thread's tools for workers and cards ([`CHAT_THREAD_TOOLS`]), its memories and
/// its reports.
fn chat_tools() -> Vec<Tool> {
    let mut tools: Vec<Tool> = orchestrator_tools()
        .into_iter()
        .filter(|tool| CHAT_THREAD_TOOLS.contains(&tool.name.as_ref()))
        .collect();
    tools.push(tool(
        "save_memory",
        SAVE_MEMORY,
        input_schema::<SaveMemory>(),
    ));
    tools.push(tool(
        "save_report",
        SAVE_REPORT,
        input_schema::<SaveReport>(),
    ));
    tools
}

/// The thread's everyday tools, in the model's list from the start (Q40, BUILD-PLAN C2): a
/// Claude thread finds the rest with its tool search when it needs one, which keeps every
/// call's fixed start near plain Claude Code's. Codex reads no such mark and loads them all.
pub const THREAD_ALWAYS_LOADED: &[&str] = &[
    "note_for_user",
    "set_headline",
    "query_brain",
    "remember",
    "code_search",
    "code_refs",
];

/// What the tool search matches a deferred tool on besides its name and description.
const SEARCH_HINTS: &[(&str, &str)] = &[
    ("delegate_task", "start a worker, delegate, parallel jobs"),
    ("message_worker", "worker"),
    ("answer_worker", "worker question"),
    ("read_report", "worker report"),
    (
        "store_output",
        "corrected file, fixed deliverable, worker output, Save",
    ),
    ("save_report", "full report, research, long findings, page"),
    (
        "suggest_setting",
        "permission level, browser on or off, settings",
    ),
    (
        "offer_feedback",
        "user annoyed, frustrated, complaint, bug report, feedback, report this, file an issue",
    ),
];

/// The thread's tools marked for Claude Code's tool search: [`THREAD_ALWAYS_LOADED`] load up
/// front, every other one waits behind the search. The marks are the same on every call, so
/// the cached prefix stays the same; a tool found later goes into the conversation, not the
/// prefix.
fn loading(tools: Vec<Tool>) -> Vec<Tool> {
    tools
        .into_iter()
        .map(|mut tool| {
            let mut meta = JsonObject::new();
            if THREAD_ALWAYS_LOADED.contains(&tool.name.as_ref()) {
                meta.insert("anthropic/alwaysLoad".into(), true.into());
            } else if let Some((_, hint)) = SEARCH_HINTS
                .iter()
                .find(|(name, _)| *name == tool.name.as_ref())
            {
                meta.insert("anthropic/searchHint".into(), (*hint).into());
            }
            if !meta.is_empty() {
                tool.meta = Some(MetaObject(meta));
            }
            tool
        })
        .collect()
}

/// The most characters of a tool's description Claude Code keeps for a tool loaded up front.
#[cfg(test)]
const DESCRIPTION_MAX: usize = 4096;

/// `delegate_task`'s fields in one line each (BUILD-PLAN C1): the rules behind them are in its
/// description, and the shared types' own docs stay as they are.
const DELEGATE_TASK_FIELDS: &[(&str, &str)] = &[
    (
        "title",
        "A plain 2–4 word job name, unique in this chat (\"Add JSON output\"); no roles, phases or ids.",
    ),
    (
        "kind",
        "What it does: scout, research, implement, review, merge, verify or operate.",
    ),
    (
        "spec",
        "The self-contained brief: goal, context, constraints, \"done when\" and how to check it.",
    ),
    (
        "provider",
        "Optional \"claude\" or \"codex\"; leave it out to let Brigadier route.",
    ),
    ("model", "Optional model id, from that provider's list."),
    (
        "effort",
        "\"low\" for a mechanical edit, \"medium\" for small bounded work, \"high\" for cross-area, risky or unclear work.",
    ),
    (
        "subject",
        "The task it acts on (\"task-2\"): a review's, a merge's or a fix's.",
    ),
    (
        "target",
        "operate: the app, dev build path or URL, and the window.",
    ),
    (
        "end_state",
        "operate: what must be true on screen at the end.",
    ),
    (
        "attachments",
        "Ids of the user's attachments it gets as files.",
    ),
    (
        "include_request",
        "True when it takes the whole request: Brigadier adds the user's message verbatim.",
    ),
    (
        "areas",
        "Optional parts it touches (\"frontend\", \"backend\", \"infra\", \"docs\", \"tests\") when the spec's paths don't say; routing goes by them.",
    ),
    (
        "quality",
        "Optional \"high\" when it needs the best models (hard or risky work).",
    ),
    ("image_generation", "True when it must generate images."),
    (
        "design",
        "True when it makes designs or images: design files (Pencil's .pen), mockups, generated images.",
    ),
    ("phase", "The plan phase it works on (plan_phases), from 1."),
    (
        "role",
        "lead (an implement task's default), parallel (its own files next to the lead), verifier, fix, merge or reviewer.",
    ),
    (
        "mockup",
        "True for a mockup the user asked for: screenshots from the real components with fake data; it lands only on their yes.",
    ),
    (
        "matches_mockup",
        "The mockup's task, for the real change built after it.",
    ),
    (
        "live_look",
        "True for the one task that looks at the landed visible changes live in the session tip's dev app; it changes nothing.",
    ),
    (
        "integrations",
        "Extra MCP servers it needs, by name from your list.",
    ),
];

/// [`DelegateTask`]'s schema with [`DELEGATE_TASK_FIELDS`]' descriptions.
fn delegate_task_schema() -> JsonObject {
    let mut schema = input_schema::<DelegateTask>();
    if let Some(Value::Object(properties)) = schema.get_mut("properties") {
        for (field, description) in DELEGATE_TASK_FIELDS {
            if let Some(Value::Object(property)) = properties.get_mut(*field) {
                property.insert("description".into(), (*description).into());
            }
        }
    }
    schema
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
        // Workers and merges are `brigadier` and git's now (S15): the thread calls only its
        // listed tools.
        Role::Orchestrator { .. } | Role::NoteFork { .. } if THREAD_TOOLS.contains(&name) => {
            thread_call(name, arguments)
        }
        Role::Orchestrator { .. } | Role::NoteFork { .. } => Err(unknown()),
        Role::Worker { checks, .. } => {
            let call = match name {
                // Its name before #66, which a worker that started on it may still call.
                "ask_thread" | "ask_orchestrator" if !checks => {
                    WorkerCall::AskOrchestrator(args::<AskOrchestrator>(name, arguments)?)
                }
                "submit_outline" if !checks => {
                    WorkerCall::SubmitOutline(args::<SubmitOutline>(name, arguments)?)
                }
                "submit_report" => WorkerCall::SubmitReport(args::<SubmitReport>(name, arguments)?),
                "query_brain" => WorkerCall::QueryBrain(args::<QueryBrain>(name, arguments)?),
                "code_search" => WorkerCall::CodeSearch(args::<CodeSearch>(name, arguments)?),
                "code_refs" => WorkerCall::CodeRefs(args::<CodeRefs>(name, arguments)?),
                "project_map" => WorkerCall::ProjectMap,
                "note_progress" => WorkerCall::NoteProgress(args::<NoteProgress>(name, arguments)?),
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
            "save_report" => Ok(ToolCall::Chat(ChatCall::SaveReport(args::<SaveReport>(
                name, arguments,
            )?))),
            // The thread's worker and card tools, read as the thread's were.
            _ if CHAT_THREAD_TOOLS.contains(&name) => thread_call(name, arguments),
            _ => Err(unknown()),
        },
        Role::TabChat { tools, .. } => {
            let call = match (tools, name) {
                (TabTools::Page, "read_page") => TabChatCall::ReadPage(args(name, arguments)?),
                (TabTools::Page, "write_page") => TabChatCall::WritePage(args(name, arguments)?),
                (TabTools::File, "read_tab_file") => TabChatCall::ReadFile,
                (TabTools::File, "write_tab_file") => {
                    TabChatCall::WriteFile(args(name, arguments)?)
                }
                _ => return Err(unknown()),
            };
            Ok(ToolCall::TabChat(call))
        }
        Role::OutputHook { .. } | Role::StopHook { .. } => Err(unknown()),
    }
}

/// A call of the thread's tool `name`, a Chat's worker and card tools included.
fn thread_call(name: &str, arguments: Value) -> Result<ToolCall, ParseError> {
    let unknown = || ParseError::UnknownTool(name.to_owned());
    let call = match name {
        "delegate_task" => OrchestratorCall::DelegateTask(args(name, arguments)?),
        "message_worker" => OrchestratorCall::MessageWorker(args(name, arguments)?),
        "answer_worker" => OrchestratorCall::AnswerWorker(args(name, arguments)?),
        "route_follow_up" => OrchestratorCall::RouteFollowUp(args(name, arguments)?),
        "stop_worker" => OrchestratorCall::StopWorker(args(name, arguments)?),
        "rerun_task" => OrchestratorCall::RerunTask(args(name, arguments)?),
        "read_report" => OrchestratorCall::ReadReport(args(name, arguments)?),
        "read_artifact" => OrchestratorCall::ReadArtifact(args(name, arguments)?),
        "store_output" => OrchestratorCall::StoreOutput(args(name, arguments)?),
        "query_brain" => OrchestratorCall::QueryBrain(args(name, arguments)?),
        "remember" => OrchestratorCall::Remember(args(name, arguments)?),
        "search_transcript" => {
            OrchestratorCall::SearchTranscript(args::<SearchTranscript>(name, arguments)?)
        }
        "plan_phases" => OrchestratorCall::PlanPhases(args(name, arguments)?),
        "propose_plan" => OrchestratorCall::ProposePlan(args(name, arguments)?),
        "approve_outline" => OrchestratorCall::ApproveOutline(args(name, arguments)?),
        "start_verifier" => OrchestratorCall::StartVerifier(args(name, arguments)?),
        "attach_proof" => OrchestratorCall::AttachProof(args(name, arguments)?),
        "request_approval" => OrchestratorCall::RequestApproval(args(name, arguments)?),
        "land_phase" => OrchestratorCall::LandPhase(args(name, arguments)?),
        "finish_session" => OrchestratorCall::FinishSession(args(name, arguments)?),
        "note_for_user" => OrchestratorCall::NoteForUser(args(name, arguments)?),
        "ci_status" => OrchestratorCall::CiStatus(args(name, arguments)?),
        "watch_ci" => OrchestratorCall::WatchCi(args(name, arguments)?),
        "suggest_setting" => OrchestratorCall::SuggestSetting(args(name, arguments)?),
        "offer_feedback" => OrchestratorCall::OfferFeedback(args(name, arguments)?),
        "set_headline" => OrchestratorCall::SetHeadline(args(name, arguments)?),
        "list_tasks" => OrchestratorCall::ListTasks,
        "settle_step" => OrchestratorCall::SettleStep(args(name, arguments)?),
        "end_run" => OrchestratorCall::EndRun(args(name, arguments)?),
        "propose_overnight" => OrchestratorCall::ProposeOvernight(args(name, arguments)?),
        "code_search" => OrchestratorCall::CodeSearch(args(name, arguments)?),
        "code_refs" => OrchestratorCall::CodeRefs(args(name, arguments)?),
        "project_map" => OrchestratorCall::ProjectMap,
        "clean_up" => OrchestratorCall::CleanUp,
        "save_report" => OrchestratorCall::SaveReport(args(name, arguments)?),
        _ => return Err(unknown()),
    };
    Ok(ToolCall::Orchestrator(call))
}

fn args<T: DeserializeOwned>(tool: &str, arguments: Value) -> Result<T, ParseError> {
    serde_json::from_value(arguments).map_err(|err| ParseError::BadArguments {
        tool: tool.to_owned(),
        reason: match example(tool) {
            Some(call) => format!("{err}. A well-formed call: {call}"),
            None => err.to_string(),
        },
    })
}

/// A well-formed call, added to an argument error where models guessed the shape: a Codex
/// worker calling tools from code sees no schema, and spent 2–3 rejected calls a report.
fn example(tool: &str) -> Option<&'static str> {
    match tool {
        "submit_report" => Some(SUBMIT_REPORT_EXAMPLE),
        _ => None,
    }
}

/// A complete `submit_report` call: the list fields are text, one item per line.
pub const SUBMIT_REPORT_EXAMPLE: &str = r#"{"summary": "Set Level to 37 through the slider.", "verification": "act's expect value_equals 37 held", "done_when": "[met] Level reads 37: the expect held", "risks": ""}"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tool fits where Claude Code cuts a description loaded up front, and
    /// `delegate_task` keeps one short line per field, each for a field it has (BUILD-PLAN C1).
    #[test]
    fn descriptions_fit_and_delegate_tasks_fields_are_one_line_each() {
        let thread = Role::Orchestrator {
            conversation_id: brigadier_core::model::ConversationId("c1".into()),
        };
        for role in [thread, worker(false), worker(true)] {
            for tool in tools_for(&role) {
                let description = tool.description.as_deref().unwrap_or_default();
                assert!(
                    description.chars().count() <= DESCRIPTION_MAX,
                    "{}: {} characters",
                    tool.name,
                    description.chars().count()
                );
            }
        }
        let schema = delegate_task_schema();
        let properties = schema["properties"].as_object().unwrap();
        assert_eq!(properties.len(), DELEGATE_TASK_FIELDS.len());
        for (field, line) in DELEGATE_TASK_FIELDS {
            assert_eq!(properties[*field]["description"], *line, "{field}");
            assert!(line.len() <= 140, "{field}: {}", line.len());
        }
        assert_eq!(
            properties["kind"]["enum"],
            serde_json::json!([
                "scout",
                "research",
                "implement",
                "review",
                "merge",
                "verify",
                "operate"
            ])
        );
        // #229: a job across back end and UI splits along a written data contract.
        assert!(DELEGATE_TASK.contains("A long job across back end and UI goes to one worker per layer, split along a data contract (the field names) in both briefs."));
        let bytes =
            serde_json::to_string(&Value::Object(schema)).unwrap().len() + DELEGATE_TASK.len();
        assert!(bytes < 6_100, "delegate_task is {bytes} bytes");
    }

    /// A thread's everyday tools load up front and the rest wait behind Claude Code's tool
    /// search, the same marks on every listing (the cached prefix stays the same); a worker's
    /// tools carry no mark (BUILD-PLAN C2).
    #[test]
    fn a_threads_everyday_tools_load_up_front_and_the_rest_wait_for_the_search() {
        {
            let thread = Role::Orchestrator {
                conversation_id: brigadier_core::model::ConversationId("c1".into()),
            };
            let tools = tools_for(&thread);
            let loaded: Vec<&str> = tools
                .iter()
                .filter(|tool| {
                    tool.meta
                        .as_ref()
                        .and_then(|meta| meta.0.get("anthropic/alwaysLoad"))
                        == Some(&Value::Bool(true))
                })
                .map(|tool| tool.name.as_ref())
                .collect();
            let expected: Vec<&str> = THREAD_ALWAYS_LOADED.to_vec();
            assert!(!expected.contains(&"run"));
            let mut sorted = loaded.clone();
            sorted.sort_unstable();
            let mut want = expected.clone();
            want.sort_unstable();
            assert_eq!(sorted, want);
            assert!(loaded.len() * 3 < tools.len(), "most wait: {}", tools.len());
            let report = tools
                .iter()
                .find(|tool| tool.name == "save_report")
                .unwrap();
            assert_eq!(
                report.meta.as_ref().unwrap().0["anthropic/searchHint"],
                "full report, research, long findings, page"
            );
            let listed = serde_json::to_value(report).unwrap();
            assert!(
                listed["_meta"]["anthropic/searchHint"].is_string(),
                "{listed}"
            );
        }
        for tool in tools_for(&worker(false)) {
            assert!(tool.meta.is_none(), "{}", tool.name);
        }
    }

    /// What the thread's instructions no longer say is in the tool it applies to, read when
    /// the thread uses it (BUILD-PLAN C1).
    #[test]
    fn the_rules_left_out_of_the_threads_instructions_are_in_their_tools() {
        for said in [
            "start in one batch, several calls in one message, each one job with its own",
            "dependent parts run one after another (plan_phases)",
            "how to check each part by its own checks only (never the full suite or an app build; screenshots only if the user asked)",
            "With `include_request` Brigadier adds the user's request verbatim",
            "nor name a /tmp path",
            "`operate` uses apps on this Mac (over about five GUI steps)",
            "a job that changes code and shows in an app stays one implement task",
            "once the full checks pass on it, delegate one task with `live_look`",
            "first ask_user \"Want a mockup first?\" (Yes / Skip",
            "message_worker with their attachments",
            "land the mockup, then delegate with `matches_mockup`",
        ] {
            assert!(DELEGATE_TASK.contains(said), "{said}");
        }
        let effort = &delegate_task_schema()["properties"]["effort"]["description"];
        assert!(
            effort
                .as_str()
                .unwrap()
                .starts_with("\"low\" for a mechanical edit")
        );
        assert!(CI_STATUS.contains("(`main` only through the usual merge into it here), push it and call watch_ci again without asking"));
    }

    fn worker(checks: bool) -> Role {
        Role::Worker {
            conversation_id: brigadier_core::model::ConversationId("c1".into()),
            task_id: brigadier_core::model::TaskId("t1".into()),
            checks,
        }
    }

    #[test]
    fn a_report_argument_error_shows_a_call_that_parses() {
        let bad = serde_json::json!({"done_when": "[met] it works"});
        let Err(err) = parse_call(&worker(false), "submit_report", bad.as_object().cloned()) else {
            panic!("a report without a summary was taken");
        };
        let text = err.to_string();
        let shown = text.split("A well-formed call: ").nth(1).expect(&text);
        let example: Value = serde_json::from_str(shown).unwrap();
        assert!(
            parse_call(
                &worker(false),
                "submit_report",
                example.as_object().cloned()
            )
            .is_ok(),
            "{shown}"
        );
    }

    #[test]
    fn phases_are_planned_and_outlines_approved_without_review_rounds() {
        let tools = orchestrator_tools();
        let names: Vec<_> = tools.iter().map(|tool| tool.name.to_string()).collect();
        assert!(names.contains(&"plan_phases".to_owned()));
        assert!(names.contains(&"approve_outline".to_owned()));
        // The plan the user decides is a document in a fixed shape, short lines first.
        let propose = tools
            .iter()
            .find(|tool| tool.name == "propose_plan")
            .unwrap();
        let shape = propose.description.as_deref().unwrap();
        for part in [
            "## Changes",
            "## Checks",
            "## Assumptions",
            "one short line",
        ] {
            assert!(shape.contains(part), "{part}");
        }
        let approve = tools
            .iter()
            .find(|tool| tool.name == "approve_outline")
            .unwrap();
        let description = approve.description.as_deref().unwrap();
        assert!(!description.contains("review"), "{description}");
        assert!(description.contains("Implement this plan?"));
        assert!(names.contains(&"start_verifier".to_owned()));
    }

    #[test]
    fn the_thread_searches_the_code_index() {
        let thread = Role::Orchestrator {
            conversation_id: brigadier_core::model::ConversationId("c1".into()),
        };
        let names: Vec<String> = tools_for(&thread)
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        for name in ["code_search", "code_refs", "project_map"] {
            assert!(names.contains(&name.to_owned()), "{name}");
        }
        // Workers and merges are `brigadier` and git's (S15): the engine's tools for them are
        // gone from the thread.
        for name in [
            "delegate_task",
            "land_phase",
            "finish_session",
            "approve_outline",
        ] {
            assert!(!names.contains(&name.to_owned()), "{name}");
            assert!(parse_call(&thread, name, None).is_err(), "{name}");
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
    }

    /// The lead names a worker's effort for every task (THREAD-UX-PLAN.md §4.1 b): the schema
    /// requires it, and a call without it is refused.
    #[test]
    fn delegate_task_needs_an_effort() {
        // A Chat's, until S16b: the thread's went in S15.
        let thread = Role::Chat {
            conversation_id: brigadier_core::model::ConversationId("c1".into()),
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

    /// The thread and its workers run commands with their CLI's own shell: no thread has
    /// `run`, `run_unsandboxed` or `suggest_full_access`, and a call of one is an unknown tool
    /// (G5).
    #[test]
    fn no_thread_has_brigadiers_command_or_full_access_tools() {
        let thread = Role::Orchestrator {
            conversation_id: brigadier_core::model::ConversationId("c1".into()),
        };
        let mut arguments = JsonObject::new();
        arguments.insert("command".into(), Value::String("cargo test".into()));
        arguments.insert("reason".into(), Value::String("r".into()));
        for name in ["run", "run_unsandboxed", "suggest_full_access"] {
            assert!(
                !tools_for(&thread).iter().any(|tool| tool.name == name),
                "{name}"
            );
            assert!(
                matches!(
                    parse_call(&thread, name, Some(arguments.clone())),
                    Err(ParseError::UnknownTool(_))
                ),
                "{name}"
            );
        }
        let hook = Role::OutputHook {
            conversation_id: brigadier_core::model::ConversationId("c1".into()),
        };
        assert!(tools_for(&hook).is_empty());
    }

    /// suggest_setting takes the permission level and the browser; any other setting (merging on
    /// its own, voice, models, providers, computer use…) doesn't parse.
    #[test]
    fn a_suggested_setting_is_one_of_the_few_the_thread_may_suggest() {
        let thread = Role::Orchestrator {
            conversation_id: brigadier_core::model::ConversationId("c1".into()),
        };
        for setting in ["permission", "browser"] {
            let arguments = serde_json::json!({ "setting": setting, "value": "on", "reason": "r" });
            assert!(
                matches!(
                    parse_call(&thread, "suggest_setting", arguments.as_object().cloned()),
                    Ok(ToolCall::Orchestrator(OrchestratorCall::SuggestSetting(_)))
                ),
                "{setting}"
            );
        }
        for setting in [
            "merge_on_own",
            "voice",
            "model",
            "providers",
            "computer_use",
            "shortcuts",
            "storage",
        ] {
            let arguments = serde_json::json!({ "setting": setting, "value": "on", "reason": "r" });
            assert!(
                parse_call(&thread, "suggest_setting", arguments.as_object().cloned()).is_err(),
                "{setting}"
            );
        }
    }

    /// Every thread can offer a feedback report, found behind the tool search by what it is for.
    #[test]
    fn every_thread_can_offer_feedback() {
        {
            let thread = Role::Orchestrator {
                conversation_id: brigadier_core::model::ConversationId("c1".into()),
            };
            let tool = tools_for(&thread)
                .iter()
                .find(|tool| tool.name == "offer_feedback")
                .expect("offer_feedback");
            let hint = tool
                .meta
                .as_ref()
                .and_then(|meta| meta.0.get("anthropic/searchHint"));
            assert!(hint.is_some_and(|hint| hint.as_str().unwrap().contains("annoyed")));
            // The user may send it after the answer is written: the card says whether (#475).
            assert!(
                tool.description
                    .as_deref()
                    .is_some_and(|text| text.contains("the card, not your answer, says whether"))
            );
            let arguments = serde_json::json!({
                "kind": "bug",
                "title": "Stop leaves the tab spinning",
                "body": "After Stop the tab keeps its spinner.",
            });
            assert!(matches!(
                parse_call(&thread, "offer_feedback", arguments.as_object().cloned()),
                Ok(ToolCall::Orchestrator(OrchestratorCall::OfferFeedback(args)))
                    if args.kind == brigadier_core::FeedbackKind::Bug && args.title == "Stop leaves the tab spinning"
            ));
            let wrong = serde_json::json!({ "kind": "rant", "title": "x", "body": "y" });
            assert!(parse_call(&thread, "offer_feedback", wrong.as_object().cloned()).is_err());
        }
    }

    /// Every thread can offer out-of-scope work as a session, found behind the tool search by
    /// what it is for.
    /// The tools a session keeps moved behind `brigadier` (STRIP-DOWN-PLAN S13), and the
    /// messages between sessions went with no successor: no role lists or calls them over MCP,
    /// but a tab chat keeps its own page tools (it gets no `brigadier`).
    #[test]
    fn the_tools_behind_brigadier_are_in_no_roles_list() {
        const GONE: &[&str] = &[
            "ask_user",
            "start_preview",
            "stop_preview",
            "preview_log",
            "read_page",
            "write_page",
            "app_state",
            "read_terminal",
            "open_in_app",
            "terminal_run",
            "show_session",
            "close_tab",
            "browser_open",
            "browser_navigate",
            "browser_read",
            "browser_click",
            "browser_type",
            "browser_screenshot",
            "browser_close",
            "browser_show",
            "offer_session",
            "list_sessions",
            "message_session",
            "wait_for_replies",
            "read_session",
            "settle_wait",
        ];
        let id = || brigadier_core::model::ConversationId("c1".into());
        let roles = [
            Role::Orchestrator {
                conversation_id: id(),
            },
            Role::Chat {
                conversation_id: id(),
            },
            worker(false),
            worker(true),
        ];
        for role in &roles {
            for name in GONE {
                assert!(
                    !tools_for(role).iter().any(|tool| tool.name == *name),
                    "{role:?} lists {name}"
                );
                assert!(
                    matches!(
                        parse_call(role, name, Some(JsonObject::new())),
                        Err(ParseError::UnknownTool(_))
                    ),
                    "{role:?} calls {name}"
                );
            }
        }
        let tab = Role::TabChat {
            conversation_id: id(),
            tools: TabTools::Page,
        };
        let names: Vec<_> = tools_for(&tab)
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect();
        assert_eq!(names, ["read_page", "write_page"]);
    }

    /// A Chat lists the thread's worker and card tools besides its own, and nothing of git,
    /// checks, landing or other sessions (Q76); it calls them as the thread does.
    #[test]
    fn a_chat_has_the_threads_worker_and_card_tools() {
        let chat = Role::Chat {
            conversation_id: brigadier_core::model::ConversationId("c1".into()),
        };
        let names: Vec<_> = tools_for(&chat)
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect();
        // `ask_user` stays a Chat's call, now made with `brigadier ask`.
        let over_mcp = CHAT_THREAD_TOOLS.iter().filter(|name| **name != "ask_user");
        for name in over_mcp.clone().chain(&["save_memory", "save_report"]) {
            assert!(names.contains(name), "{name}");
        }
        assert_eq!(names.len(), over_mcp.count() + 2, "{names:?}");
        let report = serde_json::json!({ "title": "Nozzles", "markdown": "# Nozzles" });
        assert!(matches!(
            parse_call(&chat, "save_report", report.as_object().cloned()),
            Ok(ToolCall::Chat(ChatCall::SaveReport(_)))
        ));
        for name in ["land_phase", "message_session", "remember"] {
            assert!(matches!(
                parse_call(&chat, name, Some(JsonObject::new())),
                Err(ParseError::UnknownTool(_))
            ));
        }
    }

    /// Every worker, checkers too, may say where it is in its plan; the thread has no such tool.
    #[test]
    fn workers_note_their_progress() {
        for checks in [false, true] {
            assert!(
                tools_for(&worker(checks))
                    .iter()
                    .any(|tool| tool.name == "note_progress"),
                "{checks}"
            );
            let arguments = serde_json::json!({ "note": "Step 3 of 5: wiring the exporter" });
            assert!(matches!(
                parse_call(&worker(checks), "note_progress", arguments.as_object().cloned()),
                Ok(ToolCall::Worker(WorkerCall::NoteProgress(args)))
                    if args.note.starts_with("Step 3 of 5")
            ));
            assert!(parse_call(&worker(checks), "note_progress", None).is_err());
        }
    }

    /// Reviews are plain commands (strip-down S5): no role has a review, findings or merge-hold
    /// tool, and a late call of one is an unknown tool.
    #[test]
    fn no_role_has_a_review_or_merge_hold_tool() {
        let gone = [
            "request_review",
            "record_findings",
            "review_plan",
            "stop_merge",
            "review_candidate",
            "review_code",
        ];
        {
            let thread = Role::Orchestrator {
                conversation_id: brigadier_core::model::ConversationId("c1".into()),
            };
            for role in [thread.clone(), worker(false), worker(true)] {
                for name in gone {
                    assert!(
                        tools_for(&role).iter().all(|tool| tool.name != name),
                        "{name} for {role:?}"
                    );
                    assert!(
                        matches!(
                            parse_call(&role, name, None),
                            Err(ParseError::UnknownTool(_))
                        ),
                        "{name} for {role:?}"
                    );
                }
            }
        }
    }

    /// Bug 7: the thread's instructions write every tool it has with its callable name, so
    /// the list they use names exactly the thread's tools.
    #[test]
    fn the_thread_tool_list_is_the_catalogs() {
        let thread = Role::Orchestrator {
            conversation_id: brigadier_core::model::ConversationId("c1".into()),
        };
        let mut names: Vec<&str> = tools_for(&thread)
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect();
        let mut listed = brigadier_core::tools::THREAD_TOOLS.to_vec();
        names.sort_unstable();
        listed.sort_unstable();
        assert_eq!(names, listed);
    }

    /// Merge cards are retired (BUILD-PLAN A4) and propose_merge is gone (#250): no role lists
    /// it, and a late call is an unknown tool.
    #[test]
    fn propose_merge_is_gone() {
        let thread = Role::Orchestrator {
            conversation_id: brigadier_core::model::ConversationId("c1".into()),
        };
        for role in [thread.clone(), worker(false), worker(true)] {
            assert!(
                tools_for(&role)
                    .iter()
                    .all(|tool| tool.name != "propose_merge"),
                "{role:?}"
            );
        }
        let late = serde_json::json!({ "note": "Adds NOTES.md." });
        assert!(parse_call(&thread, "propose_merge", late.as_object().cloned()).is_err());
    }

    #[test]
    fn a_gate_member_cannot_ask_the_orchestrator() {
        let names = |role: &Role| -> Vec<String> {
            tools_for(role)
                .iter()
                .map(|tool| tool.name.to_string())
                .collect()
        };
        assert!(names(&worker(false)).contains(&"ask_thread".to_owned()));
        assert!(!names(&worker(false)).contains(&"ask_orchestrator".to_owned()));
        let checker = names(&worker(true));
        assert!(!checker.contains(&"ask_thread".to_owned()));
        assert!(checker.contains(&"submit_report".to_owned()));
        let question = || {
            let mut arguments = JsonObject::new();
            arguments.insert("question".into(), Value::String("Which file?".into()));
            Some(arguments)
        };
        // #66: its old name still works for a worker that started on it.
        assert!(parse_call(&worker(false), "ask_thread", question()).is_ok());
        assert!(parse_call(&worker(false), "ask_orchestrator", question()).is_ok());
        assert!(matches!(
            parse_call(&worker(true), "ask_thread", question()),
            Err(ParseError::UnknownTool(_))
        ));
    }

    /// `stop_worker` takes a required one-line reason, which the thread's "Stopped" row shows.
    #[test]
    fn stop_worker_needs_a_reason() {
        // A Chat's, until S16b: the thread's went in S15.
        let thread = Role::Chat {
            conversation_id: brigadier_core::model::ConversationId("c1".into()),
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

    /// A full-access thread's tool list as the CLI gets it, written to
    /// `BRIGADIER_DUMP_TOOLS` when it is set, for measuring it.
    #[test]
    fn dump_thread_tools() {
        let Ok(path) = std::env::var("BRIGADIER_DUMP_TOOLS") else {
            return;
        };
        let thread = Role::Orchestrator {
            conversation_id: brigadier_core::model::ConversationId("c1".into()),
        };
        std::fs::write(
            path,
            serde_json::to_string_pretty(tools_for(&thread)).unwrap(),
        )
        .unwrap();
    }
}
