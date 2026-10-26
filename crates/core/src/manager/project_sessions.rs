//! A project's sessions (Q73): each one task, on its own branch and worktree. Sessions of the
//! same project see each other on a read-only board (title, state, repository, branch and
//! changed files). A new session starts from another one here (⌘⏎, "Start as session").

use std::collections::HashSet;

use crate::board::Board;
use crate::model::{
    Conversation, ConversationId, DomainEvent, EnvironmentRequest, Lifecycle, Mention, MessageRole,
    Setup, SetupRequest,
};
use crate::work::AttachmentRef;
use crate::work::{OrchestratorStep, OrchestratorStepKind, SettledTab, TabState};
use crate::{Error, Result, now_ms};

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

    /// `offer_session` (`brigadier session propose`): a card in the thread with one "Start as
    /// session" button for work that should run apart (the thread judges what; this only
    /// plumbs). Nothing starts until the user clicks it ([`Self::start_offered_session`]). The
    /// same task once a request.
    pub(crate) async fn offer_session(
        &self,
        id: &ConversationId,
        args: crate::tools::OfferSession,
    ) -> Result<String> {
        let task = args.task.trim();
        if task.is_empty() {
            return Err(Error::Invalid(
                "`task` must say what the session is to do".into(),
            ));
        }
        let title = args
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map_or_else(|| offer_title(task), str::to_owned);
        let conv = self.conv(id)?;
        let _offer = conv.session_offer.lock().await;
        offering_from(&self.core.conversation(id)?)?;
        let request = self.request_for(id).await;
        let board = self.core.board(id).await?;
        if board.orchestrator_steps.iter().any(|step| {
            step.request_id == request
                && matches!(&step.kind, OrchestratorStepKind::SessionOffered { task: offered, .. } if offered == task)
        }) {
            return Err(Error::Invalid(
                "This task was already offered for this request: don't offer it again.".into(),
            ));
        }
        self.store_step(
            id,
            request,
            OrchestratorStepKind::SessionOffered {
                id: uuid::Uuid::now_v7().to_string(),
                title,
                task: task.to_owned(),
            },
        )
        .await?;
        Ok("The user sees it as a card with a Start as session button: one click starts it as a session of its own, on its own branch. Don't start it yourself or do it here; carry on with the request.".into())
    }

    /// The user's Start as session on the offer `offer_id` in `from` (`offer_session`): its
    /// task starts as a new session of the project, as ⌘⏎ starts one, and the offer is spent.
    /// Once only: the checks, the new session and its record hold `from`'s offer lock, so a
    /// second click waits and is refused. An offer out of view (from an attempt the user ran again, or
    /// a message they edited away) or from a session archived or being deleted starts nothing.
    pub async fn start_offered_session(
        &self,
        from: &ConversationId,
        offer_id: &str,
    ) -> Result<(Conversation, SendOutcome)> {
        let conv = self.conv(from)?;
        let _offer = conv.session_offer.lock().await;
        offering_from(&self.core.conversation(from)?)?;
        let board = self.core.board(from).await?;
        let Some(offer) = board.orchestrator_steps.iter().find(|step| {
            matches!(&step.kind, OrchestratorStepKind::SessionOffered { id, .. } if id == offer_id)
        }) else {
            return Err(Error::NotFound(format!("session offer {offer_id}")));
        };
        let OrchestratorStepKind::SessionOffered { task, .. } = &offer.kind else {
            unreachable!("found as an offer");
        };
        if board.orchestrator_steps.iter().any(|step| {
            matches!(&step.kind, OrchestratorStepKind::SessionOfferTaken { id, .. } if id == offer_id)
        }) {
            return Err(Error::Invalid("This session was already started.".into()));
        }
        if !self.in_view(from, &board).await?(offer) {
            return Err(Error::Invalid(
                "This offer is from an earlier answer: ask again for it.".into(),
            ));
        }
        let (request, task) = (offer.request_id.clone(), task.clone());
        // Taken is on record before its task goes: the offer can't start a second session, and
        // nothing runs in one whose record failed.
        let session = self.create_session_from(from, None).await?;
        let taken = OrchestratorStepKind::SessionOfferTaken {
            id: offer_id.to_owned(),
            session: session.id.clone(),
        };
        if let Err(err) = self.store_step(from, request, taken).await {
            // Nothing works in it yet: the empty session goes, and the offer stays open.
            if let Err(err) = self.delete(session.id.clone()).await {
                tracing::warn!(conversation = %session.id, error = %err, "could not remove a session whose offer could not be marked taken");
            }
            return Err(err);
        }
        // The session is the offer's now: one whose task couldn't go stays, for the user to
        // send it there.
        let outcome = self
            .send_message(session.id.clone(), task, Vec::new(), Vec::new(), false, None)
            .await
            .map_err(|err| {
                Error::Invalid(format!(
                    "The session started, but its task couldn't be sent ({err}): open it and send the task there."
                ))
            })?;
        // As any session: its first message titled it.
        Ok((self.core.conversation(&session.id)?, outcome))
    }

    /// Records the thread's step `kind` in `id`'s request `request`, failing if it can't.
    async fn store_step(
        &self,
        id: &ConversationId,
        request: Option<String>,
        kind: OrchestratorStepKind,
    ) -> Result<()> {
        let step = OrchestratorStep {
            request_id: request,
            kind,
            at_ms: now_ms(),
            position: 0,
        };
        self.core
            .record_conversation(id, vec![DomainEvent::OrchestratorStepped { step }])
            .await
            .map(drop)
    }

    /// Whether a step of `id`'s `board` is in view, as the thread shows it: not from a request's
    /// earlier attempt (before a Try again), nor from a message the user edited away.
    async fn in_view<'a>(
        &self,
        id: &ConversationId,
        board: &'a Board,
    ) -> Result<impl Fn(&crate::work::OrchestratorStep) -> bool + 'a> {
        let messages = match &board.head {
            Some((head, _)) => self.core.branch(id, head).await?,
            None => self.core.all_messages(id).await?,
        };
        let shown: HashSet<String> = messages
            .into_iter()
            .filter(|message| message.role == MessageRole::User)
            .map(|message| message.request_id.unwrap_or(message.id))
            .collect();
        Ok(
            move |step: &crate::work::OrchestratorStep| match &step.request_id {
                Some(request) => {
                    shown.contains(request)
                        && board
                            .requests
                            .get(request)
                            .is_none_or(|request| step.at_ms >= request.started_at_ms)
                }
                None => true,
            },
        )
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
        self.tab_marks().await.0
    }

    /// [`Self::settled_tabs`], and the open sessions with nothing left to merge: no commit
    /// beyond their base or its upstream, and nothing uncommitted in their worktree. Ready or
    /// not, their thread may have put their work on the base itself (D61).
    pub async fn tab_marks(&self) -> (Vec<SettledTab>, Vec<ConversationId>) {
        let tabs = self.core.visible_catalog().conversations;
        let mut settled = Vec::new();
        let mut landed = Vec::new();
        for tab in tabs
            .iter()
            .filter(|tab| tab.is_tab() && tab.lifecycle != Lifecycle::Archived)
        {
            let ahead = self.tab_ahead(tab).await.map(|(_, ahead)| ahead);
            if ahead == Some(0) && self.tab_worktree_clean(tab).await {
                landed.push(tab.id.clone());
            }
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
        (settled, landed)
    }

    /// Whether `tab`'s session worktree has nothing uncommitted (or is gone); false when git
    /// can't tell.
    async fn tab_worktree_clean(&self, tab: &Conversation) -> bool {
        let Some(Setup::Session {
            environment: crate::model::Environment::NewWorktree { path, .. },
            ..
        }) = tab.setup.clone()
        else {
            return false;
        };
        let Some(path) = path.filter(|path| std::path::Path::new(path).exists()) else {
            return true;
        };
        let git = self.git.clone();
        super::blocking(move || {
            let worktree = git
                .open(std::path::Path::new(&path))
                .map_err(super::git_error)?;
            let state = worktree.state().map_err(super::git_error)?;
            Ok(state.dirty_files.is_empty())
        })
        .await
        .unwrap_or(false)
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
}

/// Refuses a session offer from anything but an open session of a project: a Chat, a side
/// chat or a tab's chat has no project to open one in, and an archived session or one being
/// deleted offers nothing more.
fn offering_from(conversation: &Conversation) -> Result<()> {
    if !conversation.is_tab() || conversation.project_id.is_none() {
        return Err(Error::Invalid(
            "Only a session of a project offers a new session.".into(),
        ));
    }
    if conversation.deleting || conversation.lifecycle == Lifecycle::Archived {
        return Err(Error::Invalid(
            "This session is archived: it offers no new session.".into(),
        ));
    }
    Ok(())
}

/// A session offer's name when the thread gave none: the task's first sentence, cut to a few
/// words.
fn offer_title(task: &str) -> String {
    let first = task.split(['.', ':', '\n']).next().unwrap_or(task).trim();
    let words: Vec<&str> = first.split_whitespace().take(8).collect();
    let title = words.join(" ");
    if first.split_whitespace().count() > words.len() {
        format!("{title}…")
    } else {
        title
    }
}
