//! Workspaces (WORKSPACES-PLAN.md phase 2): a project's groups of task tabs. A tab is a session
//! like any other, on a worktree of its own from its workspace's base; the engine only groups
//! them. A new chat in a project starts a workspace ([`SessionManager::create_conversation`]);
//! more tabs join it here.

use crate::model::{
    Conversation, ConversationId, ConversationKind, EnvironmentRequest, Mention, Setup,
    SetupRequest, Workspace, WorkspaceId,
};
use crate::sessions::Origin;
use crate::work::AttachmentRef;
use crate::{Error, Result};

use super::SessionManager;
use super::conversation::SendOutcome;

impl SessionManager {
    /// Opens a new task tab in `workspace`: a session of its own, on a new worktree from the
    /// workspace's base, in the composer's repository when it is one of the workspace's (else
    /// its first). `setup` brings the composer's model, permission and plan mode; without one
    /// the tab takes the project's remembered model and the default permission.
    pub async fn create_tab(
        &self,
        workspace: &WorkspaceId,
        setup: Option<SetupRequest>,
    ) -> Result<Conversation> {
        self.admit()?;
        let workspace = self.core.workspace(workspace)?;
        let project = self.core.project(&workspace.project_id)?;
        let (repo, permission, orchestrator, plan_mode) = match setup {
            Some(SetupRequest::Session {
                repo,
                permission,
                orchestrator,
                plan_mode,
                ..
            }) => (Some(repo), permission, orchestrator, plan_mode),
            Some(SetupRequest::Chat { .. }) => {
                return Err(Error::Invalid("a tab is a session, not a chat".into()));
            }
            None => {
                let settings = self.core.settings();
                let orchestrator = project
                    .prefs
                    .orchestrator
                    .clone()
                    .or(settings.default_orchestrator)
                    .ok_or_else(|| Error::Invalid("Pick a model for the new tab first.".into()))?;
                (None, settings.default_permission, orchestrator, false)
            }
        };
        let repo = repo
            .filter(|repo| workspace.repos.contains(repo))
            .or_else(|| workspace.repos.first().cloned())
            .or_else(|| project.repos.first().map(|repo| repo.path.clone()))
            .ok_or_else(|| Error::Invalid("The project has no repository.".into()))?;
        if workspace.base.is_empty() {
            return Err(Error::Invalid(
                "This workspace has no branch to start new tabs from.".into(),
            ));
        }
        self.check_choice(&orchestrator)?;
        let id = ConversationId::generate();
        let (prefix, _) = self.branch_prefix_of(Some(&workspace.project_id));
        let setup = Setup::from_request(
            SetupRequest::Session {
                repo,
                environment: EnvironmentRequest::NewWorktree {
                    base: workspace.base.clone(),
                    branch: None,
                },
                permission,
                orchestrator,
                plan_mode,
            },
            &id,
            &prefix,
        );
        let project_id = workspace.project_id.clone();
        self.core
            .create_conversation(
                id,
                ConversationKind::Session,
                Some(project_id),
                None,
                Some(setup),
                Origin {
                    workspace: Some(workspace),
                    ..Origin::default()
                },
            )
            .await
    }

    /// ⌘⏎: a new task tab in `workspace` that starts on the composer's message in the
    /// background, while the user stays where they are.
    pub async fn start_tab(
        &self,
        workspace: &WorkspaceId,
        setup: Option<SetupRequest>,
        text: String,
        attachments: Vec<AttachmentRef>,
        mentions: Vec<Mention>,
    ) -> Result<(Conversation, SendOutcome)> {
        let tab = self.create_tab(workspace, setup).await?;
        match self
            .send_message(tab.id.clone(), text, attachments, mentions, false, None)
            .await
        {
            // As it is now: its first message titled it.
            Ok(outcome) => Ok((self.core.conversation(&tab.id)?, outcome)),
            Err(err) => {
                // The message couldn't go (empty, an attachment gone): no empty tab stays.
                if let Err(err) = self.delete(tab.id.clone()).await {
                    tracing::warn!(conversation = %tab.id, error = %err, "could not remove a tab whose first message failed");
                }
                Err(err)
            }
        }
    }
}

/// The workspace a new chat in a project starts: its first tab's repository, and the branch
/// that tab starts from (a new worktree's base, a local checkout's branch). Its title is the
/// tab's until it is renamed.
pub(super) fn new_workspace(project: &crate::model::ProjectId, setup: &Setup) -> Option<Workspace> {
    let Setup::Session {
        repo, environment, ..
    } = setup
    else {
        return None;
    };
    let now = crate::now_ms();
    Some(Workspace {
        id: WorkspaceId::generate(),
        project_id: project.clone(),
        title: String::new(),
        repos: vec![repo.clone()],
        base: match environment {
            crate::model::Environment::NewWorktree { base, .. } => base.clone(),
            crate::model::Environment::LocalCheckout { branch } => branch.clone(),
        },
        created_at_ms: now,
        updated_at_ms: now,
        pinned_at_ms: None,
        lifecycle: crate::model::Lifecycle::Active,
    })
}
