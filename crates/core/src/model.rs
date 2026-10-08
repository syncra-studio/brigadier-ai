//! Domain model. These types are the wire contract too: they are exported to TypeScript.

use std::collections::HashMap;

use brigadier_providers::{
    Access, Artifact, ModelCatalog, ProviderEvent, ProviderKind, ProviderStatus, QuotaSnapshot,
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

impl OvernightRunId {
    /// The last 8 characters: the run's part of its branch and folder names.
    pub fn short(&self) -> &str {
        &self.0[self.0.len().saturating_sub(8)..]
    }
}

/// A workspace of one or more repos. Owns its sessions.
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
    /// Absent: the global default from Settings.
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
    pub fn from_request(request: SetupRequest, id: &ConversationId) -> Self {
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
                            .unwrap_or_else(|| format!("brigadier/{}/session", id.short())),
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
    /// Another conversation: its recent messages go along as context.
    Chat { id: ConversationId, title: String },
}

/// Also reads mentions stored before files and chats could be mentioned: a bare task id.
impl<'de> Deserialize<'de> for Mention {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "camelCase")]
        enum Tagged {
            Task { id: TaskId },
            File { path: String },
            Chat { id: ConversationId, title: String },
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
            Stored::Tagged(Tagged::Chat { id, title }) => Mention::Chat { id, title },
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum MessageRole {
    User,
    Assistant,
    System,
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
    /// What the message @-mentions: workers, files, other conversations.
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
}

/// Global size and spacing scale for every control.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Density {
    Compact,
    #[default]
    Normal,
}

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

/// User settings persisted by the core.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub density: Density,
    /// Permission level for projects that have not remembered one.
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
    /// A notice above the composer while a conversation is in Full access.
    pub show_full_access_notice: bool,
    /// Spend quota left over before a usage window resets on deepening the Project Brains.
    pub enrich_brain: bool,
    /// The orchestrator, an overnight run's leads included, answers the user in a few plain
    /// lines (PLAN.md §7). Off, it keeps the plain voice without the length limits.
    pub short_replies: bool,
    /// Co-authored-by trailers that name an AI are left out of commit messages: the user's
    /// own commits, what the agents write, and the commits Brigadier lands or keeps.
    #[serde(default = "default_true")]
    pub omit_ai_coauthors: bool,
    /// The first-run setup (agents, then projects) was finished or skipped.
    pub onboarded: bool,
    /// When the computer is kept from sleeping.
    pub keep_awake: KeepAwake,
    /// While kept awake, closing the lid doesn't sleep the computer either (macOS: sleep is
    /// disabled system-wide, then restored).
    pub keep_awake_lid_closed: bool,
    /// The user's routing rules, global and per project. They always win over the router's
    /// scores and quota balancing.
    pub routing_overrides: Vec<OverrideRule>,
    /// The user's manual rankings per kind of work (global and per project, with area
    /// overrides): where one is Manual, routing tries its models top-down instead of scoring.
    pub routing_rankings: Vec<Ranking>,
    /// Agents switched off on the Providers page: their models are hidden from every picker
    /// and get no work, not even in the background. Conversations already running go on.
    pub disabled_providers: Vec<ProviderKind>,
    /// Models the Providers page makes unavailable: hidden from every picker and never used.
    pub hidden_models: Vec<ModelRef>,
    /// Every model Brigadier has seen in its agent's list. One seen after its agent's first
    /// list starts without worker tasks (a rule the Routing page's switch removes).
    pub known_models: Vec<ModelRef>,
    /// The shape saved settings were last brought up to ([`SETTINGS_VERSION`]). Settings saved
    /// before it existed read as 0, so their conversions run.
    #[serde(default)]
    pub settings_version: u32,
}

/// A setting that is on unless the user turned it off, also in settings saved before it
/// existed.
fn default_true() -> bool {
    true
}

/// The settings' shape: each step up converts saved settings once (see
/// [`crate::routing::availability::migrate`]).
pub const SETTINGS_VERSION: u32 = 1;

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
            density: Density::default(),
            default_permission: PermissionLevel::FullAccess,
            default_orchestrator: None,
            default_chat_model: None,
            hibernate_after_minutes: 30,
            show_context_usage: true,
            show_full_access_notice: true,
            enrich_brain: true,
            short_replies: true,
            omit_ai_coauthors: true,
            onboarded: false,
            keep_awake: KeepAwake::default(),
            keep_awake_lid_closed: false,
            routing_overrides: Vec::new(),
            routing_rankings: Vec::new(),
            disabled_providers: Vec::new(),
            hidden_models: Vec::new(),
            known_models: Vec::new(),
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

/// Text an assistant is still writing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct StreamingMessage {
    pub message_id: String,
    pub text: String,
    /// The request the running turn serves.
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    pub level: brigadier_providers::NoticeLevel,
    pub text: String,
    pub at_ms: i64,
}

/// A grey thread row about the machine (PLAN.md §10.7): a worker or a build waiting for the
/// machine to cool down, or for another build; a build paused for the heat, or going on again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MachineStep {
    pub kind: MachineStepKind,
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
    /// The user's ratings of answers, by subject (see `DomainEvent::MessageRated`).
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

/// The newer versions found by the last check, and any update running or just finished.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UpdatesView {
    pub items: Vec<UpdateItem>,
    pub checked_at_ms: Option<i64>,
}

/// Brigadier's own use of a provider in one of its usage windows (since the window began).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WindowTokens {
    /// The window (`QuotaWindow::id`).
    pub window_id: String,
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
    /// Its last landing.
    pub landed_ms: Option<i64>,
    /// The last use of a model by anything it started, reviews after the answer included
    /// (absent while it works).
    pub settled_ms: Option<i64>,
    /// Tokens per provider, most first.
    pub providers: Vec<ProviderTokens>,
    /// Tokens per kind of step (thread, worker, review…), most first.
    pub steps: Vec<StepTokens>,
}

/// What one provider's models read and wrote for a request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTokens {
    pub provider: ProviderKind,
    pub input: i64,
    pub cached_input: i64,
    pub cache_write: i64,
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
    /// Its model calls (Claude: turns).
    pub calls: u32,
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
    /// The thread's model calls (Claude: turns) for it.
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

/// A window's samples over its current span, for the Usage page's chart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WindowHistory {
    pub window_id: String,
    /// Oldest first.
    pub samples: Vec<QuotaSample>,
}

/// One provider on the Usage page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsage {
    pub provider: ProviderKind,
    /// Absent until the first read.
    pub quota: Option<ProviderQuota>,
    pub history: Vec<WindowHistory>,
    pub tokens: Vec<WindowTokens>,
    /// What balancing does about it now ("New scouting and research go to Codex while
    /// Claude's weekly window runs hot").
    pub balancing: Option<String>,
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
    pub at_ms: i64,
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
        settings: Settings,
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
    /// These artifacts of `owner` are gone.
    CleanupRemoved {
        owner: String,
        artifacts: Vec<Artifact>,
    },
    /// Everything `owner` created is to be removed; what fails is retried at the next launch.
    CleanupRequested {
        owner: String,
    },
    /// Written before artifacts were acknowledged one by one: every artifact of `owner` was
    /// dealt with.
    CleanupCompleted {
        owner: String,
        failures: Vec<String>,
    },
    /// Model ratings changed; reload Usage, model summaries and route previews.
    RankingsChanged,
    ProviderChecked {
        overview: ProviderOverview,
    },
    /// What can be updated changed: a check finished, or an update started or ended.
    UpdatesChanged {
        updates: UpdatesView,
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
    /// The user rated an answer. Ratings stay on this machine.
    MessageRated {
        /// The answer: a message id, or `task:<id>` for a worker's report.
        subject: String,
        rating: Rating,
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
    /// The thread's commits on `branch` were looked at up to `tip`: what follows it is new
    /// (THREAD-PLAN.md Q4, Q12).
    ThreadCommitsSeen {
        conversation_id: ConversationId,
        branch: String,
        tip: String,
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
        /// The `out-…` alias of its stored output, which `read_artifact` reads.
        artifact: Option<String>,
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
    QueueChanged {
        conversation_id: ConversationId,
        queue: MessageQueue,
    },
    /// Something a worker's CLI reported.
    WorkerEvent {
        task_id: TaskId,
        event: ProviderEvent,
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
    /// Diagnostic probe used to measure ingest → paint latency end to end.
    Probe {
        burst_id: String,
        index: u32,
        count: u32,
    },
}

impl DomainEvent {
    /// The stored event `kind`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ProjectCreated { .. } => "project.created",
            Self::ConversationCreated { .. } => "conversation.created",
            Self::ConversationRenamed { .. } => "conversation.renamed",
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
            Self::CleanupRequested { .. } => "cleanup.requested",
            Self::CleanupCompleted { .. } => "cleanup.completed",
            Self::RankingsChanged => "rankings.changed",
            Self::ProviderChecked { .. } => "provider.checked",
            Self::UpdatesChanged { .. } => "updates.changed",
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
            Self::WorkerStepped { .. } => "worker.step",
            Self::OrchestratorStepped { .. } => "orchestrator.step",
            Self::MachineStepped { .. } => "machine.step",
            Self::CompactionUpdated { .. } => "compaction.updated",
            Self::MessageRated { .. } => "message.rated",
            Self::BranchSwitched { .. } => "conversation.branch",
            Self::ConversationNotice { .. } => "conversation.notice",
            Self::TaskUpdated { .. } => "task.updated",
            Self::ApprovalUpdated { .. } => "approval.updated",
            Self::QuestionUpdated { .. } => "question.updated",
            Self::PlanUpdated { .. } => "plan.updated",
            Self::ReviewUpdated { .. } => "review.updated",
            Self::ThreadCommitsSeen { .. } => "thread.seen",
            Self::OutputStored { .. } => "output.stored",
            Self::CheckRan { .. } => "check.ran",
            Self::ThreadLooked { .. } => "thread.looked",
            Self::PreviewUpdated { .. } => "preview.updated",
            Self::QueueChanged { .. } => "queue.changed",
            Self::WorkerEvent { .. } => "worker.event",
            Self::OrchestratorLogged { .. } => "orchestrator.logged",
            Self::DraftPinned { .. } => "draft.pinned",
            Self::BrainJobUpdated { .. } => "brain.job",
            Self::MemoryUpdated { .. } => "memory.updated",
            Self::DecidedForYou { .. } => "decision.made",
            Self::WaitingOnYou { .. } => "waiting.updated",
            Self::WaitingResolved { .. } => "waiting.resolved",
            Self::OvernightUpdated { .. } => "overnight.updated",
            Self::Probe { .. } => "diag.probe",
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
    /// Newer versions of Brigadier and the agent CLIs.
    pub const UPDATES: &str = "updates";

    pub fn raw_session(id: &super::RawSessionId) -> String {
        format!("raw:{id}")
    }

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

    /// A project's Brain jobs.
    pub fn brain(id: &super::ProjectId) -> String {
        format!("brain:{id}")
    }

    /// A composer draft's pinned attachments: a conversation id, or `new` for a new chat.
    pub fn draft(scope: &str) -> String {
        format!("draft:{scope}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
