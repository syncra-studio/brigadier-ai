//! A landed worker stays resumable until its review's findings settle (SESSION-FIXES-PLAN.md
//! Q6.1). At landing its CLI session files move to the ledger owner `review-hold:<task>` and its
//! branch is kept, though merged; its worktree and scratch folder go as always. The findings the
//! thread records as open go back as one fix task (`role` fix, `subject` the landed task), which
//! resumes that same session, model and account in a worktree made again at the old path, on
//! the old branch moved up to the session's tip. The batch's review part settles when its review
//! is clean or couldn't run, when the thread records none of its findings open, or when the fix
//! task ends. The session's full checks hold it too (SESSION-FIXES-PLAN.md §11): a failure is
//! sent back to the worker that broke it the same way, so the hold lasts until a passing check
//! covers its landing as well (a fix's own landing is held for that alone). Merging, archiving
//! or deleting the session ends every hold. Then the files go, and the branch with them while
//! it is still merged.

use std::path::{Path, PathBuf};

use brigadier_git::{Oid, WorktreeSpec};
use brigadier_providers::Artifact;

use super::workers::Workspace;
use super::{SessionManager, blocking, git_error, secrets};
use crate::model::{ConversationId, Setup, TaskId};
use crate::work::{FindingStatus, ReviewFor, ReviewKind, ReviewState, Task, TaskKind, WorkerRole};
use crate::{Error, Result};

/// The ledger owner of a landed task's CLI session files while its review's findings settle.
pub(crate) fn hold_owner(task: &TaskId) -> String {
    format!("review-hold:{task}")
}

/// What a resume needs of a landed worker's session: its transcript and per-session state.
fn session_file(artifact: &Artifact) -> bool {
    matches!(
        artifact,
        Artifact::ClaudeSession { .. }
            | Artifact::ClaudeProjectDir { .. }
            | Artifact::CodexThread { .. }
            | Artifact::CodexGeneratedImages { .. }
            | Artifact::CodexProjectTrust { .. }
    )
}

/// Whether `fix` is the fix of `subject`'s review findings that resumes `subject`'s held session.
pub(crate) fn fixes_held(fix: &Task, subject: &Task) -> bool {
    fix.role == Some(WorkerRole::Fix)
        && fix.kind == TaskKind::Implement
        && fix.subject.as_ref() == Some(&subject.id)
        && subject.landed.is_some()
        && subject.review_hold.is_some()
}

/// The held tasks `which` picks, but for one a fix still resumes.
pub(crate) fn batch(board: &crate::board::Board, which: impl Fn(&Task) -> bool) -> Vec<Task> {
    board
        .tasks
        .values()
        .filter(|task| task.review_hold.is_some() && which(task))
        .filter(|task| {
            !board
                .tasks
                .values()
                .any(|fix| !fix.state.is_final() && fixes_held(fix, task))
        })
        .cloned()
        .collect()
}

/// Why a fix of landed work's findings can't ask for a review: it gets none of its own, only
/// the targeted re-check of a finding the thread marked high, after it lands (Q6.1).
pub(crate) const NO_FIX_REVIEW: &str = "A fix of landed work's review findings gets no review of its own: once it lands, the orchestrator has each finding it marked high re-checked. Don't call review_code; run the checks your fix affects and call submit_report.";

/// The line a resumed fix worker reads before its findings.
pub(crate) fn workspace_line(
    number: u32,
    worktree: &str,
    branch: &str,
    tip: &Oid,
    scratch: &Path,
    test_dir: &Path,
) -> String {
    format!(
        "[workspace] Your work landed. Your earlier task is done; this is a new one, task-{number}. Your worktree was made again at {worktree}, on your branch `{branch}` moved up to the session's tip {} (your landed work and what landed after it). Your earlier task's folders are gone: your scratch folder is now {}, your outputs folder {}, and your test data folder {}. {NO_FIX_REVIEW}",
        &tip.0[..tip.0.len().min(10)],
        scratch.display(),
        super::outputs::outputs_dir(scratch).display(),
        test_dir.display(),
    )
}

impl SessionManager {
    /// At landing: keeps `task`'s CLI session files and its branch for the findings of the
    /// review that starts now and of the session's full checks (`review_settled`: no review's
    /// findings come back for it, only the checks'). False when it has no session files to keep.
    pub(crate) async fn hold_for_review(
        &self,
        task: &Task,
        target: &str,
        review_settled: bool,
    ) -> bool {
        if task.kind != TaskKind::Implement {
            return false;
        }
        let ledger = self.runtime.ledger();
        let (owner, hold) = (format!("task:{}", task.id), hold_owner(&task.id));
        let files: Vec<Artifact> = ledger
            .artifacts(&owner)
            .into_iter()
            .filter(session_file)
            .collect();
        if files.is_empty() {
            return false;
        }
        // Recorded under the hold before the task forgets them: never unowned in between.
        for artifact in files {
            if let Err(err) = ledger.record(&hold, artifact.clone()).await {
                tracing::warn!(task = %task.id, error = %err, "could not keep a landed worker's session for its review");
                continue;
            }
            if let Err(err) = ledger.unrecord(&owner, artifact).await {
                tracing::warn!(task = %task.id, error = %err, "a landed worker's session file stays with its task too");
            }
        }
        let target = target.to_owned();
        self.update_task(&task.conversation_id, &task.id, |t| {
            t.review_hold = Some(target);
            t.review_settled = review_settled;
        })
        .await
        .is_ok()
    }

    /// The review part of the batch of the landing at `tip` settles: each task it holds lets
    /// go of its session and branch once a passing full check covers its landing too, except
    /// one a fix still resumes (that one settles when its fix ends).
    pub(crate) async fn settle_review_batch(&self, conversation_id: &ConversationId, tip: &str) {
        let Ok(board) = self.core.board(conversation_id).await else {
            return;
        };
        // A landing reviewed in parts (its worker's own review, then the commits after it)
        // settles once each part has: none still running, none whose findings the thread
        // hasn't recorded yet, and none it kept open unless their fix has ended.
        let parts: Vec<_> = board
            .reviews
            .values()
            .filter(|review| {
                review.kind == ReviewKind::Code && super::review_runs::batch_tip(review) == tip
            })
            .collect();
        let findings = |open: fn(&FindingStatus) -> bool| {
            parts.iter().any(|review| {
                review.notify == ReviewFor::Orchestrator
                    && review.items.iter().any(|item| open(&item.status))
            })
        };
        let pending = parts
            .iter()
            .any(|review| review.state == ReviewState::Running)
            || findings(|status| *status == FindingStatus::New);
        if pending {
            return;
        }
        let kept_open = findings(|status| matches!(status, FindingStatus::Open { .. }));
        for task in batch(&board, |task| task.landed.as_deref() == Some(tip)) {
            // Another part's clean end doesn't settle findings kept open for a fix to come.
            if kept_open
                && !board
                    .tasks
                    .values()
                    .any(|fix| fix.state.is_final() && fixes_held(fix, &task))
            {
                continue;
            }
            if self.landing_checked(&task).await {
                self.release_hold(&task).await;
            } else if !task.review_settled {
                let _ = self
                    .update_task(&task.conversation_id, &task.id, |t| t.review_settled = true)
                    .await;
            }
        }
    }

    /// `fix` ended (landed, failed or stopped): the batch of the work it fixed settles.
    pub(crate) async fn settle_fixed(&self, fix: &Task) {
        let Some(subject) = &fix.subject else {
            return;
        };
        let Ok(subject) = self.task_by_id(&fix.conversation_id, subject).await else {
            return;
        };
        if fixes_held(fix, &subject)
            && let Some(tip) = &subject.landed
        {
            self.settle_review_batch(&fix.conversation_id, tip).await;
        }
    }

    /// The session is merged, archived or deleted: every batch it still holds lets go, its
    /// checks or not.
    pub(crate) async fn settle_all_holds(&self, conversation_id: &ConversationId) {
        let Ok(board) = self.core.board(conversation_id).await else {
            return;
        };
        for task in batch(&board, |_| true) {
            self.release_hold(&task).await;
        }
    }

    /// Removes what `task`'s hold kept: its session files, and its branch while that is still
    /// merged into where it landed (a fix that stopped with work on it keeps it).
    pub(crate) async fn release_hold(&self, task: &Task) {
        let leftovers = self.runtime.ledger().dispose(&hold_owner(&task.id)).await;
        if !leftovers.is_clean() {
            tracing::warn!(task = %task.id, ?leftovers, "some of a landed worker's session files will be retried at the next launch");
        }
        if let (Some(branch), Some(target), Ok(repo)) = (
            task.workspace.as_ref().and_then(|w| w.branch.clone()),
            task.review_hold.clone(),
            self.task_repo(task),
        ) {
            let git = self.git.clone();
            let records = repo.clone();
            let merged = {
                let (branch, repo) = (branch.clone(), repo.clone());
                blocking(move || {
                    let repo = git.open(&repo).map_err(git_error)?;
                    let (Some(tip), Some(into)) = (
                        repo.branch_tip(&branch).map_err(git_error)?,
                        repo.branch_tip(&target).map_err(git_error)?,
                    ) else {
                        return Ok(None);
                    };
                    Ok(repo
                        .ancestor(&tip, &into)
                        .map_err(git_error)?
                        .then_some(tip))
                })
                .await
            };
            match merged {
                Ok(Some(tip)) => {
                    self.forget_branch(&records, &branch);
                    let git = self.git.clone();
                    let deleted = blocking(move || {
                        git.open(&repo)
                            .map_err(git_error)?
                            .delete_branch_at(&branch, &tip)
                            .map_err(git_error)
                    })
                    .await;
                    if let Err(err) = deleted {
                        tracing::warn!(task = %task.id, error = %err, "could not delete a landed task's branch");
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!(task = %task.id, error = %err, "could not check a landed task's branch");
                }
            }
        }
        let _ = self
            .update_task(&task.conversation_id, &task.id, |t| t.review_hold = None)
            .await;
    }

    /// The native session a fix of `subject`'s findings resumes: the one its hold keeps.
    pub(crate) async fn held_session(&self, subject: &Task) -> Option<String> {
        let native_id = self.last_worker_native_id(subject).await?;
        let kept = self
            .runtime
            .ledger()
            .artifacts(&hold_owner(&subject.id))
            .iter()
            .any(|artifact| match artifact {
                Artifact::ClaudeSession { session_id, .. } => *session_id == native_id,
                Artifact::CodexThread { thread_id, .. } => *thread_id == native_id,
                _ => false,
            });
        kept.then_some(native_id)
    }

    /// A fix of `subject`'s findings works where `subject` worked: its worktree made again at
    /// the old path (the session's transcript names it), on its branch moved up to the tip of
    /// where it landed. Its scratch folder is new.
    pub(crate) async fn prepare_fix_workspace(
        &self,
        owner: &str,
        task: &Task,
        subject: &Task,
    ) -> Result<Workspace> {
        let conversation = self.core.conversation(&task.conversation_id)?;
        let Some(Setup::Session { repo, .. }) = &conversation.setup else {
            return Err(Error::Invalid("tasks belong to a session".into()));
        };
        let repo = PathBuf::from(repo);
        let recorded = subject.workspace.as_ref();
        let (Some(path), Some(branch), Some(target)) = (
            recorded.and_then(|w| w.worktree.clone()),
            recorded.and_then(|w| w.branch.clone()),
            subject.review_hold.clone(),
        ) else {
            return Err(Error::Invalid(format!(
                "task-{} has no worktree and branch to work in again",
                subject.number
            )));
        };
        let path = PathBuf::from(path);
        if path.exists() {
            return Err(Error::Invalid(format!(
                "task-{}'s old worktree {} is still there",
                subject.number,
                path.display()
            )));
        }
        let scratch = self.owned_dir("scratch", &task.id.0);
        self.prepare_owned_dir(owner, &scratch).await?;
        let (git, at, name, into) = (
            self.git.clone(),
            repo.clone(),
            branch.clone(),
            target.clone(),
        );
        let tip = blocking(move || {
            let repo = git.open(&at).map_err(git_error)?;
            let into = repo
                .branch_tip(&into)
                .map_err(git_error)?
                .ok_or_else(|| Error::Invalid(format!("branch {into} does not exist")))?;
            match repo.branch_tip(&name).map_err(git_error)? {
                None => repo.create_branch(&name, &into).map_err(git_error)?,
                Some(tip) if tip == into => {}
                Some(tip) if repo.ancestor(&tip, &into).map_err(git_error)? => {
                    repo.reset_branch(&name, &into, &tip).map_err(git_error)?
                }
                Some(_) => {
                    return Err(Error::Invalid(format!(
                        "branch {name} has commits that never landed"
                    )));
                }
            }
            Ok(into)
        })
        .await?;
        let warmed = self
            .make_worktree(
                owner,
                &repo,
                &path,
                WorktreeSpec::Branch {
                    name: branch.clone(),
                },
                true,
                &task.id,
            )
            .await?;
        let secret_files = conversation
            .project_id
            .as_ref()
            .and_then(|id| self.core.project(id).ok())
            .map(|p| p.prefs.secret_files)
            .unwrap_or_default();
        secrets::copy_secrets(self, owner, &repo, &path, &secret_files).await?;
        Ok(Workspace {
            repo,
            worktree: Some(path),
            branch: Some(branch),
            base: Some(tip),
            on_snapshot: false,
            target: Some(target),
            scratch,
            warmed,
            merge: None,
        })
    }
}
