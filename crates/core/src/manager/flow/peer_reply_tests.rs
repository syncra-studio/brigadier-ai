//! Replies between sessions (SESSION-MESSAGING-PLAN.md §5.2): a reply goes back to the
//! request that asked, which waits for it with nothing polling; one that comes after that
//! request is over is shown and kept for the next turn, starting none.

use std::sync::Arc;
use std::time::Duration;

use brigadier_providers::ProviderKind;
use serde_json::json;
use tokio::sync::Notify;

use super::peer_kit::{Peers, Sent, header_id, message_id, only, reply_sent, send};
use super::{Flow, Options, Reply};
use crate::board::Board;
use crate::model::{ConversationId, Message, PeerState};
use crate::work::RequestState;

/// Whether message `id` stands at `state` on `board`.
fn stands(board: &Board, id: &str, state: &PeerState) -> bool {
    board
        .peer_messages
        .get(id)
        .is_some_and(|record| record.state == *state)
}

/// The peer's message `id` in session `of`'s transcript.
async fn transcript_message(flow: &Flow, of: &ConversationId, id: &str) -> Message {
    flow.core
        .all_messages(of)
        .await
        .unwrap()
        .into_iter()
        .find(|message| message.id == id)
        .expect("the message is in the transcript")
}

/// The id of the session's first request (its first user message).
fn first_request(board: &Board) -> String {
    board
        .requests
        .values()
        .min_by_key(|request| request.started_at_ms)
        .unwrap()
        .id
        .clone()
}

/// A request whose message waits for its reply rests "Waiting for ‹session›", not done; the
/// reply starts a turn under it, and it ends once that turn has.
#[tokio::test]
async fn a_request_waits_for_its_reply_and_ends_after_the_replys_turn() {
    let peers = Peers::new();
    let flow = Flow::start("peer-wait", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let gate = Arc::new(Notify::new());
    let asked = peers.sends(&flow.conversation, &api, "Which column holds names?");
    peers.answers(&api, "It's `name`.", Some(gate.clone()));
    flow.say("Ask the API session where names live.").await;
    let board = flow
        .until("the request waiting for API work", |board| {
            board.requests.values().any(|request| {
                request.state == RequestState::Waiting && request.waiting_for == [api.clone()]
            })
        })
        .await;
    let request = first_request(&board);
    // Answering the reply is the thread's next turn: it works as soon as the reply comes.
    peers.on(&flow.conversation, |_| async {
        Reply::text("Names are in `name`.")
    });
    gate.notify_one();
    flow.until("the reply replied", |board| {
        stands(board, &message_id(&only(&asked).0), &PeerState::Replied)
    })
    .await;
    let board = flow
        .until("the request done", |board| {
            board.requests[&request].state == RequestState::Done
        })
        .await;
    assert!(board.requests[&request].waiting_for.is_empty());
    let answer = reply_sent(&flow, &api, &message_id(&only(&asked).0)).await;
    let inputs = peers.inputs_of(&flow.conversation);
    assert_eq!(inputs.len(), 2, "{inputs:?}");
    assert_eq!(header_id(&inputs[1]).as_deref(), Some(answer.as_str()));
    let message = transcript_message(&flow, &flow.conversation, &answer).await;
    assert_eq!(message.request_id.as_deref(), Some(request.as_str()));
    flow.settled_in(&api).await;
    flow.stop().await;
}

/// A reply that comes while a newer request of the user's runs waits for that turn to end,
/// then gets a turn of its own filed under the request that asked.
#[tokio::test]
async fn a_reply_after_a_newer_request_waits_and_is_filed_under_the_one_that_asked() {
    let peers = Peers::new();
    let flow = Flow::start("peer-later", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Codex)
        .await;
    let gate = Arc::new(Notify::new());
    let asked = peers.sends(&flow.conversation, &api, "Which column holds names?");
    peers.answers(&api, "It's `name`.", Some(gate.clone()));
    flow.say("Ask the API session where names live.").await;
    let board = flow
        .until("the request waiting", |board| {
            board
                .requests
                .values()
                .any(|request| request.state == RequestState::Waiting)
        })
        .await;
    let first = first_request(&board);
    // The user's newer message is a request of its own (the thread sorts it so), whose turn
    // holds until the reply is in.
    let hold = Arc::new(Notify::new());
    let held = hold.clone();
    peers.on(&flow.conversation, move |turn| {
        let held = held.clone();
        async move {
            if let Some(rest) = turn.input.split("[follow-up ").nth(1) {
                let item = rest.split(']').next().unwrap().to_owned();
                let routed = turn
                    .call(
                        "route_follow_up",
                        json!({ "follow_up": item, "joins": false }),
                    )
                    .await;
                assert!(!routed.is_error, "{}", routed.text);
                return Reply::text(crate::manager::prompts::QUIET);
            }
            if header_id(&turn.input).is_some() {
                return Reply::text("Noted: names are in `name`.");
            }
            held.notified().await;
            Reply::text("Renamed the file.")
        }
    });
    flow.say("Meanwhile, rename the file.").await;
    while !peers
        .inputs_of(&flow.conversation)
        .iter()
        .any(|input| input.contains("rename the file") && !input.contains("[follow-up "))
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let turns = peers.inputs_of(&flow.conversation).len();
    gate.notify_one();
    let answer = reply_sent(&flow, &api, &message_id(&only(&asked).0)).await;
    // It waits in the inbox while the newer request's turn runs.
    let board = flow
        .until("the reply delivered", |board| {
            board.peer_messages.contains_key(&answer)
        })
        .await;
    assert!(!stands(&board, &answer, &PeerState::Seen));
    hold.notify_one();
    let board = flow
        .until("the reply seen", |board| {
            stands(board, &answer, &PeerState::Seen)
        })
        .await;
    let board = if board.requests[&first].state == RequestState::Done {
        board
    } else {
        flow.until("the first request done", |board| {
            board.requests[&first].state == RequestState::Done
        })
        .await
    };
    let inputs = peers.inputs_of(&flow.conversation);
    assert_eq!(inputs.len(), turns + 1, "{inputs:?}");
    let last = inputs.last().unwrap();
    assert!(last.contains("[for the user's earlier request:"), "{last}");
    assert_eq!(header_id(last).as_deref(), Some(answer.as_str()));
    let message = transcript_message(&flow, &flow.conversation, &answer).await;
    assert_eq!(message.request_id.as_deref(), Some(first.as_str()));
    assert!(stands(
        &board,
        &message_id(&only(&asked).0),
        &PeerState::Replied
    ));
    flow.settled().await;
    flow.settled_in(&api).await;
    flow.stop().await;
}

/// The user stops a request that waits for a reply: it ends stopped, and a reply that comes
/// later is recorded and shown but starts no turn; the next turn carries it as a note.
#[tokio::test]
async fn a_reply_after_the_users_stop_starts_no_turn_and_joins_the_next() {
    let peers = Peers::new();
    let flow = Flow::start("peer-stop", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let gate = Arc::new(Notify::new());
    let asked = peers.sends(&flow.conversation, &api, "Which column holds names?");
    peers.answers(&api, "It's `name`.", Some(gate.clone()));
    flow.say("Ask the API session where names live.").await;
    let board = flow
        .until("the request waiting", |board| {
            board
                .requests
                .values()
                .any(|request| request.state == RequestState::Waiting)
        })
        .await;
    let request = first_request(&board);
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    flow.until("the request stopped", |board| {
        board.requests[&request].state == RequestState::Stopped
    })
    .await;
    gate.notify_one();
    let answer = reply_sent(&flow, &api, &message_id(&only(&asked).0)).await;
    let board = flow
        .until("the late reply delivered", |board| {
            stands(board, &answer, &PeerState::Delivered)
        })
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        peers.inputs_of(&flow.conversation).len(),
        1,
        "no turn starts"
    );
    assert_eq!(board.requests[&request].state, RequestState::Stopped);
    let message = transcript_message(&flow, &flow.conversation, &answer).await;
    assert_eq!(message.text, "It's `name`.");
    // The user's next message carries it.
    peers.on(&flow.conversation, |_| async {
        Reply::text("Carrying on.")
    });
    flow.say("Carry on with the rename.").await;
    flow.until("the late reply seen", |board| {
        stands(board, &answer, &PeerState::Seen)
    })
    .await;
    let inputs = peers.inputs_of(&flow.conversation);
    assert_eq!(inputs.len(), 2, "{inputs:?}");
    assert!(
        inputs[1].contains("A reply from \"API work\" arrived after you stopped")
            && inputs[1].contains("It's `name`."),
        "{}",
        inputs[1]
    );
    flow.settled().await;
    flow.settled_in(&api).await;
    flow.stop().await;
}

/// A reply must answer a message the replying session got from the session it names.
#[tokio::test]
async fn a_reply_to_a_message_that_wasnt_from_its_target_is_refused() {
    let peers = Peers::new();
    let flow = Flow::start("peer-reply-to", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let docs = flow.add_session("Docs", None, ProviderKind::Claude).await;
    // Docs asks this session something; this session's reply names API work.
    let asked = peers.sends(&docs, &flow.conversation, "Is the API stable?");
    let wrong = Sent::default();
    let (kept, api_id, docs_id) = (wrong.clone(), api.clone(), docs.clone());
    peers.on(&flow.conversation, move |turn| {
        let (kept, api_id, docs_id) = (kept.clone(), api_id.clone(), docs_id.clone());
        async move {
            if let Some(original) = header_id(&turn.input) {
                for (to, reply_to) in [(&api_id, original.as_str()), (&docs_id, "made-up")] {
                    let reply = send(&turn, to, "Yes.", Some(reply_to)).await;
                    kept.lock().unwrap().push(reply);
                }
            }
            Reply::text("Done.")
        }
    });
    flow.say_to(&docs, "Ask the Flow session whether the API is stable.")
        .await;
    flow.settled_in(&docs).await;
    let question = message_id(&only(&asked).0);
    flow.until("both replies refused", |_| wrong.lock().unwrap().len() == 2)
        .await;
    assert_eq!(
        wrong.lock().unwrap().clone(),
        vec![
            ("That message wasn't from API work.".to_owned(), true),
            ("That message wasn't from Docs.".to_owned(), true),
        ]
    );
    // The only reply that went is the turn's own answer, which Brigadier sends back.
    let board = flow
        .until("the answer sent back", |board| {
            board.peer_messages.values().any(|record| record.outgoing)
        })
        .await;
    let sent: Vec<_> = board
        .peer_messages
        .values()
        .filter(|record| record.outgoing)
        .collect();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(sent[0].automatic && sent[0].peer == docs);
    assert_eq!(sent[0].reply_to.as_deref(), Some(question.as_str()));
    assert!(flow.board_of(&api).await.peer_messages.is_empty());
    flow.stop().await;
}

/// `settle_wait` lets a request end without the reply it asked for.
#[tokio::test]
async fn a_settled_wait_lets_the_request_end() {
    let peers = Peers::new();
    let flow = Flow::start("peer-settle", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let gate = Arc::new(Notify::new());
    peers.answers(&api, "It's `name`.", Some(gate.clone()));
    let settled = Sent::default();
    let (kept, to) = (settled.clone(), api.clone());
    peers.on(&flow.conversation, move |turn| {
        let (kept, to) = (kept.clone(), to.clone());
        async move {
            // The reply still comes, and starts a turn of its own: it asks nothing again.
            if header_id(&turn.input).is_some() {
                return Reply::text("Noted.");
            }
            let (reply, _) = send(&turn, &to, "Which column holds names?", None).await;
            let id = message_id(&reply);
            let settle = turn
                .call(
                    "settle_wait",
                    json!({ "message_id": id, "why": "I found it myself." }),
                )
                .await;
            kept.lock().unwrap().push((settle.text, settle.is_error));
            Reply::text("Found it: `name`.")
        }
    });
    flow.say("Find where names live.").await;
    let board = flow.settled().await;
    let (text, failed) = only(&settled);
    assert!(!failed, "{text}");
    assert!(text.contains("API work"), "{text}");
    let request = first_request(&board);
    assert_eq!(board.requests[&request].state, RequestState::Done);
    let record = board.peer_messages.values().next().unwrap();
    assert!(record.settled && !record.awaits_reply());
    gate.notify_one();
    flow.settled_in(&api).await;
    assert_eq!(flow.board_of(&api).await.requests.len(), 1);
    flow.stop().await;
}

/// A reply to a message an overnight run's turn sent, coming after the run ended, is kept
/// for the morning: shown, no turn started, and carried by the user's next message.
#[tokio::test]
async fn a_reply_after_an_overnight_run_ended_waits_for_the_morning() {
    use crate::overnight::{OvernightState, ProposedPhase, ProposedPlan};
    let peers = Peers::new();
    let flow = Flow::start("peer-overnight", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let gate = Arc::new(Notify::new());
    peers.answers(&api, "It's `name`.", Some(gate.clone()));
    let asked = Sent::default();
    let (kept, to) = (asked.clone(), api.clone());
    peers.on(&flow.conversation, move |turn| {
        let (kept, to) = (kept.clone(), to.clone());
        async move {
            if !kept.lock().unwrap().is_empty() {
                return Reply::text("Good morning: the API session was asked.");
            }
            let reply = send(&turn, &to, "Which column holds names?", None).await;
            kept.lock().unwrap().push(reply);
            let ended = turn
                .call(
                    "end_run",
                    json!({ "outcome": "needsUser", "why": "The rest needs the API session." }),
                )
                .await;
            assert!(!ended.is_error, "{}", ended.text);
            Reply::text("Asked; ending the run.")
        }
    });
    let plan = ProposedPlan {
        name: "Names".into(),
        phases: vec![ProposedPhase {
            number: Some(1),
            name: "Find names".into(),
            scope: "Find where names live.".into(),
            done_when: vec!["The column is known.".into()],
            ..Default::default()
        }],
        ..Default::default()
    };
    let run = flow
        .manager
        .propose_overnight(
            flow.conversation.clone(),
            "propose-1".into(),
            "Find where names live overnight.".into(),
            Some(plan),
        )
        .await
        .unwrap();
    flow.manager
        .start_overnight(
            flow.conversation.clone(),
            run.id.clone(),
            "start-1".into(),
            run.revision,
        )
        .await
        .unwrap();
    flow.until("the run finished", |board| {
        board
            .runs
            .get(&run.id)
            .is_some_and(|run| run.state == OvernightState::Finished)
    })
    .await;
    flow.settled().await;
    let turns = peers.inputs_of(&flow.conversation).len();
    gate.notify_one();
    let answer = reply_sent(&flow, &api, &message_id(&only(&asked).0)).await;
    flow.until("the late reply delivered", |board| {
        stands(board, &answer, &PeerState::Delivered)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        peers.inputs_of(&flow.conversation).len(),
        turns,
        "no turn starts"
    );
    transcript_message(&flow, &flow.conversation, &answer).await;
    flow.say("Morning. What did the API session say?").await;
    flow.until("the late reply seen", |board| {
        stands(board, &answer, &PeerState::Seen)
    })
    .await;
    let inputs = peers.inputs_of(&flow.conversation);
    let last = inputs.last().unwrap();
    assert!(
        last.contains("A reply from \"API work\" arrived after the overnight run ended")
            && last.contains("It's `name`."),
        "{last}"
    );
    assert!(stands(
        &flow.board().await,
        &message_id(&only(&asked).0),
        &PeerState::Replied
    ));
    flow.settled().await;
    flow.settled_in(&api).await;
    flow.stop().await;
}

/// A session deleted before it replies never will: what it hadn't taken is dropped as
/// deleted on the sender's side, and the request that waited for it waits for the others
/// only, then ends.
#[tokio::test]
async fn a_request_stops_waiting_for_a_session_deleted_before_it_replied() {
    use super::FakeBehavior;
    let peers = Peers::new();
    let options = Options {
        behavior: Arc::new(FakeBehavior {
            refuse_steers: true,
            ..Default::default()
        }),
        ..Options::default()
    };
    let flow = Flow::start("peer-deleted", options, peers.script()).await;
    // API work takes the message in a turn and asks its user first, so it hasn't answered;
    // Docs is busy, so it waits.
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    peers.on(&api, |turn| async move {
        let asked = turn
            .call(
                "ask_user",
                serde_json::json!({ "questions": [{ "question": "Which table?", "options": [{ "label": "users" }, { "label": "accounts" }], "recommended": 0 }] }),
            )
            .await;
        assert!(!asked.is_error, "{}", asked.text);
        Reply::text("[quiet]")
    });
    let docs = flow.add_session("Docs", None, ProviderKind::Codex).await;
    let hold = Arc::new(Notify::new());
    let held = hold.clone();
    peers.on(&docs, move |turn| {
        let held = held.clone();
        async move {
            if header_id(&turn.input).is_none() {
                held.notified().await;
            }
            Reply::text("Done.")
        }
    });
    flow.say_to(&docs, "Refactor the docs.").await;
    while peers.inputs_of(&docs).is_empty() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let asked = Sent::default();
    let (kept, targets) = (asked.clone(), [api.clone(), docs.clone()]);
    peers.on(&flow.conversation, move |turn| {
        let (kept, targets) = (kept.clone(), targets.clone());
        async move {
            for to in &targets {
                let reply = send(&turn, to, "Which column holds names?", None).await;
                kept.lock().unwrap().push(reply);
            }
            Reply::text("Asked them both.")
        }
    });
    flow.say("Ask both sessions where names live.").await;
    let board = flow
        .until("the request waiting for both", |board| {
            board.requests.values().any(|request| {
                request.state == RequestState::Waiting
                    && request.waiting_for == [api.clone(), docs.clone()]
            })
        })
        .await;
    let request = first_request(&board);
    let (to_api, to_docs) = {
        let asked = asked.lock().unwrap();
        (message_id(&asked[0].0), message_id(&asked[1].0))
    };
    flow.until("API work took it", |board| {
        stands(board, &to_api, &PeerState::Seen)
    })
    .await;
    assert!(stands(&board, &to_docs, &PeerState::Delivered));
    flow.manager.delete(docs.clone()).await.unwrap();
    hold.notify_one();
    let board = flow
        .until("waiting for API work alone", |board| {
            board.requests[&request].waiting_for == [api.clone()]
        })
        .await;
    assert_eq!(board.requests[&request].state, RequestState::Waiting);
    assert!(stands(
        &board,
        &to_docs,
        &PeerState::Dropped {
            reason: "deleted".into()
        }
    ));
    flow.settled_in(&api).await;
    flow.manager.delete(api.clone()).await.unwrap();
    let board = flow
        .until("the request done", |board| {
            board.requests[&request].state == RequestState::Done
        })
        .await;
    assert!(board.requests[&request].waiting_for.is_empty());
    assert!(stands(&board, &to_api, &PeerState::Seen));
    assert_eq!(
        peers.inputs_of(&flow.conversation).len(),
        1,
        "no turn starts"
    );
    flow.stop().await;
}

/// A session archived with the message still in its inbox: it is dropped on both sides,
/// and the request that waited for its reply ends at once.
#[tokio::test]
async fn a_request_stops_waiting_for_a_session_archived_before_it_took_the_message() {
    use super::FakeBehavior;
    let peers = Peers::new();
    let options = Options {
        behavior: Arc::new(FakeBehavior {
            refuse_steers: true,
            ..Default::default()
        }),
        ..Options::default()
    };
    let flow = Flow::start("peer-archived", options, peers.script()).await;
    let docs = flow.add_session("Docs", None, ProviderKind::Codex).await;
    let hold = Arc::new(Notify::new());
    let held = hold.clone();
    peers.on(&docs, move |turn| {
        let held = held.clone();
        async move {
            if header_id(&turn.input).is_none() {
                held.notified().await;
            }
            Reply::text("Done.")
        }
    });
    flow.say_to(&docs, "Refactor the docs.").await;
    while peers.inputs_of(&docs).is_empty() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let asked = peers.sends(&flow.conversation, &docs, "Which column holds names?");
    flow.say("Ask the docs session where names live.").await;
    let board = flow
        .until("the request waiting for Docs", |board| {
            board.requests.values().any(|request| {
                request.state == RequestState::Waiting && request.waiting_for == [docs.clone()]
            })
        })
        .await;
    let request = first_request(&board);
    let id = message_id(&only(&asked).0);
    assert!(stands(&board, &id, &PeerState::Delivered));
    flow.manager.archive(docs.clone()).await.unwrap();
    hold.notify_one();
    let board = flow
        .until("the request done", |board| {
            board.requests[&request].state == RequestState::Done
        })
        .await;
    let dropped = PeerState::Dropped {
        reason: "archived".into(),
    };
    assert!(stands(&board, &id, &dropped));
    assert!(stands(&flow.board_of(&docs).await, &id, &dropped));
    assert_eq!(peers.count_in(&docs, &format!("· id {id}]")), 0);
    flow.stop().await;
}
