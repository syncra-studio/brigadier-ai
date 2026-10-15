//! The records of messages between sessions (SESSION-MESSAGING-PLAN.md §4): each send is one
//! transaction over both sessions' streams, so nothing is ever shown without also being due.

use super::{INLINE_TEXT_BYTES, PREVIEW_BYTES, prefix, preview};
use crate::model::{
    ConversationId, DomainEvent, MessageRole, PeerRef, PeerState, ReplyHold, streams,
};
use crate::work::{RequestState, UserRequest, WorkSpan};
use crate::{Core, Result, now_ms};

/// A message from one session to another, as its records hold it.
#[derive(Debug, Clone)]
pub(crate) struct PeerSend {
    pub message_id: String,
    pub from: ConversationId,
    pub to: ConversationId,
    pub text: String,
    pub reply_to: Option<String>,
    pub chain_id: String,
    pub hop: u32,
    /// The sender's request whose turn sent it.
    pub request_id: Option<String>,
    /// The newest input of that turn before the send.
    pub turn_input_id: Option<String>,
    /// Brigadier sends it for the session: its answer to `reply_to`.
    pub automatic: bool,
    /// An automatic reply: the answer message it carries.
    pub answer_id: Option<String>,
}

impl Core {
    /// Appends events to any streams in one transaction and applies those on a conversation's
    /// stream to that conversation's board (a message between two sessions changes both).
    pub(crate) async fn record_on_boards(
        &self,
        events: Vec<(String, DomainEvent)>,
    ) -> Result<Vec<i64>> {
        let mut boards = self.boards.lock().await;
        let owners: Vec<Option<ConversationId>> = events
            .iter()
            .map(|(stream, _)| {
                stream
                    .strip_prefix("conversation:")
                    .map(|id| ConversationId(id.to_owned()))
            })
            .collect();
        for id in owners.iter().flatten() {
            if !boards.contains_key(id) {
                let board = self.load_board(id).await?;
                boards.insert(id.clone(), board);
            }
        }
        let stored = self.record(events.clone()).await?;
        for (((_, event), stream_seq), owner) in events.iter().zip(&stored).zip(&owners) {
            if let Some(board) = owner.as_ref().and_then(|id| boards.get_mut(id)) {
                board.apply(event, *stream_seq);
            }
        }
        Ok(stored)
    }

    /// Records a send in one transaction: the sender's `Sent` and, unless it waits for the
    /// user's OK, everything of the receiver's ([`Self::receipt`]). Returns the receiver's
    /// request it is filed under (none while it waits).
    pub(crate) async fn record_peer_send(
        &self,
        send: &PeerSend,
        waits_for_ok: bool,
    ) -> Result<Option<String>> {
        let now = now_ms();
        let (text, blob) = if send.text.len() > INLINE_TEXT_BYTES {
            let hash = self
                .store
                .blobs()
                .put(send.text.clone().into_bytes())
                .await?;
            (
                prefix(&send.text, PREVIEW_BYTES).to_owned(),
                Some(hash.to_string()),
            )
        } else {
            (send.text.clone(), None)
        };
        let mut events = vec![(
            streams::conversation(&send.from),
            DomainEvent::PeerMessageSent {
                message_id: send.message_id.clone(),
                to: send.to.clone(),
                text,
                blob,
                reply_to: send.reply_to.clone(),
                chain_id: send.chain_id.clone(),
                hop: send.hop,
                request_id: send.request_id.clone(),
                turn_input_id: send.turn_input_id.clone(),
                state: if waits_for_ok {
                    PeerState::WaitingForOk
                } else {
                    PeerState::Pending
                },
                sent_at_ms: (!waits_for_ok).then_some(now),
                automatic: send.automatic,
                answer_id: send.answer_id.clone(),
            },
        )];
        if waits_for_ok {
            self.record_on_boards(events).await?;
            return Ok(None);
        }
        let (receipt, request) = self.receipt(send).await?;
        events.extend(receipt);
        if send.automatic
            && let Some(original) = &send.reply_to
        {
            events.push((
                streams::conversation(&send.from),
                DomainEvent::PeerAnswered {
                    conversation_id: send.from.clone(),
                    message_id: original.clone(),
                },
            ));
        }
        self.record_on_boards(events).await?;
        Ok(Some(request))
    }

    /// The user's Send of a send that waited for it: the receiver's side, and the message
    /// pending on both, allowed to go now. Returns the receiver's request.
    pub(crate) async fn release_peer_send(&self, send: &PeerSend) -> Result<String> {
        let (mut events, request) = self.receipt(send).await?;
        events.push((
            streams::conversation(&send.from),
            DomainEvent::PeerMessageState {
                message_id: send.message_id.clone(),
                state: PeerState::Pending,
                sent_at_ms: Some(now_ms()),
            },
        ));
        self.record_on_boards(events).await?;
        Ok(request)
    }

    /// Records `state` of message `message_id` on each of `sides` that still exists.
    pub(crate) async fn record_peer_state(
        &self,
        message_id: &str,
        sides: &[&ConversationId],
        state: PeerState,
    ) -> Result<()> {
        let events: Vec<(String, DomainEvent)> = sides
            .iter()
            .filter(|side| self.conversation(side).is_ok_and(|c| !c.deleting))
            .map(|side| {
                (
                    streams::conversation(side),
                    DomainEvent::PeerMessageState {
                        message_id: message_id.to_owned(),
                        state: state.clone(),
                        sent_at_ms: None,
                    },
                )
            })
            .collect();
        if events.is_empty() {
            return Ok(());
        }
        self.record_on_boards(events).await.map(drop)
    }

    /// Records that the thread of `id` no longer waits for a reply to `message_id`.
    pub(crate) async fn settle_peer_wait(
        &self,
        id: &ConversationId,
        message_id: &str,
    ) -> Result<()> {
        self.record_on_boards(vec![(
            streams::conversation(id),
            DomainEvent::PeerWaitSettled {
                message_id: message_id.to_owned(),
            },
        )])
        .await
        .map(drop)
    }

    /// Records where reply `message_id`, received by `id`, stands with a wait of its thread.
    pub(crate) async fn record_reply_hold(
        &self,
        id: &ConversationId,
        message_id: &str,
        hold: ReplyHold,
    ) -> Result<()> {
        self.record_on_boards(vec![(
            streams::conversation(id),
            DomainEvent::PeerReplyHeld {
                message_id: message_id.to_owned(),
                hold,
            },
        )])
        .await
        .map(drop)
    }

    /// The receiver's records of `send`: its `Received`, its transcript message (role Peer,
    /// the message's own id), a new request unless it is a reply (a reply is filed under the
    /// request that asked), and for a reply its original marked replied on both sides.
    /// Returns them with the receiver's request.
    async fn receipt(&self, send: &PeerSend) -> Result<(Vec<(String, DomainEvent)>, String)> {
        let asked = match &send.reply_to {
            Some(original) => self
                .board(&send.to)
                .await?
                .peer_messages
                .get(original)
                .filter(|record| record.outgoing)
                .and_then(|record| record.request_id.clone()),
            None => None,
        };
        let request = asked.clone().unwrap_or_else(|| send.message_id.clone());
        let mut message = self
            .new_message(
                &send.to,
                MessageRole::Peer,
                send.message_id.clone(),
                send.text.clone(),
            )
            .await?;
        message.request_id = Some(request.clone());
        message.peer = Some(Box::new(PeerRef {
            message_id: send.message_id.clone(),
            from: send.from.clone(),
            reply_to: send.reply_to.clone(),
        }));
        let stream = streams::conversation(&send.to);
        let mut events = vec![
            (
                stream.clone(),
                DomainEvent::PeerMessageReceived {
                    message_id: send.message_id.clone(),
                    from: send.from.clone(),
                    reply_to: send.reply_to.clone(),
                    chain_id: send.chain_id.clone(),
                    hop: send.hop,
                    request_id: request.clone(),
                    automatic: send.automatic,
                    answer_id: send.answer_id.clone(),
                },
            ),
            (
                stream.clone(),
                DomainEvent::MessageAppended {
                    message: message.clone(),
                },
            ),
        ];
        if asked.is_none() {
            events.push((
                stream.clone(),
                DomainEvent::RequestUpdated {
                    request: UserRequest {
                        id: request.clone(),
                        conversation_id: send.to.clone(),
                        preview: preview(&send.text),
                        state: RequestState::Working,
                        started_at_ms: message.created_at_ms,
                        ended_at_ms: None,
                        steered_into: None,
                        steered_after: None,
                        undo: None,
                        waited: Vec::new(),
                        worked: vec![WorkSpan {
                            from_ms: message.created_at_ms,
                            to_ms: None,
                        }],
                        quota_wait: false,
                        waiting_for: Vec::new(),
                        from_peer: Some(send.from.clone()),
                        held_by: None,
                        background_jobs: None,
                    },
                },
            ));
        }
        if let Some(original) = &send.reply_to {
            for side in [&send.to, &send.from] {
                events.push((
                    streams::conversation(side),
                    DomainEvent::PeerMessageState {
                        message_id: original.clone(),
                        state: PeerState::Replied,
                        sent_at_ms: None,
                    },
                ));
            }
        }
        Ok((events, request))
    }

    /// The whole text of a message between sessions, from its sender's record.
    pub(crate) async fn peer_text(
        &self,
        from: &ConversationId,
        message_id: &str,
    ) -> Option<String> {
        let board = self.board(from).await.ok()?;
        let record = board.peer_messages.get(message_id)?;
        match &record.blob {
            Some(hash) => self.read_blob_text(hash.clone()).await.ok(),
            None => Some(record.text.clone()),
        }
    }
}
