//! A session shows stopped once nothing of it runs (#507): a limited turn handed to a stand-in
//! that finds nothing left to say leaves the thread idle, and the composer's Stop of a session
//! whose thread runs no turn says so at once, its workers stopped with it.

use std::sync::Arc;

use brigadier_providers::ProviderKind;
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::work::{BackgroundRunState, RequestState, RunState, TaskState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// A session whose thread delegates one scout on "Scout it." and, on Claude, hits its usage
/// limit on the scout's report; Codex stands in. The scout reports at once.
async fn limited_on_report(name: &str) -> Flow {
    Flow::start(
        name,
        Options::default(),
        script(|turn| async move {
            if !turn.is_orchestrator() {
                let reply = turn
                    .call("submit_report", json!({"summary": "Looked."}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Reported.");
            }
            if turn.input.contains("Scout it.") {
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "high", "title": "Look around",
                               "kind": "scout", "spec": "Look around."}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("");
            }
            if turn.provider == ProviderKind::Claude && turn.input.contains("[report task-1") {
                return Reply::limited();
            }
            Reply::text("Done.")
        }),
    )
    .await
}

#[tokio::test]
async fn a_hand_over_with_nothing_left_to_say_leaves_the_thread_idle() {
    let flow = limited_on_report("stopped-run-hand-over").await;
    flow.say("Scout it.").await;
    flow.until("the scout to finish", |board| {
        board
            .tasks
            .values()
            .any(|task| task.state != TaskState::Running && task.state != TaskState::Starting)
    })
    .await;
    flow.until("Codex to stand in", |_| {
        flow.core
            .conversation(&flow.conversation)
            .is_ok_and(|conversation| conversation.fallback.is_some())
    })
    .await;
    let board = flow
        .until("the thread to be idle", |board| {
            !matches!(board.run, RunState::Running | RunState::Starting)
        })
        .await;
    assert_eq!(board.run, RunState::Idle);
    flow.stop().await;
}

/// The composer's Stop as the app sends it: the thread's turn first, then each worker. Its turn,
/// its worker and its background run all stop, and every request ends Stopped (#507).
#[tokio::test]
async fn the_composers_stop_stops_the_turn_its_workers_and_its_background_runs() {
    let flow = Flow::start(
        "stopped-run-everything",
        Options::default(),
        script(|turn| async move {
            if !turn.is_orchestrator() {
                std::future::pending::<()>().await;
                return Reply::text("");
            }
            if !turn.input.contains("Scout and run.") {
                return Reply::text("Fine.");
            }
            let reply = turn
                .call(
                    "delegate_task",
                    json!({"effort": "high", "title": "Look around",
                           "kind": "scout", "spec": "Look around."}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            let reply = turn.call("run", json!({"command": "sleep 60"})).await;
            assert!(!reply.is_error, "{}", reply.text);
            turn.stopped().await;
            Reply::text("")
        }),
    )
    .await;
    flow.background_commands();
    flow.say("Scout and run.").await;
    let board = flow
        .until("the worker, the run and the turn to run", |board| {
            board.run == RunState::Running
                && board
                    .tasks
                    .values()
                    .any(|task| task.state == TaskState::Running)
                && board
                    .background_runs
                    .values()
                    .any(|run| run.state == BackgroundRunState::Running)
        })
        .await;
    let workers: Vec<_> = board.tasks.keys().cloned().collect();
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    for task in workers {
        flow.manager.stop_task_by_user(task).await.unwrap();
    }
    let board = flow
        .until("everything to stop", |board| {
            board.run == RunState::Idle
                && board
                    .tasks
                    .values()
                    .all(|task| task.state == TaskState::Stopped)
                && board
                    .background_runs
                    .values()
                    .all(|run| run.state != BackgroundRunState::Running)
                && board
                    .requests
                    .values()
                    .all(|request| request.state != RequestState::Working)
        })
        .await;
    assert!(
        board
            .requests
            .values()
            .all(|request| request.state == RequestState::Stopped),
        "{:?}",
        board.requests
    );
    flow.stop().await;
}

/// Stop of a session whose thread runs no turn though its run still says so, as a hand-over
/// with nothing left to say once left it: the session shows stopped at once.
#[tokio::test]
async fn the_composers_stop_with_no_turn_running_shows_the_thread_stopped() {
    let flow = limited_on_report("stopped-run-no-turn").await;
    flow.say("Scout it.").await;
    flow.settled().await;
    // As the hand-over left it in the installed build.
    flow.manager
        .set_run(&flow.conversation, RunState::Running, None)
        .await;
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    assert_eq!(flow.board().await.run, RunState::Idle);
    flow.stop().await;
}
