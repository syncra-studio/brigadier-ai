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

/// A message the user steers in while the thread's command runs says so until the command
/// ends, then the thread reads it. Every write of its request names the running turn's, so no
/// frame ever draws it as a block of its own (D65).
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
    let running = flow.board().await.latest_request().unwrap().id.clone();
    let sent = flow
        .manager
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
    let crate::manager::SendOutcome::Sent(message) = sent else {
        panic!("a steer is sent, never queued: {sent:?}");
    };
    let marks = super::tab_chat_tests::steer_marks(&flow, &message.id).await;
    assert!(
        !marks.is_empty()
            && marks
                .iter()
                .all(|into| into.as_deref() == Some(running.as_str())),
        "every write of its request names the running turn's: {marks:?}"
    );
    let inputs = inputs.lock().unwrap().clone();
    assert!(
        inputs
            .iter()
            .any(|input| input.contains("Use banker's rounding instead.")),
        "{inputs:#?}"
    );
    flow.stop().await;
}
