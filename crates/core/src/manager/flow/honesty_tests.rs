//! A thread never leaves the user guessing: a request doesn't end on a line that only said
//! what came next (the thread is asked for its answer once, then a plain closing line says
//! none came), and a reply to another session carries only what answers its message.

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

const NARRATION: &str = "Now the full suites, once, on the session branch.";
const REMINDED: &str = "Nothing runs for this request any more, and the user has seen no answer";

/// Says `text` in the turn, then starts a lookup: the line only announced it, so the
/// narration filter hides it.
async fn narrate(turn: &Turn, text: &str) {
    turn.emit(ProviderEvent::Message {
        item_id: uuid::Uuid::new_v4().to_string(),
        role: Role::Assistant,
        text: text.into(),
    })
    .await;
    turn.emit(ProviderEvent::ToolCall {
        item_id: uuid::Uuid::new_v4().to_string(),
        name: "mcp__brigadier__code_search".into(),
        input: Some("{}".into()),
        status: ItemStatus::InProgress,
        output: None,
    })
    .await;
}

/// The 33fbb16e shape: the user's turn starts a check that goes to the background and ends
/// quiet; the turn its result starts says only what comes next and ends quiet. Answered by
/// `answer` when reminded.
fn narrating(inputs: Log, answer: &'static str) -> Script {
    script(move |turn| {
        let inputs = inputs.clone();
        async move {
            inputs.lock().unwrap().push(turn.input.clone());
            if turn.input.contains(REMINDED) {
                return Reply::text(answer);
            }
            if turn.input.contains("[run-1 finished") {
                narrate(&turn, NARRATION).await;
                return Reply::text(QUIET);
            }
            let reply = turn
                .call("run_check", json!({ "command": "sleep 3; echo ok" }))
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text(QUIET)
        }
    })
}

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

/// A request over with only a narration line said doesn't end on it: the thread is asked for
/// its answer, which is the block's last word.
#[tokio::test]
async fn a_request_over_on_a_narration_line_gets_the_threads_answer() {
    let inputs = Log::default();
    let flow = Flow::start(
        "narration-reminded",
        Options::default(),
        narrating(inputs.clone(), "The suites passed; nothing is pending."),
    )
    .await;
    flow.say("Run the full checks.").await;
    let board = flow
        .until("the request to end", |board| {
            !board.requests.is_empty()
                && board
                    .requests
                    .values()
                    .all(|request| request.state == RequestState::Done)
        })
        .await;
    let request = board.requests.keys().next().unwrap().clone();
    let inputs = inputs.lock().unwrap().clone();
    let reminder = inputs
        .iter()
        .find(|input| input.contains(REMINDED))
        .unwrap_or_else(|| panic!("never reminded: {inputs:#?}"));
    assert!(reminder.contains(NARRATION), "{reminder}");
    // The line said before the work shows; the answer is the block's last word.
    let shown = shown(&flow).await;
    assert_eq!(
        shown,
        [
            (NARRATION.to_owned(), Some(request.clone())),
            (
                "The suites passed; nothing is pending.".to_owned(),
                Some(request)
            )
        ],
    );
    flow.stop().await;
}

/// Reminded and still no answer: the request ends with a plain line saying so.
#[tokio::test]
async fn a_request_still_unanswered_after_the_reminder_ends_on_a_plain_line() {
    let inputs = Log::default();
    let flow = Flow::start(
        "narration-unanswered",
        Options::default(),
        narrating(inputs.clone(), QUIET),
    )
    .await;
    flow.say("Run the full checks.").await;
    let board = flow
        .until("the request to end", |board| {
            !board.requests.is_empty()
                && board
                    .requests
                    .values()
                    .all(|request| request.state == RequestState::Done)
                && board
                    .notices
                    .iter()
                    .any(|notice| notice.text.contains("without an answer"))
        })
        .await;
    assert_eq!(
        inputs
            .lock()
            .unwrap()
            .iter()
            .filter(|input| input.contains(REMINDED))
            .count(),
        1,
        "reminded once"
    );
    assert_eq!(board.requests.len(), 1);
    flow.stop().await;
}

/// An idle session that took another's message and ended on a narration line: the request
/// works until the thread answered when reminded, and that answer is the reply, not the
/// narration.
#[tokio::test]
async fn an_idle_receiver_ending_on_narration_replies_with_its_answer() {
    let peers = Peers::new();
    let flow = Flow::start("narration-peer", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let _sent = peers.sends(&flow.conversation, &api, "Which column holds names?");
    peers.on(&api, |turn| async move {
        if turn.input.contains(REMINDED) {
            return Reply::text("It's `name`.");
        }
        narrate(&turn, "Let me look that up in the schema.").await;
        Reply::text(QUIET)
    });
    flow.say("Ask the API session where names live.").await;
    super::eventually("the reply to reach the sender", || {
        peers
            .inputs_of(&flow.conversation)
            .iter()
            .any(|input| is_reply(input))
    })
    .await;
    let reply = peers
        .inputs_of(&flow.conversation)
        .into_iter()
        .find(|input| is_reply(input))
        .unwrap();
    assert!(reply.contains("It's `name`."), "{reply}");
    assert!(!reply.contains("Let me look that up"), "{reply}");
    let at_api = flow.settled_in(&api).await;
    assert!(
        at_api
            .requests
            .values()
            .all(|request| request.state == RequestState::Done),
        "{:?}",
        at_api.requests
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
