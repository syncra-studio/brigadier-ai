//! `read_page` and `write_page`: the thread works on the page the user's latest message was
//! sent from, and on no other. `save_report`: a thread or Chat keeps a research answer's full
//! report as a new page.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::model::Mention;
use crate::pages::{PageContent, Pages};
use crate::tools::{ChatCall, OrchestratorCall, ReadPage, SaveReport, WritePage};

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
async fn editing_a_delivered_page_message_keeps_only_its_pages_grant() {
    let flow = Flow::start("page-tools-edited-grant", Options::default(), quiet()).await;
    let (pages, first) = pages_with(&flow, "first");
    let second = page_holding(&pages, "second");
    flow.manager
        .send_message(
            flow.conversation.clone(),
            "edit it".into(),
            Vec::new(),
            from_page(&first),
            false,
            None,
        )
        .await
        .unwrap();
    flow.settled().await;
    let messages = flow.core.all_messages(&flow.conversation).await.unwrap();
    let at = messages
        .iter()
        .rposition(|message| message.role == crate::model::MessageRole::User)
        .unwrap();
    let original = &messages[at];
    let parent = crate::sessions::parent_of(&messages, at);

    // The edit contains no page token: its context comes from the delivered message.
    flow.manager
        .edit_message(
            flow.conversation.clone(),
            original.id.clone(),
            "change the first item".into(),
            None,
        )
        .await
        .unwrap();
    flow.settled().await;
    let board = flow.board().await;
    let branch = flow
        .core
        .branch(&flow.conversation, &board.head.as_ref().unwrap().0)
        .await
        .unwrap();
    let edited = branch
        .iter()
        .rev()
        .find(|message| message.role == crate::model::MessageRole::User)
        .unwrap();
    assert_ne!(edited.id, original.id);
    assert_eq!(edited.parent_id.as_deref(), Some(parent.as_str()));
    assert_eq!(edited.mentions, from_page(&first));
    assert!(!read(&flow, &first).await.is_error);
    assert!(read(&flow, &second).await.is_error);
    let revision = pages.read(&first).unwrap().revision;
    assert!(!write(&flow, &first, "edited", &revision).await.is_error);
    let revision = pages.read(&second).unwrap().revision;
    assert!(
        write(&flow, &second, "not allowed", &revision)
            .await
            .is_error
    );
    assert_eq!(pages.read(&first).unwrap().content.body, "edited");
    assert_eq!(pages.read(&second).unwrap().content.body, "second");
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

/// The page id a `save_report` reply links.
fn linked_page(reply: &str) -> String {
    reply
        .split("brigadier-page:")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .unwrap_or_default()
        .to_owned()
}

/// Q79: the thread's research report becomes a page of the user's, whose link the answer gives;
/// a Chat's too. Neither needs a page granted by the user's message.
#[tokio::test]
async fn a_thread_and_a_chat_save_a_research_report_as_a_page_the_answer_links() {
    let flow = Flow::start("page-tools-save-report", Options::default(), quiet()).await;
    let pages = Arc::new(Pages::open(flow.repo.with_file_name("pages")));
    flow.manager.set_pages(pages.clone());
    let report = |title: &str, markdown: &str| SaveReport {
        title: title.into(),
        markdown: markdown.into(),
    };
    let thread = flow
        .manager
        .orchestrator_call(
            flow.conversation.clone(),
            OrchestratorCall::SaveReport(report(
                "AI models for 3D printing\n",
                "## Figurines\n\nMeshy [1].\n\n### Sources\n1. [Meshy](https://meshy.ai)",
            )),
        )
        .await;
    assert!(!thread.is_error, "{}", thread.text);
    assert!(
        thread.text.contains("[Full report](brigadier-page:"),
        "{}",
        thread.text
    );
    let page = pages.read(&linked_page(&thread.text)).unwrap();
    assert_eq!(page.content.title, "AI models for 3D printing");
    assert!(page.content.body.starts_with("## Figurines"));
    let chat = flow
        .manager
        .chat_call(
            flow.conversation.clone(),
            ChatCall::SaveReport(report("Printers under 500 euros", "Bambu A1 [1].")),
        )
        .await;
    assert!(!chat.is_error, "{}", chat.text);
    let page = pages.read(&linked_page(&chat.text)).unwrap();
    assert_eq!(page.content.body, "Bambu A1 [1].");
    // Nothing to save is refused, and makes no page.
    let empty = flow
        .manager
        .chat_call(
            flow.conversation.clone(),
            ChatCall::SaveReport(report(" ", "Text.")),
        )
        .await;
    assert!(empty.is_error, "{}", empty.text);
    assert_eq!(pages.list().unwrap().len(), 2);
    flow.stop().await;
}
