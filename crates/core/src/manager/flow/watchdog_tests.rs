//! Fault injection through the real provider pump, with explicit timestamps (no timer sleeps).
use super::{Flow, Options, Reply, Script};
use crate::manager::watchdog::{Timing, stall_verdict};
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

fn command(status: &str) -> ProviderEvent {
    serde_json::from_value(serde_json::json!({"type":"command","itemId":"long","command":"sleep 999999","status":status})).unwrap()
}

async fn worker_watch(
    flow: &Flow,
    running: bool,
) -> (
    crate::work::Task,
    Arc<crate::manager::workers::TaskLive>,
    crate::manager::watchdog::WorkerWatch,
) {
    let board = flow
        .until("worker running", |b| {
            b.tasks.values().any(|t| t.state == TaskState::Running)
        })
        .await;
    let task = board.tasks.values().next().unwrap().clone();
    let live = flow.manager.task_live(&task);
    let started = tokio::time::Instant::now();
    loop {
        let watch = live.watch(crate::now_ms()).await;
        if watch.busy && watch.command_running == running {
            return (task, live, watch);
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(30));
        tokio::task::yield_now().await;
    }
}

async fn tick_worker(
    flow: &Flow,
    task: &crate::work::Task,
    live: &Arc<crate::manager::workers::TaskLive>,
    now: i64,
) {
    let board = flow.board().await;
    flow.manager
        .watch_worker(live, task, &board, &Timing::current(), now)
        .await;
    super::eventually("watchdog action finished", || {
        !live
            .watchdog_busy
            .load(std::sync::atomic::Ordering::Acquire)
    })
    .await;
}

#[tokio::test]
async fn watchdog_running_worker_command_is_never_due() {
    // Exercise a shell command and an ordinary tool call; neither has a silence deadline.
    for event in [command("inProgress"), serde_json::from_value(serde_json::json!({"type":"toolCall","itemId":"tool","name":"Read","status":"inProgress"})).unwrap()] {
        let script: Script = Arc::new(move |turn| {
            let event = event.clone();
            Box::pin(async move {
                if turn.is_orchestrator() {
                    let reply = turn.call("delegate_task", serde_json::json!({"effort":"high","title":"Long call","kind":"implement","spec":"Run the command."})).await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                turn.emit(event).await;
                assert!(turn.stopped().await);
                Reply::default()
            })
        });
        let flow = Flow::start("watchdog-running-command", Options::default(), script).await;
        flow.say("Delegate the command.").await;
        let (task, live, watch) = worker_watch(&flow, true).await;
        assert_eq!(stall_verdict(TaskState::Running, false, false, &watch, &Timing::current(), watch.last_event_ms + 24 * 60 * 60_000), None);
        for minute in [6, 30, 120] {
            tick_worker(&flow, &task, &live, watch.last_event_ms + minute * 60_000).await;
        }
        assert_eq!(live.watch(crate::now_ms()).await.reported_at_ms, None);
        assert_eq!(live.watch(crate::now_ms()).await.generation, watch.generation);
        assert_eq!(flow.specs().len(), 2, "no replacement worker");
        assert!(flow.board().await.decisions.iter().all(|d| !d.what.contains("Nudged") && !d.what.contains("Restarted")));
        // A steer is written synchronously into the fake CLI's transcript.
        let transcripts = flow.dir.join("data/transcripts");
        for entry in std::fs::read_dir(transcripts).unwrap() {
            let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            assert!(!text.contains("Nothing has come from you"), "{text}");
        }
        flow.stop().await;
    }
}

#[tokio::test]
async fn watchdog_silent_worker_sends_one_fact_to_thread_without_steering() {
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let seen = inputs.clone();
    let script: Script = Arc::new(move |turn| {
        let seen = seen.clone();
        Box::pin(async move {
            if turn.is_orchestrator() {
                seen.lock().unwrap().push(turn.input.clone());
                if turn.earlier == 0 {
                    let reply = turn.call("delegate_task", serde_json::json!({"effort":"high","title":"Silent worker","kind":"implement","spec":"Wait for the test."})).await;
                    assert!(!reply.is_error, "{}", reply.text);
                }
                return Reply::text("[quiet]");
            }
            turn.emit(command("inProgress")).await;
            turn.emit(command("completed")).await;
            // Status chatter does not count as progress.
            turn.emit(ProviderEvent::Notice {
                level: brigadier_providers::NoticeLevel::Info,
                message: "still connected".into(),
            })
            .await;
            assert!(turn.stopped().await);
            Reply::default()
        })
    });
    let flow = Flow::start("watchdog-worker-fact", Options::default(), script).await;
    flow.say("Delegate and wait.").await;
    let (task, live, mut watch) = worker_watch(&flow, false).await;
    // Wait for the completed command to pass through the real provider pump.
    while watch.last_command.is_none() || watch.command_running {
        tokio::task::yield_now().await;
        watch = live.watch(crate::now_ms()).await;
    }
    let at = watch.last_event_ms + Timing::current().silent;
    tick_worker(&flow, &task, &live, at).await;
    super::eventually("thread received silence fact", || {
        inputs
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.contains("task-1: silent 3 min, last command sleep 999999 (ran 0 min)"))
    })
    .await;
    for minute in [5, 30, 120] {
        tick_worker(&flow, &task, &live, watch.last_event_ms + minute * 60_000).await;
    }
    assert_eq!(
        inputs
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.contains("task-1: silent"))
            .count(),
        1
    );
    assert_eq!(
        live.watch(crate::now_ms()).await.generation,
        watch.generation
    );
    // Check every transcript, independent of the fake provider's native-id spelling.
    for entry in std::fs::read_dir(flow.dir.join("data/transcripts")).unwrap() {
        let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        assert!(!text.contains("Nothing has come from you"), "{text}");
    }
    assert!(
        flow.board()
            .await
            .decisions
            .iter()
            .all(|d| !d.what.contains("Nudged") && !d.what.contains("Restarted"))
    );
    flow.stop().await;
}

#[tokio::test]
async fn watchdog_silent_thread_only_reports_a_fact() {
    let script: Script = Arc::new(|turn| {
        Box::pin(async move {
            assert!(turn.stopped().await);
            Reply::default()
        })
    });
    let flow = Flow::start("watchdog-thread-fact", Options::default(), script).await;
    flow.say("Keep working.").await;
    let (conv, cli, at) = accepted(&flow).await;
    let timing = Timing::current();
    for now in [
        at + timing.silent,
        at + timing.silent * 2,
        at + timing.silent * 10,
    ] {
        flow.manager.watch_thread(&conv, &cli, now).await;
    }
    let notices: Vec<_> = flow
        .events()
        .await
        .into_iter()
        .filter_map(|e| match e {
            crate::model::DomainEvent::ConversationNotice { notice, .. }
                if notice.text.starts_with("Thread: silent") =>
            {
                Some(notice.text)
            }
            _ => None,
        })
        .collect();
    assert_eq!(notices, ["Thread: silent 3 min, last command none"]);
    assert!(flow.board().await.decisions.is_empty());
    assert_eq!(flow.thread_specs().len(), 1, "no automatic continuation");
    for entry in std::fs::read_dir(flow.dir.join("data/transcripts")).unwrap() {
        let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        assert!(!text.contains("Nothing has come from you"));
        assert!(!text.contains("Carry on"));
    }
    flow.stop().await;
}

#[tokio::test]
async fn watchdog_running_thread_call_is_left_alone_then_real_silence_is_reported() {
    let script: Script = Arc::new(|turn| {
        Box::pin(async move {
            assert!(turn.stopped().await);
            Reply::default()
        })
    });
    let flow = Flow::start("watchdog-thread-command", Options::default(), script).await;
    flow.say("Run the long command.").await;
    let (conv, cli, _) = accepted(&flow).await;
    // Inject timestamps through the same progress observer used by the event pump.
    flow.manager
        .note_thread_progress(&conv, &cli, Some(&command("inProgress")))
        .await;
    let at = conv.watchdog_snapshot().await.1;
    for minute in [6, 30, 120] {
        flow.manager
            .watch_thread(&conv, &cli, at + minute * 60_000)
            .await;
    }
    assert_eq!(conv.watchdog_snapshot().await.2, None);
    flow.manager
        .note_thread_progress(&conv, &cli, Some(&command("completed")))
        .await;
    let completed = conv.watchdog_snapshot().await.1;
    flow.manager
        .watch_thread(&conv, &cli, completed + Timing::current().silent)
        .await;
    assert!(conv.watchdog_snapshot().await.2.is_some());
    assert_eq!(flow.thread_specs().len(), 1);
    assert!(flow.board().await.decisions.is_empty());
    flow.stop().await;
}

#[tokio::test]
async fn watchdog_dead_worker_cli_is_still_replaced() {
    let script: Script = Arc::new(|turn| {
        Box::pin(async move {
            if turn.is_orchestrator() {
                if turn.earlier == 0 {
                    let reply = turn.call("delegate_task", serde_json::json!({"effort":"high","title":"Crash","kind":"implement","spec":"Wait for the test."})).await;
                    assert!(!reply.is_error, "{}", reply.text);
                }
                return Reply::text("[quiet]");
            }
            assert!(turn.stopped().await);
            Reply::default()
        })
    });
    let flow = Flow::start("watchdog-dead-worker", Options::default(), script).await;
    flow.say("Delegate the task.").await;
    let (task, live, watch) = worker_watch(&flow, false).await;
    // Detach a disappeared CLI without running the ordinary error recovery, so this
    // specifically tests the watchdog's orphan plumbing.
    let (cli, _) = live.detach_stalled(|_| true, false).await.unwrap();
    crate::manager::workers::TaskLive::end_cli(cli).await;
    live.allow_revival().await;
    let gone = live.watch(crate::now_ms() - Timing::current().grace).await;
    assert!(!gone.alive);
    let now = gone.orphaned_at_ms.unwrap() + Timing::current().grace;
    tick_worker(&flow, &task, &live, now).await;
    super::eventually("dead CLI replacement", || flow.specs().len() >= 3).await;
    assert!(live.watch(crate::now_ms()).await.generation > watch.generation);
    flow.stop().await;
}

#[tokio::test]
async fn watchdog_dead_thread_cli_still_fails_the_request() {
    let script: Script = Arc::new(|_| Box::pin(async { Reply::crashed(9) }));
    let flow = Flow::start("watchdog-dead-thread", Options::default(), script).await;
    flow.say("Keep the transcript.").await;
    flow.until("dead thread handled", |b| {
        b.latest_request()
            .is_some_and(|r| matches!(r.state, RequestState::Failed { .. }))
    })
    .await;
    assert_eq!(flow.thread_specs().len(), 1);
    flow.stop().await;
}

#[tokio::test]
async fn thread_waiting_on_a_user_card_is_never_reported() {
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
async fn thread_waiting_on_a_running_worker_is_never_reported() {
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
