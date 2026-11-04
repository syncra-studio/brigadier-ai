//! What the inbox reads from the boards. It adds no GitHub polling: a pull request is the one the
//! board last saw.

use crate::board::Board;
use crate::model::{ConversationId, Lifecycle};
use crate::work::PullRequestState;

use super::SessionManager;

/// Whether `board`'s pull request was last seen merged: its row's Merged chip.
fn pull_request_merged(board: &Board) -> bool {
    board
        .pull_request
        .as_ref()
        .is_some_and(|pr| pr.state == PullRequestState::Merged)
}

impl SessionManager {
    /// The open sessions whose pull request merged.
    pub async fn merged_sessions(&self) -> Vec<ConversationId> {
        let mut merged = Vec::new();
        for tab in self.core.visible_catalog().conversations {
            if tab.is_tab()
                && tab.lifecycle != Lifecycle::Archived
                && let Ok(board) = self.core.board(&tab.id).await
                && pull_request_merged(&board)
            {
                merged.push(tab.id);
            }
        }
        merged
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use brigadier_store::{NewEvent, Store, StoreConfig, StreamPage};

    use super::*;
    use crate::Core;
    use crate::model::{ConversationKind, DomainEvent, Lifecycle, streams};
    use crate::sessions::Origin;
    use crate::work::PullRequest;

    /// A folder in the temp directory, removed when dropped.
    struct Temp(PathBuf);

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn store() -> (Temp, Store) {
        let dir =
            Temp(std::env::temp_dir().join(format!("brigadier-inbox-{}", uuid::Uuid::now_v7())));
        let store = Store::open(StoreConfig {
            db_path: dir.0.join("test.db"),
            blobs_dir: dir.0.join("blobs"),
            readers: 1,
        })
        .unwrap();
        (dir, store)
    }

    async fn chat(core: &Core) -> ConversationId {
        core.create_conversation(
            ConversationId::generate(),
            ConversationKind::Chat,
            None,
            None,
            None,
            Origin::default(),
        )
        .await
        .unwrap()
        .id
    }

    fn sent(core: &Core, id: &ConversationId) -> Option<i64> {
        core.conversation(id).unwrap().last_sent_at_ms
    }

    #[test]
    fn only_a_merged_pull_request_earns_the_chip() {
        let with = |state| {
            let pr: PullRequest = serde_json::from_value(serde_json::json!({
                "number": 7, "title": "t", "url": "u", "state": "open"
            }))
            .unwrap();
            Board {
                pull_request: Some(Box::new(PullRequest { state, ..pr })),
                ..Board::default()
            }
        };
        assert!(pull_request_merged(&with(PullRequestState::Merged)));
        assert!(!pull_request_merged(&with(PullRequestState::Open)));
        assert!(!pull_request_merged(&with(PullRequestState::Closed)));
        assert!(!pull_request_merged(&Board::default()));
    }

    #[tokio::test]
    async fn only_the_users_send_moves_the_sort_time_and_it_survives_a_reload() {
        let (_dir, store) = store();
        let core = Core::load(store.clone()).await.unwrap();
        let id = chat(&core).await;
        let created = core.conversation(&id).unwrap().created_at_ms;
        assert_eq!(sent(&core, &id), Some(created), "its creation until then");

        tokio::time::sleep(std::time::Duration::from_millis(2)).await;

        // Everything else that touches the row: a message appended by a turn or a queue, a
        // finish, a rename, a pin, sleep, wake, archive and restore.
        core.append_message(id.clone(), "from a queue".into())
            .await
            .unwrap();
        core.record(vec![(
            streams::conversation(&id),
            DomainEvent::RequestFinished {
                conversation_id: id.clone(),
                request_id: "r".into(),
            },
        )])
        .await
        .unwrap();
        core.rename_conversation(id.clone(), "Renamed".into())
            .await
            .unwrap();
        core.set_pinned(id.clone(), true).await.unwrap();
        core.set_pinned(id.clone(), false).await.unwrap();
        for lifecycle in [
            Lifecycle::Hibernated,
            Lifecycle::Active,
            Lifecycle::Archived,
            Lifecycle::Active,
        ] {
            core.set_lifecycle(id.clone(), lifecycle).await.unwrap();
        }
        assert_eq!(sent(&core, &id), Some(created), "none of those moved it");

        core.mark_user_sent(id.clone()).await.unwrap();
        let moved = sent(&core, &id).unwrap();
        assert!(moved > created, "the user's message moves it");

        let reloaded = Core::load(store).await.unwrap();
        assert_eq!(sent(&reloaded, &id), Some(moved));
    }

    /// Stores an event as an older Brigadier wrote it.
    async fn stored_as_old(store: &Store, stream: String, kind: &str, event: serde_json::Value) {
        store
            .append(vec![NewEvent::new(stream, kind, 5, &event).unwrap()])
            .await
            .unwrap();
    }

    /// A chat as an older Brigadier stored it: fields since removed, and no sort time.
    async fn old_chat(store: &Store, id: &ConversationId) {
        stored_as_old(
            store,
            streams::CATALOG.into(),
            "conversation.created",
            serde_json::json!({ "type": "conversationCreated", "conversation": {
                "id": id, "kind": "chat", "projectId": null, "title": "Old", "pinnedAtMs": null,
                "createdAtMs": 7, "updatedAtMs": 7, "doneAtMs": 5, "snoozedUntilMs": 9,
                "snoozedAtMs": 5
            }}),
        )
        .await;
    }

    async fn old_message(store: &Store, id: &ConversationId, role: &str, at_ms: i64) {
        stored_as_old(
            store,
            streams::conversation(id),
            "message.appended",
            serde_json::json!({ "type": "messageAppended", "message": {
                "id": format!("m{at_ms}"), "conversationId": id, "seq": 0, "role": role,
                "text": "words", "blob": null, "createdAtMs": at_ms
            }}),
        )
        .await;
    }

    #[tokio::test]
    async fn old_done_and_snoozed_rows_load_back_in_the_list_with_a_sort_time() {
        let (_dir, store) = store();
        let (bare, talked, forked) = (
            ConversationId::generate(),
            ConversationId::generate(),
            ConversationId::generate(),
        );
        for id in [&bare, &talked, &forked] {
            old_chat(&store, id).await;
            stored_as_old(
                &store,
                streams::CATALOG.into(),
                "conversation.done",
                serde_json::json!({ "type": "conversationDone", "id": id, "doneAtMs": 5 }),
            )
            .await;
            stored_as_old(
                &store,
                streams::CATALOG.into(),
                "conversation.snoozed",
                serde_json::json!({
                    "type": "conversationSnoozed", "id": id, "untilMs": 9, "atMs": 5
                }),
            )
            .await;
        }
        old_message(&store, &talked, "user", 100).await;
        old_message(&store, &talked, "peer", 200).await;
        old_message(&store, &talked, "assistant", 300).await;
        // Copied from its source, from before it was created.
        old_message(&store, &forked, "user", 3).await;

        let core = Core::load(store.clone()).await.unwrap();
        assert_eq!(sent(&core, &bare), Some(7), "no message: its created time");
        assert_eq!(
            sent(&core, &talked),
            Some(100),
            "the user's, not the others'"
        );
        assert_eq!(sent(&core, &forked), Some(7), "never before it was created");

        // Written once: the next launch reads and writes nothing for it.
        let catalog = || {
            store.read_stream(
                streams::CATALOG.into(),
                StreamPage {
                    limit: 1000,
                    ..StreamPage::default()
                },
            )
        };
        let events = catalog().await.unwrap().len();
        let reloaded = Core::load(store.clone()).await.unwrap();
        assert_eq!(catalog().await.unwrap().len(), events);
        assert_eq!(sent(&reloaded, &talked), Some(100));
    }
}
