use std::collections::{HashMap, HashSet};

use crate::model::{
    Catalog, Conversation, ConversationId, DomainEvent, Environment, Lifecycle, Project, ProjectId,
    Settings, Setup, Workspace, WorkspaceId,
};

/// In-memory view of the catalog, folded from catalog and settings events.
#[derive(Debug, Default)]
pub(crate) struct Projection {
    pub(crate) projects: HashMap<ProjectId, Project>,
    pub(crate) conversations: HashMap<ConversationId, Conversation>,
    /// Recorded workspaces. Older sessions' implicit ones are made on reading
    /// ([`Projection::workspace`]).
    workspaces: HashMap<WorkspaceId, Workspace>,
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
    workspace_title_seqs: HashMap<String, i64>,
    workspace_pinned_seqs: HashMap<String, i64>,
    workspace_lifecycle_seqs: HashMap<String, i64>,
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
                self.workspaces
                    .retain(|_, workspace| workspace.project_id != *id);
            }
            DomainEvent::EngineSwitching { conversations, .. } => {
                self.engine_switch = Some(conversations.clone());
            }
            DomainEvent::EngineSwitched { engine } => {
                self.engine = Some(engine.clone());
                self.engine_switch = None;
            }
            DomainEvent::ConversationCreated { conversation } => {
                let mut conversation = (**conversation).clone();
                if conversation.is_tab() && conversation.workspace_id.is_none() {
                    conversation.workspace_id = Some(WorkspaceId::implicit(&conversation.id));
                }
                self.conversations
                    .insert(conversation.id.clone(), conversation);
            }
            DomainEvent::WorkspaceCreated { workspace } => {
                self.workspaces
                    .insert(workspace.id.clone(), workspace.clone());
            }
            DomainEvent::WorkspaceRenamed { id, title } => {
                if let Some(workspace) = self.workspaces.get_mut(id) {
                    if latest(&mut self.workspace_title_seqs, &id.0, seq) {
                        workspace.title.clone_from(title);
                    }
                    workspace.updated_at_ms = workspace.updated_at_ms.max(at_ms);
                }
            }
            DomainEvent::WorkspacePinned { id, pinned_at_ms } => {
                if let Some(workspace) = self.workspaces.get_mut(id)
                    && latest(&mut self.workspace_pinned_seqs, &id.0, seq)
                {
                    workspace.pinned_at_ms = *pinned_at_ms;
                    workspace.updated_at_ms = workspace.updated_at_ms.max(at_ms);
                }
            }
            DomainEvent::WorkspaceArchived { id, archived } => {
                if let Some(workspace) = self.workspaces.get_mut(id)
                    && latest(&mut self.workspace_lifecycle_seqs, &id.0, seq)
                {
                    workspace.lifecycle = if *archived {
                        Lifecycle::Archived
                    } else {
                        Lifecycle::Active
                    };
                    workspace.updated_at_ms = workspace.updated_at_ms.max(at_ms);
                }
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

    /// A workspace: a recorded one, or the implicit one of a session recorded before
    /// workspaces existed, made from that session (its title, pin, archive state, repository
    /// and base).
    pub(crate) fn workspace(&self, id: &WorkspaceId) -> Option<Workspace> {
        self.workspaces()
            .into_iter()
            .find(|workspace| workspace.id == *id)
    }

    /// Whether the workspace `id` was recorded without a title, and has none yet.
    pub(crate) fn untitled(&self, id: &WorkspaceId) -> bool {
        self.workspaces
            .get(id)
            .is_some_and(|workspace| workspace.title.is_empty())
    }

    /// Whether the workspace `id` was recorded (not only an older session's implicit one).
    pub(crate) fn is_recorded(&self, id: &WorkspaceId) -> bool {
        self.workspaces.contains_key(id)
    }

    /// Every workspace, recorded or implicit, with the most recent activity of its tabs. One
    /// recorded without a title has its oldest tab's (a new chat's, titled from its first
    /// message).
    pub(crate) fn workspaces(&self) -> Vec<Workspace> {
        let mut workspaces = self.workspaces.clone();
        // The oldest tab of each, for a title.
        let mut oldest: HashMap<&WorkspaceId, &Conversation> = HashMap::new();
        for conversation in self.conversations.values() {
            let Some(id) = &conversation.workspace_id else {
                continue;
            };
            let first = oldest.entry(id).or_insert(conversation);
            if (conversation.created_at_ms, &conversation.id) < (first.created_at_ms, &first.id) {
                *first = conversation;
            }
            if !workspaces.contains_key(id)
                && *id == WorkspaceId::implicit(&conversation.id)
                && let Some(workspace) = implicit_workspace(conversation)
            {
                workspaces.insert(id.clone(), workspace);
            }
            if let Some(workspace) = workspaces.get_mut(id) {
                workspace.updated_at_ms = workspace.updated_at_ms.max(conversation.updated_at_ms);
            }
        }
        for (id, tab) in oldest {
            if let Some(workspace) = workspaces.get_mut(id)
                && workspace.title.is_empty()
            {
                workspace.title.clone_from(&tab.title);
            }
        }
        let mut workspaces: Vec<Workspace> = workspaces.into_values().collect();
        workspaces.sort_by(|a, b| b.updated_at_ms.cmp(&a.updated_at_ms).then(b.id.cmp(&a.id)));
        workspaces
    }

    /// Projects newest first; workspaces and conversations by most recent activity.
    pub(crate) fn catalog(&self) -> Catalog {
        let mut projects: Vec<Project> = self.projects.values().cloned().collect();
        projects.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms).then(a.id.cmp(&b.id)));
        let mut conversations: Vec<Conversation> = self.conversations.values().cloned().collect();
        conversations.sort_by(|a, b| b.updated_at_ms.cmp(&a.updated_at_ms).then(b.id.cmp(&a.id)));
        Catalog {
            projects,
            workspaces: self.workspaces(),
            conversations,
            settings: self.settings.clone(),
        }
    }
}

/// The workspace of one tab a session recorded before workspaces existed belongs to.
fn implicit_workspace(session: &Conversation) -> Option<Workspace> {
    let project_id = session.project_id.clone()?;
    let (repos, base) = match &session.setup {
        Some(Setup::Session {
            repo, environment, ..
        }) => (
            vec![repo.clone()],
            match environment {
                Environment::LocalCheckout { branch } => branch.clone(),
                Environment::NewWorktree { base, .. } => base.clone(),
            },
        ),
        _ => (Vec::new(), String::new()),
    };
    Some(Workspace {
        id: WorkspaceId::implicit(&session.id),
        project_id,
        title: session.title.clone(),
        repos,
        base,
        created_at_ms: session.created_at_ms,
        updated_at_ms: session.updated_at_ms,
        pinned_at_ms: session.pinned_at_ms,
        lifecycle: match session.lifecycle {
            Lifecycle::Archived => Lifecycle::Archived,
            Lifecycle::Active | Lifecycle::Hibernated => Lifecycle::Active,
        },
    })
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
    use crate::model::{ConversationKind, ModelChoice, PermissionLevel};

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
            workspace_id: None,
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
    fn an_old_log_gives_each_session_a_workspace_of_one_tab() {
        let mut side = session("side", worktree("main"));
        side.side_of = Some(ConversationId("a".into()));
        let mut chat = session("chat", worktree("main"));
        chat.kind = ConversationKind::Chat;
        chat.project_id = None;
        chat.setup = None;
        let projection = fold(&[
            DomainEvent::ProjectCreated { project: project() },
            created(session("a", worktree("develop"))),
            created(session(
                "b",
                Environment::LocalCheckout {
                    branch: "feature".into(),
                },
            )),
            created(side),
            created(chat),
            DomainEvent::ConversationPinned {
                id: ConversationId("a".into()),
                pinned_at_ms: Some(50),
            },
            DomainEvent::ConversationLifecycleChanged {
                id: ConversationId("b".into()),
                lifecycle: Lifecycle::Archived,
            },
        ]);
        let catalog = projection.catalog();
        assert_eq!(catalog.workspaces.len(), 2, "{:?}", catalog.workspaces);

        let a = projection.workspace(&WorkspaceId("ws-a".into())).unwrap();
        assert_eq!(a.title, "title a");
        assert_eq!(a.project_id, ProjectId("p".into()));
        assert_eq!(a.repos, vec!["/repo".to_owned()]);
        assert_eq!(a.base, "develop");
        assert_eq!(a.pinned_at_ms, Some(50));
        assert_eq!(a.lifecycle, Lifecycle::Active);

        let b = projection.workspace(&WorkspaceId("ws-b".into())).unwrap();
        assert_eq!(b.base, "feature", "a local checkout's base is its branch");
        assert_eq!(b.lifecycle, Lifecycle::Archived);

        let tab = |id: &str| {
            catalog
                .conversations
                .iter()
                .find(|conversation| conversation.id.0 == id)
                .unwrap()
                .workspace_id
                .clone()
        };
        assert_eq!(tab("a"), Some(WorkspaceId("ws-a".into())));
        assert_eq!(tab("side"), None, "a side chat is no tab");
        assert_eq!(tab("chat"), None, "a chat is no tab");
    }

    #[test]
    fn an_implicit_workspace_follows_its_session_until_it_is_recorded() {
        let id = WorkspaceId("ws-a".into());
        let mut projection = fold(&[
            DomainEvent::ProjectCreated { project: project() },
            created(session("a", worktree("main"))),
            DomainEvent::ConversationRenamed {
                id: ConversationId("a".into()),
                title: "Redesigning X".into(),
            },
        ]);
        assert_eq!(projection.workspace(&id).unwrap().title, "Redesigning X");

        let recorded = projection.workspace(&id).unwrap();
        projection.apply(
            &DomainEvent::WorkspaceCreated {
                workspace: recorded,
            },
            10,
            200,
        );
        projection.apply(
            &DomainEvent::WorkspaceRenamed {
                id: id.clone(),
                title: "Workspace".into(),
            },
            11,
            201,
        );
        projection.apply(
            &DomainEvent::ConversationRenamed {
                id: ConversationId("a".into()),
                title: "Tab".into(),
            },
            12,
            202,
        );
        let workspaces = projection.workspaces();
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].title, "Workspace", "the recorded one wins");
        assert_eq!(
            workspaces[0].updated_at_ms, 202,
            "its tabs' activity counts"
        );
    }

    #[test]
    fn a_new_log_round_trips() {
        let id = WorkspaceId("w1".into());
        let mut one = session("a", worktree("main"));
        one.workspace_id = Some(id.clone());
        let mut two = session("b", worktree("main"));
        two.workspace_id = Some(id.clone());
        let events = vec![
            DomainEvent::ProjectCreated { project: project() },
            DomainEvent::WorkspaceCreated {
                workspace: Workspace {
                    id: id.clone(),
                    project_id: ProjectId("p".into()),
                    title: "Redesigning X".into(),
                    repos: vec!["/repo".into()],
                    base: "main".into(),
                    created_at_ms: 5,
                    updated_at_ms: 5,
                    pinned_at_ms: None,
                    lifecycle: Lifecycle::Active,
                },
            },
            created(one),
            created(two),
            DomainEvent::WorkspacePinned {
                id: id.clone(),
                pinned_at_ms: Some(7),
            },
            DomainEvent::WorkspaceRenamed {
                id: id.clone(),
                title: "Redesigning Y".into(),
            },
            DomainEvent::WorkspaceArchived {
                id: id.clone(),
                archived: true,
            },
        ];
        let decoded: Vec<DomainEvent> = events
            .iter()
            .map(|event| serde_json::from_str(&serde_json::to_string(event).unwrap()).unwrap())
            .collect();
        assert_eq!(decoded, events);

        let catalog = fold(&decoded).catalog();
        assert_eq!(catalog, fold(&events).catalog());
        assert_eq!(catalog.workspaces.len(), 1, "two tabs, one workspace");
        let workspace = &catalog.workspaces[0];
        assert_eq!(workspace.title, "Redesigning Y");
        assert_eq!(workspace.pinned_at_ms, Some(7));
        assert_eq!(workspace.lifecycle, Lifecycle::Archived);
        assert!(
            catalog
                .conversations
                .iter()
                .all(|conversation| conversation.workspace_id == Some(id.clone()))
        );
    }

    #[test]
    fn an_older_workspace_rename_does_not_win() {
        let id = WorkspaceId("ws-a".into());
        let mut projection = fold(&[
            DomainEvent::ProjectCreated { project: project() },
            created(session("a", worktree("main"))),
        ]);
        let recorded = projection.workspace(&id).unwrap();
        projection.apply(
            &DomainEvent::WorkspaceCreated {
                workspace: recorded,
            },
            3,
            103,
        );
        for (seq, title) in [(5, "newer"), (4, "older")] {
            projection.apply(
                &DomainEvent::WorkspaceRenamed {
                    id: id.clone(),
                    title: title.into(),
                },
                seq,
                100 + seq,
            );
        }
        assert_eq!(projection.workspace(&id).unwrap().title, "newer");
    }

    #[test]
    fn a_lookup_counts_tab_activity_and_a_removed_project_takes_its_workspaces() {
        let id = WorkspaceId("w1".into());
        let mut tab = session("a", worktree("main"));
        tab.workspace_id = Some(id.clone());
        let mut projection = fold(&[
            DomainEvent::ProjectCreated { project: project() },
            DomainEvent::WorkspaceCreated {
                workspace: Workspace {
                    id: id.clone(),
                    project_id: ProjectId("p".into()),
                    title: "w".into(),
                    repos: Vec::new(),
                    base: "main".into(),
                    created_at_ms: 5,
                    updated_at_ms: 5,
                    pinned_at_ms: None,
                    lifecycle: Lifecycle::Active,
                },
            },
            created(tab),
        ]);
        projection.apply(
            &DomainEvent::WorkspacePinned {
                id: id.clone(),
                pinned_at_ms: Some(1),
            },
            4,
            300,
        );
        assert_eq!(projection.workspace(&id).unwrap().updated_at_ms, 300);
        projection.touch(&ConversationId("a".into()), 400);
        assert_eq!(projection.workspace(&id).unwrap().updated_at_ms, 400);

        projection.apply(
            &DomainEvent::ConversationDeleted {
                id: ConversationId("a".into()),
            },
            5,
            401,
        );
        projection.apply(
            &DomainEvent::ProjectRemoved {
                id: ProjectId("p".into()),
            },
            6,
            402,
        );
        assert!(projection.workspace(&id).is_none());
        assert!(projection.workspaces().is_empty());
    }
}
