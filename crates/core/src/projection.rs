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
            // The retired workspace layer: each of its sessions is a plain session of its
            // project, so an old log's workspace events change nothing.
            DomainEvent::WorkspaceCreated { .. }
            | DomainEvent::WorkspaceRenamed { .. }
            | DomainEvent::WorkspacePinned { .. }
            | DomainEvent::WorkspaceArchived { .. } => {}
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
            DomainEvent::PeerAnswered {
                conversation_id, ..
            } => {
                self.peer_at(conversation_id, at_ms);
            }
            DomainEvent::RequestFinished {
                conversation_id, ..
            } => {
                self.finished_at(conversation_id, at_ms);
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
            | DomainEvent::RequestReady { .. }
            | DomainEvent::WorkerStepped { .. }
            | DomainEvent::OrchestratorStepped { .. }
            | DomainEvent::MachineStepped { .. }
            | DomainEvent::CompactionUpdated { .. }
            | DomainEvent::MessageRated { .. }
            | DomainEvent::RatingCleared { .. }
            | DomainEvent::BranchSwitched { .. }
            | DomainEvent::ConversationNotice { .. }
            | DomainEvent::TaskUpdated { .. }
            | DomainEvent::ApprovalUpdated { .. }
            | DomainEvent::QuestionUpdated { .. }
            | DomainEvent::PlanUpdated { .. }
            | DomainEvent::ReviewUpdated { .. }
            | DomainEvent::IntegratedCheckUpdated { .. }
            | DomainEvent::BackgroundRunUpdated { .. }
            | DomainEvent::ThreadCommitsSeen { .. }
            | DomainEvent::UnreviewedChanged { .. }
            | DomainEvent::OutputStored { .. }
            | DomainEvent::Retired
            | DomainEvent::CheckRan { .. }
            | DomainEvent::ThreadLooked { .. }
            | DomainEvent::PreviewUpdated { .. }
            | DomainEvent::PreviewsCleared { .. }
            | DomainEvent::OvernightUpdated { .. }
            | DomainEvent::QueueChanged { .. }
            | DomainEvent::WorkerEvent { .. }
            | DomainEvent::OutputKept { .. }
            | DomainEvent::OrchestratorLogged { .. }
            | DomainEvent::RawSessionUpdated { .. }
            | DomainEvent::RawEvent { .. }
            | DomainEvent::CleanupRecorded { .. }
            | DomainEvent::CleanupRemoved { .. }
            | DomainEvent::BranchesKept { .. }
            | DomainEvent::BranchMade { .. }
            | DomainEvent::CleanupRequested { .. }
            | DomainEvent::CleanupCompleted { .. }
            | DomainEvent::CleanupFinished { .. }
            | DomainEvent::RankingsChanged
            | DomainEvent::ProviderChecked { .. }
            | DomainEvent::AccountsChecked { .. }
            | DomainEvent::QuotaWillBeLost { .. }
            | DomainEvent::UpdatesChanged { .. }
            | DomainEvent::DraftPinned { .. }
            | DomainEvent::BrainJobUpdated { .. }
            | DomainEvent::MemoryUpdated { .. }
            | DomainEvent::DecidedForYou { .. }
            | DomainEvent::WaitingOnYou { .. }
            | DomainEvent::WaitingResolved { .. }
            | DomainEvent::PeerMessageSent { .. }
            | DomainEvent::PeerMessageReceived { .. }
            | DomainEvent::PeerMessageState { .. }
            | DomainEvent::PeerWaitSettled { .. }
            | DomainEvent::PeerReplyHeld { .. }
            | DomainEvent::Probe { .. } => {}
        }
    }

    pub(crate) fn touch(&mut self, id: &ConversationId, at_ms: i64) {
        if let Some(conversation) = self.conversations.get_mut(id) {
            conversation.updated_at_ms = conversation.updated_at_ms.max(at_ms);
        }
    }

    /// `id` finished answering a message from another session at `at_ms` (its sidebar's
    /// unread dot).
    pub(crate) fn peer_at(&mut self, id: &ConversationId, at_ms: i64) {
        if let Some(conversation) = self.conversations.get_mut(id) {
            conversation.peer_at_ms = conversation.peer_at_ms.max(Some(at_ms));
        }
    }

    /// A request of `id` finished at `at_ms` (its sidebar's unread dot).
    pub(crate) fn finished_at(&mut self, id: &ConversationId, at_ms: i64) {
        if let Some(conversation) = self.conversations.get_mut(id) {
            conversation.finished_at_ms = conversation.finished_at_ms.max(Some(at_ms));
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

#[cfg(test)]
mod tests {
    use brigadier_providers::ProviderKind;

    use super::*;
    use crate::model::{
        ConversationKind, Environment, Lifecycle, ModelChoice, PermissionLevel, Setup, WorkspaceId,
    };

    fn project() -> Project {
        Project {
            id: ProjectId("p".into()),
            name: "p".into(),
            created_at_ms: 1,
            repos: Vec::new(),
            prefs: Default::default(),
            trust: Vec::new(),
        }
    }

    fn session(id: &str, environment: Environment) -> Conversation {
        Conversation {
            id: ConversationId(id.into()),
            kind: ConversationKind::Session,
            project_id: Some(ProjectId("p".into())),
            title: format!("title {id}"),
            pinned_at_ms: None,
            created_at_ms: 10,
            updated_at_ms: 10,
            setup: Some(Setup::Session {
                repo: "/repo".into(),
                environment,
                permission: PermissionLevel::ApproveForMe,
                orchestrator: ModelChoice {
                    provider: ProviderKind::Claude,
                    model: None,
                    effort: None,
                    fast: None,
                    account: None,
                },
                workers_see_uncommitted: None,
                plan_mode: false,
            }),
            lifecycle: Lifecycle::Active,
            forked_from: None,
            side_of: None,
            tab_chat: None,
            fallback: None,
            quota_wait: None,
            cleanup_pending: false,
            deleting: false,
            peer_at_ms: None,
            finished_at_ms: None,
        }
    }

    fn worktree(base: &str) -> Environment {
        Environment::NewWorktree {
            base: base.into(),
            branch: "brigadier/x/session".into(),
            path: None,
            start: None,
        }
    }

    fn created(conversation: Conversation) -> DomainEvent {
        DomainEvent::ConversationCreated {
            conversation: Box::new(conversation),
        }
    }

    fn fold(events: &[DomainEvent]) -> Projection {
        let mut projection = Projection::default();
        for (seq, event) in events.iter().enumerate() {
            let seq = seq as i64 + 1;
            projection.apply(event, seq, 100 + seq);
        }
        projection
    }

    #[test]
    fn an_old_workspace_of_several_sessions_reads_as_plain_sessions_of_its_project() {
        // As phase 2 recorded it: a workspace, then its sessions carrying its id.
        let tab = |id: &str| {
            let mut value = serde_json::to_value(created(session(id, worktree("main")))).unwrap();
            value["conversation"]["workspaceId"] = "w1".into();
            serde_json::from_value::<DomainEvent>(value).unwrap()
        };
        let workspace: DomainEvent = serde_json::from_value(serde_json::json!({
            "type": "workspaceCreated",
            "workspace": {
                "id": "w1", "projectId": "p", "title": "Redesigning X", "repos": ["/repo"],
                "base": "main", "createdAtMs": 5, "updatedAtMs": 5, "pinnedAtMs": null,
                "lifecycle": "active"
            }
        }))
        .unwrap();
        let projection = fold(&[
            DomainEvent::ProjectCreated { project: project() },
            workspace,
            tab("a"),
            tab("b"),
            tab("c"),
            created(session("older", worktree("develop"))),
            DomainEvent::WorkspacePinned {
                id: WorkspaceId("w1".into()),
                pinned_at_ms: Some(7),
            },
            DomainEvent::WorkspaceRenamed {
                id: WorkspaceId("w1".into()),
                title: "Redesigning Y".into(),
            },
            DomainEvent::WorkspaceArchived {
                id: WorkspaceId("w1".into()),
                archived: true,
            },
        ]);
        let catalog = projection.catalog();
        let mut ids: Vec<&str> = catalog
            .conversations
            .iter()
            .map(|conversation| conversation.id.0.as_str())
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, ["a", "b", "c", "older"], "nothing is lost");
        for conversation in &catalog.conversations {
            assert_eq!(conversation.project_id, Some(ProjectId("p".into())));
            assert_eq!(conversation.title, format!("title {}", conversation.id));
            assert_eq!(
                conversation.pinned_at_ms, None,
                "the workspace's pin was its own"
            );
            assert_eq!(
                conversation.lifecycle,
                Lifecycle::Active,
                "archiving the workspace never archived its sessions"
            );
            assert!(conversation.is_tab());
        }
        let json = serde_json::to_value(&catalog).unwrap();
        assert!(json.get("workspaces").is_none());
        assert!(
            json["conversations"]
                .as_array()
                .unwrap()
                .iter()
                .all(|conversation| conversation.get("workspaceId").is_none())
        );
    }
}
