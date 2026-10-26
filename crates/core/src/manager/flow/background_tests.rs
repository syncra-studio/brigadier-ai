//! Jobs the thread's CLI keeps running in the background past its turn: the request works
//! until their end, which the thread hears in it; and the context card's Background rows.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::model::{DomainEvent, MessageRole};
use crate::work::RequestState;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

type Log = Arc<Mutex<Vec<String>>>;

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

/// How a lingering Codex command's end reaches Brigadier: its item's end and its terminal
/// leaving `/ps`, in either order, or the terminal's leaving alone.
#[derive(Clone, Copy, Debug)]
enum TerminalEnd {
    ItemFirst,
    TerminalFirst,
    TerminalOnly,
}

/// A Codex command outliving its turn as a background terminal: whichever of its item's end
/// and its terminal's leaving the list comes first, the thread hears of it once, with the
/// item's exit code when it comes, in the request that started it (D21).
async fn a_codex_terminal_ends_once(end: TerminalEnd) {
    let go = Arc::new(tokio::sync::Notify::new());
    let release = go.clone();
    let inputs: Log = Log::default();
    let log = inputs.clone();
    let flow = Flow::start(
        &format!("codex-terminal-{end:?}"),
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
                        command: "/bin/zsh -lc 'sleep 70 && echo xyz'".into(),
                        cwd: None,
                        status,
                        exit_code,
                        output: output.map(str::to_owned),
                        duration_ms: None,
                        kept: None,
                    }
                };
                let terminal =
                    |status, ended_at_ms| brigadier_providers::ProviderEvent::BackgroundTask {
                        task: brigadier_providers::BackgroundTask {
                            id: "4242".into(),
                            item_id: Some("cmd-1".into()),
                            kind: brigadier_providers::BackgroundTaskKind::Terminal,
                            label: "sleep 70 && echo xyz".into(),
                            status,
                            started_at_ms: 1,
                            ended_at_ms,
                            output_file: None,
                            summary: None,
                            last_line: None,
                        },
                    };
                let _ = events
                    .send(command(
                        brigadier_providers::ItemStatus::InProgress,
                        None,
                        None,
                    ))
                    .await;
                let _ = events
                    .send(terminal(
                        brigadier_providers::BackgroundTaskStatus::Running,
                        None,
                    ))
                    .await;
                tokio::spawn(async move {
                    go.notified().await;
                    let item = command(
                        brigadier_providers::ItemStatus::Completed,
                        Some(0),
                        Some("xyz\n"),
                    );
                    let gone = terminal(
                        brigadier_providers::BackgroundTaskStatus::Completed,
                        Some(2),
                    );
                    let order = match end {
                        TerminalEnd::ItemFirst => vec![item, gone],
                        TerminalEnd::TerminalFirst => vec![gone, item],
                        TerminalEnd::TerminalOnly => vec![gone],
                    };
                    for event in order {
                        let _ = events.send(event).await;
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
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

    release.notify_one();
    flow.until("the command's end to be told", |_| {
        inputs
            .lock()
            .unwrap()
            .iter()
            .any(|input| input.contains("finished after your turn"))
    })
    .await;
    // Past the wait for an item's end that a terminal's leaving starts: nothing more is told.
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let board = flow.settled().await;
    assert_eq!(board.requests[&root].state, RequestState::Done);
    let told: Vec<String> = inputs
        .lock()
        .unwrap()
        .iter()
        .filter(|input| input.contains("finished after your turn"))
        .cloned()
        .collect();
    let expected = match end {
        TerminalEnd::TerminalOnly => {
            "[your background command `sleep 70 && echo xyz` finished after your turn · completed]"
        }
        _ => {
            "[your background command `sleep 70 && echo xyz` finished after your turn · exit 0]\nxyz"
        }
    };
    assert_eq!(told.len(), 1, "{end:?}: {told:#?}");
    let (provenance, payload) = told[0].split_once('\n').expect("registered input prefix");
    let nonce = provenance
        .strip_prefix("[Brigadier internal input ")
        .and_then(|prefix| prefix.strip_suffix(']'))
        .expect("typed internal input provenance");
    assert!(uuid::Uuid::parse_str(nonce).is_ok(), "{provenance}");
    assert_eq!(payload.trim_end(), expected, "{end:?}: {told:#?}");
    assert_eq!(payload.matches("finished after your turn").count(), 1);
    let answers = answers_with_requests(&flow).await;
    assert!(
        answers.contains(&("It printed xyz.".into(), Some(root))),
        "{answers:?}"
    );
    flow.stop().await;
}

#[tokio::test]
async fn a_codex_terminal_ends_once_when_its_item_ends_first() {
    a_codex_terminal_ends_once(TerminalEnd::ItemFirst).await;
}

#[tokio::test]
async fn a_codex_terminal_ends_once_when_it_leaves_the_list_first() {
    a_codex_terminal_ends_once(TerminalEnd::TerminalFirst).await;
}

#[tokio::test]
async fn a_codex_terminal_leaving_the_list_ends_its_command_without_its_item() {
    a_codex_terminal_ends_once(TerminalEnd::TerminalOnly).await;
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

/// Q94: × takes an ended preview's Background row off; × on one still running, or on no
/// preview, is refused; a preview started again under its id is back; the view lists the rows
/// taken off, for a reload.
#[tokio::test]
async fn dismiss_takes_an_ended_previews_row_off() {
    let flow = Flow::start(
        "background-dismiss",
        Options::default(),
        script(|_| async { Reply::text("Noted.") }),
    )
    .await;
    let preview = |id: &str, state: serde_json::Value| -> crate::work::Preview {
        serde_json::from_value(json!({
            "id": id, "conversationId": flow.conversation.0, "name": "web",
            "command": "pnpm dev", "workdir": "/work", "workspace": "/work", "state": state,
            "startedAtMs": 1, "endedAtMs": 2
        }))
        .unwrap()
    };
    flow.core
        .record_conversation(
            &flow.conversation,
            vec![
                DomainEvent::PreviewUpdated {
                    preview: preview(
                        "preview-1",
                        json!({"type": "exited", "code": 1, "status": "exit 1"}),
                    ),
                },
                DomainEvent::PreviewUpdated {
                    preview: preview("preview-2", json!({"type": "starting"})),
                },
            ],
        )
        .await
        .unwrap();
    let id = || flow.conversation.clone();
    for row in ["preview-2", "preview-9", "run-1"] {
        assert!(
            flow.manager
                .dismiss_background(id(), row.into())
                .await
                .is_err(),
            "{row} isn't dismissed"
        );
    }
    flow.manager
        .dismiss_background(id(), "preview-1".into())
        .await
        .unwrap();
    let view = flow.core.conversation_view(id(), 50).await.unwrap();
    assert_eq!(view.background_cleared, ["preview-1"]);
    // Started again under its id, the preview is back.
    flow.core
        .record_conversation(
            &flow.conversation,
            vec![DomainEvent::PreviewUpdated {
                preview: preview("preview-1", json!({"type": "starting"})),
            }],
        )
        .await
        .unwrap();
    let view = flow.core.conversation_view(id(), 50).await.unwrap();
    assert!(view.background_cleared.is_empty());
    flow.stop().await;
}

/// An app keeps its native CLI/process owner and history while its useful work settles.
/// In the late cases the first job list/terminal arrives before a proven app command.
async fn persistent_app_lifetime(provider: brigadier_providers::ProviderKind, late: bool) {
    use brigadier_providers::{
        BackgroundTask, BackgroundTaskKind, BackgroundTaskStatus, ItemStatus, ProviderEvent,
        ProviderKind,
    };
    let release = Arc::new(tokio::sync::Notify::new());
    let metadata = release.clone();
    let feed = Arc::new(Mutex::new(None));
    let producer = feed.clone();
    let flow = Flow::start(
        &format!("persistent-app-{provider:?}-{late}"),
        Options {
            thread: provider,
            ..Options::default()
        },
        script(move |turn| {
            let metadata = metadata.clone();
            let producer = producer.clone();
            async move {
                turn.emit(ProviderEvent::Message {
                    item_id: format!("app-user-{}", turn.earlier),
                    role: brigadier_providers::Role::User,
                    text: turn.input.clone(),
                })
                .await;
                if !turn.input.contains("Start the persistent app") {
                    return Reply::text("A separate follow-up answer.");
                }
                let events = turn.events.clone();
                *producer.lock().unwrap() = Some(events.clone());
                let command = |words: &str| ProviderEvent::Command {
                    item_id: "app-call".into(),
                    command: words.into(),
                    cwd: Some(turn.cwd.display().to_string()),
                    status: if provider == ProviderKind::Claude {
                        ItemStatus::Completed
                    } else {
                        ItemStatus::InProgress
                    },
                    exit_code: None,
                    output: None,
                    duration_ms: None,
                    kept: None,
                };
                let task = |known| ProviderEvent::BackgroundTask {
                    task: BackgroundTask {
                        id: "native-app".into(),
                        item_id: Some("app-call".into()),
                        kind: if provider == ProviderKind::Claude {
                            BackgroundTaskKind::Shell
                        } else {
                            BackgroundTaskKind::Terminal
                        },
                        label: if provider == ProviderKind::Claude {
                            "Show the website"
                        } else if known {
                            "pnpm dev"
                        } else {
                            "node pending.js"
                        }
                        .into(),
                        status: BackgroundTaskStatus::Running,
                        started_at_ms: 123,
                        ended_at_ms: None,
                        output_file: None,
                        summary: None,
                        last_line: None,
                    },
                };
                if provider == ProviderKind::Codex || !late {
                    events
                        .send(command(if late { "node pending.js" } else { "pnpm dev" }))
                        .await
                        .unwrap();
                }
                events.send(task(!late)).await.unwrap();
                if provider == ProviderKind::Claude {
                    events
                        .send(ProviderEvent::BackgroundJobs {
                            running: vec!["native-app".into()],
                        })
                        .await
                        .unwrap();
                }
                if late {
                    let full = command("pnpm dev");
                    let known = task(true);
                    tokio::spawn(async move {
                        metadata.notified().await;
                        events.send(full).await.unwrap();
                        events.send(known).await.unwrap();
                    });
                }
                Reply::text("The app is available in Background.")
            }
        }),
    )
    .await;
    flow.say("Start the persistent app.").await;
    rested(&flow).await;
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    if late {
        assert!(
            flow.manager.work_holds(&conv).await,
            "unknown command evidence conservatively holds work"
        );
        assert!(
            flow.board()
                .await
                .requests
                .values()
                .any(|r| r.state == RequestState::Working)
        );
        release.notify_one();
    }
    let board = flow.settled().await;
    assert!(
        board
            .requests
            .values()
            .all(|r| r.state == RequestState::Done)
    );
    assert!(
        !flow.manager.work_holds(&conv).await,
        "a persistent app cannot hold the lead workblock"
    );
    assert!(
        conv.has_cli_jobs().await,
        "native process/Stop ownership is retained"
    );
    assert!(
        conv.live_cli().await.is_some(),
        "classification must not retire or close its CLI"
    );
    let activity = flow
        .manager
        .activity()
        .await
        .into_iter()
        .find(|a| a.conversation_id == flow.conversation);
    assert!(
        activity.is_none_or(|a| !a.commands_working && !a.turn_open),
        "settled sidebar activity is absent or no longer Working"
    );
    let view = flow.manager.thread_view(&flow.conversation).unwrap();
    assert!(
        view.background
            .iter()
            .any(|t| t.id == "native-app" && t.status == BackgroundTaskStatus::Running)
    );
    let history = flow
        .manager
        .background_history(&flow.conversation, None, 50)
        .await
        .unwrap();
    assert!(
        history
            .records
            .iter()
            .any(|r| r.task_id == "native-app" && r.status == "running")
    );
    flow.say("Follow-up while the app stays up.").await;
    let board = flow.settled().await;
    assert_eq!(board.requests.len(), 2);
    assert!(
        board.requests.values().all(|r| r.steered_into.is_none()),
        "app-only background work cannot absorb a new user's request"
    );
    assert!(
        !flow
            .manager
            .thread_view(&flow.conversation)
            .unwrap()
            .turns
            .last()
            .unwrap()
            .joins
    );
    let events = feed.lock().unwrap().clone().unwrap();
    events
        .send(ProviderEvent::Command {
            item_id: "app-call".into(),
            command: "pnpm dev".into(),
            cwd: None,
            status: ItemStatus::Completed,
            exit_code: Some(0),
            output: Some("app ended".into()),
            duration_ms: None,
            kept: None,
        })
        .await
        .unwrap();
    events
        .send(ProviderEvent::BackgroundTask {
            task: BackgroundTask {
                id: "native-app".into(),
                item_id: Some("app-call".into()),
                kind: if provider == ProviderKind::Claude {
                    BackgroundTaskKind::Shell
                } else {
                    BackgroundTaskKind::Terminal
                },
                label: "pnpm dev".into(),
                status: BackgroundTaskStatus::Completed,
                started_at_ms: 123,
                ended_at_ms: Some(crate::now_ms()),
                output_file: None,
                summary: None,
                last_line: None,
            },
        })
        .await
        .unwrap();
    if provider == ProviderKind::Claude {
        events
            .send(ProviderEvent::BackgroundJobs {
                running: Vec::new(),
            })
            .await
            .unwrap();
    }
    super::eventually_async(
        "the native app to end without owing a work turn",
        || async {
            !conv.has_cli_jobs().await
                && !flow
                    .manager
                    .thread_view(&flow.conversation)
                    .unwrap()
                    .jobs
                    .waking
        },
    )
    .await;
    flow.idle().await;
    assert!(!flow.manager.work_holds(&conv).await);
    assert_eq!(
        flow.board().await.requests.len(),
        2,
        "app completion cannot reopen or fabricate a user's request"
    );
    let history = flow
        .manager
        .background_history(&flow.conversation, None, 50)
        .await
        .unwrap();
    assert!(
        history
            .records
            .iter()
            .any(|r| r.task_id == "native-app" && r.status == "completed")
    );
    if provider == ProviderKind::Claude {
        events
            .send(ProviderEvent::BackgroundJobs {
                running: vec!["native-app".into()],
            })
            .await
            .unwrap();
        super::eventually_async(
            "a reused native id without new app evidence to hold conservatively",
            || async { flow.manager.work_holds(&conv).await },
        )
        .await;
        assert!(
            flow.manager.work_holds(&conv).await,
            "an ended app's cached command cannot classify a reused unknown invocation"
        );
    }
    flow.manager.shutdown().await;
}

#[tokio::test]
async fn persistent_app_claude_does_not_hold_work_or_sidebar() {
    persistent_app_lifetime(brigadier_providers::ProviderKind::Claude, false).await;
}
#[tokio::test]
async fn persistent_app_codex_does_not_hold_work_or_sidebar() {
    persistent_app_lifetime(brigadier_providers::ProviderKind::Codex, false).await;
}
#[tokio::test]
async fn persistent_app_late_claude_metadata_releases_only_proven_app_work() {
    persistent_app_lifetime(brigadier_providers::ProviderKind::Claude, true).await;
}
#[tokio::test]
async fn persistent_app_late_codex_metadata_releases_only_proven_app_work() {
    persistent_app_lifetime(brigadier_providers::ProviderKind::Codex, true).await;
}
