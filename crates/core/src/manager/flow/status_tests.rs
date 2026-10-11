//! No long silences (SESSION-FIXES-PLAN.md Q9): 30 minutes with nothing new for the user while
//! a request works brings the thread one `[status due]`; a second only after 30 more minutes
//! and a change in its workers.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::status::SILENCE_MS;
use crate::work::TaskState;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

#[tokio::test]
async fn a_silent_request_gets_one_status_line_then_one_per_change() {
    let release = Arc::new(AtomicBool::new(false));
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let (gate, heard) = (release.clone(), inputs.clone());
    let flow = Flow::start(
        "status-due",
        Options::default(),
        script(move |turn| {
            let (gate, heard) = (gate.clone(), heard.clone());
            async move {
                if turn.is_review() {
                    return Reply::text("No findings.");
                }
                if !turn.is_orchestrator() {
                    // Task 1 reads until released; task 2 reads on.
                    while turn.task_number() == Some(2) || !gate.load(Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    let reply = turn
                        .call("submit_report", json!({"summary": "Looked."}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                heard.lock().unwrap().push(turn.input.clone());
                if turn.input.starts_with("Look at both") {
                    for title in ["Read the docs", "Read the code"] {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "medium", "title": title, "kind": "research",
                                       "spec": "Look.", "provider": "claude"}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                    }
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[status due]") {
                    return Reply::text("Still reading: two workers on the docs and the code.");
                }
                Reply::text("[quiet]")
            }
        }),
    )
    .await;
    flow.say("Look at both, then tell me.").await;
    flow.until("both workers to run", |board| {
        board
            .tasks
            .values()
            .filter(|task| task.state == TaskState::Running)
            .count()
            == 2
    })
    .await;
    let due = |inputs: &Arc<Mutex<Vec<String>>>| {
        inputs
            .lock()
            .unwrap()
            .iter()
            .filter(|input| input.contains("[status due]"))
            .cloned()
            .collect::<Vec<_>>()
    };
    let start = crate::now_ms();
    // Not yet at 29 minutes.
    flow.manager.status_due(start + SILENCE_MS - 60_000).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(due(&inputs).is_empty());
    // At 31 minutes: one.
    flow.manager.status_due(start + SILENCE_MS + 60_000).await;
    let heard = inputs.clone();
    flow.until("the status line", move |_| !due(&heard).is_empty())
        .await;
    let first = due(&inputs).remove(0);
    assert!(
        first.contains("min with nothing new for the user")
            && first.contains("Write one status line"),
        "{first}"
    );
    // The status line it wrote is shown: its silence starts again. 30 more minutes with no
    // change: none.
    let later = crate::now_ms() + 2 * SILENCE_MS + 60_000;
    flow.manager.status_due(later).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(due(&inputs).len(), 1);
    // A worker reports: the next one comes, and says so.
    release.store(true, Ordering::SeqCst);
    flow.until("task-1 to report", |board| {
        Flow::task(board, 1).state != TaskState::Running
    })
    .await;
    flow.manager.status_due(later + SILENCE_MS + 60_000).await;
    let heard = inputs.clone();
    flow.until("the second status line", move |_| due(&heard).len() == 2)
        .await;
    let second = due(&inputs).remove(1);
    assert!(
        second.contains("Since then: task-1 \"Read the docs\""),
        "{second}"
    );
    // Only once per change.
    flow.manager
        .status_due(later + 2 * SILENCE_MS + 120_000)
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(due(&inputs).len(), 2);
    // A restart empties the clock: the last stored reply is where the silence starts again,
    // not the request's start.
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    *conv.status.lock().unwrap() = Default::default();
    let page = flow
        .manager
        .core
        .list_messages(flow.conversation.clone(), None, 50)
        .await
        .unwrap();
    let reply = page
        .messages
        .iter()
        .rev()
        .find(|message| message.role == crate::model::MessageRole::Assistant)
        .unwrap();
    let request = reply.request_id.clone().unwrap();
    flow.manager.status_due(later + 3 * SILENCE_MS).await;
    assert_eq!(
        conv.status.lock().unwrap().shown_at(&request),
        Some(reply.created_at_ms)
    );
    flow.stop().await;
}
