//! The event stream of a session, redacted on the way out.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::model::{KeptFile, ProviderEvent};
use crate::redact::{EventRedactor, Redactor};
use crate::{DIFF_CLIP, OUTPUT_CLIP, OUTPUT_KEEP_MAX, clip};

/// Sends a session's events to its listener. Every event passes the session's redactor first,
/// which may hold a delta back (it could end in the start of a secret) and release it later.
pub(crate) struct Events {
    tx: mpsc::Sender<ProviderEvent>,
    redactor: Option<Mutex<EventRedactor>>,
    /// The same redactor, for the files a result names ([`KeptFile`]).
    secrets: Option<Arc<Redactor>>,
}

impl Events {
    pub(crate) fn new(tx: mpsc::Sender<ProviderEvent>, redactor: Option<Arc<Redactor>>) -> Self {
        Self {
            tx,
            secrets: redactor.clone().filter(|redactor| !redactor.is_empty()),
            redactor: EventRedactor::new(redactor).map(Mutex::new),
        }
    }

    /// Sends `event`, redacted. Returns `false` once nobody listens any more.
    pub(crate) async fn send(&self, event: ProviderEvent) -> bool {
        let out = match &self.redactor {
            Some(redactor) => {
                let mut out = Vec::with_capacity(1);
                redactor
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .apply(event, &mut out);
                out
            }
            None => vec![event],
        };
        let mut delivered = true;
        for mut event in out {
            clip_diffs(&mut event);
            keep_output(&mut event, self.secrets.as_deref()).await;
            delivered &= self.tx.send(event).await.is_ok();
        }
        delivered
    }
}

/// A result cut to [`OUTPUT_CLIP`] once redaction saw it whole, with the whole of it waiting in
/// [`KeptOutput::whole`] for the core to store (BUILD-PLAN D2.1), and the file the CLI saved a
/// whole output to read into [`KeptFile::text`], redacted: the CLI may delete it with its
/// session, so it is copied when the event is seen (D2.2).
async fn keep_output(event: &mut ProviderEvent, secrets: Option<&Redactor>) {
    let (ProviderEvent::Command { output, kept, .. }
    | ProviderEvent::ToolCall { output, kept, .. }) = event
    else {
        return;
    };
    if let Some(file) = kept.as_mut().and_then(|kept| kept.file.as_mut()) {
        read_saved(file, secrets).await;
    }
    if let Some(text) = output.as_mut()
        && text.len() > OUTPUT_CLIP
    {
        let mut whole = std::mem::take(text);
        *text = clip(&whole, OUTPUT_CLIP);
        let kept = kept.get_or_insert_with(Box::default);
        kept.bytes = whole.len() as u64;
        if whole.len() > OUTPUT_KEEP_MAX {
            let cut = clip(&whole, OUTPUT_KEEP_MAX);
            whole = cut;
        }
        kept.whole = Some(whole.into());
    }
}

/// Reads the file `file` names into its `text`, redacted, or says why it couldn't.
async fn read_saved(file: &mut KeptFile, secrets: Option<&Redactor>) {
    use tokio::io::AsyncReadExt as _;
    let read = async {
        let handle = crate::model::open_tool_results_file(&file.path).await?;
        let size = handle.metadata().await?.len();
        let mut bytes = Vec::new();
        handle
            .take(OUTPUT_KEEP_MAX as u64)
            .read_to_end(&mut bytes)
            .await?;
        std::io::Result::Ok((bytes, size))
    };
    match read.await {
        Ok((bytes, size)) => {
            let mut text = String::from_utf8_lossy(&bytes).into_owned();
            if let Some(secrets) = secrets {
                secrets.redact_in_place(&mut text);
            }
            if size > bytes.len() as u64 {
                text.push_str(&format!("… [{} more bytes]", size - bytes.len() as u64));
            }
            file.bytes = size;
            file.text = Some(text.into());
        }
        Err(err) => file.error = Some(format!("couldn't read it: {err}")),
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
    use crate::model::{FileChange, FileChangeKind, ItemStatus, KeptOutput};

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
            original: Some(secret.into()),
            path: "a".into(),
            moved_to: None,
            kind: FileChangeKind::Update,
            diff: Some(diff),
            added: None,
            removed: None,
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
        assert!(!changes[0].original.as_deref().unwrap().contains(secret));
        let shown = changes[0].diff.as_deref().unwrap();
        assert!(
            !shown.contains(&secret[..12]),
            "part of the secret was kept"
        );
        assert!(shown.len() <= DIFF_CLIP + 32, "the diff is clipped");
    }

    fn command(output: &str, kept: Option<KeptOutput>) -> ProviderEvent {
        let kept = kept.map(Box::new);
        ProviderEvent::Command {
            item_id: "c".into(),
            command: "make".into(),
            cwd: None,
            status: ItemStatus::Completed,
            exit_code: Some(0),
            output: Some(output.into()),
            duration_ms: None,
            kept,
        }
    }

    async fn sent(
        events: &Events,
        event: ProviderEvent,
        rx: &mut mpsc::Receiver<ProviderEvent>,
    ) -> (String, Option<Box<KeptOutput>>) {
        assert!(events.send(event).await);
        match rx.recv().await {
            Some(ProviderEvent::Command { output, kept, .. }) => (output.unwrap(), kept),
            other => panic!("not a command: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_long_output_is_cut_once_redacted_and_kept_whole_for_the_store() {
        let secret = "sk-0123456789abcdef0123456789abcd";
        let output = format!(
            "{}{secret}\n{}PLANTED\n{}",
            "a".repeat(OUTPUT_CLIP - 10),
            "b\n".repeat(20_000),
            "c\n".repeat(50_000)
        );
        let (tx, mut rx) = mpsc::channel(4);
        let events = Events::new(tx, Some(Arc::new(Redactor::new([secret]))));
        let (shown, kept) = sent(&events, command(&output, None), &mut rx).await;
        assert!(shown.len() <= OUTPUT_CLIP + 32, "the event's output is cut");
        assert!(
            !shown.contains(&secret[..12]),
            "part of the secret was kept"
        );
        let kept = kept.expect("the whole output waits to be stored");
        let whole = kept.whole.as_deref().unwrap();
        assert!(whole.contains("PLANTED") && whole.contains(crate::REDACTED));
        assert!(!whole.contains(secret));
        assert_eq!(kept.bytes, whole.len() as u64);
        assert!(!kept.is_complete(), "not stored yet");

        // A short one stays as it was, with nothing to keep.
        let (shown, kept) = sent(&events, command("ok\n", None), &mut rx).await;
        assert_eq!((shown.as_str(), kept), ("ok\n", None));
    }

    #[tokio::test]
    async fn the_file_a_preview_names_is_read_redacted_and_a_missing_one_is_said() {
        let root = std::env::temp_dir().join(format!("brigadier-kept-{}", uuid::Uuid::new_v4()));
        let dir = root.join("projects/-tmp-p/s1");
        std::fs::create_dir_all(dir.join("tool-results")).unwrap();
        let path = dir.join("tool-results/x.txt");
        let secret = "sk-0123456789abcdef0123456789abcd";
        std::fs::write(
            &path,
            format!("{}MARKER {secret}\n", "line\n".repeat(40_000)),
        )
        .unwrap();
        let (tx, mut rx) = mpsc::channel(4);
        let events = Events::new(tx, Some(Arc::new(Redactor::new([secret]))));
        let file = |path: &std::path::Path| KeptOutput {
            file: Some(KeptFile {
                path: path.display().to_string(),
                ..KeptFile::default()
            }),
            ..KeptOutput::default()
        };
        let (_, kept) = sent(&events, command("preview", Some(file(&path))), &mut rx).await;
        let file_kept = kept.unwrap().file.unwrap();
        let text = file_kept.text.as_deref().unwrap();
        assert!(text.contains("MARKER") && !text.contains(secret));
        assert_eq!(file_kept.bytes, std::fs::metadata(&path).unwrap().len());
        assert!(file_kept.error.is_none());

        let gone = dir.join("tool-results/gone.txt");
        let (_, kept) = sent(&events, command("preview", Some(file(&gone))), &mut rx).await;
        let kept = kept.unwrap();
        assert!(
            kept.file
                .as_ref()
                .unwrap()
                .error
                .as_deref()
                .unwrap()
                .starts_with("couldn't read it")
        );
        assert!(!kept.is_complete());

        // A command can print the CLI's words with any path: a link out of the folder, or a
        // path that climbs out of it, is never read.
        let outside = root.join("secret.txt");
        std::fs::write(&outside, "SECRET-FILE").unwrap();
        let climb = dir.join("tool-results/../../../../secret.txt");
        #[cfg(unix)]
        let paths = {
            let link = dir.join("tool-results/link.txt");
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            vec![link, climb]
        };
        #[cfg(not(unix))]
        let paths = vec![climb];
        for path in paths {
            let (_, kept) = sent(&events, command("preview", Some(file(&path))), &mut rx).await;
            let file_kept = kept.unwrap().file.unwrap();
            assert!(file_kept.text.is_none(), "{path:?} was read");
            assert!(file_kept.error.is_some());
        }
        std::fs::remove_dir_all(&root).unwrap();
    }
}
