//! The tools each role sees, their descriptions (the models' manual), and the mapping from an
//! MCP `tools/call` onto [`ToolCall`].

use std::sync::{Arc, OnceLock};

use brigadier_core::tools::{
    ChatCall, CiPush, CodeRefs, CodeSearch, FinishSession, JobCall, LandPhase, NoteForUser,
    OfferFeedback, OrchestratorCall, QueryBrain, ReadPage, RecordNodes, Remember, RequestApproval,
    Role, SaveMemory, SaveReport, SearchTranscript, SuggestSetting, THREAD_TOOLS, TabChatCall,
    TabTools, ToolCall, WritePage, WriteTabFile,
};
use rmcp::model::{JsonObject, MetaObject, Tool};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::schema::{input_schema, no_arguments};

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
you\"). Brigadier lists its own decisions and the workers' needs itself: don't \
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

const SUGGEST_SETTING: &str = "Suggest changing one setting on a card whose one button makes \
the change, when what the user asked for needs it: this session's permission level \
(`permission`: ask, accept_edits or full_access) or the Browser setting \
(`browser`: on or off). reason: one line on why it helps with what they asked. Nothing changes until the user presses \
the button; every other setting is theirs alone. Refused when the setting is already so, and a \
second time in one request.";

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

/// The tools `role` may call, in a stable order. A plain worker gets none: it reaches
/// Brigadier through `brigadier` only.
pub fn tools_for(role: &Role) -> &'static [Tool] {
    static ORCHESTRATOR: OnceLock<Vec<Tool>> = OnceLock::new();
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
        Role::Worker { .. } | Role::OutputHook { .. } => &[],
    }
}

fn orchestrator_tools() -> Vec<Tool> {
    vec![
        tool("query_brain", QUERY_BRAIN, input_schema::<QueryBrain>()),
        tool("remember", REMEMBER, input_schema::<Remember>()),
        tool(
            "search_transcript",
            SEARCH_TRANSCRIPT,
            input_schema::<SearchTranscript>(),
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

fn job_tools() -> Vec<Tool> {
    vec![
        tool("record_nodes", RECORD_NODES, input_schema::<RecordNodes>()),
        tool("code_search", CODE_SEARCH, input_schema::<CodeSearch>()),
        tool("code_refs", CODE_REFS, input_schema::<CodeRefs>()),
        tool("project_map", PROJECT_MAP, no_arguments()),
    ]
}

/// A Chat's: its memories and its reports. It does the job itself (the `/brigade` skill).
fn chat_tools() -> Vec<Tool> {
    vec![
        tool("save_memory", SAVE_MEMORY, input_schema::<SaveMemory>()),
        tool("save_report", SAVE_REPORT, input_schema::<SaveReport>()),
    ]
}

/// The thread's everyday tools, in the model's list from the start (Q40, BUILD-PLAN C2): a
/// Claude thread finds the rest with its tool search when it needs one, which keeps every
/// call's fixed start near plain Claude Code's. Codex reads no such mark and loads them all.
pub const THREAD_ALWAYS_LOADED: &[&str] = &[
    "note_for_user",
    "query_brain",
    "remember",
    "code_search",
    "code_refs",
];

/// What the tool search matches a deferred tool on besides its name and description.
const SEARCH_HINTS: &[(&str, &str)] = &[
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
        Role::Worker { .. } | Role::OutputHook { .. } => Err(unknown()),
    }
}

/// A call of the thread's tool `name`.
fn thread_call(name: &str, arguments: Value) -> Result<ToolCall, ParseError> {
    let unknown = || ParseError::UnknownTool(name.to_owned());
    let call = match name {
        "query_brain" => OrchestratorCall::QueryBrain(args(name, arguments)?),
        "remember" => OrchestratorCall::Remember(args(name, arguments)?),
        "search_transcript" => {
            OrchestratorCall::SearchTranscript(args::<SearchTranscript>(name, arguments)?)
        }
        "request_approval" => OrchestratorCall::RequestApproval(args(name, arguments)?),
        "land_phase" => OrchestratorCall::LandPhase(args(name, arguments)?),
        "finish_session" => OrchestratorCall::FinishSession(args(name, arguments)?),
        "note_for_user" => OrchestratorCall::NoteForUser(args(name, arguments)?),
        "ci_status" => OrchestratorCall::CiStatus(args(name, arguments)?),
        "watch_ci" => OrchestratorCall::WatchCi(args(name, arguments)?),
        "suggest_setting" => OrchestratorCall::SuggestSetting(args(name, arguments)?),
        "offer_feedback" => OrchestratorCall::OfferFeedback(args(name, arguments)?),
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
        reason: err.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tool fits where Claude Code cuts a description loaded up front.
    #[test]
    fn descriptions_fit() {
        let id = || brigadier_core::model::ConversationId("c1".into());
        let roles = [
            Role::Orchestrator {
                conversation_id: id(),
            },
            Role::Chat {
                conversation_id: id(),
            },
        ];
        for role in roles {
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
    }

    /// A thread's everyday tools load up front and the rest wait behind Claude Code's tool
    /// search, the same marks on every listing (the cached prefix stays the same) (BUILD-PLAN
    /// C2).
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
            assert!(loaded.len() * 2 < tools.len(), "most wait: {}", tools.len());
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
    }

    /// What the thread's instructions no longer say is in the tool it applies to, read when
    /// the thread uses it (BUILD-PLAN C1).
    #[test]
    fn the_rules_left_out_of_the_threads_instructions_are_in_their_tools() {
        assert!(CI_STATUS.contains("(`main` only through the usual merge into it here), push it and call watch_ci again without asking"));
    }

    fn worker() -> Role {
        Role::Worker {
            conversation_id: brigadier_core::model::ConversationId("c1".into()),
            task_id: brigadier_core::model::TaskId("t1".into()),
        }
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
            worker(),
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

    /// A Chat lists its memories and reports only, and nothing of git, checks, landing or
    /// other sessions (Q76): it does the job itself.
    #[test]
    fn a_chat_has_its_memories_and_reports_only() {
        let chat = Role::Chat {
            conversation_id: brigadier_core::model::ConversationId("c1".into()),
        };
        let names: Vec<_> = tools_for(&chat)
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect();
        assert_eq!(names, ["save_memory", "save_report"]);
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

    /// Workers are plain CLIs run with `brigadier` (S16b): no role lists or calls a worker,
    /// routing or task tool, and a plain worker gets no tools at all.
    #[test]
    fn no_role_has_a_worker_or_task_tool() {
        const GONE: &[&str] = &[
            "delegate_task",
            "message_worker",
            "answer_worker",
            "stop_worker",
            "rerun_task",
            "route_follow_up",
            "list_tasks",
            "read_report",
            "read_artifact",
            "store_output",
            "ask_thread",
            "ask_orchestrator",
            "note_progress",
            "submit_report",
            "attach_proof",
        ];
        let id = || brigadier_core::model::ConversationId("c1".into());
        let roles = [
            Role::Orchestrator {
                conversation_id: id(),
            },
            Role::Chat {
                conversation_id: id(),
            },
            worker(),
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
        assert!(tools_for(&worker()).is_empty());
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
            for role in [thread.clone(), worker()] {
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
        for role in [thread.clone(), worker()] {
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
