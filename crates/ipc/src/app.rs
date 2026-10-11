//! Types shared by the desktop shell (Tauri) and its webview. The webview never talks to the
//! daemon directly: the shell is the only IPC client and forwards to the UI over a channel.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::metrics::{BudgetId, DaemonMetrics, LatencySummary};
use crate::protocol::{DaemonInfo, DictationUpdate, EventEnvelope, TerminalOutput};

/// Pushed from the shell to the webview.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BridgeEvent {
    /// Connected (or reconnected) to the daemon.
    Connected {
        daemon: DaemonInfo,
        last_seq: i64,
    },
    /// Lost the daemon; the shell is reconnecting (relaunching it if needed).
    Disconnected {
        reason: String,
    },
    Event {
        event: EventEnvelope,
    },
    /// Events were missed; reload state from the daemon.
    Lagged {
        resume_after: i64,
    },
    Metrics {
        metrics: DaemonMetrics,
    },
    /// Output of a terminal the app opened.
    Terminal {
        output: TerminalOutput,
    },
    /// How a dictation or the speech model's download goes.
    Dictation {
        update: DictationUpdate,
    },
    /// The window was hidden or shown; pause sampling while hidden.
    WindowVisibility {
        visible: bool,
    },
    /// A conversation picked in the menu-bar item's "Running" list: open it.
    OpenConversation {
        conversation_id: String,
    },
    /// Folders were opened with the app (Finder or the Dock, a command-line argument): take
    /// them with `take_opened_folders` and add them as projects.
    FoldersOpened,
    /// File › Open Folder… was chosen: pick folders to add as projects.
    OpenFolderMenu,
    /// Brigadier › Uninstall Brigadier… was chosen.
    UninstallMenu,
    /// Brigadier › Settings… (⌘,) was chosen: open Settings, or leave it when it is open.
    SettingsMenu,
}

/// A conversation the menu-bar item lists under "Running".
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RunningChat {
    pub id: String,
    pub title: String,
}

/// What the side panel's Browser tab hears about its page, over the channel it opened it with.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BrowserEvent {
    /// A page started (`loading`) or finished loading.
    Load { url: String, loading: bool },
    /// The page's title changed.
    Title { title: String },
    /// A small PNG for the page's tab icon, safe under the app's image CSP.
    Favicon { url: String, data_url: String },
    /// A navigation the tab refused (not a web page); the tab offers the system browser
    /// instead.
    Blocked { url: String },
    /// A download finished (`ok`) or failed, saved at `path`.
    Downloaded { path: String, ok: bool },
}

/// The user's Settings › Browser choices the shell acts on itself, sent at start and on every
/// change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrowserOptions {
    /// The built-in browser exists. Off, the shell makes no page and drops those it has.
    pub enabled: bool,
    /// Where downloads go; empty: the system's Downloads folder.
    pub downloads_dir: String,
    /// Ask where to save each download.
    pub ask_downloads: bool,
}

/// Which pages Settings › Browser › Browsing history lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum HistoryFilter {
    All,
    /// Pages in tabs opened from something an agent gave: a link in its reply, a preview.
    Agent,
    /// Everything else: pages the user opened, and imported history.
    Other,
}

/// A page the built-in browser showed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrowserVisit {
    pub id: i64,
    pub url: String,
    /// Empty when the page had none.
    pub title: String,
    /// When, in ms since the Unix epoch.
    pub at_ms: i64,
    pub agent: bool,
}

/// How a download of the built-in browser went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum DownloadState {
    Running,
    Done,
    Failed,
}

/// A file the built-in browser downloaded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrowserDownload {
    pub id: i64,
    pub url: String,
    /// Where it was saved.
    pub path: String,
    /// When it started, in ms since the Unix epoch.
    pub at_ms: i64,
    pub state: DownloadState,
}

/// What a site may use without asking each time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SitePermission {
    /// The page's request shows the system's own question (the default).
    Ask,
    Allow,
    Block,
}

/// A site that asked the built-in browser for the camera or microphone, and the user's choices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSite {
    /// `scheme://host[:port]`.
    pub origin: String,
    pub camera: SitePermission,
    pub microphone: SitePermission,
    /// When it last asked, in ms since the Unix epoch.
    pub asked_ms: i64,
}

/// What Settings › Browser's Clear browsing data removes, and from when on.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrowserClear {
    /// Only what changed since then (ms since the Unix epoch); none: everything.
    pub since_ms: Option<i64>,
    pub history: bool,
    /// Cookies and every other kind of site data (storage, databases, service workers).
    pub cookies: bool,
    pub cache: bool,
    pub downloads: bool,
    /// Camera and microphone choices per site.
    pub sites: bool,
}

/// Where the Browser tab's page sits in the window, in CSS pixels from its top left.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrowserBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Static facts about this app launch.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub platform: String,
    /// When the OS started this process, in ms since the Unix epoch.
    pub process_start_ms: f64,
    /// Running the launch smoke check (`--smoke`).
    pub smoke: bool,
    /// The data folder was new: no model lists were saved yet, so startup waits for the agent
    /// CLIs to report theirs. Judged against the first-launch budget instead of cold start.
    pub first_launch: bool,
    /// Multiplier applied to timing budgets (1 locally, higher on shared CI runners).
    pub budget_tolerance: f64,
}

/// What the webview measured during the smoke check.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UiMeasurements {
    pub idle_rss_bytes: u64,
    pub ingest_to_paint: LatencySummary,
    /// Gaps between animation frames while the burst was painting.
    pub frame_gaps: LatencySummary,
    /// Where the longest frame gap fell and the costliest event flush, to explain a stall.
    pub stall_context: String,
    /// Startup milestones in ms since process start (webview, script, connected, catalog, paint).
    pub startup: String,
    pub probes_expected: u32,
    pub probes_painted: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum CheckStatus {
    Pass,
    Fail,
    /// The budget covers a feature from a later phase.
    NotApplicable,
}

/// One budget evaluated by the smoke check.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BudgetCheck {
    pub id: BudgetId,
    pub metric: String,
    pub target: String,
    pub measured: Option<f64>,
    /// The limit actually applied, tolerance included.
    pub limit: Option<f64>,
    pub status: CheckStatus,
    pub note: String,
}

/// The smoke check's verdict.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SmokeReport {
    pub passed: bool,
    pub platform: String,
    pub budget_tolerance: f64,
    pub cold_start_ms: f64,
    pub checks: Vec<BudgetCheck>,
}
