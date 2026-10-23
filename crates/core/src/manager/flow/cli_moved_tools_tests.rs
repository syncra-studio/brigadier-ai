//! STRIP-DOWN-PLAN S16c: the thread's, a Chat's and a side chat's last MCP tools, behind
//! `brigadier` as the caller's own calls of them. The thread searches the Brain, its transcript
//! and the code index, saves reports, offers feedback and asks for approval; a Chat saves
//! reports, and what its CLI asks is the user's card, as a session's is (S28); a side chat reads
//! and changes only its tab's page or file. Each is refused to a caller that didn't have it.

use std::sync::{Arc, Mutex};

use brigadier_cli::protocol::{CliRequest, GRANT_ENV, Tool};
use serde_json::{Value, json};

use super::chat_thread_tests::{answered, new_chat_at};
use super::cli_tests::{Answers, brigadier, id_of, start, tool, wait_for};
use super::{Flow, Options, Reply, Script, Turn};
use crate::RequestState;
use crate::model::{ConversationId, PermissionLevel, TabTarget};
use crate::pages::{PageContent, Pages};
use crate::tools::Role;
use crate::work::CardState;
use brigadier_providers::ApprovalDecision;

fn keep(answers: &Answers, name: &str, value: Value) {
    answers.lock().unwrap().insert(name.into(), value);
}

fn is_worker(turn: &Turn) -> bool {
    matches!(
        turn.env(GRANT_ENV)
            .and_then(|grant| turn.host.grants().resolve(grant)),
        Some(Role::Worker { .. })
    )
}

/// The thread's tools, and what its worker is refused of them.
fn threads(answers: Answers) -> Script {
    Arc::new(move |turn| {
        let answers = answers.clone();
        Box::pin(async move {
            if is_worker(&turn) {
                for (name, request) in [
                    (
                        "worker brain",
                        tool(Tool::Brain, "search", json!({"query": "x"})),
                    ),
                    (
                        "worker report",
                        tool(Tool::Report, "save", json!({"title": "T", "markdown": "M"})),
                    ),
                    (
                        "worker approval",
                        tool(
                            Tool::Approval,
                            "request",
                            json!({"action": "Pay", "details": "Why"}),
                        ),
                    ),
                ] {
                    keep(&answers, name, brigadier(&turn, request).await);
                }
                return Reply::text("Tried them.");
            }
            if !turn.input.contains("Use them.") {
                return Reply::text("Noted.");
            }
            let calls = [
                (
                    "brain",
                    tool(Tool::Brain, "search", json!({"query": "the plan"})),
                ),
                (
                    "transcript",
                    tool(Tool::Transcript, "search", json!({"query": "Use them"})),
                ),
                (
                    "code search",
                    tool(Tool::Code, "search", json!({"query": "README"})),
                ),
                (
                    "code refs",
                    tool(Tool::Code, "refs", json!({"symbol": "main"})),
                ),
                ("code map", tool(Tool::Code, "map", json!({}))),
                (
                    "report",
                    tool(
                        Tool::Report,
                        "save",
                        json!({"title": "Printers", "markdown": "Bambu A1 [1]."}),
                    ),
                ),
                (
                    "feedback",
                    tool(
                        Tool::Feedback,
                        "offer",
                        json!({"kind": "bug", "title": "Stop spins",
                               "body": "After Stop the tab keeps spinning.", "asked": true}),
                    ),
                ),
                (
                    "approval",
                    tool(
                        Tool::Approval,
                        "request",
                        json!({"action": "Buy a domain", "details": "12 euros a year."}),
                    ),
                ),
                ("file read", tool(Tool::Page, "file-read", json!({}))),
            ];
            for (name, request) in calls {
                keep(&answers, name, brigadier(&turn, request).await);
            }
            let started = brigadier(&turn, CliRequest::Start(Box::new(start("Try them.")))).await;
            wait_for(&turn, &[&id_of(&started)], 1).await;
            keep(&answers, "done", json!(true));
            Reply::text("Used them.")
        })
    })
}

#[tokio::test]
async fn the_threads_moved_tools_run_behind_brigadier_and_its_workers_are_refused_them() {
    let answers = Answers::default();
    let flow = Flow::start(
        "cli-moved-thread",
        Options::default(),
        threads(answers.clone()),
    )
    .await;
    let pages = Arc::new(Pages::open(flow.repo.with_file_name("pages")));
    flow.manager.set_pages(pages.clone());
    flow.say("Use them.").await;
    flow.until("it used them", |_| {
        answers.lock().unwrap().contains_key("done")
    })
    .await;
    let answers = answers.lock().unwrap().clone();
    for (name, action) in [
        ("brain", "search"),
        ("transcript", "search"),
        ("code search", "search"),
        ("code refs", "refs"),
        ("code map", "map"),
        ("report", "save"),
        ("feedback", "offer"),
        ("approval", "request"),
    ] {
        assert_eq!(
            answers[name]["action"], action,
            "{name}: {:#}",
            answers[name]
        );
    }
    let report = answers["report"]["text"].as_str().unwrap();
    assert!(report.contains("brigadier-page:"), "{report}");
    let page = report
        .split("brigadier-page:")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .unwrap();
    assert_eq!(pages.read(page).unwrap().content.body, "Bambu A1 [1].");
    // The approval and the feedback are cards for the user.
    let board = flow.core.board(&flow.conversation).await.unwrap();
    assert!(
        board
            .approvals
            .values()
            .any(|approval| format!("{approval:?}").contains("Buy a domain")),
        "{:#?}",
        board.approvals
    );
    // A side chat's file isn't the thread's.
    assert_eq!(answers["file read"]["error"]["code"], "refused");
    // A worker has none of them.
    for name in ["worker brain", "worker report", "worker approval"] {
        assert_eq!(answers[name]["error"]["code"], "refused", "{name}");
    }
    flow.stop().await;
}

/// The answer a Chat's CLI gets to asking to run `command` (Codex's shell asks about every
/// command not on its own safe list).
async fn asked(turn: &Turn, command: &str) -> Value {
    serde_json::to_value(
        turn.ask_tool_approval("commandExecution", command, None)
            .await,
    )
    .unwrap()
}

/// A Chat reads the app and saves reports through `brigadier` (S16c); the thread's tools
/// aren't its. Its shell is its CLI's own, at its level: what the CLI asks is the user's card,
/// never Brigadier's answer (S28).
#[tokio::test]
async fn a_chat_saves_reports_behind_brigadier() {
    let answers = Answers::default();
    let script: Script = {
        let answers = answers.clone();
        Arc::new(move |turn| {
            let answers = answers.clone();
            Box::pin(async move {
                if !turn.prompt.contains("This Chat") {
                    return Reply::text("ok");
                }
                for (name, request) in [
                    (
                        "report",
                        tool(
                            Tool::Report,
                            "save",
                            json!({"title": "Printers", "markdown": "Bambu A1."}),
                        ),
                    ),
                    ("brain", tool(Tool::Brain, "search", json!({"query": "x"}))),
                    ("app", tool(Tool::App, "state", json!({}))),
                ] {
                    keep(&answers, name, brigadier(&turn, request).await);
                }
                let curl = asked(&turn, "/bin/zsh -lc 'curl https://example.com'").await;
                keep(&answers, "ask curl", curl);
                Reply::text("Noted.")
            })
        })
    };
    let flow = Flow::start("cli-moved-chat", Options::default(), script).await;
    flow.manager
        .set_pages(Arc::new(Pages::open(flow.repo.with_file_name("pages"))));
    let chat = new_chat_at(&flow, PermissionLevel::Ask).await;
    flow.say_to(&chat, "Which printer should I buy?").await;
    // Its CLI's question waits on the user's card; Brigadier answers nothing itself.
    let board = flow
        .until_in(&chat, "the Chat's permission card", |board| {
            board
                .approvals
                .values()
                .any(|card| card.state == CardState::Pending)
        })
        .await;
    let card = board
        .approvals
        .values()
        .find(|card| card.state == CardState::Pending)
        .unwrap();
    assert!(format!("{:?}", card.subject).contains("curl"), "{card:#?}");
    flow.manager
        .answer_card(chat.clone(), card.id.clone(), ApprovalDecision::Allow)
        .await
        .unwrap();
    answered(&flow, &chat).await;
    let answers = answers.lock().unwrap().clone();
    assert!(
        answers["report"]["text"]
            .as_str()
            .unwrap()
            .contains("brigadier-page:"),
        "{:#}",
        answers["report"]
    );
    assert_eq!(answers["brain"]["error"]["code"], "refused");
    assert!(
        answers["app"]["error"].is_null() && answers["app"]["text"].is_string(),
        "{:#}",
        answers["app"]
    );
    assert_eq!(answers["ask curl"], json!({"type": "allow"}));
    // It starts at its level as a thread, in its folder, with what `brigadier` needs to know
    // who calls.
    let specs = flow.specs();
    let (_, spec) = specs
        .iter()
        .find(|(_, spec)| spec.cwd == flow.manager.session_dir(&chat).work())
        .expect("the Chat started");
    assert_eq!(spec.access, brigadier_providers::Access::Ask);
    assert_eq!(spec.tools, brigadier_providers::ToolSet::Thread);
    for name in [brigadier_cli::protocol::SESSION_ENV, GRANT_ENV, "PATH"] {
        assert!(spec.env.iter().any(|(key, _)| key == name), "{name}");
    }
    flow.stop().await;
}

fn page_holding(pages: &Pages, body: &str) -> String {
    let page = pages.create().unwrap();
    let content = PageContent {
        title: "Notes".into(),
        emoji: None,
        body: body.into(),
    };
    pages.write(&page.id, &content, &page.revision).unwrap();
    page.id
}

/// The revision a read printed (`… revision <rev>` up to a colon or a line's end).
fn revision_in(text: &str) -> String {
    text.split("revision ")
        .nth(1)
        .and_then(|rest| rest.split([':', '\n']).next())
        .unwrap()
        .trim()
        .to_owned()
}

async fn open_and_say(flow: &Flow, tab: &str, target: TabTarget, text: &str) -> ConversationId {
    let chat = flow
        .manager
        .open_tab_chat(
            &flow.conversation,
            tab.into(),
            target,
            ConversationId(uuid::Uuid::new_v4().to_string()),
        )
        .await
        .unwrap()
        .id;
    flow.say_to(&chat, text).await;
    super::eventually_async("the side chat to answer", || async {
        let board = flow.core.board(&chat).await.unwrap();
        !board.requests.is_empty()
            && board
                .requests
                .values()
                .all(|request| request.state == RequestState::Done)
    })
    .await;
    chat
}

/// A side chat gets `brigadier` for its tab's page or file, and nothing else: another page,
/// the other kind of tab, and every other command are refused.
#[tokio::test]
async fn a_side_chat_reads_and_changes_only_its_tabs_page_or_file_behind_brigadier() {
    let answers = Answers::default();
    // The pages: the tab's, and another.
    let ids: Arc<Mutex<(String, String)>> = Arc::default();
    let script: Script = {
        let answers = answers.clone();
        let ids = ids.clone();
        Arc::new(move |turn| {
            let answers = answers.clone();
            let ids = ids.clone();
            Box::pin(async move {
                if !turn.prompt.contains("You are a chat beside") {
                    return Reply::text("ok");
                }
                let (mine, other) = ids.lock().unwrap().clone();
                if turn.prompt.contains("a page of the user's") {
                    let read =
                        brigadier(&turn, tool(Tool::Page, "read", json!({"page_id": mine}))).await;
                    let revision = revision_in(read["text"].as_str().unwrap_or_default());
                    keep(&answers, "page read", read);
                    let calls = [
                        (
                            "page write",
                            tool(
                                Tool::Page,
                                "write",
                                json!({"page_id": mine, "body": "- [x] milk",
                                       "base_revision": revision}),
                            ),
                        ),
                        (
                            "other page",
                            tool(Tool::Page, "read", json!({"page_id": other})),
                        ),
                        ("page file", tool(Tool::Page, "file-read", json!({}))),
                        (
                            "page brain",
                            tool(Tool::Brain, "search", json!({"query": "x"})),
                        ),
                        ("page sessions", CliRequest::Sessions),
                    ];
                    for (name, request) in calls {
                        keep(&answers, name, brigadier(&turn, request).await);
                    }
                    let read = format!("/bin/zsh -lc 'brigadier page read {mine}'");
                    keep(&answers, "ask page read", asked(&turn, &read).await);
                    keep(
                        &answers,
                        "ask cat",
                        asked(&turn, "/bin/zsh -lc 'cat notes.txt > x'").await,
                    );
                } else {
                    let read = brigadier(&turn, tool(Tool::Page, "file-read", json!({}))).await;
                    let revision = revision_in(read["text"].as_str().unwrap_or_default());
                    keep(&answers, "file read", read);
                    let calls = [
                        (
                            "file write",
                            tool(
                                Tool::Page,
                                "file-write",
                                json!({"text": "two\n", "base_revision": revision}),
                            ),
                        ),
                        (
                            "file page",
                            tool(Tool::Page, "read", json!({"page_id": mine})),
                        ),
                    ];
                    for (name, request) in calls {
                        keep(&answers, name, brigadier(&turn, request).await);
                    }
                }
                Reply::text("Done.")
            })
        })
    };
    let flow = Flow::start("cli-moved-side-chat", Options::default(), script).await;
    let pages = Arc::new(Pages::open(flow.repo.with_file_name("pages")));
    flow.manager.set_pages(pages.clone());
    let mine = page_holding(&pages, "- [ ] milk");
    let other = page_holding(&pages, "secret");
    *ids.lock().unwrap() = (mine.clone(), other.clone());
    std::fs::write(flow.repo.join("notes.txt"), "one\n").unwrap();
    flow.manager.keep_tab_chats(Vec::new()).await.unwrap();
    open_and_say(
        &flow,
        "page:1",
        TabTarget::Page {
            page_id: mine.clone(),
        },
        "Tick the milk.",
    )
    .await;
    open_and_say(
        &flow,
        "file:notes.txt",
        TabTarget::File {
            path: "notes.txt".into(),
        },
        "Make it two.",
    )
    .await;
    let answers = answers.lock().unwrap().clone();

    assert!(
        answers["page read"]["text"]
            .as_str()
            .unwrap()
            .contains("- [ ] milk"),
        "{:#}",
        answers["page read"]
    );
    assert!(
        answers["page write"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Saved"),
        "{:#}",
        answers["page write"]
    );
    assert_eq!(pages.read(&mine).unwrap().content.body, "- [x] milk");
    assert_eq!(answers["other page"]["error"]["code"], "failed");
    // Its shell runs `brigadier` and nothing else.
    assert_eq!(answers["ask page read"], json!({"type": "allow"}));
    assert_eq!(
        answers["ask cat"]["type"], "deny",
        "{:#}",
        answers["ask cat"]
    );
    assert_eq!(pages.read(&other).unwrap().content.body, "secret");
    for name in ["page file", "page brain", "page sessions", "file page"] {
        assert_eq!(
            answers[name]["error"]["code"], "refused",
            "{name}: {:#}",
            answers[name]
        );
    }
    assert!(
        answers["file read"]["text"]
            .as_str()
            .unwrap()
            .contains("one"),
        "{:#}",
        answers["file read"]
    );
    assert!(
        answers["file write"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Saved"),
        "{:#}",
        answers["file write"]
    );
    assert_eq!(
        std::fs::read_to_string(flow.repo.join("notes.txt")).unwrap(),
        "two\n"
    );
    flow.stop().await;
}
