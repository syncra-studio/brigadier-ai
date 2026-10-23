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

#[cfg(all(test, unix))]
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
use crate::thread_view::TurnEnd;
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
}

/// A message as its recipient's file says it stands.
#[derive(Debug, Clone)]
struct Kept {
    id: String,
    from: ConversationId,
    text: String,
    seen_at_ms: Option<i64>,
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
                    });
                }
            }
            Ok(Line::Seen { id, at_ms }) => {
                if let Some(message) = kept.iter_mut().find(|message| message.id == id) {
                    message.seen_at_ms.get_or_insert(at_ms);
                }
            }
            // A line a crash cut short: what it said never happened.
            Err(err) => tracing::warn!(error = %err, "skipped a session message line"),
        }
    }
    kept
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
        let text = with_header(&id, &from.title, &from.id, text);
        self.append_mail(
            &to.id,
            &Line::Message {
                id: id.clone(),
                from: from.id.clone(),
                text: text.clone(),
                at_ms: now_ms(),
            },
        );
        let running = self.deliver_session_message(&to.id, &id, text).await;
        (id, running)
    }

    /// Puts message `id` before `to`'s CLI; it is seen at once when a running turn took it.
    async fn deliver_session_message(&self, to: &ConversationId, id: &str, text: String) -> bool {
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
        let pending: Vec<Kept> = read(&record)
            .into_iter()
            .filter(|message| message.seen_at_ms.is_none())
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
            self.deliver_session_message(id, &message.id, message.text)
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
                };
            }
            // Looked at again on each turn end and message seen, and each second besides.
            let again = tokio::time::sleep(std::time::Duration::from_secs(1));
            match deadline {
                Some(deadline) => tokio::select! {
                    () = turned => {}
                    () = again => {}
                    () = tokio::time::sleep_until(deadline) => {
                        return WaitOutput { events: Vec::new(), timed_out: true };
                    }
                },
                None => tokio::select! {
                    () = turned => {}
                    () = again => {}
                },
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
        if let Ok(conv) = self.conv(&session.id)
            && conv.is_busy().await
        {
            return None;
        }
        let record = self.session_dir(&session.id).record();
        let mine: Vec<Kept> = read(&record)
            .into_iter()
            .filter(|message| message.from == *from)
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
        let message = thread
            .entries
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
