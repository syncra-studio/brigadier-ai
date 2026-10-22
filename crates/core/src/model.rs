//! Domain model. These types are the wire contract too: they are exported to TypeScript.

use std::collections::HashMap;

use brigadier_providers::{
    Access, Artifact, McpServerState, ModelCatalog, ProviderEvent, ProviderKind, ProviderStatus,
    QuotaSnapshot, TextPhase,
};
use brigadier_router::{
    Area, Explanation, Learned, MergedModel, OverrideRule, ProviderQuota, QuotaSample, RankedPlace,
    Ranking, RegistryInfo, RouteCandidate, TaskCategory,
};

use crate::knowledge::{BrainJob, MemoryChange, RebirthThresholds};
use crate::work::{
    Approval, AttachmentRef, Compaction, MessageQueue, OrchestratorEntry, OrchestratorStep, Plan,
    Question, RunState, Task, UserRequest, WorkerStep,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        // Serde serializes newtypes as their inner string.
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
        pub struct $name(pub String);

        impl $name {
            pub fn generate() -> Self {
                Self(uuid::Uuid::now_v7().to_string())
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id_type!(
    /// Identifies a project.
    ProjectId
);
id_type!(
    /// Identifies a session or a chat.
    ConversationId
);

impl ConversationId {
    /// The last 8 characters (random in a v7 uuid): the session's part of the branch and
    /// folder names Brigadier creates for it.
    pub fn short(&self) -> &str {
        &self.0[self.0.len().saturating_sub(8)..]
    }
}

id_type!(
    /// Identifies a raw provider session (a CLI session driven from the Inspector).
    RawSessionId
);
id_type!(
    /// Identifies a delegated task (and its worker).
    TaskId
);
id_type!(
    /// Identifies a card waiting for the user: an approval, a question or a plan.
    CardId
);
id_type!(
    /// Identifies one segment of an overnight run.
    OvernightRunId
);

id_type!(
    /// Identifies a workspace of the retired workspace layer: only old logs' `Workspace*`
    /// events carry it.
    WorkspaceId
);

impl OvernightRunId {
    /// The last 8 characters: the run's part of its branch and folder names.
    pub fn short(&self) -> &str {
        &self.0[self.0.len().saturating_sub(8)..]
    }
}

/// A project: one or more repos. Owns its sessions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub created_at_ms: i64,
    /// The project's repositories. Sessions work in the first one until multi-repo projects
    /// (Phase 8).
    #[serde(default)]
    pub repos: Vec<ProjectRepo>,
    /// Choices remembered for the project's next session.
    #[serde(default)]
    pub prefs: ProjectPrefs,
    /// The user's answers to "Do you trust this folder?", one per repository folder asked
    /// about. A folder with none is asked about.
    #[serde(default)]
    pub trust: Vec<FolderTrust>,
    /// A session of it merged on its own once: the merge messages after that one stay quiet
    /// about it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub auto_merged: bool,
}

impl Project {
    /// Whether the user trusts the folder `path` (one of the project's repositories), if they
    /// were asked.
    pub fn trusts(&self, path: &str) -> Option<bool> {
        self.trust
            .iter()
            .find(|folder| folder.path == path)
            .map(|folder| folder.trusted)
    }

    /// Whether sessions on the folder `path` work as their level allows: once the user trusts
    /// it, never while it's unanswered. A folder no longer the project's (changed before it was
    /// answered), which the app can't ask about any more, keeps working as its level allows.
    pub fn counts_trusted(&self, path: &str) -> bool {
        match self.trusts(path) {
            Some(trusted) => trusted,
            None => !self.repos.iter().any(|repo| repo.path == path),
        }
    }
}

/// What answering "Do you trust this folder?" did: the project as it is now, and what could
/// not be written in (or removed from) a CLI's settings, by CLI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FolderTrustReport {
    pub project: Project,
    pub failures: Vec<String>,
}

/// The user's answer for one folder. Trusted: agents work there as the session's permission
/// level allows, and the CLIs' own trust prompts are answered for it. Not trusted: every
/// session there runs under Ask for approval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FolderTrust {
    /// The repository's top folder, as the project names it.
    pub path: String,
    pub trusted: bool,
    pub decided_at_ms: i64,
}

/// A git repository in a project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRepo {
    /// Absolute path of the repository's top-level directory (the user's own checkout).
    pub path: String,
    pub name: String,
}

/// What a project remembers from its last session setup, plus its secrets list.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectPrefs {
    /// What earlier versions remembered; no longer written or read: new sessions start at
    /// the default level from Settings. Kept so old stored projects still load.
    pub permission: Option<PermissionLevel>,
    /// Orchestrator provider, model and effort. Absent: the global default.
    pub orchestrator: Option<ModelChoice>,
    pub environment: Option<EnvironmentKind>,
    /// Gitignored env files (paths relative to the repository root) copied into every worker
    /// worktree. Their values are redacted everywhere Brigadier shows or stores text.
    pub secret_files: Vec<String>,
    /// What "Create branch for this session" puts before the name it suggests. Absent:
    /// `brigadier/`.
    pub branch_prefix: Option<String>,
}

/// How much a session may do on its own (PLAN.md §5). Workers never push, publish or deploy
/// at any level: the user starts those.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PermissionLevel {
    /// You approve every plan and every landed change, and each step a worker takes outside
    /// its sandbox. Sandboxed, without network.
    AskForApproval,
    /// Brigadier approves on your behalf and stops only for questions only you can answer.
    /// Sandboxed; the CLI's own reviewer settles what leaves the sandbox.
    #[default]
    ApproveForMe,
    /// Approve for me without the OS sandbox: workers never ask for anything.
    FullAccess,
}

/// A provider, model and reasoning effort, as picked in the composer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ModelChoice {
    pub provider: ProviderKind,
    /// The CLI's model id. Absent: the CLI's own default.
    pub model: Option<String>,
    pub effort: Option<String>,
    /// On the model's fast service tier, where it has one (labeled "Fast").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub fast: Option<bool>,
    /// The provider's account it runs on ([`AccountEntry::id`], or
    /// [`crate::accounts::OWN`] for the user's own login). Absent: the one
    /// [`crate::accounts::select`] picks, recorded here once the conversation starts on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub account: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum EnvironmentKind {
    LocalCheckout,
    NewWorktree,
}

/// Where a session's accepted work lands, as asked for in the composer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum EnvironmentRequest {
    /// Commits land on `branch` in the user's own checkout.
    LocalCheckout {
        branch: String,
        /// "New branch…": create `branch` from this branch or commit first.
        create_from: Option<String>,
    },
    /// The session gets its own worktree on a new branch from `base`, merged back on approval.
    NewWorktree {
        base: String,
        /// The session branch's name. Absent: Brigadier names it (`brigadier/<session>/session`).
        branch: Option<String>,
    },
}

/// A session's environment once set up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Environment {
    LocalCheckout {
        branch: String,
    },
    NewWorktree {
        base: String,
        branch: String,
        /// The session worktree, in Brigadier's data directory. Absent until it is created.
        path: Option<String>,
        /// The commit the session branch starts from (a fork's point). Absent: `base`'s tip.
        #[serde(default)]
        start: Option<String>,
    },
}

/// Where a forked session works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ForkPlace {
    /// In the user's checkout, on a new branch from the fork's point.
    Workspace,
    /// In a worktree of its own, on a new branch from the fork's point.
    NewWorktree,
}

/// The conversation and answer a fork continues from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForkOrigin {
    pub conversation_id: ConversationId,
    /// The answer it was forked from: the last message it copied.
    pub message_id: String,
}

/// A tab's own chat: the chat in a split beside a page, a file or a web page, which belongs
/// to that one tab of a session. It goes, leaving nothing behind, when the tab closes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TabChat {
    /// The tab's id in the app.
    pub tab_id: String,
    pub target: TabTarget,
}

/// What a tab's chat is about, and so what it may change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TabTarget {
    /// A page: the chat may read and change this page, and no other.
    Page { page_id: String },
    /// A file of the session's checkout, by its path inside it: the chat may change this file
    /// and only read the rest.
    File { path: String },
    /// A web page in the built-in browser: the chat reads what the app sends with each
    /// message (the address, the page's text, the selection) and can't use the site.
    Browser,
}

/// What a new conversation is set up with, from the composer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SetupRequest {
    Session {
        /// One of the project's repositories.
        repo: String,
        environment: EnvironmentRequest,
        permission: PermissionLevel,
        orchestrator: ModelChoice,
        /// Start in plan mode (see [`Setup::Session`]).
        #[serde(default)]
        plan_mode: bool,
    },
    Chat {
        model: ModelChoice,
    },
}

/// What the user thought of an answer ("Good response" / "Bad response").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Rating {
    Good,
    Bad,
}

/// How a conversation is set up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Setup {
    Session {
        repo: String,
        environment: Environment,
        permission: PermissionLevel,
        orchestrator: ModelChoice,
        /// Local checkout with uncommitted changes: whether workers start from them. Absent
        /// until the user answered (or when the checkout was clean).
        workers_see_uncommitted: Option<bool>,

        /// Plan mode: the orchestrator plans and changes nothing until the user approves a
        /// plan, whatever the permission level. Approving one turns it off.
        #[serde(default)]
        plan_mode: bool,
    },
    Chat {
        model: ModelChoice,
    },
}

impl Setup {
    /// The setup a composer request asks for. A new-worktree session without a branch name
    /// gets `brigadier/<id.short()>/session`, next to its task branches; the runtime creates
    /// the branch and worktree when the session starts.
    /// `prefix` goes before the name of a new worktree's branch when none was asked for.
    pub fn from_request(request: SetupRequest, id: &ConversationId, prefix: &str) -> Self {
        match request {
            SetupRequest::Session {
                repo,
                environment,
                permission,
                orchestrator,
                plan_mode,
            } => Self::Session {
                repo,
                environment: match environment {
                    EnvironmentRequest::LocalCheckout { branch, .. } => {
                        Environment::LocalCheckout { branch }
                    }
                    EnvironmentRequest::NewWorktree { base, branch } => Environment::NewWorktree {
                        base,
                        branch: branch
                            .filter(|branch| !branch.trim().is_empty())
                            .unwrap_or_else(|| format!("{prefix}{}/session", id.short())),
                        path: None,
                        start: None,
                    },
                },
                permission,
                orchestrator,
                workers_see_uncommitted: None,
                plan_mode,
            },
            SetupRequest::Chat { model } => Self::Chat { model },
        }
    }

    /// The model it runs on: a session's orchestrator, a Chat's model.
    pub fn choice(&self) -> &ModelChoice {
        match self {
            Self::Session { orchestrator, .. } => orchestrator,
            Self::Chat { model } => model,
        }
    }
}

/// Changes to a project; absent fields stay as they are.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectPatch {
    pub name: Option<String>,
    /// Absolute paths of the project's repositories (replaces the list).
    pub repos: Option<Vec<String>>,
    pub prefs: Option<ProjectPrefs>,
}

/// Where a conversation is in its lifecycle (PLAN.md §5). Deleted conversations leave the
/// catalog.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Lifecycle {
    #[default]
    Active,
    /// Idle: its CLI processes stopped and temp files are gone; the next message continues it.
    Hibernated,
    /// Hidden in the Archived view; everything it created was cleaned up. Restorable.
    Archived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ConversationKind {
    /// An orchestrator conversation inside a project.
    Session,
    /// A plain conversation with the picked model, outside any project.
    Chat,
}

/// A session (inside a project) or a chat (outside any project), as listed in the sidebar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: ConversationId,
    pub kind: ConversationKind,
    /// Set for sessions, absent for chats.
    pub project_id: Option<ProjectId>,
    pub title: String,
    /// When it was pinned; absent when not pinned.
    pub pinned_at_ms: Option<i64>,
    pub created_at_ms: i64,
    /// Last activity (creation, rename, pin, message).
    pub updated_at_ms: i64,
    /// Absent for conversations created before setups existed (they get one on first use).
    #[serde(default)]
    pub setup: Option<Setup>,
    #[serde(default)]
    pub lifecycle: Lifecycle,
    /// Set for a fork: where it continues from ("Continued from chat").
    #[serde(default)]
    pub forked_from: Option<ForkOrigin>,
    /// Set for a side chat: the conversation it sits beside, whose latest messages go along
    /// with each of its turns. Side chats are temporary and left out of the sidebar.
    #[serde(default)]
    pub side_of: Option<ConversationId>,
    /// Set for a tab's own chat (with `side_of`, its session): the tab it belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tab_chat: Option<TabChat>,
    /// Set while the conversation's model (a session's orchestrator, a Chat's model) is
    /// replaced because it hit a limit. Temporary: the saved choice in `setup`, the project's
    /// remembered one and the global default are never changed by it.
    #[serde(default)]
    pub fallback: Option<ModelFallback>,
    /// Set while its model is at a limit and no model it may use can stand in: its messages
    /// wait, and go on their own when one can take them.
    #[serde(default)]
    pub quota_wait: Option<crate::work::QuotaWait>,
    /// Set while what it created is still being cleaned up after it was archived: a restart
    /// finishes the cleanup before anything of it runs again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(skip)]
    pub cleanup_pending: bool,
    /// Set once its deletion was asked for: hidden from the app at once, it goes for good when
    /// its cleanup has finished (a restart finishes one cut off).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(skip)]
    pub deleting: bool,
    /// When it last finished answering a message from another session (its sidebar row's
    /// unread dot, SESSION-MESSAGING-PLAN.md §6.4); folded from those events, never stored as
    /// such.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub peer_at_ms: Option<i64>,
    /// When a request of it last finished (its sidebar row's unread dot when that came while
    /// it wasn't open); folded from those events, never stored as such.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub finished_at_ms: Option<i64>,
}

impl Conversation {
    /// Whether it is a session the sidebar lists under its project, a sibling of the project's
    /// other sessions: a session that isn't a side chat or a tab's own chat.
    pub fn is_tab(&self) -> bool {
        self.kind == ConversationKind::Session && self.side_of.is_none() && self.tab_chat.is_none()
    }
}

/// A workspace of the retired workspace layer (a group of a project's sessions). Old logs'
/// `Workspace*` events still carry it; the projection ignores them, and each of its sessions
/// is a plain session of its project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: WorkspaceId,
    pub project_id: ProjectId,
    pub title: String,
    /// The project repositories its tabs work in, set when it is created.
    pub repos: Vec<String>,
    /// The branch its tabs start from and merge into, set when it is created.
    pub base: String,
    pub created_at_ms: i64,
    /// Last activity: creation, rename, pin, or any of its tabs'.
    pub updated_at_ms: i64,
    /// When it was pinned; absent when not pinned.
    pub pinned_at_ms: Option<i64>,
    /// Active or Archived (never Hibernated).
    #[serde(default)]
    pub lifecycle: Lifecycle,
}

/// A conversation's model standing in for the chosen one while that one is at a limit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ModelFallback {
    /// The model standing in.
    pub choice: ModelChoice,
    /// The chosen model it replaces.
    pub replaces: ModelChoice,
    /// Why (a short sentence, as on worker cards).
    pub reason: String,
    pub since_ms: i64,
    /// When the chosen model's limit resets and it takes over again, when known.
    pub until_ms: Option<i64>,
}

/// Something a user message @-mentions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Mention {
    /// A worker of the conversation.
    Task { id: TaskId },
    /// A file of the session's checkout, relative to its root.
    File { path: String },
    /// A folder of the session's checkout, relative to its root.
    Folder { path: String },
    /// Another conversation: its recent messages go along as context.
    Chat { id: ConversationId, title: String },
    /// The page the message was sent from: while it is the user's latest message, the thread
    /// may read and change that page (and no other).
    Page { id: String, title: String },
    /// A Browser tab of the session the user pointed at: `browser_read` reads it.
    BrowserTab {
        tab: String,
        title: String,
        url: String,
    },
}

/// Also reads mentions stored before files and chats could be mentioned: a bare task id.
impl<'de> Deserialize<'de> for Mention {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "camelCase")]
        enum Tagged {
            Task {
                id: TaskId,
            },
            File {
                path: String,
            },
            Folder {
                path: String,
            },
            Chat {
                id: ConversationId,
                title: String,
            },
            Page {
                id: String,
                title: String,
            },
            BrowserTab {
                tab: String,
                title: String,
                url: String,
            },
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Stored {
            Tagged(Tagged),
            Task(TaskId),
        }
        Ok(match Stored::deserialize(deserializer)? {
            Stored::Tagged(Tagged::Task { id }) | Stored::Task(id) => Mention::Task { id },
            Stored::Tagged(Tagged::File { path }) => Mention::File { path },
            Stored::Tagged(Tagged::Folder { path }) => Mention::Folder { path },
            Stored::Tagged(Tagged::Chat { id, title }) => Mention::Chat { id, title },
            Stored::Tagged(Tagged::Page { id, title }) => Mention::Page { id, title },
            Stored::Tagged(Tagged::BrowserTab { tab, title, url }) => {
                Mention::BrowserTab { tab, title, url }
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum MessageRole {
    User,
    Assistant,
    System,
    /// Another session's message ([`Message::peer`] says which): never the user's word.
    Peer,
}

/// Where a session's peer message came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PeerRef {
    pub message_id: String,
    /// The session that sent it.
    pub from: ConversationId,
    /// The message of this session's it answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reply_to: Option<String>,
}

/// Where a message between sessions stands (SESSION-MESSAGING-PLAN.md §4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PeerState {
    /// Under Ask for approval: its card waits for the user's Send.
    WaitingForOk,
    /// The user's Don't send.
    Declined,
    /// Recorded for the receiver, not yet in its inbox.
    Pending,
    /// In the receiver's inbox.
    Delivered,
    /// The receiver's CLI took it: a turn started with it, or a steer of it was acknowledged.
    Seen,
    /// A reply to it came.
    Replied,
    /// It was never sent.
    Refused { reason: String },
    /// It was sent and will never be delivered.
    Dropped { reason: String },
}

/// A message between this session and another, as this side recorded it, for the thread's
/// rows and blocks (SESSION-MESSAGING-PLAN.md §6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PeerMessage {
    pub message_id: String,
    /// This session sent it (else it received it).
    pub outgoing: bool,
    /// The other session: where it went, or where it came from.
    pub peer: ConversationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reply_to: Option<String>,
    /// Sent: the request whose turn sent it. Received: the request it is filed under.
    pub request_id: Option<String>,
    pub state: PeerState,
    /// Sent: its text (the start of it, when `blob` holds the rest). Received: empty, as the
    /// transcript message holds it.
    pub text: String,
    pub blob: Option<String>,
    /// Sent: when it was allowed to go.
    pub sent_at_ms: Option<i64>,
    /// Sent: the thread no longer waits for its reply.
    pub settled: bool,
    /// Where it was first recorded in the conversation's stream.
    pub position: i64,
    /// A reply Brigadier sent for this session: its answer to the message it replies to.
    #[serde(default)]
    pub automatic: bool,
    /// An automatic reply: the answer message it carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub answer_id: Option<String>,
    /// A received reply: where it stands with a wait of this session's thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hold: Option<ReplyHold>,
}

/// Where a reply stands with a wait of its sender's thread (`wait_for_replies`): handed to
/// the waiting call as its result, taken once the CLI shows that call finished, or returned
/// to ordinary delivery when the call ended without taking it (a Stop, a restart).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ReplyHold {
    Handed,
    /// `item_id` is the wait's tool call in the thread's work.
    Taken {
        item_id: String,
    },
    Returned,
}

impl PeerState {
    /// The receiver still has to get it.
    pub fn due(&self) -> bool {
        matches!(self, Self::Pending | Self::Delivered)
    }
}

/// One message in a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub conversation_id: ConversationId,
    /// Position in the conversation, starting at 1.
    pub seq: i64,
    pub role: MessageRole,
    /// The text, or its first part when `blob` is set.
    pub text: String,
    /// Content hash of the full text when it was too large to keep inline.
    pub blob: Option<String>,
    pub created_at_ms: i64,
    #[serde(default)]
    pub attachments: Vec<AttachmentRef>,
    /// What the message @-mentions: workers, files, other conversations, the page it was sent from.
    #[serde(default)]
    pub mentions: Vec<Mention>,
    /// For assistant messages: the model that wrote it (a Chat may fall back to another).
    #[serde(default)]
    pub model: Option<ModelChoice>,
    /// The user request it belongs to: its own id for a user message, the request the
    /// model was serving for a reply. Absent for messages from before requests existed.
    #[serde(default)]
    pub request_id: Option<String>,
    /// The message before it on its branch; empty for a first message that replaced another
    /// (an edit). Absent for messages from before branches existed: those follow the message
    /// before them.
    #[serde(default)]
    pub parent_id: Option<String>,
    /// A peer message's sender (its role is [`MessageRole::Peer`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub peer: Option<Box<PeerRef>>,
    /// For an assistant message its CLI wrote as the turn's final answer: when it started, so
    /// the turn's work folds and its time stops there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub answer_at_ms: Option<i64>,
}

/// How commit messages or pull requests are written (Settings › Git).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum GitStyle {
    /// Like the repository's recent history.
    #[default]
    Repository,
    /// By the Conventional Commits rules (`type(scope): subject`).
    Conventional,
    /// By the user's own instructions.
    Custom,
}

/// The branch prefix unless the user sets another.
pub const DEFAULT_BRANCH_PREFIX: &str = "brigadier/";

/// When Brigadier keeps the computer from sleeping.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum KeepAwake {
    /// Normal system sleep.
    Off,
    /// While a turn or a worker is running.
    #[default]
    Agents,
    /// Always, while Brigadier runs.
    Always,
}

/// What a message sent while an answer works does by default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FollowUp {
    /// It waits its turn: a Chat queues it, a session's orchestrator sorts it.
    #[default]
    Queue,
    /// It goes into the running answer now.
    Steer,
}

/// When a desktop notification says a request is finished.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum NotifyFinished {
    /// Only while no Brigadier window is in front.
    #[default]
    Background,
    Always,
    Never,
}

/// Where a kind of web link opens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum LinkTarget {
    /// A Browser tab in the session in front (the system browser when there is none).
    Brigadier,
    /// The user's default browser.
    #[default]
    System,
}

/// One project's exceptions to the user's switches, by catalog id: what it turns on for
/// itself that is off by default, and what it turns off that is on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSwitches {
    pub project_id: ProjectId,
    #[serde(default)]
    pub servers_on: Vec<String>,
    #[serde(default)]
    pub servers_off: Vec<String>,
    #[serde(default)]
    pub skills_on: Vec<String>,
    #[serde(default)]
    pub skills_off: Vec<String>,
}

impl Settings {
    /// Whether the server `id` is off for sessions in `project`: off by default and not turned
    /// on there, or turned off there.
    pub fn server_off(&self, id: &str, project: Option<&ProjectId>) -> bool {
        let switches = self.switches(project);
        if self.mcp_servers_off.iter().any(|off| off == id) {
            !switches.is_some_and(|s| s.servers_on.iter().any(|on| on == id))
        } else {
            switches.is_some_and(|s| s.servers_off.iter().any(|off| off == id))
        }
    }

    /// Whether the skill `id` is off for sessions in `project`, as [`Self::server_off`].
    pub fn skill_off(&self, id: &str, project: Option<&ProjectId>) -> bool {
        let switches = self.switches(project);
        if self.skills_off.iter().any(|off| off == id) {
            !switches.is_some_and(|s| s.skills_on.iter().any(|on| on == id))
        } else {
            switches.is_some_and(|s| s.skills_off.iter().any(|off| off == id))
        }
    }

    fn switches(&self, project: Option<&ProjectId>) -> Option<&ProjectSwitches> {
        let project = project?;
        self.project_switches
            .iter()
            .find(|switches| &switches.project_id == project)
    }
}

/// User settings persisted by the core.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Permission level for new sessions without an explicit choice.
    /// Older saved records without this field keep their former default.
    #[serde(default = "saved_default_permission")]
    pub default_permission: PermissionLevel,
    /// Orchestrator model for projects that have not remembered one. Absent: the first
    /// logged-in provider's default model.
    pub default_orchestrator: Option<ModelChoice>,
    /// Model for new Chats. Absent: as for the orchestrator.
    pub default_chat_model: Option<ModelChoice>,
    /// A conversation with nothing running hibernates after this many idle minutes.
    pub hibernate_after_minutes: u32,
    /// The composer shows how full the model's context is (a ring by the model picker).
    pub show_context_usage: bool,
    /// The composer keeps code, Markdown and links as literal text instead of showing them
    /// formatted as they are typed (Settings › General › Composer).
    pub plain_text_composer: bool,
    /// A notice above the composer while a conversation is in Full access.
    pub show_full_access_notice: bool,
    /// Spend quota left over before a usage window resets on deepening the Project Brains.
    pub enrich_brain: bool,
    /// The old Short replies setting, no longer read: the voice is fixed (BUILD-PLAN §1.4). A
    /// stored `false` is copied once into the personal preference for longer answers, and then
    /// saved as `true`.
    pub short_replies: bool,
    /// The thread starts workers when they help, saying so in one line first; off, it asks
    /// first. A worker the user asks for starts either way (BUILD-PLAN §1.4).
    #[serde(default = "default_true")]
    pub start_workers_on_own: bool,
    /// Co-authored-by trailers that name an AI are left out of commit messages: the user's
    /// own commits, what the agents write, and the commits Brigadier lands or keeps.
    #[serde(default = "default_true")]
    pub omit_ai_coauthors: bool,
    /// The model that writes the git text Brigadier writes itself (a commit message the user
    /// left blank). Absent: Automatic, routing's pick for git text.
    pub git_writer: Option<ModelChoice>,
    /// How commit messages are written: the generated ones and the agents' own.
    pub commit_style: GitStyle,
    /// The user's instructions for commit messages, kept while another style is chosen.
    pub commit_instructions: String,
    /// How an agent writes a pull request's title and body, when the user asks it to open one.
    pub pr_style: GitStyle,
    /// The user's instructions for pull requests, kept while another style is chosen.
    pub pr_instructions: String,
    /// What goes before the names of the branches Brigadier makes (sessions, forks, workers)
    /// and the name Create branch suggests, unless the project has its own. Empty: none.
    pub branch_prefix: String,
    /// Another vendor's model reviews the sessions' work in the background: each landing, the
    /// thread's own commits, a lead's outline, and a worker's own change when it asks. Off, no
    /// review starts and nothing waits for one (THREAD-PLAN.md §2 Q12).
    #[serde(default = "default_true")]
    pub review_work: bool,
    /// The tab merges its work on its own once it judges it ready: a pull request on GitHub,
    /// or into the base here, with no consent asked. Off (the default), it says the work is
    /// ready and the user merges. A user who turned it on keeps it on; settings saved before
    /// this field existed, or that never set it, read as off.
    #[serde(default)]
    pub auto_merge: bool,
    /// The first-run setup (welcome, name, agents, then projects) was finished or skipped.
    pub onboarded: bool,
    /// The first launch's welcome intro was played through to its Continue, so a setup left
    /// halfway picks up after it.
    pub intro_seen: bool,
    /// What the user asked to be called (first-run setup, Settings › General). Empty: not given.
    pub user_name: String,
    /// When the computer is kept from sleeping.
    pub keep_awake: KeepAwake,
    /// While kept awake, closing the lid doesn't sleep the computer either (macOS: sleep is
    /// disabled system-wide, then restored).
    pub keep_awake_lid_closed: bool,
    /// What a message sent while an answer works does; ⌥Enter does the opposite for one
    /// message.
    pub follow_up: FollowUp,
    /// When a finished request raises a desktop notification. Overnight runs tell the user
    /// with their report's notification instead.
    pub notify_finished: NotifyFinished,
    /// Approvals and questions waiting for the user raise a desktop notification.
    #[serde(default = "default_true")]
    pub notify_needs_you: bool,
    /// The user's routing rules, global and per project. They always win over the router's
    /// scores and quota balancing.
    pub routing_overrides: Vec<OverrideRule>,
    /// The user's manual rankings per kind of work (global and per project, with area
    /// overrides): where one is Manual, routing tries its models top-down instead of scoring.
    pub routing_rankings: Vec<Ranking>,
    /// Agents switched off on the Providers page: their models are hidden from every picker
    /// and get no work, not even in the background. Conversations already running go on.
    pub disabled_providers: Vec<ProviderKind>,
    /// The binary the user chose for an agent on its Providers page, used instead of the one on
    /// the login PATH while it is there. At most one per agent.
    pub agent_paths: Vec<AgentPath>,
    /// Models the Providers page makes unavailable: hidden from every picker and never used.
    pub hidden_models: Vec<ModelRef>,
    /// Every model Brigadier has seen in its agent's list. One seen after its agent's first
    /// list starts without worker tasks (a "Don't give work" rule, which the user removes in
    /// Settings → Providers).
    pub known_models: Vec<ModelRef>,
    /// Older models Brigadier has turned off once, when it first saw them: from then on the
    /// user's switch decides.
    pub older_models_off: Vec<ModelRef>,
    /// The user's extra CLI accounts (Settings → Accounts), in the order they were added. The
    /// user's own login, the one their terminal uses, is each provider's first account and is
    /// not listed.
    pub accounts: Vec<AccountEntry>,
    /// The one switch saved before account switching was set per agent (settings version 4):
    /// read once into [`Self::manual_accounts`] by [`crate::routing::availability::migrate`],
    /// never saved again.
    #[serde(rename = "switchAccounts", skip_serializing)]
    #[ts(skip)]
    pub legacy_switch_accounts: bool,
    /// Agents whose accounts don't switch on their own (their switch off, or an account picked
    /// with Use): their new work starts on their default account. Every other agent's new work
    /// starts on its account whose weekly window resets soonest, and work whose account hits
    /// its usage limit carries on with the next such account.
    pub manual_accounts: Vec<ProviderKind>,
    /// Work moves to another agent when its own can't take it (every account it may use at
    /// its limit). Off: it waits for the limit to reset.
    pub switch_providers: bool,
    /// The language dictation listens for, as the speech engine names it ("en", "de"…). Empty:
    /// it is detected from the speech.
    pub dictation_language: String,
    /// Words and names dictation should recognise, given to the speech engine as a hint.
    pub dictation_words: Vec<String>,
    /// The microphone dictation records from (the webview's device id). Empty: the system's
    /// default.
    pub dictation_microphone: String,
    /// Brigadier's built-in browser exists (Settings › Browser). Off, there is no Browser tab,
    /// shortcut or menu item, links open in the default browser, and sessions are told.
    pub browser_enabled: bool,
    /// Agents can see and drive apps on this Mac (Settings › Computer use): every worker and
    /// thread gets the `computer` tools and the `computer-use` skill. macOS only.
    pub computer_use: bool,
    /// While computer use is on, a menu-bar item lists the agents using apps on this Mac, with
    /// a way to stop each.
    pub computer_use_menu_bar: bool,
    /// Where web links open while the browser is on.
    pub web_links_in: LinkTarget,
    /// Where local development URLs (localhost and loopback addresses) open while the browser
    /// is on.
    pub local_links_in: LinkTarget,
    /// The Browser tab's address bar shows the whole URL, not only the site.
    pub browser_full_url: bool,
    /// Where the built-in browser saves downloads. Empty: the system's Downloads folder.
    pub browser_downloads_dir: String,
    /// The built-in browser asks where to save each download.
    pub browser_ask_downloads: bool,
    /// MCP servers switched off for Brigadier, by catalog id (`claude:user:railway`): no thread
    /// or worker gets them, their agent's own included. A change takes effect at each
    /// session's next start or resume.
    pub mcp_servers_off: Vec<String>,
    /// Skills switched off for Brigadier, by catalog id (`agents:user:<folder>`), as for
    /// [`Self::mcp_servers_off`].
    pub skills_off: Vec<String>,
    /// A project's exceptions to [`Self::mcp_servers_off`] and [`Self::skills_off`], one entry
    /// per project that has any. The project's threads and workers go by them.
    pub project_switches: Vec<ProjectSwitches>,
    /// Servers from a project's own config that the user uses in every project, by catalog id:
    /// workers of every other project get them too, by the switches above, while a project's
    /// own item otherwise stays in its project. Read only while that project's folder is
    /// trusted.
    pub servers_everywhere: Vec<String>,
    /// Skills from a project's own config used in every project, as
    /// [`Self::servers_everywhere`]. Apart from it: a project's server and skill can share an id.
    pub skills_everywhere: Vec<String>,
    /// The shape saved settings were last brought up to ([`SETTINGS_VERSION`]). Settings saved
    /// before it existed read as 0, so their conversions run.
    #[serde(default)]
    pub settings_version: u32,
}

/// Every account of each provider, for Settings → Accounts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountsView {
    /// Each provider's own login first, then its extra accounts as added.
    pub accounts: Vec<AccountView>,
    /// Weekly quota about to be lost, as warned of, until its reset.
    #[serde(default)]
    pub warnings: Vec<QuotaWarning>,
}

/// An account's weekly quota about to be lost unused: its reset 12 hours away or less with
/// 20% or more left (Q60). Told once per window per reset; nothing starts on its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWarning {
    pub account: crate::accounts::AccountRef,
    /// How the app names it: the agent ("Claude Code"), or an extra account by its name with
    /// the agent ("Work (Claude Code)").
    pub who: String,
    /// The window's id.
    pub window: String,
    /// Share of the window still unused, 0–100.
    pub left_percent: f64,
    pub resets_at_ms: i64,
}

/// One account as Settings → Accounts shows it. Nothing secret: who is logged in, as the CLI
/// says, and its quota.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub account: crate::accounts::AccountRef,
    /// The user's name for an extra account; empty for their own login.
    pub name: String,
    /// Its provider's default: with switching off, work on that provider runs here.
    pub default: bool,
    /// New work of its provider starts on it now, as the engine chooses (switching on: the
    /// soonest weekly reset among those that can take work): the provider's lead account,
    /// whose quota the Usage menu shows.
    pub in_use: bool,
    /// Its login, as the CLI last reported it; unknown until first checked.
    pub status: Option<ProviderStatus>,
    /// Its quota now; unknown until first read.
    pub quota: Option<QuotaSnapshot>,
    /// When its quota was last read or reported.
    pub quota_at_ms: Option<i64>,
    /// When its login was last checked.
    pub checked_at_ms: Option<i64>,
    /// Being checked now.
    pub checking: bool,
    /// What went wrong when last checked.
    pub error: Option<String>,
    /// The check generation its last finished check began in: a refresh answers a generation,
    /// and the account is checked afresh for it once this reaches it.
    pub checked_generation: u32,
}

/// The binary the user chose for an agent (Settings › Providers › its page › Location).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AgentPath {
    pub provider: ProviderKind,
    /// Absolute path of the binary.
    pub path: String,
}

/// An extra account of a CLI: a home of its own under the data directory
/// (`accounts/<id>`), where the CLI keeps that account's login. Brigadier keeps no secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountEntry {
    pub id: String,
    pub provider: ProviderKind,
    /// The user's name for it.
    pub name: String,
    /// Its provider's default (the user's own login when none is): with switching off, work
    /// on that provider runs here.
    #[serde(default)]
    pub default: bool,
    pub added_at_ms: i64,
}

impl Settings {
    /// `provider`'s default account: `None` for the user's own login.
    pub fn default_account(&self, provider: ProviderKind) -> Option<&AccountEntry> {
        self.accounts
            .iter()
            .find(|account| account.provider == provider && account.default)
    }

    /// Whether `provider`'s new work picks its account on its own (else it runs on the
    /// default account).
    pub fn switches_accounts(&self, provider: ProviderKind) -> bool {
        !self.manual_accounts.contains(&provider)
    }

    /// The extra account `id` of `provider`.
    pub fn account(&self, provider: ProviderKind, id: &str) -> Option<&AccountEntry> {
        self.accounts
            .iter()
            .find(|account| account.provider == provider && account.id == id)
    }
}

/// A setting that is on unless the user turned it off, also in settings saved before it
/// existed.
fn default_true() -> bool {
    true
}

/// Preserve the default of saved settings that predate an explicit permission field.
/// Fresh stores use [`Settings::default`] instead.
fn saved_default_permission() -> PermissionLevel {
    PermissionLevel::FullAccess
}

/// The settings' shape: each step up converts saved settings once (see
/// [`crate::routing::availability::migrate`]).
pub const SETTINGS_VERSION: u32 = 5;

/// A model named by its CLI's id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ModelRef {
    pub provider: ProviderKind,
    pub id: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_permission: PermissionLevel::ApproveForMe,
            default_orchestrator: None,
            default_chat_model: None,
            hibernate_after_minutes: 30,
            show_context_usage: true,
            plain_text_composer: false,
            show_full_access_notice: true,
            enrich_brain: true,
            short_replies: true,
            start_workers_on_own: true,
            omit_ai_coauthors: true,
            git_writer: None,
            commit_style: GitStyle::default(),
            commit_instructions: String::new(),
            pr_style: GitStyle::default(),
            pr_instructions: String::new(),
            branch_prefix: DEFAULT_BRANCH_PREFIX.to_owned(),
            review_work: true,
            auto_merge: false,
            onboarded: false,
            intro_seen: false,
            user_name: String::new(),
            keep_awake: KeepAwake::default(),
            keep_awake_lid_closed: false,
            follow_up: FollowUp::default(),
            notify_finished: NotifyFinished::default(),
            notify_needs_you: true,
            routing_overrides: Vec::new(),
            routing_rankings: Vec::new(),
            disabled_providers: Vec::new(),
            agent_paths: Vec::new(),
            hidden_models: Vec::new(),
            known_models: Vec::new(),
            older_models_off: Vec::new(),
            accounts: Vec::new(),
            legacy_switch_accounts: true,
            manual_accounts: Vec::new(),
            switch_providers: true,
            dictation_language: String::new(),
            dictation_words: Vec::new(),
            dictation_microphone: String::new(),
            browser_enabled: true,
            computer_use: true,
            computer_use_menu_bar: true,
            web_links_in: LinkTarget::System,
            local_links_in: LinkTarget::Brigadier,
            browser_full_url: false,
            browser_downloads_dir: String::new(),
            browser_ask_downloads: false,
            mcp_servers_off: Vec::new(),
            skills_off: Vec::new(),
            project_switches: Vec::new(),
            servers_everywhere: Vec::new(),
            skills_everywhere: Vec::new(),
            settings_version: SETTINGS_VERSION,
        }
    }
}

/// Everything the sidebar needs, in one read.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub projects: Vec<Project>,
    pub conversations: Vec<Conversation>,
    pub settings: Settings,
}

/// A page of messages, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MessagePage {
    pub messages: Vec<Message>,
    /// Whether older messages exist before the first one returned.
    pub has_more: bool,
}

/// A message a search found its text in, and how many times.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MessageMatch {
    pub id: String,
    pub seq: i64,
    pub count: u32,
}

/// The messages a search found its text in, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MessageMatches {
    pub matches: Vec<MessageMatch>,
    /// It stopped counting at the limit: older messages may hold more.
    pub more: bool,
}

/// Text an assistant is still writing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct StreamingMessage {
    pub message_id: String,
    pub text: String,
    /// The request the running turn serves.
    pub request_id: Option<String>,
    /// What it is, when its CLI said: a line between steps, or the turn's final answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub phase: Option<TextPhase>,
    /// When it started.
    #[serde(default)]
    pub started_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    pub level: brigadier_providers::NoticeLevel,
    pub text: String,
    pub at_ms: i64,
}

/// Why work is waiting for the machine. Old stored rows described every hold as heat.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum MachineStepReason {
    #[default]
    Heat,
    Memory,
}

impl MachineStepReason {
    /// Heat takes precedence when both limits hold work.
    pub(crate) fn from_load(load: brigadier_sandbox::MachineLoad) -> Self {
        if load.heat >= brigadier_sandbox::Heat::Serious {
            Self::Heat
        } else {
            Self::Memory
        }
    }
}

/// A grey thread row about the machine (PLAN.md §10.7): a worker or a build waiting for the
/// machine to ease, or for another build; a build paused for the heat, or going on again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MachineStep {
    pub kind: MachineStepKind,
    #[serde(default)]
    pub reason: MachineStepReason,
    /// The request the worker (or the orchestrator's turn) serves.
    #[serde(default)]
    pub request_id: Option<String>,
    /// The worker it is about; absent for the orchestrator's own commands.
    #[serde(default)]
    pub task_id: Option<crate::work::TaskId>,
    /// The command (`cargo test -p brigadier-core`); absent for a worker waiting to start.
    #[serde(default)]
    pub command: Option<String>,
    pub at_ms: i64,
    /// Where it happened in the conversation's stream (set when the board reads it).
    #[serde(default)]
    pub position: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum MachineStepKind {
    /// "Waiting for the Mac to cool down": the machine is hot or short on memory.
    WaitingToCool,
    /// "Waiting for another build to finish": one build or test run goes at a time.
    WaitingForBuild,
    /// "Paused {command} to let the Mac cool down".
    Paused,
    /// "Resumed {command}".
    Resumed,
}

/// How full the conversation model's context is, as its CLI last said.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    pub used_tokens: i64,
    /// Absent when the CLI does not say.
    pub window_tokens: Option<i64>,
}

/// What `/status` shows for a conversation: its model's CLI session and the usage left.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConversationStatus {
    /// The CLI serving the conversation's model (a session's orchestrator).
    pub provider: ProviderKind,
    /// Its own id for the session (Claude session, Codex thread); absent until it first ran.
    pub native_id: Option<String>,
    /// The provider's usage windows, as it last reported them.
    pub quota: Option<QuotaSnapshot>,
}

/// A provider's reasoning summary, kept in order with the turn's actions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingSegment {
    pub item_id: String,
    pub request_id: Option<String>,
    pub text: String,
    pub position: i64,
    pub started_at_ms: i64,
    pub updated_at_ms: i64,
    /// Last delta folded, so events arriving during a snapshot read are applied only once.
    pub through_position: i64,
    pub complete: bool,
}

/// Everything a conversation view shows, in one read. Live changes follow on the
/// conversation's event stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConversationView {
    pub conversation: Conversation,
    /// How full its model's context is; absent until its CLI first said.
    pub context: Option<ContextUsage>,
    /// The newest page of messages.
    pub messages: MessagePage,
    pub tasks: Vec<Task>,
    pub approvals: Vec<Approval>,
    pub questions: Vec<Question>,
    pub plans: Vec<Plan>,
    /// Every request of the conversation, oldest first.
    pub requests: Vec<UserRequest>,
    /// Every worker step, in the order they happened.
    pub worker_steps: Vec<WorkerStep>,
    /// Every orchestrator step, in the order they happened.
    pub orchestrator_steps: Vec<OrchestratorStep>,
    /// Every row about the machine (waiting for it to cool down, builds paused and resumed),
    /// in the order they happened.
    #[serde(default)]
    pub machine_steps: Vec<MachineStep>,
    #[serde(default)]
    pub thinking: Vec<ThinkingSegment>,
    /// Every compaction of a Chat's context, in the order they happened.
    pub compactions: Vec<Compaction>,
    /// What was decided on the user's behalf, in the order it was decided.
    pub decisions: Vec<crate::work::Decision>,
    /// What only the user can do and is not done yet, oldest first.
    pub waiting: Vec<crate::work::WaitingItem>,
    pub queue: MessageQueue,
    pub run: RunState,
    /// The request the running turn serves.
    pub run_request: Option<String>,
    /// The last message of the branch the thread shows (the newest message until the user
    /// edits, regenerates or switches branches).
    pub head: Option<String>,
    /// The user's ratings of answers and plans, by subject (see `DomainEvent::MessageRated`).
    pub ratings: HashMap<String, Rating>,
    pub streaming: Option<StreamingMessage>,
    /// The latest notices (environment problems, fallbacks), newest last.
    pub notices: Vec<Notice>,
    /// What a Chat's model saved to the Personal Brain (its Memory chips): the latest change
    /// per memory, in the order they were saved; one the user removed is `forgotten`.
    pub memories: Vec<crate::knowledge::MemoryChange>,
    /// Its overnight runs, a segment each, oldest first.
    pub overnight: Vec<crate::overnight::OvernightRun>,
    /// Its one-shot reviews, oldest first.
    #[serde(default)]
    pub reviews: Vec<crate::work::ReviewRun>,
    /// Its previews, oldest first.
    #[serde(default)]
    pub previews: Vec<crate::work::Preview>,
    /// Its runs of the full checks on the integrated tree, oldest first.
    #[serde(default)]
    pub integrated_checks: Vec<crate::work::IntegratedCheck>,
    /// The thread's commands that went to the background, oldest first.
    #[serde(default)]
    pub background_runs: Vec<crate::work::BackgroundRun>,
    /// What the session is doing now, in the thread's words; none once the user wrote since.
    #[serde(default)]
    pub headline: Option<crate::work::SessionHeadline>,
    /// Why the last merge into the base was refused, while it stands.
    #[serde(default)]
    pub merge_refused: Option<crate::work::MergeRefused>,
    /// The ended previews and background runs the user dismissed or restarted from the
    /// Background rows (Q94), by id.
    #[serde(default)]
    pub background_cleared: Vec<String>,
    /// The MCP servers of its agents whose CLI runs now, its own model's first, then each
    /// worker's by task id.
    #[serde(default)]
    pub mcp_servers: Vec<AgentMcpServers>,
    /// Its messages to and from other sessions, in the order they were recorded.
    #[serde(default)]
    pub peer_messages: Vec<PeerMessage>,
}

/// The MCP servers one agent of a conversation has, as its CLI last told
/// ([`DomainEvent::McpServersUpdated`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AgentMcpServers {
    /// The worker's task; absent for the conversation's own model.
    pub task_id: Option<String>,
    pub provider: ProviderKind,
    pub servers: Vec<McpServerState>,
    /// What its CLI said when it started, not kept current since (Claude).
    pub at_start: bool,
}

/// A branch, for the composer's branch picker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    pub name: String,
    pub commit: String,
    /// The worktree that has it checked out (the user's checkout or another), if any.
    pub checked_out_at: Option<String>,
}

/// A repository's state, for the composer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RepoInfo {
    /// The repository's top-level directory.
    pub path: String,
    pub name: String,
    /// The branch checked out in the user's checkout; absent when detached.
    pub current_branch: Option<String>,
    pub branches: Vec<BranchInfo>,
    /// The user's checkout has uncommitted changes (tracked or untracked).
    pub dirty: bool,
}

/// A repository the user's own CLI sessions worked in, offered as a project on first run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCandidate {
    /// The repository's top-level directory (the main checkout, for a linked worktree).
    pub path: String,
    /// `owner/name` from its `origin` remote, else the folder's name.
    pub name: String,
    /// The CLIs that worked in it.
    pub providers: Vec<ProviderKind>,
    /// How many of their sessions ran in it.
    pub sessions: u32,
    pub last_active_ms: i64,
    /// The project that already has this repository.
    pub project_id: Option<ProjectId>,
}

/// A git repository on this computer, offered in the Add project dialog's list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RepoSuggestion {
    /// The repository's top-level folder (absolute).
    pub path: String,
    /// The folder's name.
    pub name: String,
    /// When it was last worked in: the newest of its git activity and the user's own CLI
    /// sessions there.
    pub last_used_ms: i64,
    /// The project that already has this repository.
    pub project_id: Option<ProjectId>,
}

/// A folder offered while a path is typed in the Add project dialog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FolderEntry {
    pub name: String,
    /// Absolute path.
    pub path: String,
    /// It is the top-level folder of a git repository.
    pub repo: bool,
    /// The project that already has this repository.
    pub project_id: Option<ProjectId>,
}

/// The folders inside the folder a typed path points into whose names start with the path's
/// last part, by name.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FolderListing {
    /// The folder listed (absolute), empty when the path points into none.
    pub dir: String,
    pub entries: Vec<FolderEntry>,
    /// Set when more folders matched than were listed.
    pub truncated: bool,
}

/// What adding a folder as a project does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FolderCheck {
    /// The folder is in a git repository, and the project works in its top-level folder
    /// `root`. `nested` when that is not the folder itself (a subfolder, a linked worktree).
    Repo {
        root: String,
        name: String,
        nested: bool,
        /// The project that already has this repository.
        project_id: Option<ProjectId>,
    },
    /// A folder in no repository: adding it runs `git init` there first.
    Plain { path: String, name: String },
    /// Nothing is there yet: adding it creates the folder and a repository in it.
    Missing { path: String, name: String },
    /// It can't be a project.
    Invalid { reason: String },
}

/// A page of a worker's transcript, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkerPage {
    pub entries: Vec<RawEntry>,
    pub has_more: bool,
}

/// A page of the orchestrator log, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorPage {
    pub entries: Vec<OrchestratorLogEntry>,
    pub has_more: bool,
    /// The conversation's rebirth thresholds for its current model (sessions only).
    pub thresholds: Option<RebirthThresholds>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorLogEntry {
    pub stream_seq: i64,
    pub at_ms: i64,
    pub entry: OrchestratorEntry,
}

/// A thread's tool call or command in full, as its row opens to: what it was given, what it
/// returned (the whole stored output when the model got a digest), how a command exited and
/// how long it took. Nothing is set when the log holds no such item.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ThreadItem {
    pub input: Option<String>,
    pub output: Option<String>,
    pub exit: Option<i32>,
    pub ms: Option<i64>,
}

/// A page of a thread's own tool calls in one request (or one of its turns), oldest first:
/// each call's newest event, as a worker's transcript holds it, with its whole output when it
/// is stored (a digest's or a clipped command's), and a running command's output so far.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ThreadItemsPage {
    pub entries: Vec<RawEntry>,
    /// The `after` that reads the next page; `None` on the last.
    pub next: Option<i64>,
}

/// Where a raw session's events come from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RawSource {
    /// A live CLI process.
    Live,
    /// A recording replayed through the adapter's parser.
    Replay { title: String },
    /// Simulated CLI output fed through the adapter's parser (diagnostics).
    Simulation { title: String },
}

/// Who answers a raw session's approval requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RawApprovals {
    /// Brigadier's policy answers what stays inside the session's access; the user the rest.
    Delegated,
    /// Brigadier declines every request (a read-only session such as an orchestrator).
    DeclineAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RawState {
    Starting,
    Running,
    /// The CLI process ended; its CLI session is kept and can be resumed.
    Stopped,
    /// It could not start; whatever it created was removed.
    Failed,
    /// Being disposed of: the process is ending and its files are being removed.
    Closing,
    /// Everything it created is gone. The transcript stays in Brigadier.
    Closed,
}

/// A raw provider session: one CLI session driven directly, for debugging adapters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RawSession {
    pub id: RawSessionId,
    pub provider: ProviderKind,
    pub source: RawSource,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub access: Access,
    pub approvals: RawApprovals,
    /// The CLI's own session or thread id, once known.
    pub native_id: Option<String>,
    /// The session this one was forked from.
    pub parent_id: Option<RawSessionId>,
    pub state: RawState,
    pub error: Option<String>,
    /// File the raw stdio exchange is recorded to.
    pub recording: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// What Brigadier last learned about a provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProviderOverview {
    pub provider: ProviderKind,
    pub status: Option<ProviderStatus>,
    /// Live model list, or the cached one until the first refresh.
    pub models: Option<ModelCatalog>,
    pub quota: Option<QuotaSnapshot>,
    /// The quota monitor's view: every window with its rolling estimate and heat.
    #[serde(default)]
    pub usage: Option<ProviderQuota>,
    /// Why the last refresh could not complete.
    pub error: Option<String>,
    pub checked_at_ms: Option<i64>,
}

/// What can be updated: Brigadier itself or an agent CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum UpdateTarget {
    App,
    Claude,
    Codex,
}

/// What the Update button does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum UpdateAction {
    /// Opens the release's download page.
    Download { url: String },
    /// Brigadier runs `command` (shown as written) with the login shell's environment.
    Run { command: String },
    /// Brigadier can't update this install; the user runs `command` themselves.
    Manual { command: String },
}

/// How an update Brigadier runs is going.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum UpdateProgress {
    Available,
    Updating,
    Updated,
    Failed { error: String },
}

/// A newer version of Brigadier or an agent CLI that can be installed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UpdateItem {
    pub target: UpdateTarget,
    /// The version installed (after an update, the one it moved to).
    pub current: String,
    pub latest: String,
    pub action: UpdateAction,
    pub progress: UpdateProgress,
}

/// The step of Connect that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ConnectStep {
    Install,
    SignIn,
}

/// Where Connect is with an agent: its own installer runs when it is missing, then its own
/// sign-in, unless it is signed in already.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ConnectProgress {
    Installing,
    /// The vendor's sign-in page is open in the browser; it is checked every few seconds.
    SigningIn,
    Connected,
    /// `error` is one plain sentence for the user; Connect again retries. `reason`, when set,
    /// is why Connect couldn't do it at all, which the card says how to get past.
    Failed {
        step: ConnectStep,
        error: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        reason: Option<ConnectFailure>,
    },
}

/// Why Connect couldn't do a step at all, so the user does it themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ConnectFailure {
    /// On macOS or Linux, neither curl nor wget is installed to download the vendor's
    /// installer: the card links the agent's install guide.
    NoDownloader,
}

/// One agent's Connect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AgentConnect {
    pub provider: ProviderKind,
    pub progress: ConnectProgress,
    /// While it signs in: the page that shows a code to paste back into the sign-in (Claude
    /// Code's), for when the browser can't hand the sign-in back by itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub code_page: Option<String>,
}

/// Every agent Connect ran for since launch: one cancelled is left out, and one connected
/// once its overview is in.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConnectView {
    pub agents: Vec<AgentConnect>,
}

/// The newer versions found by the last check, and any update running or just finished.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UpdatesView {
    pub items: Vec<UpdateItem>,
    pub checked_at_ms: Option<i64>,
}

/// Brigadier's own use of an account over the Usage page's Activity range.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountTokens {
    /// Tokens by model, most first.
    pub by_model: Vec<TokenCount>,
    /// Tokens by conversation, most first (the top few).
    pub by_conversation: Vec<ConversationTokens>,
}

/// How a session's thread works, to tune its instructions by (THREAD-PLAN.md Q13; the
/// Inspector). Nothing is held back by it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ThreadMetrics {
    pub conversation_id: ConversationId,
    /// The commits the thread made itself; absent before its branch was first looked at.
    pub edits: Option<ThreadEdits>,
    /// Its context per user request, oldest first: only requests it made a model call for.
    pub requests: Vec<RequestContext>,
    /// The context its latest model call read, when known.
    pub context_tokens: Option<i64>,
    /// Each user request's time and tokens, oldest first.
    pub summaries: Vec<RequestSummary>,
}

/// One user request's time and tokens, everything it started included (THREAD-PLAN.md
/// phase 4; the Inspector). The times are in ms after the request was sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RequestSummary {
    pub request_id: String,
    /// The start of the user's message, on one line.
    pub preview: String,
    pub started_at_ms: i64,
    /// The first thing the thread did for it: its first step or first finished model call.
    pub first_event_ms: Option<i64>,
    /// Its answer: when it last stopped working (absent while it works).
    pub answer_ms: Option<i64>,
    /// How it ended when that wasn't an answer: stopped by the user, failed, or cut off by a
    /// quit (absent when it answered, and while it works).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ended: Option<RequestEnd>,
    /// Its last landing.
    pub landed_ms: Option<i64>,
    /// The last use of a model by anything it started, reviews after the answer included
    /// (absent while it works).
    pub settled_ms: Option<i64>,
    /// Tokens per provider, most first.
    pub providers: Vec<ProviderTokens>,
    /// Tokens per kind of step (thread, worker, review…), most first.
    pub steps: Vec<StepTokens>,
    /// Where its cache writes came from.
    pub cache: CacheCauses,
}

/// Where a request's cache writes came from (BUILD-PLAN B0, research P0.1), from each model
/// call's prompt against the one before it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CacheCauses {
    /// An estimate: what the calls didn't read back of the prompt before them.
    pub missed: i64,
    /// Calls that missed more than 20K.
    pub big_misses: u32,
    /// Calls with nothing to compare with (a new session's first, the first after a
    /// compaction), reported apart.
    pub unestimated_calls: u32,
    /// Per known cause of a session start, most written first.
    pub causes: Vec<CauseTokens>,
    /// The estimated miss no known cause explains.
    pub unexplained: i64,
}

/// What the sessions started for one cause wrote and missed with their first call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CauseTokens {
    /// "note fork", "cold resume", "fresh start"…
    pub cause: String,
    pub sessions: u32,
    pub written: i64,
    /// An estimate.
    pub missed: i64,
}

/// How a request ended without an answer (the Inspector's request report).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RequestEnd {
    Stopped,
    Failed,
    /// Brigadier quit while its turn ran (UI audit H1).
    Interrupted,
}

/// What one provider's models read and wrote for a request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTokens {
    pub provider: ProviderKind,
    pub input: i64,
    pub cached_input: i64,
    pub cache_write: i64,
    /// Of `cache_write`, what was written for an hour and for five minutes (Claude).
    pub cache_write_1h: i64,
    pub cache_write_5m: i64,
    pub output: i64,
    /// All four together.
    pub raw: i64,
    /// `raw` without cache reads.
    pub raw_without_cache_reads: i64,
    /// What it cost, when the provider says (Claude); absent when no use said.
    pub cost_usd: Option<f64>,
}

/// The tokens of one kind of step for a request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct StepTokens {
    /// `thread`, `worker`, `review`, `guardian`…
    pub step: String,
    pub raw: i64,
    /// Its model calls (a use stored before they were counted counts as one).
    pub calls: u32,
    /// Its turns, where they are counted (the thread's, a review's).
    pub turns: u32,
}

/// The commits on a session's branch marked `Brigadier-Author: thread`, from where the thread
/// first looked at the branch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ThreadEdits {
    pub branch: String,
    pub commits: u32,
    pub added: i64,
    pub removed: i64,
    /// When they were counted.
    pub at_ms: i64,
    /// The branch is gone (merged and removed): these are its last numbers.
    pub kept: bool,
}

/// The thread's context over one user request: at its first and last model call for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RequestContext {
    pub request_id: String,
    /// The start of the user's message, on one line (empty when the request is gone).
    pub preview: String,
    pub started_at_ms: i64,
    /// The thread's model calls for it, counted as the request's summary counts them.
    pub calls: u32,
    pub first_tokens: i64,
    pub last_tokens: i64,
    /// `last_tokens - first_tokens`.
    pub growth_tokens: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TokenCount {
    pub model: String,
    /// Input, cached input and output together.
    pub tokens: i64,
    pub output_tokens: i64,
    pub turns: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConversationTokens {
    pub conversation_id: ConversationId,
    pub project_id: Option<ProjectId>,
    pub title: String,
    pub tokens: i64,
}

/// A window's samples over a span, for the Usage page's chart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WindowHistory {
    pub window_id: String,
    pub label: String,
    /// Oldest first.
    pub samples: Vec<QuotaSample>,
}

/// One provider on the Usage page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsage {
    pub provider: ProviderKind,
    /// Its accounts: the user's own login first, then the extra ones as added.
    pub accounts: Vec<AccountUsage>,
    /// What balancing does about it now ("New scouting and research go to Codex while
    /// Claude's weekly window runs hot").
    pub balancing: Option<String>,
}

/// One account on the Usage page: its own windows, read from that account alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsage {
    pub account: crate::accounts::AccountRef,
    /// A chat's turn or a worker runs on it now.
    pub working: bool,
    /// Its windows with their estimates; absent until its first read.
    pub quota: Option<ProviderQuota>,
    /// When its quota was last read or reported; absent before the first.
    pub read_at_ms: Option<i64>,
}

/// The Usage page's Activity: each account's windows over the last days, and Brigadier's own
/// use of it then.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UsageActivity {
    /// Where the range starts.
    pub since_ms: i64,
    pub accounts: Vec<AccountActivity>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountActivity {
    pub account: crate::accounts::AccountRef,
    /// Its windows' samples since the range's start, the shortest window first.
    pub history: Vec<WindowHistory>,
    pub tokens: AccountTokens,
}

/// Something routing did that the user may want to know about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RoutingActivity {
    /// A task moved to another model mid-way.
    Handoff {
        conversation_id: ConversationId,
        task_id: crate::work::TaskId,
        title: String,
        from: ModelChoice,
        to: ModelChoice,
        /// Why ("Claude's 5-hour window ran out").
        cause: String,
        at_ms: i64,
    },
    /// A task waits for quota.
    Waiting {
        conversation_id: ConversationId,
        task_id: crate::work::TaskId,
        title: String,
        reason: String,
        resets_at_ms: Option<i64>,
        since_ms: i64,
    },
    /// A conversation's model stands in for the chosen one.
    Fallback {
        conversation_id: ConversationId,
        title: String,
        fallback: ModelFallback,
    },
}

/// Everything the Usage page shows, in one read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UsageView {
    pub providers: Vec<ProviderUsage>,
    /// Newest first; hand-offs from the last 7 days, and everything waiting now.
    pub activity: Vec<RoutingActivity>,
    /// Every model the CLIs offer, merged with the registry, research and trials.
    pub models: Vec<MergedModel>,
    /// What outcomes taught routing in `project_id` (every project when absent).
    pub learned: Vec<Learned>,
    pub project_id: Option<ProjectId>,
    pub registry: RegistryInfo,
}

/// What routing would choose for one category right now, for the next task submitted (the
/// Routing page and the Inspector's routing preview). Nothing is started and no trial slot is
/// taken.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RoutePreview {
    pub category: TaskCategory,
    pub areas: Vec<Area>,
    pub outcome: RoutePreviewOutcome,
    /// Every model weighed: the chosen one first, then the order routing would try the others
    /// in, then those that can't take the task now.
    pub candidates: Vec<RouteCandidate>,
    /// The ranking that applies here (Manual or Automatic), if the user has one.
    pub ranking_id: Option<String>,
    /// The manual ranking in force, place by place; empty when routing scores.
    pub places: Vec<RankedPlace>,
    /// The next task of this kind holds a trial slot (a new model may try it).
    pub trial_slot: bool,
    /// For a review asked without an author: what it would run on for each vendor's change,
    /// since a review goes to the other vendor from the author when one can take it.
    #[serde(default)]
    pub by_author: Vec<AuthorPreview>,
}

/// What a review would run on when `author`'s vendor wrote the change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AuthorPreview {
    pub author: ProviderKind,
    pub outcome: RoutePreviewOutcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RoutePreviewOutcome {
    Chosen {
        choice: ModelChoice,
        reason: String,
        explanation: Option<Explanation>,
    },
    /// Nothing it may use is available: a task would wait.
    Wait {
        reason: String,
        resets_at_ms: Option<i64>,
        rule: Option<String>,
        ranking: Option<String>,
    },
}

/// A replayable recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Fixture {
    /// `builtin:<name>` for fixtures shipped with Brigadier, `recording:<file>` for recordings
    /// made from the Inspector.
    pub id: String,
    pub title: String,
    pub provider: ProviderKind,
    pub cli_version: Option<String>,
    pub lines: u32,
}

/// Everything the Inspector's Providers tab shows, in one read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProvidersView {
    pub providers: Vec<ProviderOverview>,
    /// Newest first.
    pub sessions: Vec<RawSession>,
    pub fixtures: Vec<Fixture>,
}

/// One entry of a raw session's transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RawEntry {
    pub stream_seq: i64,
    pub at_ms: i64,
    pub event: ProviderEvent,
}

/// A page of a raw session's transcript, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RawPage {
    pub entries: Vec<RawEntry>,
    pub has_more: bool,
}

/// A branch Brigadier created and left in place, and where its work was meant to go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct KeptBranch {
    pub name: String,
    /// The branch its work lands on (a task's target, a session's base).
    pub target: String,
    /// Its commit when it was kept.
    pub tip: String,
}

/// Every change the core records. Serialized as the payload of a stored event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DomainEvent {
    ProjectCreated {
        project: Project,
    },
    ConversationCreated {
        conversation: Box<Conversation>,
    },
    ConversationRenamed {
        id: ConversationId,
        title: String,
    },
    /// Legacy, from the retired workspace layer: still read from old logs, never recorded, and
    /// ignored (each of its sessions is a plain session of its project). So are the three
    /// after it.
    WorkspaceCreated {
        workspace: Workspace,
    },
    WorkspaceRenamed {
        id: WorkspaceId,
        title: String,
    },
    WorkspacePinned {
        id: WorkspaceId,
        pinned_at_ms: Option<i64>,
    },
    /// Archived (`archived`) or restored.
    WorkspaceArchived {
        id: WorkspaceId,
        archived: bool,
    },
    ConversationPinned {
        id: ConversationId,
        pinned_at_ms: Option<i64>,
    },
    /// A stand-in model took over the conversation because its chosen one hit a limit
    /// (`fallback` set), or the chosen one took over again (`fallback` absent).
    ConversationFallback {
        id: ConversationId,
        fallback: Option<ModelFallback>,
    },
    ConversationWaiting {
        id: ConversationId,
        wait: Option<crate::work::QuotaWait>,
    },
    MessageAppended {
        message: Message,
    },
    SettingsChanged {
        settings: Box<Settings>,
    },
    RawSessionCreated {
        session: RawSession,
    },
    RawSessionUpdated {
        id: RawSessionId,
        state: RawState,
        native_id: Option<String>,
        error: Option<String>,
    },
    /// Something a raw session's CLI reported.
    RawEvent {
        session_id: RawSessionId,
        event: ProviderEvent,
    },
    /// An artifact a CLI session created, recorded before it is relied on. Owners are raw
    /// session ids, or `orch:`, `chat:`, `task:` and `session:` followed by an id.
    CleanupRecorded {
        owner: String,
        artifact: Artifact,
    },
    /// Branches Brigadier created in `repo` that a deleted conversation or removed project
    /// left in place (unmerged work the user kept): what proves them Brigadier's, so Storage
    /// can offer them later, while they still point where they did.
    BranchesKept {
        repo: String,
        branches: Vec<KeptBranch>,
    },
    /// Brigadier created `branch` in `repo` (a session's, a fork's or a worker's): what proves
    /// it Brigadier's to delete later, whatever the branch prefix was or is.
    BranchMade {
        repo: String,
        branch: String,
    },
    /// These artifacts of `owner` are gone.
    CleanupRemoved {
        owner: String,
        artifacts: Vec<Artifact>,
    },
    /// Everything `owner` created is to be removed; what fails is retried at the next launch.
    CleanupRequested {
        owner: String,
        /// The user's Close workspace asked for it (a session's own worktree), so a restart
        /// that takes it back says it didn't finish.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        #[ts(skip)]
        close: bool,
    },
    /// An empty disposal finished. Artifacts recorded meanwhile remain owned.
    CleanupFinished {
        owner: String,
    },
    /// Legacy completion: forgets every artifact of `owner` and ends its disposal.
    CleanupCompleted {
        owner: String,
        failures: Vec<String>,
    },
    /// Model ratings changed; reload Usage, model summaries and route previews.
    RankingsChanged,
    ProviderChecked {
        overview: ProviderOverview,
    },
    /// Settings → Accounts changed: an account was added, removed or checked, or its quota
    /// moved (full snapshot).
    AccountsChecked {
        accounts: AccountsView,
    },
    /// An account's weekly quota is about to be lost unused: one desktop notification.
    QuotaWillBeLost {
        warning: QuotaWarning,
    },
    /// What can be updated changed: a check finished, or an update started or ended.
    UpdatesChanged {
        updates: UpdatesView,
    },
    /// Connect moved on for an agent: it started, installed, signed in, failed or was cancelled.
    AgentConnectChanged {
        connect: ConnectView,
    },
    /// A project's name, repositories or remembered choices changed (full snapshot).
    ProjectUpdated {
        project: Project,
    },
    ConversationSetUp {
        id: ConversationId,
        setup: Setup,
    },
    ConversationLifecycleChanged {
        id: ConversationId,
        lifecycle: Lifecycle,
    },
    /// The cleanup after an archive started (`pending`), or finished.
    ConversationCleanup {
        id: ConversationId,
        pending: bool,
    },
    /// Its deletion was asked for: it is gone for the user; its cleanup goes on until
    /// [`Self::ConversationDeleted`].
    ConversationDeleting {
        id: ConversationId,
    },
    /// Permanently removed; its streams are purged.
    ConversationDeleted {
        id: ConversationId,
    },
    /// Removed from Brigadier (its conversations were deleted first). Its repository is untouched.
    ProjectRemoved {
        id: ProjectId,
    },
    /// The first start of `engine` began deleting the conversations made before it (THREAD-PLAN
    /// Q14): exactly these, also after a restart cut it off, until [`Self::EngineSwitched`].
    EngineSwitching {
        engine: String,
        conversations: Vec<ConversationId>,
    },
    /// The store belongs to `engine`: its first start is over and does not run again.
    EngineSwitched {
        engine: String,
    },
    /// Assistant text as it streams. The final `messageAppended` with the same id replaces it.
    MessageDelta {
        conversation_id: ConversationId,
        message_id: String,
        text: String,
        /// What the reply is, when its CLI says (the first delta of it says).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        phase: Option<TextPhase>,
    },
    /// Reasoning text in timeline order. A complete event replaces the accumulated deltas.
    ThinkingDelta {
        conversation_id: ConversationId,
        item_id: String,
        request_id: Option<String>,
        text: String,
        at_ms: i64,
        complete: bool,
    },
    RunStateChanged {
        conversation_id: ConversationId,
        state: RunState,
        error: Option<String>,
        /// The request the turn serves.
        #[serde(default)]
        request_id: Option<String>,
    },
    /// A user request started, or its state changed (full snapshot).
    RequestUpdated {
        request: UserRequest,
    },
    /// A session's request settled with its branch ahead of its base at `head`: the work is
    /// ready to merge (a structured fact, not read from the answer's words).
    RequestReady {
        conversation_id: ConversationId,
        request_id: String,
        head: String,
    },
    /// What a session branch's pull request on GitHub was last seen as (none: it has none
    /// now); recorded when that changed. Its row's "PR open". Boxed: every event is the size of
    /// its largest kind.
    PullRequestSeen {
        conversation_id: ConversationId,
        pull_request: Option<Box<crate::work::PullRequest>>,
    },
    /// A worker started, finished, waits for the user, and so on.
    WorkerStepped {
        step: WorkerStep,
    },
    /// The orchestrator messaged a worker, read a report, and so on.
    OrchestratorStepped {
        step: OrchestratorStep,
    },
    /// Work waits for the machine, or a build was paused or resumed for its heat.
    MachineStepped {
        conversation_id: ConversationId,
        step: MachineStep,
    },
    /// A Chat's model began compacting its context, or finished (full snapshot).
    CompactionUpdated {
        compaction: Compaction,
    },
    /// The user rated an answer or a plan. Ratings stay on this machine.
    MessageRated {
        /// The answer: a message id, or `task:<id>` for a worker's report; `plan:<id>` for a plan.
        subject: String,
        rating: Rating,
    },
    /// The user took back their rating of `subject` (as in `MessageRated`).
    RatingCleared {
        subject: String,
    },
    /// The thread now shows the branch that ends at `head`; new messages continue it.
    BranchSwitched {
        conversation_id: ConversationId,
        head: String,
    },
    ConversationNotice {
        conversation_id: ConversationId,
        notice: Notice,
    },
    /// A task was created or changed (full snapshot).
    TaskUpdated {
        task: Box<Task>,
    },
    ApprovalUpdated {
        approval: Approval,
    },
    QuestionUpdated {
        question: Question,
    },
    PlanUpdated {
        plan: Plan,
    },
    /// A one-shot review started or ended (full snapshot).
    ReviewUpdated {
        review: crate::work::ReviewRun,
    },
    /// A run of the full checks on the integrated tree became due, started or ended (full
    /// snapshot).
    IntegratedCheckUpdated {
        check: crate::work::IntegratedCheck,
    },
    /// The thread set the session's headline (`set_headline`), or it was cleared (none) by
    /// the user's next message.
    HeadlineSet {
        conversation_id: ConversationId,
        headline: Option<crate::work::SessionHeadline>,
    },
    /// A merge into the base was refused by the merge rule, or (none) the refusal is over: a
    /// merge went through, or the user said to merge anyway.
    MergeRefusalSet {
        conversation_id: ConversationId,
        refusal: Option<crate::work::MergeRefused>,
    },
    /// A command of the thread's went to the background, or ended there (full snapshot).
    BackgroundRunUpdated {
        run: crate::work::BackgroundRun,
    },
    /// The MCP servers one of the conversation's agents has, as its CLI tells them, Brigadier's
    /// own left out (full snapshot; none: its CLI ended or restarts). `taskId`: the worker's;
    /// absent for the conversation's own model. `boot`: the daemon process that heard it
    /// ([`crate::boot_id`]); one from an earlier process is shown no more.
    McpServersUpdated {
        conversation_id: ConversationId,
        task_id: Option<String>,
        provider: ProviderKind,
        servers: Vec<McpServerState>,
        at_start: bool,
        boot: String,
    },
    /// The thread's commits on `branch` were looked at up to `tip`: what follows it is new
    /// (THREAD-PLAN.md Q4, Q12).
    ThreadCommitsSeen {
        conversation_id: ConversationId,
        branch: String,
        tip: String,
        /// The thread made the commits from this one to `tip` itself (in its turn, or with its
        /// trailer): the record of who wrote them, since a commit it makes on the user's own
        /// checkout carries no trailer (UI audit M6).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        authored_from: Option<String>,
    },
    /// The thread's changes no review covered yet moved on, or a review covered them
    /// (BUILD-PLAN B2; full snapshot, `None`: there are none).
    UnreviewedChanged {
        conversation_id: ConversationId,
        range: Option<crate::work::UnreviewedRange>,
    },
    /// A command output of the thread's was stored whole; the model got a digest of it, or
    /// (a failing Claude `Bash` call) the CLI's excerpt (THREAD-PLAN.md Q4).
    OutputStored {
        conversation_id: ConversationId,
        output: crate::work::StoredOutput,
    },
    /// A `run_check` call ran its command, or answered from the check cache (THREAD-PLAN.md
    /// Q8 lever 3, [`crate::manager::checks`]).
    CheckRan {
        conversation_id: ConversationId,
        /// The worker's task; none for the thread.
        task_id: Option<TaskId>,
        /// The git tree of the checkout the command ran on, uncommitted changes included;
        /// none when the cache was bypassed before it was known.
        tree: Option<String>,
        command: String,
        /// Where it ran, relative to the repository's root ("" for the root itself).
        workdir: String,
        /// Answered from the cache: nothing ran.
        cached: bool,
        /// Why the cache was left out (the tree or an input couldn't be identified): it ran,
        /// and its result was not kept.
        bypassed: Option<String>,
        /// "exit 0", "exit 1", "timed out after 600 s".
        status: String,
        /// How long it ran (for a cached result, how long it ran then).
        duration_ms: u64,
        /// The `out-…` alias of its stored output, which `read_artifact` reads. It is also the
        /// check's evidence reference: a report or a claim cites it (BUILD-PLAN E1, E2).
        artifact: Option<String>,
        /// The hash of the check's whole input fingerprint (the tree, the command, its workdir,
        /// configuration, lockfiles, toolchains and declared inputs); none when an input
        /// couldn't be identified.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        fingerprint: Option<String>,
    },
    /// What the session's thread read and searched in one turn, its tool calls' in order
    /// (THREAD-PLAN.md Q8 lever 1; [`crate::manager::reads`]).
    ThreadLooked {
        conversation_id: ConversationId,
        reads: Vec<crate::work::ThreadRead>,
        searches: Vec<crate::work::ThreadSearch>,
    },
    /// A preview started, ended or got a new log snapshot (full snapshot).
    PreviewUpdated {
        preview: crate::work::Preview,
    },
    /// Removes only the named ended previews; their artifacts remain readable.
    PreviewsCleared {
        conversation_id: ConversationId,
        ids: Vec<String>,
    },
    /// Ended previews and background runs (by id) that leave the context card's Background
    /// rows: the user's dismiss or Restart of a failed one (Q94). The records stay.
    BackgroundCleared {
        conversation_id: ConversationId,
        ids: Vec<String>,
    },
    QueueChanged {
        conversation_id: ConversationId,
        queue: MessageQueue,
    },
    /// Something a worker's CLI reported.
    WorkerEvent {
        task_id: TaskId,
        event: ProviderEvent,
    },
    /// A file a worker's tool result named, copied later than the result was seen: before the
    /// CLI's session folder could go (BUILD-PLAN D2.2). `itemId` is the result's.
    OutputKept {
        task_id: TaskId,
        item_id: String,
        file: brigadier_providers::KeptFile,
    },
    OrchestratorLogged {
        conversation_id: ConversationId,
        entry: OrchestratorEntry,
    },
    /// The attachments a composer draft holds. Blob collection keeps what an event mentions and
    /// the draft's stream keeps only its latest event, so they stay stored until the draft is
    /// sent or discarded (an empty list).
    DraftPinned {
        scope: String,
        attachments: Vec<AttachmentRef>,
    },
    /// A Brain job started or changed (full snapshot), on the project's `brain:` stream.
    BrainJobUpdated {
        job: BrainJob,
    },
    /// A Chat's model saved a memory to the Personal Brain, or the user removed it (a Memory
    /// chip in the thread).
    MemoryUpdated {
        conversation_id: ConversationId,
        memory: MemoryChange,
    },
    /// Something was decided on the user's behalf ("Decided for you").
    DecidedForYou {
        decision: crate::work::Decision,
    },
    /// Something only the user can do was listed, or reworded (full snapshot).
    WaitingOnYou {
        item: crate::work::WaitingItem,
    },
    /// An overnight run was proposed, started or changed (full snapshot).
    OvernightUpdated {
        run: Box<crate::overnight::OvernightRun>,
    },
    /// A "Waiting on you" item is done.
    WaitingResolved {
        id: String,
        by: crate::work::ResolvedBy,
    },
    /// This session sent a message to another (SESSION-MESSAGING-PLAN.md §4), or under Ask
    /// for approval asked the user to send it. The text is whole: its start, and the rest in
    /// `blob` when it is too large to keep inline.
    PeerMessageSent {
        message_id: String,
        to: ConversationId,
        text: String,
        #[serde(default)]
        blob: Option<String>,
        #[serde(default)]
        reply_to: Option<String>,
        chain_id: String,
        hop: u32,
        /// The sender's request whose turn sent it.
        request_id: Option<String>,
        /// The newest input of that turn before the send.
        #[serde(default)]
        turn_input_id: Option<String>,
        state: PeerState,
        /// When it was allowed to go: now, or the user's Send under Ask for approval.
        #[serde(default)]
        sent_at_ms: Option<i64>,
        /// Brigadier sent it for this session: its answer to the message it replies to.
        #[serde(default)]
        automatic: bool,
        /// An automatic reply: the answer message it carries.
        #[serde(default)]
        answer_id: Option<String>,
    },
    /// This session got a message from another. Its text is the transcript message with the
    /// same id, appended with it.
    PeerMessageReceived {
        message_id: String,
        from: ConversationId,
        #[serde(default)]
        reply_to: Option<String>,
        chain_id: String,
        hop: u32,
        /// The request of this session's it is filed under.
        request_id: String,
        /// A reply Brigadier sent for the other session: its answer.
        #[serde(default)]
        automatic: bool,
        /// An automatic reply: the other session's answer message it carries.
        #[serde(default)]
        answer_id: Option<String>,
    },
    /// A message between sessions moved on; written to both sides.
    PeerMessageState {
        message_id: String,
        state: PeerState,
        /// Set when this change let it go (the user's Send).
        #[serde(default)]
        sent_at_ms: Option<i64>,
    },
    /// The thread no longer waits for a reply to this message (`settle_wait`).
    PeerWaitSettled {
        message_id: String,
    },
    /// A request of the conversation finished: its answer is written, or it failed (its
    /// sidebar row's unread dot when the user wasn't looking).
    RequestFinished {
        conversation_id: ConversationId,
        request_id: String,
    },
    /// This session's answer to another session's message went back to it, as the reply
    /// Brigadier sent for it: the session finished what it was asked (its sidebar row's unread
    /// dot, SESSION-MESSAGING-PLAN.md §6.4).
    PeerAnswered {
        conversation_id: ConversationId,
        message_id: String,
    },
    /// A reply this session received moved on with a wait of its thread (`wait_for_replies`).
    PeerReplyHeld {
        message_id: String,
        hold: ReplyHold,
    },
    /// Diagnostic probe used to measure ingest → paint latency end to end.
    Probe {
        burst_id: String,
        index: u32,
        count: u32,
    },
    /// A stored event of a kind Brigadier no longer has (a removed feature's): read back as
    /// this and ignored, so old data still loads. Never written.
    #[serde(other)]
    #[ts(skip)]
    Retired,
}

impl DomainEvent {
    /// The stored event `kind`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ProjectCreated { .. } => "project.created",
            Self::ConversationCreated { .. } => "conversation.created",
            Self::ConversationRenamed { .. } => "conversation.renamed",
            Self::WorkspaceCreated { .. } => "workspace.created",
            Self::WorkspaceRenamed { .. } => "workspace.renamed",
            Self::WorkspacePinned { .. } => "workspace.pinned",
            Self::WorkspaceArchived { .. } => "workspace.archived",
            Self::ConversationPinned { .. } => "conversation.pinned",
            Self::ConversationFallback { .. } => "conversation.fallback",
            Self::ConversationWaiting { .. } => "conversation.waiting",
            Self::MessageAppended { .. } => "message.appended",
            Self::SettingsChanged { .. } => "settings.changed",
            Self::RawSessionCreated { .. } => "raw.created",
            Self::RawSessionUpdated { .. } => "raw.updated",
            Self::RawEvent { .. } => "raw.event",
            Self::CleanupRecorded { .. } => "cleanup.recorded",
            Self::CleanupRemoved { .. } => "cleanup.removed",
            Self::BranchesKept { .. } => "branches.kept",
            Self::BranchMade { .. } => "branch.made",
            Self::CleanupRequested { .. } => "cleanup.requested",
            Self::CleanupCompleted { .. } => "cleanup.completed",
            Self::CleanupFinished { .. } => "cleanup.finished",
            Self::RankingsChanged => "rankings.changed",
            Self::ProviderChecked { .. } => "provider.checked",
            Self::AccountsChecked { .. } => "accounts.checked",
            Self::QuotaWillBeLost { .. } => "quota.willBeLost",
            Self::UpdatesChanged { .. } => "updates.changed",
            Self::AgentConnectChanged { .. } => "agent.connectChanged",
            Self::ProjectUpdated { .. } => "project.updated",
            Self::ConversationSetUp { .. } => "conversation.setUp",
            Self::ConversationLifecycleChanged { .. } => "conversation.lifecycle",
            Self::ConversationCleanup { .. } => "conversation.cleanup",
            Self::ConversationDeleting { .. } => "conversation.deleting",
            Self::ConversationDeleted { .. } => "conversation.deleted",
            Self::ProjectRemoved { .. } => "project.removed",
            Self::EngineSwitching { .. } => "engine.switching",
            Self::EngineSwitched { .. } => "engine.switched",
            Self::MessageDelta { .. } => "message.delta",
            Self::ThinkingDelta { .. } => "thinking.delta",
            Self::RunStateChanged { .. } => "conversation.run",
            Self::RequestUpdated { .. } => "request.updated",
            Self::RequestReady { .. } => "request.ready",
            Self::PullRequestSeen { .. } => "pull_request.seen",
            Self::WorkerStepped { .. } => "worker.step",
            Self::OrchestratorStepped { .. } => "orchestrator.step",
            Self::MachineStepped { .. } => "machine.step",
            Self::CompactionUpdated { .. } => "compaction.updated",
            Self::MessageRated { .. } => "message.rated",
            Self::RatingCleared { .. } => "rating.cleared",
            Self::BranchSwitched { .. } => "conversation.branch",
            Self::ConversationNotice { .. } => "conversation.notice",
            Self::TaskUpdated { .. } => "task.updated",
            Self::ApprovalUpdated { .. } => "approval.updated",
            Self::QuestionUpdated { .. } => "question.updated",
            Self::PlanUpdated { .. } => "plan.updated",
            Self::ReviewUpdated { .. } => "review.updated",
            Self::IntegratedCheckUpdated { .. } => "integrated_check.updated",
            Self::BackgroundRunUpdated { .. } => "background_run.updated",
            Self::HeadlineSet { .. } => "headline.set",
            Self::MergeRefusalSet { .. } => "merge.refusal",
            Self::McpServersUpdated { .. } => "mcp_servers.updated",
            Self::ThreadCommitsSeen { .. } => "thread.seen",
            Self::UnreviewedChanged { .. } => "thread.unreviewed",
            Self::OutputStored { .. } => "output.stored",
            Self::CheckRan { .. } => "check.ran",
            Self::ThreadLooked { .. } => "thread.looked",
            Self::PreviewUpdated { .. } => "preview.updated",
            Self::PreviewsCleared { .. } => "previews.cleared",
            Self::BackgroundCleared { .. } => "background.cleared",
            Self::QueueChanged { .. } => "queue.changed",
            Self::WorkerEvent { .. } => "worker.event",
            Self::OutputKept { .. } => "output.kept",
            Self::OrchestratorLogged { .. } => "orchestrator.logged",
            Self::DraftPinned { .. } => "draft.pinned",
            Self::BrainJobUpdated { .. } => "brain.job",
            Self::MemoryUpdated { .. } => "memory.updated",
            Self::DecidedForYou { .. } => "decision.made",
            Self::WaitingOnYou { .. } => "waiting.updated",
            Self::WaitingResolved { .. } => "waiting.resolved",
            Self::OvernightUpdated { .. } => "overnight.updated",
            Self::PeerMessageSent { .. } => "peer.sent",
            Self::PeerMessageReceived { .. } => "peer.received",
            Self::PeerMessageState { .. } => "peer.state",
            Self::PeerWaitSettled { .. } => "peer.settled",
            Self::PeerReplyHeld { .. } => "peer.held",
            Self::PeerAnswered { .. } => "peer.answered",
            Self::RequestFinished { .. } => "request.finished",
            Self::Probe { .. } => "diag.probe",
            Self::Retired => "retired",
        }
    }
}

/// Event streams. Catalog changes share one stream so the sidebar can be rebuilt in order;
/// each conversation's messages get their own.
pub mod streams {
    use super::ConversationId;

    pub const CATALOG: &str = "catalog";
    pub const SETTINGS: &str = "settings";
    pub const DIAGNOSTICS: &str = "diag";
    /// Raw sessions: creation and state changes.
    pub const RAW: &str = "raw";
    /// The cleanup ledger of every CLI session.
    pub const CLEANUP: &str = "cleanup";
    pub const PROVIDERS: &str = "providers";
    /// Settings → Accounts snapshots.
    pub const ACCOUNTS: &str = "accounts";
    /// Weekly quota about to be lost, as warned of (read again at launch: no warning twice).
    pub const QUOTA_WARNINGS: &str = "quota-warnings";
    /// Newer versions of Brigadier and the agent CLIs.
    pub const UPDATES: &str = "updates";
    /// Connect's progress with each agent.
    pub const CONNECT: &str = "connect";

    pub fn raw_session(id: &super::RawSessionId) -> String {
        format!("raw:{id}")
    }

    /// A tab's own chat's catalog events, kept apart from [`CATALOG`] so that, once its tab
    /// closes, removing this stream leaves the catalog with nothing of it.
    pub fn tab_chat(id: &ConversationId) -> String {
        format!("tabchat:{id}")
    }

    /// Where every tab chat's catalog stream starts.
    pub const TAB_CHATS: &str = "tabchat:";

    /// A conversation's messages, tasks, cards, queue and run state.
    pub fn conversation(id: &ConversationId) -> String {
        format!("conversation:{id}")
    }

    /// A worker's CLI events.
    pub fn task(id: &super::TaskId) -> String {
        format!("task:{id}")
    }

    /// The orchestrator's CLI events and context injections (Inspector).
    pub fn orchestrator(id: &ConversationId) -> String {
        format!("orch:{id}")
    }

    /// What was queued for a thread's next turn: its inbox lives in memory (#210).
    pub fn inbox(id: &ConversationId) -> String {
        format!("inbox:{id}")
    }

    /// A project's Brain jobs.
    pub fn brain(id: &super::ProjectId) -> String {
        format!("brain:{id}")
    }

    /// A composer draft's pinned attachments: a conversation id, or `new` for a new chat.
    pub fn draft(scope: &str) -> String {
        format!("draft:{scope}")
    }

    /// Whether the clients' live feed carries `stream`'s events: not the engine's own records,
    /// which are no domain events (a background run's stored output, a request's change
    /// records and pending changes, the thread's inbox), #189.
    pub fn on_live_feed(stream: &str) -> bool {
        !["background-output:", "changes:", "pending:", "inbox:"]
            .iter()
            .any(|internal| stream.starts_with(internal))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_saved_with_the_old_density_still_load() {
        let saved = r#"{"density":"compact","hibernateAfterMinutes":12}"#;
        let settings: Settings = serde_json::from_str(saved).unwrap();
        assert_eq!(settings.hibernate_after_minutes, 12);
        assert!(
            !serde_json::to_string(&settings)
                .unwrap()
                .contains("density")
        );
    }

    #[test]
    fn a_folder_mention_is_stored_and_read_back_as_a_folder() {
        let folder = Mention::Folder {
            path: "src/lib".into(),
        };
        let json = serde_json::to_string(&folder).unwrap();
        assert_eq!(json, r#"{"type":"folder","path":"src/lib"}"#);
        assert_eq!(serde_json::from_str::<Mention>(&json).unwrap(), folder);
    }

    #[test]
    fn a_folder_counts_as_trusted_only_once_trusted_unless_it_left_the_project() {
        let project = Project {
            id: ProjectId("p".into()),
            name: "p".into(),
            created_at_ms: 1,
            repos: vec![ProjectRepo {
                path: "/code/now".into(),
                name: "now".into(),
            }],
            prefs: Default::default(),
            trust: vec![FolderTrust {
                path: "/code/answered".into(),
                trusted: false,
                decided_at_ms: 1,
            }],
            auto_merged: false,
        };
        // The project's folder, never answered: asked about, and not trusted until then.
        assert!(!project.counts_trusted("/code/now"));
        // An answer stands, wherever the folder is now.
        assert!(!project.counts_trusted("/code/answered"));
        // A session left on a folder the project no longer has, never answered: nothing can ask.
        assert!(project.counts_trusted("/code/before"));
        let trusted = Project {
            trust: vec![FolderTrust {
                path: "/code/now".into(),
                trusted: true,
                decided_at_ms: 2,
            }],
            ..project
        };
        assert!(trusted.counts_trusted("/code/now"));
    }

    #[test]
    fn fresh_settings_use_approve_for_me_and_saved_permissions_stay() {
        let fresh = Settings::default();
        assert_eq!(fresh.default_permission, PermissionLevel::ApproveForMe);
        let restored: Settings =
            serde_json::from_value(serde_json::to_value(&fresh).unwrap()).unwrap();
        assert_eq!(restored.default_permission, PermissionLevel::ApproveForMe);
        for permission in [
            PermissionLevel::AskForApproval,
            PermissionLevel::ApproveForMe,
            PermissionLevel::FullAccess,
        ] {
            let saved: Settings = serde_json::from_value(serde_json::json!({
                "defaultPermission": permission
            }))
            .unwrap();
            assert_eq!(saved.default_permission, permission);
        }
        let legacy: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.default_permission, PermissionLevel::FullAccess);
    }

    #[test]
    fn old_machine_events_default_to_heat_and_memory_round_trips() {
        let event: DomainEvent = serde_json::from_value(serde_json::json!({
            "type": "machineStepped", "conversationId": "c",
            "step": { "kind": "waitingToCool", "atMs": 123 }
        }))
        .unwrap();
        let DomainEvent::MachineStepped { mut step, .. } = event else {
            panic!("expected a machine step");
        };
        assert_eq!(step.reason, MachineStepReason::Heat);
        step.reason = MachineStepReason::Memory;
        let restored: MachineStep =
            serde_json::from_value(serde_json::to_value(&step).unwrap()).unwrap();
        assert_eq!(restored, step);
    }

    #[test]
    fn saved_settings_with_the_old_usage_switches_still_load() {
        // Settings from before these behaviours were built in carry a `usage` object; it is
        // ignored, and nothing else changes.
        let settings: Settings = serde_json::from_str(
            r#"{"usage":{"conciseReplies":false,"workerHandoff":false,"workerHandoffTokens":90000,"brainRouter":false},"hibernateAfterMinutes":12}"#,
        )
        .unwrap();
        assert_eq!(settings.hibernate_after_minutes, 12);
        assert!(
            !serde_json::to_string(&settings)
                .unwrap()
                .contains(r#""usage""#)
        );
    }

    #[test]
    fn saved_settings_with_the_old_send_shortcut_still_load() {
        // Enter always sends now; a choice saved before is ignored, and nothing else changes.
        let settings: Settings =
            serde_json::from_str(r#"{"sendShortcut":"modEnter","followUp":"steer"}"#).unwrap();
        assert_eq!(settings.follow_up, FollowUp::Steer);
        assert!(
            !serde_json::to_string(&settings)
                .unwrap()
                .contains("sendShortcut")
        );
    }

    #[test]
    fn the_composer_is_rich_by_default_and_for_settings_saved_before_the_switch() {
        assert!(!Settings::default().plain_text_composer);
        let settings: Settings = serde_json::from_str(r#"{"followUp":"steer"}"#).unwrap();
        assert!(!settings.plain_text_composer);
    }

    #[test]
    fn short_replies_are_on_by_default_and_for_settings_saved_before_them() {
        assert!(Settings::default().short_replies);
        // The old hidden `conciseReplies` switch doesn't carry over: the user's choice is
        // the Settings switch alone.
        let settings: Settings = serde_json::from_str(
            r#"{"usage":{"conciseReplies":false},"hibernateAfterMinutes":12}"#,
        )
        .unwrap();
        assert!(settings.short_replies);
        let settings: Settings = serde_json::from_str(r#"{"shortReplies":false}"#).unwrap();
        assert!(!settings.short_replies);
    }

    #[test]
    fn workers_start_on_their_own_by_default_and_for_settings_saved_before_the_switch() {
        assert!(Settings::default().start_workers_on_own);
        let settings: Settings = serde_json::from_str(r#"{"hibernateAfterMinutes":12}"#).unwrap();
        assert!(settings.start_workers_on_own);
        let settings: Settings = serde_json::from_str(r#"{"startWorkersOnOwn":false}"#).unwrap();
        assert!(!settings.start_workers_on_own);
    }

    #[test]
    fn work_is_reviewed_by_default_and_for_settings_saved_before_the_switch() {
        assert!(Settings::default().review_work);
        let settings: Settings = serde_json::from_str(r#"{"hibernateAfterMinutes":12}"#).unwrap();
        assert!(settings.review_work);
        let settings: Settings = serde_json::from_str(r#"{"reviewWork":false}"#).unwrap();
        assert!(!settings.review_work);
    }

    #[test]
    fn work_does_not_merge_on_its_own_by_default_and_for_settings_saved_before_the_switch() {
        assert!(!Settings::default().auto_merge);
        let settings: Settings = serde_json::from_str(r#"{"hibernateAfterMinutes":12}"#).unwrap();
        assert!(!settings.auto_merge);
        let settings: Settings = serde_json::from_str(r#"{"autoMerge":false}"#).unwrap();
        assert!(!settings.auto_merge);
    }

    #[test]
    fn work_keeps_merging_on_its_own_when_the_user_turned_it_on() {
        let settings: Settings = serde_json::from_str(r#"{"autoMerge":true}"#).unwrap();
        assert!(settings.auto_merge);
    }

    #[test]
    fn saved_settings_that_name_a_read_aloud_voice_still_load() {
        // Read aloud speaks in the system's default voice now; a voice an earlier version
        // saved is ignored, and is gone once the settings are saved again.
        let settings: Settings = serde_json::from_str(
            r#"{"readAloudVoice":"com.apple.voice.daniel","dictationLanguage":"de"}"#,
        )
        .unwrap();
        assert_eq!(settings.dictation_language, "de");
        assert!(
            !serde_json::to_string(&settings)
                .unwrap()
                .contains("readAloudVoice")
        );
    }
}
