//! The wire protocol between the app and `brigadierd`.
//!
//! Frames are length-prefixed JSON (see [`crate::frame`]). The first client frame must be a
//! [`ClientFrame::Hello`] carrying the per-launch token, or one of the two grant-scoped frames
//! CLI sessions use ([`ClientFrame::Mcp`], [`ClientFrame::Gate`]); anything else closes the
//! connection.
//! Requests carry a client-chosen id echoed on the response. Responses reuse the request's
//! `method` tag, so TypeScript can pair them with `Extract<Response, { method: M }>`.

use brigadier_brain::{BrainAnswer, BrainGraph, BrainQuery, Node, NodeFilter};
use brigadier_core::storage::{
    BranchChoice, CleanReport, ProjectRemoval, RemoveProjectReport, StorageReport, UninstallApp,
    UninstallPlan, UninstallReport,
};
use brigadier_core::{
    AttachmentRef, BrainJobKind, BrainOverview, CardId, Catalog, CheckoutFile, CommitOutcome,
    ConventionsExport, Conversation, ConversationActivity, ConversationId, ConversationKind,
    ConversationStatus, ConversationView, DiffStat, FolderCheck, FolderListing, ForkPlace,
    GitState, Mention, Message, MessagePage, MessageQueue, OrchestratorPage, OvernightRun,
    OvernightRunId, ProbeBurst, Project, ProjectCandidate, ProjectId, ProjectPatch, ProposedPlan,
    ProvidersView, PullRequest, QueuedMessage, Rating, RawApprovals, RawPage, RawSession,
    RawSessionId, RepoInfo, RestoreOutcome, ReviewDiff, ReviewScope, RoutePreview, Settings, Setup,
    SetupRequest, TaskId, UsageView, WorkerDiff, WorkerPage,
};
use brigadier_providers::{Access, ApprovalDecision, ProviderKind};
use brigadier_router::{Area, RegistryInfo};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use ts_rs::TS;

use crate::metrics::{DaemonMetrics, Diagnostics};

/// Bumped on any incompatible change to these types.
pub const PROTOCOL_VERSION: u32 = 6;

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
    /// First frame of a Brigadier MCP connection (`brigadierd mcp`, spawned by a CLI session).
    /// After it the connection carries raw MCP (newline-delimited JSON-RPC) in both directions.
    /// The grant must belong to an orchestrator or a worker; there is no token and no reply
    /// frame, a refused grant just closes the connection.
    Mcp {
        grant: String,
    },
    /// First and only frame of an outward-command gate check: may `argv` run in `cwd`? The
    /// grant must be a gate grant. Answered with one [`GateVerdict`].
    Gate {
        grant: String,
        /// The full command line as the program received it (`argv[0]` included).
        argv: Vec<String>,
        cwd: String,
    },
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
    /// A project's Brain at a glance, or the Personal Brain's without `projectId`.
    GetBrain {
        project_id: Option<ProjectId>,
    },
    /// Runs a Brain query as the orchestrator's `query_brain` does (the Inspector's search).
    QueryBrain {
        project_id: Option<ProjectId>,
        query: BrainQuery,
    },
    /// Nodes and their edges, for the Inspector's graph viewer.
    GetBrainGraph {
        project_id: Option<ProjectId>,
        filter: NodeFilter,
    },
    /// The Personal Brain's memories (preferences), newest first.
    ListMemories,
    /// Removes a memory from the Personal Brain.
    ForgetMemory {
        node_id: String,
    },
    /// Writes the project's conventions into the marked Brigadier section of an AGENTS.md
    /// (the native save panel's choice, or a typed path), creating the file if needed.
    ExportConventions {
        project_id: ProjectId,
        path: String,
    },
    /// Starts a Brain job now (the Inspector's developer actions); it reports on the project's
    /// `brain:` stream.
    RunBrainJob {
        project_id: ProjectId,
        kind: BrainJobKind,
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
    /// One file of a session's checkout, for the side panel's Files tab.
    ReadFile {
        conversation_id: ConversationId,
        /// Relative to the checkout's root.
        path: String,
    },
    /// The session's terminal (the side panel's Terminal tab): a shell in its checkout,
    /// started if none runs, sized `cols` × `rows`. Its output then streams to this
    /// connection as [`ServerFrame::Terminal`].
    OpenTerminal {
        conversation_id: ConversationId,
        cols: u16,
        rows: u16,
    },
    /// The conversation's side chat (the side panel's Side chat tab), started if it has none.
    /// Closing it deletes it.
    OpenSideChat {
        conversation_id: ConversationId,
    },
    /// A terminal in the home folder that sets up a CLI (the first run's Install and Sign
    /// in), started if none runs for it: it runs the CLI's installer when `install`, else its
    /// sign-in, and ends with it. Its output streams like [`Request::OpenTerminal`]'s.
    OpenSetupTerminal {
        provider: ProviderKind,
        install: bool,
        cols: u16,
        rows: u16,
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
    /// Ends a terminal's shell.
    CloseTerminal {
        terminal_id: String,
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
    /// Rates an answer ("Good response" / "Bad response").
    RateMessage {
        conversation_id: ConversationId,
        /// A message id, or `task:<id>` for a worker's report.
        subject: String,
        rating: Rating,
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
    /// Stops the running turn; the queue pauses.
    Interrupt {
        conversation_id: ConversationId,
    },
    /// Continues the latest request after it was stopped, in the same block; the queue
    /// unpauses and runs after it.
    Resume {
        conversation_id: ConversationId,
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
    /// Undoes (or, with `reapply`, reapplies) what a request's workers landed, with a new
    /// commit on the branch they landed on.
    UndoChanges {
        conversation_id: ConversationId,
        request_id: String,
        reapply: bool,
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
    },
    /// The user's push of a session checkout's branch.
    PushChanges {
        conversation_id: ConversationId,
    },
    /// Answers an approval card.
    AnswerCard {
        conversation_id: ConversationId,
        card_id: CardId,
        decision: ApprovalDecision,
    },
    AnswerQuestion {
        conversation_id: ConversationId,
        card_id: CardId,
        answer: String,
    },
    DecidePlan {
        conversation_id: ConversationId,
        card_id: CardId,
        approve: bool,
        message: Option<String>,
    },
    /// Proposes an overnight run from the user's words and the plan read from them (`None`:
    /// a bare goal). Nothing runs until `startOvernight`. Every overnight command carries a
    /// client-chosen `commandId`; sending one again changes nothing.
    ProposeOvernight {
        conversation_id: ConversationId,
        command_id: String,
        words: String,
        plan: Option<ProposedPlan>,
    },
    /// The user's Start, naming the proposal revision they saw.
    StartOvernight {
        conversation_id: ConversationId,
        run_id: OvernightRunId,
        command_id: String,
        revision: u32,
    },
    /// The user's Stop: drops a proposal, or winds a started run down now.
    StopOvernight {
        conversation_id: ConversationId,
        run_id: OvernightRunId,
        command_id: String,
    },
    /// The user's words changing a run's restrictions ("until 09:00 instead").
    SteerOvernight {
        conversation_id: ConversationId,
        run_id: OvernightRunId,
        command_id: String,
        words: String,
    },
    /// Proposes the next segment of a finished run, with a new deadline from `words`.
    ContinueOvernight {
        conversation_id: ConversationId,
        run_id: OvernightRunId,
        command_id: String,
        words: String,
    },
    /// The user's Merge of a run's verified work: `verifiedCommit` is the tip the card showed;
    /// only it (never later, unverified commits) merges into the run's base.
    MergeOvernight {
        conversation_id: ConversationId,
        run_id: OvernightRunId,
        command_id: String,
        verified_commit: String,
    },
    /// What a run's branch changed since its base: up to the branch tip while it works, up to
    /// the verified tip once it finished.
    GetRunDiff {
        conversation_id: ConversationId,
        run_id: OvernightRunId,
    },
    /// Notifications of finished overnight runs the app hasn't shown yet.
    PendingOvernightNotifications,
    /// A native submission was refused; retain the report and pending notification.
    FailOvernightNotification {
        conversation_id: ConversationId,
        run_id: OvernightRunId,
        notification_id: String,
        error: String,
    },
    /// The app successfully submitted a run's notification to the OS.
    AckOvernightNotification {
        conversation_id: ConversationId,
        run_id: OvernightRunId,
        notification_id: String,
    },
    /// Stops a worker for good (its unfinished changes are kept, see `Task.kept`).
    StopTask {
        task_id: TaskId,
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
    /// The user did something only they could do (Done on a "Waiting on you" item); the
    /// orchestrator hears it.
    ResolveWaiting {
        conversation_id: ConversationId,
        id: String,
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
    /// Stops its CLI processes and removes temp files now; the next message continues it.
    Hibernate {
        id: ConversationId,
    },
    /// Stops workers, removes everything the conversation created (worktrees, CLI session
    /// files, processes, scratch folders) and hides it in the Archived view.
    Archive {
        id: ConversationId,
    },
    /// Brings an archived conversation back; its model restarts from the transcript.
    Restore {
        id: ConversationId,
    },
    /// What removing a project takes with it.
    PreviewRemoveProject {
        id: ProjectId,
    },
    /// Removes a project from Brigadier: its conversations are deleted (as `Delete` does,
    /// without forgetting the Personal Brain), then the picked branches, then its Brain and
    /// code index go to the Trash. Its repository's files and the user's own branches are
    /// never touched. Refused while one of its conversations works.
    RemoveProject {
        id: ProjectId,
        /// Brigadier branches to delete, at the tips the preview showed.
        delete_branches: Vec<BranchChoice>,
        keep_brain: bool,
    },
    /// Permanently removes a conversation and its transcript.
    Delete {
        id: ConversationId,
        /// Also delete its unmerged branches (otherwise they are kept).
        delete_branches: bool,
        /// Also forget what the Project Brain learned only from it (its transcript index always
        /// goes).
        forget_brain: bool,
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
    ReadBlobText {
        hash: String,
    },
    UpdateSettings {
        settings: Settings,
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
    /// Providers (login, models, quota), raw sessions and replayable fixtures.
    GetProviders,
    /// Checks every provider (or only `provider`) again in the background; results arrive as
    /// `providerChecked`.
    RefreshProviders {
        #[serde(default)]
        provider: Option<ProviderKind>,
    },
    /// The Usage page: quota windows with their estimates and history, Brigadier's own use,
    /// routing activity, the merged model list and what outcomes taught routing (in
    /// `projectId`, or in every project).
    GetUsage {
        #[serde(default)]
        project_id: Option<ProjectId>,
    },
    /// What routing would choose for each task category right now, with its explanation, for
    /// a task in `projectId` touching `areas`. Starts nothing.
    PreviewRoutes {
        #[serde(default)]
        project_id: Option<ProjectId>,
        #[serde(default)]
        areas: Vec<Area>,
    },
    /// Asks the repository for a newer model registry now; answers the registry in use after.
    CheckRegistry,
    /// Starts sourced web research after a registry check; returns the job id immediately.
    /// If a refresh is already running, returns that job's id instead.
    RefreshRankings,
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
    ScanStorage,
    /// Removes the picked items of a scan (by the ids it gave them), each checked again first.
    CleanStorage {
        scan_id: String,
        items: Vec<String>,
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
    CreateProject {
        project: Box<Project>,
    },
    UpdateProject {
        project: Box<Project>,
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
    ListMemories {
        memories: Vec<Node>,
    },
    ForgetMemory,
    ExportConventions {
        export: ConventionsExport,
    },
    RunBrainJob {
        job_id: String,
    },
    RebuildIndex,
    GetRepoInfo {
        repo: RepoInfo,
    },
    FindProjects {
        candidates: Vec<ProjectCandidate>,
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
    OpenSetupTerminal {
        terminal: TerminalInfo,
    },
    OpenSideChat {
        conversation: Box<Conversation>,
    },
    WriteTerminal,
    ResizeTerminal,
    CloseTerminal,
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
    },
    GetWorkerDiffs {
        diffs: Vec<WorkerDiff>,
    },
    CreateConversation {
        conversation: Box<Conversation>,
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
    Interrupt,
    Resume,
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
    UndoChanges,
    GetReviewDiff {
        review: ReviewDiff,
    },
    GetGitState {
        state: GitState,
    },
    GetPullRequest {
        pull_request: Option<PullRequest>,
    },
    CommitChanges {
        outcome: CommitOutcome,
    },
    PushChanges {
        branch: String,
    },
    AnswerCard,
    AnswerQuestion,
    DecidePlan,
    ProposeOvernight {
        run: Box<OvernightRun>,
    },
    StartOvernight {
        run: Box<OvernightRun>,
    },
    StopOvernight {
        run: Box<OvernightRun>,
    },
    SteerOvernight {
        run: Box<OvernightRun>,
    },
    ContinueOvernight {
        run: Box<OvernightRun>,
    },
    MergeOvernight {
        run: Box<OvernightRun>,
    },
    GetRunDiff {
        /// Absent before the run has a branch, and for a finished run with nothing verified.
        diff: Option<DiffStat>,
    },
    PendingOvernightNotifications {
        notifications: Vec<PendingRunNotification>,
    },
    AckOvernightNotification,
    FailOvernightNotification,
    StopTask,
    PauseTask,
    ResumeTask,
    RestoreKeptWork {
        outcome: RestoreOutcome,
    },
    ResolveWaiting,
    ListWorkerEvents {
        page: WorkerPage,
    },
    ListOrchestratorLog {
        page: OrchestratorPage,
    },
    ReadArtifact {
        text: ArtifactText,
    },
    SaveArtifact,
    OpenArtifact {
        /// The copy to open.
        path: String,
    },
    Hibernate {
        conversation: Box<Conversation>,
    },
    Archive {
        conversation: Box<Conversation>,
    },
    Restore {
        conversation: Box<Conversation>,
    },
    PreviewRemoveProject {
        removal: Box<ProjectRemoval>,
    },
    RemoveProject {
        report: RemoveProjectReport,
    },
    Delete {
        /// The space compacting the database now gives back, when Storage would offer it.
        compactable_bytes: Option<u64>,
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
    GetProviders {
        view: ProvidersView,
    },
    RefreshProviders,
    GetUsage {
        usage: Box<UsageView>,
    },
    PreviewRoutes {
        routes: Vec<RoutePreview>,
    },
    CheckRegistry {
        registry: RegistryInfo,
    },
    RefreshRankings {
        job_id: String,
    },
    GetRankingsRefresh {
        refresh: brigadier_core::routing::registry::RankingsRefresh,
    },
    ResetRankings {
        refresh: brigadier_core::routing::registry::RankingsRefresh,
    },
    #[cfg(debug_assertions)]
    DebugInjectLimit,
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
    PreviewUninstall {
        plan: Box<UninstallPlan>,
    },
    Uninstall {
        report: UninstallReport,
    },
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
    /// An overnight run keeps the computer awake (and going with the lid closed, when it can)
    /// until it ends, whatever the settings say.
    #[serde(default)]
    pub for_run: bool,
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
pub struct PendingRunNotification {
    pub conversation_id: ConversationId,
    pub run_id: OvernightRunId,
    pub notification: brigadier_core::overnight::RunNotification,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DaemonActivity {
    /// App connections other than the one asking.
    pub clients: u32,
    /// What runs, said plainly (a turn or a worker, a terminal, a Brain job, …); empty when
    /// nothing does.
    pub running: Vec<String>,
    /// An overnight run is under way: quitting the app leaves the daemon running.
    #[serde(default)]
    pub overnight: bool,
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

/// The daemon's only frame on a gate connection: the answer to its [`ClientFrame::Gate`]
/// check. A gate connection carries nothing else, so this is not a [`ServerFrame`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateVerdict {
    pub allow: bool,
    /// Why the command was denied, for the program's stderr.
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum Outcome {
    Ok { value: Response },
    Err { error: IpcError },
}

impl From<Result<Response, IpcError>> for Outcome {
    fn from(result: Result<Response, IpcError>) -> Self {
        match result {
            Ok(value) => Self::Ok { value },
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
