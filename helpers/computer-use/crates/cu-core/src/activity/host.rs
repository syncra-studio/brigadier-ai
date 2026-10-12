//! The activity host: the daemon's single writer of sessions, events and
//! frames. Joins the pure [`SessionBook`] with the [`ActivityStore`], encodes
//! thumbnails, and fans updates out to subscribers (the Brigadier pane, the CLI).
//!
//! Every method takes `&self`; one mutex orders all writes, so the event log,
//! the record files and the broadcast stream see the same order.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

use super::model::{
    CallKind, CallOutcome, Caller, ClickPoint, Effect, Event, Profile, RecordingMode, SessionId,
    SessionRecord, Target,
};
use super::redact::RedactPolicy;
use super::reducer::{
    Applied, CallFrames, CallTicket, Command, Ctx, Minted, Outcome, Reject, SessionBook,
};
use super::retention::{self, RetentionPolicy};
use super::store::{ActivityStore, FrameEntry, FrameSlot, StoredFrameKind};
use super::thumbnail::{thumbnail_dimensions, THUMBNAIL_JPEG_QUALITY, THUMBNAIL_LONG_EDGE};

/// Tools that only read the machine.
const OBSERVE_TOOLS: &[&str] = &[
    "get_window_state",
    "get_desktop_state",
    "get_accessibility_tree",
    "get_app_state",
    "list_apps",
    "list_windows",
    "zoom",
    "get_screen_size",
    "get_cursor_position",
    "get_config",
    "check_permissions",
    "health_report",
    "get_recording_state",
    "get_agent_cursor_state",
];

/// Cursor colors for new sessions (no blue or cyan: Brigadier chrome rule).
const SESSION_COLORS: &[&str] = &[
    "#E5484D", "#30A46C", "#D6409F", "#F5A524", "#8E4EC6", "#12A594", "#F76B15", "#E93D82",
];

pub fn call_kind(tool: &str) -> CallKind {
    if OBSERVE_TOOLS.contains(&tool) {
        CallKind::Observe
    } else {
        CallKind::Act
    }
}

/// What the stream pushes to subscribers.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActivityUpdate {
    Sessions {
        sessions: Vec<SessionRecord>,
    },
    Events {
        session: SessionId,
        events: Vec<Event>,
    },
}

/// Decision for one tool call.
#[derive(Debug)]
pub enum Gate {
    /// Run the tool, then call [`ActivityHost::finish`].
    Run(CallStart),
    /// Do not run: refused (stopped, paused, other agent's session).
    Refuse { code: String, message: String },
    /// Idempotent retry: reply with the first result.
    Replay(CallOutcome),
    /// Lifecycle tools (`start_session`, `end_session`): run, nothing to finish.
    RunUntracked,
}

#[derive(Debug)]
pub struct CallStart {
    pub session: SessionId,
    pub ticket: CallTicket,
    pub caller: Caller,
    pub kind: CallKind,
    pub tx: String,
    pub started_ms: u64,
}

/// Pixels the engine produced for one call (PNG bytes as captured).
#[derive(Debug, Default)]
pub struct CapturedFrames {
    pub before_png: Option<Vec<u8>>,
    pub after_png: Option<Vec<u8>>,
    pub click_point: Option<ClickPoint>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FrameInfo {
    pub width: u32,
    pub height: u32,
    /// Captured image size; click points are in these pixels.
    pub source_width: u32,
    pub source_height: u32,
    pub expired: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TimelinePage {
    pub events: Vec<Event>,
    pub next_seq: u64,
    /// Size and availability of every frame the page names.
    pub frames: BTreeMap<String, FrameInfo>,
}

/// User operations (host-authenticated connections only).
#[derive(Debug, Clone, PartialEq)]
pub enum UserOp {
    Stop(SessionId),
    Pause(SessionId),
    Resume(SessionId),
    StopAgent(String),
    AllowAgent(String),
    SetRecording(SessionId, RecordingMode),
}

struct HostState {
    book: SessionBook,
    tx_counter: u64,
    next_retention_ms: Option<u64>,
}

pub struct ActivityHost {
    machine: String,
    store: ActivityStore,
    state: Mutex<HostState>,
    updates: broadcast::Sender<ActivityUpdate>,
    retention: RetentionPolicy,
}

impl ActivityHost {
    /// Opens the store, closes sessions a previous process left live
    /// (`host_restart`) and plans retention once.
    pub fn open(
        profile: Profile,
        machine: impl Into<String>,
        root: impl Into<PathBuf>,
        now_ms: u64,
    ) -> io::Result<Self> {
        let store = ActivityStore::open(root)?;
        let stored = store.load()?;
        let book = SessionBook::restore(
            profile,
            stored.into_iter().map(|s| (s.record, s.next_seq)).collect(),
        );
        let (updates, _) = broadcast::channel(256);
        let host = ActivityHost {
            machine: machine.into(),
            store,
            state: Mutex::new(HostState {
                book,
                tx_counter: 0,
                next_retention_ms: None,
            }),
            updates,
            retention: RetentionPolicy::default(),
        };
        let caller = host_caller();
        host.apply_and_persist(&caller, now_ms, Command::HostRestart)
            .ok();
        host.run_retention(now_ms)?;
        Ok(host)
    }

    pub fn machine(&self) -> &str {
        &self.machine
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ActivityUpdate> {
        self.updates.subscribe()
    }

    pub fn profile(&self) -> Profile {
        self.lock().book.profile()
    }

    /// Decides whether a tool call runs. `label` is the caller's session
    /// string (explicit `session`, else the proxy's minted id).
    pub fn begin(
        &self,
        caller: &Caller,
        label: &str,
        tool: &str,
        args: &Value,
        target: Option<Target>,
        idempotency_key: Option<String>,
        now_ms: u64,
    ) -> Gate {
        if tool == "end_session" {
            let live = self
                .lock()
                .book
                .live_session(&caller.identity.actor, label)
                .cloned();
            if let Some(id) = live {
                let _ = self.apply_and_persist(caller, now_ms, Command::End { id });
            }
            return Gate::RunUntracked;
        }
        let started = match self.apply_and_persist(
            caller,
            now_ms,
            Command::Start {
                label: label.to_owned(),
                scope: None,
            },
        ) {
            Ok(Applied {
                outcome: Outcome::Started { id, .. },
                ..
            }) => id,
            Ok(_) => return Gate::RunUntracked,
            Err(reject) => return refuse(reject),
        };
        if tool == "start_session" {
            return Gate::RunUntracked;
        }
        let kind = call_kind(tool);
        let command = Command::BeginCall {
            id: started.clone(),
            kind,
            tool: tool.to_owned(),
            args: args.clone(),
            target,
            idempotency_key,
        };
        let tx = self.next_tx();
        match self.apply_and_persist_tx(caller, now_ms, &tx, command) {
            Ok(Applied {
                outcome: Outcome::Execute { ticket },
                ..
            }) => Gate::Run(CallStart {
                session: started,
                ticket,
                caller: caller.clone(),
                kind,
                tx,
                started_ms: now_ms,
            }),
            Ok(Applied {
                outcome: Outcome::Replay { outcome },
                ..
            }) => Gate::Replay(outcome),
            Ok(Applied {
                outcome: Outcome::Refused { reason },
                ..
            }) => refuse(reason),
            Ok(_) => Gate::RunUntracked,
            Err(reject) => refuse(reject),
        }
    }

    /// Records the result of a call [`begin`](Self::begin) let run.
    pub fn finish(
        &self,
        start: CallStart,
        outcome: CallOutcome,
        frames: CapturedFrames,
        now_ms: u64,
    ) {
        let duration_ms = now_ms.saturating_sub(start.started_ms);
        let full = self
            .lock()
            .book
            .record(&start.session)
            .map(|r| r.recording != RecordingMode::Events)
            .unwrap_or(false);
        let mut entries: Vec<(
            FrameSlot,
            StoredFrameKind,
            String,
            (u32, u32),
            (u32, u32),
            u64,
        )> = Vec::new();
        let mut stored = |slot: FrameSlot, png: &Option<Vec<u8>>| -> Option<String> {
            let png = png.as_ref()?;
            let (jpeg, w, h, sw, sh) = encode_thumbnail(png)?;
            let blob = self.store.put_blob(&jpeg, "jpg").ok()?;
            entries.push((
                slot,
                StoredFrameKind::Thumbnail,
                blob.clone(),
                (w, h),
                (sw, sh),
                jpeg.len() as u64,
            ));
            if full {
                if let Ok(full_blob) = self.store.put_blob(png, "png") {
                    entries.push((
                        slot,
                        StoredFrameKind::Full,
                        full_blob,
                        (sw, sh),
                        (sw, sh),
                        png.len() as u64,
                    ));
                }
            }
            Some(blob)
        };
        let before_frame = stored(FrameSlot::Before, &frames.before_png);
        let after_frame = stored(FrameSlot::After, &frames.after_png);
        let command = Command::FinishCall {
            id: start.session.clone(),
            ticket: start.ticket,
            outcome,
            duration_ms,
            frames: CallFrames {
                before_frame,
                after_frame,
                click_point: frames.click_point,
                ax_digest: None,
            },
        };
        if let Ok(applied) = self.apply_and_persist_tx(&start.caller, now_ms, &start.tx, command) {
            if let Some(event) = applied.events.first() {
                let rows: Vec<FrameEntry> = entries
                    .into_iter()
                    .map(
                        |(
                            slot,
                            kind,
                            blob,
                            (width, height),
                            (source_width, source_height),
                            bytes,
                        )| FrameEntry {
                            seq: event.seq,
                            slot,
                            kind,
                            blob,
                            width,
                            height,
                            source_width,
                            source_height,
                            bytes,
                            captured_at_ms: now_ms,
                        },
                    )
                    .collect();
                let _ = self.store.append_frames(&start.session, &rows);
            }
        }
        let due = self.lock().next_retention_ms.is_some_and(|at| at <= now_ms);
        if due {
            let _ = self.run_retention(now_ms);
        }
    }

    pub fn user_op(&self, caller: &Caller, op: UserOp, now_ms: u64) -> Result<bool, Reject> {
        let command = match op {
            UserOp::Stop(id) => Command::Stop { id },
            UserOp::Pause(id) => Command::Pause { id },
            UserOp::Resume(id) => Command::Resume { id },
            UserOp::StopAgent(actor) => Command::StopAgent { actor },
            UserOp::AllowAgent(actor) => Command::AllowAgent { actor },
            UserOp::SetRecording(id, mode) => Command::SetRecording { id, mode },
        };
        let applied = self.apply_and_persist(caller, now_ms, command)?;
        let changed = !applied.events.is_empty();
        if changed {
            let _ = self.run_retention(now_ms);
        }
        Ok(changed)
    }

    pub fn redact_policy(&self) -> RedactPolicy {
        self.lock().book.redact_policy()
    }

    pub fn set_redact_policy(&self, caller: &Caller, policy: RedactPolicy) -> Result<(), Reject> {
        if !caller.is_user() {
            return Err(Reject::UserRequired);
        }
        self.lock().book.set_redact_policy(policy);
        Ok(())
    }

    /// Sessions newest activity first. `live` filters by liveness.
    pub fn sessions(
        &self,
        live: Option<bool>,
        since_ms: Option<u64>,
        limit: usize,
    ) -> Vec<SessionRecord> {
        let state = self.lock();
        let mut out: Vec<SessionRecord> = state
            .book
            .records()
            .filter(|r| live.is_none_or(|l| r.status.is_live() == l))
            .filter(|r| since_ms.is_none_or(|s| r.last_action_at_ms >= s))
            .cloned()
            .collect();
        out.sort_by(|a, b| {
            b.last_action_at_ms
                .cmp(&a.last_action_at_ms)
                .then(a.id.cmp(&b.id))
        });
        out.truncate(limit);
        out
    }

    pub fn session(&self, id: &SessionId) -> Option<SessionRecord> {
        self.lock().book.record(id).cloned()
    }

    pub fn timeline(
        &self,
        id: &SessionId,
        after_seq: Option<u64>,
        limit: usize,
    ) -> io::Result<TimelinePage> {
        let next_seq = self
            .lock()
            .book
            .next_seq(id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown session"))?;
        let events = self.store.read_events(id, after_seq, limit.max(1))?;
        let sizes: BTreeMap<String, (u32, u32, u32, u32)> = self
            .store
            .read_frames(id)?
            .into_iter()
            .filter(|f| f.kind == StoredFrameKind::Thumbnail)
            .map(|f| (f.blob, (f.width, f.height, f.source_width, f.source_height)))
            .collect();
        let mut frames = BTreeMap::new();
        for event in &events {
            for blob in [&event.before_frame, &event.after_frame]
                .into_iter()
                .flatten()
            {
                let (width, height, source_width, source_height) =
                    sizes.get(blob).copied().unwrap_or((0, 0, 0, 0));
                let expired = !sizes.contains_key(blob) || !self.store.has_blob(blob);
                frames.insert(
                    blob.clone(),
                    FrameInfo {
                        width,
                        height,
                        source_width,
                        source_height,
                        expired,
                    },
                );
            }
        }
        Ok(TimelinePage {
            events,
            next_seq,
            frames,
        })
    }

    /// A frame's bytes: the thumbnail, or the full frame when recorded.
    pub fn frame(
        &self,
        session: Option<&SessionId>,
        blob: &str,
        full: bool,
    ) -> io::Result<(String, Vec<u8>)> {
        if full {
            if let Some(id) = session {
                let frames = self.store.read_frames(id)?;
                if let Some(thumb) = frames.iter().find(|f| f.blob == blob) {
                    if let Some(full) = frames.iter().find(|f| {
                        f.kind == StoredFrameKind::Full
                            && f.seq == thumb.seq
                            && f.slot == thumb.slot
                    }) {
                        return Ok(("image/png".into(), self.store.read_blob(&full.blob)?));
                    }
                }
            }
        }
        let mime = if blob.ends_with(".png") {
            "image/png"
        } else {
            "image/jpeg"
        };
        Ok((mime.into(), self.store.read_blob(blob)?))
    }

    /// Marks sessions idle after `idle_ms` without calls and ends them after
    /// `ttl_ms` (never while a call runs). Returns the ended ids.
    pub fn sweep_idle(&self, idle_ms: u64, ttl_ms: u64, now_ms: u64) -> Vec<SessionId> {
        let records: Vec<SessionRecord> = self
            .lock()
            .book
            .records()
            .filter(|r| r.status.is_live())
            .cloned()
            .collect();
        let caller = host_caller();
        let mut ended = Vec::new();
        for record in records {
            let quiet = now_ms.saturating_sub(record.last_action_at_ms);
            if quiet >= ttl_ms {
                if let Ok(applied) = self.apply_and_persist(
                    &caller,
                    now_ms,
                    Command::ExpireIdle {
                        id: record.id.clone(),
                    },
                ) {
                    if !applied.events.is_empty() {
                        ended.push(record.id);
                    }
                }
            } else if quiet >= idle_ms {
                let _ =
                    self.apply_and_persist(&caller, now_ms, Command::MarkIdle { id: record.id });
            }
        }
        if !ended.is_empty() {
            let _ = self.run_retention(now_ms);
        }
        ended
    }

    /// Runs the retention planner and returns when it is due next.
    pub fn run_retention(&self, now_ms: u64) -> io::Result<Option<u64>> {
        let mut state = self.lock();
        let live = state.book.live_ids();
        let (sessions, frames) = self.store.retention_inputs(&live)?;
        let plan = retention::plan(now_ms, &self.retention, &sessions, &frames);
        self.store.apply_plan(&plan)?;
        for id in &plan.delete_sessions {
            state.book.forget(id);
        }
        state.next_retention_ms = plan.next_run_ms;
        Ok(plan.next_run_ms)
    }

    fn next_tx(&self) -> String {
        let mut state = self.lock();
        state.tx_counter += 1;
        format!("tx_{}_{}", std::process::id(), state.tx_counter)
    }

    fn apply_and_persist(
        &self,
        caller: &Caller,
        now_ms: u64,
        command: Command,
    ) -> Result<Applied, Reject> {
        let tx = self.next_tx();
        self.apply_and_persist_tx(caller, now_ms, &tx, command)
    }

    fn apply_and_persist_tx(
        &self,
        caller: &Caller,
        now_ms: u64,
        tx: &str,
        command: Command,
    ) -> Result<Applied, Reject> {
        let mut state = self.lock();
        let ctx = Ctx { now_ms, tx, caller };
        let count = state.book.records().count();
        let mut mint = |_: &str| Minted {
            unique: format!(
                "{:x}{:04x}",
                now_ms,
                (std::process::id() as u64 ^ count as u64) & 0xffff
            ),
            color: SESSION_COLORS[count % SESSION_COLORS.len()].to_owned(),
        };
        let command_session = match &command {
            Command::BeginCall { id, .. } | Command::SetRecording { id, .. } => Some(id.clone()),
            _ => None,
        };
        let command_kind = command.clone();
        let applied = state.book.apply(&ctx, &mut mint, command)?;
        let mut touched: Vec<SessionId> =
            applied.events.iter().map(|e| e.session.clone()).collect();
        if let Outcome::Started { id, .. } = &applied.outcome {
            touched.push(id.clone());
        }
        if let Command::BeginCall { .. } | Command::SetRecording { .. } = &command_kind {
            touched.extend(command_session.clone());
        }
        touched.sort();
        touched.dedup();
        if let Err(error) = self.store.append_events(&applied.events) {
            tracing::warn!(%error, "activity: event append failed");
        }
        for id in &touched {
            if let Some((record, next_seq)) = state.book.persisted(id) {
                if let Err(error) = self.store.write_record_keeping_first_seq(record, next_seq) {
                    tracing::warn!(%error, "activity: record write failed");
                }
            }
        }
        if !touched.is_empty() {
            let mut sessions: Vec<SessionRecord> = state.book.records().cloned().collect();
            sessions.sort_by_key(|s| std::cmp::Reverse(s.last_action_at_ms));
            let _ = self.updates.send(ActivityUpdate::Sessions { sessions });
        }
        let mut by_session: BTreeMap<SessionId, Vec<Event>> = BTreeMap::new();
        for event in &applied.events {
            by_session
                .entry(event.session.clone())
                .or_default()
                .push(event.clone());
        }
        for (session, events) in by_session {
            let _ = self
                .updates
                .send(ActivityUpdate::Events { session, events });
        }
        Ok(applied)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HostState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn refuse(reject: Reject) -> Gate {
    let message = match reject {
        Reject::SessionStopped => "the user stopped this computer use session".to_owned(),
        Reject::AgentStopped => "the user stopped computer use for this agent".to_owned(),
        Reject::SessionPaused => "the user paused this computer use session".to_owned(),
        Reject::NotOwner => "this computer use session belongs to another agent".to_owned(),
        Reject::CallInFlight => "the same call is still running".to_owned(),
        other => other.code(),
    };
    Gate::Refuse {
        code: reject.code(),
        message,
    }
}

/// The daemon itself (idle sweep, restart). Never a user.
fn host_caller() -> Caller {
    use super::model::{AgentClass, AgentIdentity, Attribution, Origin};
    Caller {
        identity: AgentIdentity {
            attribution: Attribution::None,
            kind: "host".into(),
            class: AgentClass::Agent,
            actor: "host".into(),
            on_behalf_of: None,
            agent_id: None,
            terminal_id: None,
            acp_session: None,
            workspace_id: None,
            harness_session_id: None,
            proxy_pid: None,
            proxy_pid_start: None,
        },
        origin: Origin::Script,
    }
}

/// The outcome the log stores for a tool result (`isError`, `structuredContent`).
pub fn outcome_from_result(is_error: bool, structured: Option<&Value>) -> CallOutcome {
    let effect = structured
        .and_then(|s| s.get("effect"))
        .and_then(Value::as_str)
        .and_then(|e| match e {
            "confirmed" => Some(Effect::Confirmed),
            "unverifiable" => Some(Effect::Unverifiable),
            "suspected_noop" => Some(Effect::SuspectedNoop),
            _ => None,
        });
    let verified = structured
        .and_then(|s| s.get("verified"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let error_code = structured
        .and_then(|s| s.get("code").or_else(|| s.get("error_code")))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| is_error.then(|| "tool_error".to_owned()));
    CallOutcome {
        ok: !is_error,
        effect,
        verified,
        error_code,
        reply: Value::Null,
    }
}

/// An image (PNG or JPEG) to a ~320 px JPEG thumbnail; returns the JPEG,
/// its size and the source size.
pub fn encode_thumbnail(png: &[u8]) -> Option<(Vec<u8>, u32, u32, u32, u32)> {
    let image = image::load_from_memory(png).ok()?;
    let (w, h) = thumbnail_dimensions(image.width(), image.height(), THUMBNAIL_LONG_EDGE)?;
    let small = image.thumbnail(w, h).to_rgb8();
    let mut out = Vec::new();
    let mut encoder =
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, THUMBNAIL_JPEG_QUALITY);
    encoder.encode_image(&small).ok()?;
    Some((
        out,
        small.width(),
        small.height(),
        image.width(),
        image.height(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::model::{AgentClass, AgentIdentity, Attribution, Origin, SessionStatus};
    use serde_json::json;

    fn caller(actor: &str, class: AgentClass) -> Caller {
        Caller {
            identity: AgentIdentity {
                attribution: Attribution::None,
                kind: "cli".into(),
                class,
                actor: actor.into(),
                on_behalf_of: None,
                agent_id: None,
                terminal_id: None,
                acp_session: None,
                workspace_id: None,
                harness_session_id: None,
                proxy_pid: Some(42),
                proxy_pid_start: Some(1),
            },
            origin: Origin::Mcp,
        }
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 40, 40, 255]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn a_call_is_logged_with_a_thumbnail_and_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let agent = caller("proc:42:1", AgentClass::Agent);
        let host = ActivityHost::open(Profile::Native, "local", dir.path(), 1_000).unwrap();
        let mut stream = host.subscribe();
        let Gate::Run(start) = host.begin(
            &agent,
            "run-1",
            "type_text",
            &json!({"text": "secret!", "pid": 3}),
            None,
            None,
            1_000,
        ) else {
            panic!("expected run")
        };
        let session = start.session.clone();
        host.finish(
            start,
            outcome_from_result(false, None),
            CapturedFrames {
                after_png: Some(png(1280, 800)),
                ..Default::default()
            },
            1_050,
        );
        let page = host.timeline(&session, None, 100).unwrap();
        assert_eq!(page.events.len(), 2, "session start + act");
        let act = &page.events[1];
        assert_eq!(act.duration_ms, Some(50));
        let blob = act.after_frame.clone().expect("thumbnail");
        assert_eq!(page.frames[&blob].width, 320);
        assert_eq!(page.frames[&blob].height, 200);
        assert_eq!(page.frames[&blob].source_width, 1280);
        let (mime, bytes) = host.frame(Some(&session), &blob, false).unwrap();
        assert_eq!(mime, "image/jpeg");
        assert!(!bytes.is_empty());
        let raw = std::fs::read_to_string(
            dir.path()
                .join("sessions")
                .join(session.as_str())
                .join("events.jsonl"),
        )
        .unwrap();
        assert!(!raw.contains("secret!"));
        // subscribers saw records and events
        let mut kinds = Vec::new();
        while let Ok(update) = stream.try_recv() {
            kinds.push(match update {
                ActivityUpdate::Sessions { .. } => "sessions",
                ActivityUpdate::Events { .. } => "events",
            });
        }
        assert!(kinds.contains(&"sessions") && kinds.contains(&"events"));
        drop(host);

        let reopened = ActivityHost::open(Profile::Native, "local", dir.path(), 2_000).unwrap();
        let record = reopened.session(&session).unwrap();
        assert!(
            matches!(record.status, SessionStatus::Ended { .. }),
            "host_restart closes live sessions"
        );
        assert_eq!(
            reopened.timeline(&session, None, 100).unwrap().events.len(),
            3
        );
    }

    #[test]
    fn user_stop_refuses_the_agent_and_needs_a_user_connection() {
        let dir = tempfile::tempdir().unwrap();
        let agent = caller("proc:42:1", AgentClass::Agent);
        let user = caller("app", AgentClass::User);
        let host = ActivityHost::open(Profile::Native, "local", dir.path(), 1).unwrap();
        let Gate::Run(start) = host.begin(&agent, "s", "click", &json!({}), None, None, 2) else {
            panic!()
        };
        let id = start.session.clone();
        host.finish(
            start,
            outcome_from_result(false, None),
            CapturedFrames::default(),
            3,
        );
        assert_eq!(
            host.user_op(&agent, UserOp::Stop(id.clone()), 4),
            Err(Reject::UserRequired)
        );
        assert_eq!(host.user_op(&user, UserOp::Stop(id.clone()), 5), Ok(true));
        assert!(matches!(
            host.begin(&agent, "s", "click", &json!({}), None, None, 6),
            Gate::Refuse { .. }
        ));
        assert_eq!(host.sessions(Some(true), None, 10).len(), 0);
    }

    #[test]
    fn lifecycle_tools_start_and_end_without_a_call_event() {
        let dir = tempfile::tempdir().unwrap();
        let agent = caller("proc:42:1", AgentClass::Agent);
        let host = ActivityHost::open(Profile::Native, "local", dir.path(), 1).unwrap();
        assert!(matches!(
            host.begin(&agent, "s", "start_session", &json!({}), None, None, 2),
            Gate::RunUntracked
        ));
        assert!(matches!(
            host.begin(&agent, "s", "end_session", &json!({}), None, None, 3),
            Gate::RunUntracked
        ));
        let all = host.sessions(None, None, 10);
        assert_eq!(all.len(), 1);
        assert!(!all[0].status.is_live());
    }

    #[test]
    fn idle_sweep_marks_idle_then_ends() {
        let dir = tempfile::tempdir().unwrap();
        let agent = caller("proc:42:1", AgentClass::Agent);
        let host = ActivityHost::open(Profile::Native, "local", dir.path(), 0).unwrap();
        let Gate::Run(start) = host.begin(&agent, "s", "click", &json!({}), None, None, 0) else {
            panic!()
        };
        host.finish(
            start,
            outcome_from_result(true, Some(&json!({"code": "background_occluded"}))),
            CapturedFrames::default(),
            0,
        );
        assert!(host.sweep_idle(300_000, 1_800_000, 400_000).is_empty());
        assert_eq!(host.sessions(None, None, 1)[0].status, SessionStatus::Idle);
        assert_eq!(host.sweep_idle(300_000, 1_800_000, 2_000_000).len(), 1);
        let record = &host.sessions(None, None, 1)[0];
        assert_eq!(record.counters.errors, 1);
    }
}
