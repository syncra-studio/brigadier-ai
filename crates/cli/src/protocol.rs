//! What `brigadier` and the daemon say to each other, after the connection's first frame
//! (`ClientFrame::Cli`, which carries the calling session and its grant): one length-prefixed
//! [`CliCall`], answered by one length-prefixed [`CliReply`], then the connection closes. A
//! `wait` that got events says so once it printed them, with one more frame ([`Heard`]): until
//! then they are only held for it, and a `wait` that ends before frees them again.
//!
//! The `--json` output of each subcommand is the [`CliReply::Ok`] output as is, or
//! `{"error": …}` ([`ErrorOutput`]). Their shapes are the checked-in schemas
//! (`crates/cli/schemas/`): the names are stable, and no field holds an app-internal path.

use serde::{Deserialize, Serialize};

/// The version of this protocol. The daemon refuses a call of another version, saying which
/// it speaks.
pub const PROTOCOL: u32 = 2;

/// What `brigadier wait` sends once it printed the events of its answer: they were heard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heard {
    pub heard: bool,
}

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
    /// The project's sidebar sessions; with `all`, every project's and every Chat.
    Sessions {
        #[serde(default)]
        all: bool,
    },
    /// Live models, account usage, guidance and the user's rules.
    Models,
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
    /// A new worker of the calling session.
    Start(Box<StartRequest>),
    /// A message to one of the caller's workers or to another session, or the answer to a
    /// worker's open permission prompt.
    Send {
        id: String,
        /// The message; absent when `answer` is given.
        text: Option<String>,
        answer: Option<Answer>,
    },
    /// The next events of the caller's workers (all of them when `ids` is empty), or the next
    /// turn end of the sessions `ids` names.
    Wait {
        ids: Vec<String>,
        /// Absent: until an event comes.
        timeout_secs: Option<u32>,
    },
    /// Closes one of the caller's workers.
    Close { id: String },
    /// Moves the calling session to another CLI, with a fresh context and a hand-off note
    /// (S38c); while a turn runs, only with `after_turn`, once it ends.
    ContinueOn {
        agent: Agent,
        #[serde(default)]
        after_turn: bool,
    },
    /// One of the tools a session keeps: `action` with the tool's own arguments, as its
    /// subcommand took them.
    Tool {
        tool: Tool,
        action: String,
        args: serde_json::Value,
    },
    /// Which session turn wrote a line of a file (S39).
    Why {
        /// The file, as an absolute path: the daemon doesn't know the caller's folder.
        path: String,
        /// From 1.
        line: u32,
    },
    /// The calling session's judgment of a message the user queued while it works
    /// (run2-D40).
    Queue { id: String, judgment: Judgment },
    /// Questions on one card for the user, answered together when they answer it.
    Ask {
        /// One or more, in the order asked.
        questions: Vec<AskItem>,
        title: Option<String>,
    },
    /// Everything the calling thread or worker needs about its job, on pull (D13): the parts
    /// listed with their sizes, or one part.
    Context {
        /// Absent: the overview.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<ContextPart>,
        /// `skills`: the one skill to print whole.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// `transcript`: search for these words.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        grep: Option<String>,
        /// `transcript`: the last N items.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last: Option<u32>,
        /// `transcript`: one exact item, as a search hit names it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item: Option<String>,
    },
}

/// One part of `brigadier context`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContextPart {
    Waiting,
    Brief,
    Decisions,
    Open,
    Ledger,
    Checkpoint,
    Transcript,
    Skills,
    Mcps,
    Project,
    Routing,
}

impl ContextPart {
    /// Every part, in the order the overview lists them.
    pub const ALL: [ContextPart; 11] = [
        Self::Brief,
        Self::Waiting,
        Self::Decisions,
        Self::Open,
        Self::Ledger,
        Self::Checkpoint,
        Self::Transcript,
        Self::Skills,
        Self::Mcps,
        Self::Project,
        Self::Routing,
    ];

    /// Its subcommand's word.
    pub fn word(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Brief => "brief",
            Self::Decisions => "decisions",
            Self::Open => "open",
            Self::Ledger => "ledger",
            Self::Checkpoint => "checkpoint",
            Self::Transcript => "transcript",
            Self::Skills => "skills",
            Self::Mcps => "mcps",
            Self::Project => "project",
            Self::Routing => "routing",
        }
    }

    pub fn of_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|part| part.word() == word)
    }
}

/// One question of a `brigadier ask` card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskItem {
    pub question: String,
    /// Recommended first; none for a free answer.
    pub options: Vec<String>,
}

/// The kept tools behind `brigadier` (STRIP-DOWN-PLAN S13, S16c), by subcommand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tool {
    Pr,
    Waiting,
    Preview,
    Browser,
    App,
    Page,
    Ledger,
    Checkpoint,
    Brain,
    Transcript,
    Code,
    Library,
    Report,
    Feedback,
    Approval,
}

impl Tool {
    /// Every tool, in the order help lists their subcommands.
    pub const ALL: &[Tool] = &[
        Self::Pr,
        Self::Waiting,
        Self::Preview,
        Self::Browser,
        Self::App,
        Self::Page,
        Self::Ledger,
        Self::Checkpoint,
        Self::Brain,
        Self::Transcript,
        Self::Code,
        Self::Library,
        Self::Report,
        Self::Feedback,
        Self::Approval,
    ];

    /// Its subcommand's name.
    pub fn command(self) -> &'static str {
        match self {
            Self::Pr => "pr",
            Self::Waiting => "waiting",
            Self::Preview => "preview",
            Self::Browser => "browser",
            Self::App => "app",
            Self::Page => "page",
            Self::Ledger => "ledger",
            Self::Checkpoint => "checkpoint",
            Self::Brain => "brain",
            Self::Transcript => "transcript",
            Self::Code => "code",
            Self::Library => "library",
            Self::Report => "report",
            Self::Feedback => "feedback",
            Self::Approval => "approval",
        }
    }

    /// The tool behind subcommand `name`.
    pub fn of_command(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|tool| tool.command() == name)
    }
}

/// `brigadier start`'s arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRequest {
    /// Absent: from the prompt.
    pub title: Option<String>,
    /// Absent: the calling session's agent.
    pub agent: Option<Agent>,
    /// Absent: the CLI's own default.
    pub model: Option<String>,
    pub effort: Option<String>,
    /// A kind of work of the rules file (`brigadier models`' Work): its row (the user's, else
    /// the shipped one) picks the agent, model and effort the flags above leave out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<String>,
    /// An independent review (`--reviewing`): whose work it reviews, by worker id, or
    /// `session` for this session's own (its plan). With it, or with the work "Independent
    /// review" (the session's then), the row is one on another ready provider than the
    /// authors', else a fresh session of theirs (harness D22).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reviewing: Vec<String>,
    /// Works in the calling session's checkout (a Chat's: `repo`'s) rather than a worktree of
    /// its own.
    pub local: bool,
    /// A Chat's worker: the repository it works in, an absolute path. Absent in a Chat: a
    /// folder of its own, with no git. A session's workers work in its own repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Absent: the calling session's current branch.
    pub base: Option<String>,
    /// Absent: the calling session's level.
    pub permission: Option<Permission>,
    /// The MCP servers it gets (`--mcp`), by the names `brigadier context mcps` lists; none
    /// by default (D18).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcps: Vec<String>,
    /// What it has beyond the goal, the repository and its servers (`--with`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub with: Vec<Capability>,
    /// The session's mockup: the images it makes show in the session's Mockup tab, and the
    /// user's notes on them come back to it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mockup: bool,
    pub prompt: String,
}

/// What a worker may be given beyond its servers (`brigadier start --with`); none has it
/// otherwise (D13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    /// The computer tools, while Settings › Computer use is on.
    ComputerUse,
}

impl Capability {
    pub const ALL: [Self; 1] = [Self::ComputerUse];

    pub fn word(self) -> &'static str {
        match self {
            Self::ComputerUse => "computer-use",
        }
    }
}

/// A CLI's own permission mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    Ask,
    AcceptEdits,
    FullAccess,
}

impl Permission {
    pub fn word(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::AcceptEdits => "accept-edits",
            Self::FullAccess => "full-access",
        }
    }
}

/// The answer to a worker's open permission prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Answer {
    Allow,
    Deny,
}

/// The daemon's answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum CliReply {
    /// The request's output: [`SessionsOutput`], [`WorkersOutput`], [`ShowOutput`],
    /// [`ReadOutput`], [`StartOutput`], [`SendOutput`], [`WaitOutput`], [`CloseOutput`],
    /// [`ContinueOnOutput`], [`ToolOutput`], [`AskOutput`], [`WhyOutput`] or [`QueueOutput`],
    /// by request.
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
    /// Its project (`sessions --all`); absent for a Chat or without `--all`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
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
    /// A `brigadier wait` of the calling session runs now.
    #[serde(default)]
    pub wait_running: bool,
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
    /// How full its context is, as its CLI last said; absent until it said, or once it ended.
    /// What the parent weighs before giving it more work instead of starting a new worker.
    #[serde(default)]
    pub context: Option<ContextSize>,
    /// What git shows of its work (P1, O2); absent without git.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitFacts>,
    /// The commands its CLI ran in its latest turn, as its CLI reported them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<TurnCommands>,
}

/// What git shows of a worker's work, read by Brigadier and never asked of the worker
/// (harness P1, D1; STRIP-DOWN-PLAN §9.1 O2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFacts {
    /// Its branch; with `--local`, the session's.
    pub branch: Option<String>,
    /// The branch it merges into, as recorded when it started; absent with `--local`.
    pub base: Option<String>,
    /// The commit it started from, as recorded.
    pub base_commit: Option<String>,
    /// Whether the checkout that has `base` checked out has no uncommitted files; absent when
    /// no checkout has it.
    pub base_checkout_clean: Option<bool>,
    /// Its commits `base` lacks, newest first (at most 20; `ahead` counts them all).
    pub commits: Vec<CommitFact>,
    pub ahead: u32,
    /// Commits `base` has that it lacks.
    pub behind: u32,
    /// Files changed in its checkout and not committed; absent when its worktree is gone.
    pub uncommitted: Option<u32>,
    /// Where its branch stands against `base`; absent with `--local`.
    pub merge: Option<MergeWord>,
    /// With `merge: conflicts`: the conflicted paths, as `git merge-tree` names them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitFact {
    pub hash: String,
    pub subject: String,
}

/// Where a worker's branch stands against its base (O2's states, and `nothing`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MergeWord {
    /// No commit of its own.
    Nothing,
    /// Every commit of its is in its base.
    Merged,
    /// Its commits merge cleanly, as a fast-forward.
    Clean,
    /// Its commits merge cleanly, but its base moved on since (`behind` counts it).
    Behind,
    /// `git merge-tree` reports conflicts.
    Conflicts,
}

impl MergeWord {
    pub fn word(self) -> &'static str {
        match self {
            Self::Nothing => "nothing",
            Self::Merged => "merged",
            Self::Clean => "clean",
            Self::Behind => "behind",
            Self::Conflicts => "conflicts",
        }
    }
}

/// The shell commands a worker's CLI ran in one turn, raw as its CLI reported them: what
/// happened, never a verdict on it (harness P1, REPORT §7.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnCommands {
    /// The worker's turn, counted from its first; absent when unknown.
    pub turn: Option<u32>,
    /// How many commands it ran in it, or that changed in it (a background one); `commands`
    /// holds the last 30.
    pub total: u32,
    pub commands: Vec<CommandFact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandFact {
    /// The whole invocation, as its CLI reported it.
    pub command: String,
    /// The turn it started in.
    pub turn: Option<u32>,
    pub status: CommandStatus,
    /// Its exit code, when its CLI reported one; absent: unknown.
    pub exit: Option<i32>,
    /// The commit HEAD was at when Brigadier saw it start; absent: unknown.
    pub head: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CommandStatus {
    /// No end seen yet (or none came: its turn or CLI was cut off).
    Running,
    /// Its CLI moved it to the background, where it still runs.
    Background,
    Done,
    Failed,
    /// The user or the CLI's permission mode refused it.
    Declined,
    /// A background command that was stopped.
    Stopped,
}

impl CommandStatus {
    pub fn word(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Background => "background",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Declined => "declined",
            Self::Stopped => "stopped",
        }
    }
}

/// How much of a CLI's context is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSize {
    pub used_tokens: i64,
    /// Absent when neither the CLI nor Brigadier knows the model's window.
    pub window_tokens: Option<i64>,
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
    /// A worker's: what git shows of its work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitFacts>,
    /// A worker's: the commands its CLI ran in its latest turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<TurnCommands>,
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

/// `brigadier start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartOutput {
    pub worker: WorkerRow,
    /// The permission mode it runs in.
    pub permission: Permission,
    /// The branch its worktree started from and merges back into; absent with `--local`.
    pub base: Option<String>,
    /// With `--work`: the row that picked what the flags left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row: Option<StartRow>,
    /// An independent review's: whose work and which provider reviews it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<StartReview>,
}

/// Whose work an independent review covers, and on what (harness D22).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartReview {
    /// The workers' ids, or the session's, whose work it reviews.
    pub of: Vec<String>,
    /// The CLIs that work ran on.
    pub authors: Vec<Agent>,
    /// It runs on another CLI than the authors'; false: none other was ready and allowed, so
    /// a fresh session of theirs reviews.
    pub other_provider: bool,
}

/// The rules-file row `brigadier start --work` followed (S32).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRow {
    pub work: String,
    /// `user` (the user's row) or `shipped`.
    pub source: String,
    /// What the row set: `agent`, `model`, `effort`; the flags given set the rest.
    pub picked: Vec<String>,
}

/// `brigadier send`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendOutput {
    pub id: String,
    /// Queued into a running turn rather than starting one.
    pub queued: bool,
    /// The answer given to its open permission prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered: Option<Answer>,
    /// A message to another session: its id, as the other session's thread shows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `brigadier wait`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitOutput {
    /// Oldest first.
    pub events: Vec<WaitEvent>,
    pub timed_out: bool,
}

/// One event of a worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitEvent {
    /// The worker's id.
    pub id: String,
    /// The event's own id: the same each time it is shown, and heard once.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub event_id: String,
    pub event: EventKind,
    /// A turn's last message; why it failed, stopped or is gone; a usage limit; what runs
    /// while it is quiet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// What a permission prompt asks to do: the command, or the tool and its target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asks: Option<String>,
    /// When a usage limit resets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at_ms: Option<i64>,
    pub at_ms: i64,
    /// A turn's end: what git showed of its work as it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitFacts>,
    /// A turn's end: the commands its CLI ran in that turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<TurnCommands>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventKind {
    /// Its turn ended, with its last message.
    TurnEnded,
    /// Its CLI asks permission (`brigadier send <id> --allow|--deny` answers), asks a
    /// question, or one of its MCP servers wants the user to sign in.
    NeedsInput,
    /// Its turn ended on an error.
    Failed,
    /// Its turn was stopped (by the user, or by Brigadier quitting while it worked).
    Interrupted,
    /// Its CLI ended.
    Gone,
    /// Its model hit a usage limit.
    UsageLimit,
    /// Its CLI compacted its context.
    FreshContext,
    /// Its CLI said nothing for 20 minutes while it worked.
    Quiet,
}

impl EventKind {
    pub fn word(self) -> &'static str {
        match self {
            Self::TurnEnded => "turn-ended",
            Self::NeedsInput => "needs-input",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Gone => "gone",
            Self::UsageLimit => "usage-limit",
            Self::FreshContext => "fresh-context",
            Self::Quiet => "quiet",
        }
    }
}

/// `brigadier continue-on`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinueOnOutput {
    pub agent: Agent,
    /// Absent: the CLI's own default.
    pub model: Option<String>,
    pub effort: Option<String>,
    pub when: ContinueWhen,
}

/// When `brigadier continue-on` moves the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContinueWhen {
    /// It runs there already.
    Already,
    /// Now: its hand-off note first, then the new session there.
    Now,
    /// Once the running turn ends.
    AfterTurn,
}

/// `brigadier queue`'s judgment of a queued message (run2-D40).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum Judgment {
    /// It belongs to the work running now: it is taken into it.
    Join,
    /// A request of its own: it waits, and comes again on its own turn.
    Wait,
    /// It belongs to one of the caller's workers' work (run2-D38): its words go to that worker.
    Worker { id: String },
    /// A request of its own that could run apart: the user is offered a new session for it.
    NewSession,
}

/// What became of a queued message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QueueState {
    Joined,
    Waiting,
    SentToWorker,
    Offered,
}

/// `brigadier queue`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueOutput {
    pub id: String,
    pub state: QueueState,
    /// It had already joined the work by itself, unjudged, before this judgment came.
    pub already: bool,
    /// For `sent-to-worker`: the worker's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<String>,
}

/// `brigadier close`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseOutput {
    pub id: String,
    /// False when it had already ended.
    pub closed: bool,
}

/// `brigadier preview`, `browser`, `app`, `page` and the other tools' subcommands: what the
/// tool answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutput {
    pub action: String,
    pub text: String,
    /// Screenshots, as `data:` URLs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
    /// Where `--json`-less output saved them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub saved: Vec<String>,
}

/// `brigadier library`: the Library's items, and how each agent took a change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryOutput {
    pub action: String,
    /// `list`: every Library item; `add`: the items it added.
    pub items: Vec<LibraryItem>,
    /// `add`, `remove` and `login`: each agent's part.
    pub agents: Vec<LibraryAgentResult>,
    /// Done, with something to know: "It's added, but it didn't start: …".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// One Library item: a skill or an MCP server every agent can have.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryItem {
    pub name: String,
    /// `skill` or `server`.
    pub kind: String,
    /// Its command, address or folder.
    pub detail: String,
    /// On in every project.
    pub everywhere: bool,
    /// When not everywhere: the project folders it is on in.
    pub on_in: Vec<String>,
    /// When everywhere: the project folders it is off in.
    pub off_in: Vec<String>,
    /// Each agent's state where the call looked: `ready`, `off`, `needsSignIn`, `wontStart`,
    /// `missing`, `notSupported` or `working`.
    pub agents: Vec<LibraryAgentState>,
    /// A sign-in it waits for, or why it won't start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryAgentState {
    pub agent: String,
    pub state: String,
}

/// How one agent took a change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryAgentResult {
    pub agent: String,
    /// `done`, `skipped` or `failed`.
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `brigadier ledger`: what it printed, and for `check` the lists behind that text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerOutput {
    pub action: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<LedgerCheck>,
}

/// `brigadier ledger check <file>`: what the file leaves out. Plumbing, never a verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerCheck {
    pub file: String,
    /// Rules and decisions with an id that the file never cites.
    pub uncited: Vec<LedgerEntry>,
    /// Ids the file cites that are closed or superseded.
    pub ended: Vec<LedgerEnded>,
    /// Ids the file cites that the ledger doesn't have.
    pub unknown: Vec<String>,
    /// The ledger's entries with no id.
    pub no_id: Vec<LedgerNoId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerEntry {
    pub id: String,
    /// Verbatim.
    pub entry: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerEnded {
    pub id: String,
    /// `closed` or `superseded`.
    pub how: String,
    /// The entry that ended it, when that has an id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerNoId {
    /// `ledger.md` or `ledger-archive.md`.
    pub file: String,
    /// From 1.
    pub line: u32,
    pub entry: String,
}

/// `brigadier context`: the overview, or one part. `text` is what it prints; the lists beside
/// it hold the same, by item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextOutput {
    /// `overview`, or the part's word.
    pub part: String,
    pub text: String,
    /// The overview: each part, with its size and the command that opens it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<ContextPartSize>,
    /// `decisions`, `open` and `ledger`: each entry, verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<ContextEntry>,
    /// `skills` and `mcps`: each skill or server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<ContextItem>,
    /// `project`: the checkout's own instruction files, whole.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<ContextFile>,
    /// `routing`: what `brigadier models --json` prints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<crate::models::ModelsOutput>,
}

/// One part as the overview lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPartSize {
    pub part: String,
    pub summary: String,
    /// How many entries, items or files it holds, where it counts any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entries: Option<u32>,
    pub lines: u32,
    /// About how many tokens its text is (a token per four characters).
    pub tokens: u32,
    pub command: String,
}

/// One entry of a ledger, or a card waiting on the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextEntry {
    /// `L12`; a card's id; none for a ledger entry written without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// `rule`, `decision`, `open`, `close`, `other`, `question` or `approval`.
    pub kind: String,
    /// Whose it is: `session` (a worker's session's, read-only to it), `own` (the caller's
    /// own ledger, when it is a worker) or `card`.
    pub source: String,
    /// Verbatim.
    pub text: String,
}

/// One skill or MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextItem {
    /// What `brigadier context skills <name>` takes; a server's name as its tools know it.
    pub name: String,
    /// `skill` or `server`.
    pub kind: String,
    /// A skill's description; a server's origin.
    pub detail: String,
    /// The caller has it: a server its CLI runs, or will once it starts.
    pub has: bool,
    /// A server its CLI reported: `connected`, `connecting`, `needsAuth` or `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// One instruction file of the checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextFile {
    /// From the checkout's root.
    pub path: String,
    pub lines: u32,
    pub text: String,
}

/// `brigadier ask`: the user's answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskOutput {
    /// The first question's answer: the only one, when one question was asked.
    pub answer: String,
    /// The first question's chosen option's index (0 is the first), or none for an answer in
    /// their own words.
    pub option: Option<u32>,
    /// Every question with its answer, in the order asked.
    pub answers: Vec<AskAnswer>,
}

/// One question of a `brigadier ask` card, and the user's answer to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskAnswer {
    pub question: String,
    pub answer: String,
    /// The chosen option's index (0 is the first), or none for an answer in their own words.
    pub option: Option<u32>,
}

impl AskOutput {
    /// The output for `answers`, the first one's also at the top.
    pub fn of(answers: Vec<AskAnswer>) -> Self {
        let first = answers.first();
        Self {
            answer: first.map(|first| first.answer.clone()).unwrap_or_default(),
            option: first.and_then(|first| first.option),
            answers,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each tool goes over the socket as its subcommand's name, and back as itself.
    #[test]
    fn every_tool_call_round_trips_as_its_subcommand() {
        for tool in Tool::ALL {
            let call = CliCall {
                protocol: PROTOCOL,
                request: CliRequest::Tool {
                    tool: *tool,
                    action: "search".into(),
                    args: serde_json::json!({"query": "x"}),
                },
            };
            let json = serde_json::to_value(&call).unwrap();
            assert_eq!(json["request"]["tool"], tool.command());
            assert_eq!(serde_json::from_value::<CliCall>(json).unwrap(), call);
            assert_eq!(Tool::of_command(tool.command()), Some(*tool));
        }
        assert_eq!(Tool::of_command("sessions"), None);
    }
}

/// `brigadier why`: who wrote a line, read from the CLI's own session files, the turn
/// snapshots and git.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhyOutput {
    /// The file, relative to its checkout's top level.
    pub path: String,
    pub line: u32,
    /// The line as the file has it now.
    pub text: String,
    pub by: WhyBy,
    /// The session whose turn wrote it, or landed the worker's work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<WhySession>,
    /// That turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<WhyTurn>,
    /// The worker that wrote it (`by` is `worker`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<WhyWorker>,
    /// The commit that holds it: `git blame`'s, or the one made during the turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<WhyCommit>,
    /// Later turns that only moved the line, newest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub moved_by: Vec<WhyMove>,
}

/// Who wrote a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WhyBy {
    /// A session's turn.
    Turn,
    /// A worker, whose work landed in the session's checkout.
    Worker,
    /// A commit outside any session's turn.
    Commit,
    /// An edit outside any session's turn, not committed.
    Uncommitted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhySession {
    pub id: String,
    pub title: String,
    pub agent: Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhyTurn {
    /// Its number in the session, from 1.
    pub number: u32,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
    /// Where the line was when the turn ended (a later turn may have moved it).
    pub path: String,
    pub line: u32,
    /// The user's message that started it, whole.
    pub message: Option<String>,
    /// Its last reply, from the start; `reply_cut` when trimmed.
    pub reply: Option<String>,
    pub reply_cut: bool,
    /// The command that prints the message and the reply whole.
    pub read: Option<String>,
    /// The session's ledger entries stamped during the turn, verbatim.
    pub ledger: Vec<WhyLedgerEntry>,
    /// Why its message and reply can't be read: its CLI's session file is gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gone: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhyLedgerEntry {
    /// `L<n>`; absent for an entry without one.
    pub id: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhyWorker {
    pub id: String,
    pub number: u32,
    pub title: String,
    pub agent: Agent,
    pub branch: Option<String>,
    /// The first message it was given, whole.
    pub task: Option<String>,
    /// Its last reply, from the start; `reply_cut` when trimmed.
    pub reply: Option<String>,
    pub reply_cut: bool,
    /// The command that prints its task and reply whole.
    pub read: Option<String>,
    /// Why its task and reply can't be read: its CLI's session file is gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gone: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhyCommit {
    pub id: String,
    pub author: String,
    pub at_ms: i64,
    pub summary: String,
    /// The merge, made during the turn, that brought it in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge: Option<String>,
    /// The branch it came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhyMove {
    /// The session's id.
    pub session: String,
    pub turn: u32,
}

#[cfg(test)]
mod context_schema_tests {
    use super::*;
    #[test]
    fn every_context_part_is_accepted_by_its_output_schema() {
        let schema = crate::commands::find("context").unwrap().schema;
        for part in ContextPart::ALL {
            let value = serde_json::json!({"part":part.word(),"text":""});
            assert!(
                crate::schema::violations(schema, &value).is_empty(),
                "{}",
                part.word()
            );
        }
    }
}
