use std::sync::Arc;

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::work::{ApprovalSubject, CardState, RequestState, TaskState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

#[tokio::test]
async fn a_scout_reports_and_the_answer_ends_the_request() {
    let flow = Flow::start(
        "scout",
        Options::default(),
        script(|turn| async move {
            if turn.is_orchestrator() {
                if turn.input.contains("[report task-1") {
                    return Reply::text("The repository holds a README only.");
                }
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"title": "Look around", "kind": "scout", "spec": "List the files."}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("[quiet]");
            }
            let reply = turn
                .call(
                    "submit_report",
                    json!({"summary": "Only README.md and .gitignore."}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        }),
    )
    .await;
    flow.say("What is in the repository?").await;
    let board = flow.settled().await;
    let task = Flow::task(&board, 1);
    assert_eq!(task.state, TaskState::Done);
    assert!(
        board
            .requests
            .values()
            .all(|request| request.state == RequestState::Done)
    );
    flow.stop().await;
}

/// A store an earlier version left behind (real records, their text redacted, plus a landing
/// that waited for the user's approval) loads, and nothing in it acts again.
#[tokio::test]
async fn a_store_from_before_the_phase_flow_loads_and_recovers() {
    let told: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    let heard = told.clone();
    let flow = Flow::start(
        "legacy",
        Options {
            seed: Some(include_str!("fixtures/pre-flow-events.jsonl")),
            ..Options::default()
        },
        script(move |turn| {
            heard.lock().unwrap().push(turn.input.clone());
            async { Reply::text("[quiet]") }
        }),
    )
    .await;
    let seeded = [
        "01a10853-80c6-75fc-9788-21c770d02049",
        "01a108f1-a812-73cd-98ec-4eee23126abe",
    ];
    let conversations: Vec<_> = flow
        .core
        .catalog()
        .conversations
        .into_iter()
        .filter(|conversation| seeded.contains(&conversation.id.0.as_str()))
        .collect();
    assert_eq!(conversations.len(), 2, "both conversations load");
    for conversation in &conversations {
        let board = flow.core.board(&conversation.id).await.unwrap();
        assert!(!board.tasks.is_empty());
        // A landing that waited for the user's click is over: landings don't ask now.
        assert!(
            board.approvals.values().all(|approval| !matches!(
                approval.subject,
                ApprovalSubject::Landing { .. }
            ) || approval.state != CardState::Pending),
            "no landing card is live"
        );
        // Nothing is left mid-landing: an interrupted landing is the orchestrator's again.
        assert!(
            board
                .tasks
                .values()
                .filter(|task| task.kind.writes())
                .all(|task| task.state.is_final()
                    || matches!(task.state, TaskState::Reported | TaskState::ReadyToLand)),
            "{:?}",
            board
                .tasks
                .values()
                .map(|task| (task.number, task.state))
                .collect::<Vec<_>>()
        );
    }
    let legacy = "01a10888-d9e7-764b-9843-72f98b3b7d1d";
    let board = flow.core.board(&conversations[0].id).await.unwrap();
    let board = if board.tasks.keys().any(|id| id.0 == legacy) {
        board
    } else {
        flow.core.board(&conversations[1].id).await.unwrap()
    };
    let task = board
        .tasks
        .values()
        .find(|task| task.id.0 == legacy)
        .unwrap();
    assert_eq!(task.state, TaskState::Reported);
    let number = task.number;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    while !told
        .lock()
        .unwrap()
        .iter()
        .any(|input| input.contains(&format!("[not landed task-{number} ")))
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the orchestrator is told"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    flow.stop().await;
}

/// A copy of a real store (`BRIGADIER_FLOW_STORE`, made read-only with `sqlite3 "file:…?mode=ro"
/// ".backup …"`): every conversation loads after a restart, and no landing card is live.
/// Run by hand: `BRIGADIER_FLOW_STORE=/tmp/brig-store-copy.db cargo test -p brigadier-core
/// --lib real_store -- --ignored`.
#[tokio::test]
#[ignore = "needs a copy of a real store"]
async fn a_real_store_loads_and_recovers() {
    let store = std::env::var_os("BRIGADIER_FLOW_STORE").expect("BRIGADIER_FLOW_STORE");
    let flow = Flow::start(
        "real-store",
        Options {
            store: Some(store.into()),
            ..Options::default()
        },
        script(|_| async { Reply::text("[quiet]") }),
    )
    .await;
    let mut tasks = 0;
    for conversation in flow.core.catalog().conversations {
        let board = flow.core.board(&conversation.id).await.unwrap();
        tasks += board.tasks.len();
        for approval in board.approvals.values() {
            assert!(
                !matches!(approval.subject, ApprovalSubject::Landing { .. })
                    || approval.state != CardState::Pending
            );
        }
        for task in board.tasks.values().filter(|task| task.kind.writes()) {
            assert!(
                task.state.is_final()
                    || matches!(task.state, TaskState::Reported | TaskState::ReadyToLand),
                "task-{} is {:?}",
                task.number,
                task.state
            );
        }
    }
    assert!(tasks > 0, "the store holds tasks");
    flow.stop().await;
}
