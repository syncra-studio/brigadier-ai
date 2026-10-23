//! A project's sessions (Q73): each one task, on its own branch and worktree. Sessions of the
//! same project see each other on a read-only board (title, state, repository, branch and
//! changed files), and a queued message may go to the sibling it belongs to. A new session starts from another one
//! here (⌘⏎, "Open as a new session", "Start as session").

use crate::model::{
    Conversation, ConversationId, EnvironmentRequest, Lifecycle, Mention, Setup, SetupRequest,
};
use crate::work::AttachmentRef;
use crate::work::{MovedAs, SettledTab, TabState};
use crate::{Error, Result};

use super::SessionManager;
use super::conversation::SendOutcome;

impl SessionManager {
    /// Starts a new session of `from`'s project on `text`, in the background: on a new
    /// worktree from the branch `from` started from (its worktree's base, or its local
    /// checkout's branch), in the composer's repository when it is one of the project's (else
    /// `from`'s). `setup` brings the composer's model, permission and plan mode; without one it
    /// takes `from`'s.
    pub async fn start_session(
        &self,
        from: &ConversationId,
        setup: Option<SetupRequest>,
        text: String,
        attachments: Vec<AttachmentRef>,
        mentions: Vec<Mention>,
    ) -> Result<(Conversation, SendOutcome)> {
        let session = self.create_session_from(from, setup).await?;
        match self
            .send_message(session.id.clone(), text, attachments, mentions, false, None)
            .await
        {
            // As any session: its first message titled it.
            Ok(outcome) => Ok((self.core.conversation(&session.id)?, outcome)),
            Err(err) => {
                // The message couldn't go (empty, an attachment gone): no empty session stays.
                if let Err(err) = self.delete(session.id.clone()).await {
                    tracing::warn!(conversation = %session.id, error = %err, "could not remove a session whose first message failed");
                }
                Err(err)
            }
        }
    }

    /// A new, empty session of `from`'s project, as [`Self::start_session`] makes it.
    pub(crate) async fn create_session_from(
        &self,
        from: &ConversationId,
        setup: Option<SetupRequest>,
    ) -> Result<Conversation> {
        self.admit()?;
        let source = self.core.conversation(from)?;
        let project_id = source
            .project_id
            .clone()
            .filter(|_| source.is_tab())
            .ok_or_else(|| Error::Invalid("Only a session of a project starts another.".into()))?;
        let project = self.core.project(&project_id)?;
        let Some(Setup::Session {
            repo: source_repo,
            environment,
            permission: source_permission,
            orchestrator: source_orchestrator,
            plan_mode: source_plan_mode,
            ..
        }) = source.setup.clone()
        else {
            return Err(Error::Invalid(
                "This session has no setup yet to start another from.".into(),
            ));
        };
        let base = match environment {
            crate::model::Environment::NewWorktree { base, .. } => base,
            crate::model::Environment::LocalCheckout { branch } => branch,
        };
        let (repo, permission, orchestrator, plan_mode) = match setup {
            Some(SetupRequest::Session {
                repo,
                permission,
                orchestrator,
                plan_mode,
                ..
            }) => (Some(repo), permission, orchestrator, plan_mode),
            Some(SetupRequest::Chat { .. }) => {
                return Err(Error::Invalid(
                    "A new session is a session, not a chat.".into(),
                ));
            }
            None => (
                None,
                source_permission,
                source_orchestrator,
                source_plan_mode,
            ),
        };
        let repo = repo
            .filter(|repo| project.repos.iter().any(|known| known.path == *repo))
            .unwrap_or(source_repo);
        if base.is_empty() {
            return Err(Error::Invalid(
                "This session has no branch to start a new one from.".into(),
            ));
        }
        self.check_choice(&orchestrator)?;
        let id = ConversationId::generate();
        let (prefix, _) = self.branch_prefix_of(Some(&project_id));
        let setup = Setup::from_request(
            SetupRequest::Session {
                repo,
                environment: EnvironmentRequest::NewWorktree { base, branch: None },
                permission,
                orchestrator,
                plan_mode,
            },
            &id,
            &prefix,
        );
        self.core
            .create_conversation(
                id,
                crate::model::ConversationKind::Session,
                Some(project_id),
                None,
                Some(setup),
                crate::sessions::Origin::default(),
            )
            .await
    }
}

impl SessionManager {
    /// Every open tab that is ready or merged with nothing running
    /// ([`crate::board::Board::settled_state`]), its branch's commits ahead of base counted now.
    pub async fn settled_tabs(&self) -> Vec<SettledTab> {
        let tabs = self.core.visible_catalog().conversations;
        let mut settled = Vec::new();
        for tab in tabs
            .iter()
            .filter(|tab| tab.is_tab() && tab.lifecycle != Lifecycle::Archived)
        {
            let ahead = self.tab_ahead(tab).await.map(|(_, ahead)| ahead);
            let Ok(board) = self.core.board(&tab.id).await else {
                continue;
            };
            let reverted = self.merge_reverted(tab, &board, ahead).await;
            if let Some(state) = board.settled_state(ahead, reverted) {
                if state == TabState::Ready {
                    self.refresh_pull_request_soon(&tab.id);
                }
                settled.push(SettledTab {
                    conversation_id: tab.id.clone(),
                    state,
                    pull_request: board.pull_request.as_ref().map(|pr| pr.state),
                    proof: board.proof_since_merge(),
                    ahead,
                });
            }
        }
        settled
    }

    /// Whether `tab`'s latest recorded merge is reverted on its base now (asked of git only
    /// when it would read merged: nothing of it ahead).
    async fn merge_reverted(
        &self,
        tab: &Conversation,
        board: &crate::board::Board,
        ahead: Option<u32>,
    ) -> bool {
        if ahead.is_some_and(|ahead| ahead > 0) {
            return false;
        }
        let Some((merge_commit, commits)) = board.latest_merge() else {
            return false;
        };
        let Some(Setup::Session {
            repo,
            environment: crate::model::Environment::NewWorktree { base, .. },
            ..
        }) = tab.setup.clone()
        else {
            return false;
        };
        let git = self.git.clone();
        super::blocking(move || {
            let repo = git
                .open(std::path::Path::new(&repo))
                .map_err(super::git_error)?;
            repo.merge_reverted(&brigadier_git::Oid(merge_commit), commits, &base)
                .map_err(super::git_error)
        })
        .await
        .unwrap_or(false)
    }

    /// What became of merge `merge_commit` of session `id` on its base since: when it was
    /// reverted, and redone after that (`getMergeUndo`). None: not reverted, or not a merge of
    /// this session's.
    pub async fn merge_undo(
        &self,
        id: &ConversationId,
        merge_commit: &str,
    ) -> Result<Option<crate::work::MergeUndo>> {
        let tab = self.core.conversation(id)?;
        let board = self.core.board(id).await?;
        let Some((base, commits)) =
            board
                .orchestrator_steps
                .iter()
                .find_map(|step| match &step.kind {
                    crate::work::OrchestratorStepKind::Merged {
                        merge_commit: Some(commit),
                        base,
                        commits,
                        ..
                    } if commit == merge_commit => Some((base.clone(), *commits)),
                    _ => None,
                })
        else {
            return Ok(None);
        };
        let Some(Setup::Session { repo, .. }) = tab.setup.clone() else {
            return Ok(None);
        };
        let git = self.git.clone();
        let merge_commit = merge_commit.to_owned();
        let undo = super::blocking(move || {
            let repo = git
                .open(std::path::Path::new(&repo))
                .map_err(super::git_error)?;
            repo.merge_undo(&brigadier_git::Oid(merge_commit), commits, &base)
                .map_err(super::git_error)
        })
        .await?;
        Ok(undo.map(|undo| crate::work::MergeUndo {
            reverted_by: undo.reverted.0.0,
            reverted_at_ms: undo.reverted.1,
            redone_by: undo.redone.as_ref().map(|redone| redone.0.0.clone()),
            redone_at_ms: undo.redone.map(|redone| redone.1),
        }))
    }

    /// Whether merge `merge_commit` of session `id` is on its base's upstream now (as last
    /// fetched or pushed): a merge made on this Mac reads pushed once it is. False without an
    /// upstream, or when it isn't a merge of this session's.
    pub async fn merge_pushed(&self, id: &ConversationId, merge_commit: &str) -> Result<bool> {
        let tab = self.core.conversation(id)?;
        let board = self.core.board(id).await?;
        let Some(base) = board
            .orchestrator_steps
            .iter()
            .find_map(|step| match &step.kind {
                crate::work::OrchestratorStepKind::Merged {
                    merge_commit: Some(commit),
                    base,
                    ..
                } if commit == merge_commit => Some(base.clone()),
                _ => None,
            })
        else {
            return Ok(false);
        };
        let Some(Setup::Session { repo, .. }) = tab.setup.clone() else {
            return Ok(false);
        };
        let git = self.git.clone();
        let merge_commit = brigadier_git::Oid(merge_commit.to_owned());
        super::blocking(move || {
            let repo = git
                .open(std::path::Path::new(&repo))
                .map_err(super::git_error)?;
            let Some(upstream) = repo.upstream_tip(&base).map_err(super::git_error)? else {
                return Ok(false);
            };
            repo.ancestor(&merge_commit, &upstream)
                .map_err(super::git_error)
        })
        .await
    }

    /// A worktree tab's branch tip, and how many of its commits its base lacks.
    async fn tab_ahead(&self, tab: &Conversation) -> Option<(String, u32)> {
        let Some(Setup::Session {
            environment: crate::model::Environment::NewWorktree { base, .. },
            ..
        }) = &tab.setup
        else {
            return None;
        };
        self.tab_tip(tab, base).await
    }

    /// Records [`DomainEvent::RequestReady`] for `request` of `id`, which is ending, when its
    /// session branch is ahead of its base now: once per request and head.
    ///
    /// [`DomainEvent::RequestReady`]: crate::model::DomainEvent::RequestReady
    pub(super) async fn note_ready(&self, id: &ConversationId, request: &str) {
        let Ok(tab) = self.core.conversation(id) else {
            return;
        };
        let Some((head, ahead)) = self.tab_ahead(&tab).await else {
            return;
        };
        if ahead == 0 {
            return;
        }
        if let Err(err) = self.core.record_ready(id, request, head).await {
            tracing::debug!(conversation = %id, error = %err, "could not record a ready request");
        }
    }
}

impl SessionManager {
    /// `tab`'s branch tip, and how many of its commits `base` lacks. A commit its upstream has
    /// is not the session's: a new branch starts from the base as its remote has it, which may
    /// be ahead of the local base (#495).
    async fn tab_tip(&self, tab: &Conversation, base: &str) -> Option<(String, u32)> {
        let Some(Setup::Session {
            repo,
            environment: crate::model::Environment::NewWorktree { branch, .. },
            ..
        }) = tab.setup.clone()
        else {
            return None;
        };
        let (git, base) = (self.git.clone(), base.to_owned());
        super::blocking(move || {
            let repo = git
                .open(std::path::Path::new(&repo))
                .map_err(super::git_error)?;
            let (Some(base_tip), Some(tip)) = (
                repo.branch_tip(&base).map_err(super::git_error)?,
                repo.branch_tip(&branch).map_err(super::git_error)?,
            ) else {
                return Ok(None);
            };
            let upstream = repo.upstream_tip(&base).map_err(super::git_error)?;
            let since = repo
                .count_commits_beyond(
                    &[&base_tip].into_iter().chain(&upstream).collect::<Vec<_>>(),
                    &tip,
                )
                .map_err(super::git_error)?;
            Ok(Some((tip.0, since)))
        })
        .await
        .ok()
        .flatten()
    }

    /// The user's Undo (`undo`) or dismissal of moved message `item_id`'s notice. Undo sends
    /// it here again, as if just written, then takes it back from the other tab's queue if it
    /// still waits there (else that tab hears the user took it back). A resend that fails
    /// changes nothing, so the notice stays for another try.
    pub async fn settle_moved(&self, id: &ConversationId, item_id: &str, undo: bool) -> Result<()> {
        if !undo {
            return self.core.take_moved(id, item_id).await.map(|_| ());
        }
        let moved = self
            .core
            .board(id)
            .await?
            .queue
            .moved
            .into_iter()
            .find(|moved| moved.id == item_id)
            .ok_or_else(|| Error::NotFound(format!("moved message {item_id}")))?;
        crate::heap(|| {
            self.send_message(
                id.clone(),
                moved.text.clone(),
                moved.attachments.clone(),
                moved.mentions.clone(),
                false,
                None,
            )
        })
        .await?;
        if let MovedAs::Queued { item_id } = &moved.sent {
            let _ = self.core.take_queued(&moved.to, item_id).await;
        }
        self.core.take_moved(id, item_id).await.map(|_| ())
    }
}
