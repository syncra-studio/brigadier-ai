//! What Brigadier keeps on disk and what it can clean up (Settings → Storage), what removing a
//! project takes with it, and what uninstalling removes: the types the app shows. The daemon
//! does every removal; the app only previews and confirms, and never sends a path back: it
//! picks items by the ids a scan or a preview gave them.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::{ConversationId, ConversationKind, ProjectId};

/// Disk use and cleanable items, as `scanStorage` answers it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct StorageReport {
    /// Names this scan; `cleanStorage` takes items of this scan only, for a while.
    pub scan_id: String,
    pub data_dir: String,
    /// Everything in the data directory.
    pub total_bytes: u64,
    /// What the items checked by default take.
    pub cleanable_bytes: u64,
    pub projects: Vec<ProjectUsage>,
    pub shared: Vec<SharedUsage>,
    pub items: Vec<CleanItem>,
}

/// What one project takes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProjectUsage {
    pub project_id: ProjectId,
    pub name: String,
    /// Its repository folder is there (a disconnected drive isn't).
    pub repo_found: bool,
    pub worktrees_bytes: u64,
    /// Its Brain and code index.
    pub brain_bytes: u64,
    /// Stored transcripts' attachments, reports and artifacts of its conversations.
    pub blobs_bytes: u64,
    /// Its conversations' and workers' working folders.
    pub scratch_bytes: u64,
}

/// What something shared by every project takes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SharedUsage {
    pub part: SharedPart,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SharedPart {
    Database,
    /// Stored content no project's conversations reference (Chats, the Personal Brain's, …).
    OtherBlobs,
    Models,
    Logs,
    Recordings,
    PersonalBrain,
    Other,
}

/// Something Brigadier created that it can remove.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CleanItem {
    pub id: String,
    pub category: CleanCategory,
    pub label: String,
    /// Where it is, for Reveal in Finder.
    pub path: Option<String>,
    pub bytes: u64,
    /// Why it can go (or, when it can't be picked, why not).
    pub reason: String,
    /// Checked by default: safe to remove.
    pub checked: bool,
    /// False: shown for information only; it can't be removed from here.
    pub selectable: bool,
    /// Removing it moves it to the Trash: its space comes back once the Trash is emptied.
    pub to_trash: bool,
    pub badges: Vec<CleanBadge>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum CleanCategory {
    Worktrees,
    Branches,
    SessionFiles,
    Brains,
    LogsAndData,
    Models,
    Processes,
    Database,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CleanBadge {
    /// Uncommitted changes, kept as a WIP commit on its branch before it goes.
    HasChanges,
    /// Commits its base doesn't have.
    NotMerged { ahead: u32 },
    /// Made by an older Brigadier, which didn't say which data directory it belongs to.
    Legacy,
}

/// What `cleanStorage` did.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CleanReport {
    /// Items that are gone.
    pub removed: u32,
    /// Disk space given back now.
    pub reclaimed_bytes: u64,
    /// Moved to the Trash: given back once the Trash is emptied.
    pub trashed_bytes: u64,
    pub failures: Vec<CleanFailure>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CleanFailure {
    pub label: String,
    pub path: Option<String>,
    pub error: String,
}

/// What removing a project takes with it, as `previewRemoveProject` answers it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRemoval {
    pub project_id: ProjectId,
    pub name: String,
    pub repo: Option<String>,
    pub conversations: Vec<RemovalConversation>,
    pub worktrees: Vec<RemovalWorktree>,
    pub branches: Vec<RemovalBranch>,
    /// Its Brain and code index.
    pub brain_bytes: u64,
    /// Why it can't be removed now (one of its conversations is working).
    pub blocked: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RemovalConversation {
    pub id: ConversationId,
    pub title: String,
    pub kind: ConversationKind,
    pub archived: bool,
    /// A turn or a worker is running.
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RemovalWorktree {
    pub path: String,
    pub bytes: u64,
    /// Uncommitted changes, kept as a WIP commit on its branch.
    pub has_changes: bool,
}

/// A branch Brigadier created, with how it stands against the branch its work goes to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RemovalBranch {
    pub repo: String,
    pub name: String,
    /// The branch its work lands on (or the session's base).
    pub target: String,
    pub tip: String,
    /// Everything on it is in `target`.
    pub merged: bool,
    /// Commits `target` doesn't have.
    pub ahead: u32,
    /// Checked out somewhere: never deleted.
    pub checked_out: bool,
    /// Deletes it by hand.
    pub command: String,
}

/// A branch Brigadier created whose work may not have landed, about to be deleted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UnlandedBranch {
    pub repo: String,
    pub name: String,
    /// Its standing couldn't be told (the branch its work goes to is gone, or git failed):
    /// it may hold work that never landed.
    pub unknown: bool,
}

/// A branch the user chose to delete, at the tip they saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BranchChoice {
    /// The repository it is in, as the preview listed it.
    pub repo: String,
    pub name: String,
    pub tip: String,
}

/// What `removeProject` did.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RemoveProjectReport {
    /// Branches left in place, as they stand now (Storage offers them later).
    pub kept_branches: Vec<RemovalBranch>,
    /// What its Brain took, now in the Trash (0 when it was kept or there was none).
    pub brain_trashed_bytes: u64,
    pub failures: Vec<String>,
}

/// The running app, as it asks for an uninstall.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UninstallApp {
    /// Its bundle identifier (`ai.brigadier.app`, or a development one).
    pub identifier: String,
    /// The app bundle, when it runs from one.
    pub bundle_path: Option<String>,
    pub pid: u32,
}

/// What uninstalling removes, as `previewUninstall` answers it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UninstallPlan {
    pub plan_id: String,
    pub data_dir: String,
    pub data_bytes: u64,
    /// What is running now and will be stopped.
    pub running: Vec<String>,
    /// Everything that goes, apart from the data directory and branches.
    pub items: Vec<UninstallItem>,
    /// Brigadier's branches in the user's repositories. Merged ones are deleted; others only
    /// when picked.
    pub branches: Vec<RemovalBranch>,
    /// The keep-awake-with-the-lid-closed rule is installed (removing it asks for the
    /// administrator password once).
    pub sudoers_rule: bool,
    /// The microphone permission is reset.
    pub microphone: bool,
    /// The app itself: moved to the Trash once it quit, or why it can't be here.
    pub app: AppRemoval,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UninstallItem {
    pub label: String,
    pub path: Option<String>,
    pub bytes: u64,
    pub to_trash: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AppRemoval {
    /// Moved to the Trash once the app quit.
    Bundle { path: String, bytes: u64 },
    /// Not running from an app bundle (a development build): nothing to move.
    NoBundle,
    /// This platform's installer owns it.
    Unsupported { how: String },
}

/// What `uninstall` did, and what happens once the app quits.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UninstallReport {
    pub steps: Vec<UninstallStep>,
    /// Branches left in place, each with the command that deletes it.
    pub kept_branches: Vec<RemovalBranch>,
    /// What goes once Brigadier quit (the data directory, per-app folders, the app).
    pub after_quit: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UninstallStep {
    pub label: String,
    pub ok: bool,
    pub detail: Option<String>,
}
