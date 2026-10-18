//! A cheap scout or research task runs again once, one tier up, when its answer falls short
//! (WORKSPACES-PLAN.md phase 6): a [not met] check reruns it by itself, the thread's
//! `rerun_task` when it judges the answer unsupported. A citation that doesn't hold is no
//! reason, and nothing reruns twice. The thread is told plainly and stays free.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{Board, Flow, Options, Reply, Script, Turn};
use crate::work::{RequestState, Task, TaskState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// Codex lists a light model too, so a scout starts cheap.
fn cheap() -> Options {
    Options {
        behavior: Arc::new(super::FakeBehavior {
            light_model: true,
            ..Default::default()
        }),
        ..Options::default()
    }
}

const PARSER: &str = "pub fn parse(text: &str) -> usize {\n    text.len()\n}\n";

/// The thread's scout: find the parser.
async fn delegate_scout(turn: &Turn) {
    let reply = turn
        .call(
            "delegate_task",
            json!({"effort": "low", "title": "Find the parser", "kind": "scout",
                   "spec": "Find where the parser is defined. Done when: the file and line are named."}),
        )
        .await;
    assert!(!reply.is_error, "{}", reply.text);
}

/// A worker's report with one "done when" line.
async fn report(turn: &Turn, done_when: &str) {
    let reply = turn
        .call(
            "submit_report",
            json!({"summary": "Looked for the parser.", "done_when": [done_when]}),
        )
        .await;
    assert!(!reply.is_error, "{}", reply.text);
}

fn ended(board: &Board) -> bool {
    !board.tasks.is_empty()
        && board.tasks.values().all(|task| task.state.is_final())
        && board
            .requests
            .values()
            .any(|request| request.state == RequestState::Done)
}

/// The tier routing gives the model that ran `task`.
async fn tier(flow: &Flow, task: &Task) -> brigadier_router::QualityTier {
    let inputs = flow.manager.routing_inputs(None, crate::now_ms()).await;
    let id = task.route.choice.model.as_deref().unwrap();
    inputs
        .models
        .iter()
        .find(|model| {
            model.provider == task.route.choice.provider
                && brigadier_router::is_model(model, &inputs.registry, id)
        })
        .unwrap()
        .tier_for(brigadier_router::TaskCategory::Scout)
        .unwrap()
}

/// The scout's check fails: it reruns once, one tier up, and the thread reads why with the
/// report. The rerun's check fails too: nothing reruns twice. Its explanation and its outcome
/// name the cheaper model and the failed check.
#[tokio::test]
async fn a_scout_whose_check_fails_reruns_once_one_tier_up() {
    let thread: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = thread.clone();
    let flow = Flow::start(
        "rerun-failed-check",
        cheap(),
        script(move |turn| {
            let seen = seen.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.starts_with("Find the parser") {
                        delegate_scout(&turn).await;
                        return Reply::text("[quiet]");
                    }
                    seen.lock().unwrap().push(turn.input.clone());
                    if turn.input.contains("[report task-2 ") {
                        return Reply::text("Nobody found the parser.");
                    }
                    return Reply::text("[quiet]");
                }
                report(
                    &turn,
                    "[not met] the file and line are named: no file defines it",
                )
                .await;
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Find the parser.").await;
    let board = flow.until("both scouts to end", ended).await;
    assert_eq!(
        board.tasks.len(),
        2,
        "one rerun, never two: {:#?}",
        board.tasks
    );
    let (first, rerun) = (Flow::task(&board, 1), Flow::task(&board, 2));
    assert_eq!(first.state, TaskState::Done);
    let of = rerun.rerun.as_ref().expect("task-2 reruns task-1");
    assert_eq!(of.task_id, first.id.to_string());
    assert_eq!(of.provider, first.route.choice.provider);
    assert_eq!(Some(&of.model), first.route.choice.model.as_ref());
    assert!(
        of.reason.contains("[not met] the file and line are named"),
        "{}",
        of.reason
    );
    assert_eq!((rerun.kind, &rerun.spec), (first.kind, &first.spec));
    let (cheap, stronger) = (tier(&flow, first).await, tier(&flow, rerun).await);
    assert_eq!(cheap, of.tier);
    assert!(stronger > cheap, "{stronger:?} after {cheap:?}");
    assert!(rerun.floor > cheap, "{:?}", rerun.floor);
    assert!(
        rerun.route.reason.starts_with("Rerun one tier up from "),
        "{}",
        rerun.route.reason
    );
    let explained = rerun.route.explanation.as_ref().unwrap().rerun.as_deref();
    assert_eq!(explained, Some(of));
    let thread = thread.lock().unwrap().join("\n---\n");
    let told = format!(
        "[rerun task-1] task-1 ran on {} {} ({}) and its check \"[not met] the file and line are named: no file defines it\" failed, so Brigadier reruns it once, one tier up, as task-2 on ",
        of.provider.label(),
        of.model,
        brigadier_router::tier_name(cheap)
    );
    assert!(thread.contains(&told), "{thread}");
    assert!(thread.contains("use it, ignore it or stop it"), "{thread}");
    assert!(!thread.contains("[rerun task-2]"), "{thread}");
    // The rerun's outcome names the cheaper model and why.
    flow.settled().await;
    let store = flow.manager.runtime.routing_store().cloned().unwrap();
    let outcomes = store.outcomes(None).await.unwrap();
    let outcome = outcomes
        .iter()
        .find(|outcome| outcome.task_id == rerun.id.to_string())
        .expect("the rerun's outcome");
    assert_eq!(outcome.rerun.as_ref(), Some(of));
    let cheap_outcome = outcomes
        .iter()
        .find(|outcome| outcome.task_id == first.id.to_string())
        .expect("the scout's outcome");
    assert_eq!(cheap_outcome.rerun, None);
    flow.stop().await;
}

/// A [met] line whose citation doesn't hold is marked for the thread and reruns nothing; nor
/// does a [not checked] line.
#[tokio::test]
async fn a_broken_citation_is_no_reason_to_rerun() {
    let thread: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = thread.clone();
    let flow = Flow::start(
        "rerun-citation",
        Options {
            files: vec![("src/parse.rs", PARSER)],
            ..cheap()
        },
        script(move |turn| {
            let seen = seen.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.starts_with("Find the parser") {
                        delegate_scout(&turn).await;
                        return Reply::text("[quiet]");
                    }
                    seen.lock().unwrap().push(turn.input.clone());
                    return Reply::text("It is in src/parse.rs.");
                }
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "The parser is in src/parse.rs.", "done_when": [
                            "[met] the file and line are named: src/parse.rs:1-1 `fn read_tokens`",
                            "[not checked] its callers are listed",
                        ]}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Find the parser.").await;
    let board = flow.until("the scout to end", ended).await;
    assert_eq!(board.tasks.len(), 1, "no rerun: {:#?}", board.tasks);
    let thread = thread.lock().unwrap().join("\n---\n");
    assert!(
        thread.contains("src/parse.rs:1-1 `fn read_tokens` [unevidenced]"),
        "{thread}"
    );
    assert!(!thread.contains("[rerun"), "{thread}");
    flow.settled().await;
    flow.stop().await;
}

/// The thread judges a scout's answer unsupported: `rerun_task` reruns it once, one tier up,
/// with the thread's words as the reason. Asked again, or for the rerun, it says why not and
/// starts nothing.
#[tokio::test]
async fn the_thread_reruns_an_unsupported_answer_once() {
    let replies: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = replies.clone();
    let flow = Flow::start(
        "rerun-by-thread",
        Options {
            files: vec![("src/parse.rs", PARSER)],
            ..cheap()
        },
        script(move |turn| {
            let seen = seen.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.starts_with("Find the parser") {
                        delegate_scout(&turn).await;
                        return Reply::text("[quiet]");
                    }
                    if turn.input.contains("[report task-1 ") {
                        for (task, why) in [
                            ("task-1", "It names a file but no caller"),
                            ("task-1", "Still unsure"),
                            ("task-2", "Unsure"),
                        ] {
                            let reply = turn
                                .call("rerun_task", json!({"task": task, "why": why}))
                                .await;
                            assert!(!reply.is_error, "{}", reply.text);
                            seen.lock().unwrap().push(reply.text);
                        }
                        // A [met] report reruns nothing by itself.
                        assert!(!turn.input.contains("[rerun task-1]"), "{}", turn.input);
                        return Reply::text("[quiet]");
                    }
                    return Reply::text("It is in src/parse.rs.");
                }
                report(
                    &turn,
                    "[met] the file and line are named: src/parse.rs:1-1 `pub fn parse`",
                )
                .await;
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Find the parser.").await;
    let board = flow.until("both scouts to end", ended).await;
    assert_eq!(board.tasks.len(), 2, "{:#?}", board.tasks);
    let rerun = Flow::task(&board, 2);
    let of = rerun.rerun.as_ref().expect("task-2 reruns task-1");
    assert_eq!(
        of.reason,
        "you judged that its answer lacks support: It names a file but no caller"
    );
    let replies = replies.lock().unwrap().clone();
    assert_eq!(replies.len(), 3, "{replies:#?}");
    assert!(
        replies[0].starts_with("[rerun task-1] task-1 ran on ")
            && replies[0].contains("and you judged that its answer lacks support: It names a file but no caller, so Brigadier reruns it once, one tier up, as task-2"),
        "{}",
        replies[0]
    );
    assert!(
        replies[1].starts_with("task-1 already reran once, as task-2"),
        "{}",
        replies[1]
    );
    assert!(
        replies[2].starts_with("task-2 is already a rerun one tier up"),
        "{}",
        replies[2]
    );
    flow.settled().await;
    flow.stop().await;
}
