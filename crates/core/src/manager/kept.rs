//! Durable raw output (BUILD-PLAN D2.1, D2.2): every tool result a worker's or a thread's CLI
//! gives is stored whole before Brigadier cuts it to 8 KiB, and so is the file a Claude
//! preview names (its `tool-results/`, which goes when the CLI's session folder does).
//!
//! The adapter's event sender ([`brigadier_providers::KeptOutput`]) redacts the result whole,
//! cuts the event's `output`, and leaves the whole text waiting on the event; this stores it in
//! the blob store before the event is recorded, so the recorded event names the blob (and the
//! blob store keeps a blob while an event names it). What couldn't be stored is said on the
//! event (`error`), never dropped quietly.

use std::collections::HashSet;
use std::future::Future;

use brigadier_providers::{ItemStatus, KeptFile, KeptOutput, ProviderEvent};

use super::SessionManager;
use crate::Result;
use crate::model::{DomainEvent, TaskId, streams};

/// Events read per page when looking for files not copied yet.
const EVENTS_PAGE: u32 = 2_000;

impl SessionManager {
    /// Stores what `event` holds waiting to be kept, and records on it where, or why not.
    pub(crate) async fn keep_output(&self, event: &mut ProviderEvent) {
        let Some(kept) = kept_mut(event) else {
            return;
        };
        if kept.whole.is_none() && kept.file.as_ref().is_none_or(|file| file.text.is_none()) {
            return;
        }
        keep(kept, |bytes| self.put_kept(bytes)).await;
    }

    /// One kept output into the blob store; its hash.
    async fn put_kept(&self, bytes: Vec<u8>) -> Result<String> {
        #[cfg(debug_assertions)]
        if self.faults.kept_writes_fail() {
            return Err(crate::Error::Invalid(
                "the disk refused the write (a development fault)".into(),
            ));
        }
        Ok(self.core.store().blobs().put(bytes).await?.to_string())
    }
}

impl SessionManager {
    /// Copies each file a result of the task's workers named that isn't kept yet (its copy
    /// failed when the result was seen, or the result was recorded before results were kept),
    /// while the CLI's session folder still holds it: before a hand-off, a reroute, a review
    /// hold's release or the task's end, any of which may take that folder (BUILD-PLAN D2.2).
    /// Each copy is recorded as an `output.kept` event naming the result.
    pub(crate) async fn keep_saved_files(&self, task: &TaskId) {
        let store = self.core.store();
        let stream = streams::task(task);
        let mut missing: Vec<(String, String)> = Vec::new();
        let mut done: HashSet<String> = HashSet::new();
        let mut after = 0;
        loop {
            let Ok(page) = store
                .read_stream_since(stream.clone(), after, EVENTS_PAGE)
                .await
            else {
                break;
            };
            let Some(last) = page.last() else {
                break;
            };
            after = last.stream_seq;
            let full = page.len() == EVENTS_PAGE as usize;
            for stored in &page {
                if stored.kind == "output.kept" {
                    if let Ok(DomainEvent::OutputKept { item_id, .. }) =
                        serde_json::from_str::<DomainEvent>(stored.payload.get())
                    {
                        done.insert(item_id);
                    }
                    continue;
                }
                if let Some(event) = worker_event(stored)
                    && let Some(found) = uncopied_file(&event)
                {
                    missing.push(found);
                }
            }
            if !full {
                break;
            }
        }
        missing.retain(|(item, _)| !done.contains(item));
        if missing.is_empty() {
            return;
        }
        let redactor = match self.existing_task_live(task) {
            Some(live) => live.redactor().await,
            None => None,
        };
        let mut seen: HashSet<String> = HashSet::new();
        for (item_id, path) in missing {
            if !seen.insert(item_id.clone()) {
                continue;
            }
            let mut file = KeptFile {
                path: path.clone(),
                ..KeptFile::default()
            };
            match read_saved_file(&path).await {
                Ok((mut text, size)) => {
                    if let Some(redactor) = &redactor {
                        redactor.redact_in_place(&mut text);
                    }
                    file.bytes = size;
                    match self.put_kept(text.into_bytes()).await {
                        Ok(blob) => file.blob = Some(blob),
                        Err(err) => {
                            file.error = Some(format!("its copy couldn't be stored: {err}"))
                        }
                    }
                }
                // Gone already: nothing to record.
                Err(_) => continue,
            }
            let event = DomainEvent::OutputKept {
                task_id: task.clone(),
                item_id,
                file,
            };
            if let Err(err) = self.core.record(vec![(stream.clone(), event)]).await {
                tracing::warn!(task = %task, error = %err, "could not record a late output copy");
            }
        }
    }
}

/// A file the CLI saved a whole result to, up to the most of one result kept: only a file in a
/// session's `tool-results` folder is read (see [`brigadier_providers::open_tool_results_file`]).
/// Its size is the file's whole size; what lies past the most kept is said at the end.
async fn read_saved_file(path: &str) -> std::io::Result<(String, u64)> {
    use tokio::io::AsyncReadExt as _;
    let file = brigadier_providers::open_tool_results_file(path).await?;
    let size = file.metadata().await?.len();
    let mut bytes = Vec::new();
    file.take(brigadier_providers::OUTPUT_KEEP_MAX as u64)
        .read_to_end(&mut bytes)
        .await?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if size > bytes.len() as u64 {
        text.push_str(&format!("… [{} more bytes]", size - bytes.len() as u64));
    }
    Ok((text, size))
}

/// A finished result's item and the saved file it names that wasn't copied when it was seen.
fn uncopied_file(event: &ProviderEvent) -> Option<(String, String)> {
    let (item_id, output, kept) = match event {
        ProviderEvent::Command {
            item_id,
            output,
            kept,
            status: ItemStatus::Completed | ItemStatus::Failed,
            ..
        }
        | ProviderEvent::ToolCall {
            item_id,
            output,
            kept,
            status: ItemStatus::Completed | ItemStatus::Failed,
            ..
        } => (item_id, output, kept),
        _ => return None,
    };
    let path = match kept.as_ref().and_then(|kept| kept.file.as_ref()) {
        Some(file) if file.blob.is_some() => return None,
        Some(file) => file.path.clone(),
        None => brigadier_providers::saved_output_file(output.as_deref()?)?,
    };
    Some((item_id.clone(), path))
}

/// The kept part of a command or tool call event.
/// A worker's CLI event (`task:` stream), if `stored` is one.
pub(crate) fn worker_event(stored: &brigadier_store::StoredEvent) -> Option<ProviderEvent> {
    if stored.kind != "worker.event" {
        return None;
    }
    match serde_json::from_str::<DomainEvent>(stored.payload.get()) {
        Ok(DomainEvent::WorkerEvent { event, .. }) => Some(event),
        _ => None,
    }
}

pub(crate) fn kept_mut(event: &mut ProviderEvent) -> Option<&mut KeptOutput> {
    match event {
        ProviderEvent::Command { kept, .. } | ProviderEvent::ToolCall { kept, .. } => {
            kept.as_deref_mut()
        }
        _ => None,
    }
}

/// Stores `kept`'s waiting text and file with `put`, recording each hash, or the error.
pub(crate) async fn keep<F, Fut>(kept: &mut KeptOutput, put: F)
where
    F: Fn(Vec<u8>) -> Fut,
    Fut: Future<Output = Result<String>>,
{
    if let Some(whole) = kept.whole.take() {
        match put(whole.as_bytes().to_vec()).await {
            Ok(blob) => kept.blob = Some(blob),
            Err(err) => kept.error = Some(format!("the whole output couldn't be stored: {err}")),
        }
    }
    if let Some(file) = kept.file.as_mut()
        && let Some(text) = file.text.take()
    {
        match put(text.as_bytes().to_vec()).await {
            Ok(blob) => file.blob = Some(blob),
            Err(err) => file.error = Some(format!("its copy couldn't be stored: {err}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use brigadier_providers::KeptFile;

    use super::*;

    #[tokio::test]
    async fn the_whole_output_and_the_saved_file_are_stored_or_the_failure_is_said() {
        let waiting = || KeptOutput {
            bytes: 9,
            whole: Some(Arc::from("whole out")),
            file: Some(KeptFile {
                path: "/s/tool-results/a.txt".into(),
                bytes: 4,
                text: Some(Arc::from("file")),
                ..KeptFile::default()
            }),
            ..KeptOutput::default()
        };
        let mut kept = waiting();
        keep(&mut kept, |bytes: Vec<u8>| async move {
            Ok(format!("hash-of-{}", String::from_utf8(bytes).unwrap()))
        })
        .await;
        assert_eq!(kept.blob.as_deref(), Some("hash-of-whole out"));
        let file = kept.file.as_ref().unwrap();
        assert_eq!(file.blob.as_deref(), Some("hash-of-file"));
        assert!(kept.is_complete());
        // Nothing waits in what is recorded.
        assert!(kept.whole.is_none() && file.text.is_none());

        let mut kept = waiting();
        keep(&mut kept, |_| async {
            Err(crate::Error::Invalid("disk full".into()))
        })
        .await;
        assert_eq!(kept.blob, None);
        assert!(kept.error.as_deref().unwrap().contains("disk full"));
        assert!(kept.file.as_ref().unwrap().error.is_some());
        assert!(!kept.is_complete());
    }
}
