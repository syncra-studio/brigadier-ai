//! The thread's commands that outlast a tool call (`run`, `run_check`): past
//! [`FOREGROUND`](crate::manager::background_runs::FOREGROUND) they go to the background, the
//! thread may end its turn quiet, and the result still comes to it as a message in the same
//! request, which works until then and settles on the thread's answer to it. The waits of a
//! check go too; the user's Stop ends a run quietly; a restart that cut one off tells the
//! thread.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::model::{DomainEvent, MessageRole};
use crate::work::{BackgroundRun, BackgroundRunState, RequestState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

type Log = Arc<Mutex<Vec<String>>>;

/// A thread that, asked to check, runs `calls` (tool, command) and ends its turn quiet; a turn
/// that hears a run finished answers what it read. Every turn's input and every tool reply go
/// to the logs.
fn checking(calls: Vec<(&'static str, &'static str)>, inputs: Log, replies: Log) -> Script {
    script(move |turn| {
        let (calls, inputs, replies) = (calls.clone(), inputs.clone(), replies.clone());
        async move {
            inputs.lock().unwrap().push(turn.input.clone());
            if turn.input.contains("finished · ") || turn.input.contains("cut off") {
                return Reply::text("The check passed.");
            }
            for (tool, command) in calls {
                let reply = turn.call(tool, json!({ "command": command })).await;
                assert!(!reply.is_error, "{tool}: {}", reply.text);
                replies.lock().unwrap().push(reply.text);
            }
            Reply::text(crate::manager::prompts::QUIET)
        }
    })
}

fn run_state(board: &crate::board::Board, id: &str) -> Option<BackgroundRunState> {
    board.background_runs.get(id).map(|run| run.state.clone())
}

async fn answers(flow: &Flow) -> Vec<String> {
    flow.core
        .conversation_view(flow.conversation.clone(), 50)
        .await
        .unwrap()
        .messages
        .messages
        .into_iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .map(|message| message.text)
        .collect()
}

/// The 33fbb16e case: the thread starts a long check, ends its turn quiet, and the check's
/// result still reaches it in the same request, which works until the thread answered it.
#[tokio::test]
async fn a_long_check_goes_to_the_background_and_its_result_comes_back_to_the_thread() {
    let (inputs, replies): (Log, Log) = Default::default();
    let flow = Flow::start(
        "background-check",
        Options::default(),
        checking(
            vec![("run_check", "sleep 4; echo done")],
            inputs.clone(),
            replies.clone(),
        ),
    )
    .await;
    flow.background_commands();
    flow.say("Run the long check.").await;
    // The board can show the run before the thread's script has kept the call's reply.
    let board = flow
        .until("the check to go to the background", |board| {
            run_state(board, "run-1") == Some(BackgroundRunState::Running)
                && !replies.lock().unwrap().is_empty()
        })
        .await;
    // The turn ended quiet, yet the request works while its run does.
    assert!(
        board
            .requests
            .values()
            .all(|request| request.state == RequestState::Working),
        "{:?}",
        board.requests
    );
    let replies_now = replies.lock().unwrap().clone();
    assert!(
        replies_now[0].starts_with("[run-1 still runs"),
        "{replies_now:?}"
    );

    let board = flow.settled().await;
    assert_eq!(
        run_state(&board, "run-1"),
        Some(BackgroundRunState::Finished {
            status: "exit 0".into()
        })
    );
    assert!(
        board
            .requests
            .values()
            .all(|request| request.state == RequestState::Done),
        "{:?}",
        board.requests
    );
    let inputs = inputs.lock().unwrap().clone();
    let heard = inputs
        .iter()
        .find(|input| input.contains("[run-1 finished · run_check `sleep 4; echo done`]"))
        .unwrap_or_else(|| panic!("no turn heard the result: {inputs:#?}"));
    assert!(heard.contains("done"), "{heard}");
    let answers = answers(&flow).await;
    assert_eq!(
        answers.last().map(String::as_str),
        Some("The check passed."),
        "{answers:?}"
    );
    flow.stop().await;
}

/// A `run` gone to the background tells its result the same way.
#[tokio::test]
async fn a_long_command_goes_to_the_background_and_its_result_comes_back_to_the_thread() {
    let (inputs, replies): (Log, Log) = Default::default();
    let flow = Flow::start(
        "background-run",
        Options::default(),
        checking(
            vec![("run", "sleep 3; echo ran")],
            inputs.clone(),
            replies.clone(),
        ),
    )
    .await;
    flow.background_commands();
    flow.say("Run it.").await;
    let board = flow.settled().await;
    assert_eq!(
        run_state(&board, "run-1"),
        Some(BackgroundRunState::Finished {
            status: "exit 0".into()
        })
    );
    let inputs = inputs.lock().unwrap().clone();
    assert!(
        inputs.iter().any(
            |input| input.contains("[run-1 finished · run `sleep 3; echo ran`]")
                && input.contains("ran")
        ),
        "{inputs:#?}"
    );
    flow.stop().await;
}

/// A long command's short name (what it is for) is kept on its background run, for the
/// session's Activities; a blank one is none, and a long one is cut.
#[tokio::test]
async fn a_background_run_keeps_the_name_the_thread_gave_it() {
    let flow = Flow::start(
        "background-names",
        Options::default(),
        script(|turn| async move {
            if turn.input.contains("finished · ") {
                return Reply::text("Done.");
            }
            for (tool, name) in [
                ("run_check", "  Core tests "),
                ("run", " "),
                (
                    "run",
                    "The whole desktop app's debug build, bundled for macOS",
                ),
            ] {
                let reply = turn
                    .call(
                        tool,
                        json!({ "command": "sleep 3; echo ran", "name": name }),
                    )
                    .await;
                assert!(!reply.is_error, "{tool}: {}", reply.text);
            }
            Reply::text(crate::manager::prompts::QUIET)
        }),
    )
    .await;
    flow.background_commands();
    flow.say("Run them.").await;
    let board = flow.settled().await;
    let names: Vec<_> = ["run-1", "run-2", "run-3"]
        .iter()
        .map(|id| board.background_runs[*id].name.clone())
        .collect();
    assert_eq!(
        names,
        [
            Some("Core tests".to_owned()),
            None,
            Some("The whole desktop app's debug build, bu…".to_owned()),
        ]
    );
    flow.stop().await;
}

/// The same check asked again while the first still runs waits for it in the background too,
/// and answers from what the first kept.
#[tokio::test]
async fn a_check_waiting_for_the_same_check_goes_to_the_background_and_answers_from_its_result() {
    let (inputs, replies): (Log, Log) = Default::default();
    let gate = super::Scratch::new("background-same-check-release");
    let started = gate.join("started");
    let marker = gate.join("release");
    let command = format!(
        "touch '{}'; while [ ! -f '{}' ]; do sleep 0.05; done; echo once",
        started.display().to_string().replace('\'', "'\\''"),
        marker.display().to_string().replace('\'', "'\\''")
    );
    let flow = Flow::start(
        "background-same-check",
        Options::default(),
        script({
            let (inputs, replies) = (inputs.clone(), replies.clone());
            move |turn| {
                let (inputs, replies, command, started) = (
                    inputs.clone(),
                    replies.clone(),
                    command.clone(),
                    started.clone(),
                );
                async move {
                    inputs.lock().unwrap().push(turn.input.clone());
                    if turn.input.contains("finished · ") {
                        return Reply::text("The check passed.");
                    }
                    for _ in 0..2 {
                        let reply = turn.call("run_check", json!({ "command": command })).await;
                        assert!(!reply.is_error, "run_check: {}", reply.text);
                        replies.lock().unwrap().push(reply.text);
                        // Preparing a check can outlast its foreground call. Ensure the
                        // first owns the check before the second starts preparing it.
                        super::eventually("the first check's command to start", || {
                            started.exists()
                        })
                        .await;
                    }
                    Reply::text(crate::manager::prompts::QUIET)
                }
            }
        }),
    )
    .await;
    flow.background_commands();
    flow.say("Check twice.").await;
    // Keep the first check running until both calls have gone to the background, even if
    // the test process stalls while its child keeps running.
    flow.until("both checks to go to the background", |_| {
        replies.lock().unwrap().len() == 2
    })
    .await;
    std::fs::write(&marker, "").unwrap();
    let board = flow.settled().await;
    let replies = replies.lock().unwrap().clone();
    assert!(replies[0].starts_with("[run-1 still runs"), "{replies:?}");
    assert!(replies[1].starts_with("[run-2 still runs"), "{replies:?}");
    for run in ["run-1", "run-2"] {
        assert!(
            matches!(
                run_state(&board, run),
                Some(BackgroundRunState::Finished { .. })
            ),
            "{run}: {:?}",
            board.background_runs
        );
    }
    let inputs = inputs.lock().unwrap().join("\n");
    let second = inputs
        .split("[run-2 finished")
        .nth(1)
        .unwrap_or_else(|| panic!("run-2's result never came: {inputs}"));
    assert!(second.contains("[cached:"), "{second}");
    flow.stop().await;
}

/// The user's Stop of a check waiting in the background for the same check ends it at once,
/// not when the check it waits for ends.
#[tokio::test]
async fn the_users_stop_ends_a_check_waiting_for_the_same_check() {
    let (inputs, replies): (Log, Log) = Default::default();
    let flow = Flow::start(
        "background-same-check-stop",
        Options::default(),
        checking(
            vec![
                ("run_check", "sleep 30; echo once"),
                ("run_check", "sleep 30; echo once"),
            ],
            inputs.clone(),
            replies.clone(),
        ),
    )
    .await;
    flow.background_commands();
    flow.say("Check twice.").await;
    flow.until("the second check to wait in the background", |board| {
        run_state(board, "run-2") == Some(BackgroundRunState::Running)
    })
    .await;
    flow.manager
        .stop_background_run(flow.conversation.clone(), "run-2".into())
        .await
        .unwrap();
    let board = flow
        .until("the waiting check to stop", |board| {
            run_state(board, "run-2") == Some(BackgroundRunState::Stopped)
        })
        .await;
    assert_eq!(
        run_state(&board, "run-1"),
        Some(BackgroundRunState::Running),
        "the waiting check stopped only once the first ended"
    );
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    flow.stop().await;
}

/// A heavy check waiting for another build's lease past the foreground goes to the
/// background, and runs once the lease is free.
#[tokio::test]
async fn a_check_waiting_for_the_build_lease_goes_to_the_background() {
    let (inputs, replies): (Log, Log) = Default::default();
    let flow = Flow::start(
        "background-lease",
        Options::default(),
        checking(
            // Heavy by its words; the function stands in for the build tool.
            vec![("run_check", "make() { echo built; }; make all")],
            inputs.clone(),
            replies.clone(),
        ),
    )
    .await;
    flow.background_commands();
    let (held, now, _) = flow.manager.machine.ask_check("another-build", "make all");
    assert!(now, "the lease was free");
    flow.say("Build it.").await;
    flow.until("the check to go to the background", |board| {
        run_state(board, "run-1") == Some(BackgroundRunState::Running)
    })
    .await;
    flow.manager.machine.end_check(held);
    let board = flow.settled().await;
    assert_eq!(
        run_state(&board, "run-1"),
        Some(BackgroundRunState::Finished {
            status: "exit 0".into()
        })
    );
    let inputs = inputs.lock().unwrap().join("\n");
    assert!(
        inputs.contains("[run-1 finished") && inputs.contains("built"),
        "{inputs}"
    );
    flow.stop().await;
}

/// #245: the thread's heavy check doesn't queue behind the full checks holding the lease: it
/// runs at once, in its call, and its reply says what the full checks run and the machine load.
#[tokio::test]
async fn the_threads_check_runs_beside_the_full_checks() {
    let (inputs, replies): (Log, Log) = Default::default();
    let flow = Flow::start(
        "background-beside",
        Options::default(),
        checking(
            vec![("run_check", "make() { echo built; }; make all")],
            inputs.clone(),
            replies.clone(),
        ),
    )
    .await;
    flow.background_commands();
    let full = format!("checks:{}", flow.conversation);
    let (held, now, _) = flow
        .manager
        .machine
        .ask_check(&full, "cargo test --workspace");
    assert!(now, "the lease was free");
    flow.say("Build it.").await;
    let board = flow.settled().await;
    flow.manager.machine.end_check(held);
    assert!(
        board.background_runs.is_empty(),
        "{:?}",
        board.background_runs
    );
    let replies = replies.lock().unwrap().join("\n");
    assert!(
        replies.contains("built")
            && replies
                .contains("[beside the full checks] The full checks run `cargo test --workspace`"),
        "{replies}"
    );
    flow.stop().await;
}

/// The user's Stop ends a background run: recorded stopped, and the thread hears nothing of it.
#[tokio::test]
async fn the_users_stop_ends_a_background_run_quietly() {
    let (inputs, replies): (Log, Log) = Default::default();
    let flow = Flow::start(
        "background-stop",
        Options::default(),
        checking(
            vec![("run", "sleep 60; echo late")],
            inputs.clone(),
            replies.clone(),
        ),
    )
    .await;
    flow.background_commands();
    flow.say("Run the long one.").await;
    flow.until("the run to go to the background", |board| {
        run_state(board, "run-1") == Some(BackgroundRunState::Running)
    })
    .await;
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    let board = flow
        .until("the run to stop", |board| {
            run_state(board, "run-1") == Some(BackgroundRunState::Stopped)
        })
        .await;
    let board = if board
        .requests
        .values()
        .any(|request| request.state == RequestState::Working)
    {
        flow.settled().await
    } else {
        board
    };
    assert!(
        board
            .requests
            .values()
            .all(|request| request.state == RequestState::Stopped),
        "{:?}",
        board.requests
    );
    let inputs = inputs.lock().unwrap().clone();
    assert!(
        inputs.iter().all(|input| !input.contains("[run-1")),
        "{inputs:#?}"
    );
    flow.stop().await;
}

/// A tool call dropped while its command runs (its MCP request cancelled, its bridge gone)
/// ends the command rather than leaving it running with no one to hear its result.
#[tokio::test]
async fn a_dropped_call_ends_its_command() {
    let flow = Flow::start(
        "background-dropped",
        Options::default(),
        script(|_| async { Reply::text("Looked.") }),
    )
    .await;
    let cancel = tokio_util::sync::CancellationToken::new();
    let ended = cancel.clone();
    let seen = cancel.clone();
    let call = flow.manager.in_foreground(
        &flow.conversation,
        ("run", None),
        "sleep 600",
        tokio_util::sync::CancellationToken::new(),
        cancel,
        async move {
            seen.cancelled().await;
            Ok(("[stopped]".to_owned(), String::new()))
        },
    );
    let held = tokio::time::timeout(std::time::Duration::from_millis(300), call).await;
    assert!(held.is_err(), "the call returned while its command ran");
    tokio::time::timeout(std::time::Duration::from_secs(5), ended.cancelled())
        .await
        .expect("the dropped call's command was not ended");
    flow.stop().await;
}

/// The context card's Stop ends one background run: recorded stopped, and the thread hears it
/// in the run's request and answers, so the user isn't left with a silent end.
#[tokio::test]
async fn the_users_stop_of_one_background_run_tells_the_thread() {
    let inputs: Log = Default::default();
    let log = inputs.clone();
    let flow = Flow::start(
        "background-stop-one",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(turn.input.clone());
                if turn.input.contains("The user stopped run-1") {
                    return Reply::text("Stopped the long run; it didn't finish.");
                }
                let reply = turn
                    .call("run", json!({ "command": "sleep 60; echo late" }))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text(crate::manager::prompts::QUIET)
            }
        }),
    )
    .await;
    flow.background_commands();
    flow.say("Run the long one.").await;
    flow.until("the run to go to the background", |board| {
        run_state(board, "run-1") == Some(BackgroundRunState::Running)
    })
    .await;
    flow.manager
        .stop_background_run(flow.conversation.clone(), "run-1".into())
        .await
        .unwrap();
    let board = flow
        .until("the request to settle on the thread's answer", |board| {
            run_state(board, "run-1") == Some(BackgroundRunState::Stopped)
                && board
                    .requests
                    .values()
                    .all(|request| request.state == RequestState::Done)
        })
        .await;
    assert_eq!(board.requests.len(), 1, "{:?}", board.requests);
    // Its time counts from the command's start, its foreground part too (#89).
    let run = &board.background_runs["run-1"];
    let ran = run.ended_at_ms.unwrap() - run.started_at_ms;
    let foreground = crate::manager::background_runs::FOREGROUND.as_millis() as i64;
    assert!(ran >= foreground, "{ran} ms");
    assert!(
        answers(&flow)
            .await
            .iter()
            .any(|text| text == "Stopped the long run; it didn't finish."),
        "{:?}",
        answers(&flow).await
    );
    // A run already over can't be stopped.
    assert!(
        flow.manager
            .stop_background_run(flow.conversation.clone(), "run-1".into())
            .await
            .is_err()
    );
    let inputs = inputs.lock().unwrap().clone();
    assert!(
        inputs
            .iter()
            .any(|input| input.contains("[The user stopped run-1 after ")
                && input.contains(
                    ", before it finished · run `sleep 60; echo late`: it printed nothing"
                )),
        "{inputs:#?}"
    );
    flow.stop().await;
}

/// A run the user stopped after it printed a long log: the thread hears that what it printed
/// is kept, with its end, never that nothing came back; one that printed nothing says so.
#[tokio::test]
async fn the_users_stop_of_a_run_that_printed_says_what_it_printed() {
    use brigadier_providers::{ItemStatus, ProviderEvent};
    let inputs: Log = Default::default();
    let log = inputs.clone();
    let command = "for i in $(seq 1 108); do echo log line $i; done; sleep 60";
    let flow = Flow::start(
        "background-stop-printed",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(turn.input.clone());
                if turn.input.contains("The user stopped run-1") {
                    return Reply::text("Stopped the log run.");
                }
                turn.emit(ProviderEvent::ToolCall {
                    kept: None,
                    item_id: "own-run".into(),
                    name: "mcp__brigadier__run".into(),
                    input: Some(json!({ "command": command }).to_string()),
                    status: ItemStatus::InProgress,
                    output: None,
                })
                .await;
                let id = turn.conversation().unwrap();
                tokio::time::timeout(*super::PATIENCE, async {
                    while !turn
                        .host
                        .core
                        .board(&id)
                        .await
                        .unwrap()
                        .orchestrator_steps
                        .iter()
                        .any(|step| matches!(&step.kind, crate::work::OrchestratorStepKind::Tool { item_id, .. } if item_id == "own-run"))
                    {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
                let reply = turn.call("run", json!({ "command": command })).await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text(crate::manager::prompts::QUIET)
            }
        }),
    )
    .await;
    flow.background_commands();
    flow.say("Run the log.").await;
    flow.until("the run to go to the background", |board| {
        run_state(board, "run-1") == Some(BackgroundRunState::Running)
    })
    .await;
    tokio::time::timeout(*super::PATIENCE, async {
        loop {
            let (text, _) = flow
                .manager
                .read_background_run(&flow.conversation, "run-1")
                .await
                .unwrap();
            if text.contains("log line 108") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    flow.manager
        .stop_background_run(flow.conversation.clone(), "run-1".into())
        .await
        .unwrap();
    flow.until("the thread's answer", |board| {
        run_state(board, "run-1") == Some(BackgroundRunState::Stopped)
            && board
                .requests
                .values()
                .all(|request| request.state == RequestState::Done)
    })
    .await;
    let inputs = inputs.lock().unwrap().clone();
    let told = inputs
        .iter()
        .find(|input| input.contains("The user stopped run-1"))
        .unwrap_or_else(|| panic!("{inputs:#?}"));
    assert!(
        told.contains("what it printed until then is kept"),
        "{told}"
    );
    assert!(told.contains("log line 108"), "{told}");
    assert!(!told.contains("it printed nothing"), "{told}");
    flow.stop().await;
}

/// A run the restart cut off is recorded so, and the thread hears it in the run's request.
#[tokio::test]
async fn a_background_run_cut_off_by_a_restart_tells_the_thread() {
    let (inputs, replies): (Log, Log) = Default::default();
    let mut flow = Flow::start(
        "background-restart",
        Options::default(),
        checking(Vec::new(), inputs.clone(), replies.clone()),
    )
    .await;
    flow.say("Start.").await;
    let board = flow.settled().await;
    let request = board.requests.keys().next().cloned();
    flow.core
        .record_conversation(
            &flow.conversation,
            vec![DomainEvent::BackgroundRunUpdated {
                run: BackgroundRun {
                    id: "run-1".into(),
                    conversation_id: flow.conversation.clone(),
                    request_id: request,
                    tool: "run_check".into(),
                    command: "cargo test".into(),
                    started_at_ms: crate::now_ms(),
                    ended_at_ms: None,
                    state: BackgroundRunState::Running,
                    item_id: None,
                    name: None,
                },
            }],
        )
        .await
        .unwrap();
    flow.restart().await;
    let board = flow
        .until("the run to be recorded cut off", |board| {
            matches!(
                run_state(board, "run-1"),
                Some(BackgroundRunState::Interrupted { .. })
            )
        })
        .await;
    assert!(board.background_runs["run-1"].ended_at_ms.is_some());
    super::eventually("the thread to hear it", || {
        inputs
            .lock()
            .unwrap()
            .iter()
            .any(|input| input.contains("[run-1 was cut off by a restart"))
    })
    .await;
    flow.settled().await;
    flow.stop().await;
}

/// The messages the user sees from the thread: text and the request each belongs to.
async fn answers_with_requests(flow: &Flow) -> Vec<(String, Option<String>)> {
    flow.core
        .conversation_view(flow.conversation.clone(), 50)
        .await
        .unwrap()
        .messages
        .messages
        .into_iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .map(|message| (message.text, message.request_id))
        .collect()
}

async fn rested(flow: &Flow) {
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    super::eventually_async("the thread's turn to end", || {
        let conv = conv.clone();
        async move { !conv.is_busy().await }
    })
    .await;
}

/// A Claude thread whose CLI keeps a job of its own after the turn (an older CLI, or a
/// session resumed from one): the session can't rest while it runs (resting would end it),
/// the request works on, and the turn the CLI starts on its own when the job ends is that
/// request's: its answer shows there, and the next message gets a turn and answer of its own.
#[tokio::test]
async fn a_cli_job_outliving_its_turn_keeps_the_cli_and_its_end_answers_in_its_request() {
    let go = Arc::new(tokio::sync::Notify::new());
    let release = go.clone();
    let flow = Flow::start(
        "cli-job",
        Options::default(),
        script(move |turn| {
            let go = go.clone();
            async move {
                if turn.input.contains("Reply with K") {
                    return Reply::text("K-1234");
                }
                let events = turn.events.clone();
                let _ = events
                    .send(brigadier_providers::ProviderEvent::BackgroundJobs {
                        running: vec!["job".into()],
                    })
                    .await;
                tokio::spawn(async move {
                    go.notified().await;
                    for event in [
                        brigadier_providers::ProviderEvent::BackgroundJobs {
                            running: Vec::new(),
                        },
                        brigadier_providers::ProviderEvent::TurnStarted { turn_id: None },
                        brigadier_providers::ProviderEvent::Message {
                            item_id: "job-answer".into(),
                            role: brigadier_providers::Role::Assistant,
                            text: "The job printed abc123.".into(),
                        },
                        brigadier_providers::ProviderEvent::TurnCompleted {
                            turn_id: None,
                            status: brigadier_providers::TurnStatus::Completed,
                            duration_ms: Some(1),
                            usage: None,
                        },
                    ] {
                        let _ = events.send(event).await;
                    }
                });
                Reply::text("Started it in the background.")
            }
        }),
    )
    .await;
    flow.say("Start the job.").await;
    rested(&flow).await;
    let board = flow.board().await;
    let root = board.requests.keys().next().unwrap().clone();
    assert_eq!(board.requests[&root].state, RequestState::Working);
    let refused = flow.manager.hibernate(flow.conversation.clone()).await;
    assert!(refused.is_err(), "{refused:?}");
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    assert!(conv.idle_cli().await.is_none(), "nothing retires the CLI");

    release.notify_one();
    let board = flow.settled().await;
    assert_eq!(board.requests[&root].state, RequestState::Done);
    let answers = answers_with_requests(&flow).await;
    assert!(
        answers.contains(&("The job printed abc123.".into(), Some(root.clone()))),
        "{answers:?}"
    );

    flow.say("Reply with K.").await;
    let board = flow
        .until("the second request to settle", |board| {
            board.requests.len() == 2
                && board
                    .requests
                    .values()
                    .all(|request| request.state == RequestState::Done)
        })
        .await;
    let second = board
        .requests
        .values()
        .find(|request| request.id != root)
        .unwrap()
        .id
        .clone();
    let answers = answers_with_requests(&flow).await;
    assert!(
        answers.contains(&("K-1234".into(), Some(second))),
        "{answers:?}"
    );
    assert!(
        answers.iter().all(|(_, request)| request.is_some()),
        "an answer outside any request: {answers:?}"
    );
    flow.stop().await;
}

/// A Codex thread whose shell command is still running when its turn ends: the command goes
/// on as its CLI's job, the request works, the CLI stays up, and the command's end comes to
/// the thread as a message in that request.
#[tokio::test]
async fn a_codex_command_outliving_its_turn_tells_the_thread_when_it_ends() {
    let go = Arc::new(tokio::sync::Notify::new());
    let release = go.clone();
    let inputs: Log = Log::default();
    let log = inputs.clone();
    let flow = Flow::start(
        "codex-job",
        Options {
            thread: brigadier_providers::ProviderKind::Codex,
            ..Options::default()
        },
        script(move |turn| {
            let (go, log) = (go.clone(), log.clone());
            async move {
                log.lock().unwrap().push(turn.input.clone());
                if turn.input.contains("finished after your turn") {
                    return Reply::text("It printed xyz.");
                }
                let events = turn.events.clone();
                let command = |status, exit_code, output: Option<&str>| {
                    brigadier_providers::ProviderEvent::Command {
                        item_id: "cmd-1".into(),
                        command: "sleep 70 && echo xyz".into(),
                        cwd: None,
                        status,
                        exit_code,
                        output: output.map(str::to_owned),
                        duration_ms: None,
                        kept: None,
                    }
                };
                let _ = events
                    .send(command(
                        brigadier_providers::ItemStatus::InProgress,
                        None,
                        None,
                    ))
                    .await;
                tokio::spawn(async move {
                    go.notified().await;
                    let _ = events
                        .send(command(
                            brigadier_providers::ItemStatus::Completed,
                            Some(0),
                            Some("xyz\n"),
                        ))
                        .await;
                });
                Reply::text("Started it; it runs in the background.")
            }
        }),
    )
    .await;
    flow.say("Start the command.").await;
    rested(&flow).await;
    let board = flow.board().await;
    let root = board.requests.keys().next().unwrap().clone();
    assert_eq!(board.requests[&root].state, RequestState::Working);
    assert!(
        flow.manager
            .hibernate(flow.conversation.clone())
            .await
            .is_err()
    );

    release.notify_one();
    let board = flow.settled().await;
    assert_eq!(board.requests[&root].state, RequestState::Done);
    let inputs = inputs.lock().unwrap().clone();
    assert!(
        inputs.iter().any(|input| input.contains(
            "[your background command `sleep 70 && echo xyz` finished after your turn · exit 0]"
        ) && input.contains("xyz")),
        "{inputs:#?}"
    );
    let answers = answers_with_requests(&flow).await;
    assert!(
        answers.contains(&("It printed xyz.".into(), Some(root))),
        "{answers:?}"
    );
    flow.stop().await;
}

/// A CLI that ends while a job of its own still runs takes the job with it: the thread hears
/// so in the job's request, and answers.
#[tokio::test]
async fn a_cli_ending_with_its_job_running_tells_the_thread() {
    let inputs: Log = Log::default();
    let log = inputs.clone();
    let flow = Flow::start(
        "codex-job-cut",
        Options {
            thread: brigadier_providers::ProviderKind::Codex,
            ..Options::default()
        },
        script(move |turn| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(turn.input.clone());
                if turn.input.contains("stopped when your CLI ended") {
                    return Reply::text("The command was cut off; I'll run it again.");
                }
                let events = turn.events.clone();
                let _ = events
                    .send(brigadier_providers::ProviderEvent::Command {
                        item_id: "cmd-1".into(),
                        command: "sleep 70".into(),
                        cwd: None,
                        status: brigadier_providers::ItemStatus::InProgress,
                        exit_code: None,
                        output: None,
                        duration_ms: None,
                        kept: None,
                    })
                    .await;
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    let _ = events
                        .send(brigadier_providers::ProviderEvent::Exited {
                            code: Some(1),
                            signal: None,
                            stderr_tail: None,
                        })
                        .await;
                });
                Reply::text("Started it.")
            }
        }),
    )
    .await;
    flow.say("Start the command.").await;
    super::eventually("the thread to hear its job was cut off", || {
        inputs.lock().unwrap().iter().any(|input| {
            input.contains("[your background command `sleep 70` stopped when your CLI ended")
        })
    })
    .await;
    let board = flow.settled().await;
    assert!(
        board
            .requests
            .values()
            .all(|request| request.state == RequestState::Done),
        "{:?}",
        board.requests
    );
    flow.stop().await;
}

/// A stopped request's CLI job runs on while a later request starts one of its own: each job
/// keeps the request whose turn started it. The stopped one's end answers in no request, and
/// the later request works until the turn its own job's end starts is over (#3).
#[tokio::test]
async fn each_cli_job_keeps_the_request_that_started_it() {
    use brigadier_providers::ProviderEvent;
    let events: Arc<Mutex<Option<tokio::sync::mpsc::Sender<ProviderEvent>>>> = Arc::default();
    let kept = events.clone();
    let flow = Flow::start(
        "cli-job-owners",
        Options::default(),
        script(move |turn| {
            let kept = kept.clone();
            async move {
                let running: Vec<String> = if turn.input.contains("Start A.") {
                    vec!["a".into()]
                } else if turn.input.contains("Start B.") {
                    vec!["a".into(), "b".into()]
                } else {
                    return Reply::text("Noted.");
                };
                let _ = turn
                    .events
                    .send(ProviderEvent::BackgroundJobs { running })
                    .await;
                *kept.lock().unwrap() = Some(turn.events.clone());
                Reply::text("Started it in the background.")
            }
        }),
    )
    .await;
    // The CLI's turn of its own when a job ends: the job's output, then the CLI's jobs left.
    let job_ended = |left: Vec<String>, text: &str| {
        let events = events.lock().unwrap().clone().expect("the CLI's events");
        let text = text.to_owned();
        async move {
            for event in [
                ProviderEvent::BackgroundJobs { running: left },
                ProviderEvent::TurnStarted { turn_id: None },
                ProviderEvent::Message {
                    item_id: uuid::Uuid::new_v4().to_string(),
                    role: brigadier_providers::Role::Assistant,
                    text,
                },
                ProviderEvent::TurnCompleted {
                    turn_id: None,
                    status: brigadier_providers::TurnStatus::Completed,
                    duration_ms: Some(1),
                    usage: None,
                },
            ] {
                events.send(event).await.unwrap();
            }
        }
    };
    flow.say("Start A.").await;
    rested(&flow).await;
    let first = flow.board().await.latest_request().unwrap().id.clone();
    assert_eq!(
        flow.board().await.requests[&first].state,
        RequestState::Working
    );
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    flow.until("the first request stopped", |board| {
        board.requests[&first].state == RequestState::Stopped
    })
    .await;
    flow.manager
        .resume_queue(flow.conversation.clone())
        .await
        .unwrap();
    flow.say("Start B.").await;
    let board = flow
        .until("the second request", |board| board.requests.len() == 2)
        .await;
    let second = board.latest_request().unwrap().id.clone();
    rested(&flow).await;
    // The stopped request's job ends first: its turn answers in no request.
    job_ended(vec!["b".into()], "Job a printed a-1.").await;
    super::eventually_async("the first job's answer", || async {
        answers_with_requests(&flow)
            .await
            .iter()
            .any(|(text, _)| text == "Job a printed a-1.")
    })
    .await;
    rested(&flow).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let answers = answers_with_requests(&flow).await;
    assert!(
        answers.contains(&("Job a printed a-1.".into(), None)),
        "{answers:?}"
    );
    let board = flow.board().await;
    assert_eq!(board.requests[&first].state, RequestState::Stopped);
    assert_eq!(
        board.requests[&second].state,
        RequestState::Working,
        "it waits for its own job"
    );
    job_ended(Vec::new(), "Job b printed b-2.").await;
    flow.until("the second request answered", |board| {
        board.requests[&second].state == RequestState::Done
    })
    .await;
    let answers = answers_with_requests(&flow).await;
    assert!(
        answers.contains(&("Job b printed b-2.".into(), Some(second.clone()))),
        "{answers:?}"
    );
    flow.stop().await;
}

/// The CLI may start a job's turn before it says the job ended: that turn is the job's, and
/// the word after it owes no second one (#3).
#[tokio::test]
async fn a_job_turn_before_the_jobs_word_is_owed_once() {
    use brigadier_providers::ProviderEvent;
    let events: Arc<Mutex<Option<tokio::sync::mpsc::Sender<ProviderEvent>>>> = Arc::default();
    let kept = events.clone();
    let flow = Flow::start(
        "cli-job-early-turn",
        Options::default(),
        script(move |turn| {
            let kept = kept.clone();
            async move {
                if !turn.input.contains("Start A.") {
                    return Reply::text("Noted.");
                }
                let _ = turn
                    .events
                    .send(ProviderEvent::BackgroundJobs {
                        running: vec!["a".into()],
                    })
                    .await;
                *kept.lock().unwrap() = Some(turn.events.clone());
                Reply::text("Started it in the background.")
            }
        }),
    )
    .await;
    flow.say("Start A.").await;
    rested(&flow).await;
    let first = flow.board().await.latest_request().unwrap().id.clone();
    let sender = events.lock().unwrap().clone().expect("the CLI's events");
    for event in [
        ProviderEvent::TurnStarted { turn_id: None },
        ProviderEvent::BackgroundJobs {
            running: Vec::new(),
        },
        ProviderEvent::Message {
            item_id: "job-a".into(),
            role: brigadier_providers::Role::Assistant,
            text: "Job a printed a-1.".into(),
        },
        ProviderEvent::TurnCompleted {
            turn_id: None,
            status: brigadier_providers::TurnStatus::Completed,
            duration_ms: Some(1),
            usage: None,
        },
    ] {
        sender.send(event).await.unwrap();
    }
    super::eventually_async("the job's answer", || async {
        answers_with_requests(&flow)
            .await
            .contains(&("Job a printed a-1.".into(), Some(first.clone())))
    })
    .await;
    rested(&flow).await;
    // Nothing is owed for it once its turn is over, well before a missing turn's grace ends.
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    assert!(conv.request_activity().await.background.is_empty());
    flow.until("the request answered", |board| {
        board.requests[&first].state == RequestState::Done
    })
    .await;
    flow.stop().await;
}

/// A stopped request's job and a later request's job both end; the CLI's turn for the stopped
/// one's end runs long. The later request still waits for its own job's turn meanwhile, and
/// that turn answers in it (#3).
#[tokio::test]
async fn a_long_turn_for_a_stopped_jobs_end_keeps_the_next_turn_owed() {
    use brigadier_providers::ProviderEvent;
    let events: Arc<Mutex<Option<tokio::sync::mpsc::Sender<ProviderEvent>>>> = Arc::default();
    let kept = events.clone();
    let flow = Flow::start(
        "cli-job-long-own-turn",
        Options::default(),
        script(move |turn| {
            let kept = kept.clone();
            async move {
                let running: Vec<String> = if turn.input.contains("Start A.") {
                    vec!["a".into()]
                } else if turn.input.contains("Start B.") {
                    vec!["a".into(), "b".into()]
                } else {
                    return Reply::text("Noted.");
                };
                let _ = turn
                    .events
                    .send(ProviderEvent::BackgroundJobs { running })
                    .await;
                *kept.lock().unwrap() = Some(turn.events.clone());
                Reply::text("Started it in the background.")
            }
        }),
    )
    .await;
    flow.say("Start A.").await;
    rested(&flow).await;
    let first = flow.board().await.latest_request().unwrap().id.clone();
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    flow.until("the first request stopped", |board| {
        board.requests[&first].state == RequestState::Stopped
    })
    .await;
    flow.manager
        .resume_queue(flow.conversation.clone())
        .await
        .unwrap();
    flow.say("Start B.").await;
    let board = flow
        .until("the second request", |board| board.requests.len() == 2)
        .await;
    let second = board.latest_request().unwrap().id.clone();
    rested(&flow).await;
    let sender = events.lock().unwrap().clone().expect("the CLI's events");
    let turn = |text: &str| {
        [
            ProviderEvent::Message {
                item_id: uuid::Uuid::new_v4().to_string(),
                role: brigadier_providers::Role::Assistant,
                text: text.to_owned(),
            },
            ProviderEvent::TurnCompleted {
                turn_id: None,
                status: brigadier_providers::TurnStatus::Completed,
                duration_ms: Some(1),
                usage: None,
            },
        ]
    };
    // Both end; the stopped one's turn comes first and outlasts a missing turn's grace.
    for event in [
        ProviderEvent::BackgroundJobs {
            running: Vec::new(),
        },
        ProviderEvent::TurnStarted { turn_id: None },
    ] {
        sender.send(event).await.unwrap();
    }
    tokio::time::sleep(super::super::conversation::BACKGROUND_TURN_GRACE * 2).await;
    assert_eq!(
        flow.board().await.requests[&second].state,
        RequestState::Working,
        "it still waits for its own job's turn"
    );
    for event in turn("Job a printed a-1.") {
        sender.send(event).await.unwrap();
    }
    sender
        .send(ProviderEvent::TurnStarted { turn_id: None })
        .await
        .unwrap();
    for event in turn("Job b printed b-2.") {
        sender.send(event).await.unwrap();
    }
    flow.until("the second request answered", |board| {
        board.requests[&second].state == RequestState::Done
    })
    .await;
    let answers = answers_with_requests(&flow).await;
    assert!(
        answers.contains(&("Job b printed b-2.".into(), Some(second.clone())))
            && answers.contains(&("Job a printed a-1.".into(), None)),
        "{answers:?}"
    );
    flow.stop().await;
}

/// A stopped request's job ends and the CLI's turn for it starts; a later request's job ends
/// while that turn runs. The later request still waits for its own job's turn, which answers
/// in it: the turn of no request's is a running turn too (#221).
#[tokio::test]
async fn a_job_ending_during_an_unowned_jobs_turn_keeps_its_turn_owed() {
    use brigadier_providers::ProviderEvent;
    let events: Arc<Mutex<Option<tokio::sync::mpsc::Sender<ProviderEvent>>>> = Arc::default();
    let kept = events.clone();
    let flow = Flow::start(
        "cli-job-during-own-turn",
        Options::default(),
        script(move |turn| {
            let kept = kept.clone();
            async move {
                let running: Vec<String> = if turn.input.contains("Start A.") {
                    vec!["a".into()]
                } else if turn.input.contains("Start B.") {
                    vec!["a".into(), "b".into()]
                } else {
                    return Reply::text("Noted.");
                };
                let _ = turn
                    .events
                    .send(ProviderEvent::BackgroundJobs { running })
                    .await;
                *kept.lock().unwrap() = Some(turn.events.clone());
                Reply::text("Started it in the background.")
            }
        }),
    )
    .await;
    flow.say("Start A.").await;
    rested(&flow).await;
    let first = flow.board().await.latest_request().unwrap().id.clone();
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    flow.until("the first request stopped", |board| {
        board.requests[&first].state == RequestState::Stopped
    })
    .await;
    flow.manager
        .resume_queue(flow.conversation.clone())
        .await
        .unwrap();
    flow.say("Start B.").await;
    let board = flow
        .until("the second request", |board| board.requests.len() == 2)
        .await;
    let second = board.latest_request().unwrap().id.clone();
    rested(&flow).await;
    let sender = events.lock().unwrap().clone().expect("the CLI's events");
    let turn = |text: &str| {
        [
            ProviderEvent::Message {
                item_id: uuid::Uuid::new_v4().to_string(),
                role: brigadier_providers::Role::Assistant,
                text: text.to_owned(),
            },
            ProviderEvent::TurnCompleted {
                turn_id: None,
                status: brigadier_providers::TurnStatus::Completed,
                duration_ms: Some(1),
                usage: None,
            },
        ]
    };
    // The stopped one's job ends and its turn starts; then the later one's ends meanwhile.
    for event in [
        ProviderEvent::BackgroundJobs {
            running: vec!["b".into()],
        },
        ProviderEvent::TurnStarted { turn_id: None },
        ProviderEvent::BackgroundJobs {
            running: Vec::new(),
        },
    ] {
        sender.send(event).await.unwrap();
    }
    tokio::time::sleep(super::super::conversation::BACKGROUND_TURN_GRACE * 3).await;
    assert_eq!(
        flow.board().await.requests[&second].state,
        RequestState::Working,
        "it still waits for its own job's turn"
    );
    for event in turn("Job a printed a-1.") {
        sender.send(event).await.unwrap();
    }
    sender
        .send(ProviderEvent::TurnStarted { turn_id: None })
        .await
        .unwrap();
    for event in turn("Job b printed b-2.") {
        sender.send(event).await.unwrap();
    }
    flow.until("the second request answered", |board| {
        board.requests[&second].state == RequestState::Done
    })
    .await;
    let answers = answers_with_requests(&flow).await;
    assert!(
        answers.contains(&("Job b printed b-2.".into(), Some(second.clone())))
            && answers.contains(&("Job a printed a-1.".into(), None)),
        "{answers:?}"
    );
    flow.stop().await;
}

/// #220: the user's Stop ends a background run even when its work doesn't heed it: the row
/// reads stopped, and the thread hears the user stopped it.
#[tokio::test]
async fn the_users_stop_ends_a_run_whose_work_ignores_it() {
    let inputs: Log = Default::default();
    let log = inputs.clone();
    let flow = Flow::start(
        "background-stop-stubborn",
        Options::default(),
        script(move |turn| {
            log.lock().unwrap().push(turn.input.clone());
            async { Reply::text("Noted.") }
        }),
    )
    .await;
    flow.background_commands();
    let reply = flow
        .manager
        .in_foreground(
            &flow.conversation,
            ("run", None),
            "sleep 600",
            tokio_util::sync::CancellationToken::new(),
            tokio_util::sync::CancellationToken::new(),
            std::future::pending(),
        )
        .await
        .unwrap();
    assert!(reply.starts_with("[run-1 still runs"), "{reply}");
    flow.manager
        .stop_background_run(flow.conversation.clone(), "run-1".into())
        .await
        .unwrap();
    flow.until("the run stopped", |board| {
        run_state(board, "run-1") == Some(BackgroundRunState::Stopped)
    })
    .await;
    flow.until("the thread told", |_| {
        inputs
            .lock()
            .unwrap()
            .iter()
            .any(|input| input.contains("[The user stopped run-1 after "))
    })
    .await;
    flow.stop().await;
}

/// #220: a run its row shows running though nothing watches it any more (its end was missed)
/// doesn't stay listed: the user's Stop ends it and tells the thread, and the watchdog's look
/// ends one nobody stopped.
#[tokio::test]
async fn a_run_whose_end_was_missed_does_not_stay_running() {
    let inputs: Log = Default::default();
    let log = inputs.clone();
    let flow = Flow::start(
        "background-missed-end",
        Options::default(),
        script(move |turn| {
            log.lock().unwrap().push(turn.input.clone());
            async { Reply::text("Noted.") }
        }),
    )
    .await;
    for id in ["run-7", "run-8"] {
        let run = BackgroundRun {
            id: id.into(),
            conversation_id: flow.conversation.clone(),
            request_id: None,
            tool: "run_check".into(),
            name: None,
            command: "cargo fmt --check".into(),
            started_at_ms: crate::now_ms() - 14 * 60_000,
            ended_at_ms: None,
            state: BackgroundRunState::Running,
            item_id: None,
        };
        flow.core
            .record_conversation(
                &flow.conversation,
                vec![DomainEvent::BackgroundRunUpdated { run }],
            )
            .await
            .unwrap();
    }
    flow.manager
        .stop_background_run(flow.conversation.clone(), "run-7".into())
        .await
        .unwrap();
    let board = flow
        .until("the stopped row", |board| {
            run_state(board, "run-7") == Some(BackgroundRunState::Stopped)
        })
        .await;
    assert_eq!(
        run_state(&board, "run-8"),
        Some(BackgroundRunState::Running)
    );
    flow.manager.end_missed_runs(&flow.conversation).await;
    flow.until("the missed run ended", |board| {
        matches!(
            run_state(board, "run-8"),
            Some(BackgroundRunState::Interrupted { .. })
        )
    })
    .await;
    flow.until("the thread told of both", |_| {
        let inputs = inputs.lock().unwrap();
        inputs
            .iter()
            .any(|input| input.contains("[The user stopped run-7 after "))
            && inputs
                .iter()
                .any(|input| input.contains("[run-8 ended without Brigadier seeing how"))
    })
    .await;
    // Over, neither can be stopped again.
    for id in ["run-7", "run-8"] {
        assert!(
            flow.manager
                .stop_background_run(flow.conversation.clone(), id.into())
                .await
                .is_err()
        );
    }
    flow.stop().await;
}

/// What the live feed sends is a domain event a client can switch on by its `type`: a
/// background run's stored output, a request's change records and the thread's inbox are the
/// engine's own records and stay off it (#189).
#[tokio::test]
async fn the_live_feed_carries_only_typed_domain_events() {
    let (inputs, replies): (Log, Log) = Default::default();
    let flow = Flow::start(
        "20261017-0213-issues-run-w49-feed",
        Options::default(),
        checking(vec![("run", "sleep 3; echo ran")], inputs, replies),
    )
    .await;
    flow.background_commands();
    flow.say("Run it.").await;
    flow.settled().await;
    let events = flow.core.store().read_since(0, 100_000).await.unwrap();
    let mut kept_off = std::collections::BTreeSet::new();
    for event in &events {
        if !crate::model::streams::on_live_feed(&event.stream) {
            kept_off.insert(event.kind.clone());
            continue;
        }
        let typed = serde_json::from_str::<DomainEvent>(event.payload.get());
        assert!(
            typed.is_ok(),
            "{} ({}) on the feed: {:?}",
            event.stream,
            event.kind,
            typed.err()
        );
    }
    assert!(kept_off.contains("background.output"), "{kept_off:?}");
    flow.stop().await;
}
