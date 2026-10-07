//! The session's thread (THREAD-PLAN.md Q1, Q6): the CLI session that talks with the user,
//! reads, runs and makes tiny edits itself, and runs the workers.
//!
//! Its working directory stays its own scratch folder (`orch/<id>`), which keeps `--resume`
//! working. The code it works on is its *workspace*, given to the CLI as an extra folder: the
//! session's checkout, or an overnight run's worktree while the run is active. The workspace
//! is never recorded under the thread's cleanup-ledger owner, so ending the thread's processes
//! (hibernation, rebirth, fallback) never reaches what runs there.
//!
//! The thread's access follows the session's permission level exactly as a worker's does.
//! What its CLI was started with is remembered on it ([`ThreadLaunch`]): when the workspace or
//! the level changes, the CLI is closed between turns and the next turn resumes the same
//! native session with the new ones, told by a `[workspace]` or `[settings]` note.
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
use brigadier_providers::policy::Route as PolicyRoute;
use brigadier_providers::{Access, ApprovalDecision, ApprovalRequest, ProviderEvent, ProviderKind};

use super::conversation::{Cli, ConvLive};
use super::{SessionManager, blocking, git_error};
use crate::model::{
    ConversationId, ConversationKind, DomainEvent, Environment, PermissionLevel, Setup,
};
use crate::work::{ApprovalSubject, TaskKind};
use crate::{Error, Result};

/// The read-back window of a Claude thread's Bash output (the documented maximum): a failing
/// command's excerpt is cut from it, so a long failing log still ends with its last lines
/// (docs/evidence/2026-10-07-thread-phase2-contracts.md §3).
const BASH_MAX_OUTPUT_LENGTH: &str = "150000";

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
    /// The access it runs with, which its approval requests are judged against.
    pub access: Access,
}

impl ThreadLaunch {
    /// Whether a CLI started for `self` must start again for `now`: another folder, or
    /// another level (its sandbox and permission mode are fixed when it starts).
    pub(crate) fn outdated(&self, now: &ThreadLaunch) -> bool {
        self.permission != now.permission
            || self.workspace.as_ref().map(|w| &w.path) != now.workspace.as_ref().map(|w| &w.path)
    }
}

impl SessionManager {
    /// The thread's workspace as recorded now, without making anything: an active overnight
    /// run's worktree, else the session's checkout (`None` for a new-worktree session whose
    /// worktree isn't made yet, and for a Chat).
    pub(crate) fn recorded_workspace(&self, id: &ConversationId) -> Option<ThreadWorkspace> {
        let conversation = self.core.conversation(id).ok()?;
        let Some(Setup::Session {
            repo, environment, ..
        }) = &conversation.setup
        else {
            return None;
        };
        let repo = PathBuf::from(repo);
        if let Some(run) = self
            .overnight
            .active
            .get(id)
            .and_then(|active| active.workspace)
        {
            return Some(ThreadWorkspace {
                path: PathBuf::from(run.path),
                branch: run.branch,
                repo,
            });
        }
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

    /// The thread's effective workspace (THREAD-PLAN.md Q1, Q10): the overnight run's worktree
    /// while a run is active, else the session's checkout, its own worktree made first when
    /// it has none yet. `None` for a Chat.
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

    /// The thread's access at `permission`, mapped as a worker's is: Full access runs without
    /// a sandbox; otherwise it writes its scratch folder, the workspace and the repository's
    /// git folder (it commits its tiny edits), with the network unless the user is asked for
    /// everything. Under Approve for me the CLI's own reviewer settles what leaves the
    /// sandbox (`true`); under Ask for approval it reaches the user as a card.
    pub(crate) fn thread_access(
        &self,
        workspace: Option<&ThreadWorkspace>,
        scratch: &Path,
        provider: ProviderKind,
        permission: PermissionLevel,
    ) -> (Access, bool) {
        let level = super::workers::access_for(TaskKind::Implement, permission);
        if level.unsandboxed {
            return (Access::Full, false);
        }
        let mut writable_roots = vec![scratch.to_owned()];
        if let Some(workspace) = workspace {
            writable_roots.push(workspace.path.clone());
            if let Ok(repo) = self.git.open(&workspace.path) {
                for root in super::workers::commit_roots(&repo, provider) {
                    if !writable_roots.contains(&root) {
                        writable_roots.push(root);
                    }
                }
            }
        }
        (
            self.sandboxed(true, writable_roots, level.network),
            permission == PermissionLevel::ApproveForMe,
        )
    }

    /// What the thread's CLI is started for now: its workspace (made if need be), level and
    /// access.
    pub(crate) async fn thread_launch(
        &self,
        id: &ConversationId,
        scratch: &Path,
        provider: ProviderKind,
    ) -> Result<(ThreadLaunch, bool)> {
        let workspace = self.effective_workspace(id).await?;
        let permission = self.permission(id);
        let (access, auto_review) =
            self.thread_access(workspace.as_ref(), scratch, provider, permission);
        Ok((
            ThreadLaunch {
                workspace,
                permission,
                access,
            },
            auto_review,
        ))
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
        let scratch = self.owned_dir("orch", &conv.id.0);
        let now = match self.thread_launch(&conv.id, &scratch, cli.provider).await {
            Ok((now, _)) => now,
            Err(err) => {
                tracing::warn!(conversation = %conv.id, error = %err, "could not look at the thread's workspace");
                return;
            }
        };
        if launch.outdated(&now) {
            tracing::info!(conversation = %conv.id, workspace = ?now.workspace.as_ref().map(|w| &w.path), permission = ?now.permission, "the thread starts again for its new workspace or level");
            conv.retire_cli(&cli).await;
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
        let run = self
            .overnight
            .active
            .get(&conversation.id)
            .and_then(|active| active.workspace);
        let mut prompt = super::prompts::thread(
            conversation,
            project.as_ref(),
            preferences,
            run.as_ref(),
            workspace.map(ThreadWorkspace::told).as_deref(),
            provider,
            self.core.settings().short_replies,
        );
        if let Some(workspace) = workspace {
            prompt.push_str(&super::instructions::for_thread(&workspace.path).await);
        }
        // Claude's attribution settings hold its commits to it; Codex is told.
        if provider == ProviderKind::Codex && self.core.settings().omit_ai_coauthors {
            prompt.push_str("\n\n");
            prompt.push_str(super::prompts::NO_AI_COAUTHORS);
        }
        prompt
    }

    /// The thread's environment: its temporary files in its scratch folder, as a worker's
    /// are; a Claude thread reads back up to [`BASH_MAX_OUTPUT_LENGTH`] of a command's output.
    pub(crate) fn thread_env(scratch: &Path, provider: ProviderKind) -> Vec<(String, String)> {
        let mut env = super::workers::worker_env(scratch);
        if provider == ProviderKind::Claude {
            // `BASH_MAX_OUTPUT_LENGTH` (code.claude.com/docs/en/env-vars).
            env.push((
                "BASH_MAX_OUTPUT_LENGTH".into(),
                BASH_MAX_OUTPUT_LENGTH.into(),
            ));
        }
        env
    }

    /// An approval request from the thread's CLI, answered as a worker's is: what stays
    /// inside its access goes, the rest is the user's card.
    pub(crate) async fn route_thread_approval(
        &self,
        conv: &Arc<ConvLive>,
        cli: &Arc<Cli>,
        request: ApprovalRequest,
    ) {
        let access = cli
            .launch
            .as_ref()
            .map_or(Access::ReadOnly, |launch| launch.access.clone());
        let (route, decider) = self.approval_route(&conv.id, &request, &access);
        let decision = match route {
            PolicyRoute::Allow => ApprovalDecision::Allow,
            PolicyRoute::Deny => ApprovalDecision::Deny {
                message: "This session may not do that.".into(),
            },
            PolicyRoute::AskUser => {
                if let Err(err) = self
                    .open_approval(&conv.id, None, ApprovalSubject::Cli { request })
                    .await
                {
                    tracing::warn!(conversation = %conv.id, error = %err, "could not open an approval card");
                }
                return;
            }
        };
        self.pass_unsandboxed(&conv.id, &request, &decision);
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

    /// Passes the user's answer to the thread's CLI; "Allow similar commands" also allows
    /// similar requests in the conversation from now on.
    pub(crate) async fn answer_thread_approval(
        &self,
        conversation_id: &ConversationId,
        request: &ApprovalRequest,
        decision: ApprovalDecision,
    ) -> Result<()> {
        let cli = self
            .conv(conversation_id)?
            .live_cli()
            .await
            .ok_or_else(|| Error::Invalid("the session's thread has ended".into()))?;
        let answer = match &decision {
            ApprovalDecision::AllowSimilar => ApprovalDecision::Allow,
            other => other.clone(),
        };
        self.pass_unsandboxed(conversation_id, request, &answer);
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

    /// An approved `run_unsandboxed` call may run its command, once (see `super::run`).
    fn pass_unsandboxed(
        &self,
        id: &ConversationId,
        request: &ApprovalRequest,
        decision: &ApprovalDecision,
    ) {
        if request.tool == super::run::RUN_UNSANDBOXED
            && !matches!(decision, ApprovalDecision::Deny { .. })
            && let Some(command) = &request.command
        {
            self.run_passes.grant(id, command);
        }
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
    /// Whether the conversation's thread runs a turn now.
    pub(crate) async fn thread_turn_running(&self, id: &ConversationId) -> bool {
        match self.conv(id) {
            Ok(conv) => conv.turn_running().await,
            Err(_) => false,
        }
    }

    /// Looks for the thread's new commits on its workspace's branch, and starts their review.
    /// `in_turn`: a turn runs (or just ended), so every new commit is the thread's; otherwise
    /// only a range with a commit marked as the thread's is (the user's own commits between
    /// turns are not).
    pub(crate) async fn scan_thread_commits(&self, id: &ConversationId, in_turn: bool) {
        let Some(workspace) = self.recorded_workspace(id) else {
            return;
        };
        let _scan = self.thread_scans.lock().await;
        self.scan_branch(id, &workspace.repo, &workspace.branch, None, in_turn)
            .await;
    }

    /// The same for `branch` of `repo` (an overnight run's, before its merge).
    pub(crate) async fn scan_thread_branch(
        &self,
        id: &ConversationId,
        repo: &Path,
        branch: &str,
        in_turn: bool,
    ) {
        let _scan = self.thread_scans.lock().await;
        self.scan_branch(id, repo, branch, None, in_turn).await;
    }

    /// A landing moved `branch` from `from` to `tip`. What the thread committed before it, up
    /// to `from`, is looked at first (a turn that commits and then lands a worker); then the
    /// record moves past the landing, which has its own review.
    pub(crate) async fn thread_branch_landed(
        &self,
        id: &ConversationId,
        repo: &Path,
        branch: &str,
        from: &Oid,
        tip: &Oid,
    ) {
        let _scan = self.thread_scans.lock().await;
        let in_turn = self.thread_turn_running(id).await;
        self.scan_branch(id, repo, branch, Some(from.clone()), in_turn)
            .await;
        self.record_thread_tip(id, branch, tip).await;
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
                if let Some(base) = base {
                    self.review_thread_commits(id, base, tip.clone(), repo.to_owned())
                        .await;
                }
                self.record_thread_tip(id, branch, &tip).await;
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(conversation = %id, branch, error = %err, "could not look for the thread's commits");
            }
        }
    }

    async fn record_thread_tip(&self, id: &ConversationId, branch: &str, tip: &Oid) {
        if let Err(err) = self
            .core
            .record_conversation(
                id,
                vec![DomainEvent::ThreadCommitsSeen {
                    conversation_id: id.clone(),
                    branch: branch.to_owned(),
                    tip: tip.0.clone(),
                }],
            )
            .await
        {
            tracing::warn!(conversation = %id, error = %err, "could not record the thread's commits as seen");
        }
    }
}
