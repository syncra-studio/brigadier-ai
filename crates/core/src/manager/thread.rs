//! The session's thread (THREAD-PLAN.md Q1, Q6): the CLI session that talks with the user,
//! reads, runs and makes tiny edits itself, and runs the workers.
//!
//! Its working directory stays its own scratch folder (`orch/<id>`), which keeps `--resume`
//! working. The code it works on is its *workspace*, given to the CLI as an extra folder: the
//! session's checkout. The workspace
//! is never recorded under the thread's cleanup-ledger owner, so ending the thread's processes
//! (hibernation, rebirth, fallback) never reaches what runs there.
//!
//! The thread's CLI runs at the session's permission level, passed straight through as its
//! CLI's own permission mode, as a worker's is (G5). What its CLI was started with is remembered on it ([`ThreadLaunch`]): when the workspace or
//! the level changes, the CLI is closed between turns and the next turn resumes the same
//! native session with the new ones. Nothing tells the thread: a resumed session keeps the
//! instructions it started with, and the next fresh conversation starts on the new ones.
//!
//! Commits the thread makes itself get their one review like any landing (THREAD-PLAN.md Q4,
//! Q12). They are the commits that appear on the workspace's branch during a thread turn and
//! are not a landing's, or carry the [`super::prompts::THREAD_TRAILER`]. The branch is looked
//! at when a turn starts and ends, as a landing moves it, and before a merge; what was looked
//! at is recorded per branch ([`DomainEvent::ThreadCommitsSeen`]), so a restart neither
//! reviews a range again nor misses one.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use brigadier_git::Oid;
use brigadier_providers::{
    Access, ApprovalDecision, ApprovalRequest, Decider, ProviderEvent, ProviderKind,
};

use super::conversation::{Cli, ConvLive};
use super::workers::thread_temp_owner;
use super::{SessionManager, blocking, git_error};
use crate::model::{
    ConversationId, ConversationKind, DomainEvent, Environment, PermissionLevel, Setup,
};
use crate::{Error, Result};

/// Where the thread works: a checkout of the session's repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThreadWorkspace {
    pub path: PathBuf,
    /// The branch checked out there, which accepted work lands on.
    pub branch: String,
    /// The repository it belongs to (the session's).
    pub repo: PathBuf,
}

impl ThreadWorkspace {
    /// How [`crate::work::Told::workspace`] keeps it.
    pub(crate) fn told(&self) -> String {
        format!("{} @ {}", self.path.display(), self.branch)
    }
}

/// What a thread's CLI was started for.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ThreadLaunch {
    pub workspace: Option<ThreadWorkspace>,
    pub permission: PermissionLevel,
    /// Its CLI's permission mode, the level's.
    pub access: Access,
    /// Its output hook's grant, under which the subfolder rules its CLI session has had are
    /// kept; `None` without the hook (it has every rule up front).
    pub hook_grant: Option<String>,
}

impl ThreadLaunch {
    /// Whether a CLI started for `self` must start again for `now`: another folder, or
    /// another level (its permission mode is fixed when it starts).
    pub(crate) fn outdated(&self, now: &ThreadLaunch) -> bool {
        self.permission != now.permission
            || self.workspace.as_ref().map(|w| &w.path) != now.workspace.as_ref().map(|w| &w.path)
    }
}

impl SessionManager {
    /// The thread's workspace as recorded now, without making anything: the session's
    /// checkout (`None` for a new-worktree session whose worktree isn't made yet, and for a
    /// Chat).
    pub(crate) fn recorded_workspace(&self, id: &ConversationId) -> Option<ThreadWorkspace> {
        let conversation = self.core.conversation(id).ok()?;
        let Some(Setup::Session {
            repo, environment, ..
        }) = &conversation.setup
        else {
            return None;
        };
        let repo = PathBuf::from(repo);
        match environment {
            Environment::LocalCheckout { branch } => Some(ThreadWorkspace {
                path: repo.clone(),
                branch: branch.clone(),
                repo,
            }),
            Environment::NewWorktree {
                branch,
                path: Some(path),
                ..
            } => Some(ThreadWorkspace {
                path: PathBuf::from(path),
                branch: branch.clone(),
                repo,
            }),
            Environment::NewWorktree { path: None, .. } => None,
        }
    }

    /// The thread's workspace as it will be, without making anything: the recorded one, or the
    /// session worktree not made yet (after a merge removed it, THREAD-PLAN.md Q9), which the
    /// next user message makes at that same path.
    pub(crate) fn planned_workspace(&self, id: &ConversationId) -> Option<ThreadWorkspace> {
        if let Some(workspace) = self.recorded_workspace(id) {
            return Some(workspace);
        }
        let conversation = self.core.conversation(id).ok()?;
        let Some(Setup::Session {
            repo,
            environment: Environment::NewWorktree { branch, .. },
            ..
        }) = &conversation.setup
        else {
            return None;
        };
        Some(ThreadWorkspace {
            path: self.session_worktree_path(&conversation),
            branch: branch.clone(),
            repo: PathBuf::from(repo),
        })
    }

    /// The thread's effective workspace (THREAD-PLAN.md Q1, Q10): the session's checkout, its
    /// own worktree made first when it has none yet. `None` for a Chat.
    pub(crate) async fn effective_workspace(
        &self,
        id: &ConversationId,
    ) -> Result<Option<ThreadWorkspace>> {
        let conversation = self.core.conversation(id)?;
        let Some(Setup::Session {
            repo, environment, ..
        }) = &conversation.setup
        else {
            return Ok(None);
        };
        if self.recorded_workspace(id).is_none() {
            self.ensure_target(id, Path::new(repo), environment).await?;
        }
        self.recorded_workspace(id)
            .map(Some)
            .ok_or_else(|| Error::Invalid("the session's worktree could not be made".into()))
    }

    /// Where the thread's CLI runs: its workspace (#523), or its scratch folder while it has
    /// none yet.
    pub(crate) fn thread_cwd(&self, id: &ConversationId) -> PathBuf {
        self.recorded_workspace(id)
            .map_or_else(|| self.owned_dir("orch", &id.0), |workspace| workspace.path)
    }

    /// What the thread's CLI is started for now: its workspace (made if need be when `make`,
    /// else as planned) and level.
    pub(crate) async fn thread_launch(
        &self,
        id: &ConversationId,
        make: bool,
    ) -> Result<ThreadLaunch> {
        let workspace = if make {
            self.effective_workspace(id).await?
        } else {
            self.planned_workspace(id)
        };
        let permission = self.permission(id);
        Ok(ThreadLaunch {
            workspace,
            permission,
            access: permission.access(),
            hook_grant: None,
        })
    }

    /// The thread's CLI was started for another workspace or permission level than the
    /// session has now: close it while nothing runs, so the next turn resumes the same
    /// native session with the new ones (and a note saying so).
    pub(crate) async fn retire_moved_thread(&self, conv: &Arc<ConvLive>) {
        if conv.kind != ConversationKind::Session {
            return;
        }
        let Some(cli) = conv.idle_cli().await else {
            return;
        };
        let Some(launch) = cli.launch.as_ref() else {
            return;
        };
        // As planned: a merged session's next worktree is made by the user's next message,
        // at the same path, so it moves nothing.
        let now = match self.thread_launch(&conv.id, false).await {
            Ok(now) => now,
            Err(err) => {
                tracing::warn!(conversation = %conv.id, error = %err, "could not look at the thread's workspace");
                return;
            }
        };
        if launch.outdated(&now) {
            tracing::info!(conversation = %conv.id, workspace = ?now.workspace.as_ref().map(|w| &w.path), permission = ?now.permission, "the thread starts again for its new workspace or level");
            // A new level gives the thread another tool set (BUILD-PLAN B0).
            conv.retire_cli_for(&cli, Some(crate::routing::WriteCause::ToolSetChange))
                .await;
        }
    }

    /// Whether a thread on `provider` gets its output hook, which brings a subfolder's rules
    /// with the first file it touches there: a Claude thread always; a Codex thread when its
    /// login can follow it to a home of its own, where Codex runs the hook (#121).
    pub(crate) fn thread_hooked(&self, provider: ProviderKind) -> bool {
        match provider {
            ProviderKind::Claude => true,
            ProviderKind::Codex => {
                brigadier_providers::codex::runs_thread_hook(self.runtime.cli_env())
            }
        }
    }

    /// The thread's instructions: its role, its workspace, and the workspace's own
    /// instruction files (its CLI doesn't find them from its working directory).
    pub(crate) async fn thread_prompt(
        &self,
        conversation: &crate::model::Conversation,
        preferences: &[String],
        workspace: Option<&ThreadWorkspace>,
        provider: ProviderKind,
    ) -> String {
        let project = conversation
            .project_id
            .as_ref()
            .and_then(|id| self.core.project(id).ok());
        let mut prompt = super::prompts::thread(
            conversation,
            project.as_ref(),
            preferences,
            workspace.map(ThreadWorkspace::told).as_deref(),
            self.core.settings().start_workers_on_own,
        );
        prompt.push_str(&super::prompts::git_writing_section(&self.core.settings()));
        prompt.push_str(&super::prompts::browser_section(
            self.core.settings().browser_enabled,
        ));
        if let Some(workspace) = workspace {
            let hooked = self.thread_hooked(provider);
            prompt.push_str(&super::instructions::for_thread(&workspace.path, hooked).await);
        }
        // Claude's attribution settings hold its commits to it; Codex is told.
        if provider == ProviderKind::Codex && self.core.settings().omit_ai_coauthors {
            prompt.push_str("\n\n");
            prompt.push_str(super::prompts::NO_AI_COAUTHORS);
        }
        prompt
    }

    /// The thread's temporary folder (its session folder's `tmp/`), made and recorded under
    /// its own owner, which goes with the session.
    pub(crate) async fn thread_temp(&self, id: &ConversationId) -> Result<PathBuf> {
        let dir = self.session_dir(id).tmp();
        self.prepare_owned_dir(&thread_temp_owner(id), &dir).await?;
        Ok(dir)
    }

    /// The thread's environment: its temporary files in its short temporary folder `tmp`, as
    /// a worker's are in its own. Nothing else: the CLI's own shell timeouts, output limits and
    /// background tasks hold as the CLI ships them.
    pub(crate) fn thread_env(tmp: &Path) -> Vec<(String, String)> {
        super::workers::worker_env(tmp)
    }

    /// An approval request from the thread's CLI, answered as a worker's is: what the user
    /// already allowed "for this session" goes, the rest is the user's card.
    pub(crate) async fn route_thread_approval(
        &self,
        conv: &Arc<ConvLive>,
        cli: &Arc<Cli>,
        mut request: ApprovalRequest,
    ) {
        let workspace = cli
            .launch
            .as_ref()
            .and_then(|launch| launch.workspace.as_ref())
            .map(|workspace| workspace.path.as_path());
        if self.allowed_before(&conv.id, &mut request, workspace) {
            self.answer_thread(conv, cli, request, ApprovalDecision::Allow, Decider::User)
                .await;
        } else {
            self.ask_user_for_thread(conv, cli, request).await;
        }
    }

    /// The user's card for the thread CLI's request, while that CLI runs.
    async fn ask_user_for_thread(&self, conv: &ConvLive, cli: &Cli, request: ApprovalRequest) {
        if let Err(err) = self.open_cli_card(cli, &conv.id, None, request).await {
            tracing::warn!(conversation = %conv.id, error = %err, "could not open an approval card");
        }
    }

    /// Gives the thread's CLI Brigadier's answer to its request.
    async fn answer_thread(
        &self,
        conv: &Arc<ConvLive>,
        cli: &Arc<Cli>,
        request: ApprovalRequest,
        decision: ApprovalDecision,
        decider: Decider,
    ) {
        if let Err(err) = cli
            .session
            .answer(request.id.clone(), decision.clone())
            .await
        {
            tracing::warn!(conversation = %conv.id, error = %err, "could not answer an approval");
            return;
        }
        self.log_resolution(&conv.id, cli.provider, request.id, decision, decider)
            .await;
    }

    /// Passes the user's answer to the thread's CLI; "Allow … for this session" also allows
    /// similar requests in the conversation from now on.
    pub(crate) async fn answer_thread_approval(
        &self,
        conversation_id: &ConversationId,
        request: &ApprovalRequest,
        decision: ApprovalDecision,
    ) -> Result<()> {
        // Brigadier's own question: the call waiting on the card takes the answer.
        if request.id.starts_with(super::cards::OWN_APPROVAL) {
            return Ok(());
        }
        let cli = self
            .conv(conversation_id)?
            .live_cli()
            .await
            .ok_or_else(|| Error::Invalid("the session's thread has ended".into()))?;
        let answer = match &decision {
            ApprovalDecision::AllowSimilar => ApprovalDecision::Allow,
            other => other.clone(),
        };
        cli.session
            .answer(request.id.clone(), answer)
            .await
            .map_err(|err| Error::Provider(err.to_string()))?;
        if decision == ApprovalDecision::AllowSimilar {
            self.waiters.allow_similar(conversation_id, request);
        }
        self.log_resolution(
            conversation_id,
            cli.provider,
            request.id.clone(),
            decision,
            brigadier_providers::Decider::User,
        )
        .await;
        Ok(())
    }

    async fn log_resolution(
        &self,
        id: &ConversationId,
        provider: ProviderKind,
        approval_id: String,
        decision: ApprovalDecision,
        decided_by: brigadier_providers::Decider,
    ) {
        self.log_provider(
            id,
            provider,
            ProviderEvent::ApprovalResolved {
                id: approval_id,
                decision,
                decided_by,
            },
        )
        .await;
    }
}

impl SessionManager {
    /// Looks for the thread's new commits on its workspace's branch and records them: its
    /// self-edit metric reads them; nothing starts. `in_turn`: a turn runs
    /// (or just ended), so every new commit is the thread's; otherwise only a range with a
    /// commit marked as the thread's is (the user's own commits between turns are not).
    pub(crate) async fn scan_thread_commits(&self, id: &ConversationId, in_turn: bool) {
        let Some(workspace) = self.recorded_workspace(id) else {
            return;
        };
        let _scan = self.thread_scans.lock().await;
        self.scan_branch(id, &workspace.repo, &workspace.branch, None, in_turn)
            .await;
    }

    /// Looks at `branch` from its recorded tip to `upto` (its tip now by default), with
    /// [`Self::thread_scans`] held. The first look only records where it stands; a branch
    /// whose history was rewritten starts over from where it stands now.
    async fn scan_branch(
        &self,
        id: &ConversationId,
        repo: &Path,
        branch: &str,
        upto: Option<Oid>,
        in_turn: bool,
    ) {
        let seen = match self.core.board(id).await {
            Ok(board) => board.thread_tips.get(branch).cloned(),
            Err(_) => return,
        };
        let (key, value) = super::prompts::THREAD_TRAILER
            .split_once(": ")
            .unwrap_or_default();
        let (git, repo_path, name) = (self.git.clone(), repo.to_owned(), branch.to_owned());
        let found = blocking(move || {
            let repo = git.open(&repo_path).map_err(git_error)?;
            let tip = match upto {
                Some(tip) => tip,
                None => match repo.branch_tip(&name).map_err(git_error)? {
                    Some(tip) => tip,
                    None => return Ok(None),
                },
            };
            let Some(seen) = seen.map(Oid) else {
                return Ok(Some((tip, None)));
            };
            if seen == tip {
                return Ok(None);
            }
            if !repo.ancestor(&seen, &tip).unwrap_or(false) {
                return Ok(Some((tip, None)));
            }
            let theirs = in_turn
                || repo
                    .has_trailer(&seen, &tip, key, value)
                    .map_err(git_error)?;
            Ok(Some((tip, theirs.then_some(seen))))
        })
        .await;
        match found {
            Ok(Some((tip, base))) => {
                self.record_thread_tip(id, branch, &tip, base.as_ref())
                    .await;
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(conversation = %id, branch, error = %err, "could not look for the thread's commits");
            }
        }
    }

    /// Records `branch` looked at up to `tip`, and the commits from `authored_from` to it as
    /// the thread's own.
    async fn record_thread_tip(
        &self,
        id: &ConversationId,
        branch: &str,
        tip: &Oid,
        authored_from: Option<&Oid>,
    ) {
        if let Err(err) = self
            .core
            .record_conversation(
                id,
                vec![DomainEvent::ThreadCommitsSeen {
                    conversation_id: id.clone(),
                    branch: branch.to_owned(),
                    tip: tip.0.clone(),
                    authored_from: authored_from.map(|from| from.0.clone()),
                }],
            )
            .await
        {
            tracing::warn!(conversation = %id, error = %err, "could not record the thread's commits as seen");
        }
        // The branch moved: its self-edit metric is counted again, and kept past its merge.
        let (manager, id, again) = (self.arc(), id.clone(), authored_from.is_some());
        self.spawn(async move {
            manager.count_thread_edits_again(&id, again).await;
        });
    }
}
