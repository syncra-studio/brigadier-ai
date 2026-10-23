//! Brigadier pushes nothing (STRIP-DOWN-PLAN.md S1, G1): an idle thread gets no turn from
//! engine events. Two hours of the engine's timers, a sibling session changing the same file
//! and merging, and the user's settings changing start nothing. A user message still starts a
//! turn, and so does a report of a worker the thread started. Sessions open when the
//! strip-down first starts are archived, not carried over.

use std::sync::Arc;

use super::{Flow, Options, Reply};
use crate::model::{Lifecycle, Settings};

async fn edit_settings(flow: &Flow, edit: impl FnOnce(&mut Settings)) {
    let mut settings = flow.core.settings();
    edit(&mut settings);
    flow.core.update_settings(settings).await.unwrap();
}

/// The strip-down's first start archives the sessions open then, through the normal archive
/// path, and so does the first start with session folders (S7) and with the seam (S23); a later
/// start leaves sessions alone.
#[tokio::test]
async fn sessions_open_at_the_upgrade_are_archived() {
    let mut flow = Flow::start(
        "no-push-upgrade",
        Options::default(),
        Arc::new(|_| Box::pin(async { Reply::text("Hi.") })),
    )
    .await;
    flow.say("Hello.").await;
    flow.settled().await;
    for marker in &crate::manager::upgrade::MARKERS[1..] {
        // As a data folder from before that step: its marker isn't there yet.
        let marker = flow.dir.join("data").join(marker);
        std::fs::remove_file(&marker).unwrap();
        flow.restart().await;
        assert_eq!(
            flow.core
                .conversation(&flow.conversation)
                .unwrap()
                .lifecycle,
            Lifecycle::Archived,
            "{marker:?}"
        );
        assert!(marker.exists());
        let restored = flow.manager.restore(flow.conversation.clone()).await;
        assert!(restored.is_ok(), "{restored:?}");
    }
    // Once only: a session open at a later start stays open.
    flow.restart().await;
    assert_ne!(
        flow.core
            .conversation(&flow.conversation)
            .unwrap()
            .lifecycle,
        Lifecycle::Archived
    );
    flow.stop().await;
}
