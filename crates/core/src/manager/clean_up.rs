//! `clean_up` (SESSION-FIXES-PLAN.md bug 6): the thread's way to clean up when the user asks.
//! Brigadier already removes what a session's finished work leaves; this retries what it
//! couldn't and removes what is no longer in use, going only by its records of this
//! conversation: the cleanup ledger's owners of its tasks, reviews and session, and its tasks'
//! branches. Nothing found by name is touched, so another session's work never is.

use std::path::PathBuf;

use brigadier_providers::Artifact;

use super::{SessionManager, blocking, git_error};
use crate::Result;
use crate::model::ConversationId;
use crate::work::{ReviewState, Task};

impl SessionManager {
    /// Removes what this conversation's records hold that nothing uses any more, and says what
    /// went and what stayed, and why.
    pub(crate) async fn clean_up(&self, id: &ConversationId) -> Result<String> {
        let tasks = self.core.tasks(id).await?;
        let board = self.core.board(id).await?;
        let ledger = self.runtime.ledger();
        let mut removed: Vec<String> = Vec::new();
        let mut kept: Vec<String> = Vec::new();
        let mut owners: Vec<(String, String)> = Vec::new();
        for task in &tasks {
            let name = format!("task-{}", task.number);
            let held = super::review_hold::hold_owner(&task.id);
            let holds = !ledger.artifacts(&format!("task:{}", task.id)).is_empty()
                || !ledger.artifacts(&held).is_empty();
            if !task.state.is_final() {
                if holds {
                    kept.push(format!("{name}'s worktree and folders: it still works"));
                }
                continue;
            }
            if task.review_hold.is_some() {
                if holds {
                    kept.push(format!(
                        "{name}'s worktree and session: kept until its review's findings and the session's full checks settle"
                    ));
                }
                continue;
            }
            owners.push((format!("task:{}", task.id), name.clone()));
            owners.push((held, name));
        }
        for review in board.reviews.values() {
            let owner = format!("review:{}", review.id);
            if review.state == ReviewState::Running {
                if !ledger.artifacts(&owner).is_empty() {
                    kept.push(format!(
                        "review {}'s checkout: it still runs",
                        review.number
                    ));
                }
                continue;
            }
            owners.push((owner, format!("review {}", review.number)));
        }
        // The session's own worktree is in use, unless a merge's removal of it didn't finish.
        let session = format!("session:{id}");
        if ledger.disposing().contains(&session) {
            owners.push((session, "the merged session".into()));
        }
        for (owner, name) in owners {
            let artifacts = ledger.artifacts(&owner);
            if artifacts.is_empty() {
                continue;
            }
            let what = described(&artifacts);
            let leftovers = ledger.dispose(&owner).await;
            if leftovers.is_clean() {
                removed.push(format!("{name}'s {what}"));
            } else {
                kept.push(format!(
                    "{name}'s {what}: couldn't be removed yet ({}); Brigadier retries at its next launch",
                    leftovers.failures.join("; ")
                ));
            }
        }
        // Finished tasks' branches the engine left: deleted if merged, kept if not.
        for task in tasks
            .iter()
            .filter(|task| task.state.is_final() && task.review_hold.is_none())
        {
            let Some((repo, branch, target)) = branch_of(task, self) else {
                continue;
            };
            let (git, path, name) = (self.git.clone(), repo.clone(), branch.clone());
            let standing = blocking(move || {
                let repo = git.open(&path).map_err(git_error)?;
                let Some(tip) = repo.branch_tip(&name).map_err(git_error)? else {
                    return Ok(None);
                };
                Ok(Some((
                    tip,
                    repo.is_merged(&name, &target).map_err(git_error)?,
                )))
            })
            .await;
            let outcome = match standing {
                Ok(Some((tip, true))) => {
                    // The ref check no longer keeps it, then it goes at the tip found merged.
                    self.forget_branch(&repo, &branch);
                    let (git, name) = (self.git.clone(), branch.clone());
                    blocking(move || {
                        git.open(&repo)
                            .map_err(git_error)?
                            .delete_branch_at(&name, &tip)
                            .map_err(git_error)
                    })
                    .await
                    .map(|()| Some((branch, true)))
                }
                Ok(Some((_, false))) => Ok(Some((branch, false))),
                Ok(None) => Ok(None),
                Err(err) => Err(err),
            };
            match outcome {
                Ok(Some((branch, true))) => {
                    removed.push(format!("branch `{branch}` (task-{}, merged)", task.number));
                }
                Ok(Some((branch, false))) => kept.push(format!(
                    "branch `{branch}` (task-{}): its work isn't merged; the user can remove it in Settings › Storage",
                    task.number
                )),
                Ok(None) => {}
                Err(err) => kept.push(format!("task-{}'s branch: {err}", task.number)),
            }
        }
        let mut text = if removed.is_empty() {
            "Nothing to remove: Brigadier had already cleaned up after this session's finished work.".to_owned()
        } else {
            format!("Removed: {}.", removed.join("; "))
        };
        if !kept.is_empty() {
            text.push_str(&format!(" Kept: {}.", kept.join("; ")));
        }
        text.push_str(" The session's own worktree and branch, your temp folder and other sessions' work are never touched; don't remove anything by hand.");
        Ok(text)
    }
}

/// A finished task's branch, its repository and what it lands on.
fn branch_of(task: &Task, manager: &SessionManager) -> Option<(PathBuf, String, String)> {
    let workspace = task.workspace.as_ref()?;
    let branch = workspace.branch.clone()?;
    let target = workspace.target.clone()?;
    let repo = manager.task_repo(task).ok()?;
    Some((repo, branch, target))
}

/// What a ledger owner held, in a few words.
fn described(artifacts: &[Artifact]) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for artifact in artifacts {
        let part = match artifact {
            Artifact::Worktree { .. } => "worktree",
            Artifact::ScratchDir { .. } | Artifact::ClaudeTempDir { .. } => "folders",
            Artifact::Process { .. } | Artifact::ProcessesIn { .. } => "processes",
            _ => "CLI session files",
        };
        if !parts.contains(&part) {
            parts.push(part);
        }
    }
    parts.join(", ")
}
