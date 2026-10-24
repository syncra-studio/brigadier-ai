//! The thread speaks when it judges it useful: each line it writes reaches the user as it
//! writes it, before or between tool calls too, and a request over with nothing more said
//! simply ends (#248, #54, #105). A reply to another session carries only what answers its
//! message.

use std::sync::{Arc, Mutex};

use brigadier_providers::{ItemStatus, ProviderEvent, Role};
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
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

/// A message the user sends while a turn runs waits in the queue, as with a plain CLI, and goes
/// as its own turn once that turn ends; nothing asks the thread to judge it (S16b).
#[tokio::test]
async fn a_message_sent_while_a_turn_runs_waits_for_it_in_the_queue() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let inputs: Log = Default::default();
    let (release, log) = (gate.clone(), inputs.clone());
    let flow = Flow::start(
        "honesty-queue-waits",
        Options::default(),
        script(move |turn| {
            let (gate, log) = (gate.clone(), log.clone());
            async move {
                log.lock().unwrap().push(turn.input.clone());
                if !turn.input.contains("Run the tests.") {
                    return Reply::text("Formatted.");
                }
                turn.emit(bash("cargo test --workspace", ItemStatus::InProgress))
                    .await;
                gate.notified().await;
                turn.emit(bash("cargo test --workspace", ItemStatus::Completed))
                    .await;
                Reply::text("The tests pass.")
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
        .until("the message to wait in the queue", |board| {
            board
                .queue
                .items
                .first()
                .is_some_and(|queued| queued.text == "Also check the formatting.")
        })
        .await;
    assert!(
        board
            .requests
            .values()
            .all(|request| request.steered_into.is_none()),
        "nothing joined the turn: {:?}",
        board.requests
    );
    assert_eq!(
        inputs.lock().unwrap().len(),
        1,
        "the thread was told nothing of it"
    );
    release.notify_one();
    let board = flow
        .until("the queued message to go as its own turn", |board| {
            board.queue.items.is_empty() && board.requests.len() == 2
        })
        .await;
    flow.settled().await;
    let inputs = inputs.lock().unwrap().clone();
    assert_eq!(inputs.len(), 2, "{inputs:#?}");
    assert!(
        inputs[1].contains("Also check the formatting."),
        "{inputs:#?}"
    );
    assert!(!inputs[1].contains("[follow-up"), "{inputs:#?}");
    assert!(
        board
            .requests
            .values()
            .all(|request| request.steered_into.is_none())
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
