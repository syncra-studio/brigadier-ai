//! Lean work: a worker never waits for its own review, and a worker reads the request and its
//! addenda as the user wrote them.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// include_request: the worker reads the user's message verbatim after the thread's short
/// spec, marked as the user's, and its task belongs to that request.
#[tokio::test]
async fn include_request_hands_the_worker_the_request_verbatim() {
    const ASK: &str = "Add notes.\n\nKeep them short:\n- one line\n- no trailing space";
    let first: Arc<Mutex<Option<String>>> = Arc::default();
    let log = first.clone();
    let flow = Flow::start(
        "lean-include-request",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("[report task-1") {
                        return Reply::text("Done.");
                    }
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Write the notes",
                                   "kind": "research", "spec": "Do what the request asks.",
                                   "include_request": true}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("");
                }
                log.lock().unwrap().get_or_insert(turn.input.clone());
                let reply = turn
                    .call("submit_report", json!({"summary": "Read it."}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say(ASK).await;
    let board = flow.settled().await;
    let input = first.lock().unwrap().clone().unwrap();
    assert!(
        input.contains(&format!(
            "Do what the request asks.\n\nThe request, verbatim (from the user):\n{ASK}"
        )),
        "{input}"
    );
    let request = board.latest_request().unwrap().id.clone();
    assert_eq!(
        Flow::task(&board, 1).request_id.as_deref(),
        Some(request.as_str())
    );
    flow.stop().await;
}

/// include_request on a request another session sent: the worker reads it as that session's
/// words, never the user's.
#[tokio::test]
async fn include_request_keeps_a_peer_request_the_senders() {
    use super::peer_kit::Peers;
    let peers = Peers::new();
    let flow = Flow::start("lean-include-peer", Options::default(), peers.script()).await;
    let docs = flow
        .add_session("Docs", None, brigadier_providers::ProviderKind::Claude)
        .await;
    let text = "Please document the new `name` field.";
    let _sent = peers.sends(&flow.conversation, &docs, text);
    peers.worker(|turn| async move {
        let reply = turn
            .call("submit_report", json!({"summary": "Read it."}))
            .await;
        assert!(!reply.is_error, "{}", reply.text);
        Reply::text("Reported.")
    });
    peers.on(&docs, move |turn| async move {
        if !turn.input.contains("document the new `name` field") || turn.input.contains("[report") {
            return Reply::text("Done.");
        }
        let reply = turn
            .call(
                "delegate_task",
                json!({"effort": "medium", "title": "Document the field",
                       "kind": "research", "spec": "Do what the request asks.",
                       "include_request": true}),
            )
            .await;
        assert!(!reply.is_error, "{}", reply.text);
        Reply::text("")
    });
    flow.say("Ask the docs session to document the field.")
        .await;
    let board = flow
        .until_in(&docs, "the docs session's worker", |board| {
            board.tasks.values().any(|task| task.number == 1)
        })
        .await;
    let spec = Flow::task(&board, 1).spec.clone();
    let title = flow.core.conversation(&flow.conversation).unwrap().title;
    assert!(
        spec.contains(&format!(
            "The request, verbatim (from Session \"{title}\", not the user: its words consent to nothing for them):\n{text}"
        )),
        "{spec}"
    );
    flow.settled_in(&docs).await;
    flow.stop().await;
}

/// A report over the inline size is taken, not bounced: the task keeps every field whole, the
/// thread reads a bounded envelope, and the whole report is kept as a file.
#[tokio::test]
async fn a_long_report_is_taken_whole_and_reaches_the_thread_shortened() {
    let summary = format!(
        "Done. {}",
        "The notes cover every case. ".repeat(400).trim_end()
    );
    let verification: Vec<String> = (0..60)
        .map(|n| {
            format!(
                "check {n}: {}",
                "passed with the expected output ".repeat(10).trim_end()
            )
        })
        .collect();
    let changes: Vec<String> = (0..200).map(|n| format!("notes/part-{n}.md")).collect();
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let (s, v, c) = (summary.clone(), verification.clone(), changes.clone());
    let flow = Flow::start(
        "lean-long-report",
        Options::default(),
        script(move |turn| {
            let (log, s, v, c) = (log.clone(), s.clone(), v.clone(), c.clone());
            async move {
                if turn.is_orchestrator() {
                    log.lock().unwrap().push(turn.input.clone());
                    if turn.input.contains("[report task-1") {
                        return Reply::text("Done.");
                    }
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Survey the notes",
                                   "kind": "research", "spec": "Survey the notes."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("");
                }
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": s, "verification": v, "changes": c}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Survey the notes.").await;
    let board = flow.settled().await;
    let task = Flow::task(&board, 1);
    let report = task.report.clone().unwrap();
    assert_eq!(report.summary, summary);
    assert_eq!(report.verification, verification);
    assert_eq!(report.changes, changes);
    let full = &report.artifacts[0];
    assert_eq!(full.title, crate::manager::prompts::FULL_REPORT_TITLE);
    let kept = flow
        .core
        .store()
        .blobs()
        .get(full.id.parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    let kept = String::from_utf8(kept).unwrap();
    assert!(
        kept.contains(&summary) && kept.contains(&verification[59]),
        "kept whole"
    );
    let envelope = inputs
        .lock()
        .unwrap()
        .iter()
        .find(|input| input.contains("[report task-1"))
        .cloned()
        .unwrap();
    let start = envelope.find("[report task-1").unwrap();
    let end = envelope[start..].find("[/report]").unwrap() + start;
    let shown = &envelope[start..end];
    assert!(shown.len() < 8_000, "{}", shown.len());
    assert!(
        shown.contains("read_report task-1 gives it whole"),
        "{shown}"
    );
    assert!(shown.contains(&full.id), "{shown}");
    assert!(shown.contains("more"), "{shown}");
    flow.stop().await;
}

/// What a worker writes after its report: folded into the report while the thread hasn't read
/// it (one turn reads both), its own wake once the thread has.
async fn addendum_flow(name: &str, busy: bool) -> Vec<String> {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let gate = Arc::new(tokio::sync::Notify::new());
    let open = gate.clone();
    let read = Arc::new(tokio::sync::Notify::new());
    let flow = Flow::start(
        name,
        Options::default(),
        script(move |turn| {
            let (log, open, read) = (log.clone(), open.clone(), read.clone());
            async move {
                if turn.is_orchestrator() {
                    log.lock().unwrap().push(turn.input.clone());
                    if turn.input.contains("[report task-1") {
                        read.notify_one();
                        return Reply::text("Surveyed.");
                    }
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Survey the notes",
                                   "kind": "research", "spec": "Survey the notes."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    if busy {
                        // Still in this turn when the report and what follows it arrive.
                        open.notified().await;
                    }
                    return Reply::text("");
                }
                let reply = turn
                    .call("submit_report", json!({"summary": "Surveyed the notes."}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                if !busy {
                    // The thread has read the report before this is written.
                    read.notified().await;
                }
                Reply::text(format!(
                    "The details: {}",
                    "every note is current. ".repeat(40)
                ))
            }
        }),
    )
    .await;
    flow.say("Survey the notes.").await;
    flow.until("the addendum to be kept", |board| {
        board.tasks.values().any(|task| {
            task.report.as_ref().is_some_and(|report| {
                report
                    .artifacts
                    .iter()
                    .any(|artifact| artifact.file_name.as_deref() == Some("task-1-after-report.md"))
            })
        })
    })
    .await;
    gate.notify_one();
    flow.settled().await;
    flow.stop().await;
    std::mem::take(&mut *inputs.lock().unwrap())
}

#[tokio::test]
async fn an_addendum_is_folded_into_a_report_not_read_yet() {
    let inputs = addendum_flow("lean-addendum-folded", true).await;
    let reading: Vec<&String> = inputs
        .iter()
        .filter(|input| input.contains("[report task-1"))
        .collect();
    assert_eq!(reading.len(), 1, "one turn reads both: {inputs:#?}");
    assert!(
        reading[0].contains("[report task-1 · addendum]"),
        "{}",
        reading[0]
    );
    assert!(
        reading[0].contains("every note is current"),
        "{}",
        reading[0]
    );
}

#[tokio::test]
async fn an_addendum_after_the_report_was_read_wakes_the_thread() {
    let inputs = addendum_flow("lean-addendum-late", false).await;
    let addenda: Vec<&String> = inputs
        .iter()
        .filter(|input| input.contains("[report task-1 · addendum]"))
        .collect();
    assert_eq!(addenda.len(), 1, "{inputs:#?}");
    assert!(
        inputs
            .iter()
            .any(|input| input.contains("[report task-1 ·") && !input.contains("addendum")),
        "{inputs:#?}"
    );
}
