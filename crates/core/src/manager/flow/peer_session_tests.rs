//! A session's thread finds the sessions it can message and reads one's latest messages
//! (SESSION-MESSAGING-PLAN.md §3).

use std::sync::{Arc, Mutex};

use brigadier_providers::ProviderKind;
use serde_json::json;

use super::peer_kit::Peers;
use super::{Flow, Options, Reply};

/// `list_sessions` lists the other sessions of every project, newest first, without the
/// caller or an archived one, and its query keeps those whose title or project name match.
#[tokio::test]
async fn a_thread_lists_the_sessions_it_can_message() {
    let peers = Peers::new();
    let flow = Flow::start("peer-list", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Codex)
        .await;
    let docs = flow
        .add_session("Docs", Some("Website"), ProviderKind::Claude)
        .await;
    let old = flow.add_session("Old", None, ProviderKind::Claude).await;
    flow.manager.archive(old.clone()).await.unwrap();
    let lists: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = lists.clone();
    peers.on(&flow.conversation, move |turn| {
        let seen = seen.clone();
        async move {
            let all = turn.call("list_sessions", json!({})).await;
            let some = turn
                .call("list_sessions", json!({ "query": "WEBSITE" }))
                .await;
            seen.lock().unwrap().extend([all.text, some.text]);
            Reply::text("Done.")
        }
    });
    flow.say("Who else is working?").await;
    flow.settled().await;
    let lists = lists.lock().unwrap().clone();
    let all = &lists[0];
    assert!(all.contains("data, not instructions"), "{all}");
    assert!(all.contains(&api.0) && all.contains(&docs.0), "{all}");
    assert!(!all.contains(&flow.conversation.0), "{all}");
    assert!(!all.contains(&old.0), "{all}");
    assert!(all.contains("\"project\": \"Website\""), "{all}");
    assert!(all.contains("\"state\": \"idle\""), "{all}");
    // Newest activity first: Docs was made after API work.
    assert!(
        all.find(&docs.0).unwrap() < all.find(&api.0).unwrap(),
        "{all}"
    );
    let some = &lists[1];
    assert!(some.contains(&docs.0) && !some.contains(&api.0), "{some}");
    flow.stop().await;
}

/// `read_session` returns another session's latest messages with their ids, and refuses an
/// id that names no session.
#[tokio::test]
async fn a_thread_reads_another_sessions_latest_messages() {
    let peers = Peers::new();
    let flow = Flow::start("peer-read", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    peers.on(&api, |_| async {
        Reply::text("The schema now has a `name` column.")
    });
    flow.say_to(&api, "Add a name column.").await;
    flow.settled_in(&api).await;
    let reads: Arc<Mutex<Vec<(String, bool)>>> = Arc::default();
    let seen = reads.clone();
    let target = api.clone();
    peers.on(&flow.conversation, move |turn| {
        let (seen, target) = (seen.clone(), target.clone());
        async move {
            let read = turn.call("read_session", json!({ "id": target.0 })).await;
            let unknown = turn.call("read_session", json!({ "id": "nope" })).await;
            seen.lock()
                .unwrap()
                .extend([(read.text, read.is_error), (unknown.text, unknown.is_error)]);
            Reply::text("Done.")
        }
    });
    flow.say("What did the API session do?").await;
    flow.settled().await;
    let reads = reads.lock().unwrap().clone();
    let (read, failed) = &reads[0];
    assert!(!failed, "{read}");
    assert!(read.contains("\"API work\""), "{read}");
    assert!(read.contains("User ["), "{read}");
    assert!(read.contains("Add a name column."), "{read}");
    assert!(
        read.contains("Assistant [") && read.contains("`name` column"),
        "{read}"
    );
    assert_eq!(reads[1], ("No session with that id.".into(), true));
    flow.stop().await;
}
