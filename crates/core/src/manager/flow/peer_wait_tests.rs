//! A thread waiting in its turn for the replies to what it sent (`wait_for_replies`,
//! SESSION-MESSAGING-PLAN.md §5.2): the reply is the waiting call's result, in the same turn;
//! what no wait takes goes by ordinary delivery, across a timeout, a Stop or a crash. The
//! receiver's answer is the reply, which Brigadier sends back.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_providers::{ItemStatus, ProviderKind};
use serde_json::{Value, json};
use tokio::sync::Notify;

use super::peer_kit::{Peers, Sent, header_id, is_reply, message_id, only, send, wait, wait_as};
use super::{Flow, Options, Reply};
use crate::board::{Board, PeerRecord};
use crate::manager::prompts::QUIET;
use crate::model::{ConversationId, MessageRole, PeerState, PermissionLevel, ReplyHold};
use crate::work::{OrchestratorStepKind, RequestState};

fn stands(board: &Board, id: &str, state: &PeerState) -> bool {
    board
        .peer_messages
        .get(id)
        .is_some_and(|record| record.state == *state)
}

/// The reply `board`'s session got to its message `question`.
fn reply_to<'a>(board: &'a Board, question: &str) -> Option<&'a PeerRecord> {
    board
        .peer_messages
        .values()
        .find(|record| !record.outgoing && record.reply_to.as_deref() == Some(question))
}

/// The messages `board`'s session sent.
fn sent_by(board: &Board) -> Vec<&PeerRecord> {
    board
        .peer_messages
        .values()
        .filter(|record| record.outgoing)
        .collect()
}

/// Has each turn of `from` (not one a reply started, which only notes it) send `text` to
/// each of `to`, then wait with `args` until the replies are in, keeping each wait's result,
/// and answer "Names are in `name`.". A turn a reply started answers "Noted.".
fn asks_and_waits(
    peers: &Peers,
    from: &ConversationId,
    to: &[ConversationId],
    text: &str,
    args: Value,
    before_wait: Option<Arc<Notify>>,
) -> (Sent, Sent) {
    let (asked, waited) = (Sent::default(), Sent::default());
    let (kept, results, to, text) = (asked.clone(), waited.clone(), to.to_vec(), text.to_owned());
    peers.on(from, move |turn| {
        let (kept, results, to, text, args, before_wait) = (
            kept.clone(),
            results.clone(),
            to.clone(),
            text.clone(),
            args.clone(),
            before_wait.clone(),
        );
        async move {
            if is_reply(&turn.input) {
                return Reply::text("Noted.");
            }
            for to in &to {
                let reply = send(&turn, to, &text, None).await;
                kept.lock().unwrap().push(reply);
            }
            if let Some(gate) = before_wait {
                gate.notified().await;
            }
            loop {
                let result = wait(&turn, args.clone()).await;
                let again = result.0.contains("Call wait_for_replies again");
                results.lock().unwrap().push(result);
                if !again {
                    break;
                }
            }
            Reply::text("Names are in `name`.")
        }
    });
    (asked, waited)
}

/// Has each turn of `of` that took a message answer `text` once `gate` (if any) lets it.
fn answers(peers: &Peers, of: &ConversationId, text: &str, gate: Option<Arc<Notify>>) {
    let text = text.to_owned();
    peers.on(of, move |turn| {
        let (text, gate) = (text.clone(), gate.clone());
        async move {
            if header_id(&turn.input).is_some()
                && let Some(gate) = gate
            {
                gate.notified().await;
            }
            Reply::text(&text)
        }
    });
}

/// The thread sends, waits, and the other session's answer comes back as the wait's result:
/// one turn, no turn of its own for the reply, which is taken and seen. The receiver's answer
/// is the reply, sent back by Brigadier and pointing at the answer it carries.
#[tokio::test]
async fn the_reply_comes_back_inside_the_turn_that_waits() {
    let peers = Peers::new();
    let flow = Flow::start("peer-wait-in-turn", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Codex)
        .await;
    let (asked, waited) = asks_and_waits(
        &peers,
        &flow.conversation,
        std::slice::from_ref(&api),
        "Which column holds names?",
        json!({}),
        None,
    );
    answers(&peers, &api, "It's `name`.", None);
    flow.say("Ask the API session where names live.").await;
    flow.until("the wait's result", |_| !waited.lock().unwrap().is_empty())
        .await;
    let board = flow.settled().await;
    let question = message_id(&only(&asked).0);
    let answer = reply_to(&board, &question).expect("the reply").clone();
    let (result, failed) = only(&waited);
    assert!(!failed, "{result}");
    assert_eq!(
        result,
        format!(
            "[reply from session \"API work\" · session {api} · id {} · reply to {question}]\nIt's `name`.",
            answer.message_id
        )
    );
    assert_eq!(
        peers.inputs_of(&flow.conversation).len(),
        1,
        "no turn of its own"
    );
    assert!(stands(&board, &question, &PeerState::Replied));
    assert_eq!(answer.state, PeerState::Seen);
    assert!(answer.automatic);
    assert!(matches!(answer.hold, Some(ReplyHold::Taken { .. })));
    let request = board.requests.values().next().unwrap();
    assert_eq!(request.state, RequestState::Done);
    // The receiver: its answer went back, once, pointing at the answer message.
    let at_api = flow.settled_in(&api).await;
    assert!(stands(&at_api, &question, &PeerState::Replied));
    let back = sent_by(&at_api);
    assert_eq!(back.len(), 1, "{back:?}");
    assert!(back[0].automatic && back[0].message_id == answer.message_id);
    let said = flow
        .core
        .all_messages(&api)
        .await
        .unwrap()
        .into_iter()
        .find(|message| message.role == MessageRole::Assistant && message.text == "It's `name`.")
        .expect("the receiver's answer");
    assert_eq!(back[0].answer_id.as_deref(), Some(said.id.as_str()));
    flow.stop().await;
}

/// A wait the CLI shows under its server's prefix (Codex's `brigadier/wait_for_replies`)
/// takes its reply as Claude's does: the reply is taken and seen, and no turn of its own
/// brings it a second time.
#[tokio::test]
async fn a_wait_named_with_its_servers_prefix_takes_the_reply() {
    let peers = Peers::new();
    let flow = Flow::start("peer-wait-prefixed", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let (asked, waited) = (Sent::default(), Sent::default());
    let (kept, results, to) = (asked.clone(), waited.clone(), api.clone());
    peers.on(&flow.conversation, move |turn| {
        let (kept, results, to) = (kept.clone(), results.clone(), to.clone());
        async move {
            if is_reply(&turn.input) {
                return Reply::text("Noted.");
            }
            let sent = send(&turn, &to, "Which column holds names?", None).await;
            kept.lock().unwrap().push(sent);
            let result = wait_as(&turn, json!({}), "brigadier/wait_for_replies").await;
            results.lock().unwrap().push(result);
            Reply::text("Names are in `name`.")
        }
    });
    answers(&peers, &api, "It's `name`.", None);
    flow.say("Ask the API session where names live.").await;
    flow.until("the wait's result", |_| !waited.lock().unwrap().is_empty())
        .await;
    let board = flow.settled().await;
    let (result, failed) = only(&waited);
    assert!(!failed && result.ends_with("\nIt's `name`."), "{result}");
    let question = message_id(&only(&asked).0);
    let answer = reply_to(&board, &question).expect("the reply");
    assert!(
        matches!(answer.hold, Some(ReplyHold::Taken { .. })),
        "{:?}",
        answer.hold
    );
    assert_eq!(answer.state, PeerState::Seen);
    assert_eq!(
        peers.inputs_of(&flow.conversation).len(),
        1,
        "no turn of its own"
    );
    flow.stop().await;
}

/// A reply that comes before the thread starts waiting is held for its wait, which returns
/// it at once.
#[tokio::test]
async fn a_reply_before_the_wait_is_held_for_it() {
    let peers = Peers::new();
    let flow = Flow::start("peer-wait-early", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let ready = Arc::new(Notify::new());
    let (asked, waited) = asks_and_waits(
        &peers,
        &flow.conversation,
        std::slice::from_ref(&api),
        "Which column holds names?",
        json!({}),
        Some(ready.clone()),
    );
    answers(&peers, &api, "It's `name`.", None);
    flow.say("Ask the API session where names live.").await;
    while asked.lock().unwrap().is_empty() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let question = message_id(&only(&asked).0);
    flow.until("the reply in", |board| reply_to(board, &question).is_some())
        .await;
    // Nothing of it reached the turn on its own.
    tokio::time::sleep(Duration::from_millis(300)).await;
    ready.notify_one();
    flow.until("the wait's result", |_| !waited.lock().unwrap().is_empty())
        .await;
    let board = flow.settled().await;
    let (result, _) = only(&waited);
    assert!(result.ends_with("\nIt's `name`."), "{result}");
    assert_eq!(peers.inputs_of(&flow.conversation).len(), 1);
    let answer = reply_to(&board, &question).unwrap();
    assert!(matches!(answer.hold, Some(ReplyHold::Taken { .. })));
    assert_eq!(answer.state, PeerState::Seen);
    flow.stop().await;
}

/// No reply in time: the wait says who still has to reply and the turn ends; its request
/// waits, and the reply, when it comes, starts a turn of its own (the fallback). That reply
/// is answered by nothing: a reply never gets an automatic reply, so the chain ends.
#[tokio::test]
async fn a_wait_that_times_out_leaves_the_reply_to_ordinary_delivery() {
    let peers = Peers::new();
    let flow = Flow::start("peer-wait-timeout", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let gate = Arc::new(Notify::new());
    let (asked, waited) = asks_and_waits(
        &peers,
        &flow.conversation,
        std::slice::from_ref(&api),
        "Which column holds names?",
        json!({ "timeout_s": 1 }),
        None,
    );
    answers(&peers, &api, "It's `name`.", Some(gate.clone()));
    flow.say("Ask the API session where names live.").await;
    let board = flow
        .until("the request waiting for API work", |board| {
            board.requests.values().any(|request| {
                request.state == RequestState::Waiting && request.waiting_for == [api.clone()]
            })
        })
        .await;
    let request = board.requests.keys().next().unwrap().clone();
    let question = message_id(&only(&asked).0);
    let (result, failed) = only(&waited);
    assert!(!failed, "{result}");
    assert!(
        result.starts_with("No reply yet after 1 s. Still to reply: \"API work\" (id "),
        "{result}"
    );
    gate.notify_one();
    let board = flow
        .until("the request done", |board| {
            board.requests[&request].state == RequestState::Done
        })
        .await;
    let answer = reply_to(&board, &question).unwrap();
    assert_eq!(answer.hold, None);
    let inputs = peers.inputs_of(&flow.conversation);
    assert_eq!(inputs.len(), 2, "{inputs:?}");
    assert_eq!(
        header_id(&inputs[1]).as_deref(),
        Some(answer.message_id.as_str())
    );
    // The chain ends: the reply's turn sent nothing back, and the receiver got nothing more.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let board = flow.settled().await;
    assert_eq!(sent_by(&board).len(), 1, "only the question");
    let at_api = flow.settled_in(&api).await;
    assert_eq!(at_api.peer_messages.len(), 2, "the question and its answer");
    assert_eq!(peers.inputs_of(&api).len(), 1);
    flow.stop().await;
}

/// The user stops the turn while it waits: the wait ends with it, the request is stopped,
/// and the reply that comes later starts no turn (the next one carries it).
#[tokio::test]
async fn a_stop_ends_the_wait_and_the_late_reply_starts_no_turn() {
    let peers = Peers::new();
    let flow = Flow::start("peer-wait-stop", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Codex)
        .await;
    let gate = Arc::new(Notify::new());
    let waiting = Arc::new(Notify::new());
    let (asked, to, at) = (Sent::default(), api.clone(), waiting.clone());
    let kept = asked.clone();
    peers.on(&flow.conversation, move |turn| {
        let (kept, to, at) = (kept.clone(), to.clone(), at.clone());
        async move {
            if !turn.input.contains("Ask the API") {
                return Reply::text("Carrying on.");
            }
            let sent = send(&turn, &to, "Which column holds names?", None).await;
            kept.lock().unwrap().push(sent);
            at.notify_one();
            // The CLI drops the call on Stop, as it does a tool call it cancels.
            tokio::select! {
                _ = wait(&turn, json!({})) => panic!("the wait ended on its own"),
                () = async { while !turn.stopped().await {} } => {}
            }
            Reply::text("")
        }
    });
    answers(&peers, &api, "It's `name`.", Some(gate.clone()));
    flow.say("Ask the API session where names live.").await;
    waiting.notified().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    let board = flow
        .until("the request stopped", |board| {
            board
                .requests
                .values()
                .any(|request| request.state == RequestState::Stopped)
        })
        .await;
    let request = board.requests.keys().next().unwrap().clone();
    // The wait's step ends with the turn, though the CLI never said the call ended (#61).
    // It reads stopped, as of the Stop, not failed at its start.
    let waits: Vec<(ItemStatus, bool, bool)> = flow
        .settled()
        .await
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            OrchestratorStepKind::Tool {
                name,
                status,
                stopped,
                ended_at_ms,
                ..
            } if name == "wait_for_replies" => Some((
                *status,
                *stopped,
                ended_at_ms.is_some_and(|ended| ended > step.at_ms),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        waits.last(),
        Some(&(ItemStatus::Failed, true, true)),
        "{waits:?}"
    );
    let question = message_id(&only(&asked).0);
    gate.notify_one();
    let board = flow
        .until("the late reply delivered", |board| {
            reply_to(board, &question).is_some_and(|reply| reply.state == PeerState::Delivered)
        })
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(peers.inputs_of(&flow.conversation).len(), 1, "no turn");
    assert_eq!(board.requests[&request].state, RequestState::Stopped);
    flow.say("Carry on.").await;
    let answer = reply_to(&board, &question).unwrap().message_id.clone();
    flow.until("the late reply seen", |board| {
        stands(board, &answer, &PeerState::Seen)
    })
    .await;
    let inputs = peers.inputs_of(&flow.conversation);
    assert!(inputs[1].contains("It's `name`."), "{}", inputs[1]);
    flow.settled().await;
    flow.stop().await;
}

/// The daemon dies after a wait was handed the reply, before the CLI showed it took the
/// result: the next launch delivers the reply the ordinary way, once.
#[tokio::test]
async fn a_reply_handed_to_a_wait_the_cli_never_finished_is_delivered_after_a_crash() {
    let peers = Peers::new();
    let mut flow = Flow::start("peer-wait-crash-1", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let handed = Arc::new(Notify::new());
    let (asked, to, at) = (Sent::default(), api.clone(), handed.clone());
    let kept = asked.clone();
    peers.on(&flow.conversation, move |turn| {
        let (kept, to, at) = (kept.clone(), to.clone(), at.clone());
        async move {
            if is_reply(&turn.input) {
                return Reply::text("Noted.");
            }
            let sent = send(&turn, &to, "Which column holds names?", None).await;
            kept.lock().unwrap().push(sent);
            // The result reaches the call, and the daemon dies before the CLI says so.
            let result = turn.call("wait_for_replies", json!({})).await;
            assert!(result.text.contains("It's `name`."), "{}", result.text);
            at.notify_one();
            std::future::pending::<()>().await;
            Reply::text("")
        }
    });
    answers(&peers, &api, "It's `name`.", None);
    flow.say("Ask the API session where names live.").await;
    handed.notified().await;
    let question = message_id(&only(&asked).0);
    let board = flow
        .until("the reply handed", |board| {
            reply_to(board, &question).is_some_and(|reply| reply.hold == Some(ReplyHold::Handed))
        })
        .await;
    let answer = reply_to(&board, &question).unwrap().message_id.clone();
    flow.crash().await;
    flow.until("the reply seen", |board| {
        stands(board, &answer, &PeerState::Seen)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        peers.count_in(&flow.conversation, &format!("· id {answer} ")),
        1
    );
    let board = flow.settled().await;
    assert_eq!(
        reply_to(&board, &question).unwrap().hold,
        Some(ReplyHold::Returned)
    );
    flow.stop().await;
}

/// The daemon dies after the CLI showed it took the wait's result: the reply is never
/// delivered again.
#[tokio::test]
async fn a_reply_a_wait_took_before_a_crash_is_not_sent_again() {
    let peers = Peers::new();
    let mut flow = Flow::start("peer-wait-crash-2", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Codex)
        .await;
    let took = Arc::new(Notify::new());
    let (asked, to, at) = (Sent::default(), api.clone(), took.clone());
    let kept = asked.clone();
    peers.on(&flow.conversation, move |turn| {
        let (kept, to, at) = (kept.clone(), to.clone(), at.clone());
        async move {
            if is_reply(&turn.input) {
                return Reply::text("Noted.");
            }
            let sent = send(&turn, &to, "Which column holds names?", None).await;
            kept.lock().unwrap().push(sent);
            wait(&turn, json!({})).await;
            at.notify_one();
            std::future::pending::<()>().await;
            Reply::text("")
        }
    });
    answers(&peers, &api, "It's `name`.", None);
    flow.say("Ask the API session where names live.").await;
    took.notified().await;
    let question = message_id(&only(&asked).0);
    let board = flow
        .until("the reply taken", |board| {
            reply_to(board, &question)
                .is_some_and(|reply| matches!(reply.hold, Some(ReplyHold::Taken { .. })))
        })
        .await;
    let answer = reply_to(&board, &question).unwrap().message_id.clone();
    // Seen before it was recorded taken.
    assert!(stands(&board, &answer, &PeerState::Seen));
    flow.crash().await;
    flow.settled().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        peers.count_in(&flow.conversation, &format!("· id {answer} ")),
        0
    );
    flow.stop().await;
}

/// Two sessions asked, one slow: the first wait returns the fast one's reply and names the
/// other; the thread waits again and gets it, all in the same turn.
#[tokio::test]
async fn a_second_wait_takes_the_slower_of_two_replies() {
    let peers = Peers::new();
    let flow = Flow::start("peer-wait-two", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Codex)
        .await;
    let docs = flow.add_session("Docs", None, ProviderKind::Claude).await;
    let gate = Arc::new(Notify::new());
    let (asked, waited) = asks_and_waits(
        &peers,
        &flow.conversation,
        &[api.clone(), docs.clone()],
        "Which column holds names?",
        json!({}),
        None,
    );
    answers(&peers, &api, "It's `name`.", None);
    answers(&peers, &docs, "The docs say `name`.", Some(gate.clone()));
    flow.say("Ask both sessions where names live.").await;
    flow.until("the first wait's result", |_| {
        !waited.lock().unwrap().is_empty()
    })
    .await;
    let first = waited.lock().unwrap()[0].0.clone();
    let asked: Vec<String> = asked
        .lock()
        .unwrap()
        .iter()
        .map(|(reply, _)| message_id(reply))
        .collect();
    assert!(first.contains("It's `name`."), "{first}");
    assert!(
        first.ends_with(&format!(
            "Still to reply: \"Docs\" (id {}). Call wait_for_replies again for them.",
            asked[1]
        )),
        "{first}"
    );
    gate.notify_one();
    let board = flow
        .until("the request done", |board| {
            board
                .requests
                .values()
                .all(|request| request.state == RequestState::Done)
                && !board.requests.is_empty()
        })
        .await;
    let waits = waited.lock().unwrap().clone();
    assert_eq!(waits.len(), 2, "{waits:?}");
    assert!(
        waits[1].0.ends_with("\nThe docs say `name`."),
        "{}",
        waits[1].0
    );
    assert_eq!(peers.inputs_of(&flow.conversation).len(), 1);
    for question in &asked {
        assert!(stands(&board, question, &PeerState::Replied));
    }
    flow.stop().await;
}

/// A receiver that replies itself (`reply_to`) to the message that opened its request is
/// refused, plainly: its answer is the reply, which Brigadier sends, so the message is
/// replied once, with what it wrote.
#[tokio::test]
async fn a_reply_to_the_message_that_opened_the_request_is_refused() {
    let peers = Peers::new();
    let flow = Flow::start("peer-wait-explicit", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let (asked, waited) = asks_and_waits(
        &peers,
        &flow.conversation,
        std::slice::from_ref(&api),
        "Which column holds names?",
        json!({}),
        None,
    );
    let refused = Sent::default();
    let kept = refused.clone();
    peers.on(&api, move |turn| {
        let kept = kept.clone();
        async move {
            let Some(original) = header_id(&turn.input) else {
                return Reply::text("Done.");
            };
            let from = super::peer_kit::header_session(&turn.input).unwrap();
            let reply = send(&turn, &from, "It's `name`.", Some(&original)).await;
            kept.lock().unwrap().push(reply);
            Reply::text("It's `name`.")
        }
    });
    flow.say("Ask the API session where names live.").await;
    flow.until("the wait's result", |_| !waited.lock().unwrap().is_empty())
        .await;
    flow.settled().await;
    let at_api = flow.settled_in(&api).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        only(&refused),
        (
            "Write your answer as your reply; Brigadier sends it back to Flow. Nothing was sent."
                .to_owned(),
            true
        )
    );
    let question = message_id(&only(&asked).0);
    let back = sent_by(&at_api);
    assert_eq!(back.len(), 1, "{back:?}");
    assert!(back[0].automatic);
    assert_eq!(back[0].reply_to.as_deref(), Some(question.as_str()));
    assert!(stands(&at_api, &question, &PeerState::Replied));
    assert!(only(&waited).0.ends_with("\nIt's `name`."));
    flow.stop().await;
}

/// Two messages steered into one running turn: told to, the thread answers each on its own,
/// in the turn, and each sender gets that reply, not the turn's answer to the user.
#[tokio::test]
async fn each_message_steered_into_one_turn_gets_its_reply() {
    let peers = Peers::new();
    let flow = Flow::start("peer-wait-steered", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Codex)
        .await;
    let docs = flow.add_session("Docs", None, ProviderKind::Claude).await;
    let read: Arc<Mutex<Vec<String>>> = Arc::default();
    let kept = read.clone();
    let started = Arc::new(Notify::new());
    let at = started.clone();
    peers.on(&flow.conversation, move |turn| {
        let (kept, at) = (kept.clone(), at.clone());
        async move {
            at.notify_one();
            while kept.lock().unwrap().len() < 2 {
                let Some(steer) = turn.steered().await else {
                    break;
                };
                assert!(
                    steer.contains("answer it on its own now with message_session"),
                    "{steer}"
                );
                let id = header_id(&steer).unwrap();
                let from = super::peer_kit::header_session(&steer).unwrap();
                let (_, failed) = send(&turn, &from, "Names are in `name`.", Some(&id)).await;
                assert!(!failed);
                kept.lock().unwrap().push(steer);
            }
            Reply::text("The models are refactored.")
        }
    });
    let from_api = peers.sends(&api, &flow.conversation, "Where do names live?");
    let from_docs = peers.sends(&docs, &flow.conversation, "Which column holds names?");
    flow.say("Refactor the models.").await;
    started.notified().await;
    flow.say_to(&api, "Ask Flow.").await;
    flow.until_in(&api, "sent", |_| !from_api.lock().unwrap().is_empty())
        .await;
    flow.say_to(&docs, "Ask Flow.").await;
    // The script notes a message once its reply is sent, a moment after the board has it.
    let board = flow
        .until("both answered", |board| {
            sent_by(board).len() == 2
                && read.lock().unwrap().len() == 2
                && !from_api.lock().unwrap().is_empty()
                && !from_docs.lock().unwrap().is_empty()
        })
        .await;
    for (from, sent) in [(&api, &from_api), (&docs, &from_docs)] {
        let question = message_id(&only(sent).0);
        let back = sent_by(&board)
            .into_iter()
            .find(|record| record.reply_to.as_deref() == Some(question.as_str()))
            .expect("its own reply");
        assert!(!back.automatic && back.peer == *from);
        assert_eq!(back.text, "Names are in `name`.");
    }
    flow.settled_in(&api).await;
    flow.settled_in(&docs).await;
    flow.stop().await;
}

/// The receiver's turn ends on [quiet] with a worker still running: no interim reply goes.
/// The worker's report brings the answer, which is the reply.
#[tokio::test]
async fn no_reply_goes_while_the_receivers_worker_runs() {
    let peers = Peers::new();
    let flow = Flow::start("peer-wait-worker", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let (asked, waited) = asks_and_waits(
        &peers,
        &flow.conversation,
        std::slice::from_ref(&api),
        "Which column holds names?",
        json!({}),
        None,
    );
    peers.on(&api, |turn| async move {
        if turn.input.contains("[report task-1") {
            return Reply::text("It's `name`, the worker found.");
        }
        let reply = turn
            .call(
                "delegate_task",
                json!({"effort": "high", "title": "Look", "kind": "scout",
                       "spec": "Find the names column."}),
            )
            .await;
        assert!(!reply.is_error, "{}", reply.text);
        Reply::text(QUIET)
    });
    let gate = Arc::new(Notify::new());
    let held = gate.clone();
    peers.worker(move |turn| {
        let held = held.clone();
        async move {
            held.notified().await;
            let reply = turn
                .call("submit_report", json!({"summary": "Names are in `name`."}))
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        }
    });
    flow.say("Ask the API session where names live.").await;
    flow.until_in(&api, "the worker running, the turn over", |board| {
        !board.tasks.is_empty() && peers.inputs_of(&api).len() == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let question = message_id(&only(&asked).0);
    assert!(
        reply_to(&flow.board().await, &question).is_none(),
        "no interim reply"
    );
    assert!(waited.lock().unwrap().is_empty());
    gate.notify_one();
    flow.until("the wait's result", |_| !waited.lock().unwrap().is_empty())
        .await;
    let (result, _) = only(&waited);
    assert!(
        result.ends_with("\nIt's `name`, the worker found."),
        "{result}"
    );
    let at_api = flow.settled_in(&api).await;
    assert_eq!(sent_by(&at_api).len(), 1);
    flow.settled().await;
    flow.stop().await;
}

/// Under Ask for approval the wait holds through the card: once the user sends the message,
/// the reply comes back inside the same turn.
#[tokio::test]
async fn a_wait_holds_through_the_send_card() {
    let peers = Peers::new();
    let options = Options {
        permission: PermissionLevel::AskForApproval,
        ..Options::default()
    };
    let flow = Flow::start("peer-wait-ask", options, peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Codex)
        .await;
    let (asked, waited) = asks_and_waits(
        &peers,
        &flow.conversation,
        std::slice::from_ref(&api),
        "Which column holds names?",
        json!({}),
        None,
    );
    answers(&peers, &api, "It's `name`.", None);
    flow.say("Ask the API session where names live.").await;
    let board = flow
        .until("the send's card", |board| {
            board
                .approvals
                .values()
                .any(|approval| approval.state == crate::work::CardState::Pending)
        })
        .await;
    let card = board.approvals.values().next().unwrap().id.clone();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(waited.lock().unwrap().is_empty(), "the wait holds");
    assert!(peers.inputs_of(&api).is_empty());
    flow.manager
        .answer_card(
            flow.conversation.clone(),
            card,
            brigadier_providers::ApprovalDecision::Allow,
        )
        .await
        .unwrap();
    flow.until("the wait's result", |_| !waited.lock().unwrap().is_empty())
        .await;
    let (result, _) = only(&waited);
    assert!(result.ends_with("\nIt's `name`."), "{result}");
    let question = message_id(&only(&asked).0);
    let board = flow.settled().await;
    assert!(stands(&board, &question, &PeerState::Replied));
    assert_eq!(peers.inputs_of(&flow.conversation).len(), 1);
    flow.settled_in(&api).await;
    flow.stop().await;
}

/// A peer wait is visible while the sender's final answer is still streaming.
#[tokio::test]
async fn an_outstanding_peer_wait_stays_on_a_running_answer() {
    let peers = Peers::new();
    let flow = Flow::start("peer-wait-streaming", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Codex)
        .await;
    let peer_gate = Arc::new(Notify::new());
    answers(&peers, &api, "It's `name`.", Some(peer_gate.clone()));
    let answer_gate = Arc::new(Notify::new());
    let (to, gate) = (api.clone(), answer_gate.clone());
    peers.on(&flow.conversation, move |turn| {
        let (to, gate) = (to.clone(), gate.clone());
        async move {
            if is_reply(&turn.input) {
                return Reply::text("Noted.");
            }
            let (_, failed) = send(&turn, &to, "Which column holds names?", None).await;
            assert!(!failed);
            turn.emit(brigadier_providers::ProviderEvent::MessageDelta {
                item_id: "answer".into(),
                text: "The work is ready; awaiting the column name. ".repeat(12),
                phase: Some(brigadier_providers::TextPhase::Answer),
            })
            .await;
            gate.notified().await;
            Reply::text("The work is ready; awaiting the column name.")
        }
    });
    flow.say("Ask about the column and answer meanwhile.").await;
    let board = flow
        .until("the final answer streaming", |board| {
            board
                .streaming
                .as_ref()
                .is_some_and(|text| text.phase == Some(brigadier_providers::TextPhase::Answer))
        })
        .await;
    let request = board.latest_request().unwrap();
    assert_eq!(request.state, RequestState::Working);
    assert_eq!(request.waiting_for, vec![api.clone()]);
    answer_gate.notify_one();
    let board = flow
        .until("the turn resting on its peer", |board| {
            board.latest_request().unwrap().state == RequestState::Waiting
        })
        .await;
    assert_eq!(board.latest_request().unwrap().waiting_for, vec![api]);
    peer_gate.notify_one();
    let board = flow
        .until("the reply settling its wait", |board| {
            board.latest_request().unwrap().state == RequestState::Done
        })
        .await;
    assert!(board.latest_request().unwrap().waiting_for.is_empty());
    flow.stop().await;
}
