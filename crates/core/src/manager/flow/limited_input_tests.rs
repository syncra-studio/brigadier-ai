//! What a turn cut short by a usage limit was reading goes on with whatever takes over (#508):
//! a worker's report, a user message or a background run's result is read exactly once by the
//! next turn, on a stand-in, on another account (which carries on in the same session), or on
//! the chosen model back at once, without the user nudging; and a worker cut short on the
//! thread's message hands it to the model that goes on.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use brigadier_providers::ProviderKind;
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::fallback::CONTINUE_SAME_SESSION;
use crate::manager::prompts::CONTINUE_ON_ACCOUNT;
use crate::work::{RequestState, TaskState};

/// One thread turn as its scripted CLI saw it.
#[derive(Debug, Clone)]
struct Seen {
    provider: ProviderKind,
    account: Option<String>,
    native_id: String,
    input: String,
}

type Log = Arc<Mutex<Vec<Seen>>>;

fn seen(log: &Log) -> Vec<Seen> {
    log.lock().unwrap().clone()
}

/// A thread that, on "Scout it.", delegates a scout (which reports at once). Its first Claude
/// turn reading `marker` hits its usage limit, which resets `resets_in_ms` from then. Every thread turn is logged.
fn limited_on(marker: &'static str, resets_in_ms: Option<i64>, log: &Log) -> Script {
    let (log, limited) = (log.clone(), Arc::new(AtomicBool::new(false)));
    Arc::new(move |turn: Turn| {
        let (log, limited) = (log.clone(), limited.clone());
        Box::pin(async move {
            if !turn.is_orchestrator() {
                let reply = turn
                    .call("submit_report", json!({"summary": "Looked."}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Reported.");
            }
            log.lock().unwrap().push(Seen {
                provider: turn.provider,
                account: turn.account.clone(),
                native_id: turn.native_id.clone(),
                input: turn.input.clone(),
            });
            if turn.provider == ProviderKind::Claude
                && turn.input.contains(marker)
                && !limited.swap(true, Ordering::SeqCst)
            {
                let mut reply = Reply::limited();
                if let (Some(limit), Some(after)) = (&mut reply.limit, resets_in_ms) {
                    limit.resets_at_ms = Some(crate::now_ms() + after);
                }
                return reply;
            }
            if turn.input.contains("Scout it.") && !turn.input.contains("[report task-1") {
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
            Reply::text("Done.")
        })
    })
}

/// After its limited turn on `marker`, the thread's next turns read it exactly once, and its
/// request ends answered, without another word from the user. Answers the turns after the
/// limited one.
async fn read_once_after_the_limit(flow: &Flow, log: &Log, marker: &str) -> Vec<Seen> {
    let board = flow.settled().await;
    let turns = seen(log);
    let limited = turns
        .iter()
        .position(|turn| turn.input.contains(marker))
        .unwrap_or_else(|| panic!("no turn read {marker}: {turns:#?}"));
    let after = turns[limited + 1..].to_vec();
    let reads = after
        .iter()
        .filter(|turn| turn.input.contains(marker) || turn.input.contains(CONTINUE_ON_ACCOUNT))
        .count();
    assert_eq!(reads, 1, "read once after the limit: {turns:#?}");
    assert!(
        board
            .requests
            .values()
            .all(|request| request.state == RequestState::Done),
        "{:?}",
        board.requests
    );
    after
}

/// The 6ed2fe57 case: the thread's turn on a worker's report hits Claude's limit and Codex
/// stands in; the stand-in reads the report.
#[tokio::test]
async fn a_stand_in_reads_the_report_a_limited_turn_was_reading() {
    let log = Log::default();
    let flow = Flow::start(
        "limited-input-stand-in",
        Options::default(),
        limited_on("[report task-1", None, &log),
    )
    .await;
    flow.say("Scout it.").await;
    let after = read_once_after_the_limit(&flow, &log, "[report task-1").await;
    let reader = after
        .iter()
        .find(|turn| turn.input.contains("[report task-1"))
        .unwrap();
    assert_eq!(reader.provider, ProviderKind::Codex, "{after:#?}");
    flow.stop().await;
}

/// Claude is available again right after the stand-in took over: the turn back on Claude
/// reads the report.
#[tokio::test]
async fn the_chosen_model_back_at_once_reads_the_report() {
    let log = Log::default();
    let flow = Flow::start(
        "limited-input-back",
        Options::default(),
        limited_on("[report task-1", Some(0), &log),
    )
    .await;
    flow.say("Scout it.").await;
    read_once_after_the_limit(&flow, &log, "[report task-1").await;
    flow.stop().await;
}

/// Another account takes over in the same session, which already holds the report: the turn
/// there carries on with it rather than reading it again.
#[tokio::test]
async fn another_account_carries_on_with_the_report_in_the_same_session() {
    let log = Log::default();
    let flow = Flow::start(
        "limited-input-account",
        Options::default(),
        limited_on("[report task-1", None, &log),
    )
    .await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    flow.say("Scout it.").await;
    let after = read_once_after_the_limit(&flow, &log, "[report task-1").await;
    let turns = seen(&log);
    let limited = turns
        .iter()
        .find(|turn| turn.input.contains("[report task-1"))
        .unwrap();
    let next = &after[0];
    assert_eq!(next.account.as_deref(), Some("acct-b"), "{after:#?}");
    assert_eq!(next.native_id, limited.native_id, "the same session");
    assert!(next.input.contains(CONTINUE_ON_ACCOUNT), "{}", next.input);
    flow.stop().await;
}

/// The user's message a limited turn was reading goes to the stand-in.
#[tokio::test]
async fn a_stand_in_reads_the_user_message_a_limited_turn_was_reading() {
    let log = Log::default();
    let flow = Flow::start(
        "limited-input-user",
        Options::default(),
        limited_on("Look it up.", None, &log),
    )
    .await;
    flow.say("Look it up.").await;
    let after = read_once_after_the_limit(&flow, &log, "Look it up.").await;
    assert_eq!(after[0].provider, ProviderKind::Codex, "{after:#?}");
    flow.stop().await;
}

/// Each worker turn's provider, account, CLI session and input (its first turn's twice: what
/// it started with, then what was steered in).
type WorkerLog = Arc<Mutex<Vec<(ProviderKind, Option<String>, String, String)>>>;

/// A thread that delegates a scout on "Scout it." (`pin`: on that provider) and messages it on
/// "Tell it."; the scout's first turn hits its usage limit right after the thread's message was
/// steered into it, and the turn after reports.
fn worker_limited_on_message(pin: Option<&'static str>, log: &WorkerLog) -> Script {
    let log = log.clone();
    Arc::new(move |turn: Turn| {
        let log = log.clone();
        Box::pin(async move {
            if turn.is_orchestrator() {
                if turn.input.contains("Tell it.") {
                    let reply = turn
                        .call(
                            "message_worker",
                            json!({"task": "task-1", "text": "Look under the stairs."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("");
                }
                if turn.input.contains("Scout it.") {
                    let mut args = json!({"effort": "high", "title": "Look around",
                                          "kind": "scout", "spec": "Look around."});
                    if let Some(pin) = pin {
                        args["provider"] = json!(pin);
                    }
                    let reply = turn.call("delegate_task", args).await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("");
                }
                return Reply::text("Done.");
            }
            let seen = |input: String| {
                (
                    turn.provider,
                    turn.account.clone(),
                    turn.native_id.clone(),
                    input,
                )
            };
            let first = log.lock().unwrap().is_empty();
            log.lock().unwrap().push(seen(turn.input.clone()));
            if first {
                let steered = turn.steered().await.unwrap_or_default();
                log.lock().unwrap().push(seen(steered));
                return Reply::limited();
            }
            let reply = turn
                .call("submit_report", json!({"summary": "Looked again."}))
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        })
    })
}

/// Starts the scout, tells it once it works, and waits for its report: answers the worker's
/// turns, checked to be its first (with the message steered in) and the one after.
async fn tell_the_worker(
    flow: &Flow,
    log: &WorkerLog,
) -> Vec<(ProviderKind, Option<String>, String, String)> {
    flow.say("Scout it.").await;
    let started = log.clone();
    flow.until("the worker to work", move |board| {
        !started.lock().unwrap().is_empty()
            && board
                .tasks
                .values()
                .any(|task| task.state == TaskState::Running)
    })
    .await;
    flow.say("Tell it.").await;
    flow.until("the report", |board| {
        board.tasks.values().any(|task| {
            task.report
                .as_ref()
                .is_some_and(|report| report.summary.contains("Looked again."))
        })
    })
    .await;
    let turns = log.lock().unwrap().clone();
    assert_eq!(turns.len(), 3, "{turns:#?}");
    assert!(turns[1].3.contains("Look under the stairs."), "{turns:#?}");
    turns
}

/// A worker cut short by its limit right after the thread's message reached it: the other
/// model that takes the task over has the message in its hand-off.
#[tokio::test]
async fn a_worker_cut_short_on_the_threads_message_hands_it_to_the_next_model() {
    let log = WorkerLog::default();
    let flow = Flow::start(
        "limited-input-worker",
        Options::default(),
        worker_limited_on_message(None, &log),
    )
    .await;
    let turns = tell_the_worker(&flow, &log).await;
    assert_ne!(turns[2].0, turns[0].0, "another model: {turns:#?}");
    let board = flow.board().await;
    let task = Flow::task(&board, 1);
    let scratch = &task.workspace.as_ref().unwrap().scratch;
    let spec = std::fs::read_to_string(std::path::Path::new(scratch).join("handoff/spec.md"))
        .unwrap_or_default();
    assert!(
        turns[2].3.contains("Look under the stairs.") || spec.contains("Look under the stairs."),
        "{}\n---\n{spec}",
        turns[2].3
    );
    flow.stop().await;
}

/// The same on another Claude account: its own session, which holds the message, carries on.
#[tokio::test]
async fn a_worker_cut_short_on_the_threads_message_carries_on_with_it_on_another_account() {
    let log = WorkerLog::default();
    let flow = Flow::start(
        "limited-input-worker-account",
        Options::default(),
        worker_limited_on_message(Some("claude"), &log),
    )
    .await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    let turns = tell_the_worker(&flow, &log).await;
    assert_eq!(turns[0].0, ProviderKind::Claude, "{turns:#?}");
    assert_eq!(turns[2].1.as_deref(), Some("acct-b"), "{turns:#?}");
    assert_eq!(turns[2].2, turns[0].2, "the same session: {turns:#?}");
    assert!(turns[2].3.contains(CONTINUE_SAME_SESSION), "{turns:#?}");
    flow.stop().await;
}
