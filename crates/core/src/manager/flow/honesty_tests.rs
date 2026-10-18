//! The thread speaks when it judges it useful: each line it writes reaches the user as it
//! writes it, before or between tool calls too, and a request over with nothing more said
//! simply ends (#248, #54, #105). A reply to another session carries only what answers its
//! message.

use std::sync::{Arc, Mutex};

use brigadier_providers::{ItemStatus, ProviderEvent, ProviderKind, Role};
use serde_json::json;

use super::peer_kit::{Peers, is_reply};
use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::prompts::QUIET;
use crate::model::MessageRole;
use crate::work::RequestState;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

type Log = Arc<Mutex<Vec<String>>>;

/// A reply of the thread's, whole.
fn said(text: &str) -> ProviderEvent {
    ProviderEvent::Message {
        item_id: uuid::Uuid::new_v4().to_string(),
        role: Role::Assistant,
        text: text.into(),
    }
}

/// The start of the thread's call of Brigadier's `tool`, as its CLI reports it.
fn calls(tool: &str) -> ProviderEvent {
    ProviderEvent::ToolCall {
        item_id: uuid::Uuid::new_v4().to_string(),
        name: format!("mcp__brigadier__{tool}"),
        input: Some("{}".into()),
        status: ItemStatus::InProgress,
        output: None,
        kept: None,
    }
}

/// The assistant messages the user sees, oldest first, with their requests.
async fn shown(flow: &Flow) -> Vec<(String, Option<String>)> {
    flow.core
        .all_messages(&flow.conversation)
        .await
        .unwrap()
        .into_iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .map(|message| (message.text, message.request_id))
        .collect()
}

/// #54: a one-line message written before delegate_task reaches the user, after an earlier
/// line of the same turn too, and the worker starts and reports.
#[tokio::test]
async fn a_line_before_delegating_reaches_the_user_and_the_worker_starts() {
    const FIRST: &str = "The exporter lives in src/export.rs.";
    const LINE: &str = "I'll hand the exporter to a worker.";
    let flow = Flow::start(
        "honesty-line-before-delegate",
        Options::default(),
        script(|turn| async move {
            if !turn.is_orchestrator() {
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Surveyed the exporter."}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Reported.");
            }
            if turn.input.contains("[report task-1") {
                return Reply::text("The exporter writes CSV only.");
            }
            turn.emit(said(FIRST)).await;
            turn.emit(calls("code_search")).await;
            turn.emit(said(LINE)).await;
            turn.emit(calls("delegate_task")).await;
            let reply = turn
                .call(
                    "delegate_task",
                    json!({"effort": "medium", "title": "Survey the exporter",
                           "kind": "research", "spec": "Say what the exporter writes."}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text(QUIET)
        }),
    )
    .await;
    flow.say("What does the exporter write?").await;
    let board = flow.settled().await;
    let request = board.latest_request().unwrap().id.clone();
    assert_eq!(board.requests[&request].state, RequestState::Done);
    assert_eq!(
        Flow::task(&board, 1).request_id.as_deref(),
        Some(request.as_str())
    );
    assert_eq!(
        shown(&flow).await,
        [
            (FIRST.to_owned(), Some(request.clone())),
            (LINE.to_owned(), Some(request.clone())),
            ("The exporter writes CSV only.".to_owned(), Some(request)),
        ],
    );
    flow.stop().await;
}

/// #105: short commentary between two calls of one turn reaches the user, each line in turn.
#[tokio::test]
async fn commentary_between_two_calls_reaches_the_user() {
    let flow = Flow::start(
        "honesty-commentary",
        Options::default(),
        script(|turn| async move {
            turn.emit(said("Let me check how the tabs work today."))
                .await;
            turn.emit(calls("code_search")).await;
            turn.emit(said("Let me check the tab bar next.")).await;
            turn.emit(calls("code_search")).await;
            Reply::text("The tabs keep their order across restarts.")
        }),
    )
    .await;
    flow.say("How do the tabs work?").await;
    let board = flow.settled().await;
    let request = board.latest_request().unwrap().id.clone();
    let lines: Vec<String> = shown(&flow)
        .await
        .into_iter()
        .map(|(text, _)| text)
        .collect();
    assert_eq!(
        lines,
        [
            "Let me check how the tabs work today.",
            "Let me check the tab bar next.",
            "The tabs keep their order across restarts.",
        ],
    );
    assert_eq!(board.requests[&request].state, RequestState::Done);
    flow.stop().await;
}

/// A request over with only a line said mid-work simply ends: nothing asks the thread again
/// for an answer, and no closing line is added.
#[tokio::test]
async fn a_request_over_on_a_line_said_mid_work_simply_ends() {
    const LINE: &str = "Now the full suites, once, on the session branch.";
    let inputs = Log::default();
    let log = inputs.clone();
    let flow = Flow::start(
        "honesty-line-ends",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(turn.input.clone());
                if turn.input.contains("[run-1 finished") {
                    turn.emit(said(LINE)).await;
                    turn.emit(calls("code_search")).await;
                    return Reply::text(QUIET);
                }
                let reply = turn
                    .call("run_check", json!({ "command": "sleep 3; echo ok" }))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text(QUIET)
            }
        }),
    )
    .await;
    flow.background_commands();
    flow.say("Run the full checks.").await;
    flow.until("the request to end", |board| {
        !board.requests.is_empty()
            && board
                .requests
                .values()
                .all(|request| request.state == RequestState::Done)
    })
    .await;
    let board = flow.settled().await;
    let request = board.latest_request().unwrap().id.clone();
    assert_eq!(board.requests.len(), 1);
    assert_eq!(shown(&flow).await, [(LINE.to_owned(), Some(request))]);
    assert_eq!(
        inputs.lock().unwrap().len(),
        2,
        "{:#?}",
        inputs.lock().unwrap()
    );
    assert!(
        !board
            .notices
            .iter()
            .any(|notice| notice.text.contains("without an answer")),
        "{:?}",
        board.notices
    );
    flow.stop().await;
}

const PEER_REMINDED: &str = "You haven't answered the message from session";

/// The API session works on its user's request; the sender's message is steered in and the
/// turn answers the user only. When reminded, the thread answers the message with `reminded`
/// (or says nothing, when `None`). Returns the sender's replies as its turns read them.
async fn mixed_reply(name: &str, reminded: Option<&'static str>) -> (Flow, Vec<String>) {
    let peers = Peers::new();
    let flow = Flow::start(name, Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let _sent = peers.sends(&flow.conversation, &api, "Which column holds names?");
    let started = Arc::new(tokio::sync::Notify::new());
    let at = started.clone();
    let asked: Arc<Mutex<Option<(String, crate::model::ConversationId)>>> = Arc::default();
    peers.on(&api, move |turn| {
        let (at, asked) = (at.clone(), asked.clone());
        async move {
            if turn.input.contains(PEER_REMINDED) {
                let Some(text) = reminded else {
                    return Reply::text(QUIET);
                };
                let (id, from) = asked.lock().unwrap().clone().unwrap();
                assert!(turn.input.contains(&id), "{}", turn.input);
                let (_, failed) = super::peer_kit::send(&turn, &from, text, Some(&id)).await;
                assert!(!failed);
                return Reply::text(QUIET);
            }
            at.notify_one();
            let steer = turn.steered().await.expect("the message steered in");
            *asked.lock().unwrap() = Some((
                super::peer_kit::header_id(&steer).unwrap(),
                super::peer_kit::header_session(&steer).unwrap(),
            ));
            Reply::text("The command printed U990085.")
        }
    });
    flow.say_to(&api, "Run the build.").await;
    started.notified().await;
    flow.say("Ask the API session where names live.").await;
    super::eventually("the reply to reach the sender", || {
        peers
            .inputs_of(&flow.conversation)
            .iter()
            .any(|input| is_reply(input))
    })
    .await;
    flow.settled_in(&api).await;
    let replies = peers
        .inputs_of(&flow.conversation)
        .into_iter()
        .filter(|input| is_reply(input))
        .collect();
    (flow, replies)
}

/// A message steered into other work and left unanswered there: the thread is reminded to
/// answer it on its own, and that answer is the reply, not the other work's.
#[tokio::test]
async fn a_message_steered_into_other_work_gets_only_its_own_answer() {
    let (flow, replies) = mixed_reply("peer-mixed-reminded", Some("It's `name`.")).await;
    assert_eq!(replies.len(), 1, "{replies:#?}");
    assert!(replies[0].contains("It's `name`."), "{}", replies[0]);
    assert!(!replies[0].contains("U990085"), "{}", replies[0]);
    flow.stop().await;
}

/// Reminded and still silent: the sender hears that it went unanswered, never the other
/// work's answer.
#[tokio::test]
async fn a_message_left_unanswered_amid_other_work_says_so() {
    let (flow, replies) = mixed_reply("peer-mixed-silent", None).await;
    assert_eq!(replies.len(), 1, "{replies:#?}");
    assert!(
        replies[0].contains("\"API work\" didn't answer your message"),
        "{}",
        replies[0]
    );
    assert!(!replies[0].contains("U990085"), "{}", replies[0]);
    flow.stop().await;
}

/// The thread's `Bash` of `command`, started or ended.
fn bash(command: &str, status: ItemStatus) -> ProviderEvent {
    ProviderEvent::ToolCall {
        item_id: "bash-1".into(),
        name: "Bash".into(),
        input: Some(json!({ "command": command }).to_string()),
        status,
        output: (status != ItemStatus::InProgress).then(|| "ok".into()),
        kept: None,
    }
}

/// The 33fbb16e "deciding" case: a follow-up sent while the thread is inside a long command
/// says it waits behind it, and with no judgment after the wait it joins the working answer
/// rather than deciding for minutes; the thread reads it once the command ends, and its late
/// judgment hears it already joined.
#[tokio::test]
async fn a_follow_up_behind_a_long_command_joins_the_working_answer_unjudged() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let (inputs, item): (Log, Arc<Mutex<Option<String>>>) = Default::default();
    let (release, log, judged) = (gate.clone(), inputs.clone(), item.clone());
    let flow = Flow::start(
        "honesty-deciding",
        Options::default(),
        script(move |turn| {
            let (gate, log, item) = (gate.clone(), log.clone(), item.clone());
            async move {
                log.lock().unwrap().push(turn.input.clone());
                turn.emit(bash("cargo test --workspace", ItemStatus::InProgress))
                    .await;
                // Asked to judge the follow-up, then handed the user's message unjudged.
                for _ in 0..2 {
                    let steered = turn.steered().await.unwrap_or_default();
                    log.lock().unwrap().push(steered);
                }
                gate.notified().await;
                turn.emit(bash("cargo test --workspace", ItemStatus::Completed))
                    .await;
                let id = item.lock().unwrap().clone().unwrap();
                let late = turn
                    .call(
                        "route_follow_up",
                        json!({ "follow_up": id, "joins": false }),
                    )
                    .await;
                log.lock().unwrap().push(late.text);
                Reply::text("The tests pass, and the formatting is clean too.")
            }
        }),
    )
    .await;
    flow.say("Run the tests.").await;
    flow.until("the command to run", |board| {
        board.orchestrator_steps.iter().any(|step| {
            matches!(&step.kind, crate::work::OrchestratorStepKind::Tool { status, .. } if *status == ItemStatus::InProgress)
        })
    })
    .await;
    flow.say("Also check the formatting.").await;
    let board = flow
        .until("the follow-up to say what it waits behind", |board| {
            board.queue.items.first().is_some_and(|queued| {
                queued.deciding
                    && queued
                        .held_by
                        .as_ref()
                        .is_some_and(|held| held.command == "cargo test --workspace")
            })
        })
        .await;
    *judged.lock().unwrap() = Some(board.queue.items[0].id.clone());
    let board = flow
        .until(
            "the unjudged follow-up to join the working answer",
            |board| {
                board.queue.items.is_empty()
                    && board.requests.values().any(|request| {
                        request.steered_into.is_some()
                            && request
                                .held_by
                                .as_ref()
                                .is_some_and(|held| held.command == "cargo test --workspace")
                    })
            },
        )
        .await;
    assert_eq!(board.requests.len(), 2, "{:?}", board.requests);
    release.notify_one();
    let board = flow.settled().await;
    assert!(
        board
            .requests
            .values()
            .all(|request| request.held_by.is_none() && request.state == RequestState::Done),
        "{:?}",
        board.requests
    );
    let inputs = inputs.lock().unwrap().clone();
    assert!(
        inputs[2].contains("Also check the formatting."),
        "the user's message joined: {inputs:#?}"
    );
    assert!(
        inputs[3].starts_with("It already joined this work"),
        "{inputs:#?}"
    );
    flow.stop().await;
}

/// A message the user steers in while the thread's command runs says so until the command
/// ends, then the thread reads it.
#[tokio::test]
async fn a_steer_behind_a_long_command_says_so_until_the_command_ends() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let inputs: Log = Default::default();
    let (release, log) = (gate.clone(), inputs.clone());
    let flow = Flow::start(
        "honesty-steer-held",
        Options::default(),
        script(move |turn| {
            let (gate, log) = (gate.clone(), log.clone());
            async move {
                turn.emit(bash("sleep 300", ItemStatus::InProgress)).await;
                gate.notified().await;
                turn.emit(bash("sleep 300", ItemStatus::Completed)).await;
                let steered = turn.steered().await.unwrap_or_default();
                log.lock().unwrap().push(steered);
                Reply::text("Stopped waiting; using banker's rounding.")
            }
        }),
    )
    .await;
    flow.say("Wait five minutes.").await;
    flow.until("the command to run", |board| {
        board.orchestrator_steps.iter().any(|step| {
            matches!(&step.kind, crate::work::OrchestratorStepKind::Tool { status, .. } if *status == ItemStatus::InProgress)
        })
    })
    .await;
    flow.manager
        .send_message(
            flow.conversation.clone(),
            "Use banker's rounding instead.".into(),
            Vec::new(),
            Vec::new(),
            true,
            None,
        )
        .await
        .unwrap();
    flow.until("the steered message to wait behind the command", |board| {
        board.requests.values().any(|request| {
            request.steered_into.is_some()
                && request
                    .held_by
                    .as_ref()
                    .is_some_and(|held| held.command == "sleep 300")
        })
    })
    .await;
    release.notify_one();
    flow.until("the command's end to let the thread read it", |board| {
        board.requests.len() == 2
            && board
                .requests
                .values()
                .all(|request| request.held_by.is_none())
    })
    .await;
    flow.settled().await;
    let inputs = inputs.lock().unwrap().clone();
    assert!(
        inputs
            .iter()
            .any(|input| input.contains("Use banker's rounding instead.")),
        "{inputs:#?}"
    );
    flow.stop().await;
}

/// route_follow_up is optional (#248): a follow-up the thread answers without sorting it,
/// in a turn that ends before the judging wait is over, joins that turn's request rather
/// than waiting in the queue as a request of its own.
#[tokio::test]
async fn a_follow_up_left_unsorted_joins_the_request_its_turn_served() {
    let inputs: Log = Default::default();
    let log = inputs.clone();
    let flow = Flow::start(
        "honesty-unsorted",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(turn.input.clone());
                if log.lock().unwrap().len() > 1 {
                    return Reply::text("[quiet]");
                }
                turn.emit(bash("cargo test --workspace", ItemStatus::InProgress))
                    .await;
                let steered = turn.steered().await.unwrap_or_default();
                log.lock().unwrap().push(steered);
                turn.emit(bash("cargo test --workspace", ItemStatus::Completed))
                    .await;
                Reply::text("The tests pass, and the formatting is clean too.")
            }
        }),
    )
    .await;
    flow.say("Run the tests.").await;
    flow.until("the command to run", |board| {
        board.orchestrator_steps.iter().any(|step| {
            matches!(&step.kind, crate::work::OrchestratorStepKind::Tool { status, .. } if *status == ItemStatus::InProgress)
        })
    })
    .await;
    flow.say("Also check the formatting.").await;
    let board = flow.settled().await;
    assert!(board.queue.items.is_empty(), "{:?}", board.queue.items);
    assert_eq!(board.requests.len(), 2, "{:?}", board.requests);
    assert!(
        board
            .requests
            .values()
            .any(|request| request.steered_into.is_some()),
        "the follow-up joined the first request: {:?}",
        board.requests
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
    assert!(inputs[1].contains("[follow-up"), "{inputs:#?}");
    assert!(
        inputs[2].contains("Also check the formatting."),
        "the joined message reaches the thread: {inputs:#?}"
    );
    flow.stop().await;
}
