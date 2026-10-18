//! A worker's turns count as turns in its token rows (#29): the first use its turn reports
//! opens the turn, a later report in the same turn doesn't, and its next turn opens again.

use std::sync::Arc;

use brigadier_providers::{ProviderEvent, ProviderKind, TokenUsage};
use serde_json::json;

use super::{Flow, Options, Reply, Script, eventually_async};
use crate::routing::{StepKind, TurnUsage};

fn used(input: i64) -> ProviderEvent {
    ProviderEvent::Usage {
        total: TokenUsage {
            input_tokens: input,
            output_tokens: 10,
            ..TokenUsage::default()
        },
        last: None,
        calls: Vec::new(),
    }
}

/// The thread delegates one task; its worker reports its use twice in its first turn, which
/// ends without a report, and once in the turn the nudge to report starts, then submits.
fn reporting_twice() -> Script {
    Arc::new(|turn| {
        Box::pin(async move {
            if turn.is_review() {
                return Reply::text("No findings.");
            }
            if turn.is_orchestrator() {
                if turn.input.contains("Count the lines.") {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Count the lines",
                                   "kind": "scout", "spec": "Count the lines in README.",
                                   "provider": "claude"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                return Reply::text("Noted.");
            }
            if turn.earlier == 0 {
                turn.emit(used(1_000)).await;
                turn.emit(used(1_500)).await;
                return Reply::text("Counted half.");
            }
            turn.emit(used(2_000)).await;
            let reply = turn
                .call("submit_report", json!({"summary": "Three lines."}))
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        })
    })
}

async fn worker_rows(flow: &Flow) -> Vec<TurnUsage> {
    let store = flow.manager.runtime.routing_store().cloned().unwrap();
    store
        .turns_since(ProviderKind::Claude, 0)
        .await
        .unwrap()
        .into_iter()
        .filter(|row| row.step == Some(StepKind::Worker))
        .collect()
}

#[tokio::test]
async fn a_workers_turns_are_marked_where_they_open() {
    let flow = Flow::start("worker-turns", Options::default(), reporting_twice()).await;
    flow.say("Count the lines.").await;
    eventually_async("the worker's third report", || async {
        worker_rows(&flow).await.len() >= 3
    })
    .await;
    let task = Flow::task(&flow.board().await, 1).id.clone();
    let rows = worker_rows(&flow).await;
    let shown: Vec<_> = rows
        .iter()
        .map(|row| (row.input, row.opens_turn, row.task_id.as_deref()))
        .collect();
    assert_eq!(
        shown,
        [
            (1_000, true, Some(task.0.as_str())),
            (500, false, Some(task.0.as_str())),
            (500, true, Some(task.0.as_str())),
        ]
    );
    flow.stop().await;
}
