//! Sessions message each other (STRIP-DOWN-PLAN S30): `brigadier send <session> <text>` puts
//! the text before another sidebar session's CLI as a message from the sender, never as its
//! user's, and `brigadier wait <session>` returns at the end of its turn with its last message.
//!
//! - A message gets an id (`sm-<12 hex>`) and is kept in the recipient's
//!   `record/session-messages.jsonl` before anything delivers it: it is pending until the
//!   recipient's CLI took it (a steer it accepted, or the turn it started with), then seen.
//! - Its text starts with a header naming the sender and the id
//!   (`[message from session "<title>" (<id>) · sm-…]`): the recipient's CLI knows who wrote
//!   and whom to answer, and its thread draws it as that session's.
//! - After a restart each pending message is seen if the recipient's own CLI session files
//!   hold its id in an input record, and is sent again otherwise. The inbox takes it once.
//! - While the recipient works or its block holds, the message waits in its queue rail under
//!   its own id, judged as the user's follow-ups are (goal 4, D25/D68/D113); it is seen once
//!   it leaves the queue for its thread or a worker. The recipient's user deleting it there
//!   removes it for good, and a `wait` on the recipient says so (proposed).
//! - An answer: the recipient sends one back with `brigadier send`. While the first session
//!   is in a `brigadier wait` on the recipient, that wait returns the answer and it is not
//!   also given to it as a message; otherwise it reaches it as any message does (proposed).

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use brigadier_cli::protocol::{EventKind, WaitEvent, WaitOutput};
use brigadier_providers::ProviderEvent;
use serde::{Deserialize, Serialize};

use super::cli::CliCaller;
use super::conversation::Envelope;
use super::session_dir::read_lineage;
use super::{SessionManager, blocking};
use crate::model::{Conversation, ConversationId, Lifecycle};
use crate::now_ms;
use crate::thread_view::{ThreadNews, TurnEnd, WaitReply};
use crate::work::InjectionKind;

/// The recipient's messages' file, in its `record/`.
pub(crate) const FILE: &str = "session-messages.jsonl";

/// How a message's envelope label begins; its id follows.
const LABEL: &str = "session message ";

/// How a message's text begins in the recipient's CLI.
pub(crate) const HEADER: &str = "[message from session \"";

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "line", rename_all = "kebab-case")]
enum Line {
    Message {
        id: String,
        from: ConversationId,
        /// The whole text its CLI is given, header first.
        text: String,
        at_ms: i64,
    },
    /// Its CLI took it.
    Seen { id: String, at_ms: i64 },
    /// The recipient's user deleted it from the queue before it was read: never sent.
    Removed { id: String, at_ms: i64 },
    /// A reply a `brigadier wait` of this session's took: its thread shows it under that
    /// wait's row, and it is never given to its CLI as a message (goal 4, D138).
    Reply {
        from: ConversationId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
        text: String,
        at_ms: i64,
    },
}

/// A message as its recipient's file says it stands.
#[derive(Debug, Clone)]
struct Kept {
    id: String,
    from: ConversationId,
    text: String,
    seen_at_ms: Option<i64>,
    removed: bool,
}

/// The messages `record`'s file holds, oldest first.
fn read(record: &Path) -> Vec<Kept> {
    let Ok(text) = std::fs::read_to_string(record.join(FILE)) else {
        return Vec::new();
    };
    let mut kept: Vec<Kept> = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        match serde_json::from_str(line) {
            Ok(Line::Message { id, from, text, .. }) => {
                if kept.iter().all(|message| message.id != id) {
                    kept.push(Kept {
                        id,
                        from,
                        text,
                        seen_at_ms: None,
                        removed: false,
                    });
                }
            }
            Ok(Line::Seen { id, at_ms }) => {
                if let Some(message) = kept.iter_mut().find(|message| message.id == id) {
                    message.seen_at_ms.get_or_insert(at_ms);
                }
            }
            Ok(Line::Removed { id, .. }) => {
                if let Some(message) = kept.iter_mut().find(|message| message.id == id) {
                    message.removed = true;
                }
            }
            Ok(Line::Reply { .. }) => {}
            // A line a crash cut short: what it said never happened.
            Err(err) => tracing::warn!(error = %err, "skipped a session message line"),
        }
    }
    kept
}

/// The replies the waits of the session whose `record` this is took, oldest first.
pub(super) fn replies(record: &Path) -> Vec<WaitReply> {
    let Ok(text) = std::fs::read_to_string(record.join(FILE)) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| match serde_json::from_str(line) {
            Ok(Line::Reply {
                from,
                message_id,
                text,
                at_ms,
            }) => Some(WaitReply {
                from,
                message_id,
                text,
                at_ms,
            }),
            _ => None,
        })
        .collect()
}

/// The text a message from `from` (titled `title`) is given to its recipient's CLI with.
fn with_header(id: &str, title: &str, from: &ConversationId, text: &str) -> String {
    let title = title.replace('"', "'");
    format!("{HEADER}{title}\" ({from}) · {id}]\n{text}")
}

/// Whether the CLI session files of the conversation whose `record/` this is hold each of
/// `wanted` in an input record (a message it was given or took mid-turn); never in a tool's
/// output or a reply, where a session could read another's text.
pub(super) fn cli_took(record: &Path, wanted: &[String]) -> Vec<bool> {
    let mut took = vec![false; wanted.len()];
    for session in read_lineage(record) {
        let Some(file) = session.file else {
            continue;
        };
        let path = brigadier_providers::session_file::locate(session.provider, Path::new(&file));
        let Ok(entries) = brigadier_providers::session_file::read(session.provider, &path) else {
            continue;
        };
        for entry in entries.iter().filter(|entry| entry.parent.is_none()) {
            let ProviderEvent::Message {
                role: brigadier_providers::Role::User,
                text,
                ..
            } = &entry.event
            else {
                continue;
            };
            for (at, id) in wanted.iter().enumerate() {
                if text.contains(id.as_str()) {
                    took[at] = true;
                }
            }
        }
    }
    took
}

/// A `brigadier wait <session>` running now: who waits, on whom, and where an answer goes.
pub(super) struct Waiting {
    waiter: ConversationId,
    on: Vec<ConversationId>,
    answer: tokio::sync::oneshot::Sender<WaitEvent>,
}

/// The waits running now, by a token of their own.
pub(super) type Waits = std::sync::Mutex<HashMap<u64, Waiting>>;

static WAIT_TOKENS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The shown id: the last 8 characters.
fn short(id: &str) -> String {
    id[id.len().saturating_sub(8)..].to_owned()
}

impl SessionManager {
    fn append_mail(&self, to: &ConversationId, line: &Line) {
        let file = self.session_dir(to).record().join(FILE);
        let _held = self
            .session_mail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let written = serde_json::to_string(line)
            .map_err(std::io::Error::other)
            .and_then(|json| {
                if let Some(dir) = file.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let mut out = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&file)?;
                out.write_all(format!("{json}\n").as_bytes())
            });
        if let Err(err) = written {
            tracing::warn!(file = %file.display(), error = %err, "could not keep a session message");
        }
    }

    /// `from` sends session `to` `text`: kept for `to` first, then steered into its running
    /// turn or queued as its next. Answers the message's id and whether a turn of `to` runs
    /// with it.
    pub(super) async fn message_session(
        &self,
        from: &Conversation,
        to: &Conversation,
        text: &str,
    ) -> (String, bool) {
        let id = format!("sm-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let body = text;
        let text = with_header(&id, &from.title, &from.id, body);
        self.append_mail(
            &to.id,
            &Line::Message {
                id: id.clone(),
                from: from.id.clone(),
                text: text.clone(),
                at_ms: now_ms(),
            },
        );
        // `to` waits on `from` now: its wait takes the answer, which goes nowhere else.
        if self.answer_wait(&to.id, &from.id, &id, body) {
            self.session_message_seen(&to.id, &id);
            return (id, true);
        }
        let running = self
            .deliver_session_message(&to.id, &id, &from.id, text)
            .await;
        (id, running)
    }

    /// Gives `body`, message `id` of `from`'s, to a `brigadier wait` of `to`'s on `from`
    /// running now; false when none runs (or it just ended).
    fn answer_wait(
        &self,
        to: &ConversationId,
        from: &ConversationId,
        id: &str,
        body: &str,
    ) -> bool {
        let mut waits = self
            .session_waits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(token) = waits
            .iter()
            .find(|(_, wait)| wait.waiter == *to && wait.on.contains(from))
            .map(|(token, _)| *token)
        else {
            return false;
        };
        let wait = waits.remove(&token).expect("found");
        // Sent under the lock: a wait ending now finds its token gone only with the answer in.
        let event = WaitEvent {
            id: short(&from.0),
            event_id: id.to_owned(),
            event: EventKind::Answered,
            message: Some(body.to_owned()),
            asks: None,
            resets_at_ms: None,
            at_ms: now_ms(),
            git: None,
            commands: None,
        };
        let given = wait.answer.send(event).is_ok();
        drop(waits);
        given
    }

    /// Puts message `id` of `from`'s before `to`'s CLI: into its queue while it works or its
    /// block holds, judged there (goal 4); else steered or starting a turn, seen at once when
    /// a running turn took it.
    async fn deliver_session_message(
        &self,
        to: &ConversationId,
        id: &str,
        from: &ConversationId,
        text: String,
    ) -> bool {
        let peer = crate::model::PeerRef {
            message_id: id.to_owned(),
            from: from.clone(),
            reply_to: None,
        };
        // Boxed: the queue's judging is a large future, kept off `brigadier send`'s stack.
        if let Some(running) = Box::pin(self.queue_peer(to, peer, text.clone())).await {
            return running;
        }
        let envelope = Envelope {
            kind: InjectionKind::PeerMessage,
            label: format!("{LABEL}{id}"),
            task_id: None,
            text,
        };
        match self.steer_or_queue(to, envelope).await {
            Some(true) => {
                self.session_message_seen(to, id);
                true
            }
            Some(false) => match self.conv(to) {
                Ok(conv) => conv.is_busy().await,
                Err(_) => false,
            },
            None => false,
        }
    }

    fn session_message_seen(&self, to: &ConversationId, id: &str) {
        self.append_mail(
            to,
            &Line::Seen {
                id: id.to_owned(),
                at_ms: now_ms(),
            },
        );
        self.session_turns.notify_waiters();
    }

    /// Queued `item` leaves `to`'s queue for its thread: another session's message is seen
    /// first, so a crash never sends it again (goal 4).
    pub(super) async fn append_queued(
        &self,
        to: ConversationId,
        item: crate::work::QueuedMessage,
    ) -> crate::Result<crate::model::Message> {
        if item.peer.is_some() {
            self.session_message_seen(&to, &item.id);
        }
        self.core.append_queued(to, item).await
    }

    /// `to`'s user deleted queued `item`: another session's message is removed for good.
    pub(super) fn peer_removed(&self, to: &ConversationId, item: &crate::work::QueuedMessage) {
        if item.peer.is_some() {
            self.append_mail(
                to,
                &Line::Removed {
                    id: item.id.clone(),
                    at_ms: now_ms(),
                },
            );
            self.session_turns.notify_waiters();
        }
    }

    /// `to`'s CLI took the turn carrying envelope `label`: if it is a session's message, seen.
    pub(super) fn session_message_taken(&self, to: &ConversationId, label: &str) {
        if let Some(id) = label.strip_prefix(LABEL) {
            self.session_message_seen(to, id);
        }
    }

    /// After a restart: `id`'s messages its CLI never took go to it again, once; those its own
    /// session files show it took are seen.
    pub(super) async fn recover_session_messages(&self, id: &ConversationId) {
        let record = self.session_dir(id).record();
        // One still in the queue goes on from there.
        let queued: Vec<String> = match self.core.board(id).await {
            Ok(board) => board.queue.items.into_iter().map(|item| item.id).collect(),
            Err(_) => Vec::new(),
        };
        let kept = read(&record);
        // Removed, then the crash came before it left the queue: it leaves now, never sent.
        for message in kept.iter().filter(|m| m.removed && queued.contains(&m.id)) {
            if let Err(err) = self.delete_queued(id, &message.id).await {
                tracing::warn!(conversation = %id, message = %message.id, error = %err, "a removed session message stays queued");
            }
        }
        let pending: Vec<Kept> = kept
            .into_iter()
            .filter(|message| {
                message.seen_at_ms.is_none() && !message.removed && !queued.contains(&message.id)
            })
            .collect();
        if pending.is_empty() {
            return;
        }
        let ids: Vec<String> = pending.iter().map(|message| message.id.clone()).collect();
        let at = record.clone();
        let took = blocking(move || Ok(cli_took(&at, &ids)))
            .await
            .unwrap_or_default();
        for (at, message) in pending.into_iter().enumerate() {
            if took.get(at).copied().unwrap_or(false) {
                self.session_message_seen(id, &message.id);
                continue;
            }
            tracing::info!(conversation = %id, message = %message.id, from = %message.from, "a session's message the restart cut off goes again");
            self.deliver_session_message(id, &message.id, &message.from, message.text)
                .await;
        }
    }

    /// `brigadier wait <session>…`: the first turn end of `sessions`, each once it took what
    /// the caller sent it (with nothing sent, a turn that ends after the wait began), with its
    /// last message; none when the time is up.
    pub(super) async fn wait_for_sessions(
        &self,
        caller: &CliCaller,
        sessions: &[Conversation],
        timeout_secs: Option<u32>,
    ) -> WaitOutput {
        let began = now_ms();
        let deadline = timeout_secs.map(|secs| {
            tokio::time::Instant::now() + std::time::Duration::from_secs(u64::from(secs))
        });
        // An answer one of them sends meanwhile comes here, and nowhere else.
        let (answer, mut answered) = tokio::sync::oneshot::channel();
        let token = WAIT_TOKENS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.session_waits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                token,
                Waiting {
                    waiter: caller.conversation_id.clone(),
                    on: sessions.iter().map(|session| session.id.clone()).collect(),
                    answer,
                },
            );
        let out = self
            .wait_on_sessions(caller, sessions, began, deadline, &mut answered)
            .await;
        let gone = self
            .session_waits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&token)
            .is_none();
        // An answer given as it ended is its own: never lost, never given twice.
        let out = if gone
            && out
                .events
                .iter()
                .all(|event| event.event != EventKind::Answered)
            && let Ok(event) = answered.try_recv()
        {
            let mut events = out.events;
            events.insert(0, event);
            WaitOutput {
                events,
                timed_out: false,
                message: false,
            }
        } else {
            out
        };
        if caller.task_id.is_none() {
            self.keep_replies(&caller.conversation_id, sessions, &out.events);
        }
        out
    }

    /// The replies a wait of session `waiter`'s on `sessions` took: kept in its record, and
    /// its thread told at once, before its CLI reads them (goal 4, D138).
    fn keep_replies(
        &self,
        waiter: &ConversationId,
        sessions: &[Conversation],
        events: &[WaitEvent],
    ) {
        let mut kept = false;
        for event in events {
            if !matches!(event.event, EventKind::Answered | EventKind::TurnEnded) {
                continue;
            }
            let from = sessions
                .iter()
                .find(|session| short(&session.id.0) == event.id);
            let (Some(from), Some(text)) = (from, &event.message) else {
                continue;
            };
            self.append_mail(
                waiter,
                &Line::Reply {
                    from: from.id.clone(),
                    message_id: (!event.event_id.is_empty()).then(|| event.event_id.clone()),
                    text: text.clone(),
                    at_ms: now_ms(),
                },
            );
            kept = true;
        }
        if kept {
            let _ = self.thread_news.send(ThreadNews {
                conversation_id: waiter.clone(),
                entries: Vec::new(),
                opened: false,
                ended: None,
                turn_ended_at_ms: None,
                context: None,
                background: None,
                mcp: None,
                workers: None,
                jobs: None,
                replies: Some(replies(&self.session_dir(waiter).record())),
                seq: 0,
            });
        }
    }

    async fn wait_on_sessions(
        &self,
        caller: &CliCaller,
        sessions: &[Conversation],
        began: i64,
        deadline: Option<tokio::time::Instant>,
        answered: &mut tokio::sync::oneshot::Receiver<WaitEvent>,
    ) -> WaitOutput {
        loop {
            let turned = self.session_turns.notified();
            tokio::pin!(turned);
            turned.as_mut().enable();
            let mut events = Vec::new();
            for session in sessions {
                if let Some(event) = self
                    .session_turn_end(&caller.conversation_id, session, began)
                    .await
                {
                    events.push(event);
                }
            }
            if !events.is_empty() {
                return WaitOutput {
                    events,
                    timed_out: false,
                    message: false,
                };
            }
            // Looked at again on each turn end and message seen, and each second besides.
            let again = tokio::time::sleep(std::time::Duration::from_secs(1));
            let far = deadline.unwrap_or_else(|| {
                tokio::time::Instant::now() + std::time::Duration::from_secs(86_400)
            });
            tokio::select! {
                () = turned => {}
                () = again => {}
                Ok(event) = &mut *answered => {
                    return WaitOutput { events: vec![event], timed_out: false, message: false };
                }
                () = tokio::time::sleep_until(far), if deadline.is_some() => {
                    return WaitOutput { events: Vec::new(), timed_out: true, message: false };
                }
            }
        }
    }

    /// `session`'s last turn end, once it rests and took every message of `from`'s, and that
    /// turn ended after the newest of them was taken (or after `began`, with none sent).
    async fn session_turn_end(
        &self,
        from: &ConversationId,
        session: &Conversation,
        began: i64,
    ) -> Option<WaitEvent> {
        if session.lifecycle == Lifecycle::Archived {
            return None;
        }
        let record = self.session_dir(&session.id).record();
        let mine: Vec<Kept> = read(&record)
            .into_iter()
            .filter(|message| message.from == *from)
            .collect();
        // The newest removed from its queue unread: no answer comes for it (proposed).
        if let Some(removed) = mine.last().filter(|message| message.removed) {
            return Some(WaitEvent {
                id: short(&session.id.0),
                event_id: removed.id.clone(),
                event: EventKind::Removed,
                message: Some(format!(
                    "{}'s user removed your message {} from its queue before it was read; it \
                     won't answer it.",
                    session.title, removed.id
                )),
                asks: None,
                resets_at_ms: None,
                at_ms: now_ms(),
                git: None,
                commands: None,
            });
        }
        if let Ok(conv) = self.conv(&session.id)
            && conv.is_busy().await
        {
            return None;
        }
        let mine: Vec<Kept> = mine
            .into_iter()
            .filter(|message| !message.removed)
            .collect();
        if mine.iter().any(|message| message.seen_at_ms.is_none()) {
            return None;
        }
        let since = mine
            .iter()
            .filter_map(|message| message.seen_at_ms)
            .max()
            .unwrap_or(began);
        let thread = self.thread_view(&session.id).ok()?;
        let turn = thread.turns.iter().rev().find(|turn| turn.end.is_some())?;
        let ended = turn.ended_at_ms?;
        if ended < since {
            return None;
        }
        // Its answer is that turn's own: a turn that failed or stopped before one says its error.
        let opens = |entry: &brigadier_providers::session_file::FileEntry, id: Option<&str>| {
            entry.parent.is_none()
                && !entry.steer
                && matches!(&entry.event, ProviderEvent::Message {
                    role: brigadier_providers::Role::User,
                    item_id,
                    ..
                } if id.is_none_or(|id| item_id == id))
        };
        let start = thread
            .entries
            .iter()
            .position(|entry| opens(entry, Some(&turn.user_message)))?;
        let end = thread.entries[start + 1..]
            .iter()
            .position(|entry| opens(entry, None))
            .map_or(thread.entries.len(), |next| start + 1 + next);
        let message = thread.entries[start..end]
            .iter()
            .rev()
            .filter(|entry| entry.parent.is_none())
            .find_map(|entry| match &entry.event {
                ProviderEvent::Message {
                    role: brigadier_providers::Role::Assistant,
                    text,
                    ..
                } if !text.trim().is_empty() => Some(text.clone()),
                _ => None,
            });
        let event = match turn.end? {
            TurnEnd::Done => EventKind::TurnEnded,
            TurnEnd::Failed => EventKind::Failed,
            TurnEnd::Stopped | TurnEnd::Interrupted => EventKind::Interrupted,
        };
        Some(WaitEvent {
            id: short(&session.id.0),
            event_id: String::new(),
            event,
            message: message.or_else(|| turn.error.clone()),
            asks: None,
            resets_at_ms: None,
            at_ms: ended,
            git: None,
            commands: None,
        })
    }

    /// Tests: a session's messages by id, each with whether its CLI took it.
    #[cfg(all(test, unix))]
    pub(crate) fn session_messages_of(&self, to: &ConversationId) -> HashMap<String, bool> {
        read(&self.session_dir(to).record())
            .into_iter()
            .map(|message| (message.id, message.seen_at_ms.is_some()))
            .collect()
    }
}
