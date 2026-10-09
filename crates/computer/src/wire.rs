//! The helper's socket protocol (§4.2): what brigadierd's broker sends the helper and what
//! comes back.
//!
//! Frames are a 4-byte big-endian length and a body. The first frame on a connection is a
//! [`Hello`] with the per-launch token. After it, the daemon sends [`Request`]s and the helper
//! sends [`HelperFrame`]s. A reply's images follow it as binary frames, in the order its
//! [`ImageMeta`] fields list them (`image`, then `trajectory`).
//!
//! Requests are multiplexed by id. Engine work (apps, launch, observe, act, zoom, describe)
//! runs one at a time on the engine thread. Control work (permissions, cancel, end a session,
//! stop, ping) is answered at once by the connection's reader, even while the engine is busy.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::action::{ActRequest, ActionResult, ObserveRequest, ZoomRequest};
use crate::block::BlockList;
use crate::desktop::WindowInfo;
use crate::error::CuError;
use crate::geom::Provider;
use crate::record::ActionRecord;

/// Bumped when a frame changes incompatibly; the helper refuses another version.
pub const PROTOCOL: u32 = 1;
/// The largest frame either side accepts.
pub const MAX_FRAME: usize = 16 << 20;

/// The first frame on a connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub token: String,
    pub protocol: u32,
}

/// The block-list facts that depend on the session, from the broker's own state (§5). One
/// session's exceptions never reach another's requests.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Policy {
    /// The Brigadier instance that hosts the session.
    #[serde(default)]
    pub host_pid: Option<i32>,
    #[serde(default)]
    pub host_bundle_path: Option<String>,
    /// Terminal processes and windows this session launched, which it may drive.
    #[serde(default)]
    pub launched_pids: Vec<i32>,
    #[serde(default)]
    pub launched_windows: Vec<u32>,
    /// The worker's name, for its cursor's pill (§4.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl Policy {
    pub fn block_list(&self) -> BlockList {
        BlockList {
            host_pid: self.host_pid,
            host_bundle_path: self.host_bundle_path.clone(),
            launched_windows: self
                .launched_windows
                .iter()
                .copied()
                .collect::<HashSet<_>>(),
            launched_pids: self.launched_pids.iter().copied().collect::<HashSet<_>>(),
        }
    }
}

/// `launch`: an app (name, bundle id or path), a file or a URL, or an app with a file or URL.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LaunchRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// A file path or a URL to open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<String>,
}

/// Which system permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grant {
    Accessibility,
    ScreenRecording,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    // Engine work.
    Apps,
    Launch(LaunchRequest),
    Observe(ObserveRequest),
    Act(ActRequest),
    Zoom(ZoomRequest),
    /// The process behind a window, for leases and approvals before an `act`.
    Describe {
        window: u32,
    },
    /// Closes these windows of a process a worker launched, before it is quit at the worker's
    /// end, so the app doesn't reopen them at the user's next launch.
    CloseWindows {
        instance: Instance,
        windows: Vec<u32>,
    },
    // Control work, answered at once.
    Permissions,
    /// Registers the helper with the system for this permission (the system's own prompt) so
    /// it appears in System Settings, then reports the permissions.
    RequestPermission {
        grant: Grant,
    },
    /// The user's Start over: forgets this helper's own entry for the permission in the
    /// system's privacy settings (one left by an older build may never match this one), then
    /// asks again as [`Op::RequestPermission`] does.
    ResetPermission {
        grant: Grant,
    },
    /// Ends the request with this id: between two events if it runs, before it starts if it
    /// waits.
    Cancel {
        request: u64,
    },
    /// The worker ended: its requests are cancelled and its caches dropped.
    EndSession,
    /// The global stop, sent when the user stops Brigadier's computer use.
    StopAll,
    Ping,
}

impl Op {
    /// Answered by the connection's reader, never queued behind the engine.
    pub fn is_control(&self) -> bool {
        matches!(
            self,
            Self::Permissions
                | Self::RequestPermission { .. }
                | Self::ResetPermission { .. }
                | Self::Cancel { .. }
                | Self::EndSession
                | Self::StopAll
                | Self::Ping
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    /// The worker the broker took from the authenticated grant, never from the model.
    pub worker: String,
    #[serde(default = "claude")]
    pub provider: Provider,
    #[serde(default)]
    pub policy: Policy,
    #[serde(flatten)]
    pub op: Op,
}

fn claude() -> Provider {
    Provider::Claude
}

/// An image that follows a reply as a binary frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageMeta {
    pub id: String,
    pub width: u32,
    pub height: u32,
    /// The labelled estimate for the request's provider.
    pub tokens: u32,
    pub bytes: usize,
    pub mime: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permissions {
    pub accessibility: bool,
    pub screen_recording: bool,
    /// Screen recording was just allowed, and this helper restarts to start using it: macOS
    /// gives a running process the grant only after a relaunch.
    #[serde(default)]
    pub restarting: bool,
}

/// A process, told apart from a later one with the same pid by its start time.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Instance {
    pub pid: i32,
    /// The process's start time, microseconds since the Unix epoch.
    pub started_us: u64,
}

/// What `describe` returns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Described {
    pub instance: Instance,
    pub window: WindowInfo,
    pub app_name: String,
    pub bundle_id: Option<String>,
    pub bundle_path: Option<String>,
    /// The block list's reason when the window is off limits.
    pub blocked: Option<String>,
}

/// What `launch` did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Launched {
    pub instance: Instance,
    pub app_name: String,
    pub bundle_id: Option<String>,
    /// A process this launch started; `false` when the system handed back one that was
    /// already running (the user's own, perhaps), which is never the worker's to quit.
    pub new_process: bool,
    /// The windows this launch opened.
    pub new_windows: Vec<u32>,
    /// Windows the app reopened from its saved state as it started: the user's, not this
    /// launch's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub restored_windows: Vec<u32>,
    /// The app took the front and was put back behind the user's app (§2.1).
    pub front_restored: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    pub id: u64,
    pub ok: bool,
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<CuError>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<ActionResult>,
    /// The action records of an `act`, for the session's action log.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub records: Vec<ActionRecord>,
    /// The image for the model, when the request asked for one or `auto` chose one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageMeta>,
    /// The action log's image of an `act`: the window with every predicted point marked. Never
    /// shown to the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trajectory: Option<ImageMeta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launched: Option<Launched>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub described: Option<Described>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Permissions>,
    /// Time spent in the engine, for the tool-overhead measure (S6).
    #[serde(default)]
    pub engine_ms: f64,
}

impl Reply {
    pub fn error(id: u64, e: CuError) -> Self {
        Self {
            id,
            ok: false,
            text: e.to_string(),
            error: Some(e),
            ..Default::default()
        }
    }
}

/// Something the helper tells the broker on its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// The user stopped computer use from the helper's menu or hotkey: every lease goes.
    Stopped { by: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HelperFrame {
    Reply(Box<Reply>),
    Event(Event),
}

/// Reads one frame.
pub fn read_frame(r: &mut impl std::io::Read) -> std::io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let n = u32::from_be_bytes(len) as usize;
    if n > MAX_FRAME {
        return Err(std::io::Error::other("frame too large"));
    }
    let mut buf = vec![0; n];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

/// Writes one frame.
pub fn write_frame(w: &mut impl std::io::Write, b: &[u8]) -> std::io::Result<()> {
    if b.len() > MAX_FRAME {
        return Err(std::io::Error::other("frame too large"));
    }
    let len = u32::try_from(b.len()).map_err(std::io::Error::other)?;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(b)?;
    w.flush()
}

/// A reply and the images that followed it (`image`, then `trajectory`, as listed).
#[derive(Debug, Clone)]
pub struct Answer {
    pub reply: Reply,
    pub image: Option<Vec<u8>>,
    pub trajectory: Option<Vec<u8>>,
}

/// The connection ended before the reply came.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gone;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_reads_back_with_its_op_flattened() {
        let r = Request {
            id: 7,
            worker: "task-3".into(),
            provider: Provider::Codex,
            policy: Policy {
                host_pid: Some(5),
                ..Default::default()
            },
            op: Op::Cancel { request: 6 },
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["op"], "cancel");
        let back: Request = serde_json::from_value(v).unwrap();
        assert_eq!(back, r);
        assert!(back.op.is_control());
        let act: Request = serde_json::from_value(serde_json::json!({
            "id": 1, "worker": "w", "op": "act", "window": 3,
            "actions": [{"do": "select", "ref": "e4", "start": 6, "length": 5}]
        }))
        .unwrap();
        assert!(!act.op.is_control());
        assert_eq!(act.provider, Provider::Claude);
    }
}
