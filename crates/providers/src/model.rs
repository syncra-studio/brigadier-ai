//! The normalized provider model: what every adapter reports, whatever its CLI speaks.
//!
//! These types are part of the wire contract (they are stored as event payloads and exported to
//! TypeScript), so field names are camelCase and enums are internally tagged.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::redact::Redactor;

/// A CLI Brigadier can drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ProviderKind {
    Claude,
    Codex,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 2] = [ProviderKind::Claude, ProviderKind::Codex];

    /// The product name shown to the user.
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
        }
    }

    /// The binary name looked up on the login shell's PATH.
    pub fn binary(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// The vendor's own installer, as typed into a terminal (PowerShell on Windows).
    pub fn install_command(self, windows: bool) -> &'static str {
        match (self, windows) {
            (Self::Claude, false) => "curl -fsSL https://claude.ai/install.sh | bash",
            (Self::Claude, true) => "irm https://claude.ai/install.ps1 | iex",
            (Self::Codex, false) => "curl -fsSL https://chatgpt.com/codex/install.sh | sh",
            (Self::Codex, true) => "irm https://chatgpt.com/codex/install.ps1 | iex",
        }
    }

    /// Signs the user in to the CLI, as typed into a terminal.
    pub fn login_command(self) -> &'static str {
        match self {
            Self::Claude => "claude auth login",
            Self::Codex => "codex login",
        }
    }
}

impl std::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        })
    }
}

/// Whether a CLI is installed and logged in, with what to do when it is not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub provider: ProviderKind,
    /// Absolute path of the binary, when found.
    pub path: Option<String>,
    pub version: Option<String>,
    pub logged_in: bool,
    /// How the CLI is authenticated (`claude.ai`, `chatgpt`, `api key`, …).
    pub auth_method: Option<String>,
    /// Subscription plan, when the CLI reports one.
    pub plan: Option<String>,
    /// What the user should do before this provider can be used. Absent when ready.
    pub guidance: Option<String>,
    /// Its version can compact a session's context on request
    /// ([`ProviderSession::compact`](crate::ProviderSession::compact)).
    #[serde(default)]
    pub compacts: bool,
}

/// A model a provider offers, as the CLI itself reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    /// The value passed back to the CLI to select this model.
    pub id: String,
    pub display_name: String,
    pub description: String,
    /// The concrete model an alias resolves to, when the CLI says.
    pub resolved: Option<String>,
    /// Reasoning effort levels the model accepts, lowest first. Empty when it has none.
    pub efforts: Vec<String>,
    pub default_effort: Option<String>,
    pub is_default: bool,
    /// Input kinds the model accepts (`text`, `image`).
    pub input_modalities: Vec<String>,
    /// What its fast service tier offers ("1.5x speed, increased usage"), when it has one
    /// (Codex's `priority` tier).
    #[serde(default)]
    pub fast: Option<String>,
    /// An older model the CLI still offers beside a newer one ("Opus 4.8" beside "Opus 5.5"):
    /// pickers tuck it under Legacy.
    #[serde(default)]
    pub legacy: bool,
}

/// Marks legacy each model with a newer namesake: "Opus 4.8" beside "Opus 5.5", "GPT-6-Sol"
/// beside "GPT-6.1-Sol". A model without a version in its name is left alone.
pub(crate) fn mark_superseded(models: &mut [ModelInfo]) {
    let named: Vec<_> = models
        .iter()
        .map(|model| versioned(&model.display_name))
        .collect();
    for (model, own) in models.iter_mut().zip(&named) {
        if let Some((family, version)) = own
            && named
                .iter()
                .flatten()
                .any(|(other, newer)| other == family && newer > version)
        {
            model.legacy = true;
        }
    }
}

/// A model name's family (the name with its version cut out) and version: "Opus 5.5 (1M)" →
/// ("Opus  (1M)", [5, 5]), "GPT-6-Sol" → ("GPT--Sol", [6]).
pub(crate) fn versioned(name: &str) -> Option<(String, Vec<u32>)> {
    let mut start = 0;
    for token in name.split([' ', '-']) {
        let version: Option<Vec<u32>> = token.split('.').map(|part| part.parse().ok()).collect();
        if let Some(version) = version {
            let family = format!("{}{}", &name[..start], &name[start + token.len()..]);
            return Some((family, version));
        }
        start += token.len() + 1;
    }
    None
}

/// A provider's model list with its provenance, as cached on disk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalog {
    pub provider: ProviderKind,
    pub models: Vec<ModelInfo>,
    /// Version of the CLI that reported the list.
    pub cli_version: Option<String>,
    pub fetched_at_ms: i64,
}

/// One usage window (Claude's 5-hour and weekly windows, Codex's primary and secondary).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    /// Stable identifier, unique in its snapshot (`five_hour`, `seven_day`, `primary`,
    /// `secondary`, …; prefixed with the bucket for a Codex bucket other than its main one).
    pub id: String,
    pub label: String,
    /// Share of the window used, 0–100.
    pub used_percent: f64,
    pub resets_at_ms: Option<i64>,
    pub window_minutes: Option<i64>,
    /// The metered bucket it belongs to, when the provider has several (Codex's `limitId`).
    #[serde(default)]
    pub bucket: Option<String>,
    /// The one model this window limits; absent for a window every model of the provider
    /// draws on. Claude's per-model weekly windows name a family word (`opus`, `sonnet`), a
    /// Codex bucket names a model id (its `normalModelSlug`, such as `gpt-5.6-luna`): match it
    /// against a model's id, the id its alias resolves to, or its family.
    #[serde(default)]
    pub model: Option<String>,
}

/// Where a quota snapshot came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum QuotaSource {
    /// Asked for (`get_usage`, `account/rateLimits/read`): the backend's full, current view.
    #[default]
    Read,
    /// Reported by a running session as it worked (`rate_limit_event`,
    /// `account/rateLimits/updated`): may be partial.
    Event,
}

/// Remaining quota as last reported by a provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSnapshot {
    pub provider: ProviderKind,
    pub windows: Vec<QuotaWindow>,
    /// Set while the provider refuses work because a limit was reached.
    pub limit: Option<LimitHit>,
    pub observed_at_ms: i64,
    #[serde(default)]
    pub source: QuotaSource,
}

impl QuotaSnapshot {
    /// Takes in what a running session reported (`Event`) or a fresh read (`Read`):
    /// - a read replaces everything, its limit included;
    /// - an event updates the windows it names and keeps the others (Codex's rolling updates
    ///   are sparse: a window left out is unknown, not gone); it sets a limit, and lifts a
    ///   usage-window limit only
    ///   when it names the window that was hit; a spend control or credits stop is lifted
    ///   only by a read.
    pub fn merge(&mut self, incoming: &QuotaSnapshot) {
        if incoming.source == QuotaSource::Read {
            *self = incoming.clone();
            return;
        }
        self.windows
            .retain(|known| !incoming.windows.iter().any(|window| window.id == known.id));
        self.windows.extend(incoming.windows.iter().cloned());
        self.windows.sort_by_key(|window| {
            (
                window.model.is_some(),
                window.window_minutes.unwrap_or(i64::MAX),
            )
        });
        // An event without a limit lifts a usage-window limit only if it speaks for the window
        // that was hit (or, when the limit named none, for a provider-wide window): news about
        // another window or a model's own bucket says nothing about it.
        let lifts = self.limit.as_ref().is_none_or(|limit| {
            limit.kind == LimitKind::UsageWindow
                && match limit.window.as_deref() {
                    Some(hit) => incoming.windows.iter().any(|window| window.id == hit),
                    None => incoming.windows.iter().any(|window| window.model.is_none()),
                }
        });
        if incoming.limit.is_some() || lifts {
            self.limit.clone_from(&incoming.limit);
        }
        self.observed_at_ms = self.observed_at_ms.max(incoming.observed_at_ms);
    }
}

/// Why a provider refuses work.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum LimitKind {
    /// A usage window is used up; work resumes when it resets.
    #[default]
    UsageWindow,
    /// A spend control stopped it (Codex's `spendControlReached`, or ordinary usage not
    /// allowed): only a fresh read showing it clear lifts it.
    SpendControl,
    /// Out of credits.
    Credits,
}

/// A reached limit and when it lifts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LimitHit {
    /// The window that ran out, when known (`five_hour`, `seven_day`, `primary`, …).
    pub window: Option<String>,
    pub resets_at_ms: Option<i64>,
    #[serde(default)]
    pub kind: LimitKind,
}

/// What went wrong, classified so the router can react (fallback, wait, ask the user).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    /// A subscription usage window is exhausted; work resumes after the reset.
    UsageLimit,
    /// Short-term request throttling; retrying soon succeeds.
    RateLimit,
    /// The provider is overloaded or unavailable.
    Overloaded,
    /// Not logged in, or the login expired.
    Auth,
    /// Billing or credits problem on the account.
    Billing,
    /// The conversation no longer fits the model's context window.
    ContextWindow,
    /// The request was rejected as invalid.
    InvalidRequest,
    /// Content or safety policy refusal.
    Policy,
    /// The connection to the provider failed.
    Network,
    /// The CLI's sandbox got in the way.
    Sandbox,
    /// The provider or the CLI failed internally.
    Server,
    /// The CLI process itself failed (did not start, crashed, spoke garbage).
    Process,
    Other,
    /// The worker went silent mid-turn and stayed so after a nudge and a fresh session
    /// (Brigadier's stall watchdog; never from a CLI).
    Stalled,
}

/// A classified provider error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProviderError {
    pub kind: ErrorKind,
    pub message: String,
    /// The CLI retries on its own; nothing to do yet.
    pub will_retry: bool,
    /// For limit errors: which window and when it resets.
    pub limit: Option<LimitHit>,
    /// The provider's own error code, for debugging (`rate_limit`, `usageLimitExceeded`, …).
    pub code: Option<String>,
}

/// Something a session wants permission for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalKind {
    /// Run a shell command.
    Command,
    /// Write or edit files.
    FileChange,
    /// Use some other tool.
    Tool,
    /// Widen the session's permissions (more paths, network).
    Permissions,
}

/// A permission request from a CLI, routed to Brigadier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    /// Unique within the session; used to answer.
    pub id: String,
    pub kind: ApprovalKind,
    /// The CLI's tool name (`Bash`, `Edit`, `commandExecution`, …).
    pub tool: String,
    /// The command line, for command approvals.
    pub command: Option<String>,
    pub cwd: Option<String>,
    /// Files involved, for file changes.
    pub paths: Vec<String>,
    /// Why the CLI asks, in its own words.
    pub reason: Option<String>,
    /// The request is to run outside the OS sandbox or to widen it.
    pub escalation: bool,
    /// The tool input as JSON text, for display.
    pub input: Option<String>,
    /// Set when the user may allow similar requests for the rest of the conversation
    /// ([`ApprovalDecision::AllowSimilar`]): what that covers, as shown (a command's first
    /// words, such as `git push`, or a network host). See [`crate::policy::Similar`].
    #[serde(default)]
    pub grant: Option<String>,
}

/// The answer to an [`ApprovalRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ApprovalDecision {
    Allow,
    /// Allow, and allow similar requests (the same first words of a command, the same network
    /// host) for the rest of the conversation without asking ("Allow similar commands").
    /// Never persisted.
    AllowSimilar,
    Deny {
        message: String,
    },
}

/// Who answered an approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Decider {
    /// Brigadier's policy, on the user's behalf.
    Policy,
    User,
    /// Replayed from a recording, which keeps the answer but not who gave it.
    Recorded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    User,
    Assistant,
}

/// Where a tool call, command or file change stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ItemStatus {
    InProgress,
    Completed,
    Failed,
    Declined,
}

/// How a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TurnStatus {
    Completed,
    Interrupted,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FileChangeKind {
    Add,
    Update,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    pub kind: FileChangeKind,
}

/// Lines `start..=end` of a file, counted from 1. `end: None` runs to the end of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LineRange {
    pub start: u64,
    pub end: Option<u64>,
}

/// A file a session read: with its own read tool, or a command that prints it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FileRead {
    /// As the tool or command named it: absolute, or relative to the event's `cwd`.
    pub path: String,
    /// The lines it got. `None`: the whole file, or a part the tool or command doesn't tell.
    pub lines: Option<LineRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SearchKind {
    /// Through the files' contents (`Grep`, `rg`, `grep`).
    Content,
    /// For files by name, or a listing (`Glob`, `find`, `ls`, `rg --files`).
    Files,
}

/// A search a session made, with the files it found when the tool or command tells them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FileSearch {
    pub kind: SearchKind,
    /// What it looked for: the text pattern, or the file name pattern. `None` for a listing.
    pub pattern: Option<String>,
    /// Where: a folder or file, absolute or relative to the event's `cwd`; `None` for the
    /// working directory.
    pub scope: Option<String>,
    /// The file filter, when the tool has one apart from the scope (`Grep`'s `glob`/`type`).
    pub glob: Option<String>,
    /// The files it found, as reported (absolute, or relative to `cwd` or the scope). Empty
    /// when it found none or doesn't say.
    pub hits: Vec<String>,
}

/// Token usage, as totals for the session so far or for one turn.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    /// Cost reported by the CLI, when it reports one.
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum NoticeLevel {
    Info,
    Warning,
}

/// Everything a provider session reports, in one vocabulary for every CLI.
///
/// Streaming text arrives as `*Delta` events keyed by `itemId`; the matching final event
/// (`message`, `reasoning`) carries the complete text and replaces whatever the deltas built.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ProviderEvent {
    /// The CLI accepted the session. `nativeId` is its own session or thread id.
    SessionStarted {
        native_id: String,
        model: Option<String>,
        cwd: Option<String>,
        cli_version: Option<String>,
    },
    TurnStarted {
        turn_id: Option<String>,
    },
    MessageDelta {
        item_id: String,
        text: String,
    },
    Message {
        item_id: String,
        role: Role,
        text: String,
    },
    ReasoningDelta {
        item_id: String,
        text: String,
    },
    /// A reasoning summary (the model's own words about its thinking).
    Reasoning {
        item_id: String,
        text: String,
    },
    ToolCall {
        item_id: String,
        name: String,
        /// Tool input as JSON text, once complete.
        input: Option<String>,
        status: ItemStatus,
        /// Result text (truncated), once finished.
        output: Option<String>,
    },
    Command {
        item_id: String,
        command: String,
        cwd: Option<String>,
        status: ItemStatus,
        exit_code: Option<i32>,
        /// Combined output (truncated), once finished.
        output: Option<String>,
        duration_ms: Option<i64>,
    },
    CommandOutputDelta {
        item_id: String,
        text: String,
    },
    /// What a finished tool call or command (`itemId`) read and searched, in the same words
    /// for every CLI: Claude's `Read`, `Grep` and `Glob`, Codex's commands as it parsed them
    /// (`commandActions`). It follows the call's own completed event. `cwd` is what relative
    /// paths start from, when known.
    Looked {
        item_id: String,
        cwd: Option<String>,
        reads: Vec<FileRead>,
        searches: Vec<FileSearch>,
    },
    /// Work under way that the transcript doesn't show (a sub-agent's steps, a long tool's
    /// progress): the session is alive. `itemId` is the tool call or sub-agent it belongs to.
    /// Never stored.
    Progress {
        item_id: String,
    },
    FileChanges {
        item_id: String,
        changes: Vec<FileChange>,
        status: ItemStatus,
    },
    /// A generated image.
    Image {
        item_id: String,
        status: ItemStatus,
        path: Option<String>,
        prompt: Option<String>,
    },
    Usage {
        /// Totals for the session so far.
        total: TokenUsage,
        /// What the latest request used, when the CLI says (Codex).
        #[serde(default)]
        last: Option<TokenUsage>,
    },
    ContextSize {
        used_tokens: i64,
        window_tokens: Option<i64>,
    },
    /// The CLI began compacting the conversation (summarizing it to free up context): asked
    /// to ([`ProviderSession::compact`](crate::ProviderSession::compact)), or on its own
    /// (`automatic`) as the context filled up.
    CompactionStarted {
        automatic: bool,
    },
    /// The compaction ended: the context before and after it, when the CLI says, or why it
    /// failed.
    CompactionEnded {
        automatic: bool,
        tokens_before: Option<i64>,
        tokens_after: Option<i64>,
        error: Option<String>,
    },
    RateLimits {
        quota: QuotaSnapshot,
    },
    ApprovalRequested {
        request: ApprovalRequest,
    },
    ApprovalResolved {
        id: String,
        decision: ApprovalDecision,
        decided_by: Decider,
    },
    TurnCompleted {
        turn_id: Option<String>,
        status: TurnStatus,
        duration_ms: Option<i64>,
        usage: Option<TokenUsage>,
    },
    Error {
        error: ProviderError,
    },
    Notice {
        level: NoticeLevel,
        message: String,
    },
    /// The CLI process ended.
    Exited {
        code: Option<i32>,
        /// The last lines it wrote to stderr, when it failed.
        stderr_tail: Option<String>,
    },
}

/// What a session is allowed to do. The CLI's own OS sandbox enforces it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Access {
    /// Full-auto inside the OS sandbox: write only the working directory and `extraRoots`,
    /// network on. Anything else is an approval request.
    Workspace {
        #[ts(as = "Vec<String>")]
        extra_roots: Vec<PathBuf>,
    },
    /// Read-only sandbox: every write or command that needs approval goes to Brigadier.
    ReadOnly,
    /// No OS sandbox.
    Full,
    /// A worker's sandbox, each part set on its own. Everything is readable except
    /// `denyRead` (where the CLI's sandbox can deny reads).
    Scoped {
        /// The working directory is writable. Codex always makes it writable, so a read-only
        /// Codex worker runs in its scratch folder instead.
        write_cwd: bool,
        #[ts(as = "Vec<String>")]
        writable_roots: Vec<PathBuf>,
        network: bool,
        #[ts(as = "Vec<String>")]
        deny_read: Vec<PathBuf>,
        /// Unix sockets it may connect to.
        #[ts(as = "Vec<String>")]
        unix_sockets: Vec<PathBuf>,
    },
}

impl Access {
    /// Directories the session may write besides its working directory.
    pub fn writable_roots(&self) -> &[PathBuf] {
        match self {
            Self::Workspace { extra_roots } => extra_roots,
            Self::Scoped { writable_roots, .. } => writable_roots,
            Self::ReadOnly | Self::Full => &[],
        }
    }

    /// The same access with its writable roots as the file system resolves them
    /// ([`crate::policy::real_path`]): a CLI that compares paths as text then sees
    /// `/private/tmp/x` inside a root given as `/tmp/x`.
    #[must_use]
    pub fn resolved(&self) -> Self {
        let real = |roots: &[PathBuf]| -> Vec<PathBuf> {
            roots
                .iter()
                .map(|root| crate::policy::real_path(root).unwrap_or_else(|| root.clone()))
                .collect()
        };
        let mut access = self.clone();
        match &mut access {
            Self::Workspace { extra_roots } => *extra_roots = real(extra_roots),
            Self::Scoped { writable_roots, .. } => *writable_roots = real(writable_roots),
            Self::ReadOnly | Self::Full => {}
        }
        access
    }
}

/// How a session begins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Origin {
    New,
    /// Continue the CLI session or thread `nativeId`.
    Resume {
        native_id: String,
    },
    /// Branch a new session off `nativeId`, leaving the original untouched.
    Fork {
        native_id: String,
    },
}

/// A file sent with a turn. Images reach the model as images; other files are named by path,
/// for a session that can read them.
#[derive(Debug, Clone, PartialEq)]
pub struct InputFile {
    pub path: PathBuf,
    /// The name the user gave it (the file's own name when attached).
    pub name: String,
    pub mime: String,
}

impl InputFile {
    pub fn is_image(&self) -> bool {
        is_image_mime(&self.mime)
    }
}

/// Image formats supported by both provider adapters.
pub fn is_image_mime(mime: &str) -> bool {
    matches!(
        mime,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    )
}

/// One ordered piece of a turn's input.
#[derive(Debug, Clone, PartialEq)]
pub enum InputPart {
    Text(String),
    Image(InputFile),
}

/// What a turn (or a steer) sends, in model-visible order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TurnInput {
    pub parts: Vec<InputPart>,
    /// Non-image files, named after the message when the adapters serialize it.
    pub files: Vec<InputFile>,
}

impl TurnInput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            parts: vec![InputPart::Text(text.into())],
            files: Vec::new(),
        }
    }

    /// The legacy attachment order: images first, then text and non-image file notes.
    pub fn with_files(text: String, files: Vec<InputFile>) -> Self {
        let (images, files): (Vec<_>, Vec<_>) = files.into_iter().partition(InputFile::is_image);
        let mut parts: Vec<_> = images.into_iter().map(InputPart::Image).collect();
        parts.push(InputPart::Text(text));
        Self { parts, files }
    }

    /// Finish non-image file notes only after all message text, including handoff follow-ups.
    pub fn parts_with_file_notes(&self) -> Vec<InputPart> {
        let mut parts = self.parts.clone();
        for file in &self.files {
            let note = format!(
                "[Attached file \"{}\" ({}): {}]",
                file.name,
                file.mime,
                file.path.display()
            );
            if let Some(InputPart::Text(last)) = parts.last_mut() {
                if !last.is_empty() {
                    last.push('\n');
                }
                last.push_str(&note);
            } else {
                parts.push(InputPart::Text(note));
            }
        }
        parts
    }

    pub fn append_text(&mut self, text: &str) {
        if let Some(InputPart::Text(last)) = self.parts.last_mut() {
            last.push_str(text);
        } else {
            self.parts.push(InputPart::Text(text.into()));
        }
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
            && self.parts.iter().all(|part| match part {
                InputPart::Text(text) => text.trim().is_empty(),
                InputPart::Image(_) => false,
            })
    }

    /// Add context before the user's ordered message.
    pub fn prepend_text(&mut self, text: &str) {
        // Keep legacy row images before all text.
        let at = self
            .parts
            .iter()
            .position(|part| matches!(part, InputPart::Text(_)))
            .unwrap_or(self.parts.len());
        let prefix = format!("{text}\n\n");
        if let Some(InputPart::Text(first)) = self.parts.get_mut(at) {
            first.insert_str(0, &prefix);
        } else {
            self.parts.push(InputPart::Text(prefix));
        }
    }
}

impl From<String> for TurnInput {
    fn from(text: String) -> Self {
        Self::text(text)
    }
}

/// Which of the CLI's built-in tools a session gets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToolSet {
    /// The CLI's usual tools (workers, raw sessions).
    #[default]
    Default,
    /// The usual tools less the ones a Brigadier worker never uses, which only enlarge the
    /// start of every request (a worker's lean start, PLAN.md §7). Codex: as `Default`.
    Lean,
    /// None at all: only the MCP servers given (the orchestrator).
    None,
    /// Web search and fetch only (Chats).
    Web,
    /// A one-shot reviewer's: reading files and running the read-only git commands that show
    /// a change, and nothing else (Claude; Codex runs its reviews through `codex exec review`).
    Review,
    /// A session's thread (THREAD-PLAN.md Q1): reading, searching, running commands, editing
    /// and the web, with no sub-agents and nothing that runs in the background. Codex: its
    /// usual tools less sub-agents.
    Thread,
}

/// An MCP server a session gets, launched by the CLI over stdio.
#[derive(Debug, Clone, PartialEq)]
pub struct McpServer {
    pub name: String,
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Its tools may run for this long (a worker's blocking question waits for an answer).
    pub tool_timeout_secs: Option<u64>,
    /// Its tool calls run without asking for approval (Brigadier's own server).
    pub trusted: bool,
    /// Its tools are in the model's tool list from the start. Claude otherwise defers MCP
    /// tools behind its tool search, and a model that has to look a tool up first tends to
    /// fall back to its built-in ones.
    pub always_load: bool,
    /// Tools of a trusted server whose calls still go through the session's approvals
    /// (Codex: `mcp_servers.<id>.tools.<tool>.approval_mode = "prompt"`): a thread's command
    /// that leaves its sandbox.
    pub prompt_tools: Vec<String>,
}

/// A command Claude runs after each of a thread's `Bash` calls, with the call's event on its
/// stdin (`PostToolUse` and `PostToolUseFailure` hooks, matcher `Bash`). Its reply may replace
/// a successful call's output for the model: Brigadier's lossless trimming (`brigadierd hook
/// post-tool-use`, THREAD-PLAN.md Q4). Only a [`ToolSet::Thread`] session gets it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputHook {
    pub command: PathBuf,
    pub args: Vec<String>,
    /// Set in the CLI's environment, which its hooks inherit (a grant, which never goes on a
    /// command line).
    pub env: Vec<(String, String)>,
    pub timeout_secs: u64,
}

/// Everything needed to start a provider session.
#[derive(Debug, Clone)]
pub struct SessionSpec {
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Runs on the model's fast service tier, where it has one (see [`ModelInfo::fast`]).
    pub fast: bool,
    pub origin: Origin,
    pub access: Access,
    /// Appended to the CLI's own system prompt.
    pub append_system_prompt: Option<String>,
    /// MCP servers for the session. Only these are loaded.
    pub mcp_servers: Vec<McpServer>,
    pub tools: ToolSet,
    /// Folders besides `cwd` the session works in (a thread's workspace): Claude gets each as
    /// `--add-dir`. They are writable where the working directory is ([`Self::access_with_dirs`]).
    pub add_dirs: Vec<PathBuf>,
    /// Extra environment for the CLI and everything it starts (the process tag, TMPDIR).
    pub env: Vec<(String, String)>,
    /// Variables taken out of the CLI's environment, and so out of everything it starts (an
    /// overnight run's workers don't get the user's tokens or SSH agent).
    pub unset_env: Vec<String>,
    /// Runs the CLI, and everything it starts, at low OS priority (every worker: its builds
    /// yield to the user's own work).
    pub low_priority: bool,
    /// Records the raw stdio exchange to this file (JSONL), for replay fixtures.
    pub record_to: Option<PathBuf>,
    /// Secret values (the project's secret files, the session's grants) replaced in every
    /// event, logged stderr line and recorded line before they leave the adapter.
    pub redactor: Option<Arc<Redactor>>,
    /// `cwd` is a folder Brigadier created for this session (an orchestrator folder, a worker
    /// worktree or scratch folder, a Chat folder). Only then may the adapter record and later
    /// undo what the CLI persists about that exact folder in the user's own configuration
    /// (Codex's project trust entry). Raw sessions in the user's folders are never owned.
    pub owned_cwd: bool,
    /// The CLI may compact its context on its own as it fills up. Off for orchestrators,
    /// which are reborn instead (PLAN.md §2): Claude's auto-compact is switched off; Codex's
    /// cannot be, so its limit is raised as far as Codex allows (90% of the window).
    pub auto_compact: bool,
    /// The models the session may hand work to through the CLI's own sub-agents; `None`: no
    /// limit (raw sessions, orchestrators, Chats). Claude allows exactly these, or runs without
    /// its Agent tool when it can't; Codex runs without sub-agents.
    pub allowed_models: Option<AllowedModels>,
    /// The CLI's own reviewer settles requests to go beyond a sandboxed session's access
    /// (Approve for me): Claude runs in its auto mode, Codex with its auto-review. What it
    /// can't settle is declined to the model, which works around it or reports it. Otherwise
    /// such requests reach Brigadier (Ask for approval). No effect under [`Access::Full`],
    /// which never asks, or [`Access::ReadOnly`].
    pub auto_review: bool,
    /// The CLI leaves Co-authored-by trailers that name an AI out of the commits it writes
    /// (the user's setting, for sessions that commit): Claude through its attribution
    /// settings; Codex, which has none, is told in its instructions by the caller.
    pub omit_ai_coauthors: bool,
    /// A Claude thread's output hook; Codex runs no hooks.
    pub output_hook: Option<OutputHook>,
}

impl SessionSpec {
    /// The session's access with [`Self::add_dirs`] writable wherever its working directory
    /// is: every sandbox that writes its working directory writes them too.
    #[must_use]
    pub fn access_with_dirs(&self) -> Access {
        let mut access = self.access.clone();
        let roots = match &mut access {
            Access::Workspace { extra_roots } => extra_roots,
            Access::Scoped {
                write_cwd: true,
                writable_roots,
                ..
            } => writable_roots,
            Access::Scoped { .. } | Access::ReadOnly | Access::Full => return access,
        };
        for dir in &self.add_dirs {
            if !roots.contains(dir) {
                roots.push(dir.clone());
            }
        }
        access
    }
}

/// The models a worker's own sub-agents may run on (PLAN.md §7): the router's eligible set for
/// its task, in its CLI's exact model ids.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AllowedModels {
    /// Exact model ids (`claude-opus-5-5`, never an alias such as `opus`), the session's own
    /// model among them. Empty when its own model has no exact id: then no sub-agents at all.
    pub ids: Vec<String>,
    /// Every exact id of the CLI's models Brigadier knows of that the session may not use (its
    /// list, Fable and hidden models included, and the registry's), a context variant kept
    /// apart (`claude-opus-5-5[1m]`): what an allowed id must not also admit.
    pub outside: Vec<String>,
}

/// Something a session created that must be removed when it is disposed of. Recorded in the
/// session's cleanup ledger the moment it is known.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Artifact {
    /// A CLI process and its process group.
    Process {
        pid: u32,
        /// Start time, to tell it apart from a later process that reused the pid.
        started_at_ms: Option<f64>,
    },
    /// A Claude Code session: its transcript and per-session state under the config directory.
    ClaudeSession { session_id: String },
    /// A Claude Code project directory (`projects/<encoded cwd>`) that did not exist before.
    ClaudeProjectDir { path: String },
    /// Claude Code's write-staging directory in the working directory (`.claude/.cc-writes`,
    /// or `.claude` itself when that did not exist). Removed only while it holds no files.
    ClaudeStagingDir { path: String },
    /// A Codex thread: its rollout file and state records, removed through `thread/delete`.
    CodexThread { thread_id: String },
    /// The folder where Codex saves a thread's generated images
    /// (`$CODEX_HOME/generated_images/<thread id>`), which deleting the thread leaves behind.
    CodexGeneratedImages { path: String },
    /// A project trust entry Codex persisted in the user's `config.toml` when a thread started
    /// there. Removed through Codex's config API, only while it is still just `trusted`.
    CodexProjectTrust { path: String },
    /// Every process working inside `dir`, a folder Brigadier created (a worktree, a scratch
    /// folder): what a worker started there, including processes that left its tree.
    ProcessesIn { dir: String },
    /// A git worktree Brigadier created (for a task or a session) in its data directory.
    Worktree { repo: String, path: String },
    /// A folder Brigadier created in its data directory (a worker's scratch folder, the
    /// orchestrator's or a Chat's working folder).
    ScratchDir { path: String },
    /// A short temp folder of a Claude session's own (`/tmp/brigadier-<id>`), for Claude's
    /// temp files and its sandboxed commands' TMPDIR.
    ClaudeTempDir { path: String },
}

#[cfg(test)]
mod input_tests {
    use super::*;

    #[test]
    fn handoff_follow_up_precedes_file_notes_in_the_same_text_part() {
        let file = InputFile {
            path: "/attachments/readme.txt".into(),
            name: "readme.txt".into(),
            mime: "text/plain".into(),
        };
        let mut input = TurnInput::with_files("briefing".into(), vec![file]);
        input.append_text("\n\nWaiting for you now:\nquestion");
        assert_eq!(input.parts_with_file_notes(), vec![InputPart::Text("briefing\n\nWaiting for you now:\nquestion\n[Attached file \"readme.txt\" (text/plain): /attachments/readme.txt]".into())]);
    }

    #[test]
    fn legacy_files_keep_images_first_and_exact_file_notes() {
        let image = InputFile {
            path: "/attachments/photo.png".into(),
            name: "photo.png".into(),
            mime: "image/png".into(),
        };
        let file = InputFile {
            path: "/attachments/readme.txt".into(),
            name: "readme.txt".into(),
            mime: "text/plain".into(),
        };
        let mut input = TurnInput::with_files("user".into(), vec![file, image.clone()]);
        input.prepend_text("context");
        assert_eq!(input.parts_with_file_notes(), vec![InputPart::Image(image), InputPart::Text("context\n\nuser\n[Attached file \"readme.txt\" (text/plain): /attachments/readme.txt]".into())]);
        assert!(TurnInput::default().is_empty());
        assert!(TurnInput::text(" \n").is_empty());
        assert!(!input.is_empty());
    }
}
