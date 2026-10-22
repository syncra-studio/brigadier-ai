//! A Brigadier tool call whose arguments miss the tool's schema is counted for the bench's
//! tripwire report (WORKSPACES-PLAN phase 4), under the conversation and the task that made it.

use std::sync::Arc;

use serde_json::json;

use super::{Flow, Options, Reply, Script, eventually_async};
use crate::tools::ToolHost;

/// The thread delegates one task; the thread and the worker each send one call the MCP server
/// found bad, and the worker reports.
fn calling_badly() -> Script {
    Arc::new(|turn| {
        Box::pin(async move {
            if turn.is_review() {
                return Reply::text("No findings.");
            }
            if turn.is_orchestrator() {
                if turn.input.contains("Count the lines.") {
                    ToolHost::bad_call(&*turn.host, &turn.grant, "stop_worker");
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Count the lines",
                                   "kind": "scout", "spec": "Count the lines in README.",
                                   "provider": "claude"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("");
                }
                return Reply::text("Noted.");
            }
            ToolHost::bad_call(&*turn.host, &turn.grant, "submit_report");
            let reply = turn
                .call("submit_report", json!({"summary": "Three lines."}))
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        })
    })
}

#[tokio::test]
async fn bad_tool_calls_are_counted_by_conversation_and_task() {
    let flow = Flow::start("bad-tool-calls", Options::default(), calling_badly()).await;
    flow.say("Count the lines.").await;
    let store = flow.manager.runtime.routing_store().cloned().unwrap();
    eventually_async("both bad calls are counted", || async {
        store.bad_tool_call_rows(0).await.unwrap().len() == 2
    })
    .await;
    let task = Flow::task(&flow.board().await, 1).id.clone();
    let rows = store.bad_tool_call_rows(0).await.unwrap();
    let conversation = Some(flow.conversation.0.clone());
    assert_eq!(
        rows,
        [
            (conversation.clone(), None, "stop_worker".to_owned()),
            (conversation, Some(task.0), "submit_report".to_owned()),
        ]
    );
    flow.stop().await;
}
