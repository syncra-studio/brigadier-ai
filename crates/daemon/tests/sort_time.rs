//! The inbox's sort time through the real daemon: `brigadierd` itself on a scratch data folder
//! under /tmp, with a scratch home so no real agent CLI or account can be reached, spoken to over
//! its own IPC. It first checks the one-time fill of rows stored by an older Brigadier, then which
//! requests move a row. No agent answers here: a turn fails at once, and the requests' handlers
//! run all the same. An image read goes through the same real daemon at the end.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use brigadier_core::model::{Conversation, ConversationId, ConversationKind};
use brigadier_core::work::CardId;
use brigadier_ipc::protocol::{ClientFrame, ClientInfo, Outcome, Request, Response, ServerFrame};
use brigadier_ipc::transport::{Connection, connect_to};
use brigadier_providers::ApprovalDecision;
use brigadier_sandbox::AppPaths;
use brigadier_store::{NewEvent, Store, StoreConfig};
use serde_json::{Value, json};

/// A scratch folder under /tmp, removed when dropped.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch() -> Scratch {
    let dir = Path::new("/tmp").join(format!("brigadier-sort-{}", uuid::Uuid::now_v7().simple()));
    std::fs::create_dir_all(dir.join("home")).unwrap();
    Scratch(dir)
}

/// The daemon of `dir`, stopped when dropped.
struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Client {
    connection: Connection,
    next: u32,
}

impl Client {
    async fn ask(&mut self, request: Request) -> Result<Response, String> {
        self.next += 1;
        let id = self.next;
        self.connection
            .writer
            .write(&ClientFrame::Request { id, request })
            .await
            .unwrap();
        loop {
            let frame = self.connection.reader.read::<ServerFrame>().await.unwrap();
            match frame.expect("the daemon stays up") {
                ServerFrame::Response {
                    id: answered,
                    result,
                } if answered == id => {
                    return match result {
                        Outcome::Ok { value } => Ok(*value),
                        Outcome::Err { error } => Err(format!("{error:?}")),
                    };
                }
                _ => {}
            }
        }
    }

    async fn catalog(&mut self) -> Vec<Conversation> {
        match self.ask(Request::GetCatalog).await.unwrap() {
            Response::GetCatalog { catalog } => catalog.conversations,
            other => panic!("{other:?}"),
        }
    }

    async fn sort_time(&mut self, id: &ConversationId) -> i64 {
        self.catalog()
            .await
            .into_iter()
            .find(|conversation| &conversation.id == id)
            .and_then(|conversation| conversation.last_sent_at_ms)
            .expect("every row has a sort time")
    }
}

/// Starts `brigadierd` on `dir` with nothing of the user's around it, and connects.
async fn start(dir: &Path) -> (Daemon, Client) {
    let data = dir.join("data");
    let daemon = Command::new(env!("CARGO_BIN_EXE_brigadierd"))
        .arg("--foreground")
        .env_clear()
        .env("BRIGADIER_DATA_DIR", &data)
        .env("HOME", dir.join("home"))
        .env("SHELL", "/bin/sh")
        .env("PATH", "/usr/bin:/bin")
        .env("TMPDIR", dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let daemon = Daemon(daemon);
    let paths = AppPaths::resolve(data).unwrap();
    let client = || ClientInfo {
        name: "sort-time-test".into(),
        pid: std::process::id(),
    };
    for _ in 0..600 {
        if let Ok((connection, ..)) = connect_to(&paths, client()).await {
            return (
                daemon,
                Client {
                    connection,
                    next: 0,
                },
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("brigadierd did not come up");
}

async fn stored(store: &Store, stream: &str, kind: &str, event: Value) {
    store
        .append(vec![
            NewEvent::new(stream.to_owned(), kind, 5, &event).unwrap(),
        ])
        .await
        .unwrap();
}

fn conversation_stream(id: &str) -> String {
    format!("conversation:{id}")
}

/// A chat as an older Brigadier stored it: done, snoozed, and no sort time.
async fn old_chat(store: &Store, id: &str, created_at_ms: i64) {
    stored(
        store,
        "catalog",
        "conversation.created",
        json!({ "type": "conversationCreated", "conversation": {
            "id": id, "kind": "chat", "projectId": null, "title": id, "pinnedAtMs": null,
            "createdAtMs": created_at_ms, "updatedAtMs": created_at_ms, "setup": null,
            "doneAtMs": 5, "snoozedUntilMs": 9_000_000_000_000_i64, "snoozedAtMs": 5
        }}),
    )
    .await;
    stored(
        store,
        "catalog",
        "conversation.done",
        json!({ "type": "conversationDone", "id": id, "doneAtMs": 5 }),
    )
    .await;
    stored(
        store,
        "catalog",
        "conversation.snoozed",
        json!({ "type": "conversationSnoozed", "id": id, "untilMs": 9_000_000_000_000_i64, "atMs": 5 }),
    )
    .await;
}

async fn old_message(store: &Store, conversation: &str, role: &str, at_ms: i64) {
    stored(
        store,
        &conversation_stream(conversation),
        "message.appended",
        json!({ "type": "messageAppended", "message": {
            "id": format!("m{at_ms}"), "conversationId": conversation, "seq": 0, "role": role,
            "text": "words", "blob": null, "createdAtMs": at_ms
        }}),
    )
    .await;
}

/// An open card waiting in `conversation`.
async fn open_question(store: &Store, conversation: &str, card: &str, kind: Value) {
    stored(
        store,
        &conversation_stream(conversation),
        "question.updated",
        json!({ "type": "questionUpdated", "question": {
            "id": card, "conversationId": conversation, "taskId": null, "requestId": null,
            "position": 1, "kind": kind, "text": "Merge it?", "options": ["Merge into main", "Not yet"],
            "recommended": null, "items": [], "answer": null, "answers": [],
            "createdAtMs": 10, "answeredAtMs": null
        }}),
    )
    .await;
}

async fn open_approval(store: &Store, conversation: &str, card: &str, subject: Value) {
    stored(
        store,
        &conversation_stream(conversation),
        "approval.updated",
        json!({ "type": "approvalUpdated", "approval": {
            "id": card, "conversationId": conversation, "taskId": null, "requestId": null,
            "position": 2, "subject": subject,
            "state": { "type": "pending" }, "createdAtMs": 10, "resolvedAtMs": null
        }}),
    )
    .await;
}

/// Lets the handlers' writes settle and shows the clock has moved on.
async fn beat() {
    tokio::time::sleep(Duration::from_millis(50)).await;
}

/// Sends a request until the daemon takes it: a turn that is failing holds an edit off briefly.
async fn asked_until_taken(client: &mut Client, request: impl Fn() -> Request) -> Response {
    let mut last = String::new();
    for _ in 0..100 {
        match client.ask(request()).await {
            Ok(response) => return response,
            Err(error) => last = error,
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("never taken: {last}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_sort_time_fills_once_and_moves_only_for_the_users_send() {
    let dir = scratch();
    let data = dir.0.join("data");
    let paths = AppPaths::resolve(data.clone()).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    let store = Store::open(StoreConfig {
        db_path: paths.db_path.clone(),
        blobs_dir: paths.blobs_dir.clone(),
        readers: 1,
    })
    .unwrap();
    // Stores already migrated to the thread engine carry this marker.
    // The separate engine migration deletes conversations when it is absent.
    stored(
        &store,
        "catalog",
        "engine.switched",
        json!({ "type": "engineSwitched", "engine": "thread-1" }),
    )
    .await;
    old_chat(&store, "bare", 7).await;
    old_chat(&store, "talked", 7).await;
    old_chat(&store, "forked", 7).await;
    old_chat(&store, "cards", 7).await;
    old_message(&store, "talked", "user", 100).await;
    old_message(&store, "talked", "peer", 200).await;
    old_message(&store, "talked", "assistant", 300).await;
    old_message(&store, "forked", "user", 3).await;
    open_question(
        &store,
        "cards",
        "question-card",
        json!({ "type": "orchestrator" }),
    )
    .await;
    open_approval(
        &store,
        "cards",
        "action-card",
        json!({ "type": "action", "action": "deploy", "details": "" }),
    )
    .await;
    open_approval(
        &store,
        "cards",
        "plan-card",
        json!({ "type": "plan", "plan": "Do it." }),
    )
    .await;
    store.shutdown().await.unwrap();

    let id = |name: &str| ConversationId(name.to_owned());
    // First launch: every old row is back in the list with a sort time.
    let (daemon, mut client) = start(&dir.0).await;
    let rows = client.catalog().await;
    assert_eq!(rows.len(), 4, "they all load");
    assert_eq!(
        client.sort_time(&id("bare")).await,
        7,
        "no message: created"
    );
    assert_eq!(
        client.sort_time(&id("talked")).await,
        100,
        "the user's message"
    );
    assert_eq!(
        client.sort_time(&id("forked")).await,
        7,
        "never before created"
    );
    let as_json = serde_json::to_string(&rows).unwrap();
    assert!(!as_json.contains("doneAtMs") && !as_json.contains("snoozed"));

    // A send moves the row.
    let chat = match client
        .ask(Request::CreateConversation {
            kind: ConversationKind::Chat,
            project_id: None,
            title: Some("Fresh".into()),
            setup: None,
        })
        .await
        .unwrap()
    {
        Response::CreateConversation { conversation } => conversation.id,
        other => panic!("{other:?}"),
    };
    let created = client.sort_time(&chat).await;
    beat().await;
    let message = match client
        .ask(Request::SendMessage {
            conversation_id: chat.clone(),
            text: "Hello".into(),
            attachments: Vec::new(),
            mentions: Vec::new(),
            steer: false,
            fusion: false,
            queue_index: None,
        })
        .await
        .unwrap()
    {
        Response::SendMessage { outcome } => {
            serde_json::to_value(outcome).unwrap()["message"]["id"]
                .as_str()
                .expect("sent, not queued")
                .to_owned()
        }
        other => panic!("{other:?}"),
    };
    let sent = client.sort_time(&chat).await;
    assert!(sent > created, "SendMessage moves it: {created} -> {sent}");

    // A regenerate answers the same message again: no move.
    beat().await;
    asked_until_taken(&mut client, || Request::Regenerate {
        conversation_id: chat.clone(),
        request_id: message.clone(),
    })
    .await;
    assert_eq!(client.sort_time(&chat).await, sent, "Regenerate leaves it");

    // An edit is the user sending it again: it moves.
    beat().await;
    asked_until_taken(&mut client, || Request::EditMessage {
        conversation_id: chat.clone(),
        message_id: message.clone(),
        text: "Hello again".into(),
        attachments: None,
    })
    .await;
    let edited = client.sort_time(&chat).await;
    assert!(edited > sent, "EditMessage moves it: {sent} -> {edited}");

    // Card answers are not the user's composer: no move, even for the one that sends a message.
    let cards = id("cards");
    let before = client.sort_time(&cards).await;
    beat().await;
    client
        .ask(Request::AnswerQuestion {
            conversation_id: cards.clone(),
            card_id: CardId("question-card".into()),
            answers: vec!["Not yet".into()],
            input: None,
        })
        .await
        .unwrap();
    assert_eq!(
        client.sort_time(&cards).await,
        before,
        "a question's answer leaves it"
    );
    client
        .ask(Request::AnswerCard {
            conversation_id: cards.clone(),
            card_id: CardId("action-card".into()),
            decision: ApprovalDecision::Deny {
                message: "No".into(),
            },
        })
        .await
        .unwrap();
    assert_eq!(
        client.sort_time(&cards).await,
        before,
        "an approval leaves it"
    );
    client
        .ask(Request::AnswerCard {
            conversation_id: cards.clone(),
            card_id: CardId("plan-card".into()),
            decision: ApprovalDecision::Allow,
        })
        .await
        .unwrap();
    assert_eq!(client.sort_time(&cards).await, before, "Build it leaves it");
    // "Build it." did go out through the same send, to the thread or its queue: only the row stayed.
    let view = client
        .ask(Request::GetConversation {
            id: cards.clone(),
            limit: 10,
        })
        .await
        .unwrap();
    let view = serde_json::to_string(&view).unwrap();
    assert!(view.contains("Build it."), "{view}");
    assert_eq!(client.sort_time(&cards).await, before);

    // Second launch: the fill is not done again, and what the user moved stays.
    let moved = client.sort_time(&chat).await;
    drop(client);
    drop(daemon);
    let (_daemon, mut client) = start(&dir.0).await;
    assert_eq!(client.sort_time(&id("talked")).await, 100);
    assert_eq!(client.sort_time(&chat).await, moved);
}

#[tokio::test(flavor = "multi_thread")]
async fn clients_with_the_retired_inbox_commands_are_rejected_at_handshake() {
    use brigadier_ipc::frame::{FrameReader, FrameWriter};
    use brigadier_sandbox::IpcEndpoint;
    use interprocess::local_socket::{
        GenericFilePath, ToFsName, tokio::Stream, traits::tokio::Stream as _,
    };

    let dir = scratch();
    let (_daemon, _client) = start(&dir.0).await;
    let paths = AppPaths::resolve(dir.0.join("data")).unwrap();
    let IpcEndpoint::UnixSocket(path) = &paths.ipc_endpoint else {
        panic!("unix socket")
    };
    let stream = Stream::connect(path.clone().to_fs_name::<GenericFilePath>().unwrap())
        .await
        .unwrap();
    let (recv, send) = stream.split();
    let mut reader = FrameReader::new(recv);
    let mut writer = FrameWriter::new(send);
    writer
        .write(&ClientFrame::Hello {
            token: std::fs::read_to_string(&paths.token_path)
                .unwrap()
                .trim()
                .to_owned(),
            protocol: 19, // Last app that sends setDone/snooze and requires those Conversation fields.
            client: ClientInfo {
                name: "old-inbox-client".into(),
                pid: std::process::id(),
            },
        })
        .await
        .unwrap();
    // It is told the daemon's version (#771), never welcomed.
    let reply = tokio::time::timeout(Duration::from_secs(3), reader.read::<ServerFrame>())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(&reply, Some(ServerFrame::ProtocolMismatch { daemon }) if daemon.protocol != 19),
        "an incompatible old client was welcomed: {reply:?}"
    );
    // A retired inbox command is not served: the connection closes unanswered.
    writer
        .write(&json!({
            "type": "request", "id": 1,
            "request": { "method": "setDone", "id": "c1", "done": true }
        }))
        .await
        .unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(3), reader.read::<Value>())
        .await
        .unwrap()
        .unwrap();
    assert!(
        reply.is_none(),
        "an incompatible old client was answered: {reply:?}"
    );
}

/// A tile of an image a Claude session viewed, as the app reads it: the shape `threadView` gives
/// its item, sent back as `readSessionFileImage`. The app refuses any answer whose `method` isn't
/// the one it asked, and the original file doesn't have to be there.
#[tokio::test(flavor = "multi_thread")]
async fn an_image_in_a_session_file_is_answered_as_what_was_asked() {
    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    let dir = scratch();
    let (_daemon, mut client) = start(&dir.0).await;
    let chat = match client
        .ask(Request::CreateConversation {
            kind: ConversationKind::Chat,
            project_id: None,
            title: Some("Pictures".into()),
            setup: None,
        })
        .await
        .unwrap()
    {
        Response::CreateConversation { conversation } => conversation.id,
        other => panic!("{other:?}"),
    };
    // As Claude Code writes a `Read` of an image.
    let session = dir.0.join("session.jsonl");
    let lines = [
        json!({"type": "assistant", "uuid": "a", "timestamp": "2026-11-04T10:00:00.000Z", "message": {
        "id": "msg", "role": "assistant", "content": [
            {"type": "tool_use", "id": "read", "name": "Read", "input": {"file_path": dir.0.join("gone.png")}}
        ]}}),
        json!({"type": "user", "uuid": "b", "timestamp": "2026-11-04T10:00:01.000Z", "message": {
        "role": "user", "content": [{"type": "tool_result", "tool_use_id": "read", "content": [
            {"type": "image", "source": {"type": "base64", "data": PNG, "media_type": "image/png"}}
        ]}]}}),
    ];
    std::fs::write(&session, format!("{}\n{}\n", lines[0], lines[1])).unwrap();
    let paths = AppPaths::resolve(dir.0.join("data")).unwrap();
    let record = paths.sessions_dir.join(chat.short()).join("record");
    std::fs::create_dir_all(&record).unwrap();
    std::fs::write(
        record.join("lineage.json"),
        json!([{"provider": "claude", "sessionId": "s", "file": session, "startedAt": 1}])
            .to_string(),
    )
    .unwrap();

    let view = match client
        .ask(Request::ThreadView {
            conversation_id: chat.clone(),
            newest_turns: None,
        })
        .await
        .unwrap()
    {
        Response::ThreadView { view } => view,
        other => panic!("{other:?}"),
    };
    let image = view
        .entries
        .iter()
        .find(|entry| !entry.images.is_empty())
        .map(|entry| entry.images[0].clone())
        .expect("the viewed image's item carries it");
    let answer = client
        .ask(Request::ReadSessionFileImage {
            conversation_id: chat,
            image,
        })
        .await
        .unwrap();
    let answer = serde_json::to_value(&answer).unwrap();
    assert_eq!(answer["method"], "readSessionFileImage");
    assert_eq!(answer["data"], PNG);
    assert_eq!(answer["mime"], "image/png");
}
