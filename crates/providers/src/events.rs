//! The event stream of a session, redacted on the way out.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::model::ProviderEvent;
use crate::redact::{EventRedactor, Redactor};
use crate::{DIFF_CLIP, clip};

/// Sends a session's events to its listener. Every event passes the session's redactor first,
/// which may hold a delta back (it could end in the start of a secret) and release it later.
pub(crate) struct Events {
    tx: mpsc::Sender<ProviderEvent>,
    redactor: Option<Mutex<EventRedactor>>,
}

impl Events {
    pub(crate) fn new(tx: mpsc::Sender<ProviderEvent>, redactor: Option<Arc<Redactor>>) -> Self {
        Self {
            tx,
            redactor: EventRedactor::new(redactor).map(Mutex::new),
        }
    }

    /// Sends `event`, redacted. Returns `false` once nobody listens any more.
    pub(crate) async fn send(&self, mut event: ProviderEvent) -> bool {
        let Some(redactor) = &self.redactor else {
            clip_diffs(&mut event);
            return self.tx.send(event).await.is_ok();
        };
        let mut out = Vec::with_capacity(1);
        redactor
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .apply(event, &mut out);
        let mut delivered = true;
        for mut event in out {
            clip_diffs(&mut event);
            delivered &= self.tx.send(event).await.is_ok();
        }
        delivered
    }
}

/// A file change's diff cut to [`DIFF_CLIP`], once redaction saw it whole.
fn clip_diffs(event: &mut ProviderEvent) {
    if let ProviderEvent::FileChanges { changes, .. } = event {
        for diff in changes.iter_mut().filter_map(|change| change.diff.as_mut()) {
            if diff.len() > DIFF_CLIP {
                *diff = clip(diff, DIFF_CLIP);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileChange, FileChangeKind, ItemStatus};

    #[tokio::test]
    async fn a_secret_across_the_diff_cut_is_redacted_whole_before_the_cut() {
        let secret = "sk-0123456789abcdef0123456789abcd";
        // The secret starts 30 bytes before the cut: cutting first would keep all but 2 of its 32.
        let diff = format!(
            "{}{secret}\n{}",
            "+".repeat(DIFF_CLIP - 30),
            "+x\n".repeat(100)
        );
        let (tx, mut rx) = mpsc::channel(4);
        let events = Events::new(tx, Some(Arc::new(Redactor::new([secret]))));
        let change = FileChange {
            path: "a".into(),
            kind: FileChangeKind::Update,
            diff: Some(diff),
        };
        let event = ProviderEvent::FileChanges {
            item_id: "i".into(),
            changes: vec![change],
            status: ItemStatus::Completed,
        };
        assert!(events.send(event).await);
        let Some(ProviderEvent::FileChanges { changes, .. }) = rx.recv().await else {
            panic!("no file change")
        };
        let shown = changes[0].diff.as_deref().unwrap();
        assert!(
            !shown.contains(&secret[..12]),
            "part of the secret was kept"
        );
        assert!(shown.len() <= DIFF_CLIP + 32, "the diff is clipped");
    }
}
