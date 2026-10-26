//! Whole-session background history, independent of disposable live CLI/task rows.
//! Metadata is atomic and output is copied/redacted before provider files disappear.
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use brigadier_providers::{BackgroundTaskKind, ProviderEvent};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::SessionManager;
use super::session_dir::{read_lineage, write_atomic};
use crate::{ConversationId, DomainEvent, Error, Result, TaskId, now_ms};

const OUTPUT_BYTES: u64 = 1024 * 1024;
// Serializes short disk transactions, never held over an await. No task cache is authoritative.
static DISK: Mutex<()> = Mutex::new(());
fn epoch() -> &'static str {
    static EPOCH: OnceLock<String> = OnceLock::new();
    EPOCH.get_or_init(|| uuid::Uuid::now_v7().to_string())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundHistoryRecord {
    pub id: String,
    pub conversation_id: String,
    pub worker_id: Option<String>,
    pub provider: Option<String>,
    pub native_session_id: Option<String>,
    pub generation: String,
    pub task_id: String,
    pub call_id: Option<String>,
    pub kind: String,
    pub purpose: String,
    pub command: Option<String>,
    pub cwd: Option<String>,
    /// running, paused, completed, failed, stopped, or unknown (no end was observed).
    pub status: String,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    pub exit_code: Option<i32>,
    pub stop_actor: Option<String>,
    pub summary: Option<String>,
    pub output_available: bool,
    pub output_truncated: bool,
    #[ts(type = "number")]
    pub output_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundHistoryPage {
    pub records: Vec<BackgroundHistoryRecord>,
    pub next: Option<String>,
    pub total: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundHistoryOutput {
    pub record: BackgroundHistoryRecord,
    pub text: Option<String>,
    pub unavailable_reason: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Generation {
    id: String,
    native: Option<String>,
    cwd: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    record: BackgroundHistoryRecord,
    epoch: String,
    source: Option<String>,
    preview_log: Option<String>,
    #[serde(default)]
    detached: bool,
}
#[derive(Debug, Default, Serialize, Deserialize)]
struct Journal {
    records: Vec<Entry>,
    generations: HashMap<String, Generation>,
    commands: Vec<(String, String, String, Option<String>)>,
    preview_seq: i64,
}
impl Journal {
    fn load(dir: &Path) -> Result<Self> {
        let file = dir.join("history.json");
        let mut journal: Self = match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| Error::Invalid(format!("background history: {e}")))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(Error::Invalid(e.to_string())),
        };
        for entry in &mut journal.records {
            if entry.epoch != epoch()
                && entry.record.kind != "preview"
                && entry.record.status == "running"
            {
                entry.record.status = "unknown".into();
                entry.record.summary = Some("Daemon restarted; end was not observed".into());
                // Reconciliation time is not the command's end time.
                entry.record.ended_at_ms = None;
            }
        }
        Ok(journal)
    }
    fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir).map_err(|e| Error::Invalid(e.to_string()))?;
        write_atomic(
            &dir.join("history.json"),
            &serde_json::to_vec(self).map_err(|e| Error::Invalid(e.to_string()))?,
        )
    }
    fn apply(
        &mut self,
        conversation: &str,
        owner: &str,
        event: Option<&ProviderEvent>,
        at: i64,
        fallback: Generation,
        provider: Option<String>,
    ) -> Vec<(String, String)> {
        let active_before: Vec<String> = self
            .records
            .iter()
            .filter(|entry| {
                entry.epoch == epoch()
                    && entry.record.worker_id.as_deref().unwrap_or("") == owner
                    && entry.record.status == "running"
            })
            .map(|entry| entry.record.id.clone())
            .collect();
        let generation = self.generations.entry(owner.to_owned()).or_insert(fallback);
        if let Some(ProviderEvent::SessionStarted { native_id, cwd, .. }) = event {
            self.commands.retain(|(who, ..)| who != owner);
            for entry in &mut self.records {
                if entry.epoch == epoch()
                    && entry.record.worker_id.as_deref().unwrap_or("") == owner
                    && entry.record.status == "running"
                {
                    entry.record.status = "unknown".into();
                    entry.record.summary =
                        Some("CLI session replaced; end was not observed".into());
                }
            }
            *generation = Generation {
                id: uuid::Uuid::now_v7().to_string(),
                native: Some(native_id.clone()),
                cwd: cwd.clone(),
            };
        }
        let generation = generation.clone();
        match event {
            Some(ProviderEvent::Command {
                item_id,
                command,
                cwd,
                exit_code,
                output,
                ..
            }) if !command.is_empty() => {
                self.command(owner, item_id, command.clone(), cwd.clone());
                if !output.as_deref().is_some_and(|text| {
                    text.trim_start()
                        .starts_with("Command running in background with ID:")
                }) {
                    for entry in &mut self.records {
                        if entry.epoch == epoch()
                            && entry.record.worker_id.as_deref().unwrap_or("") == owner
                            && entry.record.generation == generation.id
                            && entry.record.call_id.as_deref() == Some(item_id)
                        {
                            entry.record.exit_code = *exit_code;
                        }
                    }
                }
            }
            Some(ProviderEvent::ToolCall {
                item_id,
                name,
                input,
                status,
                ..
            }) => {
                let args = input
                    .as_deref()
                    .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok());
                if let Some(args) = &args
                    && let Some(command) = args.get("command").and_then(|v| v.as_str())
                {
                    self.command(
                        owner,
                        item_id,
                        command.to_owned(),
                        args.get("cwd").and_then(|v| v.as_str()).map(str::to_owned),
                    );
                }
                if matches!(
                    name.as_str(),
                    "Agent" | "Task" | "spawnAgent" | "spawn_agent"
                ) {
                    let existing = self.records.iter().position(|entry| {
                        entry.record.worker_id.as_deref().unwrap_or("") == owner
                            && entry.record.generation == generation.id
                            && entry.record.call_id.as_deref() == Some(item_id)
                    });
                    let old = existing.map(|index| self.records[index].clone());
                    let purpose = args
                        .as_ref()
                        .map(|args| {
                            [
                                args.get("description").and_then(|v| v.as_str()),
                                args.get("prompt").and_then(|v| v.as_str()),
                            ]
                            .into_iter()
                            .flatten()
                            .collect::<Vec<_>>()
                            .join("\n\n")
                        })
                        .filter(|text| !text.is_empty())
                        .or_else(|| old.as_ref().map(|entry| entry.record.purpose.clone()))
                        .unwrap_or_else(|| name.clone());
                    let detached = args
                        .as_ref()
                        .and_then(|args| args.get("run_in_background"))
                        .and_then(|v| v.as_bool())
                        .unwrap_or_else(|| old.as_ref().is_some_and(|entry| entry.detached));
                    let state = match status {
                        brigadier_providers::ItemStatus::InProgress => "running",
                        brigadier_providers::ItemStatus::Completed if detached => old
                            .as_ref()
                            .map_or("running", |entry| entry.record.status.as_str()),
                        brigadier_providers::ItemStatus::Completed => "completed",
                        brigadier_providers::ItemStatus::Failed => "failed",
                        brigadier_providers::ItemStatus::Declined => "stopped",
                    };
                    let mut record = old
                        .as_ref()
                        .map(|entry| entry.record.clone())
                        .unwrap_or_else(|| BackgroundHistoryRecord {
                            id: uuid::Uuid::now_v7().to_string(),
                            conversation_id: conversation.into(),
                            worker_id: (!owner.is_empty()).then(|| owner.into()),
                            provider: provider.clone(),
                            native_session_id: generation.native.clone(),
                            generation: generation.id.clone(),
                            task_id: item_id.clone(),
                            call_id: Some(item_id.clone()),
                            kind: "agent".into(),
                            purpose: purpose.clone(),
                            command: None,
                            cwd: generation.cwd.clone(),
                            status: "running".into(),
                            started_at_ms: at,
                            ended_at_ms: None,
                            exit_code: None,
                            stop_actor: None,
                            summary: None,
                            output_available: false,
                            output_truncated: false,
                            output_bytes: 0,
                        });
                    record.purpose = purpose;
                    record.status = state.into();
                    if state != "running" {
                        record.ended_at_ms = old
                            .as_ref()
                            .and_then(|entry| entry.record.ended_at_ms)
                            .or(Some(at));
                    }
                    let entry = Entry {
                        record,
                        epoch: epoch().into(),
                        source: old.as_ref().and_then(|entry| entry.source.clone()),
                        preview_log: None,
                        detached,
                    };
                    match existing {
                        Some(index) => self.records[index] = entry,
                        None => self.records.push(entry),
                    }
                }
            }
            Some(ProviderEvent::BackgroundTask { task }) => {
                // Native ids (and output paths) can be reused within one CLI session.
                // The new start proves the previous invocation is no longer addressable;
                // retain its last snapshot, without inventing how or when it ended.
                for entry in &mut self.records {
                    if entry.record.worker_id.as_deref().unwrap_or("") == owner
                        && entry.record.generation == generation.id
                        && entry.record.task_id == task.id
                        && entry.record.started_at_ms != task.started_at_ms
                        && entry.record.status == "running"
                        && !(entry.record.kind == "agent"
                            && task.item_id.is_some()
                            && entry.record.call_id == task.item_id)
                    {
                        entry.record.status = "unknown".into();
                        entry.record.summary =
                            Some("Native task id reused; end was not observed".into());
                    }
                }
                let existing = self.records.iter().position(|entry| {
                    entry.record.worker_id.as_deref().unwrap_or("") == owner
                        && ((entry.record.generation == generation.id
                            && entry.record.task_id == task.id
                            && entry.record.started_at_ms == task.started_at_ms)
                            || (entry.record.generation == generation.id
                                && entry.record.kind == "agent"
                                && task.item_id.is_some()
                                && entry.record.call_id == task.item_id)
                            || (entry.epoch == "legacy"
                                && entry.record.native_session_id == generation.native
                                && task.item_id.is_some()
                                && entry.record.call_id == task.item_id))
                });
                let command = task.item_id.as_ref().and_then(|item| {
                    self.commands
                        .iter()
                        .rev()
                        .find(|(who, id, ..)| who == owner && id == item)
                });
                let actual_command = if task.kind == BackgroundTaskKind::Terminal {
                    Some(task.label.clone())
                } else {
                    command.map(|(_, _, text, _)| text.clone())
                };
                let old = existing.map(|i| &self.records[i]);
                let id = old
                    .map(|entry| entry.record.id.clone())
                    .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
                let record = BackgroundHistoryRecord {
                    id,
                    conversation_id: conversation.to_owned(),
                    worker_id: (!owner.is_empty()).then(|| owner.to_owned()),
                    provider,
                    native_session_id: generation.native.clone(),
                    generation: generation.id.clone(),
                    task_id: task.id.clone(),
                    call_id: task.item_id.clone(),
                    kind: format!("{:?}", task.kind).to_lowercase(),
                    purpose: old
                        .filter(|entry| entry.record.kind == "agent")
                        .map_or_else(|| task.label.clone(), |entry| entry.record.purpose.clone()),
                    command: actual_command.or_else(|| old.and_then(|e| e.record.command.clone())),
                    cwd: command
                        .and_then(|(_, _, _, cwd)| cwd.clone())
                        .or(generation.cwd),
                    status: format!("{:?}", task.status).to_lowercase(),
                    started_at_ms: task.started_at_ms,
                    ended_at_ms: task.ended_at_ms,
                    exit_code: old.and_then(|e| e.record.exit_code),
                    stop_actor: old.and_then(|e| e.record.stop_actor.clone()),
                    summary: task.summary.clone(),
                    output_available: old.is_some_and(|e| e.record.output_available),
                    output_truncated: old.is_some_and(|e| e.record.output_truncated),
                    output_bytes: old.map_or(0, |e| e.record.output_bytes),
                };
                let entry = Entry {
                    record,
                    epoch: epoch().to_owned(),
                    source: task
                        .output_file
                        .clone()
                        .or_else(|| old.and_then(|e| e.source.clone())),
                    preview_log: None,
                    detached: task.kind == BackgroundTaskKind::Agent,
                };
                match existing {
                    Some(i) => self.records[i] = entry,
                    None => self.records.push(entry),
                }
            }
            Some(ProviderEvent::Exited { .. }) | None => {
                for entry in &mut self.records {
                    if entry.record.worker_id.as_deref().unwrap_or("") == owner
                        && entry.epoch == epoch()
                        && entry.record.status == "running"
                    {
                        entry.record.status = "unknown".into();
                        entry.record.summary = Some("CLI ended; end was not observed".into());
                    }
                }
                self.generations.remove(owner);
                self.commands.retain(|(who, ..)| who != owner);
            }
            _ => {}
        }
        if !matches!(
            event,
            None | Some(ProviderEvent::BackgroundTask { .. } | ProviderEvent::Exited { .. })
        ) {
            return Vec::new();
        }
        self.records
            .iter()
            .filter(|entry| {
                entry.record.worker_id.as_deref().unwrap_or("") == owner
                    && entry.epoch == epoch()
                    && match event {
                        Some(ProviderEvent::BackgroundTask { task }) => {
                            entry.record.generation == generation.id
                                && entry.record.task_id == task.id
                                && entry.record.started_at_ms == task.started_at_ms
                        }
                        _ => active_before.contains(&entry.record.id),
                    }
            })
            .filter_map(|entry| {
                entry
                    .source
                    .clone()
                    .map(|source| (entry.record.id.clone(), source))
            })
            .collect()
    }
    fn command(&mut self, owner: &str, item: &str, text: String, cwd: Option<String>) {
        self.commands
            .retain(|(who, id, ..)| who != owner || id != item);
        self.commands
            .push((owner.to_owned(), item.to_owned(), text.clone(), cwd.clone()));
        let generation = self.generations.get(owner).map(|g| g.id.as_str());
        for entry in &mut self.records {
            if Some(entry.record.generation.as_str()) == generation
                && entry.record.worker_id.as_deref().unwrap_or("") == owner
                && entry.epoch == epoch()
                && entry.record.call_id.as_deref() == Some(item)
            {
                entry.record.command = Some(text.clone());
                entry.record.cwd = cwd.clone().or(entry.record.cwd.clone());
            }
        }
        if self.commands.len() > 256 {
            self.commands.remove(0);
        }
    }
}

/// Bounded snapshots report original byte length and never claim that a tail is complete.
fn tail(path: &Path) -> std::io::Result<(Vec<u8>, u64)> {
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    file.seek(SeekFrom::Start(size.saturating_sub(OUTPUT_BYTES)))?;
    let mut bytes = Vec::new();
    file.take(OUTPUT_BYTES).read_to_end(&mut bytes)?;
    Ok((bytes, size))
}

impl SessionManager {
    fn history_dir(&self, id: &ConversationId) -> PathBuf {
        self.session_dir(id).record().join("background-history")
    }

    pub(super) async fn follow_background_history(
        &self,
        id: &ConversationId,
        worker: Option<&TaskId>,
        event: Option<&ProviderEvent>,
    ) {
        if !matches!(
            event,
            None | Some(
                ProviderEvent::SessionStarted { .. }
                    | ProviderEvent::BackgroundTask { .. }
                    | ProviderEvent::Command { .. }
                    | ProviderEvent::ToolCall { .. }
                    | ProviderEvent::Exited { .. }
            )
        ) {
            return;
        }
        if let Err(error) = self.record_background_history(id, worker, event).await {
            tracing::warn!(conversation = %id, %error, "could not preserve background history");
        }
    }
    async fn record_background_history(
        &self,
        id: &ConversationId,
        worker: Option<&TaskId>,
        event: Option<&ProviderEvent>,
    ) -> Result<()> {
        let hidden_event = match event {
            Some(event) => {
                let bytes = serde_json::to_vec(event).map_err(|e| Error::Invalid(e.to_string()))?;
                let bytes = self.hide_secrets(id, bytes).await;
                Some(
                    serde_json::from_slice::<ProviderEvent>(&bytes)
                        .map_err(|e| Error::Invalid(e.to_string()))?,
                )
            }
            None => None,
        };
        let event = hidden_event.as_ref();
        let owner = worker.map_or("", |id| id.0.as_str());
        let owner_record = match worker {
            Some(task) => self.worker_record(&self.task_by_id(id, task).await?),
            None => self.session_dir(id).record(),
        };
        let lineage = read_lineage(&owner_record).into_iter().last();
        let provider = lineage
            .as_ref()
            .map(|e| format!("{:?}", e.provider).to_lowercase());
        let fallback = Generation {
            id: format!("{}:{owner}", epoch()),
            native: lineage.map(|e| e.session_id),
            cwd: None,
        };
        let dir = self.history_dir(id);
        let sources = {
            let _guard = DISK.lock().unwrap();
            let mut journal = Journal::load(&dir)?;
            let sources = journal.apply(&id.0, owner, event, now_ms(), fallback, provider);
            journal.save(&dir)?;
            sources
        };
        let inline = match event {
            Some(
                ProviderEvent::Command {
                    item_id,
                    output: Some(output),
                    kept,
                    ..
                }
                | ProviderEvent::ToolCall {
                    item_id,
                    output: Some(output),
                    kept,
                    ..
                },
            ) => Some((item_id, output, kept)),
            _ => None,
        };
        if let Some((item_id, output, kept)) = inline
            && !output
                .trim_start()
                .starts_with("Command running in background with ID:")
        {
            let snapshot = kept
                .as_ref()
                .and_then(|kept| kept.blob.as_ref())
                .and_then(|blob| blob.parse().ok())
                .map(|hash| self.core.store().blobs().file_of(&hash))
                .and_then(|path| tail(&path).ok());
            let (bytes, size, truncated) = match snapshot {
                Some((bytes, size)) => (bytes, size, size > OUTPUT_BYTES),
                None => {
                    let bytes = output.as_bytes();
                    let kept = kept.as_deref();
                    (
                        bytes[bytes.len().saturating_sub(OUTPUT_BYTES as usize)..].to_vec(),
                        kept.map_or(bytes.len() as u64, |kept| {
                            kept.bytes.max(bytes.len() as u64)
                        }),
                        bytes.len() as u64 > OUTPUT_BYTES
                            || kept.is_some_and(|kept| {
                                kept.excerpt || kept.bytes > bytes.len() as u64
                            }),
                    )
                }
            };
            let bytes = self.hide_secrets(id, bytes).await;
            let _guard = DISK.lock().unwrap();
            let mut journal = Journal::load(&dir)?;
            let generation = journal.generations.get(owner).map(|g| g.id.clone());
            for entry in &mut journal.records {
                if Some(&entry.record.generation) == generation.as_ref()
                    && entry.epoch == epoch()
                    && entry.record.worker_id.as_deref().unwrap_or("") == owner
                    && entry.record.call_id.as_deref() == Some(item_id)
                {
                    write_atomic(&dir.join(format!("{}.output", entry.record.id)), &bytes)?;
                    entry.record.output_available = true;
                    entry.record.output_truncated = truncated;
                    entry.record.output_bytes = size;
                }
            }
            journal.save(&dir)?;
        }
        for (key, source) in sources {
            if let Ok((bytes, size)) = tail(Path::new(&source)) {
                let bytes = self.hide_secrets(id, bytes).await;
                let _guard = DISK.lock().unwrap();
                let mut journal = Journal::load(&dir)?;
                if let Some(entry) = journal
                    .records
                    .iter_mut()
                    .find(|entry| entry.record.id == key)
                {
                    write_atomic(&dir.join(format!("{key}.output")), &bytes)?;
                    entry.record.output_available = true;
                    entry.record.output_truncated = size > OUTPUT_BYTES;
                    entry.record.output_bytes = size;
                }
                journal.save(&dir)?;
            }
        }
        Ok(())
    }

    /// Best-effort upgrade of old native transcripts. An observed background invocation
    /// proves its start, not its end: absent lifecycle events remain explicitly unknown.
    async fn backfill_background_history(&self, id: &ConversationId) -> Result<()> {
        let mut owners = vec![(None, self.session_dir(id).record())];
        for task in self.core.tasks(id).await? {
            owners.push((Some(task.id.0.clone()), self.worker_record(&task)));
        }
        let dir = self.history_dir(id);
        for (owner, record) in owners {
            for lineage in read_lineage(&record) {
                let Some(file) = &lineage.file else {
                    continue;
                };
                let entries = self.session_files.read(lineage.provider, Path::new(file));
                let _guard = DISK.lock().unwrap();
                let mut journal = Journal::load(&dir)?;
                for entry in entries.iter().filter(|entry| entry.parent.is_none()) {
                    let (item, purpose, command, cwd, kind) = match &entry.event {
                        ProviderEvent::ToolCall {
                            item_id,
                            name,
                            input: Some(input),
                            ..
                        } => {
                            let Ok(args) = serde_json::from_str::<serde_json::Value>(input) else {
                                continue;
                            };
                            let agent = matches!(
                                name.as_str(),
                                "Agent" | "Task" | "spawnAgent" | "spawn_agent"
                            );
                            if !agent
                                && args.get("run_in_background").and_then(|v| v.as_bool())
                                    != Some(true)
                            {
                                continue;
                            }
                            let command = args
                                .get("command")
                                .and_then(|v| v.as_str())
                                .map(str::to_owned);
                            let purpose = args
                                .get("description")
                                .or_else(|| args.get("prompt"))
                                .and_then(|v| v.as_str())
                                .map(str::to_owned)
                                .or(command.clone())
                                .unwrap_or_else(|| name.clone());
                            (
                                item_id,
                                purpose,
                                command,
                                None,
                                if agent { "agent" } else { "shell" },
                            )
                        }
                        ProviderEvent::Command {
                            item_id,
                            command,
                            cwd,
                            output: Some(output),
                            ..
                        } if output
                            .trim_start()
                            .starts_with("Command running in background with ID:") =>
                        {
                            (
                                item_id,
                                command.clone(),
                                Some(command.clone()),
                                cwd.clone(),
                                "shell",
                            )
                        }
                        _ => continue,
                    };
                    // A durable event takes precedence over transcript-only evidence.
                    if journal.records.iter().any(|e| {
                        e.record.worker_id == owner
                            && e.record.native_session_id.as_deref() == Some(&lineage.session_id)
                            && e.record.call_id.as_deref() == Some(item)
                    }) {
                        continue;
                    }
                    let hash = blake3::hash(
                        format!(
                            "{}:{}:{}",
                            owner.as_deref().unwrap_or(""),
                            lineage.session_id,
                            item
                        )
                        .as_bytes(),
                    )
                    .to_hex()
                    .to_string();
                    let key = format!("legacy-{hash}");
                    if journal.records.iter().any(|e| e.record.id == key) {
                        continue;
                    }
                    journal.records.push(Entry {
                        record: BackgroundHistoryRecord {
                            id: key,
                            conversation_id: id.0.clone(),
                            worker_id: owner.clone(),
                            provider: Some(format!("{:?}", lineage.provider).to_lowercase()),
                            native_session_id: Some(lineage.session_id.clone()),
                            generation: format!("legacy:{}", lineage.session_id),
                            task_id: item.clone(),
                            call_id: Some(item.clone()),
                            kind: kind.into(),
                            purpose,
                            command,
                            cwd,
                            status: "unknown".into(),
                            started_at_ms: entry.at_ms.unwrap_or(lineage.started_at),
                            ended_at_ms: None,
                            exit_code: None,
                            stop_actor: None,
                            summary: Some("Recorded invocation; end was not observed".into()),
                            output_available: false,
                            output_truncated: false,
                            output_bytes: 0,
                        },
                        epoch: "legacy".into(),
                        source: None,
                        preview_log: None,
                        detached: false,
                    });
                }
                journal.save(&dir)?;
            }
        }
        Ok(())
    }

    pub(super) fn note_history_stop(
        &self,
        id: &ConversationId,
        worker: Option<&TaskId>,
        task: &str,
    ) {
        let result = (|| -> Result<()> {
            let _guard = DISK.lock().unwrap();
            let dir = self.history_dir(id);
            let mut journal = Journal::load(&dir)?;
            if let Some(entry) = journal.records.iter_mut().rev().find(|entry| {
                entry.record.worker_id.as_deref() == worker.map(|id| id.0.as_str())
                    && entry.record.task_id == task
                    && entry.record.status == "running"
                    && entry.epoch == epoch()
            }) {
                entry.record.stop_actor = Some("user".into());
            }
            journal.save(&dir)
        })();
        if let Err(error) = result {
            tracing::warn!(%error, "could not preserve background stop actor");
        }
    }

    async fn refresh_preview_history(&self, id: &ConversationId) -> Result<()> {
        let dir = self.history_dir(id);
        let mut after = {
            let _guard = DISK.lock().unwrap();
            Journal::load(&dir)?.preview_seq
        };
        loop {
            let events = self
                .core
                .store()
                .read_stream_since(crate::model::streams::conversation(id), after, 500)
                .await?;
            let full = events.len() == 500;
            let _guard = DISK.lock().unwrap();
            let mut journal = Journal::load(&dir)?;
            for event in events {
                after = after.max(event.stream_seq);
                if event.stream_seq <= journal.preview_seq {
                    continue;
                }
                if event.kind != "preview.updated" {
                    continue;
                }
                let DomainEvent::PreviewUpdated { preview } =
                    serde_json::from_str(event.payload.get())
                        .map_err(|e| Error::Invalid(e.to_string()))?
                else {
                    continue;
                };
                let key = format!("preview:{}:{}", preview.id, preview.started_at_ms);
                let (status, exit, summary) = match &preview.state {
                    crate::PreviewState::Starting | crate::PreviewState::Running => {
                        ("running", None, None)
                    }
                    crate::PreviewState::Paused => ("paused", None, None),
                    crate::PreviewState::Exited { code, status } => (
                        if code.is_some_and(|n| n != 0) {
                            "failed"
                        } else {
                            "completed"
                        },
                        *code,
                        Some(status.clone()),
                    ),
                    crate::PreviewState::Stopped { reason } => {
                        ("stopped", None, Some(reason.clone()))
                    }
                };
                let entry = Entry {
                    record: BackgroundHistoryRecord {
                        id: key.clone(),
                        conversation_id: id.0.clone(),
                        worker_id: preview.task_id.map(|id| id.0),
                        provider: None,
                        native_session_id: None,
                        generation: "preview".into(),
                        task_id: preview.id,
                        call_id: None,
                        kind: "preview".into(),
                        purpose: preview.name,
                        command: Some(preview.command),
                        cwd: Some(preview.workdir),
                        status: status.into(),
                        started_at_ms: preview.started_at_ms,
                        ended_at_ms: preview.ended_at_ms,
                        exit_code: exit,
                        stop_actor: summary
                            .as_ref()
                            .filter(|s| s.as_str() == super::preview::USER_STOP)
                            .map(|_| "user".into()),
                        summary,
                        output_available: preview.log.is_some(),
                        output_truncated: false,
                        output_bytes: 0,
                    },
                    epoch: epoch().into(),
                    source: None,
                    preview_log: preview.log,
                    detached: false,
                };
                match journal.records.iter().position(|e| e.record.id == key) {
                    Some(i) => journal.records[i] = entry,
                    None => journal.records.push(entry),
                }
            }
            after = after.max(journal.preview_seq);
            journal.preview_seq = after;
            journal.save(&dir)?;
            if !full {
                break;
            }
        }
        Ok(())
    }

    pub async fn background_history(
        &self,
        id: &ConversationId,
        before: Option<String>,
        limit: u32,
    ) -> Result<BackgroundHistoryPage> {
        self.core.conversation(id)?;
        self.backfill_background_history(id).await?;
        self.refresh_preview_history(id).await?;
        let _guard = DISK.lock().unwrap();
        let dir = self.history_dir(id);
        let journal = Journal::load(&dir)?;
        journal.save(&dir)?;
        page(
            journal.records.into_iter().map(|e| e.record).collect(),
            before,
            limit,
        )
    }

    pub async fn background_history_output(
        &self,
        id: &ConversationId,
        record_id: &str,
    ) -> Result<BackgroundHistoryOutput> {
        self.core.conversation(id)?;
        self.backfill_background_history(id).await?;
        self.refresh_preview_history(id).await?;
        let dir = self.history_dir(id);
        let entry = {
            let _guard = DISK.lock().unwrap();
            Journal::load(&dir)?
                .records
                .into_iter()
                .find(|e| e.record.id == record_id)
                .ok_or_else(|| Error::NotFound("background history record".into()))?
        };
        if entry.epoch == epoch()
            && entry.record.status == "running"
            && let Some(source) = &entry.source
            && let Ok((bytes, size)) = tail(Path::new(source))
        {
            let bytes = self.hide_secrets(id, bytes).await;
            let _guard = DISK.lock().unwrap();
            let mut journal = Journal::load(&dir)?;
            write_atomic(&dir.join(format!("{}.output", entry.record.id)), &bytes)?;
            if let Some(held) = journal
                .records
                .iter_mut()
                .find(|e| e.record.id == entry.record.id)
            {
                held.record.output_available = true;
                held.record.output_bytes = size;
                held.record.output_truncated = size > OUTPUT_BYTES;
            }
            journal.save(&dir)?;
        }
        let mut record = {
            let _guard = DISK.lock().unwrap();
            Journal::load(&dir)?
                .records
                .into_iter()
                .find(|e| e.record.id == entry.record.id)
                .map(|e| e.record)
                .unwrap_or(entry.record)
        };
        let output = if let Some(alias) = entry.preview_log {
            let board = self.core.board(id).await?;
            board
                .outputs
                .get(&alias)
                .and_then(|output| output.blob.parse().ok())
                .map(|hash| self.core.store().blobs().file_of(&hash))
                .and_then(|file| tail(&file).ok())
        } else {
            // Only use the durable entry's id, never a caller-supplied path.
            tail(&dir.join(format!("{}.output", record.id))).ok()
        };
        if let Some((bytes, size)) = output {
            if record.kind == "preview" {
                record.output_bytes = size;
                record.output_truncated = size > OUTPUT_BYTES;
            }
            record.output_available = true;
            Ok(BackgroundHistoryOutput {
                record,
                text: Some(String::from_utf8_lossy(&bytes).into_owned()),
                unavailable_reason: None,
            })
        } else {
            record.output_available = false;
            Ok(BackgroundHistoryOutput {
                record,
                text: None,
                unavailable_reason: Some("No output snapshot was available".into()),
            })
        }
    }
}
fn page(
    mut records: Vec<BackgroundHistoryRecord>,
    before: Option<String>,
    limit: u32,
) -> Result<BackgroundHistoryPage> {
    records.sort_by(|a, b| b.started_at_ms.cmp(&a.started_at_ms).then(b.id.cmp(&a.id)));
    let total = records.len().min(u32::MAX as usize) as u32;
    let start = match before {
        Some(id) => records
            .iter()
            .position(|e| e.id == id)
            .map(|i| i + 1)
            .ok_or_else(|| Error::Invalid("Unknown history cursor; reload history".into()))?,
        None => 0,
    };
    let end = (start + limit.clamp(1, 100) as usize).min(records.len());
    let next = (end < records.len()).then(|| records[end - 1].id.clone());
    Ok(BackgroundHistoryPage {
        records: records.drain(start..end).collect(),
        next,
        total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use brigadier_providers::{BackgroundTask, BackgroundTaskStatus};
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("bg-history-{}", uuid::Uuid::now_v7()));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn task(status: BackgroundTaskStatus) -> ProviderEvent {
        ProviderEvent::BackgroundTask {
            task: BackgroundTask {
                id: "reused-id".into(),
                item_id: Some("call".into()),
                kind: BackgroundTaskKind::Shell,
                label: "Watch CI".into(),
                status,
                started_at_ms: 100,
                ended_at_ms: (status != BackgroundTaskStatus::Running).then_some(200),
                output_file: None,
                summary: None,
                last_line: None,
            },
        }
    }
    fn apply(journal: &mut Journal, owner: &str, event: Option<&ProviderEvent>) {
        journal.apply(
            "session",
            owner,
            event,
            300,
            Generation {
                id: "native-generation".into(),
                native: Some("native".into()),
                cwd: None,
            },
            Some("claude".into()),
        );
    }
    #[test]
    fn exit_close_restart_keep_stopped_watchers_and_unknown_ends() {
        let temp = Temp::new();
        let mut journal = Journal::default();
        apply(
            &mut journal,
            "worker-1",
            Some(&task(BackgroundTaskStatus::Running)),
        );
        apply(
            &mut journal,
            "worker-1",
            Some(&task(BackgroundTaskStatus::Stopped)),
        );
        apply(&mut journal, "", Some(&task(BackgroundTaskStatus::Running)));
        apply(&mut journal, "worker-1", None);
        journal.save(&temp.0).unwrap();
        // Simulate records written by a previous daemon process.
        for entry in &mut journal.records {
            entry.epoch = "old-daemon".into();
        }
        journal.save(&temp.0).unwrap();
        let reloaded = Journal::load(&temp.0).unwrap();
        assert_eq!(reloaded.records.len(), 2);
        assert_eq!(reloaded.records[0].record.status, "stopped");
        assert_eq!(
            reloaded.records[0].record.worker_id.as_deref(),
            Some("worker-1")
        );
        assert_eq!(reloaded.records[1].record.status, "unknown");
        assert_eq!(reloaded.records[1].record.ended_at_ms, None);
    }
    #[test]
    fn duplicate_events_and_reused_task_ids_across_native_generations() {
        let mut journal = Journal::default();
        apply(&mut journal, "", Some(&task(BackgroundTaskStatus::Running)));
        apply(&mut journal, "", Some(&task(BackgroundTaskStatus::Running)));
        assert_eq!(journal.records.len(), 1);
        let first = journal.records[0].record.id.clone();
        apply(
            &mut journal,
            "",
            Some(&ProviderEvent::SessionStarted {
                native_id: "new-native".into(),
                model: None,
                cwd: None,
                cli_version: None,
                file: None,
            }),
        );
        apply(&mut journal, "", Some(&task(BackgroundTaskStatus::Running)));
        assert_eq!(journal.records.len(), 2);
        assert_ne!(journal.records[1].record.id, first);
        assert_eq!(
            journal.records[1].record.native_session_id.as_deref(),
            Some("new-native")
        );
    }
    #[test]
    fn durable_output_outlives_provider_file_and_reports_tail() {
        let temp = Temp::new();
        let provider = temp.0.join("provider.output");
        std::fs::write(&provider, vec![b'x'; OUTPUT_BYTES as usize + 10]).unwrap();
        let (snapshot, total) = tail(&provider).unwrap();
        assert_eq!(total, OUTPUT_BYTES + 10);
        assert_eq!(snapshot.len(), OUTPUT_BYTES as usize);
        let copy = temp.0.join("durable.output");
        write_atomic(&copy, &snapshot).unwrap();
        std::fs::remove_file(&provider).unwrap();
        assert_eq!(tail(&copy).unwrap().0, snapshot);
    }
    #[test]
    fn pages_are_stable_bounded_and_do_not_drop_history() {
        let mut journal = Journal::default();
        for owner in ["one", "two", "three"] {
            apply(
                &mut journal,
                owner,
                Some(&task(BackgroundTaskStatus::Running)),
            );
        }
        let records: Vec<_> = journal.records.into_iter().map(|e| e.record).collect();
        let first = page(records.clone(), None, 2).unwrap();
        assert_eq!(first.total, 3);
        assert_eq!(first.records.len(), 2);
        let next = page(records.clone(), first.next, 2).unwrap();
        assert_eq!(next.records.len(), 1);
        assert_eq!(next.next, None);
        assert!(!first.records.iter().any(|e| e.id == next.records[0].id));
        assert!(page(records, Some("not-a-cursor".into()), 2).is_err());
    }
    #[test]
    fn reused_provider_output_paths_never_resnapshot_finished_records() {
        let mut journal = Journal::default();
        let mut first = task(BackgroundTaskStatus::Stopped);
        if let ProviderEvent::BackgroundTask { task } = &mut first {
            task.output_file = Some("/provider/reused.output".into());
        }
        apply(&mut journal, "", Some(&first));
        let old_id = journal.records[0].record.id.clone();
        let mut second = task(BackgroundTaskStatus::Running);
        if let ProviderEvent::BackgroundTask { task } = &mut second {
            task.started_at_ms = 400;
            task.output_file = Some("/provider/reused.output".into());
        }
        let sources = journal.apply(
            "session",
            "",
            Some(&second),
            500,
            Generation {
                id: "native-generation".into(),
                ..Default::default()
            },
            None,
        );
        assert_eq!(sources.len(), 1);
        assert_ne!(sources[0].0, old_id);
        let on_exit = journal.apply("session", "", None, 600, Generation::default(), None);
        assert_eq!(on_exit.len(), 1);
        assert_ne!(on_exit[0].0, old_id);
    }

    #[test]
    fn reused_running_task_id_retires_the_old_output_source() {
        let mut journal = Journal::default();
        let mut first = task(BackgroundTaskStatus::Running);
        if let ProviderEvent::BackgroundTask { task } = &mut first {
            task.output_file = Some("/provider/reused.output".into());
        }
        apply(&mut journal, "", Some(&first));
        let mut second = first.clone();
        if let ProviderEvent::BackgroundTask { task } = &mut second {
            task.started_at_ms += 100;
        }
        apply(&mut journal, "", Some(&second));
        assert_eq!(journal.records.len(), 2);
        assert_eq!(journal.records[0].record.status, "unknown");
        assert_eq!(journal.records[0].record.ended_at_ms, None);
        assert_eq!(journal.records[1].record.status, "running");
        let sources = journal.apply("session", "", None, 600, Generation::default(), None);
        assert_eq!(
            sources.len(),
            1,
            "exit must not read a reused source into the old record"
        );
        assert_eq!(sources[0].0, journal.records[1].record.id);
    }

    #[test]
    fn live_lifecycle_upgrades_legacy_invocation_without_duplicate_history() {
        let mut journal = Journal::default();
        apply(&mut journal, "", Some(&task(BackgroundTaskStatus::Running)));
        let id = journal.records[0].record.id.clone();
        journal.records[0].epoch = "legacy".into();
        journal.records[0].record.generation = "legacy:native".into();
        journal.records[0].record.task_id = "call".into();
        apply(&mut journal, "", Some(&task(BackgroundTaskStatus::Stopped)));
        assert_eq!(journal.records.len(), 1);
        assert_eq!(journal.records[0].record.id, id);
        assert_eq!(journal.records[0].record.status, "stopped");
        assert_eq!(journal.records[0].record.task_id, "reused-id");
    }

    #[test]
    fn foreground_subagent_calls_have_durable_purpose_and_upgrade_to_native_background_ids() {
        let mut journal = Journal::default();
        let call = |status, detached| {
            ProviderEvent::ToolCall { item_id: "agent-call".into(), name: "Agent".into(), input: Some(serde_json::json!({"description":"Review parser", "prompt":"Read the whole parser and report concrete findings", "run_in_background":detached}).to_string()), status, output: None, kept: None }
        };
        apply(
            &mut journal,
            "",
            Some(&call(brigadier_providers::ItemStatus::InProgress, false)),
        );
        assert_eq!(journal.records.len(), 1);
        assert!(
            journal.records[0]
                .record
                .purpose
                .contains("Read the whole parser")
        );
        apply(
            &mut journal,
            "",
            Some(&call(brigadier_providers::ItemStatus::Completed, false)),
        );
        assert_eq!(journal.records[0].record.status, "completed");
        let mut background = task(BackgroundTaskStatus::Running);
        if let ProviderEvent::BackgroundTask { task } = &mut background {
            task.item_id = Some("agent-call".into());
            task.kind = BackgroundTaskKind::Agent;
        }
        apply(&mut journal, "", Some(&background));
        assert_eq!(journal.records.len(), 1);
        assert_eq!(journal.records[0].record.task_id, "reused-id");
        assert!(
            journal.records[0]
                .record
                .purpose
                .contains("Read the whole parser")
        );
    }

    #[tokio::test]
    async fn manager_history_outlives_cli_exit_preview_clear_and_restart() {
        use crate::manager::flow::{Flow, Options, Reply, Script};
        let script: Script = std::sync::Arc::new(|_| Box::pin(async { Reply::text("Done") }));
        let mut flow = Flow::start("durable-background-history", Options::default(), script).await;
        let temp = Temp::new();
        let provider = temp.0.join("watcher.output");
        std::fs::write(&provider, "CI was still waiting\n").unwrap();
        let mut event = task(BackgroundTaskStatus::Running);
        if let ProviderEvent::BackgroundTask { task } = &mut event {
            task.output_file = Some(provider.display().to_string());
        }
        flow.manager
            .follow_background_history(&flow.conversation, None, Some(&event))
            .await;
        if let ProviderEvent::BackgroundTask { task } = &mut event {
            task.status = BackgroundTaskStatus::Stopped;
            task.ended_at_ms = Some(200);
        }
        flow.manager
            .follow_background_history(&flow.conversation, None, Some(&event))
            .await;
        flow.manager
            .follow_background_history(&flow.conversation, None, None)
            .await;
        std::fs::remove_file(provider).unwrap();
        let preview: crate::Preview = serde_json::from_value(serde_json::json!({
            "id": "preview-1", "conversationId": flow.conversation, "name": "Preview", "command": "pnpm dev", "workdir": "/repo", "workspace": "/repo", "state": {"type":"stopped", "reason":"stopped by the user"}, "startedAtMs": 50, "endedAtMs": 60
        })).unwrap();
        flow.core
            .record_conversation(
                &flow.conversation,
                vec![DomainEvent::PreviewUpdated { preview }],
            )
            .await
            .unwrap();
        flow.manager
            .clear_previews(flow.conversation.clone())
            .await
            .unwrap();
        assert!(
            flow.core
                .board(&flow.conversation)
                .await
                .unwrap()
                .previews
                .is_empty()
        );
        let history = flow
            .manager
            .background_history(&flow.conversation, None, 50)
            .await
            .unwrap();
        assert_eq!(history.total, 2);
        let watcher = history
            .records
            .iter()
            .find(|r| r.kind == "shell")
            .unwrap()
            .id
            .clone();
        assert_eq!(
            history
                .records
                .iter()
                .find(|r| r.id == watcher)
                .unwrap()
                .status,
            "stopped"
        );
        flow.restart().await;
        let after = flow
            .manager
            .background_history(&flow.conversation, None, 50)
            .await
            .unwrap();
        assert_eq!(after.total, 2);
        let output = flow
            .manager
            .background_history_output(&flow.conversation, &watcher)
            .await
            .unwrap();
        assert_eq!(output.record.status, "stopped");
        assert_eq!(output.text.as_deref(), Some("CI was still waiting\n"));
        assert!(!output.record.output_truncated);
        flow.manager.shutdown().await;
    }
}
