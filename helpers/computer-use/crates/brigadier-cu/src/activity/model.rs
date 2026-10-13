//! Records and events of the activity store.
//!
//! The CU host is the single writer of these types (brigadier-next
//! `plans/brigadier-next/computer-use.md` section 2). Identity fields are stamped
//! from the connection that sent a request, never from tool arguments.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Daemon profile that owns a session. Each profile is its own daemon process
/// and its own store, so the profile is part of the session id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Native,
    CodexCompat,
}

impl Profile {
    fn id_tag(self) -> &'static str {
        match self {
            Profile::Native => "n",
            Profile::CodexCompat => "c",
        }
    }
}

/// Public session id, `cu_<profile tag>_<unique>`. Minted by the host; the
/// caller's free-form `session` string is only the [`SessionRecord::label`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new(profile: Profile, unique: &str) -> Self {
        SessionId(format!("cu_{}_{}", profile.id_tag(), unique))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// How the host learned which agent drives a session, strongest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Attribution {
    /// The proxy presented a launch credential the session host verified.
    Credential,
    /// Kernel peer credentials of the proxy matched a terminal's process tree.
    ProcessTree,
    /// No session host vouched for the caller (raw CLI outside Brigadier).
    None,
}

/// Principal class (identity-and-permissions.md section 4a).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentClass {
    /// The user's own interactive client (the app, the user's CLI).
    User,
    /// A user's mux principal.
    Mux,
    /// An ordinary agent (terminal agent, ACP subagent, MCP client).
    Agent,
}

/// Request channel. Governs view-state rules only; never identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    User,
    Cli,
    Mcp,
    Acp,
    Script,
    Remote,
}

/// Identity of the principal behind a connection, stamped by the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentIdentity {
    pub attribution: Attribution,
    /// `claude`, `codex`, `acp:<harness>`, `mux`, `cli`, `script`, `unknown`.
    pub kind: String,
    pub class: AgentClass,
    /// Stable principal key. Two connections with the same actor are the same
    /// writer for session ownership checks.
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_behalf_of: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_pid_start: Option<i64>,
}

/// The connection a request arrived on, as the host authenticated it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub identity: AgentIdentity,
    pub origin: Origin,
}

impl Caller {
    /// User operations (stop, pause, resume, policy) are allowed only for a
    /// connection the host authenticated as the user's own client. The
    /// claimed `origin` channel alone never grants them.
    pub fn is_user(&self) -> bool {
        self.identity.class == AgentClass::User
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    AgentEnd,
    IdleTtl,
    UserStop,
    HostRestart,
    Policy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SessionStatus {
    Active,
    Idle,
    Paused,
    Ended { reason: EndReason },
}

impl SessionStatus {
    pub fn is_live(self) -> bool {
        !matches!(self, SessionStatus::Ended { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    #[default]
    Background,
    ForegroundOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingMode {
    /// Events and thumbnails. The floor; always on.
    #[default]
    Events,
    EventsFrames,
    Video,
}

/// A window or app a session acted on or observed.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Target {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_start: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_redacted: Option<String>,
}

impl Target {
    /// Two targets name the same window (or the same app when no window).
    pub fn same_surface(&self, other: &Target) -> bool {
        self.pid == other.pid
            && self.pid_start == other.pid_start
            && self.window_id == other.window_id
            && self.app_name == other.app_name
            && self.app_bundle_id == other.app_bundle_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Scope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apps_allowed: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apps_denied: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent_ref: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Counters {
    pub observes: u64,
    pub acts: u64,
    pub errors: u64,
    pub frames: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: SessionId,
    pub profile: Profile,
    pub label: String,
    pub agent: AgentIdentity,
    pub origin: Origin,
    pub color: String,
    pub started_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<u64>,
    pub last_action_at_ms: u64,
    pub status: SessionStatus,
    #[serde(default)]
    pub delivery: Delivery,
    #[serde(default)]
    pub targets: Vec<Target>,
    #[serde(default)]
    pub scope: Scope,
    #[serde(default)]
    pub counters: Counters,
    #[serde(default)]
    pub recording: RecordingMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    SessionStart,
    SessionEnd,
    SessionStop,
    SessionPause,
    SessionResume,
    SessionIdle,
    Observe,
    Act,
    PolicyReject,
    ConsentRequest,
    ConsentDecide,
    Error,
}

/// Whether a tool call reads or changes the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallKind {
    Observe,
    Act,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Confirmed,
    Unverifiable,
    SuspectedNoop,
}

/// Result of one engine call, as stored and as replayed on a retry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallOutcome {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<Effect>,
    #[serde(default)]
    pub verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// The tool's reply as returned to the caller, replayed verbatim on an
    /// idempotent retry. Not persisted in the event log (it can hold AX text).
    #[serde(default, skip_serializing)]
    pub reply: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClickPoint {
    pub x: f64,
    pub y: f64,
}

/// One row of a session's append-only log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub session: SessionId,
    pub seq: u64,
    pub ts_ms: u64,
    pub tx: String,
    pub kind: EventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    pub actor: String,
    pub origin: Origin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
    /// Arguments after [`crate::activity::redact::redact_args`].
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub args_redacted: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<CallOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub click_point: Option<ClickPoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_frame: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_frame: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ax_digest: Option<String>,
}
