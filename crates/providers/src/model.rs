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
    /// Who is logged in (the account's email), when the CLI reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub email: Option<String>,
    /// The organization the login belongs to, when the CLI reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub organization: Option<String>,
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
    /// words, such as `git push`, a network host, or the workspace for file changes). See [`crate::policy::Similar`].
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
    /// host, or files inside the granted workspace) for the rest of the conversation without asking ("Allow … for this session").
    /// Never persisted.
    AllowSimilar,
    Deny {
        message: String,
    },
}

/// What a Deny with no words tells the model. The CLI puts a deny's message into an error tool
/// result verbatim, and the API refuses an empty one, failing the whole turn.
pub const DECLINED: &str = "The user declined this. Don't try it again; tell the user what you were going to do and that you didn't do it.";

/// The words a user's Deny sends the model: their reason when they gave one, never nothing.
pub fn user_deny_message(reason: &str) -> String {
    match reason.trim() {
        "" => DECLINED.to_string(),
        reason => format!("The user declined this: {reason}"),
    }
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
    /// The file before a completed edit, when the provider supplies it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub original: Option<String>,
    pub path: String,
    pub kind: FileChangeKind,
    /// The change as diff lines (`-` out, `+` in), when the CLI says (clipped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub diff: Option<String>,
    /// Lines in and out, counted from the whole diff before it was clipped
    /// ([`FileChange::counted`]); `None` when the CLI gave no diff, and in events stored
    /// before they were counted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub added: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub removed: Option<u32>,
}

impl FileChange {
    /// A change whose lines in and out are counted from `diff`, the whole diff as the CLI gave
    /// it; the diff kept is `diff` cut to `clip` bytes.
    pub fn counted(path: String, kind: FileChangeKind, diff: Option<&str>, clip: usize) -> Self {
        let (added, removed) = diff.map(|diff| diff_counts(kind, diff)).unzip();
        Self {
            original: None,
            path,
            kind,
            diff: diff.map(|diff| crate::clip(diff, clip)),
            added,
            removed,
        }
    }
}

/// Lines in and out of a file change's diff. A new file's diff is all lines in and a deleted
/// one's all lines out, whether the CLI marks them (`+`/`-`, as Claude's `Write` does) or gives
/// the bare content. An update's are its `+` and `-` lines, past a unified diff's `---`/`+++`
/// header and its `@@` hunk lines.
pub fn diff_counts(kind: FileChangeKind, diff: &str) -> (u32, u32) {
    let mut lines: Vec<&str> = diff.lines().collect();
    if lines.len() >= 2 && lines[0].starts_with("--- ") && lines[1].starts_with("+++ ") {
        lines.drain(..2);
    }
    lines.retain(|line| !line.starts_with("@@"));
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    match kind {
        FileChangeKind::Add => (count(lines.len()), 0),
        FileChangeKind::Delete => (0, count(lines.len())),
        FileChangeKind::Update => (
            count(lines.iter().filter(|line| line.starts_with('+')).count()),
            count(lines.iter().filter(|line| line.starts_with('-')).count()),
        ),
    }
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
    /// The model calls behind it: Claude's assistant messages (a subagent's included), each
    /// Codex request. 0 where the CLI's reports don't show them.
    #[serde(default)]
    pub model_calls: i64,
    /// Of `cache_write_tokens`, what was written for an hour and for five minutes (Claude's
    /// `cache_creation`; 0 where the CLI doesn't split it).
    #[serde(default)]
    pub cache_write_1h_tokens: i64,
    #[serde(default)]
    pub cache_write_5m_tokens: i64,
}

/// One model call's prompt as its CLI reported it, to tell where cache writes come from
/// (research P0.1). Within one CLI process the previous call's whole prompt should come back
/// as this call's cache read; what didn't is the estimated missed prefix, worked out by
/// Brigadier from these (a session's first call against the session it came from).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CallCache {
    /// Input, cache read and cache write together: the prompt the call read. No output.
    pub prompt: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    /// The prompt of the call before it in the same process and chain (the main agent's, or
    /// one subagent's); `None` for a chain's first call.
    #[serde(default)]
    pub previous_prompt: Option<i64>,
    /// The first call after a compaction, whose prefix changed on purpose: reported apart.
    #[serde(default)]
    pub after_compaction: bool,
    /// A subagent's call (Claude's Agent tool), a chain of its own.
    #[serde(default)]
    pub subagent: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum NoticeLevel {
    Info,
    Warning,
}

/// What a streaming reply is: a line between the turn's steps, or the turn's final answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TextPhase {
    Commentary,
    Answer,
}

/// A tool result or command output kept whole (BUILD-PLAN D2.1): an event's `output` is cut at
/// 8 KiB, so the result as the CLI gave it is stored first, in Brigadier's blob store, and the
/// event names it here. The adapter fills `whole` (and the saved file's `text`), redacted; the
/// core stores them and records the hashes, or why it couldn't.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct KeptOutput {
    /// The blob holding the result as the CLI gave it, before Brigadier cut it; `None` when it
    /// wasn't stored (see `error`), or the result was short enough to be whole in `output`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub blob: Option<String>,
    /// Its size in bytes.
    #[serde(default)]
    pub bytes: u64,
    /// The whole output the CLI saved to a file of its own and showed the model only a preview
    /// of (Claude's `tool-results/`), copied when the event was seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub file: Option<KeptFile>,
    /// The CLI gave only an excerpt (a failing Claude command's head and tail): no more of it
    /// exists anywhere.
    #[serde(default)]
    pub excerpt: bool,
    /// Why the result couldn't be kept (a failed write).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<String>,
    /// The redacted result, waiting to be stored. Never serialized.
    #[serde(skip)]
    #[ts(skip)]
    pub whole: Option<Arc<str>>,
}

/// A file the CLI saved a whole output to, and Brigadier's copy of it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct KeptFile {
    /// Where the CLI saved it.
    pub path: String,
    /// The blob holding the copy; `None` when it couldn't be copied (see `error`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub blob: Option<String>,
    #[serde(default)]
    pub bytes: u64,
    /// Why it couldn't be copied (the file was gone, a failed write).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<String>,
    /// The redacted file, waiting to be stored. Never serialized.
    #[serde(skip)]
    #[ts(skip)]
    pub text: Option<Arc<str>>,
}

/// Whether `path` is shaped as a file the Claude CLI saves a whole tool result to: absolute,
/// right in a session's `tool-results` folder (`…/projects/<project>/<session>/tool-results/
/// <file>`), with no `..` in it (BUILD-PLAN D2.2). A command's output can print any path after
/// the CLI's words, so no other file is read as one.
pub fn is_tool_results_file(path: &str) -> bool {
    use std::path::{Component, Path};
    let path = Path::new(path);
    if !path.is_absolute() {
        return false;
    }
    let parts: Vec<Component> = path.components().collect();
    if parts
        .iter()
        .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return false;
    }
    let names: Vec<&std::ffi::OsStr> = parts
        .iter()
        .rev()
        .take(5)
        .filter_map(|part| match part {
            Component::Normal(name) => Some(*name),
            _ => None,
        })
        .collect();
    names.len() == 5 && names[1] == "tool-results" && names[4] == "projects"
}

/// Opens a file the Claude CLI saved a whole tool result to: shaped as
/// [`is_tool_results_file`], a regular file and not a link, and still so with every link on its
/// way resolved. Anything else is refused, so a forged path never has the daemon read a file
/// the session couldn't.
pub async fn open_tool_results_file(path: &str) -> std::io::Result<tokio::fs::File> {
    let refused = || {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "not a file the CLI saved a tool result to",
        )
    };
    if !is_tool_results_file(path) {
        return Err(refused());
    }
    if !tokio::fs::symlink_metadata(path)
        .await?
        .file_type()
        .is_file()
    {
        return Err(refused());
    }
    let real = tokio::fs::canonicalize(path).await?;
    if !real.to_str().is_some_and(is_tool_results_file) {
        return Err(refused());
    }
    tokio::fs::File::open(real).await
}

impl KeptOutput {
    /// Whether all of it that exists is kept: the result is stored (or was short enough to be
    /// whole in the event), and so is the CLI's saved file, if any.
    pub fn is_complete(&self) -> bool {
        self.error.is_none()
            && self.whole.is_none()
            && self
                .file
                .as_ref()
                .is_none_or(|file| file.blob.is_some() && file.error.is_none())
    }
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
        /// What the reply is, when its CLI says so as it starts (Codex does; Claude doesn't).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        phase: Option<TextPhase>,
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
        /// Where the whole result is kept, when `output` is cut or the CLI saved it to a file of
        /// its own.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        kept: Option<Box<KeptOutput>>,
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
        /// Where the whole output is kept, when `output` is cut or the CLI saved it to a file of
        /// its own.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        kept: Option<Box<KeptOutput>>,
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
    /// How many of the CLI's own background jobs (commands, sub-agents) still run. Each one
    /// that ends starts a turn of its own, so a session that ended its turn with jobs running
    /// still has work coming. Never stored.
    BackgroundJobs {
        running: u32,
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
        /// The model calls since the last report, each as it read its prompt.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        calls: Vec<CallCache>,
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
    /// The CLI began compacting on its own and the session's `PreCompact` hook refused it: a
    /// session that must not compact ([`SessionSpec::auto_compact`] off) is handed off or
    /// reborn instead (BUILD-PLAN D2.3, D2.3b).
    CompactionRefused,
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
    /// The CLI's own guess at the user's next message, after a turn
    /// ([`SessionSpec::suggestions`]): shown in the empty composer, never sent by itself.
    PromptSuggestion {
        text: String,
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
        /// The signal that ended it, when one did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        signal: Option<i32>,
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
    /// A Brain job's (BUILD-PLAN C5): reading and searching files only, with the lean start's
    /// settings and a system prompt with no per-session sections, so the jobs of a batch share
    /// one cached start. Codex: as `Default` (it reads through its shell).
    Reader,
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
    /// A one-shot run that nothing ever resumes, forks or continues (a Brain pass, a commit
    /// message, a handoff note): a new or forked Codex thread is started ephemeral, never saved,
    /// so it stays out of the Codex app's lists and leaves nothing to clean up. Such a thread
    /// can't be resumed once its process ends. Claude ignores it.
    pub ephemeral: bool,
    pub access: Access,
    /// Appended to the CLI's own system prompt.
    pub append_system_prompt: Option<String>,
    /// MCP servers for the session. Only these are loaded.
    pub mcp_servers: Vec<McpServer>,
    pub tools: ToolSet,
    /// Folders besides `cwd` the session works in (a thread's workspace): Claude gets each as
    /// `--add-dir`. They are writable where the working directory is ([`Self::access_with_dirs`]).
    pub add_dirs: Vec<PathBuf>,
    /// Brigadier-owned skill plugins: each folder holds `skills/<name>/SKILL.md` (and a
    /// `.claude-plugin/plugin.json`). Claude loads each with `--plugin-dir`; Codex adds each
    /// one's `skills` folder as an extra skills root. The user's own skills are unaffected.
    pub skill_plugins: Vec<PathBuf>,
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
    /// A folder that is this session's alone (its own scratch folder or worktree, never a
    /// folder shared with other sessions): whatever still runs inside it when the CLI goes is
    /// ended too. `None`: only the CLI's own tree is ended.
    pub sweep_dir: Option<PathBuf>,
    /// The CLI may compact its context on its own as it fills up. Off for orchestrators,
    /// which are reborn instead (PLAN.md §2): Claude's auto-compact is switched off; Codex's
    /// cannot be, so its limit is raised as far as Codex allows (90% of the window).
    pub auto_compact: bool,
    /// The models the session may hand work to through the CLI's own sub-agents; `None`: no
    /// limit (raw sessions, orchestrators, Chats). Claude allows exactly these, or runs without
    /// its Agent tool when it can't; Codex runs without sub-agents.
    pub allowed_models: Option<AllowedModels>,
    /// A reviewer settles requests to go beyond a sandboxed session's access (Approve for
    /// me): Claude's own auto mode; for Codex, whose requests always reach Brigadier, the
    /// caller's reviewer (Codex only gets the feature that lets a command ask for more). What
    /// the reviewer doesn't allow is declined to the model, which works around it or reports
    /// it. Otherwise such requests reach Brigadier for the user (Ask for approval). No effect under [`Access::Full`],
    /// which never asks, or [`Access::ReadOnly`].
    pub auto_review: bool,
    /// The CLI leaves Co-authored-by trailers that name an AI out of the commits it writes
    /// (the user's setting, for sessions that commit): Claude through its attribution
    /// settings; Codex, which has none, is told in its instructions by the caller.
    pub omit_ai_coauthors: bool,
    /// A Claude thread's output hook; Codex runs no hooks.
    pub output_hook: Option<OutputHook>,
    /// A Claude worker's `Stop` hook: the command answers whether its turn may end (it
    /// refuses a turn that ends with no report, BUILD-PLAN E2). Codex runs no hooks.
    pub stop_hook: Option<OutputHook>,
    /// Every tool call is refused before it runs, with this reason for the model: a Claude
    /// `PreToolUse` hook that denies all tools, native and MCP. Hooks run in the client, so the
    /// tool list (and the cached prefix it starts) stays the same; `--disallowedTools` would
    /// change it (BUILD-PLAN B5, a thread's note fork). Codex runs no hooks: it ignores this.
    pub deny_tools: Option<String>,
    /// The CLI predicts the user's next message after each turn, with its own small request
    /// ([`ProviderEvent::PromptSuggestion`]): a conversation the user types into, never a
    /// worker or a one-shot run. [`ProviderSession::pause_suggestions`] holds them while its
    /// composer is off screen. A CLI that has none ignores it.
    ///
    /// [`ProviderSession::pause_suggestions`]: crate::ProviderSession::pause_suggestions
    pub suggestions: bool,
    /// What the session may never write (other sessions' branches, the base, Brigadier's
    /// bookkeeping; PLAN bug 6). Below Full access the CLI's sandbox grants none of it
    /// already, and Claude also gets it as `denyWrite` and file-tool deny rules (its file
    /// tools work outside the OS sandbox); under [`Access::Full`] the CLI runs inside
    /// Brigadier's allow-all-but-these profile. Empty: nothing is added.
    pub protection: brigadier_sandbox::Protection,
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

/// The rule every approver of a session's requests to leave its sandbox gets (Claude's auto
/// mode, Brigadier's own reviewer, which settles a Codex session's): a command run outside the sandbox
/// reaches what [`SessionSpec::protection`] keeps from it, so the approver must refuse it.
/// `None` when nothing is protected.
#[must_use]
pub fn approver_rule(protection: &brigadier_sandbox::Protection) -> Option<String> {
    if protection.deny.is_empty() && protection.deny_patterns.is_empty() {
        return None;
    }
    let quoted = |paths: &[PathBuf]| {
        paths
            .iter()
            .map(|path| format!("`{}`", path.display()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut rule = String::from(
        "Never allow a command, even one the user asked for (a cleanup included), that deletes, \
         moves, empties, resets or writes another session's work or Brigadier's own, however it \
         is written (`rm`, `mv`, `find -delete`, `git worktree remove`, `git worktree prune`, \
         `git branch -D`, `git update-ref`, `git reset`, `git push --delete`, a script): ",
    );
    if !protection.deny.is_empty() {
        rule.push_str(&format!(
            "these paths and everything under them: {}",
            quoted(&protection.deny)
        ));
    }
    if !protection.deny_patterns.is_empty() {
        if !protection.deny.is_empty() {
            rule.push_str("; ");
        }
        rule.push_str(&format!(
            "the paths these regular expressions match: {}",
            protection
                .deny_patterns
                .iter()
                .map(|pattern| format!("`{pattern}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    rule.push_str(
        "; and every checkout (worktree) of a repository whose git folder is under one of those \
         paths, its files included.",
    );
    if !protection.allow.is_empty() {
        rule.push_str(&format!(
            " Only these inside them are the session's own: {}.",
            quoted(&protection.allow)
        ));
    }
    rule.push_str(" Brigadier removes the others itself once their sessions are done.");
    Some(rule)
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
    ClaudeSession {
        session_id: String,
        /// The config directory of the account it ran on; absent: the user's own.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        home: Option<String>,
        /// The folder it ran in, so its files are known as Brigadier's for good (absent in
        /// records older than this field).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        cwd: Option<String>,
    },
    /// A Claude Code project directory (`projects/<encoded cwd>`) that did not exist before.
    ClaudeProjectDir { path: String },
    /// Claude Code's write-staging directory in the working directory (`.claude/.cc-writes`,
    /// or `.claude` itself when that did not exist). Removed only while it holds no files.
    ClaudeStagingDir { path: String },
    /// A Codex thread: its rollout file and state records, removed through `thread/delete`.
    CodexThread {
        thread_id: String,
        /// The `CODEX_HOME` of the account it ran on; absent: the user's own.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        home: Option<String>,
        /// The folder it ran in (absent in records older than this field).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        cwd: Option<String>,
    },
    /// The folder where Codex saves a thread's generated images
    /// (`$CODEX_HOME/generated_images/<thread id>`), which deleting the thread leaves behind.
    CodexGeneratedImages { path: String },
    /// A project trust entry Codex persisted in the user's `config.toml` when a thread started
    /// there. Removed through Codex's config API, only while it is still just `trusted`.
    CodexProjectTrust { path: String },
    /// A file or folder Free up space proved to be Brigadier's leftover and the user approved
    /// removing: exactly that entry (bound under `root` with the identity it had when it was
    /// shown), never anything found again by name. Removed only while it is still that entry.
    Adopted {
        root: String,
        path: String,
        /// Its identity when it was shown (device and inode, or volume and file index).
        identity: (u64, u64),
        /// Why it is Brigadier's, as shown.
        evidence: String,
        /// A folder removed only once empty (a CLI project folder whose files went with it).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        empty_only: bool,
    },
    /// Every process working inside `dir`, a folder Brigadier created (a worktree, a scratch
    /// folder): what a worker started there, including processes that left its tree.
    ProcessesIn { dir: String },
    /// A git worktree Brigadier created (for a task or a session) in its data directory.
    Worktree { repo: String, path: String },
    /// A folder Brigadier created in its data directory (a worker's scratch folder, the
    /// orchestrator's or a Chat's working folder).
    ScratchDir { path: String },
    /// Preview data, recorded before creation. Only a confirmed device and inode grant
    /// permission to reuse or remove it; an interrupted claim has no identity.
    PreviewDataDir {
        path: String,
        identity: Option<(u64, u64)>,
    },
    /// A short temp folder of a Claude session's own (`/tmp/brigadier-<id>`), for Claude's
    /// temp files and its sandboxed commands' TMPDIR.
    ClaudeTempDir { path: String },
    /// Folder trust Brigadier wrote in a CLI's settings `file` for `folder` (the user trusted
    /// it in Brigadier), recorded before the write with what it replaced: removed by putting
    /// that back, only while the folder is still trusted as Brigadier left it
    /// ([`crate::trust::undo`]).
    CliTrust {
        cli: crate::trust::TrustCli,
        file: String,
        folder: String,
        before: crate::trust::TrustBefore,
    },
}

#[cfg(test)]
mod input_tests {
    use super::*;

    #[test]
    fn an_approver_rule_names_every_protected_path_and_the_session_s_own() {
        assert_eq!(
            approver_rule(&brigadier_sandbox::Protection::default()),
            None
        );
        let protection = brigadier_sandbox::Protection {
            deny: vec![
                PathBuf::from("/repo/.git/refs/heads/brigadier"),
                PathBuf::from("/repo/.git/worktrees"),
            ],
            deny_patterns: vec!["^/tmp/brigadier-[0-9a-f]{12}(/|$)".into()],
            allow: vec![PathBuf::from("/repo/.git/worktrees/session-aa")],
            keep: Vec::new(),
        };
        let rule = approver_rule(&protection).expect("rule");
        for part in [
            "`/repo/.git/refs/heads/brigadier`",
            "`/repo/.git/worktrees`",
            "`^/tmp/brigadier-[0-9a-f]{12}(/|$)`",
            "own: `/repo/.git/worktrees/session-aa`",
            "git worktree remove",
            "even one the user asked for",
        ] {
            assert!(rule.contains(part), "{part} in {rule}");
        }
    }

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

    /// Lines in and out for each shape a CLI gives a change in: a unified diff with its header
    /// and hunks (Codex's update), a new or deleted file's bare content (Codex) or marked lines
    /// (Claude's `Write`), an edit's marked lines with `@@` between edits (Claude's `MultiEdit`).
    #[test]
    fn a_diff_counts_its_lines_in_and_out_in_every_shape() {
        let unified = "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,3 +1,4 @@\n fn a() {\n-    1\n+    2\n+    3\n }\n";
        assert_eq!(diff_counts(FileChangeKind::Update, unified), (2, 1));
        assert_eq!(diff_counts(FileChangeKind::Add, "one\ntwo\n"), (2, 0));
        assert_eq!(diff_counts(FileChangeKind::Add, "+one\n+two"), (2, 0));
        assert_eq!(diff_counts(FileChangeKind::Delete, "gone\n"), (0, 1));
        assert_eq!(
            diff_counts(FileChangeKind::Update, "-x\n+y\n@@\n+z\n"),
            (2, 1)
        );
        // A removed line that reads like a header is still a line out.
        assert_eq!(
            diff_counts(FileChangeKind::Update, "--- old\n+new\n"),
            (1, 1)
        );
        // Counted before the clip, which keeps only the start.
        let long = "+line\n".repeat(5_000);
        let change = FileChange::counted("f".into(), FileChangeKind::Add, Some(&long), 1024);
        assert_eq!((change.added, change.removed), (Some(5_000), Some(0)));
        assert!(change.diff.unwrap().len() < 1100);
        let none = FileChange::counted("f".into(), FileChangeKind::Update, None, 1024);
        assert_eq!((none.diff, none.added, none.removed), (None, None, None));
    }
}
