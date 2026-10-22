//! The one-time pass at the first start after the strip-down (STRIP-DOWN-PLAN.md S1): nothing
//! is migrated. Every session open then is archived through the normal archive path, and every
//! open Chat's CLI session is forgotten, so its next turn starts over without the old
//! instructions. A marker file in the data folder keeps it to one launch.

use super::SessionManager;
use crate::model::{ConversationKind, Lifecycle};

/// The marker of [`SessionManager::archive_open_at_upgrade`], in the data folder.
pub(super) const MARKER: &str = "strip-down-1";

impl SessionManager {
    pub(super) async fn archive_open_at_upgrade(&self) {
        let marker = self.data_dir.join(MARKER);
        if marker.exists() {
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
        if let Err(err) = std::fs::write(&marker, "") {
            tracing::warn!(error = %err, "could not record the upgrade pass; the next launch runs it again");
        }
        tracing::info!(archived, forgotten, "sessions open at the upgrade archived");
    }
}
