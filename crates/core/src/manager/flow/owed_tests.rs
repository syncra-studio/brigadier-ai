//! A request isn't over while Brigadier still owes it a turn (w43 bug 2): a CI watch with no
//! result, the user's merge held for a landing's review, a review its thread left the answer
//! to. It works on between turns and is done once, at its real ending; a stop or a failure
//! still ends it, and the user's Stop ends the watch.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_providers::{ProviderEvent, Role};
use serde_json::json;

use super::thread_tests::{commit_in_workspace, reviewed_notes};
use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::conversation::Envelope;
use crate::manager::landing::pending_merge;
use crate::model::DomainEvent;
use crate::work::{InjectionKind, OrchestratorStepKind, RequestState, ReviewState, RunState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// Waits for `gate` to be set, as a slow worker or reviewer does.
async fn until_set(gate: &AtomicBool) {
    for _ in 0..3000 {
        if gate.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// How many times `request` became done, from its stored updates.
async fn times_done(flow: &Flow, request: &str) -> usize {
    let mut done = false;
    let mut times = 0;
    for event in flow.events().await {
        if let DomainEvent::RequestUpdated { request: updated } = event
            && updated.id == request
        {
            let now = updated.state == RequestState::Done;
            if now && !done {
                times += 1;
            }
            done = now;
        }
    }
    times
}

/// The request's state now, and still a moment later: nothing settles it otherwise meanwhile.
async fn stays(flow: &Flow, request: &str) -> RequestState {
    let before = flow.board().await.requests[request].state.clone();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let after = flow.board().await.requests[request].state.clone();
    assert_eq!(before, after, "it changed meanwhile");
    after
}

/// "Merge when done" with one writer: the thread lands it after a line mid-way ("Diff looks
/// right.") and ends its turn on [quiet]. The merge is due once the writer landed, while the
/// landing's review still runs (#242); the request is done once, after the review ends clean.
#[tokio::test]
async fn a_held_merge_is_due_while_the_review_runs_and_its_request_is_done_once() {
    let (write, review) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let (writing, reviewing, said) = (write.clone(), review.clone(), log.clone());
    let flow = Flow::start(
        "owed-held-merge",
        Options {
            reviews: Some(script(move |_| {
                let gate = reviewing.clone();
                async move {
                    until_set(&gate).await;
                    Reply::text("No findings.")
                }
            })),
            ..Options::default()
        },
        script(move |turn| {
            let (gate, said) = (writing.clone(), said.clone());
            async move {
                if turn.is_review() {
                    return Reply::text("No findings.");
                }
                if !turn.is_orchestrator() {
                    until_set(&gate).await;
                    turn.write("A.md", "note\n");
                    turn.git(&["add", "A.md"]);
                    turn.git(&["commit", "-qm", "Write A.md"]);
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "Wrote A.md.", "changes": ["A.md"]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                said.lock().unwrap().push(turn.input.clone());
                if turn.input.starts_with("Add a note") {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Write note A", "kind": "implement",
                                   "spec": "Write the note.", "provider": "claude"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    let reply = turn
                        .call(
                            "finish_session",
                            json!({"user_words": "merge it when done", "when": "workers_finish"}),
                        )
                        .await;
                    assert!(reply.text.contains("[merge held]"), "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[report task-1") {
                    turn.emit(ProviderEvent::Message {
                        item_id: uuid::Uuid::new_v4().to_string(),
                        role: Role::Assistant,
                        text: "Diff looks right.".into(),
                    })
                    .await;
                    let reply = turn.call("land_phase", json!({"task": "task-1"})).await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[merge due]") {
                    let reply = turn.call("finish_session", json!({})).await;
                    assert!(reply.text.contains("[finished]"), "{}", reply.text);
                    return Reply::text("Merged the note into main.");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("Add a note, and merge it when done.").await;
    flow.until("the merge to be held", |board| {
        pending_merge(board).is_some()
    })
    .await;
    write.store(true, Ordering::SeqCst);
    // The landing's review doesn't hold the merge (#242): it is due once the writer landed.
    let board = flow
        .until("the merge, while the landing's review runs", |board| {
            board
                .orchestrator_steps
                .iter()
                .any(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
                && board
                    .reviews
                    .values()
                    .any(|review| review.state == ReviewState::Running)
        })
        .await;
    let request = board.latest_request().unwrap().id.clone();
    assert!(
        log.lock()
            .unwrap()
            .iter()
            .any(|input| input.contains("[report task-1")),
        "landed on its report"
    );
    review.store(true, Ordering::SeqCst);
    let board = flow.settled().await;
    assert_eq!(board.requests[&request].state, RequestState::Done);
    assert_eq!(times_done(&flow, &request).await, 1, "done once");
    flow.stop().await;
}

/// A session that pushes and watches CI: its turn waits at `push` while the test files the
/// watch's row for it (as `watch_ci` does), then ends on [quiet]. The watch's result turn
/// answers in one line.
async fn watching(name: &str, ends: Reply) -> (Flow, String) {
    let push = Arc::new(tokio::sync::Notify::new());
    let gate = push.clone();
    let ends = Arc::new(Mutex::new(Some(ends)));
    let flow = Flow::start(
        name,
        Options::default(),
        script(move |turn| {
            let (gate, ends) = (gate.clone(), ends.clone());
            async move {
                if turn.input.contains("[ci 97c298d5 · passed") {
                    return Reply::text("CI passed on `main`.");
                }
                if turn.input.contains("Push main") {
                    gate.notified().await;
                    return ends.lock().unwrap().take().unwrap();
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("Push main to GitHub.").await;
    let board = flow
        .until("the push's turn", |board| board.run == RunState::Running)
        .await;
    let request = board.latest_request().unwrap().id.clone();
    flow.manager
        .orchestrator_step(&flow.conversation, ci_row(None))
        .await;
    push.notify_one();
    flow.until("the push's turn to end", |board| {
        !matches!(board.run, RunState::Running | RunState::Starting)
    })
    .await;
    (flow, request)
}

fn ci_row(result: Option<&str>) -> OrchestratorStepKind {
    OrchestratorStepKind::CiWatch {
        repo: "o/n".into(),
        branch: "main".into(),
        sha: "97c298d5aaaa".into(),
        waiting: result.is_none(),
        runs: Vec::new(),
        result: result.map(str::to_owned),
    }
}

/// How the watch's row ended ("" while it checks).
async fn ci_result(flow: &Flow) -> Option<String> {
    let board = flow.board().await;
    board
        .orchestrator_steps
        .iter()
        .find_map(|step| match &step.kind {
            OrchestratorStepKind::CiWatch { result, .. } => {
                Some(result.clone().unwrap_or_default())
            }
            _ => None,
        })
}

/// Push, watch, a turn that ends on [quiet] (w43 B1): the request works until the watch's
/// result turn answers, then is done once, with that answer.
#[tokio::test]
async fn a_ci_watch_keeps_its_request_working_until_its_result_turn() {
    let (flow, request) = watching("owed-ci", Reply::text("[quiet]")).await;
    assert_eq!(stays(&flow, &request).await, RequestState::Working);
    assert_eq!(times_done(&flow, &request).await, 0);
    // The watch ends as `watch_ci`'s does: its row's result, then the message.
    flow.manager
        .orchestrator_step(&flow.conversation, ci_row(Some("passed")))
        .await;
    flow.manager
        .deliver_for(
            &flow.conversation,
            Envelope {
                kind: InjectionKind::Report,
                label: "ci 97c298d5".into(),
                task_id: None,
                text: "[ci 97c298d5 · passed · 1 job]".into(),
            },
            Some(request.clone()),
        )
        .await;
    let board = flow.settled().await;
    assert_eq!(board.requests[&request].state, RequestState::Done);
    assert_eq!(times_done(&flow, &request).await, 1, "done once");
    flow.stop().await;
}

/// The user's Stop while only the CI watch keeps the request working: it ends Stopped, the
/// watch's task ends and its row reads "stopped", and nothing wakes the thread for it later.
#[tokio::test]
async fn stop_during_a_ci_watch_stops_the_request_and_ends_the_watch() {
    let (flow, request) = watching("owed-ci-stop", Reply::text("[quiet]")).await;
    assert_eq!(stays(&flow, &request).await, RequestState::Working);
    // The watch's own task, as `watch_ci` keeps it.
    let task = tokio::spawn(tokio::time::sleep(Duration::from_secs(600)));
    flow.manager
        .conv(&flow.conversation)
        .unwrap()
        .ci_watches
        .lock()
        .unwrap()
        .push(task.abort_handle());
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    let board = flow
        .until("the request stopped", |board| {
            board.requests[&request].state == RequestState::Stopped
        })
        .await;
    assert!(task.await.unwrap_err().is_cancelled(), "the watch ended");
    assert_eq!(ci_result(&flow).await.as_deref(), Some("stopped"));
    assert_ne!(board.run, RunState::Running);
    assert_eq!(stays(&flow, &request).await, RequestState::Stopped);
    assert_eq!(times_done(&flow, &request).await, 0);
    flow.stop().await;
}

/// A turn that fails while the CI watch runs: the request reads failed, not working; what
/// the watch brings later works it again as any late event does.
#[tokio::test]
async fn a_failed_turn_stays_failed_while_its_ci_watch_runs() {
    let (flow, request) = watching("owed-ci-failed", Reply::failed("API error 500")).await;
    // The turn's end settles it a moment later (a tab's ready check reads git first).
    flow.until("the request to end", |board| {
        board.requests[&request].state != RequestState::Working
    })
    .await;
    let state = stays(&flow, &request).await;
    assert!(matches!(state, RequestState::Failed { .. }), "{state:?}");
    assert_eq!(ci_result(&flow).await.as_deref(), Some(""), "still watched");
    flow.stop().await;
}

/// The thread commits and ends its turn on [quiet]: the review of its commit, filed under
/// the request, keeps it working until it ends clean, and it is done once.
#[tokio::test]
async fn a_review_of_the_threads_commit_keeps_a_quiet_request_working_until_it_ends() {
    let review = Arc::new(AtomicBool::new(false));
    let gate = review.clone();
    let flow = Flow::start(
        "owed-thread-review",
        Options {
            reviews: Some(script(move |_| {
                let gate = gate.clone();
                async move {
                    until_set(&gate).await;
                    Reply::text("No findings.")
                }
            })),
            ..Options::default()
        },
        script(|turn| async move {
            if turn.input.contains("Add notes.") {
                commit_in_workspace(&turn, "notes.txt", &reviewed_notes(), "Add notes");
                return Reply::text("[quiet]");
            }
            if turn.input.starts_with("[checks passed") {
                // The test repo has no checks, so they pass at once: the thread still leaves
                // its answer to the review (answering here would make the review a late event).
                return Reply::text("[quiet]");
            }
            Reply::text("Noted.")
        }),
    )
    .await;
    flow.say("Add notes.").await;
    let board = flow
        .until("the review running, the thread idle", |board| {
            board.run == RunState::Idle
                && board
                    .reviews
                    .values()
                    .any(|review| review.state == ReviewState::Running)
        })
        .await;
    let request = board.latest_request().unwrap().id.clone();
    let linked = board.reviews.values().next().unwrap();
    assert!(linked.task_id.is_none(), "the thread's own commit");
    assert_eq!(linked.request_id.as_deref(), Some(request.as_str()));
    assert_eq!(stays(&flow, &request).await, RequestState::Working);
    review.store(true, Ordering::SeqCst);
    flow.until("the review to end", |board| {
        board
            .reviews
            .values()
            .all(|review| review.state == ReviewState::Clean)
    })
    .await;
    let board = flow.settled().await;
    assert_eq!(board.requests[&request].state, RequestState::Done);
    assert_eq!(times_done(&flow, &request).await, 1, "done once");
    flow.stop().await;
}

/// The thread commits and answers; the review of its commit runs on, and keeps the request
/// working until it ends (BUILD-PLAN B2). A late addendum for the request then gets a [quiet]
/// turn while that review still runs: the request was answered already, so that turn defers
/// nothing, and the clean review ends it done without waking the thread again.
#[tokio::test]
async fn a_quiet_late_turn_leaves_an_answered_request_answered_while_its_review_runs() {
    let (review, late) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let (gate, heard, log) = (review.clone(), late.clone(), inputs.clone());
    let flow = Flow::start(
        "owed-late-quiet",
        Options {
            reviews: Some(script(move |_| {
                let gate = gate.clone();
                async move {
                    until_set(&gate).await;
                    Reply::text("No findings.")
                }
            })),
            ..Options::default()
        },
        script(move |turn| {
            let (heard, log) = (heard.clone(), log.clone());
            async move {
                log.lock().unwrap().push(turn.input.clone());
                if turn.input.contains("Add notes.") {
                    commit_in_workspace(&turn, "notes.txt", &reviewed_notes(), "Add notes");
                    return Reply::text("Added the notes.");
                }
                if turn.input.contains("late addendum") {
                    heard.store(true, Ordering::SeqCst);
                    return Reply::text("[quiet]");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("Add notes.").await;
    let board = flow
        .until("the review running, the thread idle", |board| {
            board.run == RunState::Idle
                && board
                    .reviews
                    .values()
                    .any(|review| review.state == ReviewState::Running)
        })
        .await;
    let request = board.latest_request().unwrap().id.clone();
    assert_eq!(
        stays(&flow, &request).await,
        RequestState::Working,
        "its review runs"
    );
    flow.manager
        .deliver_for(
            &flow.conversation,
            Envelope {
                kind: InjectionKind::Report,
                label: "addendum".into(),
                task_id: None,
                text: "[late addendum · nothing to change]".into(),
            },
            Some(request.clone()),
        )
        .await;
    until_set(&late).await;
    flow.until("the late turn over", |board| board.run == RunState::Idle)
        .await;
    assert!(
        flow.board()
            .await
            .reviews
            .values()
            .any(|review| review.state == ReviewState::Running),
        "the review still runs"
    );
    assert_eq!(stays(&flow, &request).await, RequestState::Working);
    review.store(true, Ordering::SeqCst);
    let board = flow.settled().await;
    assert_eq!(board.requests[&request].state, RequestState::Done);
    assert!(
        inputs
            .lock()
            .unwrap()
            .iter()
            .all(|input| !input.contains("found nothing")),
        "an answered request isn't woken by its clean review"
    );
    flow.stop().await;
}

/// Regenerating restarts the same request: its earlier answer no longer settles a quiet
/// replacement turn while the review of its new commit is still running.
#[tokio::test]
async fn a_regenerated_quiet_turn_waits_for_its_review_despite_the_earlier_answer() {
    let review = Arc::new(AtomicBool::new(false));
    let gate = review.clone();
    let answered = Arc::new(AtomicBool::new(false));
    let flow = Flow::start(
        "20261012-2245-review-fixes-w04-regenerated-quiet",
        Options {
            reviews: Some(script(move |_| {
                let gate = gate.clone();
                async move {
                    until_set(&gate).await;
                    Reply::text("No findings.")
                }
            })),
            ..Options::default()
        },
        script(move |turn| {
            let answered = answered.clone();
            async move {
                if turn.input.contains("Add notes.") {
                    if !answered.swap(true, Ordering::SeqCst) {
                        return Reply::text("Ready to add the notes.");
                    }
                    commit_in_workspace(&turn, "notes.txt", &reviewed_notes(), "Add notes");
                    return Reply::text("[quiet]");
                }
                if turn.input.starts_with("[checks passed") {
                    // As in the review test above: the answer is still left to the review.
                    return Reply::text("[quiet]");
                }
                Reply::text("Added and reviewed the notes.")
            }
        }),
    )
    .await;
    flow.say("Add notes.").await;
    let board = flow.settled().await;
    let request = board.latest_request().unwrap().id.clone();
    assert_eq!(board.requests[&request].state, RequestState::Done);
    flow.manager
        .regenerate(flow.conversation.clone(), request.clone())
        .await
        .unwrap();
    flow.until(
        "the new review running, the regenerated turn idle",
        |board| {
            board.run == RunState::Idle
                && board
                    .reviews
                    .values()
                    .any(|review| review.state == ReviewState::Running)
        },
    )
    .await;
    assert_eq!(stays(&flow, &request).await, RequestState::Working);
    assert_eq!(
        times_done(&flow, &request).await,
        1,
        "only the earlier answer was done"
    );
    review.store(true, Ordering::SeqCst);
    let board = flow.settled().await;
    assert_eq!(board.requests[&request].state, RequestState::Done);
    assert_eq!(times_done(&flow, &request).await, 2);
    flow.stop().await;
}
