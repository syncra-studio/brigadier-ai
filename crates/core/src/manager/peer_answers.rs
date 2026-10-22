//! A session's answer to another session's message goes back to that session as the reply,
//! sent by Brigadier (SESSION-MESSAGING-PLAN.md §5.2): the receiving thread answers a peer's
//! request the way it answers anything, and never messages back itself.
//!
//! Each message another session sent asking for something (not a reply) is owed one reply,
//! once its CLI took it. It is sent when every request that answers it is over: the request it
//! opened, those it was steered into, and those steered into them. An interim turn end, a
//! turn, or a worker, review or card still going keeps its request working, so the
//! reply carries the finished answer. A reply that already went (the thread's own, or this
//! one) marks the message replied in the same write, so it is sent once, across restarts.
//!
//! The final answer is the reply only when it answers that message alone. A message steered
//! into other work (or with the user's messages steered into its request) is answered by the
//! thread on its own (message_session with reply_to); for one it left unanswered the sender
//! hears it went unanswered, never the other work's answer.

use super::SessionManager;
use crate::board::{Board, PeerRecord};
use crate::model::{ConversationId, Message, MessageRole, PeerState};
use crate::sessions::PeerSend;
use crate::work::RequestState;

/// Whether `record` is a message another session sent this one asking for something, taken
/// by its CLI, with no reply yet.
pub(super) fn owes_reply(record: &PeerRecord) -> bool {
    !record.outgoing && record.reply_to.is_none() && record.state == PeerState::Seen
}

/// The request `request` was steered into, and so on, up to the one whose block shows it.
fn root_of<'a>(board: &'a Board, request: &'a str) -> &'a str {
    let mut at = request;
    // A steer chain is short; the bound only guards against a cycle in stored data.
    for _ in 0..board.requests.len() {
        match board
            .requests
            .get(at)
            .and_then(|of| of.steered_into.as_deref())
        {
            Some(into) => at = into,
            None => break,
        }
    }
    at
}

/// Whether request `request` answers its message alone: not steered into other work, with
/// nothing steered into it. Brigadier then sends its final answer back as the reply.
pub(super) fn answers_alone(board: &Board, request: &str) -> bool {
    root_of(board, request) == request
        && !board
            .requests
            .keys()
            .any(|other| other != request && root_of(board, other) == request)
}

/// How the requests answering a message ended, once all are over: the answer message, if
/// the thread wrote one after the message, and how they ended.
enum Over {
    Answer(Box<Message>),
    Stopped,
    Failed(String),
    /// A restart cut the thread off before it answered.
    CutOff,
    Silent,
    /// The message came amid other work, and the thread didn't answer it on its own even
    /// when reminded: that work's answer isn't the reply.
    Unanswered,
}

impl SessionManager {
    /// Sends session `id`'s answer to each message it owes a reply whose requests are over.
    /// Boxed: request settlement starts it, and its delivery settles requests in turn.
    pub(super) fn answer_peers<'a>(
        &'a self,
        id: &'a ConversationId,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(self.answer_owed(id))
    }

    async fn answer_owed(&self, id: &ConversationId) {
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        let mut owed: Vec<&PeerRecord> = board
            .peer_messages
            .values()
            .filter(|record| owes_reply(record))
            .collect();
        if owed.is_empty() {
            return;
        }
        owed.sort_by_key(|record| record.position);
        let mut messages: Option<Vec<Message>> = None;
        for record in owed {
            let Some(request) = record.request_id.as_deref() else {
                continue;
            };
            let root = root_of(&board, request);
            let family: Vec<&str> = board
                .requests
                .keys()
                .map(String::as_str)
                .filter(|other| root_of(&board, other) == root)
                .collect();
            let states: Vec<&RequestState> = family
                .iter()
                .filter_map(|of| board.requests.get(*of).map(|of| &of.state))
                .collect();
            if states
                .iter()
                .any(|state| matches!(state, RequestState::Working | RequestState::Waiting))
            {
                continue;
            }
            // A sender gone (deleted, or no longer reachable) hears nothing.
            if self.peer_session(&record.peer.0).is_err() {
                continue;
            }
            if messages.is_none() {
                messages = Some(self.core.all_messages(id).await.unwrap_or_default());
            }
            let all = messages.as_deref().unwrap_or_default();
            let asked_at = all
                .iter()
                .find(|message| message.id == record.message_id)
                .map_or(0, |message| message.seq);
            let answer = all
                .iter()
                .filter(|message| {
                    message.role == MessageRole::Assistant
                        && message.seq > asked_at
                        && message
                            .request_id
                            .as_deref()
                            .is_some_and(|of| family.contains(&of))
                })
                .max_by_key(|message| message.seq);
            let mut over = match answer {
                Some(answer) => Over::Answer(Box::new(answer.clone())),
                None => match states.iter().find_map(|state| match state {
                    // A quit cut the turn off (UI audit H1 writes it so): the asker hears that,
                    // as for a request the restart's settling ended.
                    state if super::conversation::interrupted_by_quit(state) => Some(Over::CutOff),
                    RequestState::Failed { error } => Some(Over::Failed(error.clone())),
                    RequestState::Stopped => Some(Over::Stopped),
                    _ => None,
                }) {
                    Some(over) => over,
                    None if self.cut_off_any(&family) => Over::CutOff,
                    None => Over::Silent,
                },
            };
            // Steered into other work, or with the user's messages steered into it: the
            // final answer is that work's too, so only the thread's own reply answers it.
            if !answers_alone(&board, request) && matches!(over, Over::Answer(_) | Over::Silent) {
                over = Over::Unanswered;
            }
            self.send_answer(id, record, root, over).await;
        }
    }

    /// The requests of session `id` still working, as its board has them.
    pub(super) async fn working_requests(&self, id: &ConversationId) -> Vec<String> {
        self.core.board(id).await.map_or_else(
            |_| Vec::new(),
            |board| {
                board
                    .requests
                    .values()
                    .filter(|request| request.state == RequestState::Working)
                    .map(|request| request.id.clone())
                    .collect()
            },
        )
    }

    /// Whether the last restart cut off any of `requests`.
    fn cut_off_any(&self, requests: &[&str]) -> bool {
        let cut_off = self.cut_off_lock();
        requests.iter().any(|request| cut_off.contains(*request))
    }

    /// Sends the reply to `record` from session `id`, filed under its request `root`.
    async fn send_answer(&self, id: &ConversationId, record: &PeerRecord, root: &str, over: Over) {
        let Ok(me) = self.core.conversation(id) else {
            return;
        };
        let (text, answer_id) = match over {
            Over::Answer(message) => {
                let text = self.full_text(&message).await;
                (text, Some(message.id.clone()))
            }
            Over::Stopped => (
                format!("[\"{}\" was stopped before it answered]", me.title),
                None,
            ),
            Over::Failed(error) => (format!("[\"{}\" couldn't answer: {error}]", me.title), None),
            Over::CutOff => (
                format!(
                    "[\"{}\" was cut off by a restart before it answered]",
                    me.title
                ),
                None,
            ),
            Over::Silent => (
                format!("[\"{}\" finished without an answer]", me.title),
                None,
            ),
            Over::Unanswered => (
                format!("[\"{}\" didn't answer your message]", me.title),
                None,
            ),
        };
        let send = PeerSend {
            message_id: uuid::Uuid::now_v7().to_string(),
            from: id.clone(),
            to: record.peer.clone(),
            text,
            reply_to: Some(record.message_id.clone()),
            chain_id: record.chain_id.clone(),
            hop: record.hop + 1,
            request_id: Some(root.to_owned()),
            turn_input_id: None,
            automatic: true,
            answer_id,
        };
        {
            // Once: a settle running alongside sees it replied.
            let _checked = self.peer_sends.lock().await;
            let still = self.core.board(id).await.is_ok_and(|board| {
                board
                    .peer_messages
                    .get(&record.message_id)
                    .is_some_and(owes_reply)
            });
            if !still {
                return;
            }
            if let Err(err) = self.core.record_peer_send(&send, false).await {
                tracing::warn!(conversation = %id, message = %record.message_id, error = %err, "could not send the answer to another session's message");
                return;
            }
        }
        self.deliver_peer(&send.to, &send.message_id).await;
    }
}
