//! Side chats (the "Side chat" tab, ⌥⌘S): a temporary Chat beside a conversation, for
//! questions about it that shouldn't enter its thread. Each of its turns carries the
//! conversation's latest messages; like any Chat it changes nothing. It goes when its tab
//! closes, or with the conversation.

use super::SessionManager;
use crate::model::{Conversation, ConversationId, ConversationKind, Lifecycle, Setup};
use crate::sessions::Origin;
use crate::{Error, Result};

const SIDE_CHAT_TITLE: &str = "Side chat";

impl SessionManager {
    /// The conversation's side chat, started if it has none, on the model the conversation
    /// uses.
    pub async fn open_side_chat(
        &self,
        parent: &ConversationId,
        side_id: Option<ConversationId>,
    ) -> Result<Conversation> {
        let conversation = self.core.conversation(parent)?;
        if conversation.side_of.is_some() {
            return Err(Error::Invalid(
                "a side chat has no side chat of its own".into(),
            ));
        }
        if conversation.lifecycle == Lifecycle::Archived {
            return Err(Error::Invalid(
                "an archived conversation has no side chat".into(),
            ));
        }
        if let Some(id) = side_id.as_ref() {
            if uuid::Uuid::parse_str(&id.0).is_err() {
                return Err(Error::Invalid("side chat id must be a UUID".into()));
            }
            if let Ok(existing) = self.core.conversation(id) {
                if existing.side_of.as_ref() != Some(parent)
                    || existing.lifecycle == Lifecycle::Archived
                {
                    return Err(Error::Invalid(
                        "this side chat does not belong to the session".into(),
                    ));
                }
                return Ok(existing);
            }
        } else if let Some(existing) = self.side_chats(parent).into_iter().next() {
            return Ok(existing);
        }
        let model = match conversation.setup {
            Some(Setup::Session { orchestrator, .. }) => orchestrator,
            Some(Setup::Chat { model }) => model,
            None => {
                return Err(Error::Invalid(
                    "the conversation has no model yet; send it a message first".into(),
                ));
            }
        };
        self.core
            .create_conversation(
                side_id.unwrap_or_else(ConversationId::generate),
                ConversationKind::Chat,
                None,
                Some(SIDE_CHAT_TITLE.into()),
                Some(Setup::Chat { model }),
                Origin {
                    forked_from: None,
                    side_of: Some(parent.clone()),
                },
            )
            .await
    }

    /// The conversation's side chats that weren't archived.
    pub(super) fn side_chats(&self, parent: &ConversationId) -> Vec<Conversation> {
        self.core
            .catalog()
            .conversations
            .into_iter()
            .filter(|side| {
                side.side_of.as_ref() == Some(parent) && side.lifecycle != Lifecycle::Archived
            })
            .collect()
    }

    /// Deletes a conversation's side chats: it was archived or deleted.
    pub(super) async fn delete_side_chats(&self, parent: &ConversationId) {
        for side in self.side_chats(parent) {
            // A side chat has none of its own, so this goes one level deep.
            if let Err(err) =
                Box::pin(self.delete_conversation(side.id.clone(), false, false)).await
            {
                tracing::warn!(side_chat = %side.id, error = %err, "could not delete a side chat");
            }
        }
    }
}
