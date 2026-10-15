//! Fault injection through the real provider pump, with explicit timestamps (no timer sleeps).
use super::{Flow, Options, Reply, Script};
use crate::manager::watchdog::{StallVerdict, Timing, stall_verdict};
use crate::work::{RequestState, TaskState};
use brigadier_providers::ProviderEvent;
use std::sync::{Arc, Mutex};

async fn accepted(
    flow: &Flow,
) -> (
    Arc<crate::manager::conversation::ConvLive>,
    Arc<crate::manager::conversation::Cli>,
    i64,
) {
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    let start = tokio::time::Instant::now();
    loop {
        let (landed, last, _) = conv.watchdog_snapshot().await;
        if landed {
            return (conv.clone(), conv.live_cli().await.unwrap(), last);
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(30));
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn silent_thread_is_nudged_and_continues_without_resending_accepted_input() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let inputs = seen.clone();
    let script: Script = Arc::new(move |turn| {
        let seen = seen.clone();
        Box::pin(async move {
            seen.lock()
                .unwrap()
                .push((turn.input.clone(), turn.native_id.clone()));
            if turn.earlier == 0 {
                assert!(turn.stopped().await);
                Reply::default()
            } else {
                Reply::text("Carried on from the saved session. Done.")
            }
        })
    });
    let flow = Flow::start("watchdog-thread-continue", Options::default(), script).await;
    flow.say("Perform the action once.").await;
    let (conv, cli, at) = accepted(&flow).await;
    let timing = Timing::current();
    flow.manager
        .watch_thread(&conv, &cli, at + timing.silent - 1)
        .await;
    assert_eq!(conv.watchdog_snapshot().await.2, None);
    flow.manager
        .watch_thread(&conv, &cli, at + timing.silent)
        .await;
    assert!(conv.watchdog_snapshot().await.2.is_some());
    flow.manager
        .watch_thread(&conv, &cli, at + timing.silent + timing.after_nudge)
        .await;
    flow.until("continued request done", |b| {
        b.latest_request()
            .is_some_and(|r| r.state == RequestState::Done)
    })
    .await;
    let inputs = inputs.lock().unwrap().clone();
    assert_eq!(inputs.len(), 2, "{inputs:?}");
    assert_eq!(inputs[0].1, inputs[1].1, "same native CLI session");
    assert!(
        !inputs[1].0.contains("Perform the action once."),
        "{}",
        inputs[1].0
    );
    assert!(inputs[1].0.contains("Carry on"), "{}", inputs[1].0);
    let board = flow.board().await;
    assert!(board.decisions.len() >= 2);
    flow.stop().await;
}

#[tokio::test]
async fn thread_that_stalls_again_fails_honestly_and_keeps_the_transcript() {
    let turns = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count = turns.clone();
    let script: Script = Arc::new(move |turn| {
        let turns = turns.clone();
        Box::pin(async move {
            turns.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert!(turn.stopped().await);
            Reply::default()
        })
    });
    let flow = Flow::start("watchdog-thread-twice", Options::default(), script).await;
    flow.say("Keep my request.").await;
    for index in 0..2 {
        if index == 1 {
            super::eventually("second accepted turn", || {
                count.load(std::sync::atomic::Ordering::SeqCst) >= 2
            })
            .await;
        }
        let (conv, cli, at) = accepted(&flow).await;
        let timing = Timing::current();
        flow.manager
            .watch_thread(&conv, &cli, at + timing.silent)
            .await;
        flow.manager
            .watch_thread(&conv, &cli, at + timing.silent + timing.after_nudge)
            .await;
    }
    let board = flow.until("honest model failure", |b| b.latest_request().is_some_and(|r| matches!(&r.state, RequestState::Failed { error } if error.contains("model stopped responding")))).await;
    assert_eq!(board.requests.len(), 1);
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(
        flow.core
            .all_messages(&flow.conversation)
            .await
            .unwrap()
            .iter()
            .any(|m| m.text.contains("Keep my request."))
    );
    flow.stop().await;
}

#[tokio::test]
async fn worker_notice_chatter_does_not_reset_its_progress_clock() {
    let script: Script = Arc::new(move |turn| {
        Box::pin(async move {
            if turn.is_orchestrator() {
                if turn.earlier == 0 {
                    let reply = turn.call("delegate_task", serde_json::json!({"effort":"high","title":"Hold","kind":"implement","spec":"Wait for a test interrupt."})).await;
                    assert!(!reply.is_error, "{}", reply.text);
                }
                return Reply::text("[quiet]");
            }
            for n in 0..20 {
                turn.emit(ProviderEvent::Notice {
                    level: brigadier_providers::NoticeLevel::Info,
                    message: format!("status {n}"),
                })
                .await;
                turn.emit(ProviderEvent::Progress {
                    item_id: "heartbeat".into(),
                })
                .await;
            }
            assert!(turn.stopped().await);
            Reply::default()
        })
    });
    let flow = Flow::start("watchdog-worker-chatter", Options::default(), script).await;
    flow.say("Delegate and wait.").await;
    let board = flow
        .until("worker running", |b| {
            b.tasks.values().any(|t| t.state == TaskState::Running)
        })
        .await;
    let task = board.tasks.values().next().unwrap();
    let live = flow.manager.task_live(task);
    let watch = live.watch(crate::now_ms()).await;
    let timing = Timing::current();
    assert_eq!(
        stall_verdict(
            TaskState::Running,
            false,
            false,
            &watch,
            &timing,
            watch.last_event_ms + timing.silent - 1
        ),
        None
    );
    assert_eq!(
        stall_verdict(
            TaskState::Running,
            false,
            false,
            &watch,
            &timing,
            watch.last_event_ms + timing.silent
        ),
        Some(StallVerdict::Nudge)
    );
    flow.stop().await;
}

#[tokio::test]
async fn thread_waiting_on_a_user_card_is_never_nudged() {
    let script: Script = Arc::new(|turn| {
        Box::pin(async move {
            let reply = turn
                .call(
                    "ask_user",
                    serde_json::json!({"questions":[{"question":"Choose a format."}]}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            assert!(turn.stopped().await);
            Reply::default()
        })
    });
    let flow = Flow::start("watchdog-thread-card", Options::default(), script).await;
    flow.say("Ask for a format.").await;
    flow.until("open user card", |b| {
        b.questions.values().any(|q| q.is_open())
    })
    .await;
    let (conv, cli, at) = accepted(&flow).await;
    flow.manager
        .watch_thread(&conv, &cli, at + 24 * 60 * 60_000)
        .await;
    assert_eq!(conv.watchdog_snapshot().await.2, None);
    assert!(
        flow.board()
            .await
            .decisions
            .iter()
            .all(|d| !d.what.contains("No progress"))
    );
    flow.stop().await;
}

#[tokio::test]
async fn thread_waiting_on_a_running_worker_is_never_nudged() {
    let script: Script = Arc::new(|turn| {
        Box::pin(async move {
            if turn.is_orchestrator() {
                let reply = turn.call("delegate_task", serde_json::json!({"effort":"high","title":"Hold","kind":"implement","spec":"Wait for the test."})).await;
                assert!(!reply.is_error, "{}", reply.text);
            }
            assert!(turn.stopped().await);
            Reply::default()
        })
    });
    let flow = Flow::start("watchdog-thread-worker", Options::default(), script).await;
    flow.say("Delegate and wait for the worker.").await;
    flow.until("worker running", |b| {
        b.tasks.values().any(|t| t.state == TaskState::Running)
    })
    .await;
    let (conv, cli, at) = accepted(&flow).await;
    flow.manager
        .watch_thread(&conv, &cli, at + 24 * 60 * 60_000)
        .await;
    assert_eq!(conv.watchdog_snapshot().await.2, None);
    flow.stop().await;
}

#[tokio::test]
async fn a_thread_cli_that_exits_on_interrupt_resumes_the_same_native_session() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let inputs = seen.clone();
    let script: Script = Arc::new(move |turn| {
        let seen = seen.clone();
        Box::pin(async move {
            let first = {
                let mut seen = seen.lock().unwrap();
                seen.push((turn.input.clone(), turn.native_id.clone()));
                seen.len() == 1
            };
            if first {
                assert!(turn.stopped().await);
                Reply::crashed(9)
            } else {
                Reply::text("Resumed the same session. Done.")
            }
        })
    });
    let flow = Flow::start("watchdog-thread-resume", Options::default(), script).await;
    flow.say("Do this once, then keep the result.").await;
    let (conv, cli, at) = accepted(&flow).await;
    let timing = Timing::current();
    flow.manager
        .watch_thread(&conv, &cli, at + timing.silent)
        .await;
    flow.manager
        .watch_thread(&conv, &cli, at + timing.silent + timing.after_nudge)
        .await;
    flow.until("resumed request done", |b| {
        b.latest_request()
            .is_some_and(|r| r.state == RequestState::Done)
    })
    .await;
    let inputs = inputs.lock().unwrap().clone();
    assert_eq!(inputs.len(), 2, "{inputs:?}");
    assert_eq!(inputs[0].1, inputs[1].1);
    assert!(!inputs[1].0.contains("Do this once, then keep the result."));
    assert!(inputs[1].0.contains("Carry on"));
    let specs = flow.thread_specs();
    assert_eq!(specs.len(), 2);
    assert!(
        matches!(&specs[1].1.origin, brigadier_providers::Origin::Resume { native_id } if native_id == &inputs[0].1)
    );
    flow.stop().await;
}
