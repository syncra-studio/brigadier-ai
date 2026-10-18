//! Types shared by the desktop shell (Tauri) and its webview. The webview never talks to the
//! daemon directly: the shell is the only IPC client and forwards to the UI over a channel.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use brigadier_core::storage::CleanProgress;

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
    /// A page changed on disk (no `revision`: it was deleted). An editor showing an older
    /// revision reads it again.
    PageChanged {
        id: String,
        revision: Option<String>,
    },
    /// How a clean under way goes (Settings › Storage).
    StorageClean {
        progress: CleanProgress,
    },
    /// The checkout the right sidebar watches changed: read it again.
    WorkspaceChanged {
        target: brigadier_core::WorkspaceTarget,
    },
    /// What the right sidebar's search with `id` found next, or that it ended.
    Search {
        id: u32,
        update: brigadier_core::SearchUpdate,
    },
    /// Computer use changed: permissions, setup or the agents using apps.
    ComputerUse {
        status: brigadier_core::ComputerUseStatus,
    },
    /// An agent's call of a Browser tab: run it and answer with `browserAnswer`.
    BrowserCall {
        call: brigadier_core::BrowserCall,
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
    /// The computer-use menu-bar item's "Computer use settings…" was chosen.
    OpenComputerUseSettings,
    /// Brigadier › Settings… (⌘,) was chosen: open Settings, or leave it when it is open.
    SettingsMenu,
    /// Help › Send Feedback… was chosen.
    FeedbackMenu,
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
    /// Escape in the native page closes this tab's find bar.
    Escape,
    /// The shifted + key zooms the sender's page in, like the = menu accelerator.
    ZoomIn,
    /// The page opened a popup (`window.open`, `target=_blank`): its page waits, as tab `id`,
    /// for a tab to the right of this one; `url` is where it goes ("" or `about:blank` when the
    /// page writes it itself).
    Popup { id: String, url: String },
    /// The page closed itself (`window.close()` in a popup): its tab closes.
    Closed,
    /// The page asked for the camera or microphone, which its site's choices leave to the user:
    /// the tab asks, and answers `request` (`browser_permission`).
    Permission {
        request: u32,
        origin: String,
        camera: bool,
        microphone: bool,
    },
}

/// What a Browser tab's page shows, for the chat beside it: its address, its title, its text
/// and what the user selected (each cut to a length a chat can take).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
pub struct BrowserText {
    pub url: String,
    pub title: String,
    pub text: String,
    pub selection: String,
}

/// What finding text in a Browser tab's page came to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrowserFound {
    /// The page has the text: its match in turn is selected and scrolled to.
    pub found: bool,
    /// Which match that is, from 1; 0 when it can't be told.
    pub current: u32,
    /// How many matches the page shows, counted up to 1000; 0 when they can't be counted.
    pub total: u32,
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

/// Which kind of browser a profile belongs to, which decides what macOS asks before an import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ImportFamily {
    /// Chrome, Edge, Brave, Arc, Dia and the like: cookies need the browser's Keychain key.
    Chromium,
    /// Firefox, Zen and the like: nothing to ask.
    Gecko,
    /// Safari: needs Full Disk Access.
    Safari,
}

/// Where an installed browser stands for an import, which decides its row's status and button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ImportState {
    /// Nothing in the way; its profiles say how many cookies there are.
    Ready,
    /// The browser is open; it must be quit first.
    Running,
    /// macOS keeps its files from Brigadier until Brigadier has Full Disk Access.
    NeedsFullDiskAccess,
    /// The last import was refused the browser's cookie key in the Keychain.
    NeedsKeychainAccess,
    /// The browser's cookie key isn't in the Keychain at all.
    NoKeychainKey,
    /// It can't be imported from now for another reason.
    Unavailable,
}

/// One profile of a browser an import can copy from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ImportSourceProfile {
    pub id: String,
    /// The profile's name, e.g. "Person 1".
    pub name: String,
    /// How many cookies it keeps; absent when they couldn't be counted.
    pub cookies: Option<u32>,
    /// The browser's own default profile, listed first.
    pub is_default: bool,
}

/// What the last import from a browser brought, kept so its row can say so.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ImportRecord {
    /// When it finished, in Unix milliseconds.
    pub at_ms: i64,
    /// The names of the profiles it copied from.
    pub profiles: Vec<String>,
    /// Cookies now in Brigadier's browser.
    pub imported: u32,
    /// Cookies left out: expired, kept per site, unreadable or refused by the browser.
    pub skipped: u32,
    /// Up to 20 of the domains whose cookies were unreadable or refused.
    pub skipped_domains: Vec<String>,
    /// How many such domains there were in all.
    pub skipped_domain_count: u32,
    /// Pages of history added.
    pub history: u32,
}

/// An installed browser an import can copy from, as Settings › Import Browser Data lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ImportSource {
    /// The browser's key, e.g. "dia".
    pub key: String,
    /// Its name, e.g. "Google Chrome", or "Comet (Chromium)" for one found by its files.
    pub name: String,
    pub family: ImportFamily,
    /// Its app icon as a `data:image/png` URL; absent when it couldn't be read.
    pub icon: Option<String>,
    pub state: ImportState,
    /// Its profiles that keep cookies, the default first; empty unless it is ready or needs
    /// Keychain access.
    pub profiles: Vec<ImportSourceProfile>,
    /// The last import from it.
    pub record: Option<ImportRecord>,
}

/// How an import was refused or failed, which decides what the page shows next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ImportFailure {
    /// The browser is open.
    Running,
    NeedsFullDiskAccess,
    /// macOS didn't hand over the browser's cookie key.
    NeedsKeychainAccess,
    NoKeychainKey,
    /// The browser or a chosen profile is no longer there.
    Gone,
    /// Its cookies couldn't be read.
    ReadFailed,
}

/// How an import is going, sent while it runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ImportProgress {
    /// Reading the other browser's files (and, for cookies, waiting on macOS's question).
    Reading,
    /// Adding cookies to Brigadier's browser.
    Cookies { done: u32, total: u32 },
    /// Adding history.
    History,
}

/// How an import ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ImportOutcome {
    /// It ran; `record` is what it brought (also kept for the browser's row). `history_error`
    /// says why history couldn't be added when it was asked for and wasn't.
    Done {
        record: ImportRecord,
        history_error: Option<String>,
    },
    /// Nothing was imported, for `reason`; `message` says it in plain words.
    Failed {
        reason: ImportFailure,
        message: String,
    },
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
