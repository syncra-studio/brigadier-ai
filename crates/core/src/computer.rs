//! Computer use as Settings and the menu bar show it: the helper's permissions, where setup
//! stands, and the agents using apps.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{ConversationId, TaskId};

/// A macOS permission computer use needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ComputerPermission {
    Accessibility,
    ScreenRecording,
}

/// Whether the helper has a permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PermissionState {
    Granted,
    NotGranted,
    /// Nobody has asked the helper yet.
    Unknown,
}

/// Where computer use's setup stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ComputerSetup {
    /// Computer use is turned off.
    Off,
    /// The helper has never run: it sets up when an agent first needs it.
    NotSetUp,
    /// A permission is missing.
    Missing,
    /// Both permissions are granted; screen capture hasn't been confirmed yet.
    ConfirmCapture,
    Ready,
    /// The helper stopped answering and is being restarted.
    Restarting,
    /// The helper couldn't be started or restarted.
    Failed {
        message: String,
    },
}

/// An agent using an app through computer use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ComputerSession {
    /// The helper's session id.
    pub session: String,
    /// The conversation (a thread) or task (a worker) the agent works for.
    pub conversation_id: Option<ConversationId>,
    pub task_id: Option<TaskId>,
    /// The agent as the user knows it: its thread's or task's title.
    pub label: String,
    /// The app it drives.
    pub app: String,
    pub bundle_id: String,
    pub pid: i32,
}

/// The operating system computer use runs on. Setup differs: macOS asks for two permissions
/// and a capture check; Windows asks for nothing; Linux depends on the desktop session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ComputerPlatform {
    Macos,
    Windows,
    Linux,
}

impl ComputerPlatform {
    /// The platform this build runs on.
    pub const fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Macos
        }
    }
}

/// Computer use, as `getComputerUse` returns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ComputerUseStatus {
    /// The page is listed at all: on macOS, Windows and Linux.
    pub available: bool,
    pub platform: ComputerPlatform,
    pub accessibility: PermissionState,
    pub screen_recording: PermissionState,
    /// The permissions were read from the running helper just now, not from what it last
    /// wrote.
    pub current: bool,
    pub setup: ComputerSetup,
    pub sessions: Vec<ComputerSession>,
    /// Windows and Linux: what this desktop session lets agents do, in plain sentences, as
    /// the running helper last said (Linux: X11 or Wayland). Empty on macOS.
    pub notes: Vec<String>,
}
