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
    flow.say("Run the long check.").await;
    let board = flow
        .until("the check to go to the background", |board| {
            run_state(board, "run-1") == Some(BackgroundRunState::Running)
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

/// The same check asked again while the first still runs waits for it in the background too,
/// and answers from what the first kept.
#[tokio::test]
async fn a_check_waiting_for_the_same_check_goes_to_the_background_and_answers_from_its_result() {
    let (inputs, replies): (Log, Log) = Default::default();
    let flow = Flow::start(
        "background-same-check",
        Options::default(),
        checking(
            vec![
                ("run_check", "sleep 6; echo once"),
                ("run_check", "sleep 6; echo once"),
            ],
            inputs.clone(),
            replies.clone(),
        ),
    )
    .await;
    flow.say("Check twice.").await;
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
                    .send(brigadier_providers::ProviderEvent::BackgroundJobs { running: 1 })
                    .await;
                tokio::spawn(async move {
                    go.notified().await;
                    for event in [
                        brigadier_providers::ProviderEvent::BackgroundJobs { running: 0 },
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
