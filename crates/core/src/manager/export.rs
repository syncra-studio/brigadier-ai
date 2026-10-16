//! The streamed transcript export (BUILD-PLAN D2.1): what a fresh session continuing a worker's
//! or a thread's work reads instead of the CLI's own session files, which may be gone.
//!
//! - **Streamed.** Events are read a page at a time, oldest first, and each is written as it is
//!   read: nothing holds the whole transcript in memory.
//! - **Whole outputs beside it.** An event's output is cut at 8 KiB; its whole copy, kept when
//!   the event was seen ([`super::kept`]), is copied to `outputs/` and named where the output is
//!   cut. So is the file a Claude preview named.
//! - **Honest.** What couldn't be exported is listed, by event, in [`Export::gaps`] and in the
//!   transcript's last section: a page that couldn't be read (and every event after it), an
//!   event that couldn't be decoded, a write that failed (the export stops there), a kept copy
//!   that wasn't stored or is missing, and an output cut before Brigadier kept outputs whole.
//!   The transcript says on its first line that it is complete only if it ends with
//!   [`COMPLETE_LINE`].

use std::collections::HashMap;
use std::fmt::Write as _;
use std::future::Future;
use std::path::{Path, PathBuf};

use brigadier_providers::{ItemStatus, KeptFile, KeptOutput, ProviderEvent, Role};
use brigadier_store::{BlobHash, StoredEvent};
use tokio::io::{AsyncWrite, AsyncWriteExt as _};

use super::SessionManager;
use crate::model::{ConversationId, DomainEvent, TaskId, streams};
use crate::work::OrchestratorEntry;

/// Events read per page.
const EVENTS_PAGE: u32 = 2_000;
/// The last messages a fresh session gets word for word.
pub(crate) const TAIL_MESSAGES: usize = 6;
/// The transcript's last line when nothing is missing.
pub(crate) const COMPLETE_LINE: &str = "End of the transcript: nothing is missing.";
/// The folder beside the transcript the whole outputs are copied to.
pub(crate) const OUTPUTS_DIR: &str = "outputs";

/// What an export wrote, and what it couldn't.
#[derive(Debug, Default)]
pub(crate) struct Export {
    /// The last messages, oldest first.
    pub tail: Vec<String>,
    /// What is missing, one line each, by event.
    pub gaps: Vec<String>,
    /// Whole outputs copied to `outputs/`.
    pub outputs: usize,
}

impl Export {
    pub(crate) fn complete(&self) -> bool {
        self.gaps.is_empty()
    }

    /// What a fresh session's first message says of the transcript.
    pub(crate) fn summary(&self, file: &str) -> String {
        let outputs = if self.outputs > 0 {
            format!(
                " Each output cut there names its whole copy in {OUTPUTS_DIR}/ ({} in all).",
                self.outputs
            )
        } else {
            String::new()
        };
        if self.complete() {
            return format!("{file} is complete.{outputs}");
        }
        const SHOWN: usize = 5;
        let mut text = format!(
            "{file} is NOT complete: {} part(s) are missing, listed in its last section.{outputs} \
             Missing: ",
            self.gaps.len()
        );
        text.push_str(
            &self
                .gaps
                .iter()
                .take(SHOWN)
                .cloned()
                .collect::<Vec<_>>()
                .join("; "),
        );
        if self.gaps.len() > SHOWN {
            let _ = write!(text, "; and {} more", self.gaps.len() - SHOWN);
        }
        text.push_str(
            ". Don't guess what is missing: re-read or rerun what you need, or say it is unknown.",
        );
        text
    }
}

/// One stored event, as the export reads it.
pub(crate) enum Decoded {
    Event(ProviderEvent),
    /// Not part of the transcript.
    Skip,
    /// It couldn't be read.
    Bad(String),
}

/// A worker's CLI event (`task:` stream).
pub(crate) fn worker_event(stored: &StoredEvent) -> Decoded {
    if stored.kind != "worker.event" {
        return Decoded::Skip;
    }
    match serde_json::from_str::<DomainEvent>(stored.payload.get()) {
        Ok(DomainEvent::WorkerEvent { event, .. }) => Decoded::Event(event),
        Ok(_) => Decoded::Skip,
        Err(err) => Decoded::Bad(err.to_string()),
    }
}

/// A thread's CLI event (`orch:` stream).
#[allow(dead_code)] // The thread's mid-turn rebirth uses it next (BUILD-PLAN D2.3b).
pub(crate) fn thread_event(stored: &StoredEvent) -> Decoded {
    if stored.kind != "orchestrator.logged" {
        return Decoded::Skip;
    }
    match serde_json::from_str::<DomainEvent>(stored.payload.get()) {
        Ok(DomainEvent::OrchestratorLogged {
            entry: OrchestratorEntry::Provider { event, .. },
            ..
        }) => Decoded::Event(event),
        Ok(_) => Decoded::Skip,
        Err(err) => Decoded::Bad(err.to_string()),
    }
}

impl SessionManager {
    /// A worker hand-off's transcript.md and outputs/ in `dir` (`<scratch>/handoff`), written
    /// again from the start: an earlier hand-off's outputs go first.
    pub(crate) async fn export_handoff_transcript(
        &self,
        task: &crate::work::Task,
        dir: &Path,
    ) -> Export {
        if let Err(err) = tokio::fs::remove_dir_all(dir.join(OUTPUTS_DIR)).await
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(task = %task.id, error = %err, "could not clear an earlier hand-off's outputs");
        }
        self.export_worker_transcript(&task.id, dir).await
    }

    /// Exports the task's worker transcript to `dir`/transcript.md, with whole outputs in
    /// `dir`/outputs/.
    pub(crate) async fn export_worker_transcript(&self, id: &TaskId, dir: &Path) -> Export {
        self.export_stream(streams::task(id), worker_event, "The worker", dir)
            .await
    }

    /// Exports the conversation's thread transcript likewise.
    #[allow(dead_code)] // The thread's mid-turn rebirth uses it next (BUILD-PLAN D2.3b).
    pub(crate) async fn export_thread_transcript(&self, id: &ConversationId, dir: &Path) -> Export {
        self.export_stream(streams::orchestrator(id), thread_event, "The thread", dir)
            .await
    }

    async fn export_stream(
        &self,
        stream: String,
        decode: fn(&StoredEvent) -> Decoded,
        actor: &str,
        dir: &Path,
    ) -> Export {
        let store = self.core.store();
        let file = match tokio::fs::create_dir_all(dir).await {
            Ok(()) => tokio::fs::File::create(dir.join("transcript.md")).await,
            Err(err) => Err(err),
        };
        let file = match file {
            Ok(file) => file,
            Err(err) => {
                return Export {
                    gaps: vec![format!(
                        "the whole transcript: it couldn't be written ({err})"
                    )],
                    ..Export::default()
                };
            }
        };
        let blobs = store.blobs().clone();
        let late = late_copies(store, &stream).await;
        export(
            |after| store.read_stream_since(stream.clone(), after, EVENTS_PAGE),
            |hash: String| {
                let blobs = blobs.clone();
                async move {
                    let hash: BlobHash = hash.parse()?;
                    Ok(blobs.file_of(&hash))
                }
            },
            &late,
            decode,
            actor,
            tokio::io::BufWriter::new(file),
            &dir.join(OUTPUTS_DIR),
        )
        .await
    }
}

/// Writes the transcript `read` pages through to `out`, and the kept outputs `blob` names into
/// `outputs`. `read(after)` gives the stream's events after `after`, oldest first; `blob(hash)`
/// the file holding a blob.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn export<R, RF, B, BF, W>(
    mut read: R,
    blob: B,
    late: &HashMap<String, KeptFile>,
    decode: fn(&StoredEvent) -> Decoded,
    actor: &str,
    mut out: W,
    outputs: &Path,
) -> Export
where
    R: FnMut(i64) -> RF,
    RF: Future<Output = brigadier_store::Result<Vec<StoredEvent>>>,
    B: Fn(String) -> BF,
    BF: Future<Output = brigadier_store::Result<PathBuf>>,
    W: AsyncWrite + Unpin,
{
    let mut export = Export::default();
    let mut tail: Vec<String> = Vec::new();
    let head = format!(
        "# The full transcript\n\nEvery message, command and tool call, oldest first, as \
         Brigadier recorded them. Outputs are cut at 8 KiB; where one is cut, its whole copy is \
         named in {OUTPUTS_DIR}/. This file is complete only if its last line is \"{COMPLETE_LINE}\"\n"
    );
    if let Err(err) = out.write_all(head.as_bytes()).await {
        export.gaps.push(format!(
            "the whole transcript: it couldn't be written ({err})"
        ));
        return export;
    }
    let mut after = 0;
    'pages: loop {
        let page = match read(after).await {
            Ok(page) => page,
            Err(err) => {
                export.gaps.push(format!(
                    "every event after #{after}: they couldn't be read ({err})"
                ));
                break;
            }
        };
        let Some(last) = page.last() else {
            break;
        };
        let full = page.len() == EVENTS_PAGE as usize;
        after = last.stream_seq;
        for stored in &page {
            let event = match decode(stored) {
                Decoded::Event(event) => with_late_copy(event, late),
                Decoded::Skip => continue,
                Decoded::Bad(err) => {
                    export.gaps.push(format!(
                        "event #{}: it couldn't be decoded ({err})",
                        stored.stream_seq
                    ));
                    continue;
                }
            };
            if let ProviderEvent::Message {
                role: Role::Assistant,
                text,
                ..
            } = &event
            {
                tail.push(text.clone());
            }
            let mut text = String::new();
            entry(&mut text, &event, actor);
            if let Some(kept) = kept_of(&event) {
                text.push_str(
                    &copy_kept(kept, stored.stream_seq, &blob, outputs, &mut export).await,
                );
            } else if let Some(gap) = cut_without_copy(&event, stored.stream_seq) {
                let _ = writeln!(text, "\n({gap}.)");
                export.gaps.push(gap);
            }
            if let Err(err) = out.write_all(text.as_bytes()).await {
                export.gaps.push(format!(
                    "every event from #{}: the export stopped there ({err})",
                    stored.stream_seq
                ));
                break 'pages;
            }
        }
        if !full {
            break;
        }
    }
    let mut end = String::from("\n## Export status\n\n");
    if export.gaps.is_empty() {
        end.push_str(COMPLETE_LINE);
        end.push('\n');
    } else {
        end.push_str("Missing from this transcript:\n");
        for gap in &export.gaps {
            let _ = writeln!(end, "- {gap}");
        }
    }
    // A failure here leaves the file without its last line, which says it is incomplete.
    let ended = out.write_all(end.as_bytes()).await.is_ok() && out.flush().await.is_ok();
    if !ended && export.gaps.is_empty() {
        export
            .gaps
            .push("the transcript's end: it couldn't be written".into());
    }
    export.tail = tail.split_off(tail.len().saturating_sub(TAIL_MESSAGES));
    export
}

/// `event` with the copy of its saved file made later ([`DomainEvent::OutputKept`]), if one
/// was.
fn with_late_copy(mut event: ProviderEvent, late: &HashMap<String, KeptFile>) -> ProviderEvent {
    if let ProviderEvent::Command { item_id, kept, .. }
    | ProviderEvent::ToolCall { item_id, kept, .. } = &mut event
        && let Some(file) = late.get(item_id.as_str())
    {
        let kept = kept.get_or_insert_with(Box::default);
        if kept.file.as_ref().is_none_or(|own| own.blob.is_none()) {
            kept.file = Some(file.clone());
        }
    }
    event
}

/// The copies of saved files made after their results were seen, by result.
async fn late_copies(store: &brigadier_store::Store, stream: &str) -> HashMap<String, KeptFile> {
    let mut late = HashMap::new();
    let mut before = None;
    loop {
        let Ok(page) = store
            .read_stream(
                stream.to_owned(),
                brigadier_store::StreamPage {
                    before,
                    kinds: vec!["output.kept".into()],
                    limit: EVENTS_PAGE,
                },
            )
            .await
        else {
            break;
        };
        let Some(oldest) = page.last() else {
            break;
        };
        before = Some(oldest.stream_seq);
        let full = page.len() == EVENTS_PAGE as usize;
        for stored in &page {
            if let Ok(DomainEvent::OutputKept { item_id, file, .. }) =
                serde_json::from_str::<DomainEvent>(stored.payload.get())
            {
                late.entry(item_id).or_insert(file);
            }
        }
        if !full {
            break;
        }
    }
    late
}

/// A finished command's or tool call's kept output.
fn kept_of(event: &ProviderEvent) -> Option<&KeptOutput> {
    match event {
        ProviderEvent::Command {
            status: ItemStatus::Completed | ItemStatus::Failed,
            kept,
            ..
        }
        | ProviderEvent::ToolCall {
            status: ItemStatus::Completed | ItemStatus::Failed,
            kept,
            ..
        } => kept.as_deref(),
        _ => None,
    }
}

/// Whether `text` was cut by Brigadier's 8 KiB clip ("… [N more bytes]").
pub(crate) fn was_cut(text: &str) -> bool {
    text.ends_with(" more bytes]") && text.contains("… [")
}

/// An output cut with nothing kept (recorded before outputs were kept whole), or a preview
/// whose saved file wasn't copied: what is lost.
fn cut_without_copy(event: &ProviderEvent, seq: i64) -> Option<String> {
    let (what, output) = match event {
        ProviderEvent::Command {
            command,
            output: Some(output),
            status: ItemStatus::Completed | ItemStatus::Failed,
            kept: None,
            ..
        } => (format!("`{command}`"), output),
        ProviderEvent::ToolCall {
            name,
            output: Some(output),
            status: ItemStatus::Completed | ItemStatus::Failed,
            kept: None,
            ..
        } => (name.clone(), output),
        _ => return None,
    };
    if was_cut(output) {
        return Some(format!(
            "event #{seq}, the output of {what} past its first 8 KiB: it was recorded cut, before \
             Brigadier kept outputs whole"
        ));
    }
    (output.contains("/tool-results/") && output.contains("saved to")).then(|| {
        format!(
            "event #{seq}, the whole output of {what}: the CLI saved it to a file of its own, \
             which Brigadier didn't copy"
        )
    })
}

/// Copies `kept`'s whole output and saved file to `outputs`; the lines naming them, or saying
/// what is missing (also added to the gaps).
async fn copy_kept<B, BF>(
    kept: &KeptOutput,
    seq: i64,
    blob: &B,
    outputs: &Path,
    export: &mut Export,
) -> String
where
    B: Fn(String) -> BF,
    BF: Future<Output = brigadier_store::Result<PathBuf>>,
{
    let mut lines = String::new();
    if kept.excerpt {
        lines.push_str(
            "\n(The CLI gave only this excerpt of the output: no more of it exists anywhere.)\n",
        );
    }
    let mut copied = |result: std::result::Result<u64, String>, name: &str, what: &str| match result
    {
        Ok(bytes) => {
            export.outputs += 1;
            format!("\n({what}, {bytes} bytes: {OUTPUTS_DIR}/{name})\n")
        }
        Err(err) => {
            let gap = format!("event #{seq}, {what}: {err}");
            let line = format!("\n({gap}.)\n");
            export.gaps.push(gap);
            line
        }
    };
    match (&kept.blob, &kept.error) {
        (Some(hash), _) => {
            let name = format!("{seq}-output.txt");
            let result = copy_blob(blob, hash, outputs, &name).await;
            lines.push_str(&copied(result, &name, "The whole output"));
        }
        (None, Some(error)) => {
            lines.push_str(&copied(
                Err(error.clone()),
                "",
                "the output past its first 8 KiB",
            ));
        }
        (None, None) => {}
    }
    if let Some(file) = &kept.file {
        let what = format!("the whole output the CLI saved to {}", file.path);
        match (&file.blob, &file.error) {
            (Some(hash), _) => {
                let name = format!("{seq}-saved.txt");
                let result = copy_blob(blob, hash, outputs, &name).await;
                lines.push_str(&copied(result, &name, "The whole output the CLI saved"));
            }
            (None, error) => {
                let error = error.clone().unwrap_or_else(|| "it wasn't copied".into());
                lines.push_str(&copied(Err(error), "", &what));
            }
        }
    }
    lines
}

/// Copies the blob `hash` to `outputs`/`name`; its size.
async fn copy_blob<B, BF>(
    blob: &B,
    hash: &str,
    outputs: &Path,
    name: &str,
) -> std::result::Result<u64, String>
where
    B: Fn(String) -> BF,
    BF: Future<Output = brigadier_store::Result<PathBuf>>,
{
    let source = blob(hash.to_owned()).await.map_err(|err| err.to_string())?;
    tokio::fs::create_dir_all(outputs)
        .await
        .map_err(|err| err.to_string())?;
    tokio::fs::copy(&source, outputs.join(name))
        .await
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                "its kept copy is missing".to_owned()
            } else {
                err.to_string()
            }
        })
}

/// One transcript entry, in full.
fn entry(text: &mut String, event: &ProviderEvent, actor: &str) {
    match event {
        ProviderEvent::SessionStarted { model, .. } => {
            let _ = write!(
                text,
                "\n## A session started ({})\n",
                model.as_deref().unwrap_or("its default model")
            );
        }
        ProviderEvent::Message {
            role, text: said, ..
        } => {
            let who = match role {
                Role::Assistant => actor.to_owned(),
                _ => format!("{actor} was told"),
            };
            let _ = write!(text, "\n### {who}\n\n{said}\n");
        }
        ProviderEvent::Command {
            command,
            status: ItemStatus::Completed | ItemStatus::Failed,
            exit_code,
            output,
            ..
        } => {
            let code = exit_code.map_or_else(String::new, |code| format!(" (exit {code})"));
            let _ = write!(text, "\n### Ran `{command}`{code}\n");
            if let Some(output) = output.as_deref().filter(|output| !output.trim().is_empty()) {
                let _ = write!(text, "\n```\n{}\n```\n", output.trim_end());
            }
        }
        ProviderEvent::ToolCall {
            name,
            input,
            status: ItemStatus::Completed | ItemStatus::Failed,
            output,
            ..
        } => {
            let _ = write!(text, "\n### Used {name}\n");
            if let Some(input) = input {
                let _ = write!(text, "\nInput: {input}\n");
            }
            if let Some(output) = output.as_deref().filter(|output| !output.trim().is_empty()) {
                let _ = write!(text, "\n```\n{}\n```\n", output.trim_end());
            }
        }
        ProviderEvent::FileChanges {
            changes,
            status: ItemStatus::Completed,
            ..
        } => {
            let paths: Vec<&str> = changes.iter().map(|change| change.path.as_str()).collect();
            let _ = write!(text, "\n### Changed files: {}\n", paths.join(", "));
        }
        ProviderEvent::Error { error } if !error.will_retry => {
            let _ = write!(text, "\n### Stopped by an error\n\n{}\n", error.message);
        }
        ProviderEvent::Notice { message, .. } => {
            let _ = write!(text, "\n### Brigadier\n\n{message}\n");
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll};

    use serde_json::value::RawValue;

    use super::*;
    use crate::work::TaskId;

    fn stored(seq: i64, payload: String) -> StoredEvent {
        StoredEvent {
            seq,
            stream: "task:t".into(),
            stream_seq: seq,
            kind: "worker.event".into(),
            at_ms: 0,
            payload: RawValue::from_string(payload).unwrap(),
        }
    }

    fn worker(seq: i64, event: ProviderEvent) -> StoredEvent {
        let event = DomainEvent::WorkerEvent {
            task_id: TaskId("t".into()),
            event,
        };
        stored(seq, serde_json::to_string(&event).unwrap())
    }

    fn command(output: &str, kept: Option<KeptOutput>) -> ProviderEvent {
        let kept = kept.map(Box::new);
        ProviderEvent::Command {
            item_id: "c".into(),
            command: "make test".into(),
            cwd: None,
            status: ItemStatus::Completed,
            exit_code: Some(0),
            output: Some(output.into()),
            duration_ms: None,
            kept,
        }
    }

    fn said(text: &str) -> ProviderEvent {
        ProviderEvent::Message {
            item_id: "m".into(),
            role: Role::Assistant,
            text: text.into(),
        }
    }

    /// A scratch folder with blobs `a` (holding "WHOLE A") and `b` (holding "SAVED B").
    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("brigadier-export-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("blobs")).unwrap();
        std::fs::write(dir.join("blobs/a"), "WHOLE A").unwrap();
        std::fs::write(dir.join("blobs/b"), "SAVED B").unwrap();
        dir
    }

    /// Exports `pages` (each read once, in order; an `Err` page fails its read) to a string.
    async fn run(
        dir: &Path,
        pages: Vec<std::result::Result<Vec<StoredEvent>, String>>,
        out: impl AsyncWrite + Unpin,
    ) -> Export {
        let pages = Arc::new(std::sync::Mutex::new(pages.into_iter()));
        let blobs = dir.join("blobs");
        export(
            move |_after| {
                let next = pages.lock().unwrap().next();
                async move {
                    match next {
                        Some(Ok(page)) => Ok(page),
                        Some(Err(err)) => {
                            Err(brigadier_store::Error::Io(std::io::Error::other(err)))
                        }
                        None => Ok(Vec::new()),
                    }
                }
            },
            move |hash: String| {
                let path = blobs.join(hash);
                async move { Ok(path) }
            },
            &HashMap::new(),
            worker_event,
            "The worker",
            out,
            &dir.join(OUTPUTS_DIR),
        )
        .await
    }

    #[tokio::test]
    async fn kept_outputs_are_copied_beside_the_transcript_and_a_whole_export_says_so() {
        let dir = scratch();
        let kept = KeptOutput {
            blob: Some("a".into()),
            bytes: 7,
            file: Some(KeptFile {
                path: "/s/tool-results/x.txt".into(),
                blob: Some("b".into()),
                bytes: 7,
                ..KeptFile::default()
            }),
            ..KeptOutput::default()
        };
        let mut out = Vec::new();
        let export = run(
            &dir,
            vec![Ok(vec![
                worker(1, said("Looking.")),
                worker(2, command("cut… [9 more bytes]", Some(kept))),
            ])],
            &mut out,
        )
        .await;
        let text = String::from_utf8(out).unwrap();
        assert!(export.complete(), "{:?}", export.gaps);
        assert_eq!(export.outputs, 2);
        assert!(text.contains("outputs/2-output.txt") && text.contains("outputs/2-saved.txt"));
        assert!(text.trim_end().ends_with(COMPLETE_LINE), "{text}");
        assert_eq!(
            std::fs::read_to_string(dir.join("outputs/2-output.txt")).unwrap(),
            "WHOLE A"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("outputs/2-saved.txt")).unwrap(),
            "SAVED B"
        );
        assert_eq!(export.tail, ["Looking."]);
        assert!(
            export
                .summary("transcript.md")
                .starts_with("transcript.md is complete.")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn a_failed_store_a_missing_blob_and_an_old_cut_output_each_mark_the_export_incomplete() {
        let dir = scratch();
        let failed = KeptOutput {
            bytes: 300_000,
            error: Some("the whole output couldn't be stored: disk full".into()),
            ..KeptOutput::default()
        };
        let missing = KeptOutput {
            blob: Some("gone".into()),
            bytes: 300_000,
            ..KeptOutput::default()
        };
        let mut out = Vec::new();
        let export = run(
            &dir,
            vec![Ok(vec![
                worker(1, command("a… [5 more bytes]", Some(failed))),
                worker(2, command("b… [5 more bytes]", Some(missing))),
                // Recorded before outputs were kept whole.
                worker(3, command("c… [5 more bytes]", None)),
                worker(4, command("short and whole", None)),
            ])],
            &mut out,
        )
        .await;
        let text = String::from_utf8(out).unwrap();
        assert_eq!(export.gaps.len(), 3, "{:?}", export.gaps);
        assert!(export.gaps[0].starts_with("event #1") && export.gaps[0].contains("disk full"));
        assert!(export.gaps[1].starts_with("event #2") && export.gaps[1].contains("missing"));
        assert!(export.gaps[2].starts_with("event #3") && export.gaps[2].contains("before"));
        assert!(
            !text.contains(COMPLETE_LINE.trim_end_matches('.'))
                || !text.trim_end().ends_with(COMPLETE_LINE)
        );
        assert!(text.contains("Missing from this transcript:"));
        let summary = export.summary("transcript.md");
        assert!(
            summary.contains("NOT complete") && summary.contains("unknown"),
            "{summary}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn a_page_that_cant_be_read_and_an_event_that_cant_be_decoded_are_gaps() {
        let dir = scratch();
        let mut page: Vec<StoredEvent> = (1..=EVENTS_PAGE as i64)
            .map(|seq| worker(seq, said("step")))
            .collect();
        page[4] = stored(
            5,
            r#"{"type":"workerEvent","taskId":"t","event":{"type":"message","itemId":3}}"#.into(),
        );
        let mut out = Vec::new();
        let export = run(&dir, vec![Ok(page), Err("disk I/O error".into())], &mut out).await;
        assert_eq!(export.gaps.len(), 2, "{:?}", export.gaps);
        assert!(export.gaps[0].starts_with("event #5: it couldn't be decoded"));
        assert!(
            export.gaps[1].starts_with(&format!("every event after #{}", EVENTS_PAGE))
                && export.gaps[1].contains("disk I/O error"),
            "{:?}",
            export.gaps
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A writer that fails once it has taken `left` bytes.
    struct Failing {
        left: usize,
    }

    impl AsyncWrite for Failing {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            if buf.len() > self.left {
                return Poll::Ready(Err(std::io::Error::other("no space left on device")));
            }
            self.left -= buf.len();
            Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn an_interrupted_export_stops_where_it_was_cut_and_says_so() {
        let dir = scratch();
        let page: Vec<StoredEvent> = (1..=20)
            .map(|seq| worker(seq, said("a step of the work")))
            .collect();
        let export = run(&dir, vec![Ok(page)], Failing { left: 700 }).await;
        assert!(!export.complete());
        assert!(
            export
                .gaps
                .iter()
                .any(|gap| gap.starts_with("every event from #") && gap.contains("no space left")),
            "{:?}",
            export.gaps
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
