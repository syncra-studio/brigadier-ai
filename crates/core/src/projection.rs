use std::collections::{HashMap, HashSet};

use crate::model::{
    Catalog, Conversation, ConversationId, DomainEvent, Project, ProjectId, Settings,
};

/// In-memory view of the catalog, folded from catalog and settings events.
#[derive(Debug, Default)]
pub(crate) struct Projection {
    pub(crate) projects: HashMap<ProjectId, Project>,
    pub(crate) conversations: HashMap<ConversationId, Conversation>,
    /// Conversations that were deleted: nothing more is recorded on their streams.
    pub(crate) deleted: HashSet<ConversationId>,
    pub(crate) settings: Settings,
    /// The engine the store belongs to ([`DomainEvent::EngineSwitched`]); none before the first
    /// start of the thread engine.
    pub(crate) engine: Option<String>,
    /// The conversations an engine's first start is deleting, until it has finished.
    pub(crate) engine_switch: Option<Vec<ConversationId>>,
    /// Global seq of the event that last set each last-writer-wins field. Concurrent writers
    /// can resume in a different order than they committed; an older event must not win.
    title_seqs: HashMap<String, i64>,
    pinned_seqs: HashMap<String, i64>,
    setup_seqs: HashMap<String, i64>,
    lifecycle_seqs: HashMap<String, i64>,
    project_seqs: HashMap<String, i64>,
    settings_seq: i64,
}

impl Projection {
    /// Applies a committed event. `seq` is its global sequence, `at_ms` its ingest time.
    pub(crate) fn apply(&mut self, event: &DomainEvent, seq: i64, at_ms: i64) {
        match event {
            DomainEvent::ProjectCreated { project } => {
                self.projects.insert(project.id.clone(), project.clone());
            }
            DomainEvent::ProjectUpdated { project } => {
                if self.projects.contains_key(&project.id)
                    && latest(&mut self.project_seqs, &project.id.0, seq)
                {
                    self.projects.insert(project.id.clone(), project.clone());
                }
            }
            DomainEvent::ConversationSetUp { id, setup } => {
                if let Some(conversation) = self.conversations.get_mut(id)
                    && latest(&mut self.setup_seqs, &id.0, seq)
                {
                    conversation.setup = Some(setup.clone());
                }
            }
            DomainEvent::ConversationLifecycleChanged { id, lifecycle } => {
                if let Some(conversation) = self.conversations.get_mut(id)
                    && latest(&mut self.lifecycle_seqs, &id.0, seq)
                {
                    conversation.lifecycle = *lifecycle;
                    conversation.updated_at_ms = conversation.updated_at_ms.max(at_ms);
                }
            }
            DomainEvent::ConversationCleanup { id, pending } => {
                if let Some(conversation) = self.conversations.get_mut(id) {
                    conversation.cleanup_pending = *pending;
                }
            }
            DomainEvent::ConversationDeleting { id } => {
                if let Some(conversation) = self.conversations.get_mut(id) {
                    conversation.deleting = true;
                }
            }
            DomainEvent::ConversationDeleted { id } => {
                self.conversations.remove(id);
                self.deleted.insert(id.clone());
            }
            DomainEvent::ProjectRemoved { id } => {
                self.projects.remove(id);
            }
            DomainEvent::EngineSwitching { conversations, .. } => {
                self.engine_switch = Some(conversations.clone());
            }
            DomainEvent::EngineSwitched { engine } => {
                self.engine = Some(engine.clone());
                self.engine_switch = None;
            }
            DomainEvent::ConversationCreated { conversation } => {
                self.conversations
                    .insert(conversation.id.clone(), (**conversation).clone());
            }
            DomainEvent::ConversationRenamed { id, title } => {
                if let Some(conversation) = self.conversations.get_mut(id) {
                    if latest(&mut self.title_seqs, &id.0, seq) {
                        conversation.title.clone_from(title);
                    }
                    conversation.updated_at_ms = conversation.updated_at_ms.max(at_ms);
                }
            }
            DomainEvent::ConversationFallback { id, fallback } => {
                if let Some(conversation) = self.conversations.get_mut(id) {
                    conversation.fallback.clone_from(fallback);
                }
            }
            DomainEvent::ConversationWaiting { id, wait } => {
                if let Some(conversation) = self.conversations.get_mut(id) {
                    conversation.quota_wait.clone_from(wait);
                }
            }
            DomainEvent::ConversationPinned { id, pinned_at_ms } => {
                if let Some(conversation) = self.conversations.get_mut(id)
                    && latest(&mut self.pinned_seqs, &id.0, seq)
                {
                    conversation.pinned_at_ms = *pinned_at_ms;
                }
            }
            DomainEvent::MessageAppended { message } => {
                self.touch(&message.conversation_id, message.created_at_ms);
            }
            DomainEvent::SettingsChanged { settings } => {
                if seq >= self.settings_seq {
                    self.settings = settings.clone();
                    self.settings_seq = seq;
                }
            }
            // Raw sessions, the cleanup ledger and providers belong to the runtime; boards
            // fold the rest of a conversation's stream.
            DomainEvent::RawSessionCreated { .. }
            | DomainEvent::MessageDelta { .. }
            | DomainEvent::ThinkingDelta { .. }
            | DomainEvent::RunStateChanged { .. }
            | DomainEvent::RequestUpdated { .. }
            | DomainEvent::WorkerStepped { .. }
            | DomainEvent::OrchestratorStepped { .. }
            | DomainEvent::MachineStepped { .. }
            | DomainEvent::CompactionUpdated { .. }
            | DomainEvent::MessageRated { .. }
            | DomainEvent::BranchSwitched { .. }
            | DomainEvent::ConversationNotice { .. }
            | DomainEvent::TaskUpdated { .. }
            | DomainEvent::ApprovalUpdated { .. }
            | DomainEvent::QuestionUpdated { .. }
            | DomainEvent::PlanUpdated { .. }
            | DomainEvent::ReviewUpdated { .. }
            | DomainEvent::ThreadCommitsSeen { .. }
            | DomainEvent::OutputStored { .. }
            | DomainEvent::ComputerActed { .. }
            | DomainEvent::CheckRan { .. }
            | DomainEvent::ThreadLooked { .. }
            | DomainEvent::PreviewUpdated { .. }
            | DomainEvent::OvernightUpdated { .. }
            | DomainEvent::QueueChanged { .. }
            | DomainEvent::WorkerEvent { .. }
            | DomainEvent::OrchestratorLogged { .. }
            | DomainEvent::RawSessionUpdated { .. }
            | DomainEvent::RawEvent { .. }
            | DomainEvent::CleanupRecorded { .. }
            | DomainEvent::CleanupRemoved { .. }
            | DomainEvent::BranchesKept { .. }
            | DomainEvent::CleanupRequested { .. }
            | DomainEvent::CleanupCompleted { .. }
            | DomainEvent::RankingsChanged
            | DomainEvent::ProviderChecked { .. }
            | DomainEvent::AccountsChecked { .. }
            | DomainEvent::UpdatesChanged { .. }
            | DomainEvent::DraftPinned { .. }
            | DomainEvent::BrainJobUpdated { .. }
            | DomainEvent::MemoryUpdated { .. }
            | DomainEvent::DecidedForYou { .. }
            | DomainEvent::WaitingOnYou { .. }
            | DomainEvent::WaitingResolved { .. }
            | DomainEvent::Probe { .. } => {}
        }
    }

    pub(crate) fn touch(&mut self, id: &ConversationId, at_ms: i64) {
        if let Some(conversation) = self.conversations.get_mut(id) {
            conversation.updated_at_ms = conversation.updated_at_ms.max(at_ms);
        }
    }

    /// Projects newest first; conversations by most recent activity.
    pub(crate) fn catalog(&self) -> Catalog {
        let mut projects: Vec<Project> = self.projects.values().cloned().collect();
        projects.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms).then(a.id.cmp(&b.id)));
        let mut conversations: Vec<Conversation> = self.conversations.values().cloned().collect();
        conversations.sort_by(|a, b| b.updated_at_ms.cmp(&a.updated_at_ms).then(b.id.cmp(&a.id)));
        Catalog {
            projects,
            conversations,
            settings: self.settings.clone(),
        }
    }
}

/// Records `seq` as the field's newest writer; false if a newer event already set it.
fn latest(seqs: &mut HashMap<String, i64>, id: &str, seq: i64) -> bool {
    let last = seqs.entry(id.to_owned()).or_insert(seq);
    if seq < *last {
        return false;
    }
    *last = seq;
    true
}
