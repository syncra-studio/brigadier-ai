//! A thread waiting in its turn for the replies to the messages it sent (`wait_for_replies`,
//! SESSION-MESSAGING-PLAN.md §5.2): the turn stays live while the other sessions answer, and
//! each reply is the waiting call's result rather than a turn of its own.
//!
//! A reply to a message the running turn sent is held for it ([`PeerReplies`]): one that came
//! before a wait started waits there for one. A wait hands it over as its result, recorded
//! `Handed`, and it is taken (`seen`, `Taken`) only once the CLI shows that call finished. What
//! a turn ends without taking (no wait came, a Stop, a limit) goes back to ordinary delivery,
//! and a restart returns what was handed and never taken the same way, so a reply is never
//! lost between the two.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use super::SessionManager;
use super::conversation::ConvLive;
use crate::board::PeerRecord;
use crate::model::{ConversationId, MessageRole, PeerState, ReplyHold};
use crate::tools::WaitForReplies;
use crate::{Error, Result};

/// How long a wait lasts when the thread doesn't say.
const WAIT_DEFAULT: Duration = Duration::from_secs(300);
/// The longest wait: within the thread's MCP call timeout.
const WAIT_MAX: Duration = Duration::from_secs(1800);
const _: () = assert!(super::conversation::RUNNER_TOOL_TIMEOUT_SECS > WAIT_MAX.as_secs());

/// The replies a session's running turn holds for its waits.
#[derive(Debug, Default)]
pub(crate) struct PeerReplies {
    /// The messages the running turn sent: a reply to one is held for a wait.
    pub(super) sent: HashSet<String>,
    /// Replies that came while the turn ran, no wait has taken yet, oldest first.
    pub(super) held: Vec<String>,
    /// Replies handed to a wait's result whose call the CLI hasn't shown finished.
    pub(super) handed: Vec<String>,
    /// The open wait, by number: a reply that comes wakes it; a newer wait or the turn's end
    /// ends it.
    pub(super) waiting: Option<u64>,
    next_wait: u64,
}

/// What one look at the held replies found for a wait.
enum Look {
    /// Replies for it, now handed over.
    Replies(Vec<String>),
    /// Nothing yet.
    Nothing,
    /// The wait is over: the turn ended, or a newer wait replaced it.
    Ended,
}

impl PeerReplies {
    fn open_wait(&mut self) -> u64 {
        self.next_wait += 1;
        self.waiting = Some(self.next_wait);
        self.next_wait
    }

    /// Hands wait `wait` the held replies `answers` accepts.
    fn look(&mut self, wait: u64, answers: impl Fn(&str) -> bool) -> Look {
        if self.waiting != Some(wait) {
            return Look::Ended;
        }
        let (taken, held): (Vec<String>, Vec<String>) = std::mem::take(&mut self.held)
            .into_iter()
            .partition(|reply| answers(reply));
        self.held = held;
        if taken.is_empty() {
            return Look::Nothing;
        }
        self.handed.extend(taken.iter().cloned());
        Look::Replies(taken)
    }

    /// Everything the turn holds, at its end: what was held and what was handed and not
    /// taken, oldest first. Any wait ends.
    pub(super) fn drain(&mut self) -> (Vec<String>, Vec<String>) {
        self.waiting = None;
        self.sent.clear();
        (
            std::mem::take(&mut self.held),
            std::mem::take(&mut self.handed),
        )
    }
}

impl SessionManager {
    /// `wait_for_replies`: waits in `caller`'s running turn until a reply comes to one of the
    /// messages it names (by default every message of the turn's request still due a reply),
    /// or the time is up. Its result is the replies' text, and who still has to reply.
    pub(super) async fn wait_for_replies(
        &self,
        caller: &ConversationId,
        args: WaitForReplies,
    ) -> Result<String> {
        let conv = self.conv(caller)?;
        let Some((request, _)) = conv.sending_turn().await else {
            return Err(Error::Invalid(
                "A wait runs in a turn; none runs now.".into(),
            ));
        };
        let board = self.core.board(caller).await?;
        // Replies held for this turn's waits: their messages are still due here, though
        // already replied.
        let held: HashSet<String> = conv.replies.lock().unwrap().held.iter().cloned().collect();
        let reply_to = |record: &PeerRecord| {
            board.peer_messages.values().find(|reply| {
                !reply.outgoing && reply.reply_to.as_deref() == Some(&record.message_id)
            })
        };
        let held_for = |record: &PeerRecord| {
            reply_to(record).is_some_and(|reply| held.contains(&reply.message_id))
        };
        let asked: Vec<PeerRecord> = match &args.message_ids {
            Some(ids) if !ids.is_empty() => {
                let mut asked = Vec::new();
                for id in ids {
                    let Some(record) = board
                        .peer_messages
                        .get(id.trim())
                        .filter(|record| record.outgoing && record.reply_to.is_none())
                    else {
                        return Err(Error::Invalid(format!(
                            "No message this session sent has the id {id}."
                        )));
                    };
                    asked.push(record.clone());
                }
                asked
            }
            _ => board
                .peer_messages
                .values()
                .filter(|record| {
                    record.outgoing
                        && record.reply_to.is_none()
                        && record.request_id.is_some()
                        && record.request_id == request
                        && (record.awaits_reply()
                            || record.state == PeerState::WaitingForOk
                            || held_for(record))
                })
                .cloned()
                .collect(),
        };
        let mut asked = asked;
        asked.sort_by_key(|record| record.position);
        // Answered already, before this wait, and not held for it: said at once.
        let mut lines = Vec::new();
        for record in asked
            .iter()
            .filter(|record| record.state == PeerState::Replied && !held_for(record))
        {
            let Some(reply) = reply_to(record) else {
                continue;
            };
            lines.push(match reply.hold {
                None | Some(ReplyHold::Returned) => format!(
                    "[that reply already reached you as a message of its own · id {} · reply to {}]",
                    reply.message_id, record.message_id
                ),
                Some(ReplyHold::Handed | ReplyHold::Taken { .. }) => format!(
                    "[an earlier wait already gave you that reply · id {} · reply to {}]",
                    reply.message_id, record.message_id
                ),
            });
        }
        // Never sent, never delivered, or its wait settled: no reply comes, said at once.
        let none_comes = |record: &PeerRecord| match &record.state {
            PeerState::Declined => Some("the user didn't send it".to_owned()),
            PeerState::Refused { reason } => Some(format!("it was never sent: {reason}")),
            PeerState::Dropped { reason } => Some(format!("it won't be delivered: {reason}")),
            PeerState::Replied => None,
            _ if record.settled => Some("you settled its wait".to_owned()),
            _ => None,
        };
        for record in &asked {
            if let Some(why) = none_comes(record) {
                lines.push(format!("[no reply comes to {}: {why}]", record.message_id));
            }
        }
        let due: Vec<&PeerRecord> = asked
            .iter()
            .filter(|record| none_comes(record).is_none())
            .filter(|record| record.state != PeerState::Replied || held_for(record))
            .collect();
        if due.is_empty() {
            return Ok(if lines.is_empty() {
                "No message of yours waits for a reply.".into()
            } else {
                lines.join("\n")
            });
        }
        let due_ids: HashSet<String> = due.iter().map(|r| r.message_id.clone()).collect();
        let timeout = args
            .timeout_s
            .map_or(WAIT_DEFAULT, Duration::from_secs)
            .clamp(Duration::from_secs(1), WAIT_MAX);
        let deadline = tokio::time::Instant::now() + timeout;
        let wait = conv.replies.lock().unwrap().open_wait();
        let taken = loop {
            let woken = conv.reply_wake.notified();
            tokio::pin!(woken);
            woken.as_mut().enable();
            let look = {
                let board = self.core.board(caller).await?;
                let answers = |reply: &str| {
                    board
                        .peer_messages
                        .get(reply)
                        .and_then(|record| record.reply_to.as_ref())
                        .is_some_and(|original| due_ids.contains(original))
                };
                conv.replies.lock().unwrap().look(wait, answers)
            };
            match look {
                Look::Replies(taken) => break taken,
                Look::Ended => {
                    return Err(Error::Invalid(
                        "The wait ended with the turn: no reply came in it.".into(),
                    ));
                }
                Look::Nothing => {}
            }
            tokio::select! {
                () = &mut woken => {}
                () = tokio::time::sleep_until(deadline) => break Vec::new(),
            }
        };
        {
            let mut replies = conv.replies.lock().unwrap();
            if replies.waiting == Some(wait) {
                replies.waiting = None;
            }
        }
        for reply in &taken {
            if let Err(err) = self
                .core
                .record_reply_hold(caller, reply, ReplyHold::Handed)
                .await
            {
                tracing::warn!(conversation = %caller, message = %reply, error = %err, "could not record a reply handed to a wait");
            }
        }
        let board = self.core.board(caller).await?;
        let title = |id: &ConversationId| {
            self.core
                .conversation(id)
                .map(|peer| peer.title)
                .unwrap_or_else(|_| "a deleted session".into())
        };
        let mut answered = HashSet::new();
        for reply in &taken {
            let Some(record) = board.peer_messages.get(reply) else {
                continue;
            };
            let original = record.reply_to.clone().unwrap_or_default();
            answered.insert(original.clone());
            let text = self.reply_text(caller, reply).await;
            lines.push(format!(
                "[reply from session \"{}\" · session {} · id {reply} · reply to {original}]\n{text}",
                title(&record.peer),
                record.peer
            ));
        }
        let still: Vec<String> = due
            .iter()
            .filter(|record| !answered.contains(&record.message_id))
            .map(|record| format!("\"{}\" (id {})", title(&record.peer), record.message_id))
            .collect();
        if taken.is_empty() {
            lines.push(format!(
                "No reply yet after {} s. Still to reply: {}. Wait again, or end your turn: your \
                 request then waits for the reply, which starts a turn of its own.",
                timeout.as_secs(),
                still.join(", ")
            ));
        } else if !still.is_empty() {
            lines.push(format!(
                "Still to reply: {}. Call wait_for_replies again for them.",
                still.join(", ")
            ));
        }
        Ok(lines.join("\n\n"))
    }

    /// The whole text of reply `reply`, as `id`'s transcript holds it.
    async fn reply_text(&self, id: &ConversationId, reply: &str) -> String {
        let message = self.core.all_messages(id).await.ok().and_then(|messages| {
            messages
                .into_iter()
                .find(|message| message.id == reply && message.role == MessageRole::Peer)
        });
        match message {
            Some(message) => self.full_text(&message).await,
            None => String::new(),
        }
    }

    /// Holds reply `reply` for a wait of `conv`'s running turn, when that turn sent the
    /// message it answers (`reply_to`) or a wait is open: returns whether it did. A wait open
    /// now is woken.
    pub(super) async fn hold_reply(
        &self,
        conv: &Arc<ConvLive>,
        reply_to: &str,
        reply: &str,
    ) -> bool {
        let held = conv
            .while_serving(|replies| {
                if !replies.sent.contains(reply_to) && replies.waiting.is_none() {
                    return false;
                }
                if !replies.held.iter().any(|held| held == reply) {
                    replies.held.push(reply.to_owned());
                }
                true
            })
            .await;
        if held {
            conv.reply_wake.notify_waiters();
        }
        held
    }

    /// The CLI shows the thread's wait call `item_id` finished: the replies handed to it are
    /// taken, and seen.
    pub(super) async fn replies_taken(&self, conv: &Arc<ConvLive>, item_id: &str) {
        let handed = std::mem::take(&mut conv.replies.lock().unwrap().handed);
        for reply in handed {
            // Seen first: a reply taken and not yet seen at a crash would be delivered again.
            self.peer_seen(&conv.id, &reply).await;
            let hold = ReplyHold::Taken {
                item_id: item_id.to_owned(),
            };
            if let Err(err) = self.core.record_reply_hold(&conv.id, &reply, hold).await {
                tracing::warn!(conversation = %conv.id, message = %reply, error = %err, "could not record a reply a wait took");
            }
        }
    }

    /// The turn ended: what it held and didn't take goes by ordinary delivery (a turn of its
    /// own, or a note when its request is over). A wait still open ends.
    pub(super) async fn return_held_replies(&self, conv: &Arc<ConvLive>) {
        let (held, handed) = conv.replies.lock().unwrap().drain();
        conv.reply_wake.notify_waiters();
        if held.is_empty() && handed.is_empty() {
            return;
        }
        // Its request's end (a Stop) first, so a reply to it becomes a note and starts nothing.
        self.settle_requests(&conv.id).await;
        for reply in &handed {
            if let Err(err) = self
                .core
                .record_reply_hold(&conv.id, reply, ReplyHold::Returned)
                .await
            {
                tracing::warn!(conversation = %conv.id, message = %reply, error = %err, "could not record a reply returned from a wait");
            }
        }
        for reply in handed.iter().chain(&held) {
            self.deliver_peer(&conv.id, reply).await;
        }
    }

    /// At startup: a reply handed to a wait the CLI never showed finished goes back to
    /// ordinary delivery (its recovery delivers it again); one a wait took is seen.
    pub(super) async fn return_handed_replies(&self, id: &ConversationId) {
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        let due = board
            .peer_messages
            .values()
            .filter(|record| !record.outgoing && record.state.due());
        for record in due
            .clone()
            .filter(|record| matches!(record.hold, Some(ReplyHold::Taken { .. })))
        {
            self.peer_seen(id, &record.message_id).await;
        }
        for record in due.filter(|record| record.hold == Some(ReplyHold::Handed)) {
            if let Err(err) = self
                .core
                .record_reply_hold(id, &record.message_id, ReplyHold::Returned)
                .await
            {
                tracing::warn!(conversation = %id, message = %record.message_id, error = %err, "could not record a reply returned from a wait");
            }
        }
    }
}
