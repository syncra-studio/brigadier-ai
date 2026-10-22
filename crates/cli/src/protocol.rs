//! What `brigadier` and the daemon say to each other, after the connection's first frame
//! (`ClientFrame::Cli`, which carries the calling session and its grant): one length-prefixed
//! [`CliCall`], answered by one length-prefixed [`CliReply`], then the connection closes.
//!
//! The `--json` output of each subcommand is the [`CliReply::Ok`] output as is, or
//! `{"error": …}` ([`ErrorOutput`]). Their shapes are the checked-in schemas
//! (`crates/cli/schemas/`): the names are stable, and no field holds an app-internal path.

use serde::{Deserialize, Serialize};

/// The version of this protocol. The daemon refuses a call of another version, saying which
/// it speaks.
pub const PROTOCOL: u32 = 1;

/// The calling session's id, set by Brigadier in each session's environment.
pub const SESSION_ENV: &str = "BRIGADIER_SESSION";

/// The calling session's grant, set beside [`SESSION_ENV`]. No KEY, SECRET or TOKEN in the
/// name: Codex's default environment filter drops those from the commands it runs.
pub const GRANT_ENV: &str = "BRIGADIER_SESSION_GRANT";

/// One call: the protocol version and the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliCall {
    pub protocol: u32,
    pub request: CliRequest,
}

/// The requests the daemon answers. Subcommands not built yet never reach it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "command",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CliRequest {
    /// The project's sidebar sessions.
    Sessions,
    /// The calling session's own workers.
    Workers,
    /// One session or worker in full.
    Show { id: String },
    /// A session's or worker's last message, last items, or screen.
    Read {
        id: String,
        last: Option<u32>,
        screen: bool,
    },
}

/// The daemon's answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum CliReply {
    /// The request's output: [`SessionsOutput`], [`WorkersOutput`], [`ShowOutput`] or
    /// [`ReadOutput`], by request.
    Ok {
        output: serde_json::Value,
    },
    Error {
        error: CliError,
    },
}

impl CliReply {
    pub fn ok<T: Serialize>(output: &T) -> Self {
        match serde_json::to_value(output) {
            Ok(output) => Self::Ok { output },
            Err(err) => Self::error(ErrorCode::Failed, format!("could not encode: {err}")),
        }
    }

    pub fn error(code: ErrorCode, message: impl Into<String>) -> Self {
        Self::Error {
            error: CliError {
                code,
                message: message.into(),
            },
        }
    }
}

/// Why a call failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliError {
    pub code: ErrorCode,
    pub message: String,
}

/// `--json`'s output for a failed call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorOutput {
    pub error: CliError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCode {
    /// Wrong arguments (exit 2).
    Usage,
    /// No valid session grant (exit 3).
    Refused,
    /// The subcommand is not built yet (exit 4).
    NotAvailable,
    /// Brigadier isn't running, or didn't answer (exit 5).
    Unreachable,
    /// No session or worker has that id.
    NotFound,
    /// The id is the start of several ids.
    Ambiguous,
    /// The daemon speaks another protocol version.
    Version,
    /// Anything else.
    Failed,
}

impl ErrorCode {
    /// The process's exit code for this error.
    pub fn exit_code(self) -> u8 {
        match self {
            Self::Usage => 2,
            Self::Refused => 3,
            Self::NotAvailable => 4,
            Self::Unreachable => 5,
            Self::NotFound | Self::Ambiguous | Self::Version | Self::Failed => 1,
        }
    }
}

/// Lines added and removed, and files touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Changes {
    pub files: u32,
    pub insertions: u32,
    pub deletions: u32,
}

/// `brigadier sessions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionsOutput {
    /// The project's name; absent for a session outside a project.
    pub project: Option<String>,
    /// Pinned first, then the latest active first, as the sidebar lists them.
    pub sessions: Vec<SessionRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    pub id: String,
    pub title: String,
    pub state: SessionState,
    pub branch: Option<String>,
    /// What its branch changed since its base; absent for a Local checkout session.
    pub changes: Option<Changes>,
    /// How many workers it has, finished ones included.
    pub workers: u32,
    /// It is the calling session.
    pub caller: bool,
}

/// A session's state, as its sidebar row shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionState {
    Starting,
    Working,
    /// A question, approval or plan waits for the user.
    NeedsYou,
    Failed,
    Stopped,
    /// Its CLI stopped while idle; the next message continues it.
    Asleep,
    Ready,
}

impl SessionState {
    pub fn word(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Working => "working",
            Self::NeedsYou => "needs-you",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
            Self::Asleep => "asleep",
            Self::Ready => "ready",
        }
    }
}

/// `brigadier workers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkersOutput {
    /// The calling session's id.
    pub session: String,
    /// Oldest first.
    pub workers: Vec<WorkerRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerRow {
    pub id: String,
    /// Its number in its session (`task-N`).
    pub number: u32,
    pub title: String,
    pub agent: Agent,
    /// Absent: the CLI's own default.
    pub model: Option<String>,
    pub effort: Option<String>,
    pub state: WorkerState,
    pub branch: Option<String>,
    /// What its worktree changed since it started; absent when it has none, or once it ended.
    pub changes: Option<Changes>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    pub fn word(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

/// A worker's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkerState {
    Queued,
    Starting,
    Working,
    /// It asked its session a question and waits for the answer.
    Asking,
    Paused,
    /// Its turn ended with a report.
    Reported,
    /// The user continues its CLI session in a terminal.
    TakenOver,
    Landing,
    ReadyToLand,
    Landed,
    Done,
    Rejected,
    Stopped,
    Failed,
}

impl WorkerState {
    pub fn word(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Starting => "starting",
            Self::Working => "working",
            Self::Asking => "asking",
            Self::Paused => "paused",
            Self::Reported => "reported",
            Self::TakenOver => "taken-over",
            Self::Landing => "landing",
            Self::ReadyToLand => "ready-to-land",
            Self::Landed => "landed",
            Self::Done => "done",
            Self::Rejected => "rejected",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }
}

/// Whether an id names a sidebar session or a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Session,
    Worker,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Worker => "worker",
        }
    }
}

/// Where a session works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Checkout {
    /// Its own worktree on its own branch.
    Worktree,
    /// The user's own checkout.
    Local,
}

/// `brigadier show <id>`. The session fields are absent for a worker, and the reverse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShowOutput {
    pub kind: Kind,
    pub id: String,
    pub title: String,
    /// A [`SessionState`] or [`WorkerState`] word.
    pub state: String,
    pub agent: Agent,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub branch: Option<String>,
    pub changes: Option<Changes>,
    pub created_at_ms: i64,
    /// A session's: where it works.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout: Option<Checkout>,
    /// A worktree session's: the branch it started from and merges into.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// A session's: what the user waits on, counted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting: Option<Waiting>,
    /// A session's: its workers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workers: Option<Vec<WorkerRow>>,
    /// A worker's: its session's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// A worker's: its number in its session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
    /// A worker's: the task it was given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// A worker's: its latest report's summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
    /// A worker's: why it failed or is blocked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What waits for the user in a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Waiting {
    pub questions: u32,
    pub approvals: u32,
}

/// `brigadier read <id>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadOutput {
    pub kind: Kind,
    pub id: String,
    /// A [`SessionState`] or [`WorkerState`] word.
    pub state: String,
    /// What was read: the last message, the last N items, or the screen.
    pub view: ReadView,
    /// Oldest first.
    pub items: Vec<Item>,
    /// Older items exist before the first one.
    pub has_more: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReadView {
    /// The last message the session's or worker's model wrote.
    Last,
    /// The last N items (`--last N`).
    Items,
    /// The turn under way, or the last one: from its message on (`--screen`).
    Screen,
}

/// One item of a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub kind: ItemKind,
    /// A message's author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    /// A message's text, a command line, a tool's name and input, or an error.
    pub text: String,
    /// A finished command's or tool's output, cut to its end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// A command's or tool's status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ItemStatus>,
    /// A finished command's exit code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    pub at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ItemKind {
    Message,
    Reasoning,
    Tool,
    Command,
    TurnEnd,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    User,
    Assistant,
    System,
    /// Another session's message.
    Peer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ItemStatus {
    Running,
    Done,
    Failed,
}
