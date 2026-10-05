use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use brigadier_providers::{ProviderEvent, ProviderKind};
use brigadier_store::{NewEvent, Retention, Store, StreamPage};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::board::{self, Board};
use crate::model::{
    Catalog, ContextUsage, Conversation, ConversationId, ConversationKind, ConversationView,
    DomainEvent, EnvironmentKind, ForkOrigin, Lifecycle, Message, MessagePage, MessageRole,
    ModelChoice, OrchestratorLogEntry, OrchestratorPage, Project, ProjectId, ProjectPatch,
    ProjectRepo, RawEntry, Settings, Setup, WorkerPage, streams,
};
use crate::projection::Projection;
use crate::work::{
    AttachmentRef, ConversationActivity, Mention, MessageQueue, OrchestratorEntry, QueuedMessage,
    RequestState, RequestUndo, Task, TaskId, UserRequest,
};
use crate::{Error, Result, now_ms};

const MAX_NAME_CHARS: usize = 200;
/// Messages longer than this keep only a preview inline; the full text goes to the blob store.
const INLINE_TEXT_BYTES: usize = 64 * 1024;
const PREVIEW_BYTES: usize = 4 * 1024;
const TITLE_CHARS: usize = 60;
const REQUEST_PREVIEW_CHARS: usize = 80;
const NEW_SESSION_TITLE: &str = "New session";
const NEW_CHAT_TITLE: &str = "New chat";

/// Where a new conversation comes from: a fork's source, or the conversation a side chat
/// sits beside. Neither for a new session or Chat.
#[derive(Debug, Clone, Default)]
pub struct Origin {
    pub forked_from: Option<ForkOrigin>,
    pub side_of: Option<ConversationId>,
}
/// Diagnostic probes kept on disk; older ones are trimmed as new ones arrive.
const PROBES_KEPT: u32 = 10_000;
const MAX_PROBES: u32 = 5_000;
const CATALOG_PAGE: u32 = 1_000;
/// Serialized size a message page may reach, well under the IPC frame cap. A single message
/// always fits: its inline text is capped at `INLINE_TEXT_BYTES`.
const PAGE_BYTES: usize = 4 * 1024 * 1024;
/// Largest attachment accepted (it travels base64-encoded in one IPC frame).
pub const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;
const MAX_ATTACHMENTS: usize = 20;
const MAX_QUEUED: usize = 50;
const BOARD_PAGE: u32 = 1_000;
/// Newest orchestrator log entries searched for the model's context size.
const CONTEXT_SCAN: u32 = 200;

/// A synthetic event burst started by [`Core::probe_burst`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProbeBurst {
    pub burst_id: String,
    pub count: u32,
    pub interval_ms: u32,
}

/// The session manager. Validates commands, records them as events, and keeps the catalog
/// projection current.
pub struct Core {
    store: Store,
    projection: Mutex<Projection>,
    /// Boards of the conversations read since start, kept current by every write through
    /// [`Core::record_conversation`].
    boards: tokio::sync::Mutex<HashMap<ConversationId, Board>>,
    /// Held across a settings change the core makes itself (read, change, write), so a
    /// change from a client isn't lost in between.
    settings_writes: tokio::sync::Mutex<()>,
}

impl Core {
    /// Rebuilds the catalog from the event log.
    pub async fn load(store: Store) -> Result<Arc<Self>> {
        let mut projection = Projection::default();

        let mut after = 0;
        loop {
            let page = store
                .read_stream_since(streams::CATALOG.into(), after, CATALOG_PAGE)
                .await?;
            for event in &page {
                projection.apply(&decode(event)?, event.seq, event.at_ms);
                after = event.stream_seq;
            }
            if page.len() < CATALOG_PAGE as usize {
                break;
            }
        }

        let latest_settings = store
            .read_stream(
                streams::SETTINGS.into(),
                StreamPage {
                    limit: 1,
                    ..StreamPage::default()
                },
            )
            .await?;
        if let Some(event) = latest_settings.first() {
            projection.apply(&decode(event)?, event.seq, event.at_ms);
        }

        // Last activity per conversation comes from the head of its message stream.
        let prefix = streams::conversation(&ConversationId(String::new()));
        for head in store.stream_heads(prefix.clone()).await? {
            let id = ConversationId(head.stream[prefix.len()..].to_owned());
            projection.touch(&id, head.at_ms);
        }

        let core = Arc::new(Self {
            store,
            projection: Mutex::new(projection),
            boards: tokio::sync::Mutex::new(HashMap::new()),
            settings_writes: tokio::sync::Mutex::new(()),
        });
        if let Some(settings) = crate::routing::availability::migrate(&core.settings()) {
            core.update_settings(settings).await?;
        }
        Ok(core)
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn catalog(&self) -> Catalog {
        self.projection().catalog()
    }

    /// What runs and what waits for the user, per conversation (those whose board is loaded:
    /// every conversation that ran or changed since the daemon started).
    pub async fn activity(&self) -> Vec<ConversationActivity> {
        let boards = self.boards.lock().await;
        let mut activity: Vec<_> = boards
            .iter()
            .filter_map(|(id, board)| board.activity(id))
            .collect();
        activity.sort_by(|a, b| a.conversation_id.0.cmp(&b.conversation_id.0));
        activity
    }

    /// Creates a project, optionally with its repository (named after it when `name` is
    /// empty).
    pub async fn create_project(&self, name: String, repo: Option<String>) -> Result<Project> {
        let repos = match repo {
            Some(path) => vec![check_repo(path).await?],
            None => Vec::new(),
        };
        if let Some(repo) = repos.first()
            && let Some(existing) = self
                .projection()
                .projects
                .values()
                .find(|project| project.repos.iter().any(|r| r.path == repo.path))
        {
            return Err(Error::Invalid(format!(
                "{} is already the project \"{}\"",
                repo.path, existing.name
            )));
        }
        let name = match (name.trim().is_empty(), repos.first()) {
            (true, Some(repo)) => repo.name.clone(),
            _ => clean_name(&name, "project name")?,
        };
        let project = Project {
            id: ProjectId::generate(),
            name,
            created_at_ms: now_ms(),
            repos,
            prefs: Default::default(),
        };
        self.record(vec![(
            streams::CATALOG.into(),
            DomainEvent::ProjectCreated {
                project: project.clone(),
            },
        )])
        .await?;
        Ok(project)
    }

    pub async fn update_project(&self, id: ProjectId, patch: ProjectPatch) -> Result<Project> {
        let mut project = self.project(&id)?;
        if let Some(name) = patch.name {
            project.name = clean_name(&name, "project name")?;
        }
        if let Some(paths) = patch.repos {
            let mut repos = Vec::with_capacity(paths.len());
            for path in paths {
                repos.push(check_repo(path).await?);
            }
            project.repos = repos;
        }
        if let Some(prefs) = patch.prefs {
            for file in &prefs.secret_files {
                check_secret_path(file)?;
            }
            project.prefs = prefs;
        }
        self.record(vec![(
            streams::CATALOG.into(),
            DomainEvent::ProjectUpdated {
                project: project.clone(),
            },
        )])
        .await?;
        Ok(project)
    }

    pub fn project(&self, id: &ProjectId) -> Result<Project> {
        self.projection()
            .projects
            .get(id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("project {id}")))
    }

    pub fn settings(&self) -> Settings {
        self.projection().settings.clone()
    }

    /// Creates a session or a chat under `id` (picked by the caller, whose setup may name
    /// branches after it). A session's setup is remembered by its project.
    pub async fn create_conversation(
        &self,
        id: ConversationId,
        kind: ConversationKind,
        project_id: Option<ProjectId>,
        title: Option<String>,
        setup: Option<Setup>,
        origin: Origin,
    ) -> Result<Conversation> {
        match (&setup, kind) {
            (Some(Setup::Session { .. }), ConversationKind::Chat)
            | (Some(Setup::Chat { .. }), ConversationKind::Session) => {
                return Err(Error::Invalid(
                    "the setup does not match the conversation kind".into(),
                ));
            }
            _ => {}
        }
        match (kind, &project_id) {
            (ConversationKind::Session, None) => {
                return Err(Error::Invalid("a session belongs to a project".into()));
            }
            (ConversationKind::Chat, Some(_)) => {
                return Err(Error::Invalid("a chat is outside any project".into()));
            }
            (ConversationKind::Session, Some(id))
                if !self.projection().projects.contains_key(id) =>
            {
                return Err(Error::NotFound(format!("project {id}")));
            }
            _ => {}
        }
        let title = match title {
            Some(title) => clean_name(&title, "title")?,
            None => match kind {
                ConversationKind::Session => NEW_SESSION_TITLE.into(),
                ConversationKind::Chat => NEW_CHAT_TITLE.into(),
            },
        };
        let now = now_ms();
        let mut events = Vec::new();
        if let (Some(Setup::Session { repo, .. }), Some(project_id)) = (&setup, &project_id) {
            let mut project = self.project(project_id)?;
            if !project.repos.iter().any(|known| known.path == *repo) {
                return Err(Error::Invalid(format!(
                    "{repo} is not one of the project's repositories"
                )));
            }
            if remember(&mut project, setup.as_ref()) {
                events.push((
                    streams::CATALOG.into(),
                    DomainEvent::ProjectUpdated { project },
                ));
            }
        }
        let conversation = Conversation {
            id,
            kind,
            project_id,
            title,
            pinned_at_ms: None,
            created_at_ms: now,
            updated_at_ms: now,
            setup,
            lifecycle: Lifecycle::Active,
            forked_from: origin.forked_from,
            side_of: origin.side_of,
            fallback: None,
            quota_wait: None,
            cleanup_pending: false,
        };
        events.insert(
            0,
            (
                streams::CATALOG.into(),
                DomainEvent::ConversationCreated {
                    conversation: Box::new(conversation.clone()),
                },
            ),
        );
        self.record(events).await?;
        Ok(conversation)
    }

    /// Changes a conversation's setup (model, effort, permission level; a session's
    /// environment only while it has none). A session's project remembers it.
    pub async fn set_setup(&self, id: ConversationId, setup: Setup) -> Result<Conversation> {
        let conversation = self.conversation(&id)?;
        match (&conversation.setup, &setup, conversation.kind) {
            (_, Setup::Chat { .. }, ConversationKind::Chat) => {}
            (
                Some(Setup::Session {
                    repo, environment, ..
                }),
                Setup::Session {
                    repo: new_repo,
                    environment: new_environment,
                    ..
                },
                ConversationKind::Session,
            ) if repo != new_repo || !same_environment(environment, new_environment) => {
                return Err(Error::Invalid(
                    "a session's repository and environment cannot change".into(),
                ));
            }
            (_, Setup::Session { .. }, ConversationKind::Session) => {}
            _ => {
                return Err(Error::Invalid(
                    "the setup does not match the conversation kind".into(),
                ));
            }
        }
        let mut events = vec![(
            streams::CATALOG.into(),
            DomainEvent::ConversationSetUp {
                id: id.clone(),
                setup: setup.clone(),
            },
        )];
        if let Some(project_id) = &conversation.project_id {
            let mut project = self.project(project_id)?;
            if remember(&mut project, Some(&setup)) {
                events.push((
                    streams::CATALOG.into(),
                    DomainEvent::ProjectUpdated { project },
                ));
            }
        }
        self.record(events).await?;
        self.conversation(&id)
    }

    pub async fn set_lifecycle(
        &self,
        id: ConversationId,
        lifecycle: Lifecycle,
    ) -> Result<Conversation> {
        let current = self.conversation(&id)?;
        if current.lifecycle != lifecycle {
            self.record(vec![(
                streams::CATALOG.into(),
                DomainEvent::ConversationLifecycleChanged {
                    id: id.clone(),
                    lifecycle,
                },
            )])
            .await?;
        }
        self.conversation(&id)
    }

    /// Archives a conversation and marks its cleanup as pending, in one durable write: once
    /// this returns, a restart finishes the cleanup if it was cut off.
    pub async fn mark_archived(&self, id: ConversationId) -> Result<Conversation> {
        self.conversation(&id)?;
        self.record(vec![
            (
                streams::CATALOG.into(),
                DomainEvent::ConversationLifecycleChanged {
                    id: id.clone(),
                    lifecycle: Lifecycle::Archived,
                },
            ),
            (
                streams::CATALOG.into(),
                DomainEvent::ConversationCleanup {
                    id: id.clone(),
                    pending: true,
                },
            ),
        ])
        .await?;
        self.conversation(&id)
    }

    /// The cleanup after an archive finished: nothing is left for a restart to do.
    pub async fn finish_cleanup(&self, id: ConversationId) -> Result<()> {
        if self.conversation(&id)?.cleanup_pending {
            self.record(vec![(
                streams::CATALOG.into(),
                DomainEvent::ConversationCleanup { id, pending: false },
            )])
            .await?;
        }
        Ok(())
    }

    /// Removes a project from the catalog. Its conversations are deleted by the caller first.
    pub async fn forget_project(&self, id: ProjectId) -> Result<()> {
        self.project(&id)?;
        self.record(vec![(
            streams::CATALOG.into(),
            DomainEvent::ProjectRemoved { id },
        )])
        .await?;
        Ok(())
    }

    /// Removes a conversation from the catalog. Its streams are purged by the caller.
    pub async fn forget_conversation(&self, id: ConversationId) -> Result<()> {
        self.conversation(&id)?;
        self.record(vec![(
            streams::CATALOG.into(),
            DomainEvent::ConversationDeleted { id: id.clone() },
        )])
        .await?;
        self.boards.lock().await.remove(&id);
        Ok(())
    }

    pub async fn rename_conversation(
        &self,
        id: ConversationId,
        title: String,
    ) -> Result<Conversation> {
        self.conversation(&id)?;
        let title = clean_name(&title, "title")?;
        self.record(vec![(
            streams::CATALOG.into(),
            DomainEvent::ConversationRenamed {
                id: id.clone(),
                title,
            },
        )])
        .await?;
        self.conversation(&id)
    }

    pub async fn set_pinned(&self, id: ConversationId, pinned: bool) -> Result<Conversation> {
        let current = self.conversation(&id)?;
        if current.pinned_at_ms.is_some() == pinned {
            return Ok(current);
        }
        self.record(vec![(
            streams::CATALOG.into(),
            DomainEvent::ConversationPinned {
                id: id.clone(),
                pinned_at_ms: pinned.then(now_ms),
            },
        )])
        .await?;
        self.conversation(&id)
    }

    /// Appends a user message. The first message of an untitled conversation names it.
    pub async fn append_message(&self, id: ConversationId, text: String) -> Result<Message> {
        self.append_user_message(id, text, Vec::new(), Vec::new())
            .await
    }

    /// Appends a user message with its attachments and @-mentions. It may be attachments
    /// only.
    pub async fn append_user_message(
        &self,
        id: ConversationId,
        text: String,
        attachments: Vec<AttachmentRef>,
        mentions: Vec<Mention>,
    ) -> Result<Message> {
        self.append_user_message_under(id, text, attachments, mentions, None)
            .await
    }

    /// Appends a user message after `parent` (a message id, or [`ROOT`] for a new first
    /// message) instead of after the branch shown: an edit starts a branch beside the
    /// message it edits.
    pub(crate) async fn append_user_message_under(
        &self,
        id: ConversationId,
        text: String,
        attachments: Vec<AttachmentRef>,
        mentions: Vec<Mention>,
        parent: Option<String>,
    ) -> Result<Message> {
        let conversation = self.conversation(&id)?;
        let trimmed = text.trim().to_owned();
        if trimmed.is_empty() && attachments.is_empty() {
            return Err(Error::Invalid("message is empty".into()));
        }
        check_attachments(&attachments)?;
        let mut message = self
            .new_message(
                &id,
                MessageRole::User,
                uuid::Uuid::now_v7().to_string(),
                text,
            )
            .await?;
        message.attachments = attachments;
        message.mentions = mentions;
        if parent.is_some() {
            message.parent_id = parent;
        }
        // Each user message starts a request of its own.
        message.request_id = Some(message.id.clone());
        // What the user wrote: typed text, or else the first thing they pasted.
        let words = if trimmed.is_empty() {
            self.pasted_texts(&message.attachments)
                .await
                .into_iter()
                .find(|text| !text.trim().is_empty())
                .unwrap_or_default()
        } else {
            display_text(&trimmed, &message.attachments)
        };

        let mut events = vec![
            (
                streams::conversation(&id),
                DomainEvent::MessageAppended {
                    message: message.clone(),
                },
            ),
            (
                streams::conversation(&id),
                DomainEvent::RequestUpdated {
                    request: UserRequest {
                        id: message.id.clone(),
                        conversation_id: id.clone(),
                        preview: preview(&words),
                        state: RequestState::Working,
                        started_at_ms: message.created_at_ms,
                        ended_at_ms: None,
                        steered_into: None,
                        steered_after: None,
                        undo: None,
                    },
                },
            ),
        ];
        let untitled = matches!(
            conversation.title.as_str(),
            NEW_SESSION_TITLE | NEW_CHAT_TITLE
        );
        if untitled {
            let title = if words.is_empty() {
                message
                    .attachments
                    .iter()
                    .find(|attachment| !attachment.pasted)
                    .map(|attachment| title_from(&attachment.name))
                    .unwrap_or_else(|| conversation.title.clone())
            } else {
                title_from(&words)
            };
            events.push((
                streams::CATALOG.into(),
                DomainEvent::ConversationRenamed {
                    id: id.clone(),
                    title,
                },
            ));
        }
        let stored = self.record_with_board(&id, events).await?;
        Ok(Message {
            seq: stored[0],
            ..message
        })
    }

    /// Appends an assistant message, ending the stream of deltas with the same `message_id`.
    pub async fn append_assistant_message(
        &self,
        id: ConversationId,
        message_id: String,
        text: String,
        model: Option<ModelChoice>,
        request_id: Option<String>,
    ) -> Result<Message> {
        self.conversation(&id)?;
        let mut message = self
            .new_message(&id, MessageRole::Assistant, message_id, text)
            .await?;
        message.model = model;
        message.request_id = request_id;
        let stored = self
            .record_conversation(
                &id,
                vec![DomainEvent::MessageAppended {
                    message: message.clone(),
                }],
            )
            .await?;
        Ok(Message {
            seq: stored[0],
            ..message
        })
    }

    async fn new_message(
        &self,
        id: &ConversationId,
        role: MessageRole,
        message_id: String,
        text: String,
    ) -> Result<Message> {
        let (inline, blob) = if text.len() > INLINE_TEXT_BYTES {
            let hash = self.store.blobs().put(text.clone().into_bytes()).await?;
            (
                prefix(&text, PREVIEW_BYTES).to_owned(),
                Some(hash.to_string()),
            )
        } else {
            (text, None)
        };
        let parent_id = self.head(id).await?;
        Ok(Message {
            id: message_id,
            conversation_id: id.clone(),
            // Assigned by the store; the stream sequence is the message position.
            seq: 0,
            role,
            text: inline,
            blob,
            created_at_ms: now_ms(),
            attachments: Vec::new(),
            mentions: Vec::new(),
            model: None,
            request_id: None,
            parent_id,
        })
    }

    /// The last message of the branch the thread shows: new messages continue from it.
    pub(crate) async fn head(&self, id: &ConversationId) -> Result<Option<String>> {
        let mut boards = self.boards.lock().await;
        if !boards.contains_key(id) {
            let board = self.load_board(id).await?;
            boards.insert(id.clone(), board);
        }
        Ok(boards
            .get(id)
            .and_then(|board| board.head.as_ref())
            .map(|(head, _)| head.clone()))
    }

    /// Shows the branch that ends at `head` (a message of the conversation).
    pub(crate) async fn switch_branch(&self, id: &ConversationId, head: String) -> Result<()> {
        self.record_conversation(
            id,
            vec![DomainEvent::BranchSwitched {
                conversation_id: id.clone(),
                head,
            }],
        )
        .await
        .map(|_| ())
    }

    /// Every message of the conversation, every branch, oldest first.
    pub(crate) async fn all_messages(&self, id: &ConversationId) -> Result<Vec<Message>> {
        let mut messages = Vec::new();
        let mut before = None;
        loop {
            let page = self
                .list_messages(id.clone(), before, brigadier_store::MAX_PAGE - 1)
                .await?;
            before = page.messages.first().map(|message| message.seq);
            let more = page.has_more && before.is_some();
            let mut older = page.messages;
            older.append(&mut messages);
            messages = older;
            if !more {
                return Ok(messages);
            }
        }
    }

    /// The branch that ends at `leaf`, oldest first: each message's parent, back to the
    /// first message. Empty when `leaf` is unknown.
    pub(crate) async fn branch(&self, id: &ConversationId, leaf: &str) -> Result<Vec<Message>> {
        let messages = self.all_messages(id).await?;
        Ok(branch_of(&messages, leaf))
    }

    /// Stores an attachment in the blob store. `pasted` marks text pasted into the composer,
    /// which must be UTF-8 plain text.
    pub async fn add_attachment(
        &self,
        name: String,
        mime: String,
        bytes: Vec<u8>,
        pasted: bool,
    ) -> Result<AttachmentRef> {
        if bytes.is_empty() {
            return Err(Error::Invalid("the attachment is empty".into()));
        }
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(Error::Invalid(format!(
                "attachments are limited to {} MB",
                MAX_ATTACHMENT_BYTES / (1024 * 1024)
            )));
        }
        let name = clean_name(&name, "attachment name")?;
        let mime = if mime.trim().is_empty() {
            "application/octet-stream".to_owned()
        } else {
            mime.trim().to_owned()
        };
        if pasted && (!mime.starts_with("text/plain") || std::str::from_utf8(&bytes).is_err()) {
            return Err(Error::Invalid(
                "pasted text must be UTF-8 plain text".into(),
            ));
        }
        let size = bytes.len() as u64;
        let hash = self.store.blobs().put(bytes).await?;
        Ok(AttachmentRef {
            id: hash.to_string(),
            name,
            mime,
            bytes: size,
            pasted,
            inline: None,
        })
    }

    /// The text the user pasted into a message, one entry per pasted attachment, in order. A
    /// paste that cannot be read is left out.
    pub(crate) async fn pasted_texts(&self, attachments: &[AttachmentRef]) -> Vec<String> {
        let mut texts = Vec::new();
        for attachment in attachments.iter().filter(|attachment| attachment.pasted) {
            if let Ok(text) = self.read_blob_text(attachment.id.clone()).await {
                texts.push(text);
            }
        }
        texts
    }

    /// A user message's words for a transcript or a note: its text, then the start of each
    /// text they pasted.
    pub(crate) async fn brief_words(&self, text: &str, attachments: &[AttachmentRef]) -> String {
        let mut words = display_text(text, attachments);
        for pasted in self.pasted_texts(attachments).await {
            let start = prefix(&pasted, PREVIEW_BYTES);
            let cut = if start.len() < pasted.len() {
                "…"
            } else {
                ""
            };
            push_block(&mut words, &format!("{start}{cut}"));
        }
        words
    }

    /// A derived snippet may quote a user message; match images against its conversation.
    pub(crate) async fn display_quote(&self, id: &ConversationId, text: &str) -> String {
        if !text.contains("[image:") && !text.contains("[Image #") {
            return text.to_owned();
        }
        let mut attachments = Vec::new();
        for message in self.all_messages(id).await.unwrap_or_default() {
            if message.role != MessageRole::User {
                continue;
            }
            let original = match &message.blob {
                Some(hash) => self
                    .read_blob_text(hash.clone())
                    .await
                    .unwrap_or_else(|_| message.text.clone()),
                None => message.text.clone(),
            };
            attachments.extend(
                inline_image_tokens(&original, &message.attachments)
                    .into_iter()
                    .map(|(_, attachment)| attachment.clone()),
            );
        }
        display_text(text, &attachments)
    }

    /// Keeps a composer draft's attachments stored until it is sent or discarded (see
    /// [`DomainEvent::DraftPinned`]); an empty list lets the previous ones go. `scope` is a
    /// conversation id, or `new` for a new chat.
    pub async fn pin_draft_attachments(
        &self,
        scope: String,
        attachments: Vec<AttachmentRef>,
    ) -> Result<()> {
        let valid = !scope.is_empty()
            && scope.len() <= 64
            && scope.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if !valid {
            return Err(Error::Invalid(
                "a draft scope is a conversation id or \"new\"".into(),
            ));
        }
        check_attachments(&attachments)?;
        let stream = streams::draft(&scope);
        let event = DomainEvent::DraftPinned { scope, attachments };
        self.store
            .append_with(
                vec![to_new_event(stream, &event)?],
                Some(Retention { keep_last: 1 }),
            )
            .await?;
        Ok(())
    }

    /// A stored attachment's bytes.
    pub async fn read_attachment(&self, id: &str) -> Result<Vec<u8>> {
        self.store
            .blobs()
            .get(id.parse::<brigadier_store::BlobHash>()?)
            .await?
            .ok_or_else(|| Error::NotFound(format!("attachment {id}")))
    }

    /// Messages before `before` (a message `seq`), oldest first.
    pub async fn list_messages(
        &self,
        id: ConversationId,
        before: Option<i64>,
        limit: u32,
    ) -> Result<MessagePage> {
        self.conversation(&id)?;
        let limit = limit.clamp(1, brigadier_store::MAX_PAGE - 1);
        let mut events = self
            .store
            .read_stream(
                streams::conversation(&id),
                StreamPage {
                    before,
                    kinds: vec!["message.appended".into()],
                    // One extra row tells us whether there is an older page.
                    limit: limit + 1,
                },
            )
            .await?;
        let mut has_more = events.len() > limit as usize;
        events.truncate(limit as usize);
        // Newest first: stop where the size budget runs out, so the page stays contiguous.
        let mut bytes = 0;
        let fits = events
            .iter()
            .take_while(|event| {
                bytes += event.payload.get().len();
                bytes <= PAGE_BYTES
            })
            .count()
            .max(1);
        if fits < events.len() {
            events.truncate(fits);
            has_more = true;
        }
        events.reverse();
        let messages = events
            .iter()
            .map(|event| match decode(event)? {
                DomainEvent::MessageAppended { message } => Ok(Message {
                    seq: event.stream_seq,
                    ..message
                }),
                _ => Err(Error::Invalid(format!(
                    "event {} is not a message",
                    event.seq
                ))),
            })
            .collect::<Result<_>>()?;
        Ok(MessagePage { messages, has_more })
    }

    /// A page of a worker's transcript, oldest first.
    pub async fn list_worker_events(
        &self,
        task_id: &TaskId,
        before: Option<i64>,
        limit: u32,
    ) -> Result<WorkerPage> {
        let (events, has_more) = self
            .read_page(streams::task(task_id), "worker.event", before, limit)
            .await?;
        let entries = events
            .iter()
            .filter_map(|event| match decode(event) {
                Ok(DomainEvent::WorkerEvent {
                    event: provider, ..
                }) => Some(Ok(RawEntry {
                    stream_seq: event.stream_seq,
                    at_ms: event.at_ms,
                    event: provider,
                })),
                Ok(_) => None,
                Err(err) => Some(Err(err)),
            })
            .collect::<Result<_>>()?;
        Ok(WorkerPage { entries, has_more })
    }

    /// A page of the orchestrator log, oldest first.
    pub async fn list_orchestrator_log(
        &self,
        id: &ConversationId,
        before: Option<i64>,
        limit: u32,
    ) -> Result<OrchestratorPage> {
        let conversation = self.conversation(id)?;
        let (events, has_more) = self
            .read_page(
                streams::orchestrator(id),
                "orchestrator.logged",
                before,
                limit,
            )
            .await?;
        let entries = events
            .iter()
            .filter_map(|event| match decode(event) {
                Ok(DomainEvent::OrchestratorLogged { entry, .. }) => {
                    Some(Ok(OrchestratorLogEntry {
                        stream_seq: event.stream_seq,
                        at_ms: event.at_ms,
                        entry,
                    }))
                }
                Ok(_) => None,
                Err(err) => Some(Err(err)),
            })
            .collect::<Result<_>>()?;
        let thresholds = match &conversation.setup {
            Some(Setup::Session { orchestrator, .. }) => {
                let window = self
                    .context_usage(id)
                    .await?
                    .and_then(|usage| usage.window_tokens);
                Some(crate::knowledge::rebirth_thresholds(
                    orchestrator.provider,
                    window,
                ))
            }
            _ => None,
        };
        Ok(OrchestratorPage {
            entries,
            has_more,
            thresholds,
        })
    }

    /// Up to `limit` events of `kind` before `before`, oldest first, and whether older ones
    /// exist.
    async fn read_page(
        &self,
        stream: String,
        kind: &str,
        before: Option<i64>,
        limit: u32,
    ) -> Result<(Vec<brigadier_store::StoredEvent>, bool)> {
        let limit = limit.clamp(1, brigadier_store::MAX_PAGE - 1);
        let mut events = self
            .store
            .read_stream(
                stream,
                StreamPage {
                    before,
                    kinds: vec![kind.to_owned()],
                    limit: limit + 1,
                },
            )
            .await?;
        let has_more = events.len() > limit as usize;
        events.truncate(limit as usize);
        events.reverse();
        Ok((events, has_more))
    }

    /// Bytes `offset..offset + limit` of a blob, and its total size.
    pub async fn read_blob_range(
        &self,
        hash: String,
        offset: u64,
        limit: u32,
    ) -> Result<(Vec<u8>, u64)> {
        let hash = hash.parse()?;
        let bytes = self
            .store
            .blobs()
            .get(hash)
            .await?
            .ok_or_else(|| Error::NotFound("artifact".into()))?;
        let total = bytes.len() as u64;
        let start = offset.min(total) as usize;
        let end = start.saturating_add(limit as usize).min(bytes.len());
        Ok((bytes[start..end].to_vec(), total))
    }

    /// Full text of a message stored in the blob store.
    pub async fn read_blob_text(&self, hash: String) -> Result<String> {
        let hash = hash.parse()?;
        let bytes = self
            .store
            .blobs()
            .get(hash)
            .await?
            .ok_or_else(|| Error::NotFound("blob".into()))?;
        String::from_utf8(bytes).map_err(|_| Error::Invalid("blob is not text".into()))
    }

    /// Everything a conversation view shows, with the newest `limit` messages.
    pub async fn conversation_view(
        &self,
        id: ConversationId,
        limit: u32,
    ) -> Result<ConversationView> {
        let conversation = self.conversation(&id)?;
        let messages = self.list_messages(id.clone(), None, limit).await?;
        let board = self.board(&id).await?;
        let context = self.context_usage(&id).await?;
        Ok(ConversationView {
            conversation,
            context,
            messages,
            tasks: board.sorted_tasks(),
            approvals: board.sorted_approvals(),
            questions: board.sorted_questions(),
            plans: board.sorted_plans(),
            requests: board.sorted_requests(),
            queue: board.queue.clone(),
            run: board.run,
            run_request: board.run_request.clone(),
            head: board.head.as_ref().map(|(head, _)| head.clone()),
            worker_steps: board.worker_steps.clone(),
            orchestrator_steps: board.orchestrator_steps.clone(),
            machine_steps: board.machine_steps.clone(),
            thinking: board.thinking.clone(),
            compactions: board.sorted_compactions(),
            decisions: board.decisions.clone(),
            waiting: board.sorted_waiting(),
            ratings: board.ratings.clone(),
            streaming: board.streaming.clone(),
            notices: board.notices.clone(),
            memories: board.memories.clone(),
            overnight: board.sorted_runs(),
        })
    }

    /// The context size the conversation's CLI last reported, from the newest orchestrator
    /// log entries (a window it stopped repeating comes from an earlier report).
    async fn context_usage(&self, id: &ConversationId) -> Result<Option<ContextUsage>> {
        let entries = self
            .store
            .read_stream(
                streams::orchestrator(id),
                StreamPage {
                    before: None,
                    kinds: vec!["orchestrator.logged".into()],
                    limit: CONTEXT_SCAN,
                },
            )
            .await?;
        let mut usage: Option<ContextUsage> = None;
        for stored in &entries {
            let DomainEvent::OrchestratorLogged {
                entry:
                    OrchestratorEntry::Provider {
                        event:
                            ProviderEvent::ContextSize {
                                used_tokens,
                                window_tokens,
                            },
                        ..
                    },
                ..
            } = decode(stored)?
            else {
                continue;
            };
            match &mut usage {
                None => {
                    usage = Some(ContextUsage {
                        used_tokens,
                        window_tokens,
                    });
                }
                Some(latest) if latest.window_tokens.is_none() => {
                    latest.window_tokens = window_tokens;
                }
                Some(_) => break,
            }
            if usage.is_some_and(|latest| latest.window_tokens.is_some()) {
                break;
            }
        }
        Ok(usage)
    }

    /// Appends events to a conversation's stream and applies them to its board. Returns each
    /// event's stream sequence.
    pub async fn record_conversation(
        &self,
        id: &ConversationId,
        events: Vec<DomainEvent>,
    ) -> Result<Vec<i64>> {
        let stream = streams::conversation(id);
        self.record_with_board(
            id,
            events
                .into_iter()
                .map(|event| (stream.clone(), event))
                .collect(),
        )
        .await
    }

    /// Appends events (to any streams) and applies those on the conversation's stream to its
    /// board. Returns each event's stream sequence.
    async fn record_with_board(
        &self,
        id: &ConversationId,
        events: Vec<(String, DomainEvent)>,
    ) -> Result<Vec<i64>> {
        let stream = streams::conversation(id);
        let mut boards = self.boards.lock().await;
        if !boards.contains_key(id) {
            let board = self.load_board(id).await?;
            boards.insert(id.clone(), board);
        }
        let stored = self.record(events.clone()).await?;
        if let Some(board) = boards.get_mut(id) {
            for ((on, event), stream_seq) in events.iter().zip(&stored) {
                if *on == stream {
                    board.apply(event, *stream_seq);
                }
            }
        }
        Ok(stored)
    }

    /// Records a request's new state, unless it already has it. Returns whether it changed.
    pub(crate) async fn update_request(
        &self,
        id: &ConversationId,
        request_id: &str,
        state: RequestState,
    ) -> Result<bool> {
        let mut boards = self.boards.lock().await;
        if !boards.contains_key(id) {
            let board = self.load_board(id).await?;
            boards.insert(id.clone(), board);
        }
        let Some(mut request) = boards
            .get(id)
            .and_then(|board| board.requests.get(request_id))
            .cloned()
        else {
            return Ok(false);
        };
        if request.state == state {
            return Ok(false);
        }
        request.ended_at_ms = match state {
            RequestState::Working => None,
            _ => Some(now_ms()),
        };
        request.state = state;
        let event = DomainEvent::RequestUpdated { request };
        let stored = self
            .record(vec![(streams::conversation(id), event.clone())])
            .await?;
        if let Some(board) = boards.get_mut(id) {
            board.apply(&event, stored[0]);
        }
        Ok(true)
    }

    /// Starts a request over (the user asked for a new answer): it works again from now, so
    /// what its earlier attempt started is older than it.
    pub(crate) async fn restart_request(
        &self,
        id: &ConversationId,
        request_id: &str,
    ) -> Result<()> {
        let mut boards = self.boards.lock().await;
        if !boards.contains_key(id) {
            let board = self.load_board(id).await?;
            boards.insert(id.clone(), board);
        }
        let Some(mut request) = boards
            .get(id)
            .and_then(|board| board.requests.get(request_id))
            .cloned()
        else {
            return Err(Error::NotFound(format!("request {request_id}")));
        };
        request.state = RequestState::Working;
        request.started_at_ms = now_ms();
        request.ended_at_ms = None;
        let event = DomainEvent::RequestUpdated { request };
        let stored = self
            .record(vec![(streams::conversation(id), event.clone())])
            .await?;
        if let Some(board) = boards.get_mut(id) {
            board.apply(&event, stored[0]);
        }
        Ok(())
    }

    /// Records the user's Undo or Reapply of what a request's workers landed.
    pub(crate) async fn set_request_undo(
        &self,
        id: &ConversationId,
        request_id: &str,
        undo: RequestUndo,
    ) -> Result<()> {
        let mut boards = self.boards.lock().await;
        if !boards.contains_key(id) {
            let board = self.load_board(id).await?;
            boards.insert(id.clone(), board);
        }
        let Some(mut request) = boards
            .get(id)
            .and_then(|board| board.requests.get(request_id))
            .cloned()
        else {
            return Err(Error::NotFound(format!("request {request_id}")));
        };
        request.undo = Some(undo);
        let event = DomainEvent::RequestUpdated { request };
        let stored = self
            .record(vec![(streams::conversation(id), event.clone())])
            .await?;
        if let Some(board) = boards.get_mut(id) {
            board.apply(&event, stored[0]);
        }
        Ok(())
    }

    /// Records that a request's message was steered into `into`'s running turn, while
    /// `after` was streaming.
    pub(crate) async fn mark_steered(
        &self,
        id: &ConversationId,
        request_id: &str,
        into: &str,
        after: Option<String>,
    ) -> Result<()> {
        let mut boards = self.boards.lock().await;
        if !boards.contains_key(id) {
            let board = self.load_board(id).await?;
            boards.insert(id.clone(), board);
        }
        let Some(mut request) = boards
            .get(id)
            .and_then(|board| board.requests.get(request_id))
            .cloned()
        else {
            return Err(Error::NotFound(format!("request {request_id}")));
        };
        request.steered_into = Some(into.to_owned());
        request.steered_after = after;
        let event = DomainEvent::RequestUpdated { request };
        let stored = self
            .record(vec![(streams::conversation(id), event.clone())])
            .await?;
        if let Some(board) = boards.get_mut(id) {
            board.apply(&event, stored[0]);
        }
        Ok(())
    }

    /// A snapshot of a conversation's tasks.
    pub async fn tasks(&self, id: &ConversationId) -> Result<Vec<Task>> {
        Ok(self.board(id).await?.sorted_tasks())
    }

    /// The number the next task of a conversation gets (`task-N`).
    pub async fn next_task_number(&self, id: &ConversationId) -> Result<u32> {
        Ok(self.board(id).await?.next_task_number())
    }

    /// Queues a message while a turn runs: last, or at `index` (clamped to the queue).
    /// `deciding` while the orchestrator judges whether it joins the working answer.
    pub async fn enqueue(
        &self,
        id: &ConversationId,
        text: String,
        attachments: Vec<AttachmentRef>,
        mentions: Vec<Mention>,
        index: Option<u32>,
        deciding: bool,
    ) -> Result<QueuedMessage> {
        if text.trim().is_empty() && attachments.is_empty() {
            return Err(Error::Invalid("message is empty".into()));
        }
        check_attachments(&attachments)?;
        let item = QueuedMessage {
            id: uuid::Uuid::now_v7().to_string(),
            text,
            attachments,
            mentions,
            queued_at_ms: now_ms(),
            edited_at_ms: None,
            deciding,
        };
        let queued = item.clone();
        self.change_queue(id, move |queue| {
            if queue.items.len() >= MAX_QUEUED {
                return Err(Error::Invalid(format!(
                    "at most {MAX_QUEUED} messages can wait in the queue"
                )));
            }
            let at = index.map_or(queue.items.len(), |index| {
                (index as usize).min(queue.items.len())
            });
            queue.items.insert(at, queued);
            Ok(())
        })
        .await?;
        Ok(item)
    }

    pub async fn edit_queued(
        &self,
        id: &ConversationId,
        item_id: &str,
        text: String,
        attachments: Vec<AttachmentRef>,
        mentions: Vec<Mention>,
    ) -> Result<MessageQueue> {
        if text.trim().is_empty() && attachments.is_empty() {
            return Err(Error::Invalid("message is empty".into()));
        }
        check_attachments(&attachments)?;
        self.change_queue(id, |queue| {
            let item = find_queued(queue, item_id)?;
            item.text = text;
            item.attachments = attachments;
            item.mentions = mentions;
            item.edited_at_ms = Some(now_ms());
            Ok(())
        })
        .await
    }

    /// Moves a queued message to `index` (clamped to the queue).
    pub async fn move_queued(
        &self,
        id: &ConversationId,
        item_id: &str,
        index: u32,
    ) -> Result<MessageQueue> {
        self.change_queue(id, |queue| {
            let from = queue
                .items
                .iter()
                .position(|item| item.id == item_id)
                .ok_or_else(|| Error::NotFound(format!("queued message {item_id}")))?;
            let item = queue.items.remove(from);
            let to = (index as usize).min(queue.items.len());
            queue.items.insert(to, item);
            Ok(())
        })
        .await
    }

    /// Takes a message out of the queue (deleted, or sent now as a steer).
    pub async fn take_queued(&self, id: &ConversationId, item_id: &str) -> Result<QueuedMessage> {
        let mut taken = None;
        self.change_queue(id, |queue| {
            let index = queue
                .items
                .iter()
                .position(|item| item.id == item_id)
                .ok_or_else(|| Error::NotFound(format!("queued message {item_id}")))?;
            taken = Some(queue.items.remove(index));
            Ok(())
        })
        .await?;
        taken.ok_or_else(|| Error::NotFound(format!("queued message {item_id}")))
    }

    /// Ends the orchestrator's judging of queued messages `item_ids`: those still in the queue
    /// wait there for their turn. Returns the ones that were being judged.
    pub async fn settle_queued(
        &self,
        id: &ConversationId,
        item_ids: &[String],
    ) -> Result<Vec<String>> {
        let board = self.board(id).await?;
        let judged: Vec<String> = board
            .queue
            .items
            .iter()
            .filter(|item| item.deciding && item_ids.contains(&item.id))
            .map(|item| item.id.clone())
            .collect();
        if judged.is_empty() {
            return Ok(judged);
        }
        let settled = judged.clone();
        self.change_queue(id, move |queue| {
            for item in &mut queue.items {
                if settled.contains(&item.id) {
                    item.deciding = false;
                }
            }
            Ok(())
        })
        .await?;
        Ok(judged)
    }

    /// Takes the next message to send, unless the queue is paused or empty.
    pub async fn pop_queued(&self, id: &ConversationId) -> Result<Option<QueuedMessage>> {
        let board = self.board(id).await?;
        if board.queue.paused || board.queue.items.is_empty() {
            return Ok(None);
        }
        let first = board.queue.items[0].id.clone();
        self.take_queued(id, &first).await.map(Some)
    }

    pub async fn set_queue_paused(
        &self,
        id: &ConversationId,
        paused: bool,
    ) -> Result<MessageQueue> {
        self.change_queue(id, |queue| {
            queue.paused = paused;
            Ok(())
        })
        .await
    }

    async fn change_queue(
        &self,
        id: &ConversationId,
        change: impl FnOnce(&mut MessageQueue) -> Result<()>,
    ) -> Result<MessageQueue> {
        self.conversation(id)?;
        // Held across the write so two changes cannot interleave.
        let mut boards = self.boards.lock().await;
        if !boards.contains_key(id) {
            let board = self.load_board(id).await?;
            boards.insert(id.clone(), board);
        }
        let mut queue = boards
            .get(id)
            .map(|board| board.queue.clone())
            .unwrap_or_default();
        change(&mut queue)?;
        let event = DomainEvent::QueueChanged {
            conversation_id: id.clone(),
            queue: queue.clone(),
        };
        let stored = self
            .record(vec![(streams::conversation(id), event.clone())])
            .await?;
        if let Some(board) = boards.get_mut(id) {
            board.apply(&event, stored[0]);
        }
        Ok(queue)
    }

    pub(crate) async fn board(&self, id: &ConversationId) -> Result<Board> {
        let mut boards = self.boards.lock().await;
        if let Some(board) = boards.get(id) {
            return Ok(board.clone());
        }
        let board = self.load_board(id).await?;
        boards.insert(id.clone(), board.clone());
        Ok(board)
    }

    /// Folds a conversation's stream into its board, newest page first.
    async fn load_board(&self, id: &ConversationId) -> Result<Board> {
        let mut events = Vec::new();
        let mut before = None;
        loop {
            let page = self
                .store
                .read_stream(
                    streams::conversation(id),
                    StreamPage {
                        before,
                        kinds: board::KINDS.iter().map(|kind| (*kind).to_owned()).collect(),
                        limit: BOARD_PAGE,
                    },
                )
                .await?;
            let full = page.len() == BOARD_PAGE as usize;
            before = page.last().map(|event| event.stream_seq);
            events.extend(page);
            if !full {
                break;
            }
        }
        let mut board = Board::default();
        for event in events.iter().rev() {
            board.apply(&decode(event)?, event.stream_seq);
        }
        // The newest message is the head unless the user switched branches after it.
        let newest = self
            .store
            .read_stream(
                streams::conversation(id),
                StreamPage {
                    before: None,
                    kinds: vec!["message.appended".into()],
                    limit: 1,
                },
            )
            .await?;
        if let Some(event) = newest.first()
            && board
                .head
                .as_ref()
                .is_none_or(|(_, seq)| *seq < event.stream_seq)
            && let DomainEvent::MessageAppended { message } = decode(event)?
        {
            board.head = Some((message.id, event.stream_seq));
        }
        // Nothing runs across a daemon restart.
        board.run = match board.run {
            crate::work::RunState::Hibernated => crate::work::RunState::Hibernated,
            _ => crate::work::RunState::Idle,
        };
        board.run_request = None;
        board.streaming = None;
        Ok(board)
    }

    /// Records the models an agent's list names as known; one seen after the agent's first
    /// list gets no worker tasks until the user allows it
    /// ([`crate::routing::availability::note_models`]).
    pub async fn note_models(&self, provider: ProviderKind, ids: &[String]) -> Result<()> {
        let _writes = self.settings_writes.lock().await;
        let settings = self.settings();
        if let Some(next) =
            crate::routing::availability::note_models(&settings, provider, ids, now_ms())
        {
            self.write_settings(next).await?;
        }
        Ok(())
    }

    /// Writes settings a client sent: the models recorded as known since it read its copy
    /// stay known ([`crate::routing::availability::rebase`]).
    pub async fn update_settings(&self, settings: Settings) -> Result<Settings> {
        let _writes = self.settings_writes.lock().await;
        let settings = crate::routing::availability::rebase(&self.settings(), settings);
        self.write_settings(settings).await
    }

    async fn write_settings(&self, settings: Settings) -> Result<Settings> {
        self.record(vec![(
            streams::SETTINGS.into(),
            DomainEvent::SettingsChanged {
                settings: settings.clone(),
            },
        )])
        .await?;
        Ok(settings)
    }

    /// Prepares a burst of `count` diagnostic events, `interval_ms` apart, through the normal
    /// write path; clients time each one from ingest to paint. The caller spawns the returned
    /// future.
    pub fn probe_burst(
        self: &Arc<Self>,
        count: u32,
        interval_ms: u32,
    ) -> Result<(ProbeBurst, impl Future<Output = ()> + Send + 'static)> {
        if count == 0 || count > MAX_PROBES {
            return Err(Error::Invalid(format!(
                "probe count must be 1..={MAX_PROBES}"
            )));
        }
        let burst = ProbeBurst {
            burst_id: uuid::Uuid::now_v7().to_string(),
            count,
            interval_ms: interval_ms.min(1_000),
        };
        let core = self.clone();
        let spec = burst.clone();
        let run = async move {
            let period = Duration::from_millis(spec.interval_ms.max(1).into());
            let mut ticker = tokio::time::interval(period);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            for index in 0..spec.count {
                ticker.tick().await;
                let event = DomainEvent::Probe {
                    burst_id: spec.burst_id.clone(),
                    index,
                    count: spec.count,
                };
                let result = async {
                    let new = to_new_event(streams::DIAGNOSTICS.into(), &event)?;
                    let retention = Retention {
                        keep_last: PROBES_KEPT,
                    };
                    core.store.append_with(vec![new], Some(retention)).await?;
                    Ok::<_, Error>(())
                }
                .await;
                if let Err(err) = result {
                    tracing::warn!(error = %err, "probe burst stopped");
                    break;
                }
            }
        };
        Ok((burst, run))
    }

    fn projection(&self) -> MutexGuard<'_, Projection> {
        // The projection holds no invariants a panic could break mid-update.
        self.projection
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    pub fn conversation(&self, id: &ConversationId) -> Result<Conversation> {
        self.projection()
            .conversations
            .get(id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("conversation {id}")))
    }

    /// Appends events atomically, applies them to the projection, and returns each event's
    /// stream sequence.
    /// Records the model standing in for a conversation's chosen one (or that none does now).
    pub(crate) async fn set_fallback(
        &self,
        id: &ConversationId,
        fallback: Option<crate::model::ModelFallback>,
    ) -> Result<()> {
        self.record(vec![(
            streams::CATALOG.into(),
            DomainEvent::ConversationFallback {
                id: id.clone(),
                fallback,
            },
        )])
        .await?;
        Ok(())
    }

    /// Records that a conversation's messages wait for quota (or no longer do).
    pub(crate) async fn set_conversation_wait(
        &self,
        id: &ConversationId,
        wait: Option<crate::work::QuotaWait>,
    ) -> Result<()> {
        self.record(vec![(
            streams::CATALOG.into(),
            DomainEvent::ConversationWaiting {
                id: id.clone(),
                wait,
            },
        )])
        .await?;
        Ok(())
    }

    pub(crate) async fn record(&self, events: Vec<(String, DomainEvent)>) -> Result<Vec<i64>> {
        {
            let projection = self.projection();
            if let Some(id) = events
                .iter()
                .find_map(|(stream, _)| deleted_owner(&projection.deleted, stream))
            {
                // Something late (a report, a worker's last step) after the delete purged it.
                return Err(Error::NotFound(format!("conversation {id} was deleted")));
            }
        }
        let new = events
            .iter()
            .map(|(stream, event)| to_new_event(stream.clone(), event))
            .collect::<Result<Vec<_>>>()?;
        let stored = self.store.append(new).await?;
        let mut projection = self.projection();
        for ((_, event), stored) in events.iter().zip(&stored) {
            projection.apply(event, stored.seq, stored.at_ms);
        }
        Ok(stored.iter().map(|event| event.stream_seq).collect())
    }
}

/// The deleted conversation `stream` belongs to (its transcript, orchestrator or draft).
fn deleted_owner(deleted: &HashSet<ConversationId>, stream: &str) -> Option<ConversationId> {
    if deleted.is_empty() {
        return None;
    }
    let (kind, id) = stream.split_once(':')?;
    let id = ConversationId(id.to_owned());
    (matches!(kind, "conversation" | "orch" | "draft") && deleted.contains(&id)).then_some(id)
}

fn to_new_event(stream: String, event: &DomainEvent) -> Result<NewEvent> {
    Ok(NewEvent::new(stream, event.kind(), now_ms(), event)?)
}

pub(crate) fn decode(event: &brigadier_store::StoredEvent) -> Result<DomainEvent> {
    let mut decoded =
        serde_json::from_str(event.payload.get()).map_err(|source| Error::Corrupt {
            seq: event.seq,
            source,
        })?;
    crate::legacy::normalize(&mut decoded);
    Ok(decoded)
}

/// Checks that `path` is the top-level directory of a git repository.
async fn check_repo(path: String) -> Result<ProjectRepo> {
    tokio::task::spawn_blocking(move || {
        let dir = PathBuf::from(path.trim());
        if !dir.is_absolute() {
            return Err(Error::Invalid(format!(
                "{} is not an absolute path",
                dir.display()
            )));
        }
        let dir = dir
            .canonicalize()
            .map_err(|err| Error::Invalid(format!("{}: {err}", dir.display())))?;
        if !dir.join(".git").exists() {
            return Err(Error::Invalid(format!(
                "{} is not the top-level folder of a git repository",
                dir.display()
            )));
        }
        let name = dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| dir.display().to_string());
        Ok(ProjectRepo {
            path: dir.display().to_string(),
            name,
        })
    })
    .await
    .map_err(|err| Error::Invalid(format!("checking the repository: {err}")))?
}

/// Secret files are paths inside the repository.
fn check_secret_path(file: &str) -> Result<()> {
    let path = Path::new(file);
    let inside = !file.trim().is_empty()
        && path.is_relative()
        && path
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)));
    if inside {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "{file}: secret files are paths relative to the repository root"
        )))
    }
}

fn check_attachments(attachments: &[AttachmentRef]) -> Result<()> {
    if attachments.len() > MAX_ATTACHMENTS {
        return Err(Error::Invalid(format!(
            "at most {MAX_ATTACHMENTS} attachments per message"
        )));
    }
    for attachment in attachments {
        attachment.id.parse::<brigadier_store::BlobHash>()?;
    }
    Ok(())
}

fn find_queued<'a>(queue: &'a mut MessageQueue, item_id: &str) -> Result<&'a mut QueuedMessage> {
    queue
        .items
        .iter_mut()
        .find(|item| item.id == item_id)
        .ok_or_else(|| Error::NotFound(format!("queued message {item_id}")))
}

/// Records a session setup's choices as the project's remembered ones. Returns whether
/// anything changed.
fn remember(project: &mut Project, setup: Option<&Setup>) -> bool {
    let Some(Setup::Session {
        environment,
        permission,
        orchestrator,
        ..
    }) = setup
    else {
        return false;
    };
    let before = project.prefs.clone();
    project.prefs.permission = Some(*permission);
    // Fast spends usage faster: each session opts in again rather than inheriting it.
    project.prefs.orchestrator = Some(ModelChoice {
        fast: None,
        ..orchestrator.clone()
    });
    project.prefs.environment = Some(match environment {
        crate::model::Environment::LocalCheckout { .. } => EnvironmentKind::LocalCheckout,
        crate::model::Environment::NewWorktree { .. } => EnvironmentKind::NewWorktree,
    });
    project.prefs != before
}

/// Whether two environments are the same, ignoring the session worktree path (set once it
/// exists).
fn same_environment(a: &crate::model::Environment, b: &crate::model::Environment) -> bool {
    use crate::model::Environment as E;
    match (a, b) {
        (E::LocalCheckout { branch: a }, E::LocalCheckout { branch: b }) => a == b,
        (
            E::NewWorktree {
                base: base_a,
                branch: branch_a,
                ..
            },
            E::NewWorktree {
                base: base_b,
                branch: branch_b,
                ..
            },
        ) => base_a == base_b && branch_a == branch_b,
        _ => false,
    }
}

fn clean_name(name: &str, what: &str) -> Result<String> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Err(Error::Invalid(format!("{what} is empty")));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(Error::Invalid(format!(
            "{what} is longer than {MAX_NAME_CHARS} characters"
        )));
    }
    Ok(name)
}

/// Valid inline image tokens, using the same attachment rules as model input.
pub(crate) fn inline_image_tokens<'a>(
    text: &str,
    attachments: &'a [AttachmentRef],
) -> Vec<(std::ops::Range<usize>, &'a AttachmentRef)> {
    let mut tokens = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = text[cursor..].find("[image:") {
        let start = cursor + offset;
        let id_start = start + 7;
        // A nested opening bracket means this token is incomplete. Resume there so a real
        // token after the malformed prefix can still match.
        let Some(boundary) = text[id_start..].find(['[', ']']) else {
            break;
        };
        let end = id_start + boundary;
        if text.as_bytes()[end] == b'[' {
            cursor = end;
            continue;
        }
        let id = &text[id_start..end];
        if let Some(attachment) = attachments.iter().find(|a| {
            a.inline.is_some() && a.id == id && brigadier_providers::model::is_image_mime(&a.mime)
        }) {
            tokens.push((start..end + 1, attachment));
        }
        cursor = end + 1;
    }
    // Numbered markers from the AB composer share the same display/title rules.
    for attachment in attachments.iter().filter(|a| {
        a.inline.is_some_and(|n| n > 0) && brigadier_providers::model::is_image_mime(&a.mime)
    }) {
        let marker = format!("[Image #{}]", attachment.inline.unwrap());
        for (start, _) in text.match_indices(&marker) {
            tokens.push((start..start + marker.len(), attachment));
        }
    }
    tokens.sort_by_key(|(range, _)| range.start);
    tokens.dedup_by(|a, b| a.0 == b.0);

    tokens
}

/// Plain text for titles, previews and quoted snippets; never changes the stored message.
pub(crate) fn display_text(text: &str, attachments: &[AttachmentRef]) -> String {
    let mut shown = String::with_capacity(text.len());
    let mut cursor = 0;
    for (range, _) in inline_image_tokens(text, attachments) {
        shown.push_str(&text[cursor..range.start]);
        shown.push_str("[image]");
        cursor = range.end;
    }
    shown.push_str(&text[cursor..]);
    shown
}

fn title_from(text: &str) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(text);
    one_line(line, TITLE_CHARS)
}

/// A request's preview: the start of the user's message, on one line.
fn preview(text: &str) -> String {
    one_line(text, REQUEST_PREVIEW_CHARS)
}

/// `text` with its whitespace collapsed, cut to `chars` characters (with an ellipsis).
pub(crate) fn one_line(text: &str, chars: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= chars {
        line
    } else {
        let mut cut: String = line.chars().take(chars - 1).collect();
        cut.push('…');
        cut
    }
}

/// The parent of a message that starts a conversation's first branch anew (an edited first
/// message). A message without any parent follows the message before it.
pub(crate) const ROOT: &str = "";

/// The parent a message has on its branch: its own, or the message before it.
pub(crate) fn parent_of(messages: &[Message], at: usize) -> String {
    match &messages[at].parent_id {
        Some(parent) => parent.clone(),
        None => at
            .checked_sub(1)
            .map_or_else(|| ROOT.to_owned(), |before| messages[before].id.clone()),
    }
}

/// The branch of `messages` (oldest first, every branch) that ends at `leaf`, oldest first.
/// A message without a parent follows the message before it.
pub(crate) fn branch_of(messages: &[Message], leaf: &str) -> Vec<Message> {
    let index: HashMap<&str, usize> = messages
        .iter()
        .enumerate()
        .map(|(at, message)| (message.id.as_str(), at))
        .collect();
    let mut path = Vec::new();
    let mut at = index.get(leaf).copied();
    while let Some(current) = at {
        let message = &messages[current];
        path.push(message.clone());
        at = match message.parent_id.as_deref() {
            Some(ROOT) => None,
            Some(parent) => index.get(parent).copied(),
            None => current.checked_sub(1),
        };
        // A parent is always older; anything else is a broken link.
        if at.is_some_and(|parent| parent >= current) {
            break;
        }
    }
    path.reverse();
    path
}

/// Adds `block` to `text` as a paragraph of its own.
pub(crate) fn push_block(text: &mut String, block: &str) {
    if text.trim().is_empty() {
        block.clone_into(text);
    } else {
        text.push_str("\n\n");
        text.push_str(block);
    }
}

/// The longest prefix of `text` that fits in `bytes` without splitting a character.
fn prefix(text: &str, bytes: usize) -> &str {
    let mut end = bytes.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(id: &str, inline: bool, mime: &str) -> AttachmentRef {
        AttachmentRef {
            id: id.into(),
            name: "icon.png".into(),
            mime: mime.into(),
            bytes: 1,
            pasted: false,
            inline: inline.then_some(0),
        }
    }

    #[test]
    fn numbered_images_keep_main_title_cleaning_and_literal_markers() {
        let mut attachment = image("a", true, "image/png");
        attachment.inline = Some(2);
        assert_eq!(
            display_text(
                "前[Image #2] again [Image #2] literal [Image #3]",
                &[attachment]
            ),
            "前[image] again [image] literal [Image #3]"
        );
    }

    #[test]
    fn previews_only_replace_eligible_inline_images_and_preserve_literal_text() {
        let attachments = [
            image("a", true, "image/png"),
            image("row", false, "image/png"),
            image("svg", true, "image/svg+xml"),
        ];
        let text = "前[image:a][image:a][image:unknown][image:row][image:svg][Image:a][image:broken[image:a]後[image:unclosed";
        assert_eq!(
            display_text(text, &attachments),
            "前[image][image][image:unknown][image:row][image:svg][Image:a][image:broken[image]後[image:unclosed"
        );
        assert_eq!(display_text(text, &[]), text);
    }

    #[tokio::test]
    async fn auto_titles_and_request_previews_hide_image_ids_without_changing_messages() {
        let dir = std::env::temp_dir().join(format!("brigadier-title-{}", uuid::Uuid::now_v7()));
        let store = Store::open(brigadier_store::StoreConfig {
            db_path: dir.join("test.db"),
            blobs_dir: dir.join("blobs"),
            readers: 1,
        })
        .unwrap();
        let core = Core::load(store.clone()).await.unwrap();
        let mut attachment = core
            .add_attachment("icon.png".into(), "image/png".into(), vec![1], false)
            .await
            .unwrap();
        attachment.inline = Some(0);
        for (text, expected) in [
            (
                format!("Paste test. Here is an icon: [image:{}]", attachment.id),
                "Paste test. Here is an icon: [image]".to_owned(),
            ),
            (format!("[image:{}]", attachment.id), "[image]".to_owned()),
            (
                format!(
                    "See [image:{}] twice [image:{}]",
                    attachment.id, attachment.id
                ),
                "See [image] twice [image]".to_owned(),
            ),
            (
                format!("\n Иконка [image:{}] after\nsecond line", attachment.id),
                "Иконка [image] after".to_owned(),
            ),
            (
                format!("{}[image:{}] after", "界".repeat(54), attachment.id),
                format!("{}[imag…", "界".repeat(54)),
            ),
        ] {
            let conversation = core
                .create_conversation(
                    ConversationId::generate(),
                    ConversationKind::Chat,
                    None,
                    None,
                    None,
                    Origin::default(),
                )
                .await
                .unwrap();
            let message = core
                .append_user_message(
                    conversation.id.clone(),
                    text.clone(),
                    vec![attachment.clone()],
                    vec![],
                )
                .await
                .unwrap();
            assert_eq!(core.conversation(&conversation.id).unwrap().title, expected);
            let board = core.board(&conversation.id).await.unwrap();
            assert_eq!(
                board.requests[&message.id].preview,
                preview(&display_text(&text, &message.attachments))
            );
            assert!(!board.requests[&message.id].preview.contains(&attachment.id));
            assert_eq!(
                core.display_quote(&conversation.id, &text).await,
                display_text(&text, &message.attachments)
            );
            assert_eq!(message.text, text);
            assert_eq!(
                core.list_messages(conversation.id.clone(), None, 10)
                    .await
                    .unwrap()
                    .messages[0]
                    .text,
                text
            );
            assert_eq!(
                core.brief_words(&text, &message.attachments).await,
                display_text(&text, &message.attachments)
            );
            let reloaded = Core::load(store.clone()).await.unwrap();
            assert_eq!(
                reloaded.conversation(&conversation.id).unwrap().title,
                expected
            );
        }
        // A token that is ordinary typed text must remain ordinary text, even in a title.
        for inline in [false, true] {
            let conversation = core
                .create_conversation(
                    ConversationId::generate(),
                    ConversationKind::Chat,
                    None,
                    None,
                    None,
                    Origin::default(),
                )
                .await
                .unwrap();
            let token = "Literal [image:unknown]";
            let refs = if inline {
                vec![attachment.clone()]
            } else {
                vec![]
            };
            core.append_user_message(conversation.id.clone(), token.into(), refs, vec![])
                .await
                .unwrap();
            assert_eq!(core.conversation(&conversation.id).unwrap().title, token);
        }
        let conversation = core
            .create_conversation(
                ConversationId::generate(),
                ConversationKind::Chat,
                None,
                None,
                None,
                Origin::default(),
            )
            .await
            .unwrap();
        core.append_user_message(
            conversation.id.clone(),
            "No inline image here".into(),
            vec![attachment.clone()],
            vec![],
        )
        .await
        .unwrap();
        let token = format!("Literal [image:{}]", attachment.id);
        assert_eq!(core.display_quote(&conversation.id, &token).await, token);
        let mut row = attachment.clone();
        row.inline = None;
        let conversation = core
            .create_conversation(
                ConversationId::generate(),
                ConversationKind::Chat,
                None,
                None,
                None,
                Origin::default(),
            )
            .await
            .unwrap();
        let message = core
            .append_user_message(conversation.id.clone(), token.clone(), vec![row], vec![])
            .await
            .unwrap();
        assert_eq!(
            core.conversation(&conversation.id).unwrap().title,
            title_from(&token)
        );
        assert_eq!(core.brief_words(&token, &message.attachments).await, token);
        store.shutdown().await.unwrap();
        drop(core);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
