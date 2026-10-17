//! A tab's own chat: it lives as long as its tab (across restarts too), reads and changes
//! only what its tab shows, and once its tab closes nothing of it is left: no messages, no
//! records, no CLI files.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::{Flow, Options, Reply, Script, Turn};
use crate::model::{ConversationId, TabTarget, streams};
use crate::pages::{PageContent, Pages};
use crate::tools::{ReadPage, TabChatCall, ToolReply, WritePage, WriteTabFile};
use crate::work::RequestState;

/// What a tab chat's turn saw.
#[derive(Debug, Clone)]
struct Seen {
    prompt: String,
    input: String,
    cwd: PathBuf,
    add_dirs: Vec<PathBuf>,
    native_id: String,
}

type Log = Arc<Mutex<Vec<Seen>>>;

/// Answers every tab chat turn with `answer` (after logging it); other sessions say "ok".
fn tab_script<F, Fut>(log: &Log, answer: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    let log = log.clone();
    let answer = Arc::new(answer);
    Arc::new(move |turn: Turn| {
        let log = log.clone();
        let answer = answer.clone();
        Box::pin(async move {
            if !turn.prompt.contains("You are a chat beside") {
                return Reply::text("ok");
            }
            log.lock().unwrap().push(Seen {
                prompt: turn.prompt.clone(),
                input: turn.input.clone(),
                cwd: turn.cwd.clone(),
                add_dirs: turn.add_dirs.clone(),
                native_id: turn.native_id.clone(),
            });
            answer(turn).await
        })
    })
}

fn quiet(log: &Log) -> Script {
    tab_script(log, |_| async { Reply::text("Sure.") })
}

fn cleaning() -> Options {
    Options {
        behavior: Arc::new(super::FakeBehavior {
            cleanup: true,
            ..Default::default()
        }),
        ..Options::default()
    }
}

/// The app has said which tabs it has (none to drop): tab chats may run.
async fn app_ready(flow: &Flow) {
    flow.manager.keep_tab_chats(Vec::new()).await.unwrap();
}

async fn open(flow: &Flow, tab: &str, target: TabTarget) -> ConversationId {
    flow.manager
        .open_tab_chat(
            &flow.conversation,
            tab.into(),
            target,
            ConversationId(uuid::Uuid::new_v4().to_string()),
        )
        .await
        .unwrap()
        .id
}

async fn send(flow: &Flow, chat: &ConversationId, text: &str) {
    flow.manager
        .send_message(chat.clone(), text.into(), vec![], vec![], false, None)
        .await
        .unwrap();
}

/// Sends `text` and waits until the chat has answered every message.
async fn say(flow: &Flow, chat: &ConversationId, text: &str) {
    send(flow, chat, text).await;
    super::eventually_async("the tab chat to answer", || async {
        let board = flow.core.board(chat).await.unwrap();
        !board.requests.is_empty()
            && board
                .requests
                .values()
                .all(|request| request.state == RequestState::Done)
    })
    .await;
}

/// Every stored event, of every stream.
async fn every_event(flow: &Flow) -> Vec<brigadier_store::StoredEvent> {
    let mut events = Vec::new();
    let mut after = 0;
    loop {
        let page = flow.core.store().read_since(after, 1_000).await.unwrap();
        let full = page.len() == 1_000;
        if let Some(last) = page.last() {
            after = last.seq;
        }
        events.extend(page);
        if !full {
            return events;
        }
    }
}

/// Where the flow's own Claude CLI keeps a session's files.
fn cli_files(flow: &Flow, native_id: &str) -> [PathBuf; 2] {
    super::fake_files(&flow.dir.join("data/own/claude"), native_id)
}

/// Nothing of `chat` is left: not in the catalog, the store, the cleanup ledger, the data
/// folder or the CLI's own files.
async fn left_nothing_of(flow: &Flow, chat: &ConversationId, natives: &[String]) {
    super::eventually("the tab chat to go", || {
        flow.core.conversation(chat).is_err()
    })
    .await;
    flow.manager.cleanup_finished(chat).await;
    assert!(
        !flow
            .core
            .catalog()
            .conversations
            .iter()
            .any(|conversation| &conversation.id == chat)
    );
    let mentions: Vec<String> = every_event(flow)
        .await
        .into_iter()
        .filter(|event| {
            event.stream.contains(&chat.0)
                || event.payload.get().contains(&chat.0)
                || natives.iter().any(|id| event.payload.get().contains(id))
        })
        .map(|event| format!("{} {}", event.stream, event.kind))
        .collect();
    assert!(
        mentions.is_empty(),
        "stored events still name it: {mentions:?}"
    );
    let ledger = flow.manager.runtime.ledger();
    assert!(
        ledger
            .owners()
            .iter()
            .all(|(owner, _, _)| !owner.contains(&chat.0)),
        "{:?}",
        ledger.owners()
    );
    let known = ledger.native_sessions();
    for id in natives {
        assert!(!known.contains_key(id), "the ledger still knows {id}");
        for path in cli_files(flow, id) {
            assert!(!path.exists(), "{} survived", path.display());
        }
    }
    let scratch = flow.dir.join("data/tabchat").join(&chat.0);
    assert!(!scratch.exists(), "{} survived", scratch.display());
    // Nothing of what it said or which CLI sessions it had stays in the databases' freed
    // pages or write-ahead logs, and the logs don't hold its id. (Its bare id can stay in the
    // index's inner pages until the database is next compacted.)
    for file in [
        "brigadier.db",
        "brigadier.db-wal",
        "routing.sqlite",
        "routing.sqlite-wal",
    ] {
        let bytes = database_text(flow, file);
        if file.ends_with("-wal") {
            assert!(!bytes.contains(&chat.0), "{file} still holds its id");
        }
        for id in natives {
            assert!(!bytes.contains(id.as_str()), "{file} still holds {id}");
        }
    }
    // A store reloaded from disk has nothing of it either.
    let reloaded = crate::Core::load(flow.core.store().clone()).await.unwrap();
    assert!(reloaded.conversation(chat).is_err());
}

/// A database file of the flow's data folder, as text (empty when it isn't there).
fn database_text(flow: &Flow, file: &str) -> String {
    let bytes = std::fs::read(flow.dir.join("data").join(file)).unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn natives(log: &Log) -> Vec<String> {
    let mut ids: Vec<String> = log
        .lock()
        .unwrap()
        .iter()
        .map(|seen| seen.native_id.clone())
        .collect();
    ids.dedup();
    ids
}

#[tokio::test]
async fn a_tab_chat_outlives_a_restart_and_its_closed_tab_leaves_nothing() {
    let log = Log::default();
    let mut flow = Flow::start("tab-chat-lifetime", cleaning(), quiet(&log)).await;
    app_ready(&flow).await;
    let chat = open(
        &flow,
        "file:README.md",
        TabTarget::File {
            path: "README.md".into(),
        },
    )
    .await;
    say(&flow, &chat, "What is this file?").await;
    {
        let seen = log.lock().unwrap()[0].clone();
        assert!(
            seen.prompt.contains("a file of the session's checkout"),
            "{}",
            seen.prompt
        );
        assert!(
            seen.input
                .contains("This chat is beside the file README.md"),
            "{}",
            seen.input
        );
        // Not the session's thread: none of its messages or side-chat wording.
        assert!(!seen.input.contains("side chat beside the conversation"));
        // Its own scratch folder; the checkout is only read.
        assert!(
            seen.cwd.ends_with(format!("tabchat/{chat}")),
            "{:?}",
            seen.cwd
        );
        assert_eq!(seen.add_dirs, vec![flow.repo.clone()]);
    }
    // The shared catalog and cleanup records never name it.
    for stream in [streams::CATALOG, streams::CLEANUP] {
        let events = flow
            .core
            .store()
            .read_stream_since(stream.into(), 0, 10_000)
            .await
            .unwrap();
        assert!(
            events
                .iter()
                .all(|event| !event.payload.get().contains(&chat.0)),
            "{stream} names the tab chat"
        );
    }

    flow.restart().await;
    let back = flow
        .core
        .conversation(&chat)
        .expect("the chat is still there");
    assert!(back.tab_chat.is_some());
    assert_eq!(flow.core.all_messages(&chat).await.unwrap().len(), 2);
    flow.manager
        .keep_tab_chats(vec![chat.clone()])
        .await
        .unwrap();
    say(&flow, &chat, "And now?").await;
    let ids = natives(&log);
    assert_eq!(
        ids.len(),
        1,
        "the restarted chat resumed its CLI session: {ids:?}"
    );
    for path in cli_files(&flow, &ids[0]) {
        assert!(
            path.exists(),
            "{} is there while the tab is",
            path.display()
        );
    }

    // Something else works in the checkout while the tab closes.
    let mut other = std::process::Command::new("sleep")
        .arg("30")
        .current_dir(&flow.repo)
        .spawn()
        .unwrap();
    flow.manager.delete(chat.clone()).await.unwrap();
    left_nothing_of(&flow, &chat, &ids).await;
    assert!(
        other.try_wait().unwrap().is_none(),
        "a process in the checkout was stopped"
    );
    let _ = other.kill();
    let _ = other.wait();
    assert!(
        flow.repo.join("README.md").exists(),
        "the checkout is untouched"
    );
    assert!(flow.core.conversation(&flow.conversation).is_ok());
    flow.stop().await;
}

#[tokio::test]
async fn closing_a_tab_stops_its_working_chat_and_wipes_it() {
    let log = Log::default();
    let flow = Flow::start(
        "tab-chat-close-working",
        cleaning(),
        tab_script(&log, |turn| async move {
            turn.stopped().await;
            Reply::text("Stopped.")
        }),
    )
    .await;
    app_ready(&flow).await;
    let chat = open(&flow, "browser:1", TabTarget::Browser).await;
    send(&flow, &chat, "Summarise this page.").await;
    super::eventually("the chat to work", || !log.lock().unwrap().is_empty()).await;
    flow.manager.delete(chat.clone()).await.unwrap();
    left_nothing_of(&flow, &chat, &natives(&log)).await;
    flow.stop().await;
}

#[tokio::test]
async fn archiving_or_deleting_the_session_wipes_its_working_tab_chat() {
    for archive in [true, false] {
        let log = Log::default();
        let flow = Flow::start(
            if archive {
                "tab-chat-session-archived"
            } else {
                "tab-chat-session-deleted"
            },
            cleaning(),
            tab_script(&log, |turn| async move {
                turn.stopped().await;
                Reply::text("Stopped.")
            }),
        )
        .await;
        app_ready(&flow).await;
        let chat = open(&flow, "browser:1", TabTarget::Browser).await;
        send(&flow, &chat, "Summarise this page.").await;
        super::eventually("the chat to work", || !log.lock().unwrap().is_empty()).await;
        if archive {
            flow.manager
                .archive(flow.conversation.clone())
                .await
                .unwrap();
        } else {
            flow.manager
                .delete(flow.conversation.clone())
                .await
                .unwrap();
        }
        flow.manager.cleanup_finished(&flow.conversation).await;
        left_nothing_of(&flow, &chat, &natives(&log)).await;
        flow.stop().await;
    }
}

/// A crash after the app saved a tab's close but before it asked for the delete: after the
/// restart the chat doesn't run, and it goes once the app says which tabs it has.
#[tokio::test]
async fn after_a_crash_an_orphaned_tab_chat_never_runs_and_goes_at_the_next_start() {
    let log = Log::default();
    let mut flow = Flow::start("tab-chat-orphan", cleaning(), quiet(&log)).await;
    app_ready(&flow).await;
    let kept = open(&flow, "browser:kept", TabTarget::Browser).await;
    let orphan = open(&flow, "browser:closed", TabTarget::Browser).await;
    say(&flow, &kept, "Hello.").await;
    say(&flow, &orphan, "Hello.").await;
    let ids = natives(&log);
    assert_eq!(ids.len(), 2);

    flow.restart().await;
    // Until the app has said, neither runs.
    let before = log.lock().unwrap().len();
    let _ = flow
        .manager
        .send_message(orphan.clone(), "Again.".into(), vec![], vec![], false, None)
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(
        log.lock().unwrap().len(),
        before,
        "an unchecked tab chat ran"
    );

    flow.manager
        .keep_tab_chats(vec![kept.clone()])
        .await
        .unwrap();
    left_nothing_of(&flow, &orphan, &ids[1..]).await;
    assert!(flow.core.conversation(&kept).is_ok());
    say(&flow, &kept, "Still here?").await;
    flow.stop().await;
}

#[tokio::test]
async fn a_tab_chat_whose_session_went_while_brigadier_was_down_goes_at_launch() {
    let log = Log::default();
    let mut flow = Flow::start("tab-chat-session-gone", cleaning(), quiet(&log)).await;
    app_ready(&flow).await;
    let chat = open(&flow, "browser:1", TabTarget::Browser).await;
    say(&flow, &chat, "Hello.").await;
    // The session is marked archived, and Brigadier stops before any cleanup ran.
    flow.core
        .mark_archived(flow.conversation.clone())
        .await
        .unwrap();
    flow.restart().await;
    left_nothing_of(&flow, &chat, &natives(&log)).await;
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

async fn tab_call(flow: &Flow, chat: &ConversationId, call: TabChatCall) -> ToolReply {
    flow.manager.tab_chat_call(chat.clone(), call).await
}

#[tokio::test]
async fn a_page_tab_chat_reads_and_changes_its_page_and_no_other() {
    let log = Log::default();
    let flow = Flow::start("tab-chat-page", cleaning(), quiet(&log)).await;
    let pages = Arc::new(Pages::open(flow.repo.with_file_name("pages")));
    flow.manager.set_pages(pages.clone());
    let mine = page_holding(&pages, "- [ ] milk");
    let other = page_holding(&pages, "secret");
    let chat = open(
        &flow,
        "page:1",
        TabTarget::Page {
            page_id: mine.clone(),
        },
    )
    .await;
    let read = tab_call(
        &flow,
        &chat,
        TabChatCall::ReadPage(ReadPage {
            page_id: mine.clone(),
        }),
    )
    .await;
    assert!(!read.is_error, "{}", read.text);
    assert!(read.text.contains("- [ ] milk"), "{}", read.text);
    let revision = pages.read(&mine).unwrap().revision;
    let wrote = tab_call(
        &flow,
        &chat,
        TabChatCall::WritePage(WritePage {
            page_id: mine.clone(),
            body: "- [x] milk".into(),
            title: None,
            base_revision: revision.clone(),
        }),
    )
    .await;
    assert!(wrote.text.starts_with("Saved"), "{}", wrote.text);
    assert_eq!(pages.read(&mine).unwrap().content.body, "- [x] milk");
    // An old revision is refused, never overwritten.
    let stale = tab_call(
        &flow,
        &chat,
        TabChatCall::WritePage(WritePage {
            page_id: mine.clone(),
            body: "lost".into(),
            title: None,
            base_revision: revision,
        }),
    )
    .await;
    assert!(stale.is_error, "{}", stale.text);
    // Another page is out of reach.
    for call in [
        TabChatCall::ReadPage(ReadPage {
            page_id: other.clone(),
        }),
        TabChatCall::WritePage(WritePage {
            page_id: other.clone(),
            body: "changed".into(),
            title: None,
            base_revision: pages.read(&other).unwrap().revision,
        }),
    ] {
        let reply = tab_call(&flow, &chat, call).await;
        assert!(reply.is_error, "{}", reply.text);
    }
    assert_eq!(pages.read(&other).unwrap().content.body, "secret");
    // Nor can it reach a file.
    assert!(tab_call(&flow, &chat, TabChatCall::ReadFile).await.is_error);

    // Deleting the page takes the chats of the tabs that showed it.
    pages.delete(&mine).unwrap();
    flow.manager.page_deleted(&mine).await;
    left_nothing_of(&flow, &chat, &[]).await;
    flow.stop().await;
}

#[tokio::test]
async fn a_file_tab_chat_changes_its_file_and_only_reads_the_rest() {
    let log = Log::default();
    let flow = Flow::start("tab-chat-file", cleaning(), quiet(&log)).await;
    std::fs::write(flow.repo.join("notes.txt"), "one\n").unwrap();
    let chat = open(
        &flow,
        "file:notes.txt",
        TabTarget::File {
            path: "notes.txt".into(),
        },
    )
    .await;
    // The file's tab hears of a save even while no sidebar watches the checkout.
    let mut saves = flow.manager.subscribe_checkout_saves();
    let read = tab_call(&flow, &chat, TabChatCall::ReadFile).await;
    assert!(read.text.contains("one"), "{}", read.text);
    let revision = read
        .text
        .split("revision ")
        .nth(1)
        .and_then(|rest| rest.split(':').next())
        .unwrap()
        .to_owned();
    let wrote = tab_call(
        &flow,
        &chat,
        TabChatCall::WriteFile(WriteTabFile {
            text: "two\n".into(),
            base_revision: revision.clone(),
        }),
    )
    .await;
    assert!(wrote.text.starts_with("Saved"), "{}", wrote.text);
    assert_eq!(
        std::fs::read_to_string(flow.repo.join("notes.txt")).unwrap(),
        "two\n"
    );
    assert_eq!(
        saves.try_recv().ok(),
        Some(crate::work::WorkspaceTarget::Conversation {
            conversation_id: flow.conversation.clone()
        }),
        "a save says the session's checkout changed"
    );
    let stale = tab_call(
        &flow,
        &chat,
        TabChatCall::WriteFile(WriteTabFile {
            text: "lost\n".into(),
            base_revision: revision,
        }),
    )
    .await;
    assert!(stale.is_error, "{}", stale.text);
    assert_eq!(
        std::fs::read_to_string(flow.repo.join("notes.txt")).unwrap(),
        "two\n"
    );
    assert!(saves.try_recv().is_err(), "a refused save says nothing");
    // Its tools are its file's; a page is out of reach.
    let page = tab_call(
        &flow,
        &chat,
        TabChatCall::ReadPage(ReadPage {
            page_id: "any".into(),
        }),
    )
    .await;
    assert!(page.is_error);
    // A tab can't name a file outside the checkout.
    for path in ["../outside.txt", "/etc/hosts", ""] {
        let refused = flow
            .manager
            .open_tab_chat(
                &flow.conversation,
                "file:x".into(),
                TabTarget::File { path: path.into() },
                ConversationId(uuid::Uuid::new_v4().to_string()),
            )
            .await;
        assert!(refused.is_err(), "{path} was accepted");
    }
    flow.stop().await;
}

#[tokio::test]
async fn a_browser_tab_chat_reads_what_its_tab_sent_and_has_no_tools() {
    let log = Log::default();
    let flow = Flow::start("tab-chat-browser", cleaning(), quiet(&log)).await;
    app_ready(&flow).await;
    let chat = open(&flow, "browser:1", TabTarget::Browser).await;
    flow.manager
        .set_tab_page(
            &chat,
            crate::manager::BrowserPage {
                url: "https://example.com/".into(),
                title: "Example Domain".into(),
                text: "This domain is for use in examples.".into(),
                selection: "examples".into(),
            },
        )
        .unwrap();
    say(&flow, &chat, "What does it say?").await;
    let seen = log.lock().unwrap()[0].clone();
    assert!(
        seen.input.contains("Address: https://example.com/"),
        "{}",
        seen.input
    );
    assert!(seen.input.contains("This domain is for use in examples."));
    assert!(seen.input.contains("The user selected: \"examples\""));
    let spec = flow
        .specs
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|(_, spec)| spec.cwd.ends_with(format!("tabchat/{chat}")))
        .map(|(_, spec)| spec.clone())
        .unwrap();
    assert!(
        spec.mcp_servers.is_empty(),
        "a browser tab's chat has no tools"
    );
    assert_eq!(spec.access, brigadier_providers::Access::ReadOnly);
    // Only a browser tab's chat takes a page.
    let file_chat = open(
        &flow,
        "file:README.md",
        TabTarget::File {
            path: "README.md".into(),
        },
    )
    .await;
    assert!(
        flow.manager
            .set_tab_page(&file_chat, crate::manager::BrowserPage::default())
            .is_err()
    );
    // Closing the tab (or New chat) deletes its chat: what the page held goes from memory too.
    assert!(flow.manager.tab_chats.has_page(&chat));
    flow.manager.delete(chat.clone()).await.unwrap();
    super::eventually("the closed tab's chat to go", || {
        flow.core.conversation(&chat).is_err()
    })
    .await;
    assert!(
        !flow.manager.tab_chats.has_page(&chat),
        "the closed tab's page text stays in memory"
    );
    flow.stop().await;
}

#[tokio::test]
async fn the_side_chat_shortcut_never_opens_a_tab_chat() {
    let log = Log::default();
    let flow = Flow::start("tab-chat-not-side-chat", Options::default(), quiet(&log)).await;
    let chat = open(&flow, "browser:1", TabTarget::Browser).await;
    // The session needs a model for a side chat; it has one from its setup.
    let side = flow
        .manager
        .open_side_chat(&flow.conversation, None)
        .await
        .unwrap();
    assert_ne!(side.id, chat);
    assert!(side.tab_chat.is_none());
    assert!(
        flow.manager
            .open_side_chat(&flow.conversation, Some(chat.clone()))
            .await
            .is_err(),
        "a tab chat can't be opened as a side chat"
    );
    // And a tab's chat belongs to that tab only.
    assert!(
        flow.manager
            .open_tab_chat(
                &flow.conversation,
                "browser:2".into(),
                TabTarget::Browser,
                chat.clone()
            )
            .await
            .is_err()
    );
    flow.stop().await;
}

/// Captures the same tracing events the daemon writes, including tasks spawned by the
/// manager and ledger. The unique canaries distinguish this flow from concurrent tests.
#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for CapturedLogs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn a_tab_chat_leaves_no_canaries_in_logs_even_when_deletion_is_retried() {
    use std::sync::atomic::Ordering;

    let captured = CapturedLogs::default();
    let writer = captured.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .json()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || writer.clone())
            .finish(),
    )
    .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let title = format!("canary-title-{}", uuid::Uuid::new_v4());
    let text = format!("canary-message-{}", uuid::Uuid::new_v4());
    let log = Log::default();
    let mut flow = Flow::start(
        "20261012-2245-review-fixes-w10-log-canaries",
        cleaning(),
        quiet(&log),
    )
    .await;
    app_ready(&flow).await;
    let chat = flow
        .manager
        .open_tab_chat(
            &flow.conversation,
            "canary-tab".into(),
            TabTarget::Browser,
            ConversationId(id.clone()),
        )
        .await
        .unwrap()
        .id;
    flow.core
        .rename_conversation(chat.clone(), title.clone())
        .await
        .unwrap();
    say(&flow, &chat, &text).await;
    // Real subprocess stderr can echo a whole prompt or a path. Exercise the shared
    // process used by both adapters, with the tab's actual ledger handle.
    #[cfg(unix)]
    {
        let mut spec = brigadier_sandbox::SpawnSpec::new("/bin/sh")
            .arg("-c")
            .arg("printf '%s\\n' \"$@\" >&2")
            .arg("tab-chat-stderr")
            .arg(id.as_str())
            .arg(title.as_str())
            .arg(text.as_str());
        spec.cwd = Some(flow.dir.join("data/tabchat").join(&id));
        let child = brigadier_providers::process::spawn(
            flow.manager.runtime.platform().clone(),
            &spec,
            brigadier_providers::process::Options {
                ledger: Some(flow.manager.runtime.ledger().handle(format!(
                    "{}{}",
                    crate::ledger::TAB_CHAT_OWNER,
                    id
                ))),
                ..Default::default()
            },
        )
        .unwrap();
        child.process.exited().await;
        super::eventually("stderr to be captured", || {
            child.process.stderr_tail().is_some_and(|stderr| {
                [&id, &title, &text]
                    .into_iter()
                    .all(|canary| stderr.contains(canary.as_str()))
            })
        })
        .await;
    }
    flow.restart().await;
    flow.manager
        .keep_tab_chats(vec![chat.clone()])
        .await
        .unwrap();
    assert_eq!(flow.core.conversation(&chat).unwrap().title, title);
    say(&flow, &chat, &text).await;
    let ids = natives(&log);

    // The heat/build guard observes every CLI owner, including tab chats.
    flow.manager
        .machine_row(crate::machine::Row {
            owner: format!("{}{id}", crate::ledger::TAB_CHAT_OWNER),
            command: format!("{title}: {text}"),
            note: crate::machine::builds::Note::Paused,
        })
        .await;

    let mut parser = brigadier_providers::codex::parse::Parser::live().with_private_logs(true);
    let malformed = serde_json::json!({
        "method": "thread/started",
        "params": { "thread": format!("{id}:{title}:{text}") },
    });
    assert!(!parser.feed(&malformed.to_string()).is_empty());

    // A provider's formatted error may echo identifiers, titles and content together.
    *flow.behavior.cleanup_error.lock().unwrap() = Some(format!("failed {id}: {title}: {text}"));
    flow.behavior.fail_cleanup.store(true, Ordering::SeqCst);
    flow.manager.delete(chat.clone()).await.unwrap();
    flow.manager.cleanup_finished(&chat).await;
    assert!(
        flow.manager
            .runtime
            .ledger()
            .owners()
            .iter()
            .any(|(owner, _, _)| owner.contains(&id))
    );
    assert!(cli_files(&flow, &ids[0])[0].exists());
    assert!(!flow.behavior.removals.lock().unwrap().is_empty());
    let action = crate::manager::disk::Action::Dispose {
        owners: vec![(format!("{}{id}", crate::ledger::TAB_CHAT_OWNER), 0)],
    };
    assert!(action.private_logs());
    let cleaned = flow
        .manager
        .clean_storage(action, crate::manager::disk::ScanContext::default())
        .await
        .unwrap();
    assert!(
        !cleaned.failures.is_empty(),
        "the storage retry must fail too"
    );
    flow.behavior.fail_cleanup.store(false, Ordering::SeqCst);
    flow.restart().await;
    left_nothing_of(&flow, &chat, &ids).await;
    // What it was called and said is overwritten in the databases, not left in freed pages.
    for file in [
        "brigadier.db",
        "brigadier.db-wal",
        "routing.sqlite",
        "routing.sqlite-wal",
    ] {
        let bytes = database_text(&flow, file);
        for canary in [&title, &text] {
            assert!(
                !bytes.contains(canary.as_str()),
                "{file} still holds {canary}"
            );
        }
    }
    // A normal conversation still logs its identifying fields.
    let normal = flow.conversation.0.clone();
    flow.manager
        .delete(flow.conversation.clone())
        .await
        .unwrap();
    flow.manager.cleanup_finished(&flow.conversation).await;
    flow.stop().await;

    let output = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(output.contains("Could not decode a Codex message"));
    #[cfg(unix)]
    assert!(output.contains("cli stderr"));
    assert!(
        output.contains("some artifacts were not removed"),
        "failed cleanup wasn't captured: {output}"
    );
    assert!(
        output.contains("conversation deleted"),
        "successful deletion wasn't captured: {output}"
    );
    assert!(
        output.contains(&normal),
        "normal conversation logs lost their id: {output}"
    );
    let leaked: Vec<_> = [&id, &title, &text]
        .into_iter()
        .filter(|canary| output.contains(canary.as_str()))
        .collect();
    assert!(
        leaked.is_empty(),
        "tab chat canaries leaked: {leaked:?}\n{output}"
    );
}
