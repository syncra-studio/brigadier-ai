//! What the app reads of messages between sessions (SESSION-MESSAGING-PLAN.md §6): each side's
//! records in its conversation view, the receiver's time of its newest one for the sidebar's
//! unread dot, and a row for every send refused before anything was recorded.

use std::sync::Arc;

use brigadier_providers::ProviderKind;

use super::peer_kit::{Peers, message_id, only, send};
use super::{Flow, Options, Reply};
use crate::model::{ConversationId, PeerState};
use crate::work::OrchestratorStepKind;

/// Both views list the message, the sender's with its text and request, the receiver's filed
/// under the request it opened; only the receiver's conversation marks when one came.
#[tokio::test]
async fn each_side_reads_the_message_and_the_receiver_when_it_came() {
    let peers = Peers::new();
    let flow = Flow::start("peer-view", Options::default(), peers.script()).await;
    let api = flow.add_session("API", None, ProviderKind::Codex).await;
    let text = "The schema changed.";
    let sent = peers.sends(&flow.conversation, &api, text);
    flow.say("Tell the API session.").await;
    flow.settled().await;
    let id = message_id(&only(&sent).0);
    flow.until_in(&api, "the message seen", |board| {
        board
            .peer_messages
            .get(&id)
            .is_some_and(|record| record.state == PeerState::Seen)
    })
    .await;
    flow.settled_in(&api).await;

    let mine = flow
        .core
        .conversation_view(flow.conversation.clone(), 50)
        .await
        .unwrap();
    let [record] = mine.peer_messages.as_slice() else {
        panic!("{:?}", mine.peer_messages);
    };
    assert!(record.outgoing);
    assert_eq!(record.message_id, id);
    assert_eq!(record.peer, api);
    assert_eq!(record.text, text);
    assert_eq!(record.state, PeerState::Seen);
    let asked = mine
        .requests
        .iter()
        .find(|request| request.from_peer.is_none())
        .unwrap();
    assert_eq!(record.request_id.as_deref(), Some(asked.id.as_str()));
    assert_eq!(mine.conversation.peer_at_ms, None);

    let theirs = flow.core.conversation_view(api.clone(), 50).await.unwrap();
    let [received] = theirs.peer_messages.as_slice() else {
        panic!("{:?}", theirs.peer_messages);
    };
    assert!(!received.outgoing);
    assert_eq!(received.peer, flow.conversation);
    assert_eq!(received.request_id.as_deref(), Some(id.as_str()));
    let message = theirs
        .messages
        .messages
        .iter()
        .find(|message| message.id == id)
        .unwrap();
    assert_eq!(theirs.conversation.peer_at_ms, Some(message.created_at_ms));
    // A daemon started again on the same store still knows when it came (the event's own time,
    // written with the message), and that the sender received none.
    let reloaded = crate::Core::load(flow.core.store().clone()).await.unwrap();
    let at = reloaded.conversation(&api).unwrap().peer_at_ms.unwrap();
    assert!(
        (message.created_at_ms..message.created_at_ms + 5_000).contains(&at),
        "{at} vs {}",
        message.created_at_ms
    );
    assert_eq!(
        reloaded
            .conversation(&flow.conversation)
            .unwrap()
            .peer_at_ms,
        None
    );
    flow.stop().await;
}

/// Every send the engine refuses before recording it leaves one "Couldn't message" row with
/// the reason the thread heard, naming the session when the id named one; a send that went
/// leaves none.
#[tokio::test]
async fn a_refused_send_leaves_a_row_with_its_reason() {
    let peers = Peers::new();
    let flow = Flow::start("peer-refused-row", Options::default(), peers.script()).await;
    let old = flow.add_session("Old", None, ProviderKind::Claude).await;
    flow.manager.archive(old.clone()).await.unwrap();
    let api = flow.add_session("API", None, ProviderKind::Claude).await;
    let own = flow.conversation.clone();
    let replies: super::peer_kit::Sent = Arc::default();
    let kept = replies.clone();
    let (to_api, to_old) = (api.clone(), old.clone());
    peers.on(&flow.conversation, move |turn| {
        let (kept, api, old, own) = (kept.clone(), to_api.clone(), to_old.clone(), own.clone());
        async move {
            let nope = ConversationId("nope".into());
            for (to, text, reply_to) in [
                (&old, "Hello.", None),
                (&nope, "Hello.", None),
                (&own, "Hello.", None),
                (&api, "   ", None),
                (&api, "Done.", Some("not-a-message")),
                (&api, "Hello.", None),
            ] {
                let reply = send(&turn, to, text, reply_to).await;
                kept.lock().unwrap().push(reply);
            }
            Reply::text("Done.")
        }
    });
    flow.say("Say hello around.").await;
    let board = flow.settled().await;
    let replies = replies.lock().unwrap().clone();
    assert_eq!(
        replies.iter().filter(|(_, failed)| *failed).count(),
        5,
        "{replies:?}"
    );
    let refused: Vec<_> = board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            OrchestratorStepKind::MessageRefused { to, title, reason } => Some((
                to.clone(),
                title.clone(),
                reason.clone(),
                step.request_id.clone(),
            )),
            _ => None,
        })
        .collect();
    let request = board
        .requests
        .values()
        .next()
        .map(|request| request.id.clone());
    assert_eq!(
        refused,
        vec![
            (
                Some(old.clone()),
                Some("Old".to_owned()),
                "Old is archived. Nothing was sent.".to_owned(),
                request.clone()
            ),
            (
                None,
                None,
                "No session with that id.".to_owned(),
                request.clone()
            ),
            (
                Some(flow.conversation.clone()),
                Some("Flow".to_owned()),
                "That's this session.".to_owned(),
                request.clone()
            ),
            (
                Some(api.clone()),
                Some("API".to_owned()),
                "Write the message: its text is empty.".to_owned(),
                request.clone()
            ),
            (
                Some(api.clone()),
                Some("API".to_owned()),
                "That message wasn't from API.".to_owned(),
                request.clone()
            ),
        ]
    );
    assert_eq!(board.peer_messages.len(), 1);
    flow.stop().await;
}
