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
use brigadier_providers::{
    Access, ApprovalDecision, ApprovalRequest, Decider, ProviderEvent, ProviderKind,
};

use super::conversation::{Cli, ConvLive};
use super::escalation::Settled;
use super::workers::{SessionTemp, session_temp_dir, thread_temp_owner};
use super::{SessionManager, blocking, git_error};
use crate::model::{
    ConversationId, ConversationKind, DomainEvent, Environment, PermissionLevel, Setup,
};
use crate::work::TaskKind;
use crate::{Error, Result};

/// The read-back window of a Claude thread's Bash output (the documented maximum): a failing
/// command's excerpt is cut from it, so a long failing log still ends with its last lines
/// (docs/evidence/2026-10-07-thread-phase2-contracts.md §3).
const BASH_MAX_OUTPUT_LENGTH: &str = "150000";

/// The longest a Claude thread's own shell command may run (2 minutes): longer ones go through
/// `run` or `run_check`, which go to the background rather than hold the thread.
const BASH_TIMEOUT_MS: &str = "120000";

/// `run_unsandboxed` as a Claude thread's CLI names it in its permission prompt.
const CLAUDE_RUN_UNSANDBOXED: &str = "mcp__brigadier__run_unsandboxed";

/// A Claude thread's `run_unsandboxed` leaves the sandbox, though its CLI asks about it as an
/// ordinary tool call (Ask for approval's `ask` rule): only the user answers it.
fn mark_unsandboxed(request: &mut ApprovalRequest) {
    if request.tool == CLAUDE_RUN_UNSANDBOXED {
        request.escalation = true;
    }
}

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
    /// What it may never write ([`SessionManager::thread_protection`]).
    pub protection: brigadier_sandbox::Protection,
}

impl ThreadLaunch {
    /// Whether a CLI started for `self` must start again for `now`: another folder, or
    /// another level (its sandbox and permission mode are fixed when it starts).
    pub(crate) fn outdated(&self, now: &ThreadLaunch) -> bool {
        self.permission != now.permission
            || self.workspace.as_ref().map(|w| &w.path) != now.workspace.as_ref().map(|w| &w.path)
    }
}

/// Whether Brigadier's own reviewer settles a thread CLI's requests to go beyond its sandbox:
/// a Codex thread at Approve for me in a sandbox (`approvalsReviewer: user`, see
/// [`super::escalation`]). A Claude thread's run in its CLI's auto mode instead.
fn brigadier_reviews(provider: ProviderKind, launch: Option<&ThreadLaunch>) -> bool {
    provider == ProviderKind::Codex
        && launch.is_some_and(|launch| {
            launch.permission == PermissionLevel::ApproveForMe
                && !matches!(launch.access, Access::Full | Access::ReadOnly)
        })
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
        id: &ConversationId,
        workspace: Option<&ThreadWorkspace>,
        scratch: &Path,
        permission: PermissionLevel,
    ) -> (Access, bool) {
        let level = super::workers::access_for(TaskKind::Implement, permission);
        if level.unsandboxed {
            return (Access::Full, false);
        }
        let mut writable_roots = vec![
            scratch.to_owned(),
            session_temp_dir(id, SessionTemp::Thread),
        ];
        if let Some(workspace) = workspace {
            writable_roots.push(workspace.path.clone());
            if let Ok(repo) = self.git.open(&workspace.path) {
                for root in super::workers::commit_roots(&repo, Some(&workspace.branch), true) {
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

    /// What the thread's CLI is started for now: its workspace (made if need be when `make`,
    /// else as planned), level and access.
    pub(crate) async fn thread_launch(
        &self,
        id: &ConversationId,
        scratch: &Path,
        make: bool,
    ) -> Result<(ThreadLaunch, bool)> {
        let workspace = if make {
            self.effective_workspace(id).await?
        } else {
            self.planned_workspace(id)
        };
        let permission = self.permission(id);
        let (access, auto_review) = self.thread_access(id, workspace.as_ref(), scratch, permission);
        let protection =
            self.thread_protection(id, workspace.as_ref(), scratch, access == Access::Full);
        Ok((
            ThreadLaunch {
                workspace,
                permission,
                access,
                protection,
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
        // As planned: a merged session's next worktree is made by the user's next message,
        // at the same path, so it moves nothing.
        let now = match self.thread_launch(&conv.id, &scratch, false).await {
            Ok((now, _)) => now,
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
            self.core.settings().start_workers_on_own,
        );
        // Without computer use nothing can look at an app live, so nothing is routed there.
        if !self.core.settings().computer_use {
            prompt = prompt.replacen(super::prompts::COMPUTER_ROUTING, "", 1);
        }
        if !self.reviews_on() {
            prompt.push_str(&super::prompts::reviews_off_section());
        }
        prompt.push_str(&super::prompts::git_writing_section(&self.core.settings()));
        prompt.push_str(&super::prompts::browser_section(
            self.core.settings().browser_enabled,
        ));
        prompt.push_str(&super::prompts::auto_merge_section(
            self.auto_merge_in(&conversation.id),
        ));
        if let Some(workspace) = workspace {
            prompt.push_str(&super::instructions::for_thread(&workspace.path, provider).await);
        }
        // Claude's attribution settings hold its commits to it; Codex is told.
        if provider == ProviderKind::Codex && self.core.settings().omit_ai_coauthors {
            prompt.push_str("\n\n");
            prompt.push_str(super::prompts::NO_AI_COAUTHORS);
        }
        prompt
    }

    /// The thread's short temporary folder ([`session_temp_dir`]), made and recorded under
    /// its own owner, which goes with the session.
    pub(crate) async fn thread_temp(&self, id: &ConversationId) -> Result<PathBuf> {
        let dir = session_temp_dir(id, SessionTemp::Thread);
        self.prepare_owned_dir(&thread_temp_owner(id), &dir).await?;
        Ok(dir)
    }

    /// The thread's environment: its temporary files in its short temporary folder `tmp`, as
    /// a worker's are in its own. A Claude thread reads back up to [`BASH_MAX_OUTPUT_LENGTH`]
    /// of a command's output, has no background jobs of its own, and its shell holds it at
    /// most [`BASH_TIMEOUT_MS`]: a job of the CLI's dies with it, unseen (a closed or replaced
    /// CLI, a restart), and a long command holds the thread from the user. Its long commands
    /// go through `run` and `run_check`, which go to the background on their own
    /// ([`super::background_runs`]).
    pub(crate) fn thread_env(tmp: &Path, provider: ProviderKind) -> Vec<(String, String)> {
        let mut env = super::workers::worker_env(tmp);
        if provider == ProviderKind::Claude {
            // code.claude.com/docs/en/env-vars. The ceiling holds whatever timeout a call asks
            // for, and a command past it is stopped, not moved to the background (checked on
            // Claude Code 2.1.299).
            for (name, value) in [
                ("BASH_MAX_OUTPUT_LENGTH", BASH_MAX_OUTPUT_LENGTH),
                ("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS", "1"),
                ("BASH_DEFAULT_TIMEOUT_MS", BASH_TIMEOUT_MS),
                ("BASH_MAX_TIMEOUT_MS", BASH_TIMEOUT_MS),
            ] {
                env.push((name.into(), value.into()));
            }
        }
        env
    }

    /// An approval request from the thread's CLI, answered as a worker's is: what stays
    /// inside its access goes, the rest is the user's card.
    pub(crate) async fn route_thread_approval(
        &self,
        conv: &Arc<ConvLive>,
        cli: &Arc<Cli>,
        mut request: ApprovalRequest,
    ) {
        mark_unsandboxed(&mut request);
        let access = cli
            .launch
            .as_ref()
            .map_or(Access::ReadOnly, |launch| launch.access.clone());
        let workspace = cli
            .launch
            .as_ref()
            .and_then(|launch| launch.workspace.as_ref())
            .map(|workspace| workspace.path.as_path());
        let reviewed = brigadier_reviews(cli.provider, cli.launch.as_ref());
        let (route, decider) =
            self.approval_route(&conv.id, &mut request, &access, workspace, reviewed);
        let decision = match route {
            PolicyRoute::Allow => ApprovalDecision::Allow,
            PolicyRoute::Deny => ApprovalDecision::Deny {
                message: "This session may not do that.".into(),
            },
            // A Codex thread at Approve for me: Brigadier's reviewer settles it
            // (`super::escalation`), while the CLI waits.
            PolicyRoute::AskUser if reviewed => {
                let manager = self.arc();
                let (conv, cli) = (conv.clone(), cli.clone());
                self.spawn(async move {
                    let Some(launch) = cli.launch.as_ref() else {
                        return;
                    };
                    let workspace = launch.workspace.as_ref().map(|w| w.path.as_path());
                    let unattended = manager.overnight.active.get(&conv.id).is_some();
                    match manager
                        .settle_codex_escalation(
                            &conv.id,
                            &request,
                            workspace,
                            &launch.protection,
                            unattended,
                        )
                        .await
                    {
                        Settled::Answer(decision) => {
                            manager
                                .answer_thread(&conv, &cli, request, decision, Decider::Policy)
                                .await;
                        }
                        Settled::Ask(why) => {
                            request.reason = Some(why);
                            manager.ask_user_for_thread(&conv, &cli, request).await;
                        }
                    }
                });
                return;
            }
            PolicyRoute::AskUser => {
                self.ask_user_for_thread(conv, cli, request).await;
                return;
            }
        };
        self.answer_thread(conv, cli, request, decision, decider)
            .await;
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

    /// Passes the user's answer to the thread's CLI; "Allow … for this session" also allows
    /// similar requests in the conversation from now on.
    pub(crate) async fn answer_thread_approval(
        &self,
        conversation_id: &ConversationId,
        request: &ApprovalRequest,
        decision: ApprovalDecision,
    ) -> Result<()> {
        // Brigadier's own question: the `run_unsandboxed` call waiting on the card takes the
        // answer (`super::escalation`).
        if request.id.starts_with(super::escalation::OWN_APPROVAL) {
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
    /// Codex names the tool and gives the call's `workdir` as the request's folder; Claude
    /// names it as an MCP tool, and the `workdir` is in its input.
    fn pass_unsandboxed(
        &self,
        id: &ConversationId,
        request: &ApprovalRequest,
        decision: &ApprovalDecision,
    ) {
        if matches!(decision, ApprovalDecision::Deny { .. }) {
            return;
        }
        let Some(command) = &request.command else {
            return;
        };
        if request.tool == super::run::RUN_UNSANDBOXED {
            self.run_passes.grant(id, command, request.cwd.as_deref());
        } else if request.tool == CLAUDE_RUN_UNSANDBOXED {
            let input: Option<serde_json::Value> = request
                .input
                .as_deref()
                .and_then(|input| serde_json::from_str(input).ok());
            let workdir = input
                .as_ref()
                .and_then(|input| input.get("workdir"))
                .and_then(serde_json::Value::as_str);
            self.run_passes.grant(id, command, workdir);
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
        self.record_thread_tip(id, branch, tip, None).await;
        // The landing has its own review: the thread's range goes on after it.
        crate::heap(|| self.unreviewed_moved(id, branch, None, tip)).await;
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
        // A local checkout's changes are measured from snapshots of its files, its commits
        // included ([`SessionManager::local_changes`]).
        let local = self
            .local_checkout(id)
            .is_some_and(|(_, local)| local == branch);
        match found {
            Ok(Some((tip, base))) => {
                let theirs = base.is_some();
                if let Some(base) = &base {
                    let base = base.clone();
                    // Commits that only fix review findings get no review of their own, and a
                    // held merge's consent covers them (SESSION-FIXES-PLAN.md Q3.4, Q6.1).
                    match self.thread_fixes(id, repo, &base, &tip).await {
                        Some(fix) => {
                            if fix.since_merge {
                                self.move_held_merge(id, branch, &base, &tip).await;
                            }
                            self.recheck_thread_fix(
                                id,
                                base.clone(),
                                tip.clone(),
                                repo.to_owned(),
                                &fix.fixed,
                            )
                            .await;
                            if !local {
                                crate::heap(|| {
                                    self.unreviewed_moved(id, branch, Some(&base), &tip)
                                })
                                .await;
                            }
                        }
                        // Measured with the request's other unreviewed changes: one review
                        // once they are big enough or marked risky (BUILD-PLAN B2).
                        None if !local => {
                            crate::heap(|| {
                                self.thread_commits_unreviewed(id, repo, branch, &base, &tip)
                            })
                            .await;
                        }
                        None => {}
                    }
                } else if !local {
                    crate::heap(|| self.unreviewed_moved(id, branch, None, &tip)).await;
                }
                self.record_thread_tip(id, branch, &tip, base.as_ref())
                    .await;
                // The thread's own commits make the session's full checks due, as a landing
                // does (SESSION-FIXES-PLAN.md §11).
                if theirs {
                    let request = self.request_for(id, None).await;
                    self.checks_due(id, repo, branch, &tip, request).await;
                }
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

#[cfg(test)]
mod tests {
    use brigadier_providers::model::ApprovalKind;
    use brigadier_providers::policy::{self, ApprovalMode};

    use super::*;

    #[test]
    fn a_claude_thread_s_run_unsandboxed_at_ask_for_approval_goes_to_the_user() {
        let ask = Access::Scoped {
            write_cwd: true,
            writable_roots: Vec::new(),
            network: false,
            deny_read: Vec::new(),
            unix_sockets: Vec::new(),
        };
        let mut request = ApprovalRequest {
            id: "1".into(),
            kind: ApprovalKind::Tool,
            tool: CLAUDE_RUN_UNSANDBOXED.into(),
            command: Some("curl -sS https://example.com".into()),
            cwd: None,
            paths: Vec::new(),
            reason: None,
            escalation: false,
            input: None,
            grant: None,
        };
        // As the CLI asks it, any tool call inside the sandbox's access would be allowed.
        assert_eq!(
            policy::route(&request, &ask, ApprovalMode::Delegated),
            PolicyRoute::Allow
        );
        mark_unsandboxed(&mut request);
        assert_eq!(
            policy::route(&request, &ask, ApprovalMode::Delegated),
            PolicyRoute::AskUser
        );
    }
}
