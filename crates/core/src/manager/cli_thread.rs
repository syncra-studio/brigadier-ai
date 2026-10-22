//! The thread from the CLI's own files (S9, [`crate::thread_view`]): the live stream's items
//! of the running turn, the turn ends written to `record/turns.jsonl`, and the view read back.

use brigadier_providers::session_file::{self, FileEntry, FileImage};
use brigadier_providers::{ProviderEvent, Role, TurnStatus};

use super::SessionManager;
use super::conversation::ConvLive;
use super::session_dir::read_lineage;
use crate::model::ConversationId;
use crate::thread_view::{self, LiveTurn, ThreadNews, ThreadView, TurnEnd, TurnRecord};
use crate::{Error, Result, now_ms};

impl SessionManager {
    /// Follows the running turn of `conv`'s CLI through one of its events (`None`: the CLI's
    /// events ended): its items, and its end, written to `record/turns.jsonl`.
    pub(super) async fn follow_cli_thread(&self, conv: &ConvLive, event: Option<&ProviderEvent>) {
        // A tab's chat keeps nothing of its own.
        if conv.private_logs {
            return;
        }
        let at = now_ms();
        let mut news = Vec::new();
        let mut ended = None;
        {
            let mut threads = self.cli_threads.lock().unwrap();
            let live = threads.entry(conv.id.clone()).or_default();
            match event {
                Some(ProviderEvent::TurnStarted { .. }) => {
                    if !live.open {
                        *live = LiveTurn {
                            open: true,
                            awaiting_user: true,
                            ..LiveTurn::default()
                        };
                    }
                }
                Some(ProviderEvent::Message {
                    item_id,
                    role: Role::User,
                    ..
                }) => {
                    let steer = live.open && !live.awaiting_user;
                    if !live.open {
                        *live = LiveTurn {
                            open: true,
                            ..LiveTurn::default()
                        };
                    }
                    if !steer {
                        live.awaiting_user = false;
                        live.user_message = Some(item_id.clone());
                    }
                    let entry = FileEntry {
                        steer,
                        ..entry(event.cloned(), at)
                    };
                    news.push(entry);
                }
                Some(ProviderEvent::Error { error }) if live.open => {
                    live.error = Some(error.message.clone());
                    news.push(entry(event.cloned(), at));
                }
                Some(ProviderEvent::TurnCompleted { status, .. }) if live.open => {
                    let end = match status {
                        TurnStatus::Completed if live.error.is_some() => TurnEnd::Failed,
                        TurnStatus::Completed => TurnEnd::Done,
                        TurnStatus::Interrupted => TurnEnd::Stopped,
                        TurnStatus::Failed => TurnEnd::Failed,
                    };
                    ended = Some(end);
                }
                Some(ProviderEvent::Exited { .. }) | None if live.open => {
                    ended = Some(TurnEnd::Interrupted);
                }
                Some(event) if shown_live(event) => news.push(entry(Some(event.clone()), at)),
                _ => {}
            }
            if !news.is_empty() {
                live.entries = session_file::merge(std::mem::take(&mut live.entries), news.clone());
            }
            if let Some(end) = ended {
                live.open = false;
                let record = TurnRecord {
                    user_message: live.user_message.clone(),
                    end,
                    error: live.error.take(),
                    ended_at_ms: at,
                };
                let dir = self.session_dir(&conv.id).record();
                if let Err(err) = thread_view::append_turn(&dir, &record) {
                    tracing::warn!(conversation = %conv.id, error = %err, "could not note a turn's end");
                }
            }
        }
        if !news.is_empty() || ended.is_some() {
            let _ = self.thread_news.send(ThreadNews {
                conversation_id: conv.id.clone(),
                entries: news,
                turn_ended: ended.is_some(),
            });
        }
    }

    /// Conversation `id`'s thread as its CLI keeps it.
    pub fn thread_view(&self, id: &ConversationId) -> Result<ThreadView> {
        self.core.conversation(id)?;
        let record = self.session_dir(id).record();
        let files = read_lineage(&record)
            .into_iter()
            .filter_map(|session| {
                let file = session.file?;
                Some(
                    self.session_files
                        .read(session.provider, std::path::Path::new(&file)),
                )
            })
            .collect();
        let records = thread_view::read_turns(&record);
        let threads = self.cli_threads.lock().unwrap();
        Ok(thread_view::view(files, threads.get(id), &records))
    }

    /// An image a CLI session file of conversation `id` holds.
    /// Base64, with its media type.
    pub fn thread_image(&self, id: &ConversationId, image: &FileImage) -> Result<(String, String)> {
        let ours = read_lineage(&self.session_dir(id).record())
            .iter()
            .filter_map(|session| session.file.as_deref())
            .any(|file| {
                let file = std::path::Path::new(file);
                let image = std::path::Path::new(&image.file);
                image == file || image.starts_with(file.with_extension("").join("subagents"))
            });
        if !ours {
            return Err(Error::NotFound(format!("image in {}", image.file)));
        }
        use base64::Engine as _;
        let bytes =
            session_file::read_image(image).map_err(|err| Error::Invalid(err.to_string()))?;
        Ok((
            base64::engine::general_purpose::STANDARD.encode(bytes),
            image.mime.clone(),
        ))
    }

    /// What live streams add to threads, as it comes.
    pub fn subscribe_thread_news(&self) -> tokio::sync::broadcast::Receiver<ThreadNews> {
        self.thread_news.subscribe()
    }
}

fn entry(event: Option<ProviderEvent>, at: i64) -> FileEntry {
    FileEntry {
        event: event.expect("an event"),
        at_ms: Some(at),
        parent: None,
        steer: false,
        images: Vec::new(),
    }
}

/// What the live thread shows of the stream: its items and their deltas.
fn shown_live(event: &ProviderEvent) -> bool {
    matches!(
        event,
        ProviderEvent::Message { .. }
            | ProviderEvent::MessageDelta { .. }
            | ProviderEvent::Reasoning { .. }
            | ProviderEvent::ReasoningDelta { .. }
            | ProviderEvent::ToolCall { .. }
            | ProviderEvent::Command { .. }
            | ProviderEvent::CommandOutputDelta { .. }
            | ProviderEvent::FileChanges { .. }
            | ProviderEvent::Image { .. }
            | ProviderEvent::CompactionEnded { .. }
    )
}
