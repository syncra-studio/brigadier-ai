//! Undo and Reapply of what a request's workers landed (the turn diff card, PLAN §4).
//!
//! Undo is a new commit that reverts the request's landed commits; Reapply reverts that one,
//! and so on. Each lands like any landing: a fast-forward of the branch the work landed on,
//! under the landing lock, never over the user's uncommitted changes to the same files, and
//! never a history rewrite. It is refused when a later commit changed the same files. The
//! orchestrator hears of it with the user's next message, not in a turn of its own.

use brigadier_git::{LandBlock, LandOutcome, LandRequest, Oid, RevertOutcome};

use super::{SessionManager, blocking, git_error};
use crate::model::{ConversationId, ConversationKind};
use crate::work::{RequestUndo, Task};
use crate::{Error, Result};

/// How often an Undo chases a branch that moves under it.
const UNDO_ATTEMPTS: usize = 3;

/// Why an Undo or Reapply didn't happen, in the words its toast shows after "Couldn't undo the
/// changes" (or reapply).
fn refused(reason: impl Into<String>) -> Error {
    Error::Invalid(reason.into())
}

impl SessionManager {
    /// The user's Undo (or, with `reapply`, Reapply) of what a request's workers landed.
    pub async fn undo_request(
        &self,
        conversation_id: ConversationId,
        request_id: String,
        reapply: bool,
    ) -> Result<crate::work::UndoOutcome> {
        if self
            .change_record(&conversation_id, &request_id)
            .await?
            .is_some()
        {
            return self
                .undo_record(&conversation_id, &request_id, reapply)
                .await;
        }
        let conversation = self.core.conversation(&conversation_id)?;
        if conversation.kind != ConversationKind::Session {
            return Err(refused("only a session's workers land changes"));
        }
        let board = self.core.board(&conversation_id).await?;
        let request = board
            .requests
            .get(&request_id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("request {request_id}")))?;
        let tasks: Vec<&Task> = board
            .tasks
            .values()
            .filter(|task| task.request_id.as_deref() == Some(request_id.as_str()))
            .collect();
        if tasks
            .iter()
            .any(|task| task.kind.writes() && !task.state.is_final())
        {
            return Err(refused("its workers are still landing changes"));
        }
        let landed: Vec<&Task> = tasks
            .into_iter()
            .filter(|task| task.landed.is_some() && task.candidate.is_some())
            .collect();
        let first = landed
            .first()
            .ok_or_else(|| refused("its workers landed no changes"))?;
        if self.local_checkout(&conversation_id).is_some() {
            self.record_legacy_local_changes(
                &conversation_id,
                &request,
                landed
                    .iter()
                    .filter_map(|task| {
                        task.candidate
                            .as_ref()
                            .map(|candidate| Oid(candidate.commit.clone()))
                    })
                    .collect(),
            )
            .await?;
            return self
                .undo_record(&conversation_id, &request_id, reapply)
                .await;
        }
        let reverted = request.undo.as_ref().is_some_and(|undo| undo.reverted);
        if reverted != reapply {
            return Err(refused(if reapply {
                "its changes are not reverted"
            } else {
                "its changes are already reverted"
            }));
        }
        let target = first
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.target.clone())
            .ok_or_else(|| refused("the branch its changes landed on is unknown"))?;
        let repo = self.task_repo(first)?;
        // Undo reverts the request's landings; after that, each step reverts the one before.
        let mut commits: Vec<Oid> = match request.undo.as_ref().and_then(|u| u.commits.last()) {
            Some(last) => vec![Oid(last.clone())],
            None => landed
                .iter()
                .filter_map(|task| task.candidate.as_ref().map(|c| Oid(c.commit.clone())))
                .collect(),
        };
        commits.dedup();
        let verb = if reapply { "Reapply" } else { "Undo" };
        let message = format!(
            "{verb} \"{}\"\n\nThe user's {} of what this request's workers landed, from Brigadier.",
            request.preview,
            verb.to_lowercase()
        );
        let restored = {
            let (git, repo, commits) = (self.git.clone(), repo.clone(), commits.clone());
            blocking(move || {
                let repo = git.open(&repo).map_err(git_error)?;
                let (from, to) = repo.changes_of(&commits).map_err(git_error)?;
                Ok(repo
                    .diff_stat(&from, &to)
                    .map_err(git_error)?
                    .files
                    .into_iter()
                    .map(|file| file.path)
                    .collect())
            })
            .await?
        };
        let mut outcome = None;
        for _ in 0..UNDO_ATTEMPTS {
            let (git, repo, target, commits, message) = (
                self.git.clone(),
                repo.clone(),
                target.clone(),
                commits.clone(),
                message.clone(),
            );
            let attempt = blocking(move || {
                let repo = git.open(&repo).map_err(git_error)?;
                let tip = repo.branch_commit(&target).map_err(git_error)?;
                match repo
                    .prepare_revert(&commits, &tip, &message)
                    .map_err(git_error)?
                {
                    RevertOutcome::Ready { commit } => repo
                        .land(&LandRequest {
                            branch: target,
                            expected_tip: tip,
                            commit,
                        })
                        .map_err(git_error),
                    RevertOutcome::Touched { paths } => Err(refused(format!(
                        "later changes touch the same files ({})",
                        paths.join(", ")
                    ))),
                    RevertOutcome::Conflicts { paths } => Err(refused(format!(
                        "it conflicts with later changes in {}",
                        paths.join(", ")
                    ))),
                }
            })
            .await?;
            match attempt {
                LandOutcome::Landed { new_tip } => {
                    outcome = Some(new_tip);
                    break;
                }
                LandOutcome::Blocked(LandBlock::TipMoved { .. }) => {}
                LandOutcome::Blocked(LandBlock::Collisions { paths, .. }) => {
                    let paths: Vec<String> = paths.into_iter().map(|p| p.path).collect();
                    return Err(refused(format!(
                        "you have uncommitted changes to {}",
                        paths.join(", ")
                    )));
                }
                LandOutcome::Blocked(block) => return Err(refused(block.to_string())),
            }
        }
        let new_tip = outcome.ok_or_else(|| refused(format!("`{target}` kept moving")))?;
        let mut undo = request.undo.unwrap_or(RequestUndo {
            reverted: false,
            commits: Vec::new(),
        });
        undo.reverted = !reapply;
        undo.commits.push(new_tip.0.clone());
        self.core
            .set_request_undo(&conversation_id, &request_id, undo)
            .await?;
        let short: String = new_tip.0.chars().take(8).collect();
        let note = if reapply {
            format!(
                "The user reapplied what request \"{}\" landed (commit {short} on `{target}`).",
                request.preview
            )
        } else {
            format!(
                "The user undid what request \"{}\" landed: commit {short} on `{target}` reverts it.",
                request.preview
            )
        };
        self.conv(&conversation_id)?.note(note).await;
        Ok(crate::work::UndoOutcome {
            restored,
            held: Vec::new(),
        })
    }
}
