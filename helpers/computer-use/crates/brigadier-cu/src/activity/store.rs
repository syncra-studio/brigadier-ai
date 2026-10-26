//! File-backed activity store (one per daemon profile).
//!
//! Layout under `root` (created 0700):
//!
//! ```text
//! sessions/<id>/record.json   {record, next_seq, first_seq}, rewritten atomically (temp + rename)
//! sessions/<id>/events.jsonl  one Event per line, append-only (retention may drop a prefix)
//! sessions/<id>/frames.jsonl  one FrameEntry per line (index for retention and frame lookups)
//! blobs/<aa>/<sha256>.<ext>   content-addressed images, written before the event that names them
//! ```
//!
//! No database dependency: brigadier-cu builds with `--locked` and the lockfile is
//! not regenerated for this step. The API is the contract; an index can be
//! added behind it later.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::model::{Event, SessionId, SessionRecord};
use super::retention::{FrameKind, FrameRef, Plan, SessionSummary};

/// One stored frame of an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameEntry {
    pub seq: u64,
    pub slot: FrameSlot,
    pub kind: StoredFrameKind,
    pub blob: String,
    pub width: u32,
    pub height: u32,
    /// Size of the captured image the thumbnail was made from (click points
    /// are in its pixels).
    #[serde(default)]
    pub source_width: u32,
    #[serde(default)]
    pub source_height: u32,
    pub bytes: u64,
    pub captured_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameSlot {
    Before,
    After,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredFrameKind {
    Thumbnail,
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecordFile {
    record: SessionRecord,
    next_seq: u64,
    #[serde(default)]
    first_seq: u64,
}

/// A stored session as loaded at start.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredSession {
    pub record: SessionRecord,
    pub next_seq: u64,
    pub first_seq: u64,
}

pub struct ActivityStore {
    root: PathBuf,
    temp_counter: std::sync::atomic::AtomicU64,
}

impl ActivityStore {
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        create_private_dir(&root)?;
        create_private_dir(&root.join("sessions"))?;
        create_private_dir(&root.join("blobs"))?;
        Ok(ActivityStore {
            root,
            temp_counter: std::sync::atomic::AtomicU64::new(0),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn session_dir(&self, id: &SessionId) -> io::Result<PathBuf> {
        if !valid_name(id.as_str()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid session id",
            ));
        }
        Ok(self.root.join("sessions").join(id.as_str()))
    }

    /// Every stored session, oldest first by start time.
    pub fn load(&self) -> io::Result<Vec<StoredSession>> {
        let mut out = Vec::new();
        for entry in fs::read_dir(self.root.join("sessions"))? {
            let path = entry?.path().join("record.json");
            let Ok(bytes) = fs::read(&path) else { continue };
            let Ok(file) = serde_json::from_slice::<RecordFile>(&bytes) else {
                continue;
            };
            out.push(StoredSession {
                record: file.record,
                next_seq: file.next_seq,
                first_seq: file.first_seq,
            });
        }
        out.sort_by(|a, b| {
            (a.record.started_at_ms, &a.record.id).cmp(&(b.record.started_at_ms, &b.record.id))
        });
        Ok(out)
    }

    pub fn write_record(
        &self,
        record: &SessionRecord,
        next_seq: u64,
        first_seq: u64,
    ) -> io::Result<()> {
        let dir = self.session_dir(&record.id)?;
        create_private_dir(&dir)?;
        let body = serde_json::to_vec(&RecordFile {
            record: record.clone(),
            next_seq,
            first_seq,
        })
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.write_atomic(&dir.join("record.json"), &body)
    }

    /// Writes the record, keeping the stored `first_seq` (retention moves it).
    pub fn write_record_keeping_first_seq(
        &self,
        record: &SessionRecord,
        next_seq: u64,
    ) -> io::Result<()> {
        let path = self.session_dir(&record.id)?.join("record.json");
        let first_seq = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<RecordFile>(&bytes).ok())
            .map_or(0, |file| file.first_seq);
        self.write_record(record, next_seq, first_seq)
    }

    /// Appends events (any sessions, seq order per session).
    pub fn append_events(&self, events: &[Event]) -> io::Result<()> {
        let mut by_session: BTreeMap<&SessionId, Vec<&Event>> = BTreeMap::new();
        for event in events {
            by_session.entry(&event.session).or_default().push(event);
        }
        for (id, list) in by_session {
            let dir = self.session_dir(id)?;
            create_private_dir(&dir)?;
            let mut buf = Vec::new();
            for event in list {
                serde_json::to_writer(&mut buf, event)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                buf.push(b'\n');
            }
            append_private(&dir.join("events.jsonl"), &buf)?;
        }
        Ok(())
    }

    pub fn append_frames(&self, id: &SessionId, frames: &[FrameEntry]) -> io::Result<()> {
        if frames.is_empty() {
            return Ok(());
        }
        let dir = self.session_dir(id)?;
        create_private_dir(&dir)?;
        let mut buf = Vec::new();
        for frame in frames {
            serde_json::to_writer(&mut buf, frame)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            buf.push(b'\n');
        }
        append_private(&dir.join("frames.jsonl"), &buf)
    }

    /// Events with `seq > after_seq` (all when `None`), at most `limit`.
    pub fn read_events(
        &self,
        id: &SessionId,
        after_seq: Option<u64>,
        limit: usize,
    ) -> io::Result<Vec<Event>> {
        let path = self.session_dir(id)?.join("events.jsonl");
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut out = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            let Ok(event) = serde_json::from_str::<Event>(&line) else {
                continue;
            };
            if after_seq.is_none_or(|after| event.seq > after) {
                out.push(event);
                if out.len() >= limit {
                    break;
                }
            }
        }
        Ok(out)
    }

    pub fn read_frames(&self, id: &SessionId) -> io::Result<Vec<FrameEntry>> {
        read_jsonl(&self.session_dir(id)?.join("frames.jsonl"))
    }

    /// Stores `bytes` content-addressed; returns the blob name `<sha256>.<ext>`.
    pub fn put_blob(&self, bytes: &[u8], ext: &str) -> io::Result<String> {
        let digest = Sha256::digest(bytes);
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        let name = format!("{hex}.{ext}");
        let path = self.blob_path(&name)?;
        if !path.exists() {
            create_private_dir(path.parent().expect("blob parent"))?;
            self.write_atomic(&path, bytes)?;
        }
        Ok(name)
    }

    pub fn read_blob(&self, name: &str) -> io::Result<Vec<u8>> {
        fs::read(self.blob_path(name)?)
    }

    pub fn has_blob(&self, name: &str) -> bool {
        self.blob_path(name).map(|p| p.exists()).unwrap_or(false)
    }

    fn blob_path(&self, name: &str) -> io::Result<PathBuf> {
        if !valid_name(name) || name.len() < 3 || !name.contains('.') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid blob name",
            ));
        }
        Ok(self.root.join("blobs").join(&name[..2]).join(name))
    }

    /// Inputs for the retention planner.
    pub fn retention_inputs(
        &self,
        live: &BTreeSet<SessionId>,
    ) -> io::Result<(Vec<SessionSummary>, Vec<FrameRef>)> {
        let mut sessions = Vec::new();
        let mut frames = Vec::new();
        for stored in self.load()? {
            let id = stored.record.id.clone();
            let ended = if live.contains(&id) {
                None
            } else {
                stored
                    .record
                    .ended_at_ms
                    .or(Some(stored.record.last_action_at_ms))
            };
            sessions.push(SessionSummary {
                id: id.clone(),
                ended_at_ms: ended,
                first_seq: stored.first_seq,
                next_seq: stored.next_seq,
            });
            for frame in self.read_frames(&id)? {
                frames.push(FrameRef {
                    session: id.clone(),
                    seq: frame.seq,
                    kind: match frame.kind {
                        StoredFrameKind::Full => FrameKind::Full,
                        StoredFrameKind::Thumbnail => FrameKind::Thumbnail,
                    },
                    blob: frame.blob,
                    captured_at_ms: frame.captured_at_ms,
                    bytes: frame.bytes,
                });
            }
        }
        Ok((sessions, frames))
    }

    /// Applies a retention plan, then removes blobs nothing references.
    pub fn apply_plan(&self, plan: &Plan) -> io::Result<()> {
        for id in &plan.delete_sessions {
            let dir = self.session_dir(id)?;
            match fs::remove_dir_all(&dir) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        for (id, before) in &plan.truncate_events {
            let dir = self.session_dir(id)?;
            let events: Vec<Event> = read_jsonl(&dir.join("events.jsonl"))?;
            let kept: Vec<&Event> = events.iter().filter(|e| e.seq >= *before).collect();
            self.rewrite_jsonl(&dir.join("events.jsonl"), &kept)?;
            let frames = self.read_frames(id)?;
            let kept: Vec<&FrameEntry> = frames.iter().filter(|f| f.seq >= *before).collect();
            self.rewrite_jsonl(&dir.join("frames.jsonl"), &kept)?;
            let path = dir.join("record.json");
            if let Ok(bytes) = fs::read(&path) {
                if let Ok(mut file) = serde_json::from_slice::<RecordFile>(&bytes) {
                    file.first_seq = file.first_seq.max(*before);
                    let body = serde_json::to_vec(&file)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                    self.write_atomic(&path, &body)?;
                }
            }
        }
        let mut drop_by_session: BTreeMap<&SessionId, BTreeSet<(u64, &str)>> = BTreeMap::new();
        for frame in &plan.delete_frames {
            drop_by_session
                .entry(&frame.session)
                .or_default()
                .insert((frame.seq, frame.blob.as_str()));
        }
        for (id, drop) in drop_by_session {
            let dir = self.session_dir(id)?;
            if !dir.exists() {
                continue;
            }
            let frames = self.read_frames(id)?;
            let kept: Vec<&FrameEntry> = frames
                .iter()
                .filter(|f| !drop.contains(&(f.seq, f.blob.as_str())))
                .collect();
            self.rewrite_jsonl(&dir.join("frames.jsonl"), &kept)?;
        }
        self.collect_garbage()
    }

    /// Deletes blob files no `frames.jsonl` references.
    pub fn collect_garbage(&self) -> io::Result<()> {
        let mut referenced = BTreeSet::new();
        for entry in fs::read_dir(self.root.join("sessions"))? {
            let frames: Vec<FrameEntry> = read_jsonl(&entry?.path().join("frames.jsonl"))?;
            referenced.extend(frames.into_iter().map(|f| f.blob));
        }
        for shard in fs::read_dir(self.root.join("blobs"))? {
            let shard = shard?.path();
            if !shard.is_dir() {
                continue;
            }
            for blob in fs::read_dir(&shard)? {
                let blob = blob?.path();
                let name = blob
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_owned();
                if name.starts_with('.') {
                    continue; // a write in progress
                }
                if !referenced.contains(&name) {
                    let _ = fs::remove_file(&blob);
                }
            }
        }
        Ok(())
    }

    fn rewrite_jsonl<T: Serialize>(&self, path: &Path, rows: &[&T]) -> io::Result<()> {
        let mut buf = Vec::new();
        for row in rows {
            serde_json::to_writer(&mut buf, row)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            buf.push(b'\n');
        }
        self.write_atomic(path, &buf)
    }

    fn write_atomic(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let n = self
            .temp_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
        let temp = path.with_file_name(format!(".{name}.tmp-{}-{n}", std::process::id()));
        let result = (|| {
            let mut file = private_options().write(true).create_new(true).open(&temp)?;
            file.write_all(bytes)?;
            fs::rename(&temp, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<Vec<T>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for line in BufReader::new(file).lines() {
        if let Ok(row) = serde_json::from_str::<T>(&line?) {
            out.push(row);
        }
    }
    Ok(out)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn private_options() -> OpenOptions {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn append_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = private_options().create(true).append(true).open(path)?;
    file.write_all(bytes)
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
    }
    // Windows: the folder inherits the user profile's ACL, which already
    // keeps other users out.
    #[cfg(not(unix))]
    {
        fs::create_dir_all(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::model::*;
    use crate::activity::retention::{plan, RetentionPolicy, DAY_MS};
    use serde_json::Value;

    fn record(id: &str, ended: Option<u64>) -> SessionRecord {
        SessionRecord {
            id: SessionId::new(Profile::Native, id),
            profile: Profile::Native,
            label: "l".into(),
            agent: AgentIdentity {
                attribution: Attribution::None,
                kind: "cli".into(),
                class: AgentClass::Agent,
                actor: "proc:1:1".into(),
                on_behalf_of: None,
                agent_id: None,
                terminal_id: None,
                acp_session: None,
                workspace_id: None,
                harness_session_id: None,
                proxy_pid: None,
                proxy_pid_start: None,
            },
            origin: Origin::Mcp,
            color: "#E5484D".into(),
            started_at_ms: 1,
            ended_at_ms: ended,
            last_action_at_ms: 1,
            status: match ended {
                Some(_) => SessionStatus::Ended {
                    reason: EndReason::AgentEnd,
                },
                None => SessionStatus::Active,
            },
            delivery: Delivery::Background,
            targets: vec![],
            scope: Scope::default(),
            counters: Counters::default(),
            recording: RecordingMode::Events,
        }
    }

    fn event(id: &SessionId, seq: u64) -> Event {
        Event {
            session: id.clone(),
            seq,
            ts_ms: seq,
            tx: "tx".into(),
            kind: EventKind::Act,
            tool: Some("click".into()),
            actor: "proc:1:1".into(),
            origin: Origin::Mcp,
            target: None,
            args_redacted: Value::Null,
            result: None,
            reject: None,
            duration_ms: None,
            click_point: None,
            before_frame: None,
            after_frame: None,
            ax_digest: None,
        }
    }

    #[test]
    fn records_events_and_blobs_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = ActivityStore::open(dir.path().join("a")).unwrap();
        let r = record("one", None);
        store.write_record(&r, 3, 0).unwrap();
        store
            .append_events(&[event(&r.id, 0), event(&r.id, 1)])
            .unwrap();
        store.append_events(&[event(&r.id, 2)]).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].record, r);
        assert_eq!(loaded[0].next_seq, 3);
        let seqs: Vec<u64> = store
            .read_events(&r.id, Some(0), 10)
            .unwrap()
            .iter()
            .map(|e| e.seq)
            .collect();
        assert_eq!(seqs, vec![1, 2]);
        assert_eq!(store.read_events(&r.id, None, 1).unwrap().len(), 1);
        let a = store.put_blob(b"jpeg-bytes", "jpg").unwrap();
        assert_eq!(
            store.put_blob(b"jpeg-bytes", "jpg").unwrap(),
            a,
            "content addressed"
        );
        assert_eq!(store.read_blob(&a).unwrap(), b"jpeg-bytes");
        assert!(store.read_blob("../../etc/passwd").is_err());
        assert!(store
            .read_events(&SessionId("../x".into()), None, 1)
            .is_err());
    }

    #[test]
    fn retention_plan_deletes_sessions_prefixes_frames_and_orphan_blobs() {
        let dir = tempfile::tempdir().unwrap();
        let store = ActivityStore::open(dir.path()).unwrap();
        let now = 100 * DAY_MS;
        let old = record("old", Some(now - 40 * DAY_MS));
        let live = record("live", None);
        store.write_record(&old, 1, 0).unwrap();
        store.write_record(&live, 5, 0).unwrap();
        store.append_events(&[event(&old.id, 0)]).unwrap();
        store
            .append_events(&(0..5).map(|s| event(&live.id, s)).collect::<Vec<_>>())
            .unwrap();
        let stale = store.put_blob(b"stale", "jpg").unwrap();
        let fresh = store.put_blob(b"fresh", "jpg").unwrap();
        let gone = store.put_blob(b"gone", "jpg").unwrap();
        let entry = |seq, blob: &str, at| FrameEntry {
            seq,
            slot: FrameSlot::After,
            kind: StoredFrameKind::Thumbnail,
            blob: blob.into(),
            width: 320,
            height: 200,
            source_width: 1280,
            source_height: 800,
            bytes: 5,
            captured_at_ms: at,
        };
        store
            .append_frames(&old.id, &[entry(0, &gone, now - 40 * DAY_MS)])
            .unwrap();
        store
            .append_frames(
                &live.id,
                &[entry(1, &stale, now - 8 * DAY_MS), entry(4, &fresh, now)],
            )
            .unwrap();

        let live_ids: BTreeSet<SessionId> = [live.id.clone()].into_iter().collect();
        let (sessions, frames) = store.retention_inputs(&live_ids).unwrap();
        let policy = RetentionPolicy {
            session_event_cap: 3,
            ..RetentionPolicy::default()
        };
        let p = plan(now, &policy, &sessions, &frames);
        store.apply_plan(&p).unwrap();

        let loaded = store.load().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].first_seq, 2);
        let seqs: Vec<u64> = store
            .read_events(&live.id, None, 10)
            .unwrap()
            .iter()
            .map(|e| e.seq)
            .collect();
        assert_eq!(seqs, vec![2, 3, 4]);
        assert_eq!(store.read_frames(&live.id).unwrap().len(), 1);
        assert!(store.has_blob(&fresh));
        assert!(!store.has_blob(&stale));
        assert!(!store.has_blob(&gone));
    }
}
