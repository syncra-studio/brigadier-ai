//! The one-time pass at the first start after the strip-down (STRIP-DOWN-PLAN.md S1): nothing
//! is migrated. Every session open then is archived through the normal archive path, and every
//! open Chat's CLI session is forgotten, so its next turn starts over without the old
//! instructions. A marker file in the data folder keeps it to one launch. S7's session folders
//! (`session_dir`) ran it again once: sessions and Chats then move to folders of their own.
//! S23's seam runs it once more: a session open then was built for the context restarts it no
//! longer gets.

use super::SessionManager;
use crate::model::{ConversationKind, Lifecycle};

/// The markers of [`SessionManager::archive_open_at_upgrade`], in the data folder: the pass
/// runs once more for each new one.
pub(super) const MARKERS: [&str; 3] = ["strip-down-1", "session-folders", "seam-on"];

impl SessionManager {
    pub(super) async fn archive_open_at_upgrade(&self) {
        if MARKERS
            .iter()
            .all(|marker| self.data_dir.join(marker).exists())
        {
            return;
        }
        let (mut archived, mut forgotten) = (0, 0);
        for conversation in self.core.catalog().conversations {
            if conversation.deleting || conversation.lifecycle == Lifecycle::Archived {
                continue;
            }
            if conversation.is_tab() {
                match self.archive(conversation.id.clone()).await {
                    Ok(_) => archived += 1,
                    Err(err) => {
                        tracing::warn!(conversation = %conversation.id, error = %err, "could not archive a session open at the upgrade")
                    }
                }
            } else if conversation.kind == ConversationKind::Chat {
                self.forget_native_session(&conversation.id).await;
                forgotten += 1;
            }
        }
        for marker in MARKERS {
            if let Err(err) = std::fs::write(self.data_dir.join(marker), "") {
                tracing::warn!(error = %err, "could not record the upgrade pass; the next launch runs it again");
            }
        }
        tracing::info!(archived, forgotten, "sessions open at the upgrade archived");
    }
}
