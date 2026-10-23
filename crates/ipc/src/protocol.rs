//! The wire protocol between the app and `brigadierd`.
//!
//! Frames are length-prefixed JSON (see [`crate::frame`]). The first client frame must be a
//! [`ClientFrame::Hello`] carrying the per-launch token, or the grant-scoped frame CLI sessions
//! use ([`ClientFrame::Hook`], [`ClientFrame::Cli`]); anything else closes the connection.
//! Requests carry a client-chosen id echoed on the response. Responses reuse the request's
//! `method` tag, so TypeScript can pair them with `Extract<Response, { method: M }>`.

use brigadier_brain::{BrainAnswer, BrainGraph, BrainQuery, NodeFilter};
use brigadier_core::pages::{Page, PageContent, PageError, PageSummary, PageWriteOutcome};
use brigadier_core::storage::{
    BranchChoice, CleanProgress, CleanReport, FreeUpProjectsReport, ProjectRemoval,
    RemoveProjectReport, StorageReport, UninstallApp, UninstallPlan, UninstallReport,
    UnlandedBranch,
};
use brigadier_core::{
    AccountEntry, AccountsView, AppAnswer, AppCall, AttachmentRef, BrainOverview, CardId, Catalog,
    CheckoutFile, CommitOutcome, ComputerPermission, ComputerUseStatus, ConflictFile, ConnectView,
    ConventionsExport, Conversation, ConversationActivity, ConversationId, ConversationKind,
    ConversationStatus, ConversationView, DiffStat, FeedbackFiled, FeedbackFiling, FeedbackWriteUp,
    FolderCheck, FolderListing, FolderTrustReport, ForkPlace, GitHubSignIn, GitState, GuidanceRow,
    Mention, MergeUndo, Message, MessageMatches, MessagePage, MessageQueue, ModelRule, ModelRules,
    OrchestratorPage, PendingChanges, ProbeBurst, Project, ProjectCandidate, ProjectId,
    ProjectPatch, ProvidersView, PullRequest, QueuedMessage, Rating, RawApprovals, RawPage,
    RawSession, RawSessionId, ReplaceFile, ReplaceOutcome, RepoInfo, RepoSuggestion,
    RestoreOutcome, ReviewDiff, ReviewScope, SearchQuery, SearchUpdate, Settings, SettledTab,
    Setup, SetupRequest, SourceAction, SourceBranch, SourceChange, SourceGitRun, SourceScope,
    SourceState, StoredCopy, TaskId, ThreadMetrics, TurnRevert, UndoOutcome, UpdateTarget,
    UpdatesView, UsageActivity, UsageView, WorkerDiff, WorkerPage, WorkspaceFolder,
    WorkspaceTarget, WorkspaceTree,
};
use brigadier_providers::{Access, ApprovalDecision, ProviderKind};
use brigadier_router::RegistryInfo;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use ts_rs::TS;

use crate::metrics::{DaemonMetrics, Diagnostics};

/// Bumped on any incompatible change to these types.
pub const PROTOCOL_VERSION: u32 = 17;

/// What a development build's injected limit applies to.
#[cfg(debug_assertions)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FaultTarget {
    /// A task's worker.
    Task { task_id: TaskId },
    /// A session's orchestrator or a Chat's model.
    Conversation { conversation_id: ConversationId },
}

/// An error a development build can make a provider report.
#[cfg(debug_assertions)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum InjectedError {
    /// "Prompt is too long".
    ContextWindow,
    /// A failed sign-in.
    Auth,
    /// Credits run out.
    Billing,
}

/// What an archive, restore or delete did to one of its conversations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleOutcome {
    pub id: ConversationId,
    /// The conversation as it is now (an archive's or a restore's).
    pub conversation: Option<Box<Conversation>>,
    /// Why it failed, in plain words.
    pub error: Option<String>,
}

/// Who is connecting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub name: String,
    pub pid: u32,
}

/// Frames sent by a client.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ClientFrame {
    Hello {
        token: String,
        protocol: u32,
        client: ClientInfo,
    },
    Request {
        id: u32,
        request: Request,
    },
    /// First frame of a Claude thread's output hook (`brigadierd hook post-tool-use`): the
    /// hook's grant and the output that follows the frame, `output.bytes` raw bytes. The daemon
    /// answers with one [`HookReply`] frame; a refused grant just closes the connection.
    Hook {
        grant: String,
        output: HookOutput,
    },
    /// First frame of a `brigadier` command run by a session: the session's id and grant. One
    /// length-prefixed `brigadier_cli::protocol::CliCall` follows, answered by one `CliReply`;
    /// a grant that isn't the session's just closes the connection.
    Cli {
        session: String,
        grant: String,
    },
}

/// What a thread's output hook sends after its [`ClientFrame::Hook`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HookOutput {
    /// A failing call's excerpt (the most the CLI gives a failure's hook), not a whole output.
    pub excerpt: bool,
    /// How the command ended ("exit 0", "exit 1", or the CLI's interpretation of a benign
    /// non-zero exit).
    pub status: String,
    pub bytes: u64,
}

/// The daemon's answer to a thread's output hook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HookReply {
    /// What the model gets instead of the output (its digest); none: the output as it is.
    pub replacement: Option<String>,
}

/// Commands and queries.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(
    tag = "method",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Request {
    GetCatalog,
    /// Which conversations run and which wait for the user (the sidebar's spinner and pills).
    GetActivity,
    /// The open tabs that are ready or merged (their tab and workspace row states).
    GetSettledTabs,
    /// What became of a session's merge on its base since it merged: when it was reverted,
    /// and redone after that (the Changes card's "Undone on main · ‹time›"), and whether it is
    /// pushed.
    GetMergeUndo {
        conversation_id: ConversationId,
        merge_commit: String,
    },
    /// Creates a project. With `repo` (a repository's top-level folder, from the native
    /// picker or typed), an empty `name` names it after the folder.
    CreateProject {
        name: String,
        repo: Option<String>,
    },
    UpdateProject {
        id: ProjectId,
        patch: ProjectPatch,
    },
    /// The user's answer to "Do you trust this folder?" for the project's repository folder
    /// `path` (its first one when absent).
    SetFolderTrust {
        id: ProjectId,
        path: Option<String>,
        trusted: bool,
    },
    /// A project's Brain at a glance.
    GetBrain {
        project_id: ProjectId,
    },
    /// Runs a Brain query as the orchestrator's `query_brain` does (the Inspector's search).
    QueryBrain {
        project_id: ProjectId,
        query: BrainQuery,
    },
    /// Nodes and their edges, for the Inspector's graph viewer.
    GetBrainGraph {
        project_id: ProjectId,
        filter: NodeFilter,
    },
    /// Writes the project's conventions into the marked Brigadier section of an AGENTS.md
    /// (the native save panel's choice, or a typed path), creating the file if needed.
    ExportConventions {
        project_id: ProjectId,
        path: String,
    },
    /// Rebuilds a project's code index from scratch.
    RebuildIndex {
        project_id: ProjectId,
    },
    /// Branches and state of a repository, for the composer's branch picker.
    GetRepoInfo {
        path: String,
    },
    /// The repositories the user's own Claude Code and Codex sessions worked in, most recent
    /// first: the first run's project suggestions.
    FindProjects,
    /// The git repositories on this computer, most recent first: the ones the user's own CLI
    /// sessions worked in, the projects', and a quick look through the usual code folders.
    /// The Add project dialog's list.
    FindRepos,
    /// The folders a typed path points into (`~` is the home folder): the Add project
    /// dialog's suggestions.
    BrowseFolders {
        path: String,
    },
    /// What adding a folder (typed, picked, dropped or opened from the file manager) as a
    /// project does.
    CheckFolder {
        path: String,
    },
    /// Adds a folder as a project: the top-level folder of the repository it is in, or with
    /// `init`, a new repository made there (and the folder, when missing). An empty `name`
    /// names it after the folder.
    AddProject {
        path: String,
        name: String,
        init: bool,
    },
    /// Clones `url` into the new folder `folder` in `parent` and adds it as a project. The
    /// answer comes when the clone ends; the connection serves other requests meanwhile.
    CloneProject {
        url: String,
        parent: String,
        folder: String,
        name: String,
    },
    /// The files of a session's checkout, for the composer's @-mentions and the Files tab.
    ListFiles {
        conversation_id: ConversationId,
        /// Only the files whose path has these letters in order (ignoring case), searched in
        /// the whole checkout: how the Files tab finds files past the listed ones.
        #[serde(default)]
        query: Option<String>,
    },
    /// The files of a project's folder, for the Files pane of a session not started yet.
    ListProjectFiles {
        project_id: ProjectId,
        /// As [`Request::ListFiles`]'s.
        #[serde(default)]
        query: Option<String>,
    },
    /// A checkout's files and what it ignores, for the right sidebar's tree.
    GetWorkspaceTree {
        target: WorkspaceTarget,
    },
    /// One folder of a checkout read from disk: an ignored folder opened in the tree.
    ListWorkspaceFolder {
        target: WorkspaceTarget,
        /// Relative to the checkout's root.
        path: String,
    },
    /// Watches a checkout for this connection, in place of the one it watched before (none
    /// stops watching): `WorkspaceChanged` frames say when to read it again.
    WatchWorkspace {
        target: Option<WorkspaceTarget>,
    },
    /// Searches a checkout for this connection, stopping the search it ran before: `Search`
    /// frames with the returned id bring the results.
    StartSearch {
        target: WorkspaceTarget,
        query: SearchQuery,
    },
    /// Stops this connection's search, if it is still the one with `id`.
    CancelSearch {
        id: u32,
    },
    /// Replaces a search's matches in the files listed, each only if unchanged since.
    ReplaceInFiles {
        target: WorkspaceTarget,
        query: SearchQuery,
        replacement: String,
        files: Vec<ReplaceFile>,
    },
    /// One file of a session's checkout, for the side panel's Files tab.
    ReadFile {
        conversation_id: ConversationId,
        /// Relative to the checkout's root.
        path: String,
    },
    /// A bottom-pane shell, started if none runs, sized `cols` × `rows`. A conversation's runs
    /// in its checkout, else its project's first repository, else the home folder; without a
    /// conversation it is Home's, in `project_id`'s first repository, else the home folder. Its
    /// output then streams to this connection as [`ServerFrame::Terminal`].
    OpenTerminal {
        #[serde(default)]
        #[ts(optional)]
        conversation_id: Option<ConversationId>,
        #[serde(default)]
        #[ts(optional)]
        project_id: Option<ProjectId>,
        /// Independent shell session; omitted for the original default shell.
        #[serde(default)]
        #[ts(optional)]
        session_id: Option<String>,
        /// Main tabs restore a fresh shell in their remembered folder.
        #[serde(default)]
        #[ts(optional)]
        cwd: Option<String>,
        /// A main tab's first open after app startup replaces any previous process.
        #[serde(default)]
        #[ts(optional)]
        fresh: Option<bool>,
        cols: u16,
        rows: u16,
    },
    /// Opens a side conversation. A supplied ID makes creation retryable and allows multiples.
    OpenSideChat {
        conversation_id: ConversationId,
        #[serde(default)]
        #[ts(optional)]
        side_chat_id: Option<ConversationId>,
    },
    /// Opens the chat of a session's tab (a page, file or browser tab), made under `chatId`
    /// if it has none yet.
    OpenTabChat {
        conversation_id: ConversationId,
        tab_id: String,
        target: brigadier_core::TabTarget,
        chat_id: ConversationId,
    },
    /// The tab chats the app's tabs still have: every other one goes, with all of it. Until
    /// this is sent after a launch, no tab chat runs.
    KeepTabChats {
        chat_ids: Vec<ConversationId>,
    },
    /// What a browser tab shows, for its chat's next turn (kept only in memory).
    SetTabPage {
        chat_id: ConversationId,
        url: String,
        title: String,
        text: String,
        selection: String,
    },
    /// Typed input for a terminal.
    WriteTerminal {
        terminal_id: String,
        data: String,
    },
    ResizeTerminal {
        terminal_id: String,
        cols: u16,
        rows: u16,
    },
    /// Forgets a terminal's saved output (⌘K), so reopening its tab doesn't replay it.
    ClearTerminal {
        terminal_id: String,
    },
    /// Ends a terminal's shell.
    CloseTerminal {
        terminal_id: String,
    },
    /// Computer use as Settings › Computer use shows it. Never starts the helper: permissions
    /// come from a helper already running, or from what it last wrote.
    GetComputerUse,
    /// Asks macOS for `permission`, from the helper (it is the app that holds the grant).
    /// Starts the helper if needed.
    GrantComputerPermission {
        permission: ComputerPermission,
    },
    /// Checks the permissions again, refreshing the helper only when macOS needs that to see a
    /// new Screen Recording grant.
    CheckComputerUse,
    /// Takes one test picture, to confirm screen capture works end to end; setup is finished
    /// once it does. macOS may ask the user about it the first time.
    ConfirmComputerCapture,
    /// Ends `session`'s control of `app`: its further actions there are refused, and the agent
    /// is told the user stopped it.
    StopComputerSession {
        session: String,
        bundle_id: Option<String>,
        app: String,
    },
    /// Whether dictation (the composer's Dictate button) can run, and its speech model.
    GetDictation,
    /// Downloads the speech model into the data folder. Answered at once; progress and the
    /// end come to this connection as [`ServerFrame::Dictation`].
    DownloadDictationModel,
    /// Stops the model's download; what came so far is kept to resume from.
    CancelDictationDownload,
    /// Starts a dictation: the speech model loads while the user speaks. Its audio follows
    /// with `appendDictation`, and its text comes to this connection as
    /// [`ServerFrame::Dictation`].
    StartDictation,
    /// The next piece of a dictation's audio.
    AppendDictation {
        dictation_id: String,
        /// 16 kHz mono 16-bit little-endian PCM, base64-encoded.
        audio: String,
    },
    /// The dictation's audio is complete: transcribe it. Answered at once.
    FinishDictation {
        dictation_id: String,
    },
    /// Drops a dictation and its audio.
    CancelDictation {
        dictation_id: String,
    },
    /// Rates an answer ("Good response" / "Bad response") or a plan ("Good plan" / "Bad plan");
    /// no rating takes it back ("Clear rating").
    RateMessage {
        conversation_id: ConversationId,
        /// A message id, or `task:<id>` for a worker's report; `plan:<id>` for a plan.
        subject: String,
        rating: Option<Rating>,
    },
    /// What a worktree session's branch changed against its base (the pinned summary card).
    GetSessionDiff {
        id: ConversationId,
    },
    /// What each worker at work on a change has changed in its worktree so far: its +N −N in
    /// the Workers summary while it works.
    GetWorkerDiffs {
        conversation_id: ConversationId,
    },
    /// Creates a session (with a project) or a chat (without). The setup comes from the
    /// composer; sessions need one to run, and their project remembers it.
    CreateConversation {
        kind: ConversationKind,
        project_id: Option<ProjectId>,
        title: Option<String>,
        setup: Option<SetupRequest>,
    },
    /// A new session of `from`'s project that starts on `text` in the background (⌘⏎, "Open
    /// as a new session", "Start as session"): on a new worktree from the branch `from` started
    /// from. `setup` is the composer's (its model, permission and plan mode); absent, `from`'s.
    StartSession {
        from: ConversationId,
        setup: Option<Box<SetupRequest>>,
        text: String,
        attachments: Vec<AttachmentRef>,
        mentions: Vec<Mention>,
    },
    /// "Fork chat from here": a new conversation with the thread up to the answer
    /// `message_id`; a session's fork works from the commit current then, in `place`.
    ForkConversation {
        conversation_id: ConversationId,
        message_id: String,
        place: ForkPlace,
    },
    /// Changes the model, effort or permission level (a session's repository and environment
    /// cannot change once set).
    UpdateSetup {
        id: ConversationId,
        setup: Setup,
    },
    /// Messages (newest `limit`), tasks, cards, queue and run state in one read.
    GetConversation {
        id: ConversationId,
        limit: u32,
    },
    /// Sends a message. While a turn runs it waits in the queue (when queueing is on), or with
    /// `steer` goes into the running turn now.
    SendMessage {
        conversation_id: ConversationId,
        text: String,
        attachments: Vec<AttachmentRef>,
        mentions: Vec<Mention>,
        steer: bool,
        /// Queue it at this slot whenever it would wait (a turn runs, an answer works, or the
        /// queue is paused): a queued message pulled into the composer to edit, or a deleted
        /// one restored, goes back where it was. Without `steer` only.
        #[serde(default)]
        queue_index: Option<u32>,
    },
    EditQueued {
        conversation_id: ConversationId,
        item_id: String,
        text: String,
        attachments: Vec<AttachmentRef>,
        mentions: Vec<Mention>,
    },
    DeleteQueued {
        conversation_id: ConversationId,
        item_id: String,
    },
    /// Moves a queued message to `index` (drag to reorder).
    MoveQueued {
        conversation_id: ConversationId,
        item_id: String,
        index: u32,
    },
    /// Sends a queued message into the running turn now.
    SteerQueued {
        conversation_id: ConversationId,
        item_id: String,
    },
    /// Resumes a queue paused by an interrupt.
    ResumeQueue {
        conversation_id: ConversationId,
    },
    /// The notice of a queued message the thread sent on to another tab: Undo (`undo`) takes
    /// it back and sends it here again; otherwise the notice goes.
    SettleMoved {
        conversation_id: ConversationId,
        item_id: String,
        undo: bool,
    },
    /// "Run here" on a queued message the thread offered as a new tab: it waits like any other
    /// and is sent once nothing works.
    RunQueuedHere {
        conversation_id: ConversationId,
        item_id: String,
    },
    /// Stops the running turn; the queue pauses.
    Interrupt {
        conversation_id: ConversationId,
    },
    /// Continues the latest request after it was stopped, in the same block; the queue
    /// unpauses and runs after it.
    Resume {
        conversation_id: ConversationId,
    },
    /// Stops a session's preview (`preview_id`), or every running one of it.
    StopPreview {
        conversation_id: ConversationId,
        preview_id: Option<String>,
    },
    PausePreview {
        conversation_id: ConversationId,
        preview_id: String,
    },
    ResumePreview {
        conversation_id: ConversationId,
        preview_id: String,
    },
    /// Start again on an ended preview: its own command from its record, same folder, same
    /// variables, its old port when free; the thread hears of it.
    RestartPreview {
        conversation_id: ConversationId,
        preview_id: String,
    },
    ClearPreviews {
        conversation_id: ConversationId,
    },
    /// The × on an ended preview's Background row (Q94): it leaves the rows.
    DismissBackground {
        conversation_id: ConversationId,
        id: String,
    },
    PreviewLog {
        conversation_id: ConversationId,
        preview_id: String,
        lines: u32,
    },
    /// Compacts a Chat's context now, in a turn of its own (a `/compact` command).
    Compact {
        conversation_id: ConversationId,
    },
    /// What `/status` shows: the model's CLI session and the usage left.
    GetConversationStatus {
        conversation_id: ConversationId,
    },
    /// Replaces a sent message: the new text starts a branch beside it and is answered. In a
    /// session only the latest message, while nothing from it has landed.
    EditMessage {
        conversation_id: ConversationId,
        message_id: String,
        text: String,
        /// When omitted, reuse the original refs and drop removed inline tokens.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        attachments: Option<Vec<AttachmentRef>>,
    },
    /// Answers a request again from its user message (same limits as editing).
    Regenerate {
        conversation_id: ConversationId,
        request_id: String,
    },
    /// Shows the branch of a Chat that ends at `head`.
    SwitchBranch {
        conversation_id: ConversationId,
        head: String,
    },
    /// Stores a file for a message. `data` is base64; at most 10 MB decoded. `pasted`: text
    /// pasted into the composer rather than a file (UTF-8 plain text).
    AddAttachment {
        name: String,
        mime: String,
        data: String,
        pasted: bool,
    },
    /// A stored attachment's bytes, for previews: base64.
    ReadAttachment {
        id: String,
    },
    /// Keeps a composer draft's attachments stored until it is sent or discarded; `scope` is a
    /// conversation id or `new`.
    PinDraftAttachments {
        scope: String,
        attachments: Vec<AttachmentRef>,
    },
    /// Preview the later files before confirming a turn snapshot restore.
    PrepareTurnRevert {
        conversation_id: ConversationId,
        request_id: String,
    },
    /// Restore the turn's start tree, only while the confirmed current tree still matches.
    RevertTurn {
        conversation_id: ConversationId,
        request_id: String,
        expected_tree: String,
    },
    /// The changes agents made in a session's checkout that the user hasn't kept or undone yet
    /// (the file review, a turn diff card's Undo All and Keep All): every file, or `path`'s.
    GetPendingChanges {
        conversation_id: ConversationId,
        #[serde(default)]
        #[ts(optional)]
        path: Option<String>,
    },
    /// Keeps pending changes: every file's, `path`'s, or some of its (`hunks`, by id). No file
    /// changes; a change the file no longer has is refused.
    KeepChanges {
        conversation_id: ConversationId,
        #[serde(default)]
        #[ts(optional)]
        path: Option<String>,
        #[serde(default)]
        #[ts(optional)]
        hunks: Option<Vec<String>>,
    },
    /// Undoes pending changes, picked as `KeepChanges` picks them: their lines go back in the
    /// checkout, uncommitted, or by a commit on the session's branch when they were committed.
    /// A file someone else changed too is left as it is.
    UndoPendingChanges {
        conversation_id: ConversationId,
        #[serde(default)]
        #[ts(optional)]
        path: Option<String>,
        #[serde(default)]
        #[ts(optional)]
        hunks: Option<Vec<String>>,
    },
    /// An image the thread looked at or made (its item's `itemId`), as base64 with its media
    /// type: only an image an item of this conversation's CLI thread names, or its proof.
    ReadThreadImage {
        conversation_id: ConversationId,
        item_id: String,
    },
    /// Stops one of the thread's CLI's background tasks (`task_id`, the CLI's own id), as the
    /// CLI's own Stop does.
    StopBackgroundTask {
        conversation_id: ConversationId,
        task_id: String,
    },
    /// What a background command of the thread's CLI printed so far: the end of its output
    /// file, up to 256 KiB.
    ReadBackgroundTaskOutput {
        conversation_id: ConversationId,
        task_id: String,
    },
    /// The thread as its CLI keeps it (S9, S10): its CLI session files' items, the live
    /// stream's after them, and its turns. `threadNews` frames extend it.
    ThreadView {
        conversation_id: ConversationId,
    },
    /// The user's Continue now on a turn that waits for a usage, spend or credits limit: its
    /// messages go now (MA5).
    ContinueWaiting {
        conversation_id: ConversationId,
    },
    /// The user's Don't resume on such a turn: nothing goes on its own (MA5).
    DontResume {
        conversation_id: ConversationId,
    },
    /// Whether the user's copy of the `/brigade` skill (`~/.brigadier/brigade/SKILL.md`)
    /// overrides the shipped one (S15).
    GetBrigadeSkill,
    /// Settings' Reset to shipped: the user's copy goes; new sessions get the shipped skill.
    ResetBrigadeSkill,
    /// Settings' Create: the user's copy, written as the shipped skill to edit; one already
    /// there is kept as it is.
    CreateBrigadeSkill,
    /// Settings › Routing: the user's rules file (`~/.brigadier/models.md`) as it is on disk
    /// now, beside the shipped guidance (S32).
    GetModelRules,
    /// Settings › Routing's save: the rules file's rows and rules become these, written in the
    /// file's own text format (S32).
    SetModelRules {
        rows: Vec<GuidanceRow>,
        rules: Vec<ModelRule>,
    },
    /// A worker's thread as its CLI keeps it (S14): its CLI session files' items and turns,
    /// read-only (G19.Q6).
    WorkerThread {
        conversation_id: ConversationId,
        task_id: TaskId,
    },
    /// An image a CLI session file of the conversation holds, as base64 with its media type.
    ReadSessionFileImage {
        conversation_id: ConversationId,
        image: brigadier_providers::session_file::FileImage,
    },
    /// A session's changes in the Review tab's scope: whole files unless `whole_files` is
    /// off, whitespace changes left out with `ignore_whitespace`.
    GetReviewDiff {
        conversation_id: ConversationId,
        scope: ReviewScope,
        whole_files: bool,
        ignore_whitespace: bool,
    },
    /// A session checkout's branch, changes and remote, for its Git actions.
    GetGitState {
        conversation_id: ConversationId,
    },
    /// The GitHub pull request of a session checkout's branch, looked up with `gh` (read only).
    GetPullRequest {
        conversation_id: ConversationId,
    },
    /// The user's commit of a session checkout's changes (every change with
    /// `include_unstaged`); a blank `message` is written for them. With `push`, then pushes.
    CommitChanges {
        conversation_id: ConversationId,
        message: Option<String>,
        include_unstaged: bool,
        push: bool,
        /// Rewrites the last commit instead (a blank message keeps its own).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        amend: Option<bool>,
    },
    /// The user's push of a session checkout's branch.
    PushChanges {
        conversation_id: ConversationId,
    },
    /// A session checkout's staged and unstaged files, for the Source panel.
    GetSourceState {
        conversation_id: ConversationId,
    },
    /// Stages the listed changes in a session checkout (every change, untracked files
    /// included, when absent).
    StageFiles {
        conversation_id: ConversationId,
        paths: Option<Vec<String>>,
    },
    /// Unstages the listed staged files in a session checkout (every one when absent).
    UnstageFiles {
        conversation_id: ConversationId,
        paths: Option<Vec<String>>,
    },
    /// Throws away the listed files of one side in a session checkout (the whole side when
    /// absent): unstaged changes go back to the index (untracked files are deleted), staged
    /// ones back to HEAD in both the index and the files.
    DiscardFiles {
        conversation_id: ConversationId,
        scope: SourceScope,
        paths: Option<Vec<String>>,
    },
    /// A project folder's staged and unstaged files, for the Source pane of a session not
    /// started yet.
    GetProjectSourceState {
        project_id: ProjectId,
    },
    /// Stages, unstages or discards changes in a project's folder, from the Source pane of a
    /// session not started yet that will work in that folder.
    ChangeProjectSource {
        project_id: ProjectId,
        change: SourceChange,
    },
    /// The user's commit in a project's folder, from the Source pane of a session not started
    /// yet that will work in that folder; as [`Request::CommitChanges`] otherwise.
    CommitProjectChanges {
        project_id: ProjectId,
        message: Option<String>,
        include_unstaged: bool,
        push: bool,
        /// Rewrites the last commit instead (a blank message keeps its own).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        amend: Option<bool>,
    },
    /// The branches a Source panel can merge from or switch to.
    ListSourceBranches {
        target: WorkspaceTarget,
    },
    /// One of a Source panel's git actions: merge, abort the merge, switch branch, pull,
    /// push, fetch, or write a conflicted file as the user resolved it.
    SourceAction {
        target: WorkspaceTarget,
        action: SourceAction,
    },
    /// A Source panel's Git Output: the git commands it ran on the checkout, oldest first.
    GetSourceGitOutput {
        target: WorkspaceTarget,
    },
    /// A conflicted file's versions, for its conflict and merge editor tabs.
    GetConflictFile {
        target: WorkspaceTarget,
        path: String,
    },
    /// A commit message for what is staged, written for the Source panel's box.
    WriteCommitMessage {
        target: WorkspaceTarget,
    },
    /// Answers an approval card.
    AnswerCard {
        conversation_id: ConversationId,
        card_id: CardId,
        decision: ApprovalDecision,
    },
    /// Answers a question card's round: one answer per question, the picked option's label or
    /// the user's own words.
    AnswerQuestion {
        conversation_id: ConversationId,
        card_id: CardId,
        answers: Vec<String>,
        /// How the user gave the answer, kept with it.
        #[serde(default)]
        #[ts(optional)]
        input: Option<brigadier_core::work::AnswerInput>,
    },
    /// Send feedback: the user's note as reports in Brigadier's issue forms, checked against
    /// its existing issues. Fails when no writer can run.
    WriteUpFeedback {
        text: String,
    },
    /// Send feedback: files the reports as the user with their `gh`; nothing is filed when it
    /// isn't signed in.
    FileFeedback {
        items: Vec<FeedbackFiling>,
    },
    /// Send feedback: who it posts as on GitHub.
    FeedbackAccount,
    /// Send feedback, signed out: starts signing `gh` in to GitHub (in place of one at work)
    /// and answers with its one-time code.
    StartGitHubSignIn,
    /// Send feedback: waits for the GitHub sign-in at work to end; fails, in plain words, when
    /// it didn't sign in.
    WaitGitHubSignIn,
    /// Send feedback: stops the GitHub sign-in at work, if any.
    CancelGitHubSignIn,
    /// The user's answer to the thread's feedback offer (`offer_feedback`): opened on GitHub
    /// (`sent`) or dismissed. Sent, what became of its reports; a Try again that files the
    /// rest answers again with them all.
    AnswerFeedbackOffer {
        conversation_id: ConversationId,
        offer_id: String,
        sent: bool,
        #[serde(default)]
        #[ts(optional, as = "Option<Vec<brigadier_core::FeedbackSent>>")]
        results: Vec<brigadier_core::FeedbackSent>,
    },
    /// The user clicked "Merge into main" on the side panel: the thread is told to merge the
    /// session branch into its base.
    MergeSession {
        conversation_id: ConversationId,
    },
    /// "Close workspace": removes the session's own worktree, its uncommitted changes kept as
    /// a commit on its branch first. The next change makes a fresh one in the same folder.
    CloseWorkspace {
        conversation_id: ConversationId,
    },
    /// The app ran an agent's app call (`appCall`): what came of it.
    AppAnswer {
        id: String,
        result: AppAnswer,
    },
    /// The user took an agent's Browser tab over (`paused`): its calls wait. Continue, or the
    /// tab closing, hands it back.
    BrowserPaused {
        conversation_id: ConversationId,
        tab: String,
        paused: bool,
    },
    /// The user's Stop for one worker: stops it for good (its unfinished changes are kept,
    /// see `Task.kept`), shows "Stopped by the user", and tells the thread at once.
    StopTask {
        task_id: TaskId,
    },
    /// Stop all: stops every worker of the conversation that runs or waits, as `stopTask`
    /// does; the thread shows a "Stopped" row for each, and the orchestrator hears one note
    /// naming them.
    StopWorkers {
        conversation_id: ConversationId,
    },
    /// Interrupts a worker's turn; `resumeTask` continues it.
    PauseTask {
        task_id: TaskId,
    },
    ResumeTask {
        task_id: TaskId,
    },
    /// Restores a task's kept patch (`KeptWork::Diff`) as a new branch on its target branch.
    RestoreKeptWork {
        task_id: TaskId,
    },
    /// A page of a worker's live transcript.
    ListWorkerEvents {
        task_id: TaskId,
        /// Only entries with a smaller `streamSeq` (for paging backwards).
        before: Option<i64>,
        limit: u32,
    },
    /// A page of the orchestrator log (Inspector): CLI events and context injections.
    ListOrchestratorLog {
        conversation_id: ConversationId,
        before: Option<i64>,
        limit: u32,
    },
    /// A session thread's metrics (Inspector): its own commits and its context per request.
    GetThreadMetrics {
        conversation_id: ConversationId,
    },
    /// Part of an artifact's text.
    ReadArtifact {
        id: String,
        offset: u64,
        limit: u32,
    },
    /// Writes an artifact to a file the user picked ("Save to…"), replacing it.
    SaveArtifact {
        id: String,
        /// Absolute path.
        path: String,
    },
    /// Copies an artifact under `file_name` into Brigadier's cache (emptied at the next start),
    /// for the user to open with its default app.
    OpenArtifact {
        id: String,
        file_name: String,
    },
    /// What a file link in a conversation's messages opens: whether `path` (the link as the
    /// app resolved it) is still there, and the copy Brigadier stored of it with a task's
    /// report or outputs, found from `link` as written.
    ResolveFileLink {
        conversation_id: ConversationId,
        link: String,
        path: String,
        /// When the message holding the link was written: a link to `outputs/<name>` opens the
        /// copy of the newest task made by then, not a later task's file of the same name.
        #[serde(default)]
        #[ts(optional)]
        at_ms: Option<i64>,
    },
    /// The icon of a web site an answer links to (`https://host`), fetched by the daemon from
    /// the site itself and kept in a small cache; `None` when it has none or can't be reached.
    SiteIcon {
        origin: String,
    },
    /// Stops its CLI processes and removes temp files now; the next message continues it.
    Hibernate {
        id: ConversationId,
    },
    /// Stops its workers and processes, removes its scratch, logs, build output and merged
    /// worktrees, and hides it in the Archived view; its record, CLI transcripts, Brain entries
    /// and snapshot refs stay, so it can be restored (G15). Answered once each is stored as
    /// archived; the cleanup goes on in the background.
    Archive {
        ids: Vec<ConversationId>,
    },
    /// Brings archived conversations back; their models restart from the transcript.
    Restore {
        ids: Vec<ConversationId>,
    },
    /// What removing a project takes with it.
    PreviewRemoveProject {
        id: ProjectId,
    },
    /// Removes a project from Brigadier: its conversations are deleted (as `Delete` does),
    /// then the picked branches, then its Brain and
    /// code index go to the Trash. Its repository's files and the user's own branches are
    /// never touched. Refused while one of its conversations works.
    RemoveProject {
        id: ProjectId,
        /// Brigadier branches to delete, at the tips the preview showed.
        delete_branches: Vec<BranchChoice>,
        keep_brain: bool,
    },
    /// Permanently removes conversations and their workers: their folders, CLI transcripts,
    /// Brain entries, snapshot refs, worktrees and the branches Brigadier created for them,
    /// unmerged ones too (the app asks first) (G15). Answered once each is marked as being
    /// deleted, which hides it; the rest goes on in the background.
    Delete {
        ids: Vec<ConversationId>,
    },
    /// The branches deleting `ids` would take that hold work that never landed, or whose
    /// standing can't be told.
    PreviewDelete {
        ids: Vec<ConversationId>,
    },
    RenameConversation {
        id: ConversationId,
        title: String,
    },
    SetPinned {
        id: ConversationId,
        pinned: bool,
    },
    AppendMessage {
        conversation_id: ConversationId,
        text: String,
    },
    ListMessages {
        conversation_id: ConversationId,
        /// Only messages with a smaller `seq` (for paging backwards).
        before: Option<i64>,
        limit: u32,
    },
    /// Where a text is, case aside, in the messages before `leaf` on its branch (the
    /// thread's Find, in pages it hasn't loaded).
    SearchMessages {
        conversation_id: ConversationId,
        leaf: String,
        query: String,
        limit: u32,
    },
    ReadBlobText {
        hash: String,
    },
    UpdateSettings {
        settings: Box<Settings>,
    },
    /// Starts the live event feed. Events after `afterSeq` that were already committed are
    /// replayed first; then new events stream as they commit.
    Subscribe {
        after_seq: i64,
        /// Also stream daemon metrics once a second.
        metrics: bool,
    },
    /// Turns the metrics stream on or off (the Inspector is shown or hidden).
    SetMetricsStreaming {
        enabled: bool,
    },
    /// A page of committed events, for resync after `lagged`.
    EventsSince {
        after_seq: i64,
        limit: u32,
    },
    GetDiagnostics,
    /// Emits `count` diagnostic events through the normal write path, `intervalMs` apart.
    ProbeBurst {
        count: u32,
        interval_ms: u32,
    },
    /// Whether the computer is being kept awake now, and whether it can be with the lid
    /// closed. Applies the current settings first.
    GetKeepAwake,
    /// Lets Brigadier disable sleep with the lid closed without asking again: asks for an
    /// administrator password once (macOS). Answers the status after.
    SetUpLidClosed,
    /// The newer versions of Brigadier and the agent CLIs the last check found; changes
    /// arrive as `updatesChanged`.
    GetUpdates,
    /// Updates an agent CLI in the background; how it goes arrives as `updatesChanged`.
    RunUpdate {
        target: UpdateTarget,
    },
    /// Connect's progress with each agent; changes arrive as `agentConnectChanged`.
    GetConnect,
    /// Gets an agent working in the background: its own native installer when it is missing,
    /// then its own sign-in in the browser, unless it is signed in already. Progress arrives
    /// as `agentConnectChanged`; asked again after a failure, it retries.
    ConnectAgent {
        provider: ProviderKind,
    },
    /// Stops the agent's Connect (its installer or sign-in); its row leaves the view.
    CancelConnect {
        provider: ProviderKind,
    },
    /// A code pasted from the agent's code page (`AgentConnect::code_page`), passed to the
    /// sign-in running for it.
    SendConnectCode {
        provider: ProviderKind,
        code: String,
    },
    /// Settings → Accounts: each provider's own login and extra accounts, who is signed in
    /// to each (as its CLI says) and its quota. Changes arrive as `accountsChecked`.
    GetAccounts,
    /// Settings → MCP servers and Skills: every server and skill the agents' configs on this
    /// machine name, with a project's own when `projectId` is given. Switching one off goes
    /// through [`Request::UpdateSettings`] (`mcpServersOff`, `skillsOff`).
    GetIntegrations {
        project_id: Option<ProjectId>,
    },
    /// Signs in to the remote MCP server `serverId` (an id of [`Request::GetIntegrations`]'s
    /// catalog, of `projectId`'s when given; a project the user doesn't trust is refused) once
    /// in Brigadier, in the user's browser. Answers at once with the sign-in's id; how it went
    /// comes from [`Request::GetMcpSignIn`]. The sign-in then serves every agent's workers.
    SignInMcpServer {
        server_id: String,
        project_id: Option<ProjectId>,
    },
    /// How sign-in `id` ([`Request::SignInMcpServer`]) went. Never carries a token.
    GetMcpSignIn {
        id: String,
    },
    /// Stops sign-in `id` while it waits for the browser: its callback listener closes,
    /// nothing is saved, and [`Request::GetMcpSignIn`] reports it cancelled.
    CancelMcpSignIn {
        id: String,
    },
    /// Opens sign-in `id`'s page in the user's browser again, while it waits.
    ReopenMcpSignIn {
        id: String,
    },
    /// Deletes Brigadier's own sign-in to the server `serverId` (of `projectId`'s catalog when
    /// given), if it has one. Claude Code's and Codex's own stores are never touched.
    SignOutMcpServer {
        server_id: String,
        project_id: Option<ProjectId>,
    },
    /// Forgets that macOS refused to let Brigadier read another agent's sign-in for the
    /// server `serverId` and reads it once more, so macOS asks again. The new state arrives
    /// with the next [`Request::GetIntegrations`].
    AskMcpSignInAgain {
        server_id: String,
        project_id: Option<ProjectId>,
    },
    /// Adds an extra account of `provider` (a CLI home of its own) and opens the terminal
    /// that signs it in: the CLI's own sign-in, run with that home. Its output streams like
    /// [`Request::OpenTerminal`]'s; the account is checked again when it ends.
    AddAccount {
        provider: ProviderKind,
        cols: u16,
        rows: u16,
    },
    /// Opens (or shows again) the terminal that signs extra account `id` in again.
    SignInAccount {
        id: String,
        cols: u16,
        rows: u16,
    },
    /// Signs extra account `id` out with its CLI's own sign-out and removes it. Refused while
    /// a chat or worker runs on it. Renaming, the default account and switching are settings.
    RemoveAccount {
        id: String,
    },
    /// Checks every provider and every account's login and quota again; results arrive as
    /// `accountsChecked`. Answers the check generation every account reaches once checked afresh.
    RefreshAccounts,
    /// Providers (login, models, quota), raw sessions and replayable fixtures.
    GetProviders,
    /// Checks every provider (or only `provider`) again in the background; results arrive as
    /// `providerChecked`.
    RefreshProviders {
        #[serde(default)]
        provider: Option<ProviderKind>,
    },
    /// Whether `path` is `provider`'s own CLI, before the user's choice of it is saved in
    /// `agentPaths`: answers its version, or fails with what is wrong in plain words.
    CheckAgentPath {
        provider: ProviderKind,
        path: String,
    },
    /// The Usage page: quota windows with their estimates and history, Brigadier's own use,
    /// past hand-offs and waits, and the merged model list.
    GetUsage {},
    /// The Usage page's Activity: each account's windows over the last `days` (at most 31), and
    /// Brigadier's own use of it then.
    GetUsageActivity {
        days: u32,
    },
    /// Asks the repository for a newer model registry now; answers the registry in use after.
    CheckRegistry,
    /// The latest refresh job, its findings and whether its saved overlay is in use.
    GetRankingsRefresh,
    /// Cancels a running refresh and removes its overlay, restoring published ratings.
    /// User rankings and learned outcomes are kept.
    ResetRankings,
    /// Development builds only: makes a provider refuse work as if a usage window ran out, for
    /// one task or conversation, after its next `afterToolCalls` tool calls (0: at once). The
    /// session's turn fails with a usage-limit error and the provider counts as limited until
    /// `resetInMinutes` from now; everything after that is the real fallback path.
    #[cfg(debug_assertions)]
    DebugInjectLimit {
        target: FaultTarget,
        provider: ProviderKind,
        /// The window that runs out (`five_hour`, `seven_day`, `primary`, …).
        window: String,
        reset_in_minutes: u32,
        after_tool_calls: u32,
    },
    /// Development builds only: makes a provider end the running turn of one task or
    /// conversation with an error after its next `afterToolCalls` tool calls (0: at once), as a
    /// CLI that hit it would: a context window too small, a failed sign-in or credits run out.
    /// Everything after that is the real path.
    #[cfg(debug_assertions)]
    DebugInjectError {
        target: FaultTarget,
        provider: ProviderKind,
        error: InjectedError,
        after_tool_calls: u32,
    },
    /// Development builds only: adds `text` to the `learned` items of the next report a worker
    /// of the conversation submits (the continuity battery's false claim).
    #[cfg(debug_assertions)]
    DebugPlantLearned {
        conversation_id: ConversationId,
        text: String,
    },
    /// Starts a raw CLI session in the background; its state arrives as `rawSessionUpdated`.
    StartRawSession {
        provider: ProviderKind,
        /// Absolute path of the working directory.
        cwd: String,
        model: Option<String>,
        effort: Option<String>,
        access: Access,
        approvals: RawApprovals,
        /// Record the raw stdio exchange as a replayable fixture.
        record: bool,
    },
    /// Starts a stopped raw session's CLI session again.
    ResumeRawSession {
        id: RawSessionId,
    },
    /// Branches a new raw session off this one's CLI session.
    ForkRawSession {
        id: RawSessionId,
    },
    /// Sends a message: a new turn, or with `steer` into the running turn.
    SendRawSession {
        id: RawSessionId,
        text: String,
        steer: bool,
    },
    InterruptRawSession {
        id: RawSessionId,
    },
    /// The user's answer to an approval routed to them.
    AnswerApproval {
        id: RawSessionId,
        approval_id: String,
        decision: ApprovalDecision,
    },
    /// Ends the CLI process, keeping its CLI session for a resume.
    StopRawSession {
        id: RawSessionId,
    },
    /// Ends the CLI process and removes everything its CLI session created.
    CloseRawSession {
        id: RawSessionId,
    },
    /// A page of a raw session's transcript.
    ListRawEvents {
        id: RawSessionId,
        /// Only entries with a smaller `streamSeq` (for paging backwards).
        before: Option<i64>,
        limit: u32,
    },
    /// Replays a fixture through a fresh parser into a new, isolated raw session.
    ReplayFixture {
        fixture_id: String,
    },
    /// Development builds only: feeds a simulated usage-limit turn through a fresh parser in an
    /// isolated raw session.
    #[cfg(debug_assertions)]
    SimulateUsageLimit {
        provider: ProviderKind,
    },
    /// Whether this daemon is in use: another Brigadier asks before offering to stop it.
    GetDaemonActivity,
    /// What Brigadier keeps on disk and what it can clean up (Settings → Storage).
    ScanStorage {
        /// The asking app's bundle identifier: its own caches are never offered. Without it, no
        /// app's caches are.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        app: Option<String>,
    },
    /// Removes the picked items of a scan (by the ids it gave them), each checked again first.
    /// Subscribed connections hear how it goes (`storageClean`) after each item.
    CleanStorage {
        scan_id: String,
        items: Vec<String>,
    },
    /// Frees up projects not in use (Settings › Storage): their conversations are deleted (as
    /// `Delete` does, keeping every branch, recorded as kept) and their Brains go to the
    /// Trash. The projects, their repositories, files and branches stay. A project one of
    /// whose conversations works is left as it is.
    FreeUpProjects {
        ids: Vec<ProjectId>,
    },
    /// What uninstalling Brigadier removes (Settings → Storage, or the app menu).
    PreviewUninstall {
        app: UninstallApp,
    },
    /// Uninstalls Brigadier as the plan said: everything stops and what Brigadier created goes
    /// (the picked branches too); the data directory (unless `keep_data`), the per-app folders
    /// and the app go to the Trash once the app and the daemon quit.
    Uninstall {
        plan_id: String,
        keep_data: bool,
        delete_branches: Vec<BranchChoice>,
    },
    /// Makes a new, empty page (a local Markdown document in the data folder).
    PageCreate,
    /// A page as it is on disk now.
    PageRead {
        id: String,
    },
    /// Saves a page's title, emoji and Markdown, if it is still at `baseRevision`; otherwise
    /// nothing is written and the answer is a conflict with the page as it is now.
    PageWrite {
        id: String,
        content: PageContent,
        base_revision: String,
    },
    /// Every page, the most recently changed first.
    PageList,
    /// Deletes a page; one already gone is fine.
    PageDelete {
        id: String,
    },
    /// Orderly quit: stop admitting writes, commit what is queued, acknowledge, exit.
    Shutdown,
}

/// Results, tagged with the method of the request they answer.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(
    tag = "method",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Response {
    GetCatalog {
        catalog: Box<Catalog>,
    },
    GetActivity {
        activity: Vec<ConversationActivity>,
    },
    GetSettledTabs {
        tabs: Vec<SettledTab>,
    },
    GetMergeUndo {
        /// None: never reverted.
        undo: Option<MergeUndo>,
        /// The merge is on its base's upstream: pushed since, if it was made on this Mac.
        #[serde(default)]
        pushed: bool,
    },
    CreateProject {
        project: Box<Project>,
    },
    UpdateProject {
        project: Box<Project>,
    },
    SetFolderTrust {
        report: Box<FolderTrustReport>,
    },
    GetBrain {
        overview: Box<BrainOverview>,
    },
    QueryBrain {
        answer: Box<BrainAnswer>,
    },
    GetBrainGraph {
        graph: Box<BrainGraph>,
    },
    ExportConventions {
        export: ConventionsExport,
    },
    RebuildIndex,
    GetRepoInfo {
        repo: RepoInfo,
    },
    FindProjects {
        candidates: Vec<ProjectCandidate>,
    },
    FindRepos {
        repos: Vec<RepoSuggestion>,
    },
    BrowseFolders {
        listing: FolderListing,
    },
    CheckFolder {
        check: FolderCheck,
    },
    AddProject {
        project: Box<Project>,
    },
    CloneProject {
        project: Box<Project>,
    },
    ListFiles {
        /// Paths relative to the checkout's root, tracked and untracked (not ignored).
        files: Vec<String>,
        /// Set when the checkout has more files than were listed.
        truncated: bool,
    },
    ReadFile {
        file: CheckoutFile,
    },
    OpenTerminal {
        terminal: TerminalInfo,
    },
    OpenSideChat {
        conversation: Box<Conversation>,
    },
    OpenTabChat {
        conversation: Box<Conversation>,
    },
    KeepTabChats,
    SetTabPage,
    WriteTerminal,
    ResizeTerminal,
    ClearTerminal,
    CloseTerminal,
    GetComputerUse {
        computer_use: ComputerUseStatus,
    },
    GrantComputerPermission,
    CheckComputerUse {
        computer_use: ComputerUseStatus,
    },
    ConfirmComputerCapture {
        computer_use: ComputerUseStatus,
        /// Why screen capture didn't work, when it didn't.
        problem: Option<String>,
    },
    StopComputerSession,
    GetDictation {
        dictation: DictationStatus,
    },
    DownloadDictationModel,
    CancelDictationDownload,
    StartDictation {
        dictation_id: String,
    },
    AppendDictation,
    FinishDictation,
    CancelDictation,
    RateMessage,
    GetSessionDiff {
        /// Absent for Chats and local-checkout sessions.
        stat: Option<DiffStat>,
        /// The branch's commits its base lacks: what a merge would take, even when their sum
        /// changes nothing. 0 without a stat.
        #[serde(default)]
        commits: u32,
    },
    GetWorkerDiffs {
        diffs: Vec<WorkerDiff>,
    },
    CreateConversation {
        conversation: Box<Conversation>,
    },
    StartSession {
        conversation: Box<Conversation>,
        outcome: SendOutcome,
    },
    ForkConversation {
        conversation: Box<Conversation>,
    },
    UpdateSetup {
        conversation: Box<Conversation>,
    },
    GetConversation {
        view: Box<ConversationView>,
    },
    SendMessage {
        outcome: SendOutcome,
    },
    EditQueued {
        queue: MessageQueue,
    },
    DeleteQueued {
        queue: MessageQueue,
    },
    MoveQueued {
        queue: MessageQueue,
    },
    SteerQueued,
    ResumeQueue {
        queue: MessageQueue,
    },
    SettleMoved,
    RunQueuedHere {
        queue: MessageQueue,
    },
    Interrupt,
    Resume,
    StopPreview,
    PausePreview,
    ResumePreview,
    RestartPreview,
    ClearPreviews,
    DismissBackground,
    PreviewLog {
        tail: String,
        url: Option<String>,
    },
    Compact,
    GetConversationStatus {
        status: ConversationStatus,
    },
    EditMessage,
    Regenerate,
    SwitchBranch,
    AddAttachment {
        attachment: AttachmentRef,
    },
    ReadAttachment {
        data: String,
    },
    PinDraftAttachments,
    PrepareTurnRevert {
        preview: TurnRevert,
    },
    RevertTurn {
        outcome: UndoOutcome,
    },
    GetPendingChanges {
        pending: PendingChanges,
    },
    KeepChanges,
    UndoPendingChanges {
        outcome: UndoOutcome,
    },
    ReadThreadImage {
        data: String,
        mime: String,
    },
    ThreadView {
        view: brigadier_core::thread_view::ThreadView,
    },
    WorkerThread {
        view: brigadier_core::thread_view::ThreadView,
    },
    ContinueWaiting,
    DontResume,
    GetBrigadeSkill {
        custom: bool,
        path: String,
    },
    ResetBrigadeSkill {
        custom: bool,
        path: String,
    },
    CreateBrigadeSkill {
        custom: bool,
        path: String,
    },
    GetModelRules {
        rules: ModelRules,
    },
    SetModelRules {
        rules: ModelRules,
    },
    StopBackgroundTask,
    ReadBackgroundTaskOutput {
        output: String,
    },
    GetReviewDiff {
        review: ReviewDiff,
    },
    GetGitState {
        state: GitState,
    },
    GetPullRequest {
        pull_request: Option<PullRequest>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        #[ts(as = "Option<bool>", optional)]
        auth_required: bool,
    },
    CommitChanges {
        outcome: CommitOutcome,
    },
    PushChanges {
        branch: String,
    },
    GetSourceState {
        state: SourceState,
    },
    StageFiles {
        state: SourceState,
    },
    UnstageFiles {
        state: SourceState,
    },
    DiscardFiles {
        state: SourceState,
    },
    ListProjectFiles {
        files: Vec<String>,
        truncated: bool,
    },
    GetWorkspaceTree {
        tree: WorkspaceTree,
    },
    ListWorkspaceFolder {
        folder: WorkspaceFolder,
    },
    WatchWorkspace {
        /// Whether a watch runs now (false when stopped, or the target has no checkout).
        watching: bool,
    },
    StartSearch {
        id: u32,
    },
    CancelSearch,
    ReplaceInFiles {
        outcome: ReplaceOutcome,
    },
    GetProjectSourceState {
        state: SourceState,
    },
    ChangeProjectSource {
        state: SourceState,
    },
    CommitProjectChanges {
        outcome: CommitOutcome,
    },
    ListSourceBranches {
        branches: Vec<SourceBranch>,
    },
    SourceAction {
        state: SourceState,
    },
    GetSourceGitOutput {
        runs: Vec<SourceGitRun>,
    },
    GetConflictFile {
        file: ConflictFile,
    },
    WriteCommitMessage {
        message: String,
        /// Why the user's chosen model didn't write it, when it didn't.
        note: Option<String>,
    },
    AnswerCard,
    AnswerQuestion,
    WriteUpFeedback {
        write_up: FeedbackWriteUp,
    },
    FileFeedback {
        /// Whether `gh` is signed in; when it isn't, `results` is empty.
        signed_in: bool,
        /// What became of each item, in order.
        results: Vec<FeedbackFiled>,
    },
    FeedbackAccount {
        /// Whether `gh` is signed in; when it isn't, Send signs it in first.
        signed_in: bool,
        /// The GitHub login, when `gh` could say it.
        login: Option<String>,
    },
    StartGitHubSignIn {
        sign_in: GitHubSignIn,
    },
    WaitGitHubSignIn,
    CancelGitHubSignIn,
    AnswerFeedbackOffer,
    /// What the click set going, for the user.
    MergeSession {
        text: String,
    },
    /// What went and what stayed, for the user.
    CloseWorkspace {
        text: String,
    },
    AppAnswer,
    BrowserPaused,
    StopTask,
    StopWorkers {
        /// The tasks stopped, oldest first.
        task_ids: Vec<TaskId>,
    },
    PauseTask,
    ResumeTask,
    RestoreKeptWork {
        outcome: RestoreOutcome,
    },
    ListWorkerEvents {
        page: WorkerPage,
    },
    ListOrchestratorLog {
        page: OrchestratorPage,
    },
    GetThreadMetrics {
        metrics: ThreadMetrics,
    },
    ReadArtifact {
        text: ArtifactText,
    },
    SaveArtifact,
    OpenArtifact {
        /// The copy to open.
        path: String,
    },
    ResolveFileLink {
        /// The file at the link's path exists.
        exists: bool,
        stored: Option<StoredCopy>,
    },
    SiteIcon {
        /// The icon as a `data:` URL.
        icon: Option<String>,
    },
    Hibernate {
        conversation: Box<Conversation>,
    },
    Archive {
        outcomes: Vec<LifecycleOutcome>,
    },
    Restore {
        outcomes: Vec<LifecycleOutcome>,
    },
    PreviewRemoveProject {
        removal: Box<ProjectRemoval>,
    },
    RemoveProject {
        report: RemoveProjectReport,
    },
    Delete {
        outcomes: Vec<LifecycleOutcome>,
    },
    PreviewDelete {
        branches: Vec<UnlandedBranch>,
    },
    RenameConversation {
        conversation: Box<Conversation>,
    },
    SetPinned {
        conversation: Box<Conversation>,
    },
    AppendMessage {
        message: Box<Message>,
    },
    ListMessages {
        page: MessagePage,
    },
    SearchMessages {
        found: MessageMatches,
    },
    ReadBlobText {
        text: String,
    },
    UpdateSettings {
        settings: Box<Settings>,
    },
    Subscribe {
        last_seq: i64,
    },
    SetMetricsStreaming {
        enabled: bool,
    },
    EventsSince {
        events: Vec<EventEnvelope>,
        last_seq: i64,
    },
    GetDiagnostics {
        diagnostics: Box<Diagnostics>,
    },
    ProbeBurst {
        burst: ProbeBurst,
    },
    GetKeepAwake {
        status: KeepAwakeStatus,
    },
    SetUpLidClosed {
        status: KeepAwakeStatus,
    },
    GetUpdates {
        updates: UpdatesView,
    },
    RunUpdate,
    GetConnect {
        connect: ConnectView,
    },
    ConnectAgent,
    CancelConnect,
    SendConnectCode,
    GetAccounts {
        accounts: AccountsView,
    },
    GetIntegrations {
        integrations: brigadier_core::manager::integrations::IntegrationsView,
    },
    SignInMcpServer {
        id: String,
    },
    GetMcpSignIn {
        status: brigadier_core::manager::McpSignInStatus,
    },
    CancelMcpSignIn,
    ReopenMcpSignIn,
    SignOutMcpServer,
    AskMcpSignInAgain,
    AddAccount {
        account: AccountEntry,
        terminal: TerminalInfo,
    },
    SignInAccount {
        terminal: TerminalInfo,
    },
    RemoveAccount {
        settings: Box<Settings>,
    },
    RefreshAccounts {
        generation: u32,
    },
    GetProviders {
        view: ProvidersView,
    },
    RefreshProviders,
    CheckAgentPath {
        version: String,
    },
    GetUsage {
        usage: Box<UsageView>,
    },
    GetUsageActivity {
        activity: UsageActivity,
    },
    CheckRegistry {
        registry: RegistryInfo,
    },
    GetRankingsRefresh {
        refresh: brigadier_core::routing::registry::RankingsRefresh,
    },
    ResetRankings {
        refresh: brigadier_core::routing::registry::RankingsRefresh,
    },
    #[cfg(debug_assertions)]
    DebugInjectLimit,
    #[cfg(debug_assertions)]
    DebugInjectError,
    #[cfg(debug_assertions)]
    DebugPlantLearned,
    StartRawSession {
        session: Box<RawSession>,
    },
    ResumeRawSession {
        session: Box<RawSession>,
    },
    ForkRawSession {
        session: Box<RawSession>,
    },
    SendRawSession,
    InterruptRawSession,
    AnswerApproval,
    StopRawSession,
    CloseRawSession {
        session: Box<RawSession>,
    },
    ListRawEvents {
        page: RawPage,
    },
    ReplayFixture {
        session: Box<RawSession>,
    },
    #[cfg(debug_assertions)]
    SimulateUsageLimit {
        session: Box<RawSession>,
    },
    GetDaemonActivity {
        activity: DaemonActivity,
    },
    ScanStorage {
        report: Box<StorageReport>,
    },
    CleanStorage {
        report: CleanReport,
    },
    FreeUpProjects {
        report: FreeUpProjectsReport,
    },
    PreviewUninstall {
        plan: Box<UninstallPlan>,
    },
    Uninstall {
        report: UninstallReport,
    },
    PageCreate {
        page: Page,
    },
    PageRead {
        page: Page,
    },
    PageWrite {
        outcome: PageWriteOutcome,
    },
    PageList {
        pages: Vec<PageSummary>,
    },
    PageDelete,
    Shutdown,
}

/// What happened to a sent message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SendOutcome {
    /// It is in the transcript (a new turn, or steered into the running one).
    Sent { message: Box<Message> },
    /// It waits in the queue.
    Queued { item: QueuedMessage },
}

/// A slice of an artifact's text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactText {
    pub text: String,
    pub offset: u64,
    pub total_bytes: u64,
    /// Not text: `text` is empty.
    pub binary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    NotFound,
    Invalid,
    ShuttingDown,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct IpcError {
    pub code: ErrorCode,
    pub message: String,
}

/// The daemon's identity, sent after a successful hello.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DaemonInfo {
    pub version: String,
    pub protocol: u32,
    pub pid: u32,
    pub platform: String,
    pub started_at_ms: i64,
    pub data_dir: String,
}

/// A committed event as delivered to clients.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct EventEnvelope {
    pub seq: i64,
    pub stream: String,
    pub stream_seq: i64,
    /// When the daemon ingested the event, in ms since the Unix epoch.
    pub at_ms: i64,
    /// The stored payload, passed through without re-encoding.
    #[ts(as = "brigadier_core::DomainEvent")]
    pub event: RawJson,
}

/// Pre-encoded JSON. The daemon sends stored payloads as they are (no re-encoding per
/// subscriber). Deserializing goes through [`serde_json::Value`], because the frames are
/// internally tagged enums and serde buffers their content, which a borrowed [`RawValue`]
/// cannot be read back from.
#[derive(Debug, Clone)]
pub struct RawJson(pub Box<RawValue>);

impl Serialize for RawJson {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RawJson {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        serde_json::value::to_raw_value(&value)
            .map(RawJson)
            .map_err(serde::de::Error::custom)
    }
}

/// Frames sent by the daemon.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ServerFrame {
    Welcome {
        daemon: DaemonInfo,
        last_seq: i64,
    },
    Response {
        id: u32,
        result: Outcome,
    },
    Event {
        event: EventEnvelope,
    },
    /// The client fell behind the live feed and was unsubscribed. Resync with `eventsSince`
    /// from `resumeAfter`, then subscribe again.
    Lagged {
        resume_after: i64,
    },
    Metrics {
        metrics: DaemonMetrics,
    },
    /// Output of a terminal this connection opened. Live only: never stored.
    Terminal {
        output: TerminalOutput,
    },
    /// How a dictation or the speech model's download this connection started goes. Live
    /// only: never stored.
    Dictation {
        update: DictationUpdate,
    },
    /// A page changed on disk: saved, edited by another program, or deleted (no `revision`).
    /// Sent to every subscribed connection; editors showing an older revision read it again.
    PageChanged {
        id: String,
        revision: Option<String>,
    },
    /// The checkout this connection watches changed: its files, index or HEAD. Also sent to
    /// every subscribed connection when a tab chat saves a file in a session's checkout. Live only.
    WorkspaceChanged {
        target: WorkspaceTarget,
    },
    /// What a CLI's live stream added to a thread ([`Request::ThreadView`]). Sent to every
    /// subscribed connection. Live only.
    ThreadNews {
        news: brigadier_core::thread_view::ThreadNews,
    },
    /// What this connection's search with `id` found next, or that it ended. Live only.
    Search {
        id: u32,
        update: SearchUpdate,
    },
    /// Computer use changed: permissions, setup or the agents using apps. Sent to every
    /// subscribed connection. Live only.
    ComputerUse {
        status: ComputerUseStatus,
    },
    /// An agent needs computer use and its setup isn't finished: the app opens the setup
    /// window. Live only.
    ComputerUseSetupNeeded,
    /// An agent's call of the app's window (Q90; a Browser tab's call is one), for the app to
    /// run and answer with `appAnswer`. Sent to every subscribed connection. Live only.
    AppCall {
        call: AppCall,
    },
    /// How a clean under way goes (Settings › Storage). Sent to every subscribed connection.
    /// Live only.
    StorageClean {
        progress: CleanProgress,
    },
    /// The daemon is shutting down; the connection closes next.
    Closing,
}

/// Whether the computer is kept awake, as `getKeepAwake` answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct KeepAwakeStatus {
    /// Sleep is being prevented now.
    pub active: bool,
    pub lid_closed: LidClosed,
    /// Keeping awake also keeps the screen on here (not on Linux: the desktop decides).
    #[serde(default)]
    pub screen_on: bool,
    /// Running on battery power now.
    #[serde(default)]
    pub on_battery: bool,
    /// Why keeping awake (or the lid-closed part of it) isn't working, when it isn't.
    pub error: Option<String>,
}

/// Whether a daemon is in use, as `getDaemonActivity` answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DaemonActivity {
    /// App connections other than the one asking.
    pub clients: u32,
    /// What runs, said plainly (a turn or a worker, a terminal, a Brain job, …); empty when
    /// nothing does.
    pub running: Vec<String>,
}

/// Staying awake with the lid closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum LidClosed {
    /// This platform can't (Windows: the lid follows the power settings).
    Unsupported,
    /// Needs an administrator password once (`setUpLidClosed`).
    NeedsSetup,
    /// Can be turned on without asking.
    Ready,
    /// Can, but not now: the battery is too low, so closing the lid sleeps the computer.
    LowBattery,
    /// Closing the lid doesn't sleep the computer now.
    Active,
}

/// A session's terminal, as `openTerminal` returns it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInfo {
    pub id: String,
    /// The shell it runs, and where.
    pub shell: String,
    pub cwd: String,
    /// Its latest output, for a tab opened again while it runs.
    pub scrollback: String,
}

/// What a terminal printed, or that its shell ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TerminalOutput {
    Data {
        terminal_id: String,
        data: String,
    },
    Exited {
        terminal_id: String,
        code: Option<u32>,
    },
}

/// Whether dictation can run here, as `getDictation` returns it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DictationStatus {
    /// This build has the speech engine.
    pub available: bool,
    /// The speech model's file name.
    pub model: String,
    /// Its size, in bytes.
    pub model_bytes: u64,
    /// It is downloaded and checked.
    pub installed: bool,
    /// Its download is under way.
    pub downloading: bool,
    /// The languages the speech engine knows, in its own order.
    pub languages: Vec<DictationLanguage>,
}

/// A language dictation can listen for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DictationLanguage {
    /// As the speech engine names it ("en").
    pub code: String,
    /// In English ("english"), as the speech engine has it.
    pub name: String,
}

/// A step of a dictation or of the speech model's download.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DictationUpdate {
    /// Bytes of the speech model downloaded so far.
    Download { received: u64, total: u64 },
    /// The speech model is downloaded and checked.
    Downloaded,
    /// The download stopped: cancelled, or failed with `message`.
    DownloadStopped { message: Option<String> },
    /// What was said.
    Transcribed { dictation_id: String, text: String },
    /// The dictation failed.
    Failed {
        dictation_id: String,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum Outcome {
    Ok { value: Box<Response> },
    Err { error: IpcError },
}

impl From<Result<Response, IpcError>> for Outcome {
    fn from(result: Result<Response, IpcError>) -> Self {
        match result {
            Ok(value) => Self::Ok {
                value: Box::new(value),
            },
            Err(error) => Self::Err { error },
        }
    }
}

impl From<brigadier_core::Error> for IpcError {
    fn from(err: brigadier_core::Error) -> Self {
        use brigadier_core::Error as E;
        let code = match &err {
            _ if err.is_shutting_down() => ErrorCode::ShuttingDown,
            E::NotFound(_) => ErrorCode::NotFound,
            E::Invalid(_) | E::Provider(_) => ErrorCode::Invalid,
            E::Store(_) | E::Corrupt { .. } => ErrorCode::Internal,
        };
        Self {
            code,
            message: err.to_string(),
        }
    }
}

impl From<PageError> for IpcError {
    fn from(err: PageError) -> Self {
        let code = match &err {
            PageError::NotFound(_) => ErrorCode::NotFound,
            PageError::Invalid(_) => ErrorCode::Invalid,
            PageError::Io { .. } => ErrorCode::Internal,
        };
        Self {
            code,
            message: err.to_string(),
        }
    }
}

#[cfg(test)]
mod inline_edit_tests {
    use super::*;

    #[test]
    fn edit_attachments_are_optional_and_preserve_inline_refs() {
        let old: Request = serde_json::from_value(serde_json::json!({
            "method": "editMessage", "conversationId": "conv", "messageId": "message", "text": "edited"
        })).unwrap();
        assert!(matches!(
            old,
            Request::EditMessage {
                attachments: None,
                ..
            }
        ));
        let new: Request = serde_json::from_value(serde_json::json!({
            "method": "editMessage", "conversationId": "conv", "messageId": "message", "text": "[image:a]",
            "attachments": [{"id":"a", "name":"a.png", "mime":"image/png", "bytes":1, "inline":true}]
        })).unwrap();
        let Request::EditMessage {
            attachments: Some(refs),
            ..
        } = new
        else {
            panic!("expected refs")
        };
        assert!(refs[0].inline.is_some());
    }
}

#[cfg(test)]
mod preview_tests {
    use super::*;

    #[test]
    fn preview_controls_and_log_tail_keep_their_wire_methods() {
        for method in ["pausePreview", "resumePreview", "previewLog"] {
            let wire = serde_json::json!({"method": method, "conversationId": "c", "previewId": "preview-1", "lines": 2});
            let request: Request = serde_json::from_value(wire).unwrap();
            assert_eq!(serde_json::to_value(request).unwrap()["method"], method);
        }
        let request: Request = serde_json::from_value(
            serde_json::json!({"method":"clearPreviews", "conversationId":"c"}),
        )
        .unwrap();
        assert!(matches!(request, Request::ClearPreviews { .. }));
        let response = Response::PreviewLog {
            tail: "last two\nlines".into(),
            url: Some("http://localhost:5173/".into()),
        };
        let wire = serde_json::to_value(response).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({"method":"previewLog", "tail":"last two\nlines", "url":"http://localhost:5173/"})
        );
        let decoded: Response = serde_json::from_value(wire).unwrap();
        assert!(matches!(decoded, Response::PreviewLog { tail, .. } if tail == "last two\nlines"));
    }
}

#[cfg(test)]
mod browser_tests {
    use super::*;

    #[test]
    fn a_browser_call_and_its_answer_keep_their_wire_shape() {
        let frame = ServerFrame::AppCall {
            call: AppCall {
                id: "app-call-1".into(),
                conversation_id: ConversationId("c".into()),
                task_id: None,
                label: "Check the form".into(),
                op: brigadier_core::AppOp::Browser {
                    op: brigadier_core::BrowserOp::Click {
                        tab: "browser:agent-1".into(),
                        target: "e3".into(),
                        guard: brigadier_core::BrowserGuard {
                            declared: Some(brigadier_core::BrowserSensitive::Send),
                            approved: None,
                        },
                    },
                },
            },
        };
        let wire = serde_json::to_value(&frame).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({"type": "appCall", "call": {
                "id": "app-call-1", "conversationId": "c", "taskId": null, "label": "Check the form",
                "op": {"app": "browser", "op": {"op": "click", "tab": "browser:agent-1", "target": "e3",
                       "guard": {"declared": "send", "approved": null}}}}})
        );
        let answer = serde_json::json!({"method": "appAnswer", "id": "app-call-1",
            "result": {"app": "browser", "result": {"result": "paused", "tab": "browser:agent-1"}}});
        let request: Request = serde_json::from_value(answer.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), answer);
        let paused = serde_json::json!({"method": "browserPaused", "conversationId": "c",
            "tab": "browser:agent-1", "paused": true});
        let request: Request = serde_json::from_value(paused.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), paused);
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    /// `brigadier` writes its first frame without this crate: both must agree on its shape.
    #[test]
    fn the_brigadier_commands_first_frame_is_a_cli_frame() {
        let written = serde_json::to_value(brigadier_cli::client::FirstFrame {
            session: "s",
            grant: "g",
        })
        .unwrap();
        let frame: ClientFrame = serde_json::from_value(written.clone()).unwrap();
        assert!(
            matches!(&frame, ClientFrame::Cli { session, grant } if session == "s" && grant == "g")
        );
        assert_eq!(serde_json::to_value(frame).unwrap(), written);
    }
}
