//! The thread engine's first start (THREAD-PLAN Q14): every conversation the earlier engine
//! left, sessions and Chats alike, is deleted; nothing of them carries over.
//!
//! - **Once**: a store marked as the thread engine's (`EngineSwitched`) skips all of it, so a
//!   conversation made after the first start is never touched.
//! - **Exactly the old ones**: the ids are recorded first (`EngineSwitching`), in one write with
//!   each one's mark as being deleted and the default permission going back to Full access; a
//!   restart that cut the first start off goes on with that list, never with what exists by then.
//! - **The normal delete**: each one is then deleted as the user's Delete does: gone from the
//!   app at once, its cleanup in the background, and one a quit cuts off finished at the next
//!   launch. This runs before recovery and overnight resumption, which pass over a conversation
//!   being deleted, and before any client connects.

use super::SessionManager;
use crate::Result;
use crate::model::{ConversationId, PermissionLevel};

/// The engine a store's conversations belong to once its first start is over.
pub(crate) const ENGINE: &str = "thread-1";

impl SessionManager {
    /// Deletes what the earlier engine left, on the thread engine's first start; nothing after.
    /// Fails only if the conversations to delete could not be recorded: then nothing of them
    /// may resume either.
    pub(super) async fn switch_engine(&self) -> Result<()> {
        if self.core.engine().as_deref() == Some(ENGINE) {
            return Ok(());
        }
        let old = match self.core.engine_switch() {
            // A restart cut it off: the same conversations (marked in the same write), whatever
            // exists now.
            Some(old) => old,
            None => {
                let old: Vec<ConversationId> = self
                    .core
                    .catalog()
                    .conversations
                    .into_iter()
                    .map(|conversation| conversation.id)
                    .collect();
                let reset = self.core.settings().default_permission != PermissionLevel::FullAccess;
                if !old.is_empty() || reset {
                    self.core.begin_engine_switch(ENGINE, old.clone()).await?;
                }
                old
            }
        };
        let mut deleting = 0;
        for id in &old {
            // Gone already: deleted at this launch, as a delete a quit cut off, or with the
            // conversation it sat beside.
            if self.core.conversation(id).is_err() {
                continue;
            }
            match self.delete(id.clone()).await {
                Ok(()) => deleting += 1,
                // Still marked as being deleted: the next launch finishes it.
                Err(err) => {
                    tracing::warn!(conversation = %id, error = %err, "could not delete a conversation of the earlier engine; the next launch tries again")
                }
            }
        }
        // Without this record the next launch goes through the same list again, which is safe.
        if let Err(err) = self.core.finish_engine_switch(ENGINE).await {
            tracing::warn!(error = %err, "could not record the end of the thread engine's first start");
        }
        tracing::info!(
            listed = old.len(),
            deleting,
            "the thread engine's first start is deleting the earlier engine's conversations"
        );
        Ok(())
    }
}
