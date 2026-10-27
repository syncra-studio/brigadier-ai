use super::SessionManager;
use crate::{ConversationId, ConversationKind, Error, Result};

impl SessionManager {
    pub(crate) fn visuals_guide(&self, id: &ConversationId) -> Result<String> {
        let conversation = self.core.conversation(id)?;
        if conversation.kind != ConversationKind::Chat
            || conversation.side_of.is_some()
            || conversation.tab_chat.is_some()
        {
            return Err(Error::Invalid(
                "Drawing is available in top-level Chats only.".into(),
            ));
        }
        Ok(include_str!("GUIDE.md").to_owned())
    }
}
