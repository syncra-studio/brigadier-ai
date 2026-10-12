//! `read_page` and `write_page`: the thread works on the page the user's latest message was
//! sent from, and on no other.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::model::Mention;
use crate::pages::{PageContent, Pages};
use crate::tools::{OrchestratorCall, ReadPage, WritePage};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

fn quiet() -> Script {
    script(|_| async { Reply::text("ok") })
}

/// The flow's pages folder, shared with its manager, and a page in it holding `body`.
fn pages_with(flow: &Flow, body: &str) -> (Arc<Pages>, String) {
    let pages = Arc::new(Pages::open(flow.repo.with_file_name("pages")));
    flow.manager.set_pages(pages.clone());
    (pages.clone(), page_holding(&pages, body))
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

fn from_page(id: &str) -> Vec<Mention> {
    vec![Mention::Page {
        id: id.into(),
        title: "Notes".into(),
    }]
}

/// The user's message, as delivered to the thread.
async fn wrote(flow: &Flow, mentions: Vec<Mention>) {
    flow.core
        .append_user_message(
            flow.conversation.clone(),
            "edit it".into(),
            Vec::new(),
            mentions,
        )
        .await
        .unwrap();
}

async fn read(flow: &Flow, page_id: &str) -> crate::tools::ToolReply {
    let call = OrchestratorCall::ReadPage(ReadPage {
        page_id: page_id.into(),
    });
    flow.manager
        .orchestrator_call(flow.conversation.clone(), call)
        .await
}

async fn write(flow: &Flow, page_id: &str, body: &str, base: &str) -> crate::tools::ToolReply {
    let call = OrchestratorCall::WritePage(WritePage {
        page_id: page_id.into(),
        body: body.into(),
        title: None,
        base_revision: base.into(),
    });
    flow.manager
        .orchestrator_call(flow.conversation.clone(), call)
        .await
}

#[tokio::test]
async fn the_thread_reads_and_changes_the_page_a_message_was_sent_from() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = seen.clone();
    let flow = Flow::start(
        "page-tools-thread",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("ok");
                }
                let id = turn
                    .input
                    .split("[page ")
                    .nth(1)
                    .and_then(|rest| rest.split_whitespace().next())
                    .unwrap_or_default()
                    .to_owned();
                let read = turn.call("read_page", json!({ "page_id": id })).await;
                log.lock().unwrap().push(read.text.clone());
                let revision = read
                    .text
                    .split("revision ")
                    .nth(1)
                    .and_then(|rest| rest.split_whitespace().next())
                    .unwrap_or_default()
                    .to_owned();
                let wrote = turn
                    .call(
                        "write_page",
                        json!({ "page_id": id, "body": "- [x] milk\n- [ ] eggs", "base_revision": revision }),
                    )
                    .await;
                log.lock().unwrap().push(wrote.text.clone());
                Reply::text("Ticked milk.")
            }
        }),
    )
    .await;
    let (pages, id) = pages_with(&flow, "- [ ] milk\n- [ ] eggs");
    flow.manager
        .send_message(
            flow.conversation.clone(),
            "tick milk".into(),
            Vec::new(),
            from_page(&id),
            false,
            None,
        )
        .await
        .unwrap();
    flow.settled().await;
    let seen = seen.lock().unwrap().clone();
    assert!(seen[0].contains("- [ ] milk"), "{seen:?}");
    assert!(seen[1].starts_with("Saved"), "{seen:?}");
    let page = pages.read(&id).unwrap();
    assert_eq!(page.content.body, "- [x] milk\n- [ ] eggs");
    assert_eq!(page.content.title, "Notes");
    flow.stop().await;
}

#[tokio::test]
async fn only_the_latest_messages_page_is_granted_and_a_message_without_one_revokes_it() {
    let flow = Flow::start("page-tools-grant", Options::default(), quiet()).await;
    let (pages, first) = pages_with(&flow, "first");
    let second = page_holding(&pages, "second");

    // No message from a page yet: nothing.
    assert!(read(&flow, &first).await.is_error);

    wrote(&flow, from_page(&first)).await;
    assert!(!read(&flow, &first).await.is_error);
    let other = read(&flow, &second).await;
    assert!(
        other.is_error && other.text.contains("isn't the page"),
        "{}",
        other.text
    );

    // A message queued from the second page moves nothing until it is delivered.
    flow.core
        .enqueue(
            &flow.conversation,
            "later".into(),
            Vec::new(),
            from_page(&second),
            None,
            false,
        )
        .await
        .unwrap();
    assert!(!read(&flow, &first).await.is_error);
    assert!(read(&flow, &second).await.is_error);
    wrote(&flow, from_page(&second)).await;
    assert!(read(&flow, &first).await.is_error);
    assert!(!read(&flow, &second).await.is_error);

    // A queued message without a page revokes the grant once delivered.
    flow.core
        .enqueue(
            &flow.conversation,
            "plain".into(),
            Vec::new(),
            Vec::new(),
            None,
            false,
        )
        .await
        .unwrap();
    assert!(!read(&flow, &second).await.is_error);
    wrote(&flow, Vec::new()).await;
    let revoked = read(&flow, &second).await;
    assert!(
        revoked.is_error && revoked.text.contains("wasn't sent from a page"),
        "{}",
        revoked.text
    );
    flow.stop().await;
}

#[tokio::test]
async fn a_write_on_a_stale_revision_conflicts_and_changes_nothing() {
    let flow = Flow::start("page-tools-stale", Options::default(), quiet()).await;
    let (pages, id) = pages_with(&flow, "draft");
    wrote(&flow, from_page(&id)).await;
    let stale = pages.read(&id).unwrap().revision;
    let now = pages.read(&id).unwrap();
    let edited = PageContent {
        body: "the user's edit".into(),
        ..now.content
    };
    pages.write(&id, &edited, &now.revision).unwrap();

    let reply = write(&flow, &id, "the agent's edit", &stale).await;
    assert!(
        reply.is_error && reply.text.contains("changed since"),
        "{}",
        reply.text
    );
    assert_eq!(pages.read(&id).unwrap().content.body, "the user's edit");
    flow.stop().await;
}

#[tokio::test]
async fn plan_mode_refuses_page_writes_and_leaves_the_page_as_it_was() {
    let flow = Flow::start(
        "page-tools-plan",
        Options {
            plan_mode: true,
            ..Options::default()
        },
        quiet(),
    )
    .await;
    let (pages, id) = pages_with(&flow, "as it was");
    wrote(&flow, from_page(&id)).await;
    let revision = pages.read(&id).unwrap().revision;
    // Reading is fine in plan mode.
    assert!(!read(&flow, &id).await.is_error);
    let reply = write(&flow, &id, "changed", &revision).await;
    assert!(
        reply.is_error && reply.text.contains("Plan mode is on"),
        "{}",
        reply.text
    );
    assert_eq!(pages.read(&id).unwrap().content.body, "as it was");
    assert_eq!(pages.read(&id).unwrap().revision, revision);
    flow.stop().await;
}
