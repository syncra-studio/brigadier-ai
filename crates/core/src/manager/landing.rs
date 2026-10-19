//! Landing a phase's work (the delegator flow, §2.4 of the flow spec): the worker that ends a
//! phase (its lead, or a verifier the orchestrator started on top of the lead's work)
//! committed its work in small steps in its own worktree, branched from the session's tip. `land_phase` moves those
//! commits onto the target branch, with no card and no checks of its own:
//!
//! 1. **Leftovers**: anything the worker left uncommitted is committed, litter left out.
//! 2. **Litter guard over the whole range** (phase start..HEAD): logs, scratch files and new
//!    files no report of the phase names are dropped from every commit and listed.
//! 3. **Rebase** onto the target's tip when it moved (each commit replayed, authors and
//!    messages kept), and land at once: no worker checks it again first. Conflicts go to a
//!    merge worker.
//! 4. **Fast-forward** the target, never over uncommitted, untracked or ignored files, and
//!    only if the target is still where it was (the git engine checks it right before
//!    mutating). Anything unsafe leaves the task "ready to land" with nothing changed.
//!
//! Every landing then gets one read-only review by the other vendor in the background
//! ([`super::review_runs`]); nothing waits for it. It also makes the session's full checks due
//! at the new tip: they run once for the landings close together, in the background, on the
//! integrated branch ([`super::integrated_checks`], SESSION-FIXES-PLAN.md §11), and a failure
//! goes back to the worker that broke it. A merge of the session needs them passed at its tip.
//!
//! Finishing a new-worktree session merges the session branch into its base the same way, on
//! the user's word as the thread judges it: their message, card answer or click.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use brigadier_git::{
    ChangeKind, CommitOutcome, LandBlock, LandOutcome, LandRequest, MergeOutcome, Oid,
    SeriesOutcome, Side, litter,
};

use super::conversation::Envelope;
use super::workers::Workspace;
use super::{SessionManager, blocking, git_error, merge_consent};
use crate::model::{ConversationId, Environment, MessageRole, Setup};
use crate::work::{
    AnswerOrigin, DecisionKind, DecisionSource, DiffStat, ExcludedFile, FileStat, InjectionKind,
    MergeInputs, MergedUpstream, OrchestratorStepKind, PhaseStage, QuestionKind, Task, TaskState,
};
use crate::{Error, Result};

/// Times a landing starts over when the target moves between the rebase and the
/// fast-forward.
const LAND_TRIES: usize = 3;

/// What moving a task's commits onto its target came to.
enum Moved {
    /// The target's new tip holds them, on top of `from`, its tip before.
    Landed {
        from: Oid,
        tip: Oid,
        commits: u32,
        excluded: Vec<ExcludedFile>,
    },
    /// Nothing was left to land once litter was left out.
    Nothing {
        excluded: Vec<ExcludedFile>,
    },
    Conflicts {
        onto: Oid,
        paths: Vec<String>,
    },
    HookFailed {
        output: String,
    },
    Blocked(LandBlock),
    /// A merge task's merge failed its check (SESSION-FIXES-PLAN.md Q2): `text` tells its
    /// worker what to fix.
    MergeRefused {
        text: String,
    },
    /// A merge task's target moved to `tip` while its worker merged: the worker merges it in
    /// too.
    Outrun {
        tip: Oid,
    },
}

impl SessionManager {
    /// `land_phase`: lands a reported write task's commits (and so its phase's: a verifier
    /// works on top of its lead's commits) on the session's branch. Returns the outcome.
    pub(crate) async fn land_phase(&self, id: &ConversationId, task: Task) -> Result<String> {
        let _fence = self.enter(id)?;
        if !task.kind.writes() {
            return Err(Error::Invalid(format!(
                "task-{} is a {:?} task: only implement and merge tasks land",
                task.number, task.kind
            )));
        }
        if !matches!(task.state, TaskState::Reported | TaskState::ReadyToLand) {
            return Err(Error::Invalid(format!(
                "task-{} is {:?}; only a reported task lands",
                task.number, task.state
            )));
        }
        // A verifier the orchestrator started on this work lands it with its own: landing it
        // first would replay its commits under the verifier's. A fact, not an error (#251).
        if let Some(verifier) = self.verifier_of(&task).await {
            return Ok(format!(
                "Nothing landed: task-{}'s work is under task-{}'s verification, built on its commits, so it lands with task-{} once that reports.",
                task.number, verifier.number, verifier.number
            ));
        }
        let later = self.later_request_for(id, &task).await;
        // Checked and changed in one step: a stop, a steer or a newer report that came after
        // `task` was read wins.
        let still = |now: &Task| now.state == task.state && super::workers::same_report(now, &task);
        let Some(task) = self
            .update_task_if(id, &task.id, still, |t| {
                t.state = TaskState::Landing;
                t.blocked_reason = None;
                t.landing = Some(t.title.clone());
                t.landing_from = None;
                if later.is_some() {
                    t.request_id = later;
                }
            })
            .await?
        else {
            return Err(Error::Invalid(format!(
                "task-{} changed meanwhile; look at it again before landing it",
                task.number
            )));
        };
        // Boxed: a landing's futures are large, and it runs deep in a tool call's.
        match Box::pin(self.move_commits(&task)).await {
            Ok(moved) => Ok(Box::pin(self.settle_landing(&task, moved, false)).await.1),
            Err(err) => {
                self.hand_back(&task, TaskState::Reported, None).await;
                Err(err)
            }
        }
    }

    /// A merge worker sent back during its landing (its merge was refused, or the target moved
    /// under it) reported again: its work lands now, on its own. The orchestrator hears the
    /// outcome.
    pub(crate) async fn land_after_rework(&self, task: &Task) {
        let Ok(_fence) = self.enter(&task.conversation_id) else {
            return;
        };
        let Ok(task) = self
            .update_task(&task.conversation_id, &task.id, |t| {
                t.state = TaskState::Landing;
                t.blocked_reason = None;
                t.landing_from = None;
            })
            .await
        else {
            return;
        };
        let (landed, text) = match Box::pin(self.move_commits(&task)).await {
            Ok(moved) => Box::pin(self.settle_landing(&task, moved, true)).await,
            Err(err) => {
                self.landing_problem(&task, &err.to_string(), TaskState::Reported)
                    .await;
                return;
            }
        };
        // Problems were handed to the orchestrator by `settle_landing`; a landing is news too.
        let text = if landed {
            format!("[landed task-{}] {text}", task.number)
        } else {
            text
        };
        if landed || text.starts_with("[nothing to land") {
            self.deliver(
                &task.conversation_id,
                Envelope {
                    kind: InjectionKind::Decision,
                    label: format!("landed task-{}", task.number),
                    task_id: Some(task.id.clone()),
                    text,
                },
            )
            .await;
        }
    }

    /// After a restart, a landing the restart cut off once the target had moved: its commits
    /// are on the target, above the tip recorded before it moved, so it is recorded as landed
    /// and the orchestrator hears so (BUILD-PLAN D1.4). False when nothing of it landed.
    pub(crate) async fn landed_before_restart(&self, task: &Task) -> bool {
        let Some(from) = task.landing_from.clone().map(Oid) else {
            return false;
        };
        let Some(workspace) = &task.workspace else {
            return false;
        };
        let (Some(target), Some(branch)) = (workspace.target.clone(), workspace.branch.clone())
        else {
            return false;
        };
        let Ok(repo) = self.task_repo(task) else {
            return false;
        };
        let git = self.git.clone();
        let (since, into) = (from.clone(), target.clone());
        let found = blocking(move || {
            let repo = git.open(&repo).map_err(git_error)?;
            let (Some(tip), Some(now)) = (
                repo.branch_tip(&branch).map_err(git_error)?,
                repo.branch_tip(&into).map_err(git_error)?,
            ) else {
                return Ok(None);
            };
            let landed = tip != since
                && repo.ancestor(&since, &tip).map_err(git_error)?
                && repo.ancestor(&tip, &now).map_err(git_error)?;
            if !landed {
                return Ok(None);
            }
            let commits = repo.count_commits(&since, &tip).map_err(git_error)?;
            Ok(Some((tip, commits)))
        })
        .await;
        let (tip, commits) = match found {
            Ok(Some(found)) => found,
            Ok(None) => return false,
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "could not check an interrupted landing");
                return false;
            }
        };
        self.landed(task, &target, &from, &tip, commits).await;
        self.deliver(
            &task.conversation_id,
            Envelope {
                kind: InjectionKind::Decision,
                label: format!("landed task-{}", task.number),
                task_id: Some(task.id.clone()),
                text: format!(
                    "[landed task-{}] Brigadier restarted as it landed: {} on `{target}` (at {}).",
                    task.number,
                    commits_word(commits),
                    short(&tip)
                ),
            },
        )
        .await;
        true
    }

    /// The verifier that works on top of `task`'s commits and lands them, while it lives.
    pub(crate) async fn verifier_of(&self, task: &Task) -> Option<Task> {
        let board = self.core.board(&task.conversation_id).await.ok()?;
        board
            .tasks
            .values()
            .filter(|other| {
                other.subject.as_ref() == Some(&task.id)
                    && other.role == Some(crate::work::WorkerRole::Verifier)
                    && !other.state.is_final()
            })
            .max_by_key(|other| other.number)
            .cloned()
    }

    /// Steps 1–4 in the task's worktree and repository.
    async fn move_commits(&self, task: &Task) -> Result<Moved> {
        let workspace = task
            .workspace
            .clone()
            .ok_or_else(|| Error::Invalid("the task has no workspace".into()))?;
        let worktree = PathBuf::from(
            workspace
                .worktree
                .clone()
                .ok_or_else(|| Error::Invalid("the task has no worktree".into()))?,
        );
        let base = Oid(workspace
            .base
            .clone()
            .ok_or_else(|| Error::Invalid("the task has no base".into()))?);
        let target = workspace
            .target
            .clone()
            .ok_or_else(|| Error::Invalid("the task has no target branch".into()))?;
        let repo = self.task_repo(task)?;
        let reported = self.phase_reported(task).await;
        // The ref check, for the landing task's worker: what others broke is put back first.
        self.ref_check(&worktree, super::refcheck::Caller::Task(task.id.clone()))
            .await;
        let git = self.git.clone();
        let checkout = worktree.clone();
        let moved = if task.kind == crate::work::TaskKind::Merge {
            let merge = workspace.merge.clone().ok_or_else(|| {
                Error::Invalid(format!(
                    "task-{} is a merge task from before merge workers ran git themselves: stop it with stop_worker and delegate a new merge task.",
                    task.number
                ))
            })?;
            let deleted: Vec<String> = task
                .report
                .iter()
                .flat_map(|report| &report.deleted)
                .map(|line| normalize(line.split(':').next().unwrap_or_default().trim()))
                .collect();
            blocking(move || {
                let repo = git.open(&repo).map_err(git_error)?;
                let worktree = git.open_worktree(&worktree).map_err(git_error)?;
                land_merge(
                    &repo, &worktree, &base, &target, &merge, &reported, &deleted,
                )
            })
            .await?
        } else {
            // Boxed: its loop holds a task update and two blocking calls, too big a frame for
            // the landing's stack.
            crate::heap(|| {
                self.land_commits(
                    task, git, worktree, repo, base, target, reported, &workspace,
                )
            })
            .await?
        };
        // The merge worker moved the task branch (the engine's own rewrite was recorded as it
        // was made): its new tip is the recorded one.
        if let Some(branch) = &workspace.branch {
            self.moved_branch(&checkout, branch);
        }
        Ok(moved)
    }

    /// Steps 1–4 for a task's own commits.
    #[allow(clippy::too_many_arguments)]
    async fn land_commits(
        &self,
        task: &Task,
        git: brigadier_git::Git,
        worktree: PathBuf,
        repo: PathBuf,
        base: Oid,
        target: String,
        reported: Vec<String>,
        workspace: &crate::work::TaskWorkspace,
    ) -> Result<Moved> {
        let leftovers = self.commit_message(format!(
            "{}\n\nWhat task-{} left uncommitted, committed as it landed.",
            task.title, task.number
        ));
        let omit_ai_coauthors = self.core.settings().omit_ai_coauthors;
        let (records, branch) = (self.ref_records.clone(), workspace.branch.clone());
        let mut tries = 0;
        loop {
            tries += 1;
            #[cfg(test)]
            let rewritten = self.landing_rewritten.lock().unwrap().clone();
            let prepared = {
            let (git, repo, worktree, target, reported, leftovers, records, branch) = (
                git.clone(),
                repo.clone(),
                worktree.clone(),
                target.clone(),
                reported.clone(),
                leftovers.clone(),
                records.clone(),
                branch.clone(),
            );
            let base = base.clone();
            blocking(move || {
                let repo = git.open(&repo).map_err(git_error)?;
                let worktree = git.open_worktree(&worktree).map_err(git_error)?;
                let mut excluded = Vec::new();
                let onto = repo
                    .branch_tip(&target)
                    .map_err(git_error)?
                    .ok_or_else(|| Error::Invalid(format!("branch {target} does not exist")))?;
                let (tip, commits) = {
                    // 1. What it left uncommitted.
                    let head = worktree.head().map_err(git_error)?;
                    let left = worktree.changes(&head).map_err(git_error)?;
                    let include = keep_paths(&left, &reported, &mut excluded);
                    if !include.is_empty()
                        && let CommitOutcome::HookFailed { output } = worktree
                            .commit_candidate(&include, &leftovers)
                            .map_err(git_error)?
                    {
                        return Ok(Err(Moved::HookFailed { output }));
                    }
                    // 2. Litter over the whole range: a committed new file needs a report
                    // that names it, as an untracked one does.
                    // A file added and removed again within the series counts too: its
                    // content would stay in the history that lands. So does a tracked file
                    // changed and put back (a log): only the always-excluded patterns drop it.
                    let net = worktree.changes(&base).map_err(git_error)?;
                    let passing = worktree.passing_files(&base).map_err(git_error)?;
                    let restored: Vec<String> = worktree
                        .series_paths(&base)
                        .map_err(git_error)?
                        .into_iter()
                        .filter(|path| {
                            !net.iter().any(|change| &change.path == path)
                                && !passing.contains(path)
                        })
                        .collect();
                    let range: Vec<_> = net
                        .into_iter()
                        .map(|mut change| {
                            if change.kind == ChangeKind::Added {
                                change.untracked = true;
                            }
                            change
                        })
                        .chain(passing.into_iter().map(|path| brigadier_git::Change {
                            path,
                            kind: ChangeKind::Added,
                            untracked: true,
                        }))
                        .chain(restored.into_iter().map(|path| brigadier_git::Change {
                            path,
                            kind: ChangeKind::Modified,
                            untracked: false,
                        }))
                        .collect();
                    let mut drop = Vec::new();
                    for (change, verdict) in litter::classify(&range, &reported) {
                        if let litter::Verdict::Exclude { reason } = verdict {
                            if !excluded
                                .iter()
                                .any(|e: &ExcludedFile| e.path == change.path)
                            {
                                excluded.push(ExcludedFile {
                                    path: change.path.clone(),
                                    reason,
                                });
                            }
                            if let ChangeKind::Renamed { from } = change.kind {
                                drop.push(from);
                            }
                            drop.push(change.path);
                        }
                    }
                    // 3. Onto the target's tip, and the messages as the user wants them, before
                    // they land: the worker's branch moves to the rewritten commits, so it is
                    // found merged once they have landed. The engine's own rewrite, under the
                    // ref lock and recorded there: no check takes it for damage, whether or
                    // not the worker still runs.
                    let rewrite = || -> Result<std::result::Result<(Oid, u32), Moved>> {
                        let (tip, commits) = match worktree
                            .replay_series(&base, &onto, &drop)
                            .map_err(git_error)?
                        {
                            SeriesOutcome::Conflicts { paths } => {
                                return Ok(Err(Moved::Conflicts {
                                    onto: onto.clone(),
                                    paths,
                                }));
                            }
                            SeriesOutcome::Replayed { tip, commits, .. } => (tip, commits),
                        };
                        let tip = if omit_ai_coauthors && commits > 0 {
                            match worktree.clean_ai_coauthors(&onto, &tip) {
                                Ok(cleaned) => cleaned.unwrap_or(tip),
                                Err(err) => {
                                    tracing::warn!(error = %err, "could not leave AI co-authors out of the commits that land");
                                    tip
                                }
                            }
                        } else {
                            tip
                        };
                        Ok(Ok((tip, commits)))
                    };
                    let rewritten_now = match &branch {
                        Some(branch) => super::refcheck::engine_rewrite(&repo, &records, branch, rewrite),
                        None => rewrite(),
                    };
                    match rewritten_now? {
                        Ok(done) => done,
                        Err(conflicts) => return Ok(Err(conflicts)),
                    }
                };
                #[cfg(test)]
                if let Some(rewritten) = &rewritten {
                    rewritten();
                }
                if commits == 0 {
                    return Ok(Err(Moved::Nothing { excluded }));
                }
                Ok(Ok((onto, tip, commits, excluded)))
            })
            }
            .await?;
            let (onto, tip, commits, excluded) = match prepared {
                Ok(ready) => ready,
                Err(moved) => return Ok(moved),
            };
            // Recorded before the target moves: a restart between here and the landing's
            // record finds it landed, or not, by the target's ancestry (BUILD-PLAN D1.4).
            self.update_task(&task.conversation_id, &task.id, |t| {
                t.landing_from = Some(onto.0.clone());
            })
            .await?;
            // 4. Fast-forward, rebased or not: the session's full checks run on what lands
            // (§11).
            let request = LandRequest {
                branch: target.clone(),
                expected_tip: onto.clone(),
                commit: tip,
            };
            let (git, repo) = (git.clone(), repo.clone());
            let outcome = blocking(move || {
                git.open(&repo)
                    .map_err(git_error)?
                    .land(&request)
                    .map_err(git_error)
            })
            .await?;
            match outcome {
                LandOutcome::Landed { new_tip } => {
                    return Ok(Moved::Landed {
                        from: onto,
                        tip: new_tip,
                        commits,
                        excluded,
                    });
                }
                LandOutcome::Blocked(LandBlock::TipMoved { .. }) if tries < LAND_TRIES => {}
                LandOutcome::Blocked(block) => return Ok(Moved::Blocked(block)),
            }
        }
    }

    /// Acts on what moving the commits came to; returns the outcome for the orchestrator.
    /// `on_its_own`: Brigadier lands it after its worker was sent back, so a problem reaches the
    /// orchestrator as a message (the caller announces a landing). Whether it landed comes
    /// with it.
    async fn settle_landing(&self, task: &Task, moved: Moved, on_its_own: bool) -> (bool, String) {
        let target = task
            .workspace
            .as_ref()
            .and_then(|w| w.target.clone())
            .unwrap_or_default();
        // Boxed, like each landing and merge_again below: each call's future is otherwise made
        // in this frame, a slot apiece in an unoptimized build.
        let problem = |reason: String, state: TaskState| {
            Box::pin(async move {
                if on_its_own {
                    self.landing_problem(task, &reason, state).await;
                } else {
                    self.hand_back(task, state, Some(&reason)).await;
                }
                format!("[not landed task-{}] {reason}", task.number)
            })
        };
        let text = match moved {
            Moved::Landed {
                from,
                tip,
                commits,
                excluded,
            } => {
                crate::heap(|| self.landed(task, &target, &from, &tip, commits)).await;
                return (
                    true,
                    format!(
                        "Landed {} on `{target}` (now at {}).{}",
                        commits_word(commits),
                        short(&tip),
                        litter_note(&excluded)
                    ),
                );
            }
            Moved::Nothing { excluded } => {
                // Nothing of the work it carries is left to land (all of it was litter, say):
                // that work ends with it.
                for done in crate::heap(|| self.landed_with(task)).await {
                    crate::heap(|| self.dispose_task(&done, TaskState::Done)).await;
                    crate::heap(|| self.set_phase_stage(&done, PhaseStage::Done)).await;
                }
                format!(
                    "[nothing to land task-{}] It has no commits to land.{}",
                    task.number,
                    litter_note(&excluded)
                )
            }
            Moved::Conflicts { onto, paths } => {
                problem(
                    format!(
                        "Its commits conflict with the current `{target}` ({}) in: {}. {}",
                        short(&onto),
                        paths.join(", "),
                        conflict_step(task, &target)
                    ),
                    TaskState::Reported,
                )
                .await
            }
            Moved::HookFailed { output } => {
                problem(
                    format!(
                        "The repository's commit hooks refused to commit what it left uncommitted:\n{}\nSend task-{} back with message_worker to fix this.",
                        clip(&output, 3_000),
                        task.number
                    ),
                    TaskState::Reported,
                )
                .await
            }
            Moved::Blocked(block) => {
                problem(
                    format!(
                        "It is ready to land, but landing now is not safe: {block} Nothing was changed. Call land_phase for task-{} again once that is resolved.",
                        task.number
                    ),
                    TaskState::ReadyToLand,
                )
                .await
            }
            // A second refusal in a row goes to the orchestrator: the worker had its chance.
            Moved::MergeRefused { text } if on_its_own => {
                problem(
                    format!(
                        "Its merge was refused again: {text} Send task-{} back with message_worker, or stop it.",
                        task.number
                    ),
                    TaskState::Reported,
                )
                .await
            }
            Moved::MergeRefused { text } => {
                let why = format!("Nothing landed: {text}");
                match crate::heap(|| self.merge_again(task, why, None)).await {
                    Ok(()) => format!(
                        "[not landed task-{}] Its merge failed the check: {text} Its worker was sent back with that; its work lands on its own after its next report, and you hear when it has landed.",
                        task.number
                    ),
                    Err(err) => {
                        problem(
                            format!("Its merge failed the check ({text}), but its worker could not be told: {err}"),
                            TaskState::Reported,
                        )
                        .await
                    }
                }
            }
            Moved::Outrun { tip } => {
                let why = format!(
                    "`{target}` moved to {} while you merged, so nothing landed. Merge it in too with `git merge {target}`, resolve any conflict and commit; then build, test and call submit_report again.",
                    short(&tip)
                );
                match crate::heap(|| self.merge_again(task, why, Some(tip.clone()))).await {
                    Ok(()) => format!(
                        "`{target}` moved ({}) while task-{} merged, so its worker was sent back to merge it in too; its work lands on its own after its next report, and you hear when it has landed.",
                        short(&tip),
                        task.number
                    ),
                    Err(err) => {
                        problem(
                            format!("`{target}` moved while it merged, but its worker could not be told to merge it in too: {err}"),
                            TaskState::Reported,
                        )
                        .await
                    }
                }
            }
        };
        (false, text)
    }

    /// Sends a merge task's worker back to its merge with `text` (Q2), the target's new tip
    /// recorded as where its merge starts when the target `moved`. Its task stays landing, so
    /// its next report lands on its own (`land_after_rework`).
    async fn merge_again(&self, task: &Task, text: String, moved: Option<Oid>) -> Result<()> {
        let task = self
            .update_task(&task.conversation_id, &task.id, |t| {
                if let (Some(w), Some(tip)) = (t.workspace.as_mut(), &moved) {
                    w.base = Some(tip.0.clone());
                }
            })
            .await?;
        self.message_worker(&task.conversation_id, &task, text, "Brigadier")
            .await
            .map(|_| ())
    }

    /// Commits what a write task left uncommitted in its worktree (litter left out; `extra`:
    /// paths a report not stored yet names), so a review or a verifier starts from its
    /// committed work. A Codex worker's sandbox can't write the worktree's git directory, so
    /// Brigadier commits for it.
    pub(crate) async fn commit_leftovers(
        &self,
        task: &Task,
        extra: &[String],
        message: &str,
    ) -> Result<()> {
        if !task.kind.writes() || task.kind == crate::work::TaskKind::Merge {
            return Ok(());
        }
        let Some(worktree) = task.workspace.as_ref().and_then(|w| w.worktree.clone()) else {
            return Ok(());
        };
        let mut reported = self.phase_reported(task).await;
        reported.extend(extra.iter().flat_map(|p| reported_paths(p)));
        let (git, message) = (self.git.clone(), self.commit_message(message.to_owned()));
        blocking(move || {
            let worktree = git.open_worktree(Path::new(&worktree)).map_err(git_error)?;
            let head = worktree.head().map_err(git_error)?;
            let left = worktree.changes(&head).map_err(git_error)?;
            let include = keep_paths(&left, &reported, &mut Vec::new());
            if include.is_empty() {
                return Ok(());
            }
            match worktree
                .commit_candidate(&include, &message)
                .map_err(git_error)?
            {
                CommitOutcome::HookFailed { output } => Err(Error::Invalid(format!(
                    "The repository's commit hooks refused to commit the work:\n{}",
                    clip(&output, 2_000)
                ))),
                _ => Ok(()),
            }
        })
        .await
    }

    /// The new files in an implement task's worktree (committed, staged or untracked) that
    /// no report of its phase names, nor `extra` (the report being submitted): the landing
    /// would leave them out, so the worker must list or delete them first. Litter is not
    /// among them.
    pub(crate) async fn unreported_files(
        &self,
        task: &Task,
        extra: &[String],
    ) -> Result<Vec<String>> {
        if task.kind != crate::work::TaskKind::Implement {
            return Ok(Vec::new());
        }
        let Some(workspace) = task.workspace.as_ref() else {
            return Ok(Vec::new());
        };
        let (Some(worktree), Some(base)) = (workspace.worktree.clone(), workspace.base.clone())
        else {
            return Ok(Vec::new());
        };
        let mut reported = self.phase_reported(task).await;
        reported.extend(extra.iter().flat_map(|p| reported_paths(p)));
        let git = self.git.clone();
        blocking(move || {
            let worktree = git.open_worktree(Path::new(&worktree)).map_err(git_error)?;
            // A new file needs a report whether it is committed, staged or untracked, as at
            // landing.
            let changes: Vec<_> = worktree
                .changes(&Oid(base))
                .map_err(git_error)?
                .into_iter()
                .map(|mut change| {
                    if change.kind == ChangeKind::Added {
                        change.untracked = true;
                    }
                    change
                })
                .collect();
            Ok(litter::unreported(&changes, &reported))
        })
        .await
    }

    /// Every file a report of `task`'s phase names: the provenance a new file needs to land.
    async fn phase_reported(&self, task: &Task) -> Vec<String> {
        let mut reported: Vec<String> = Vec::new();
        let mut add = |t: &Task| {
            if let Some(report) = &t.report {
                reported.extend(report.changes.iter().flat_map(|p| reported_paths(p)));
            }
        };
        add(task);
        if let Ok(board) = self.core.board(&task.conversation_id).await {
            let mut subject = task.subject.clone();
            while let Some(id) = subject {
                let Some(t) = board.tasks.get(&id) else { break };
                add(t);
                subject = t.subject.clone();
            }
            if let Some(phase) = task.phase {
                for t in board.tasks.values() {
                    if t.request_id == task.request_id && t.phase == Some(phase) && t.kind.writes()
                    {
                        add(t);
                    }
                }
            }
        }
        reported.sort();
        reported.dedup();
        reported
    }

    /// The tasks whose work landed with `task`'s: it, and the work it builds on (a verifier's
    /// lead, a merge task's conflicting task).
    pub(crate) async fn landed_with(&self, task: &Task) -> Vec<Task> {
        let mut tasks = vec![task.clone()];
        let Ok(board) = self.core.board(&task.conversation_id).await else {
            return tasks;
        };
        let mut subject = task.subject.clone();
        while let Some(id) = subject {
            let Some(t) = board.tasks.get(&id) else { break };
            if t.kind.writes() && !t.state.is_final() && !tasks.iter().any(|known| known.id == t.id)
            {
                tasks.push(t.clone());
            }
            subject = t.subject.clone();
        }
        tasks
    }

    /// The session's own work moved `branch` from `from` to `to`: a landing, or the thread's
    /// commits that fix review findings. A held merge's consent covers it (SESSION-FIXES-PLAN.md
    /// Q3.4): its tip moves on along such moves, in whatever order they are told.
    pub(crate) async fn move_held_merge(
        &self,
        id: &ConversationId,
        branch: &str,
        from: &Oid,
        to: &Oid,
    ) {
        let Ok(conv) = self.conv(id) else {
            return;
        };
        let mut moves = conv.held_moves.lock().await;
        if let Ok(board) = self.core.board(id).await
            && let Some(held) = pending_merge(&board)
            && held.branch == branch
        {
            moves.insert(from.0.clone(), to.0.clone());
            let tip = advance_tip(&held.tip, &mut moves);
            if tip != held.tip {
                self.orchestrator_step(
                    id,
                    OrchestratorStepKind::MergeHeld {
                        branch: held.branch,
                        base: held.base,
                        asked_in: held.asked_in,
                        consented_at_ms: held.consented_at_ms,
                        condition: held.condition,
                        waiting: held.waiting,
                        tip,
                    },
                )
                .await;
            }
        }
    }

    async fn landed(&self, task: &Task, target: &str, from: &Oid, new_tip: &Oid, commits: u32) {
        // Each step on the heap: an unoptimized build gives every inline `.await` its own stack
        // slots at once, and this frame alone took a fifth of a runtime thread's stack (#356).
        crate::heap(|| {
            self.refresh_changes(
                &task.conversation_id,
                task.request_id.as_deref(),
                super::request_changes::Refresh::Landing { from, tip: new_tip },
            )
        })
        .await;
        // Counted before the task is final, until its review is stored: a held merge waits.
        let review_due = self.conv(&task.conversation_id).ok().map(ReviewDue::new);
        // The thread's own commits below the landing get their review; the landing has its own.
        if let Ok(repo) = self.task_repo(task) {
            crate::heap(|| {
                self.thread_branch_landed(&task.conversation_id, &repo, target, from, new_tip)
            })
            .await;
        }
        let tasks = crate::heap(|| self.landed_with(task)).await;
        // The session's own landing on a held merge's branch is covered by its consent.
        crate::heap(|| self.move_held_merge(&task.conversation_id, target, from, new_tip)).await;
        // A fix of landed work's review findings gets no review of its own, only the
        // targeted re-check of a finding the thread marked high (SESSION-FIXES-PLAN.md Q6.1).
        let fixed = match (&task.role, &task.subject) {
            (Some(crate::work::WorkerRole::Fix), Some(subject)) => {
                crate::heap(|| self.task_by_id(&task.conversation_id, subject))
                    .await
                    .ok()
                    .filter(|subject| subject.landed.is_some())
            }
            _ => None,
        };
        // The landed workers stay resumable for their review's findings, kept before the
        // review starts so that its end always finds them, and for the session's full checks
        // (§11): a fix's landing for those alone, so a fix that breaks them again is sent back
        // to the same session.
        let mut held = std::collections::HashSet::new();
        for landed in &tasks {
            if crate::heap(|| self.hold_for_review(landed, target, fixed.is_some())).await {
                held.insert(landed.id.clone());
            }
        }
        for landed in &tasks {
            let updated = crate::heap(|| {
                self.update_task(&landed.conversation_id, &landed.id, |t| {
                    t.landed = Some(new_tip.0.clone());
                    t.landing = None;
                    t.landing_from = None;
                })
            })
            .await;
            // Its report enters the Brain now, at the commit that landed.
            if let Ok(updated) = &updated
                && let Some(report) = &updated.report
            {
                self.learn_report(updated, report, None);
            }
        }
        crate::heap(|| {
            self.orchestrator_step(
                &task.conversation_id,
                OrchestratorStepKind::Landed {
                    task_ids: tasks.iter().map(|t| t.id.clone()).collect(),
                    commits,
                    branch: target.to_owned(),
                    head: new_tip.0.clone(),
                },
            )
        })
        .await;
        let request = crate::heap(|| self.request_for(&task.conversation_id, Some(&task.id))).await;
        crate::heap(|| {
            self.record_decision(
                &task.conversation_id,
                request,
                DecisionSource::Task {
                    task_id: task.id.clone(),
                },
                DecisionKind::Routine,
                landed_line(task, target),
                format!("{} landed fast-forward.", commits_word(commits)),
            )
        })
        .await;
        for landed in &tasks {
            crate::heap(|| self.set_phase_stage(landed, PhaseStage::Done)).await;
        }
        // The session's full checks are due at the new tip: once, in the background, for
        // every landing close to it (SESSION-FIXES-PLAN.md §11).
        if let Ok(repo) = self.task_repo(task) {
            let request =
                crate::heap(|| self.request_for(&task.conversation_id, Some(&task.id))).await;
            crate::heap(|| self.checks_due(&task.conversation_id, &repo, target, new_tip, request))
                .await;
        }
        // The task branches are fully on the target now; they were Brigadier's, so they go
        // too. A merge's conflicting task whose work was rebuilt without its litter landed
        // as that rebuild: a branch at a rewritten commit counts as merged once its
        // replacement (or the one that replaced it in turn, by a later merge) is.
        let rebuilt: HashMap<Oid, Oid> = tasks
            .iter()
            .filter_map(|t| t.workspace.as_ref()?.merge.as_ref())
            .flat_map(|m| &m.rebuilt)
            .map(|r| (Oid(r.from.clone()), Oid(r.to.clone())))
            .collect();
        for landed in &tasks {
            let branch = landed.workspace.as_ref().and_then(|w| w.branch.clone());
            let rebuilt = rebuilt.clone();
            crate::heap(|| self.dispose_task(landed, TaskState::Landed)).await;
            // A held branch goes when its batch settles.
            let branch = branch.filter(|_| !held.contains(&landed.id));
            if let (Some(branch), Ok(repo)) = (branch, self.task_repo(landed)) {
                self.forget_branch(&repo, &branch);
                let git = self.git.clone();
                let into = target.to_owned();
                let deleted = blocking(move || {
                    let repo = git.open(&repo).map_err(git_error)?;
                    // `git branch -d` would compare with the main checkout's HEAD, which is
                    // not the target in a new-worktree session; the merge check here is the
                    // real one. Deleted only at the tip found merged: a commit added since
                    // then stays.
                    let Some(tip) = repo.branch_tip(&branch).map_err(git_error)? else {
                        return Ok(());
                    };
                    let landed = landed_as(&tip, &rebuilt);
                    if let Some(into) = repo.branch_tip(&into).map_err(git_error)?
                        && repo.ancestor(landed, &into).map_err(git_error)?
                    {
                        repo.delete_branch_at(&branch, &tip).map_err(git_error)?;
                    }
                    Ok(())
                })
                .await;
                if let Err(err) = deleted {
                    tracing::warn!(task = %landed.id, error = %err, "could not delete the landed task branch");
                }
            }
        }
        // Its one review, by the other vendor, starts now and runs on its own: only after the
        // landing is recorded and its worktrees are gone, since a review that ends at once
        // (one already done on the range, or a reviewer that can't start) settles the held
        // batch, which needs both.
        let manager = self.arc();
        let (task, from, tip) = (task.clone(), from.clone(), new_tip.clone());
        // Then a held merge looks again (SESSION-FIXES-PLAN.md Q3): before, no review of this
        // landing was running yet, and it would have come due without one.
        self.spawn(async move {
            #[cfg(test)]
            if let Some(conv) = review_due.as_ref().map(|due| due.0.clone()) {
                let pause = {
                    let mut pause = conv.review_pause.lock().unwrap();
                    match pause.as_mut() {
                        Some((0, ..)) => {
                            pause.take().map(|(_, reached, release)| (reached, release))
                        }
                        Some((skip, ..)) => {
                            *skip -= 1;
                            None
                        }
                        None => None,
                    }
                };
                if let Some((reached, release)) = pause {
                    reached.notify_one();
                    release.notified().await;
                }
            }
            match fixed {
                Some(subject) => manager.recheck_fix(&task, &subject, from, tip).await,
                None => manager.review_landing(&task, from, tip).await,
            }
            drop(review_due);
            manager.check_held_merge(&task.conversation_id).await;
            // A request that waited for this review to be stored may be over now.
            manager.settle_requests(&task.conversation_id).await;
        });
    }

    /// What a reviewer reads about the work it reviews: the task, what the worker was told
    /// since, the worker's report, and where its commits are.
    pub(crate) async fn review_brief(&self, subject: &Task, _scratch: &Path) -> String {
        let mut text = format!(
            "\n\nThe task it implements (task-{}):\n{}",
            subject.number, subject.spec
        );
        if !subject.messages.is_empty() {
            text.push_str(
                "\n\nWhat the thread told the worker after that, oldest first (it changes the task where it differs):",
            );
            for message in &subject.messages {
                text.push_str(&format!("\n---\n{message}"));
            }
        }
        // The worker's whole report, its risks, open questions and artifacts included
        // (BUILD-PLAN E2): what it claims is checked against the code, not taken on trust.
        if let Some(report) = &subject.report {
            text.push_str(&format!(
                "\n\nThe worker's report (check its claims in the code; don't take them on trust):\n{}",
                super::prompts::report_text(subject, report, "the worker")
            ));
        }
        if let Some(base) = subject.workspace.as_ref().and_then(|w| w.base.clone()) {
            text.push_str(&format!(
                "\n\nThe work starts at {}: see it with `git log --oneline {base}..HEAD` and `git diff {base}..HEAD` (uncommitted changes, if any, with `git status` and `git diff`).",
                short(&Oid(base.clone()))
            ));
        }
        text
    }

    /// What a merge worker reads: the task whose work it merges with the target, and how
    /// (SESSION-FIXES-PLAN.md Q2): it runs the merge with git and commits it itself.
    pub(crate) async fn merge_brief(&self, subject: &Task, workspace: &Workspace) -> String {
        let target = workspace.target.clone().unwrap_or_default();
        let number = subject.number;
        let reported = subject
            .report
            .as_ref()
            .filter(|r| !r.changes.is_empty())
            .map(|r| {
                format!(
                    " task-{number} reported changing: {}. List those and every file you edit in the report's `changes`.",
                    r.changes.join(", ")
                )
            })
            .unwrap_or_default();
        format!(
            "\n\nThis worktree's branch holds task-{number}'s work (\"{}\"), which conflicts with `{target}`. {}{reported}\n\nThe original task:\n{}",
            subject.title,
            merge_steps(&target, &format!("task-{number}'s")),
            subject.spec
        )
    }

    /// A merge task with a subject starts from the conflicting task's work, kept as a WIP
    /// commit: the target's tip (where its merge starts) and that work, without the files the
    /// landing's litter guard would leave out, so they reach neither the merge nor its history;
    /// and each commit that rebuild rewrote, with its replacement.
    pub(crate) async fn merge_start(
        &self,
        subject: &Task,
        target: &str,
    ) -> Result<(Oid, Oid, Vec<(Oid, Oid)>)> {
        let workspace = subject
            .workspace
            .clone()
            .ok_or_else(|| Error::Invalid(format!("task-{} has no workspace", subject.number)))?;
        if workspace.on_snapshot {
            return Err(Error::Invalid(snapshot_conflict(subject.number)));
        }
        let base = Oid(workspace
            .base
            .ok_or_else(|| Error::Invalid("no base".into()))?);
        let path = match workspace.worktree {
            Some(path) => PathBuf::from(path),
            None => {
                return Err(Error::Invalid(format!(
                    "task-{} has no worktree",
                    subject.number
                )));
            }
        };
        let reported: Vec<String> = subject
            .report
            .as_ref()
            .map(|r| r.changes.iter().flat_map(|p| reported_paths(p)).collect())
            .unwrap_or_default();
        let (git, target) = (self.git.clone(), target.to_owned());
        let wip = format!("WIP: task-{} before merging", subject.number);
        blocking(move || {
            let worktree = git.open_worktree(&path).map_err(git_error)?;
            let leave_out: Vec<String> =
                litter::classify(&worktree.changes(&base).map_err(git_error)?, &reported)
                    .into_iter()
                    .filter(|(_, verdict)| matches!(verdict, litter::Verdict::Exclude { .. }))
                    .flat_map(|(change, _)| match change.kind {
                        ChangeKind::Renamed { from } => vec![change.path, from],
                        _ => vec![change.path],
                    })
                    .collect();
            worktree.commit_wip(&wip).map_err(git_error)?;
            let work = worktree.head().map_err(git_error)?;
            // The subject's own checkout, so the commit uses the identity its WIP commit did.
            let repo = git.open(&path).map_err(git_error)?;
            let (without, rebuilt) = repo
                .work_without(&base, &work, &leave_out)
                .map_err(git_error)?;
            let onto = repo
                .branch_tip(&target)
                .map_err(git_error)?
                .ok_or_else(|| Error::Invalid(format!("branch {target} does not exist")))?;
            Ok((onto, without, rebuilt))
        })
        .await
    }

    /// A merge task with no subject starts from the session branch `target` and merges its
    /// base `base_branch` into it: their tips.
    pub(crate) async fn base_merge_start(
        &self,
        repo: &Path,
        target: &str,
        base_branch: &str,
    ) -> Result<(Oid, Oid)> {
        let (git, repo, target, base_branch) = (
            self.git.clone(),
            repo.to_owned(),
            target.to_owned(),
            base_branch.to_owned(),
        );
        blocking(move || {
            let repo = git.open(&repo).map_err(git_error)?;
            let tip = |branch: &str| {
                repo.branch_tip(branch)
                    .map_err(git_error)?
                    .ok_or_else(|| Error::Invalid(format!("branch {branch} does not exist")))
            };
            Ok((tip(&target)?, tip(&base_branch)?))
        })
        .await
    }

    /// What a merge worker with no subject reads: merge the session's base into its branch.
    pub(crate) async fn base_merge_brief(&self, task: &Task, workspace: &Workspace) -> String {
        let target = workspace.target.clone().unwrap_or_default();
        let base_branch = match self.core.conversation(&task.conversation_id) {
            Ok(conversation) => match conversation.setup {
                Some(Setup::Session {
                    environment: Environment::NewWorktree { base, .. },
                    ..
                }) => base,
                _ => String::new(),
            },
            Err(_) => String::new(),
        };
        format!(
            "\n\nThis worktree's branch starts at the session branch `{target}`, which conflicts with the current `{base_branch}`: merge `{base_branch}` into it, so the session can merge into `{base_branch}` cleanly. {} List every file you edit in the report's `changes`.",
            merge_steps(&base_branch, &format!("`{target}`'s"))
        )
    }

    /// Something stopped a landing: the task goes to `state` and the orchestrator hears why,
    /// and decides what happens next.
    pub(super) async fn landing_problem(&self, task: &Task, reason: &str, state: TaskState) {
        // The user is stopping it, or stopped it: its landing failing is no news.
        let stopped = self
            .task_by_id(&task.conversation_id, &task.id)
            .await
            .is_ok_and(|now| now.state == TaskState::Stopped);
        if stopped || self.is_stopping(&task.id) {
            tracing::debug!(task = %task.id, reason, "a stopped task's landing ended");
            return;
        }
        self.announcing(task).await;
        self.hand_back(task, state, Some(reason)).await;
        self.deliver(
            &task.conversation_id,
            Envelope {
                kind: InjectionKind::Decision,
                label: format!("landing task-{}", task.number),
                task_id: Some(task.id.clone()),
                text: format!(
                    "[not landed task-{} \"{}\"] {reason}",
                    task.number, task.title
                ),
            },
        )
        .await;
    }

    /// A landing ends without landing and the orchestrator decides next: the task goes to
    /// `state` (`blocked` is why, for a task ready to land) and Brigadier no longer lands it
    /// on its own.
    async fn hand_back(&self, task: &Task, state: TaskState, blocked: Option<&str>) {
        // A task stopped meanwhile, or being stopped, stays stopped.
        let _ = self
            .update_task_if(
                &task.conversation_id,
                &task.id,
                |now| !now.state.is_final() && !self.is_stopping(&now.id),
                |t| {
                    t.state = state;
                    t.blocked_reason = blocked
                        .filter(|_| state == TaskState::ReadyToLand)
                        .map(str::to_owned);
                    t.landing = None;
                },
            )
            .await;
    }

    /// The app's explicit Merge of this run's verified SHA, independent of session setup.
    /// Workers have no IPC/MCP route to this action. The base must still be the reviewed base
    /// (or this lineage's last Merge), and git rechecks its tip and checkout under its lock.
    pub async fn merge_overnight(
        &self,
        id: ConversationId,
        run_id: crate::model::OvernightRunId,
        command_id: String,
        verified_commit: String,
    ) -> Result<crate::overnight::OvernightRun> {
        use crate::overnight::{AppliedCommand, OvernightState, RunMerge};
        let _held = self.overnight.changes.lock().await;
        let board = self.core.board(&id).await?;
        let mut run = board
            .runs
            .get(&run_id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("overnight run {run_id}")))?;
        if run.commands.iter().any(|command| command.id == command_id) {
            return Ok(run);
        }
        if run.state != OvernightState::Finished {
            return Err(Error::Invalid(
                "This run must finish before its verified work can merge.".into(),
            ));
        }
        if run.verified_commit.as_deref() != Some(&verified_commit) {
            return Err(Error::Invalid(
                "The verified tip changed; look again before merging.".into(),
            ));
        }
        let workspace = run
            .workspace
            .clone()
            .ok_or_else(|| Error::Invalid("This run has no branch to merge.".into()))?;
        if verified_commit == workspace.base_commit {
            return Err(Error::Invalid("Nothing new is verified to merge.".into()));
        }
        // Verified means its full checks passed there too (SESSION-FIXES-PLAN.md §11).
        if super::integrated_checks::tip_checked(&board, &workspace.branch, &verified_commit)
            != super::integrated_checks::TipChecked::Passed
        {
            return Err(Error::Invalid(
                "The run's full checks haven't passed at its verified tip; nothing merged.".into(),
            ));
        }
        let lineage: Vec<_> = board
            .runs
            .values()
            .filter(|other| {
                other
                    .workspace
                    .as_ref()
                    .is_some_and(|w| w.branch == workspace.branch)
            })
            .collect();
        if lineage.iter().any(|other| other.state.is_active()) {
            return Err(Error::Invalid(
                "A continuation is still running on this branch; let it finish first.".into(),
            ));
        }
        let open: Vec<_> = board
            .tasks
            .values()
            .filter(|task| {
                task.kind.writes()
                    && !task.state.is_final()
                    && task.run.as_ref().is_some_and(|context| {
                        lineage.iter().any(|other| other.id == context.run_id)
                    })
            })
            .map(|task| format!("task-{}", task.number))
            .collect();
        if !open.is_empty() {
            return Err(Error::Invalid(format!(
                "This run's write tasks are still open: {}. Land or stop them first.",
                open.join(", ")
            )));
        }
        let expected_base = lineage
            .iter()
            .filter_map(|other| other.merged.as_ref())
            .max_by_key(|merged| merged.at_ms)
            .map_or_else(
                || workspace.base_commit.clone(),
                |merged| merged.commit.clone(),
            );
        let Some(Setup::Session { repo, .. }) = self.core.conversation(&id)?.setup else {
            return Err(Error::Invalid("This run has no repository.".into()));
        };
        // What the thread committed itself on the run branch gets its review before it merges.
        self.scan_thread_branch(
            &id,
            Path::new(&repo),
            &workspace.branch,
            self.thread_turn_running(&id).await,
        )
        .await;
        let git = self.git.clone();
        let approved = verified_commit.clone();
        let into = workspace.base.clone();
        let landed = blocking(move || {
            let repo = git.open(Path::new(&repo)).map_err(git_error)?;
            let tip = Oid(approved);
            let branch_tip = repo.branch_tip(&workspace.branch).map_err(git_error)?
                .ok_or_else(|| Error::Invalid("The run branch is gone.".into()))?;
            if !repo.ancestor(&tip, &branch_tip).map_err(git_error)? {
                return Err(Error::Invalid("The verified commit is no longer on the run branch.".into()));
            }
            let base_tip = repo.branch_tip(&workspace.base).map_err(git_error)?
                .ok_or_else(|| Error::Invalid("The base branch is gone.".into()))?;
            // Reconcile a crash after git landed but before the run recorded it.
            if base_tip == tip || (base_tip.0 == expected_base && repo.ancestor(&tip, &base_tip).map_err(git_error)?) {
                return Ok(base_tip);
            }
            if base_tip.0 != expected_base
                && !repo.ancestor(&base_tip, &tip).map_err(git_error)?
            {
                return Err(Error::Invalid(format!("`{}` moved since this work was verified. Nothing merged. Once the run has ended, the session can bring the new `{}` in with a regular merge of it into the session branch, if the user asks.", workspace.base, workspace.base)));
            }
            // A base already included in this phase-verified candidate is safe. Otherwise
            // its change must be verified again before it merges.
            let approved_base_tip = base_tip;
            let message = format!("Merge verified overnight work into {}", workspace.base);
            let (commit, base_tip) = match repo.prepare_merge_commit(&workspace.base, &tip, &message).map_err(git_error)? {
                MergeOutcome::Ready { commit, base_tip, .. } => (commit, base_tip),
                MergeOutcome::Conflicts { paths } => return Err(Error::Invalid(format!("The run's verified work conflicts with `{}` in: {}. Nothing merged. Once the run has ended, the session can resolve it with a regular merge of `{}` into the session branch, if the user asks.", workspace.base, paths.join(", "), workspace.base))),
            };
            if base_tip != approved_base_tip {
                return Err(Error::Invalid("The base moved while preparing the merge; look again. Nothing merged.".into()));
            }
            match repo.land(&LandRequest { branch: workspace.base, expected_tip: base_tip, commit }).map_err(git_error)? {
                LandOutcome::Landed { new_tip } => Ok(new_tip),
                LandOutcome::Blocked(block) => Err(Error::Invalid(format!("Nothing merged: {block}"))),
            }
        }).await?;
        run.merged = Some(RunMerge {
            verified_commit,
            commit: landed.0,
            into,
            at_ms: crate::now_ms(),
        });
        run.commands.push(AppliedCommand {
            id: command_id,
            at_ms: crate::now_ms(),
        });
        self.record_runs(
            &id,
            vec![crate::model::DomainEvent::OvernightUpdated {
                run: Box::new(run.clone()),
            }],
        )
        .await?;
        Ok(run)
    }

    /// `finish_session` with `when` "now": see [`Self::finish_session_when`].
    #[cfg(all(test, unix))]
    pub(crate) async fn finish_session(
        &self,
        id: &ConversationId,
        named: &str,
        message: Option<String>,
    ) -> Result<String> {
        self.finish_session_when(id, named, message, crate::tools::MergeWhen::Now)
            .await
    }

    /// `finish_session`: merges the session branch into its base once the user consented: their
    /// click on "Merge into main", their card answer, or their message (`consent`, its id or their words;
    /// [`merge_consent`]). `when`: now, what has landed, or held until the workers writing now
    /// land (#243). What the user wrote since, work that landed since their consent and work
    /// still running are facts its result states, for the thread to judge.
    pub(crate) async fn finish_session_when(
        &self,
        id: &ConversationId,
        named: &str,
        message: Option<String>,
        when: crate::tools::MergeWhen,
    ) -> Result<String> {
        if self.overnight.active.get(id).is_some() {
            return Err(Error::Invalid(
                "An overnight run is going in this session: nothing merges into the base until it ends, and then only its verified work, by the user's Merge.".into(),
            ));
        }
        let conversation = self.core.conversation(id)?;
        let Some(Setup::Session {
            repo,
            environment: Environment::NewWorktree { base, branch, .. },
            ..
        }) = conversation.setup
        else {
            return Err(Error::Invalid(
                "this session works on a local checkout: its commits are already on the picked branch".into(),
            ));
        };
        let conv = self.conv(id)?;
        // The landing lock, held to the move of the base: a Cancel either comes first and
        // spends the consent, or finds it merged.
        let landing_lock = conv.merge_lock.lock().await;
        // Merging on its own (the user's setting, Q62): the thread's judgement is the go-ahead,
        // and no consent is asked. A consent the user gave still counts as theirs; a merge held
        // on an auto go-ahead stays an auto merge.
        let consent = match self.merge_consent(id, named, &branch, &base).await {
            // A merge held on an auto go-ahead after the user turned merging on its own off.
            Ok(consent)
                if consent.asked_in.starts_with(AUTO_GO_AHEAD) && !self.auto_merge_in(id) =>
            {
                return Err(not_consented(
                    "merging on its own was turned off since this merge was held, so it needs the user's consent now".into(),
                ));
            }
            Ok(consent) => consent,
            Err(_) if self.auto_merge_in(id) => Consent {
                asked_in: format!("{AUTO_GO_AHEAD}{}", crate::now_ms()),
                at_ms: crate::now_ms(),
            },
            Err(err) => return Err(err),
        };
        let auto = consent.asked_in.starts_with(AUTO_GO_AHEAD);
        // What the thread committed itself gets its review before the branch is merged.
        self.scan_thread_commits(id, self.thread_turn_running(id).await)
            .await;
        let tasks = self.core.tasks(id).await?;
        let writing: Vec<&Task> = tasks
            .iter()
            .filter(|t| t.kind.writes() && !t.state.is_final())
            .collect();
        let board = self.core.board(id).await?;
        let pending = pending_merge(&board).filter(|held| held.asked_in == consent.asked_in);
        // The thread asked to merge once the workers writing now finish (SESSION-FIXES-PLAN.md
        // Q3): held, stored, so it survives a restart. Otherwise it merges what has landed.
        if when == crate::tools::MergeWhen::WorkersFinish && !writing.is_empty() {
            if pending.is_none() {
                let (git, repo_path, branch_name) =
                    (self.git.clone(), PathBuf::from(&repo), branch.clone());
                let tip = blocking(move || {
                    git.open(&repo_path)
                        .map_err(git_error)?
                        .branch_tip(&branch_name)
                        .map_err(git_error)?
                        .ok_or_else(|| Error::Invalid("the session branch is gone".into()))
                })
                .await?;
                self.orchestrator_step(
                    id,
                    OrchestratorStepKind::MergeHeld {
                        branch: branch.clone(),
                        base: base.clone(),
                        asked_in: consent.asked_in.clone(),
                        consented_at_ms: consent.at_ms,
                        condition: crate::work::MergeCondition::WorkersFinish,
                        waiting: writing.iter().map(|t| t.id.clone()).collect(),
                        tip: tip.0,
                    },
                )
                .await;
            }
            drop(landing_lock);
            let names: Vec<String> = writing
                .iter()
                .map(|t| format!("task-{}", t.number))
                .collect();
            return Ok(format!(
                "[merge held] Merging after {} worker{} finish ({}). Nothing is merged yet, and the user's consent holds: Brigadier tells you [merge due] once they have landed, or [merge stopped] if one fails. Tell the user in one line, then reply {} while they work.",
                writing.len(),
                if writing.len() == 1 { "" } else { "s" },
                names.join(", "),
                super::prompts::QUIET,
            ));
        }
        // Review findings and a review still running inform the merge; its result lists them
        // (#242).
        if let Some(held) = &pending
            && let Gate::Stop(reason) = held_gate(held, &tasks)
        {
            self.stop_held_merge(id, &conv, held, &reason).await;
            return Err(Error::Invalid(stopped_text(&reason)));
        }
        let message = self.commit_message(
            message
                .filter(|m| !m.trim().is_empty())
                .unwrap_or_else(|| format!("Merge {branch} into {base}")),
        );
        // Workers that still write: their work isn't in this merge.
        let running = if writing.is_empty() {
            String::new()
        } else {
            let names: Vec<String> = writing
                .iter()
                .map(|t| format!("task-{} \"{}\"", t.number, t.title))
                .collect();
            format!(
                " Still running, so not in this merge: {}.",
                names.join(", ")
            )
        };
        // The base as its upstream has it (#224): the merge, and any conflicts, are against
        // the main others pushed to, not a stale copy of it.
        let fetch_failed = self.sync_base(&repo, &base).await;
        enum Prepared {
            /// The merge commit, the base's tip, the branch's, its commit count, and whether
            /// work landed on the branch since the held consent.
            Ready(Oid, Oid, Oid, u32, bool),
            Conflicts(Vec<String>, Oid),
        }
        let (git, repo_path, base_name, branch_name, covered) = (
            self.git.clone(),
            PathBuf::from(&repo),
            base.clone(),
            branch.clone(),
            pending.as_ref().map(|held| held.tip.clone()),
        );
        // A held consent covers the branch as it was then, moved on by the session's own
        // landings since; other work since is a fact the result states (#243).
        let prepared = blocking(move || {
            let repo = git.open(&repo_path).map_err(git_error)?;
            // A rebase the user is resolving in the Source panel: the branch is half rebased.
            if repo
                .rebasing_checkout(&branch_name)
                .map_err(git_error)?
                .is_some()
            {
                return Err(Error::Invalid(format!(
                    "[not merged] The user is rebasing `{branch_name}` in the Source panel and hasn't finished. Nothing was merged, and the user's consent still holds; call finish_session again once the rebase is continued to its end or aborted."
                )));
            }
            let tip = repo
                .branch_tip(&branch_name)
                .map_err(git_error)?
                .ok_or_else(|| Error::Invalid("the session branch is gone".into()))?;
            let moved = covered.is_some_and(|covered| covered != tip.0);
            let outcome = repo
                .prepare_merge_commit(&base_name, &tip, &message)
                .map_err(git_error)?;
            match outcome {
                MergeOutcome::Ready {
                    commit, base_tip, ..
                } => {
                    let commits = repo.count_commits(&base_tip, &tip).map_err(git_error)?;
                    Ok(Prepared::Ready(commit, base_tip, tip, commits, moved))
                }
                MergeOutcome::Conflicts { paths } => Ok(Prepared::Conflicts(paths, tip)),
            }
        })
        .await?;
        #[cfg(test)]
        {
            let pause = conv.merge_pause.lock().unwrap().clone();
            if let Some((reached, release)) = pause {
                reached.notify_one();
                release.notified().await;
            }
        }
        let (commit, base_tip, session_tip, commits, moved) = match prepared {
            Prepared::Ready(commit, base_tip, tip, commits, moved) => {
                (commit, base_tip, tip, commits, moved)
            }
            Prepared::Conflicts(paths, tip) => {
                let upstream = self.base_upstream(&repo, &base).await;
                // The merge used no consent: it is held, stored, until the merge task that
                // merges the base into the branch lands.
                if pending.is_none() {
                    self.orchestrator_step(
                        id,
                        OrchestratorStepKind::MergeHeld {
                            branch: branch.clone(),
                            base: base.clone(),
                            asked_in: consent.asked_in,
                            consented_at_ms: consent.at_ms,
                            condition: crate::work::MergeCondition::Now,
                            waiting: Vec::new(),
                            tip: tip.0,
                        },
                    )
                    .await;
                }
                return Err(Error::Invalid(format!(
                    "`{branch}` conflicts with the current `{base}` in: {}.{} Nothing was merged, and the user's consent to this merge still holds. Resolve it as you judge: merge `{base}` into `{branch}` with git yourself, or delegate a merge task without `subject` for its worker to (Brigadier tells you [merge due] once it lands). Then call finish_session again on that consent; if the user takes it back meanwhile, call stop_merge.",
                    paths.join(", "),
                    diverged_line(&base, upstream.as_ref(), fetch_failed.as_deref()),
                )));
            }
        };
        if commits == 0 {
            return Err(Error::Invalid(format!(
                "`{branch}` has no commits that `{base}` lacks"
            )));
        }
        // What the user wrote while the merge was prepared is a fact its result states (#243):
        // a write under way finishes first, and one sent from now on comes after the merge.
        let writes = conv.user_wrote.lock().await;
        let landing = {
            let (git, repo_path, session_branch) =
                (self.git.clone(), PathBuf::from(&repo), branch.clone());
            let request = LandRequest {
                branch: base.clone(),
                expected_tip: base_tip,
                commit,
            };
            // What the user asked for is the session branch as it was: work that landed on it
            // meanwhile would be left out.
            let expected = session_tip.clone();
            blocking(move || {
                let repo = git.open(&repo_path).map_err(git_error)?;
                if repo.branch_tip(&session_branch).map_err(git_error)? != Some(expected) {
                    return Ok(None);
                }
                repo.land(&request).map(Some).map_err(git_error)
            })
            .await
        };
        let new_tip = match landing {
            Ok(Some(LandOutcome::Landed { new_tip })) => {
                let since = self.wrote_since_consent(id, &consent).await;
                drop(writes);
                (new_tip, since)
            }
            Ok(None) => {
                return Err(Error::Invalid(format!(
                    "[not merged] `{branch}` changed while the merge was being prepared. Nothing was merged; call finish_session again."
                )));
            }
            Ok(Some(LandOutcome::Blocked(block))) => {
                return Err(Error::Invalid(format!(
                    "[not merged] Merging `{branch}` into `{base}` is not safe now: {block} Nothing was changed; call finish_session again."
                )));
            }
            Err(err) => {
                return Err(Error::Invalid(format!(
                    "[not merged] Merging failed: {err}. Nothing was changed."
                )));
            }
        };
        let reviews = match self.core.board(id).await {
            Ok(board) => {
                // A merge card the user left open (they asked in words instead) has nothing
                // left to ask: it goes, so its request doesn't wait on it.
                let open: Vec<_> = board
                    .questions
                    .values()
                    .filter(|question| {
                        question.is_open() && matches!(&question.kind, QuestionKind::Merge { .. })
                    })
                    .cloned()
                    .collect();
                for question in &open {
                    self.withdraw_question(question).await;
                }
                if !open.is_empty() {
                    self.settle_requests(id).await;
                }
                merged_findings(
                    &board,
                    conv.reviews_due.load(std::sync::atomic::Ordering::SeqCst) > 0,
                )
            }
            Err(_) => String::new(),
        };
        let (new_tip, since) = new_tip;
        let upstream = self.base_upstream(&repo, &base).await;
        let pushed = pushed_line(&base, upstream.as_ref(), fetch_failed.as_deref());
        let checks = self.checks_line(id, &branch, &session_tip).await;
        let moved = if moved {
            format!(
                " Work landed on `{branch}` after the user's consent, and it is in this merge too."
            )
        } else {
            String::new()
        };
        let requests = board.merging_requests(None);
        // The turn that merged has its work so far in the merge too.
        let mut reviewed = requests.clone();
        if let Some(current) = self.request_for(id, None).await
            && !reviewed.contains(&current)
        {
            reviewed.push(current);
        }
        let record = self
            .merge_record(id, &repo, &branch, &session_tip.0, auto)
            .await;
        self.orchestrator_step(
            id,
            OrchestratorStepKind::Merged {
                branch: branch.clone(),
                base: base.clone(),
                commits,
                asked_in: Some(consent.asked_in),
                upstream,
                head: Some(session_tip.0.clone()),
                merge_commit: Some(new_tip.0.clone()),
                pr: None,
                auto,
                requests,
                record,
            },
        )
        .await;
        self.merged_pending(id, &reviewed, &session_tip.0).await;
        self.tell_merged(id, &base, &new_tip.0).await;
        *conv.merge_told.lock().unwrap() = None;
        drop(landing_lock);
        let previews = self.stop_merged_previews(id).await;
        // The session's work is merged. Its worktree and branch stay with the session (BUILD-PLAN
        // A3): the branch moves on to the merge, and the next change starts from there.
        // The landed workers kept for their reviews' findings are done with, and the checkout
        // its full checks ran in.
        self.settle_all_holds(id).await;
        self.drop_check_checkout(id).await;
        let released = self
            .follow_merge(&repo, &branch, &base, session_tip, new_tip.clone())
            .await;
        Ok(format!(
            "[finished] {}: `{branch}` ({commits} commit{}) is merged into `{base}` at {}.{pushed}{running}{moved}{checks}{reviews}{released}{previews}{since}",
            if auto {
                "On its own, as the user's setting has it"
            } else {
                "As the user asked"
            },
            if commits == 1 { "" } else { "s" },
            short(&new_tip)
        ))
    }

    /// Brings the base up to date with its upstream before a merge (#224): fetches its remote
    /// (git is stopped after [`FETCH_PATIENCE`]) and fast-forwards local `base` when it is only
    /// behind. A checkout of it that is busy, or has changes the fast-forward would touch,
    /// keeps it as it is: the merge result says how far it is behind. Returns why the fetch
    /// failed.
    pub(super) async fn sync_base(&self, repo: &str, base: &str) -> Option<String> {
        let (git, path, branch) = (self.git.clone(), PathBuf::from(repo), base.to_owned());
        let failed = blocking(move || {
            git.open(&path)
                .map_err(git_error)?
                .fetch_remote_of(&branch, FETCH_PATIENCE)
                .map_err(git_error)
        })
        .await
        .err()
        .map(|err| err.to_string());
        let (git, path, branch) = (self.git.clone(), PathBuf::from(repo), base.to_owned());
        let forwarded = blocking(move || {
            let repo = git.open(&path).map_err(git_error)?;
            let Some(gap) = repo.upstream_gap(&branch).map_err(git_error)? else {
                return Ok(());
            };
            if gap.ahead > 0 || gap.behind == 0 {
                return Ok(());
            }
            let Some(tip) = repo.branch_tip(&branch).map_err(git_error)? else {
                return Ok(());
            };
            repo.land(&LandRequest {
                branch,
                expected_tip: tip,
                commit: gap.tip,
            })
            .map(drop)
            .map_err(git_error)
        })
        .await;
        if let Err(err) = forwarded {
            tracing::warn!(%err, "the base couldn't be brought up to date with its upstream");
        }
        failed
    }

    /// The base against its upstream as last fetched; none without one.
    async fn base_upstream(&self, repo: &str, base: &str) -> Option<MergedUpstream> {
        let (git, path, branch) = (self.git.clone(), PathBuf::from(repo), base.to_owned());
        let gap = blocking(move || {
            git.open(&path)
                .map_err(git_error)?
                .upstream_gap(&branch)
                .map_err(git_error)
        })
        .await
        .ok()
        .flatten()?;
        Some(MergedUpstream {
            name: gap.name,
            behind: gap.behind,
        })
    }

    /// Once no write task is left, the held merge is stopped, waits for findings to be
    /// recorded or a review to end, or is due; the thread hears which, once each
    /// (SESSION-FIXES-PLAN.md Q3 step 3). The merge stays the thread's call: it is woken, not
    /// merged for. Called when a write task ends, a review ends and findings are recorded.
    pub(crate) async fn check_held_merge(&self, id: &ConversationId) {
        let Ok(conv) = self.conv(id) else {
            return;
        };
        let text = {
            let _landing = conv.merge_lock.lock().await;
            let (Ok(board), Ok(tasks)) = (self.core.board(id).await, self.core.tasks(id).await)
            else {
                return;
            };
            let Some(held) = pending_merge(&board) else {
                return;
            };
            if tasks.iter().any(|t| t.kind.writes() && !t.state.is_final()) {
                return;
            }
            let told = |kind: &'static str| {
                let mut told = conv.merge_told.lock().unwrap();
                let again = told.as_ref() == Some(&(held.asked_in.clone(), kind));
                *told = Some((held.asked_in.clone(), kind));
                again
            };
            match held_gate(&held, &tasks) {
                Gate::Stop(reason) => {
                    self.stop_held_merge(id, &conv, &held, &reason).await;
                    stopped_text(&reason)
                }
                // A message the user wrote since waits to be read: it decides first, and the
                // thread's turn on it looks again when it ends.
                Gate::Due if wrote_since(&board, held.consented_at_ms) => return,
                Gate::Due => {
                    if told("due") {
                        return;
                    }
                    "[merge due] The workers have finished and nothing stops it: the user's merge is due. Call finish_session now; their held consent covers it, so don't ask them again.".to_owned()
                }
            }
        };
        self.deliver(
            id,
            Envelope {
                kind: InjectionKind::Decision,
                label: "merge".into(),
                task_id: None,
                text,
            },
        )
        .await;
    }

    /// Ends the held merge without merging: its consent is spent.
    async fn stop_held_merge(
        &self,
        id: &ConversationId,
        conv: &super::conversation::ConvLive,
        held: &PendingMerge,
        reason: &str,
    ) {
        self.orchestrator_step(
            id,
            OrchestratorStepKind::MergeStopped {
                asked_in: held.asked_in.clone(),
                reason: reason.to_owned(),
            },
        )
        .await;
        *conv.merge_told.lock().unwrap() = None;
    }

    /// The user's Stop ends the held merge with what it stopped: nothing merges on its own
    /// afterwards, and the thread isn't woken for it. Its consent is spent, so a resumed
    /// request asks again.
    pub(super) async fn stop_merge_for_stop(&self, id: &ConversationId) {
        let Ok(conv) = self.conv(id) else {
            return;
        };
        let _landing = conv.merge_lock.lock().await;
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        if let Some(held) = pending_merge(&board) {
            self.stop_held_merge(id, &conv, &held, "The user pressed Stop.")
                .await;
        }
    }

    /// The Cancel on the held merge's pill (SESSION-FIXES-PLAN.md Q3 step 5): under the
    /// landing lock, a stored `MergeCancelled` spends the consent, so neither the answered
    /// card nor the message counts again, after a restart too; the thread hears it. A merge
    /// that already went answers "Too late".
    pub async fn cancel_merge(&self, id: &ConversationId) -> Result<String> {
        let conv = self.conv(id)?;
        {
            let _landing = conv.merge_lock.lock().await;
            let board = self.core.board(id).await?;
            let Some(held) = pending_merge(&board) else {
                let merged = board
                    .orchestrator_steps
                    .iter()
                    .rev()
                    .find_map(|step| match &step.kind {
                        OrchestratorStepKind::MergeHeld { asked_in, .. } => Some(asked_in),
                        _ => None,
                    })
                    .is_some_and(|asked| {
                        board.orchestrator_steps.iter().any(|step| {
                            matches!(&step.kind, OrchestratorStepKind::Merged { asked_in: Some(done), .. } if done == asked)
                        })
                    });
                return if merged {
                    Ok("Too late: already merged.".into())
                } else {
                    Err(Error::Invalid("No merge is waiting.".into()))
                };
            };
            self.orchestrator_step(
                id,
                OrchestratorStepKind::MergeCancelled {
                    asked_in: held.asked_in,
                },
            )
            .await;
            *conv.merge_told.lock().unwrap() = None;
        }
        self.deliver(
            id,
            Envelope {
                kind: InjectionKind::Decision,
                label: "merge cancelled".into(),
                task_id: None,
                text: "[merge cancelled by the user] The user cancelled the merge they had asked for. Don't merge: their consent is spent, and only their new words give it again. Reply in one line at most.".into(),
            },
        )
        .await;
        Ok("Merge cancelled.".into())
    }

    /// `stop_merge`: the thread read the user's words since their consent as taking it back
    /// or putting it off (#222). The held merge stops, its consent spent, and the thread tells
    /// the user in its one answer.
    pub(crate) async fn stop_merge(&self, id: &ConversationId, why: &str) -> Result<String> {
        let conv = self.conv(id)?;
        let _landing = conv.merge_lock.lock().await;
        let board = self.core.board(id).await?;
        let Some(held) = pending_merge(&board) else {
            return Err(Error::Invalid("No merge is waiting.".into()));
        };
        let why = why.trim().trim_end_matches('.');
        let reason = if why.is_empty() {
            "The user took it back.".to_owned()
        } else {
            format!("The user took it back: {why}.")
        };
        self.stop_held_merge(id, &conv, &held, &reason).await;
        Ok(
            "Stopped: nothing merges until the user asks again. Say so in your answer to them."
                .into(),
        )
    }

    /// Merge cards are retired (BUILD-PLAN A4). After an upgrade, a session's open merge card
    /// is withdrawn and the thread hears of it in one line. A held merge stays, whether its
    /// consent came from the user's words or a card they answered (#240).
    pub(super) async fn retire_merge_cards(&self, id: &ConversationId) {
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        let open: Vec<_> = board
            .questions
            .values()
            .filter(|question| {
                matches!(question.kind, QuestionKind::Merge { .. }) && question.is_open()
            })
            .cloned()
            .collect();
        if open.is_empty() {
            return;
        }
        for question in &open {
            self.withdraw_question(question).await;
        }
        self.queue_envelope(
            id,
            Envelope {
                kind: InjectionKind::Decision,
                label: "merge cards retired".into(),
                task_id: None,
                text: "[merge cards retired] Brigadier no longer asks about merges on cards, so the open merge card is closed. Merge when the user's words ask for it.".into(),
            },
            None,
        )
        .await;
    }

    /// After a merge, the session branch fast-forwards to the merge commit in its worktree, so
    /// the session goes on from what is on the base, with its build caches. Returns what the
    /// thread is told about it.
    async fn follow_merge(
        &self,
        repo: &str,
        branch: &str,
        base: &str,
        session_tip: Oid,
        merged: Oid,
    ) -> String {
        let (git, repo_path, request) = (
            self.git.clone(),
            PathBuf::from(repo),
            LandRequest {
                branch: branch.to_owned(),
                expected_tip: session_tip,
                commit: merged,
            },
        );
        let moved = blocking(move || {
            git.open(&repo_path)
                .map_err(git_error)?
                .land(&request)
                .map_err(git_error)
        })
        .await;
        match moved {
            Ok(LandOutcome::Landed { .. }) => format!(
                " The session keeps its worktree and branch: `{branch}` is at the merge now, so later work goes on from what is on `{base}`."
            ),
            Ok(LandOutcome::Blocked(block)) => format!(
                " The session keeps its worktree and branch; `{branch}` stays where it was ({block}), and its work is on `{base}` all the same."
            ),
            Err(err) => format!(
                " The session keeps its worktree and branch; `{branch}` couldn't move to the merge ({err}), and its work is on `{base}` all the same."
            ),
        }
    }

    /// The user clicked "Merge into main" (BUILD-PLAN A4): the click is recorded with where it
    /// came from, and the thread is told to merge. Consent holds once, until the user writes
    /// again; behind running writers it waits as a held merge does.
    pub async fn merge_session(&self, id: &ConversationId, origin: AnswerOrigin) -> Result<String> {
        if self.overnight.active.get(id).is_some() {
            return Err(Error::Invalid(
                "An overnight run is going in this session: its verified work merges by Merge on the run's card.".into(),
            ));
        }
        let Some(Setup::Session {
            repo,
            environment: Environment::NewWorktree { base, branch, .. },
            ..
        }) = self.core.conversation(id)?.setup
        else {
            return Err(Error::Invalid(
                "This session works in your checkout: its commits are already on the branch."
                    .into(),
            ));
        };
        let (git, repo_path, base_name, branch_name) = (
            self.git.clone(),
            PathBuf::from(&repo),
            base.clone(),
            branch.clone(),
        );
        let ahead = blocking(move || {
            let repo = git.open(&repo_path).map_err(git_error)?;
            let (Some(tip), Some(base_tip)) = (
                repo.branch_tip(&branch_name).map_err(git_error)?,
                repo.branch_tip(&base_name).map_err(git_error)?,
            ) else {
                return Ok(0);
            };
            repo.count_commits(&base_tip, &tip).map_err(git_error)
        })
        .await?;
        if ahead == 0 {
            return Err(Error::Invalid(format!(
                "Nothing to merge: `{branch}` has no commits that `{base}` lacks."
            )));
        }
        let asked = format!("merge-click:{}", crate::model::CardId::generate());
        self.orchestrator_step(
            id,
            OrchestratorStepKind::MergeRequested {
                id: asked,
                branch: branch.clone(),
                base: base.clone(),
                origin,
            },
        )
        .await;
        self.deliver(
            id,
            Envelope {
                kind: InjectionKind::Decision,
                label: "merge".into(),
                task_id: None,
                text: format!(
                    "[merge] The user clicked \"Merge into main\" for `{branch}` into `{base}`. Call finish_session now, without consent."
                ),
            },
        )
        .await;
        Ok(format!("Merging `{branch}` into `{base}`."))
    }

    /// The user's consent to the session's merge: given now ([`Self::fresh_consent`]), or held
    /// (while workers wrote, or a merge stopped at conflicts with the base), while the user
    /// took nothing back since they gave it ([`Self::held_consent`]).
    async fn merge_consent(
        &self,
        id: &ConversationId,
        consent: &str,
        branch: &str,
        base: &str,
    ) -> Result<Consent> {
        let fresh = self.fresh_consent(id, consent, branch, base).await;
        let held = pending_merge(&self.core.board(id).await?)
            .filter(|held| held.branch == branch && held.base == base);
        // A new yes replaces the held one. Reusing the same yes still checks what has
        // happened since the conflict, even when it remains the latest user message.
        if let Ok(consent) = &fresh
            && held
                .as_ref()
                .is_none_or(|held| held.asked_in != consent.asked_in)
        {
            return fresh;
        }
        let refused = fresh.err().unwrap_or_else(|| {
            not_consented("the original merge consent no longer covers this branch".into())
        });
        match held {
            Some(held) => self.held_consent(id, held, refused).await,
            None => Err(refused),
        }
    }

    /// Consent `held` (a pending merge), unless it was used, cancelled or stopped (the thread
    /// stops it with stop_merge when the user's words since take it back). `refused` is why
    /// fresh consent was refused.
    async fn held_consent(
        &self,
        id: &ConversationId,
        held: PendingMerge,
        refused: Error,
    ) -> Result<Consent> {
        let board = self.core.board(id).await?;
        if pending_merge(&board).is_none_or(|now| now.asked_in != held.asked_in) {
            return Err(refused);
        }
        Ok(Consent {
            asked_in: held.asked_in,
            at_ms: held.consented_at_ms,
        })
    }

    /// What the user wrote after the consent a merge went on, as a fact for the thread (#240):
    /// it judges whether they meant the merge to wait, and tells them.
    async fn wrote_since_consent(&self, id: &ConversationId, consent: &Consent) -> String {
        let Ok(messages) = self.core.all_messages(id).await else {
            return String::new();
        };
        let quoted = |text: &str| {
            let text = text.trim();
            match text.char_indices().nth(300) {
                Some((at, _)) => format!("\"{}…\"", &text[..at]),
                None => format!("\"{text}\""),
            }
        };
        // A message still in the queue, unread (#243).
        let queued: Vec<String> = match self.core.board(id).await {
            Ok(board) => board
                .queue
                .items
                .iter()
                .filter(|item| {
                    item.queued_at_ms.max(item.edited_at_ms.unwrap_or(0)) >= consent.at_ms
                })
                .map(|item| quoted(&item.text))
                .collect(),
            Err(_) => Vec::new(),
        };
        let queued = if queued.is_empty() {
            String::new()
        } else {
            format!(
                " Still waiting for you to read, sent after the consent: {}. If it meant the merge should wait, tell them at once.",
                queued.join(", ")
            )
        };
        let said: Vec<String> = messages
            .iter()
            .filter(|message| {
                message.role == MessageRole::User
                    && message.id != consent.asked_in
                    && message.created_at_ms > consent.at_ms
            })
            .map(|message| quoted(&message.text))
            .collect();
        if said.is_empty() {
            return queued;
        }
        format!(
            " The user wrote after the consent it went on: {}. If that meant it should wait, tell them at once.{queued}",
            said.join(", ")
        )
    }

    /// What gives consent to the session's merge, as the thread names it (`consent`, #240):
    /// the id of the user's message, card answer or "Merge into main" click it acts on, or the
    /// user's words quoted from their message. Empty: their click since their latest message.
    /// The thread judges whether the user consents; this only checks that it is the user's and
    /// that no earlier merge used it.
    async fn fresh_consent(
        &self,
        id: &ConversationId,
        consent: &str,
        branch: &str,
        base: &str,
    ) -> Result<Consent> {
        let refuse = not_consented;
        let consent = consent.trim();
        let board = self.core.board(id).await?;
        let messages = match &board.head {
            Some((head, _)) => self.core.branch(id, head).await?,
            None => self.core.all_messages(id).await?,
        };
        let latest = messages
            .iter()
            .rfind(|message| message.role == MessageRole::User);
        let clicks = board
            .orchestrator_steps
            .iter()
            .filter_map(|step| match &step.kind {
                OrchestratorStepKind::MergeRequested {
                    id,
                    branch: b,
                    base: a,
                    ..
                } if b == branch && a == base => Some((id.clone(), step.at_ms)),
                _ => None,
            });
        // Merged since, or cancelled by the user, it can't merge again.
        let used = |asked: &str| used(&board, asked);
        let named = if consent.is_empty() {
            // The user's click on "Merge into main" after their latest message (BUILD-PLAN A4).
            let click = clicks
                .clone()
                .next_back()
                .filter(|(_, at)| latest.is_none_or(|latest| *at >= latest.created_at_ms));
            let Some((asked, at)) = click else {
                return Err(refuse(
                    "name the user's message, card answer or click you act on in `consent`, or quote their words".into(),
                ));
            };
            (asked, at)
        } else if let Some((asked, at)) = clicks.clone().find(|(click, _)| click == consent) {
            (asked, at)
        } else if let Some(card) = board
            .questions
            .values()
            .find(|card| card.id.to_string() == consent && card.answer.is_some())
        {
            (
                card.id.to_string(),
                card.answered_at_ms.unwrap_or(card.created_at_ms),
            )
        } else {
            let merged = board
                .orchestrator_steps
                .iter()
                .filter(|step| {
                    matches!(
                        step.kind,
                        OrchestratorStepKind::Merged { .. }
                            | OrchestratorStepKind::MergeCancelled { .. }
                    )
                })
                .map(|step| step.at_ms)
                .max();
            let message = messages.iter().rev().find(|message| {
                message.role == MessageRole::User
                    && (message.id == consent
                        || (merged.is_none_or(|merged| message.created_at_ms > merged)
                            && merge_consent::quotes(consent, &message.text)))
            });
            let Some(message) = message else {
                return Err(refuse(format!(
                    "\"{consent}\" is no message, card answer or click of the user's since the last merge; name the one you act on, or quote their words from it"
                )));
            };
            (message.id.clone(), message.created_at_ms)
        };
        let (asked, at_ms) = named;
        if used(&asked) {
            return Err(refuse(
                "that consent was used for a merge already, or the user cancelled it; another merge needs their fresh word".into(),
            ));
        }
        Ok(Consent {
            asked_in: asked,
            at_ms,
        })
    }

    /// A commit message Brigadier writes, without AI co-authors while the user leaves them out.
    pub(super) fn commit_message(&self, message: String) -> String {
        if self.core.settings().omit_ai_coauthors {
            brigadier_git::strip_ai_coauthors(&message)
        } else {
            message
        }
    }

    pub(super) fn task_repo(&self, task: &Task) -> Result<PathBuf> {
        match self.core.conversation(&task.conversation_id)?.setup {
            Some(Setup::Session { repo, .. }) => Ok(PathBuf::from(repo)),
            _ => Err(Error::Invalid("tasks belong to a session".into())),
        }
    }
}

/// A merge task's landing (SESSION-FIXES-PLAN.md Q2): its worker merged with git and
/// committed on its own branch; the engine only checks and moves the target. HEAD must hold
/// a merge of the target's tip and `merge.incoming` (a fix committed after the merge is fine),
/// with no conflict marker left and nothing of either side gone unless the report's `deleted`
/// names it. Then the target fast-forwards to HEAD, if it is still where the check saw it.
/// No litter guard: the merge lands as the worker committed it. `started`: the target's tip
/// when the worker started.
#[allow(clippy::too_many_arguments)]
fn land_merge(
    repo: &brigadier_git::Repo,
    worktree: &brigadier_git::Worktree,
    started: &Oid,
    target: &str,
    merge: &MergeInputs,
    reported: &[String],
    deleted: &[String],
) -> Result<Moved> {
    let refused = |text: String| {
        Ok(Moved::MergeRefused {
            text: format!("{text} Then build, test and call submit_report again."),
        })
    };
    let head = worktree.head().map_err(git_error)?;
    // Only what it committed lands.
    let left = worktree.changes(&head).map_err(git_error)?;
    let uncommitted = keep_paths(&left, reported, &mut Vec::new());
    if !uncommitted.is_empty() {
        return refused(format!(
            "Not committed: {}. Finish the merge and commit it on your branch with git; commit any fix after it too.",
            uncommitted.join(", ")
        ));
    }
    let onto = repo
        .branch_tip(target)
        .map_err(git_error)?
        .ok_or_else(|| Error::Invalid(format!("branch {target} does not exist")))?;
    let incoming = Oid(merge.incoming.clone());
    let audit = repo
        .audit_merge(&head, &onto, &incoming)
        .map_err(git_error)?;
    if !audit.merged {
        // Merged as the target was when it started; the target moved since.
        if onto != *started
            && repo
                .audit_merge(&head, started, &incoming)
                .map_err(git_error)?
                .merged
        {
            return Ok(Moved::Outrun { tip: onto });
        }
        return refused(format!(
            "Your branch holds no merge of `{target}` and {}: merge with `git merge`, resolve every conflict and commit the merge on your branch.",
            merge.label
        ));
    }
    let mut problems = Vec::new();
    let mut markers = Vec::new();
    for path in &audit.touched {
        if let Some(content) = repo.file_at(&head, path).map_err(git_error)?
            && content.split(|byte| *byte == b'\n').any(conflict_marker)
        {
            markers.push(path.as_str());
        }
    }
    if !markers.is_empty() {
        problems.push(format!(
            "Conflict markers are left in: {}. Resolve every hunk.",
            markers.join(", ")
        ));
    }
    let name = |side: Side| match side {
        Side::Ours => format!("`{target}`"),
        Side::Theirs => merge.label.clone(),
    };
    let missing: Vec<_> = audit
        .missing
        .iter()
        .filter(|gone| !deleted.contains(&gone.path))
        .collect();
    let plain: Vec<String> = missing
        .iter()
        .filter(|gone| !gone.contested)
        .map(|gone| format!("{} (from {})", gone.path, name(gone.side)))
        .collect();
    if !plain.is_empty() {
        problems.push(format!(
            "Missing from the merge: {}. Restore them, or list them under deleted with why.",
            plain.join(", ")
        ));
    }
    for gone in missing.iter().filter(|gone| gone.contested) {
        let other = match gone.side {
            Side::Ours => Side::Theirs,
            Side::Theirs => Side::Ours,
        };
        problems.push(format!(
            "{}: {} changed it, {} deleted it. Keep a version, or list it under deleted with why.",
            gone.path,
            name(gone.side),
            name(other)
        ));
    }
    if !problems.is_empty() {
        problems.push("Fix it with a commit on top of your merge.".into());
        return refused(problems.join(" "));
    }
    let commits = repo.count_commits(&onto, &head).map_err(git_error)?;
    if commits == 0 {
        return Ok(Moved::Nothing {
            excluded: Vec::new(),
        });
    }
    let request = LandRequest {
        branch: target.to_owned(),
        expected_tip: onto.clone(),
        commit: head,
    };
    Ok(match repo.land(&request).map_err(git_error)? {
        LandOutcome::Landed { new_tip } => Moved::Landed {
            from: onto,
            tip: new_tip,
            commits,
            excluded: Vec::new(),
        },
        LandOutcome::Blocked(LandBlock::TipMoved { actual }) => Moved::Outrun { tip: actual },
        LandOutcome::Blocked(block) => Moved::Blocked(block),
    })
}

/// Whether `line` opens, splits or closes a conflict as git writes one: a run of at least
/// seven `<`, `|` or `>`, alone on its line or followed by a space. The width follows the
/// file's `conflict-marker-size`, so it isn't fixed. A `=======` separator only ever stands
/// between such an opener and closer, so it adds nothing, and a heading's underline
/// (`==========`) never counts.
fn conflict_marker(line: &[u8]) -> bool {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let Some(&first) = line.first().filter(|byte| b"<|>".contains(byte)) else {
        return false;
    };
    let run = line.iter().take_while(|byte| **byte == first).count();
    run >= 7 && line.get(run).is_none_or(|byte| *byte == b' ')
}

/// The user's consent to a session merge.
/// What an auto merge's go-ahead is recorded as, where a merge on the user's consent records
/// their message or click: no consent was asked.
const AUTO_GO_AHEAD: &str = "auto:";

pub(super) struct Consent {
    /// The user message, or "Merge into main" click, that gave it.
    asked_in: String,
    /// When they gave it.
    at_ms: i64,
}

/// The session's merge the user consented to while workers wrote, or that stopped at
/// conflicts with the base, as its `MergeHeld` step stored it: pending until it merges, is
/// cancelled or stops (SESSION-FIXES-PLAN.md Q3).
#[derive(Debug, Clone)]
pub(super) struct PendingMerge {
    pub asked_in: String,
    pub consented_at_ms: i64,
    pub branch: String,
    pub base: String,
    pub condition: crate::work::MergeCondition,
    pub waiting: Vec<crate::work::TaskId>,
    /// The branch's tip its consent covers.
    pub tip: String,
    /// When it was held.
    pub at_ms: i64,
    /// The request whose turn held it.
    pub request: Option<String>,
}

/// Whether a message the user wrote at or after `at_ms` still waits in the queue.
fn wrote_since(board: &crate::board::Board, at_ms: i64) -> bool {
    board
        .queue
        .items
        .iter()
        .any(|item| item.queued_at_ms.max(item.edited_at_ms.unwrap_or(0)) >= at_ms)
}

/// The consent (`asked_in`) of the merge held now, if any, for the sidebar's "Merging".
pub(crate) fn held_merge(board: &crate::board::Board) -> Option<String> {
    pending_merge(board).map(|held| held.asked_in)
}

/// The pending merge, if any: the latest `MergeHeld` whose consent isn't spent.
pub(super) fn pending_merge(board: &crate::board::Board) -> Option<PendingMerge> {
    let steps = &board.orchestrator_steps;
    let at = steps
        .iter()
        .rposition(|step| matches!(step.kind, OrchestratorStepKind::MergeHeld { .. }))?;
    // A merge since, on a fresh yes, leaves nothing pending.
    if steps[at + 1..]
        .iter()
        .any(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
    {
        return None;
    }
    let OrchestratorStepKind::MergeHeld {
        branch,
        base,
        asked_in,
        consented_at_ms,
        condition,
        waiting,
        tip,
    } = &steps[at].kind
    else {
        return None;
    };
    Some(PendingMerge {
        asked_in: asked_in.clone(),
        consented_at_ms: *consented_at_ms,
        branch: branch.clone(),
        base: base.clone(),
        condition: *condition,
        waiting: waiting.clone(),
        tip: tip.clone(),
        at_ms: steps[at].at_ms,
        request: steps[at].request_id.clone(),
    })
    .filter(|held| {
        !used(board, &held.asked_in)
            && !steps[at + 1..]
                .iter()
                .any(|step| step.kind.ends_merge(&held.asked_in))
    })
}

/// Whether the consent given in `asked` was used for a merge or the user cancelled it: a stop
/// leaves it for the thread to use again (#240).
fn used(board: &crate::board::Board, asked: &str) -> bool {
    board
        .orchestrator_steps
        .iter()
        .any(|step| match &step.kind {
            OrchestratorStepKind::Merged {
                asked_in: Some(done),
                ..
            } => done == asked,
            OrchestratorStepKind::MergeCancelled { asked_in } => asked_in == asked,
            _ => false,
        })
}

/// What a held merge does once no worker writes (SESSION-FIXES-PLAN.md Q3 step 3). Review
/// findings, and a review still running, don't hold it: the merge's result lists them (#242).
#[derive(Debug, PartialEq)]
pub(super) enum Gate {
    /// Nothing merges: a writer failed or was stopped.
    Stop(String),
    Due,
}

/// A held merge's tip moved on along the session's own landings, each `from` → `to`, in
/// whatever order they were told; the moves it follows are spent.
fn advance_tip(tip: &str, moves: &mut HashMap<String, String>) -> String {
    let mut tip = tip.to_owned();
    while let Some(next) = moves.remove(&tip) {
        tip = next;
    }
    tip
}

/// A landing's review not stored yet, counted on its conversation until dropped.
struct ReviewDue(std::sync::Arc<super::conversation::ConvLive>);

impl ReviewDue {
    fn new(conv: std::sync::Arc<super::conversation::ConvLive>) -> Self {
        conv.reviews_due
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self(conv)
    }
}

impl Drop for ReviewDue {
    fn drop(&mut self) {
        self.0
            .reviews_due
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// [`Gate`] of `held` with every write task final.
pub(super) fn held_gate(held: &PendingMerge, tasks: &[Task]) -> Gate {
    let ended = tasks.iter().find(|task| {
        task.kind.writes()
            && (held.waiting.contains(&task.id) || task.created_at_ms >= held.at_ms)
            && matches!(task.state, TaskState::Failed | TaskState::Stopped)
    });
    if let Some(task) = ended {
        let how = if task.state == TaskState::Failed {
            "failed"
        } else {
            "was stopped"
        };
        return Gate::Stop(match task.error.as_deref().or(task.end_state.as_deref()) {
            Some(why) => format!("task-{} {how}: {}", task.number, why.trim()),
            None => format!("task-{} {how}.", task.number),
        });
    }
    Gate::Due
}

fn stopped_text(reason: &str) -> String {
    format!(
        "[merge stopped] {reason} Nothing was merged, and the user's consent isn't spent: tell them why, and judge what follows."
    )
}

/// The code reviews the thread hears, started since the session's previous merge.
pub(super) fn reviews_since_merge(board: &crate::board::Board) -> Vec<&crate::work::ReviewRun> {
    use crate::work::{ReviewFor, ReviewKind};
    let since = board
        .orchestrator_steps
        .iter()
        .filter(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
        .map(|step| step.at_ms)
        .max()
        .unwrap_or(0);
    let mut reviews: Vec<_> = board
        .reviews
        .values()
        .filter(|review| {
            review.kind == ReviewKind::Code
                && review.notify == ReviewFor::Orchestrator
                && review.started_at_ms >= since
        })
        .collect();
    reviews.sort_by_key(|review| (review.number, review.started_at_ms));
    reviews
}

/// How long a merge waits for the fetch of its base's remote (#224).
const FETCH_PATIENCE: std::time::Duration = std::time::Duration::from_secs(20);

/// Where the merge landed against the base's upstream (#224), for the `[finished]` text.
fn pushed_line(
    base: &str,
    upstream: Option<&MergedUpstream>,
    fetch_failed: Option<&str>,
) -> String {
    let mut line = upstream.map_or_else(String::new, |upstream| {
        format!(
            " It is merged on this Mac only, not pushed to `{}`.",
            upstream.name
        )
    });
    line.push_str(&diverged_line(base, upstream, fetch_failed));
    line
}

/// The base diverged from its upstream, or couldn't be fetched (#224): what that means.
fn diverged_line(
    base: &str,
    upstream: Option<&MergedUpstream>,
    fetch_failed: Option<&str>,
) -> String {
    let mut line = String::new();
    if let Some(upstream) = upstream.filter(|upstream| upstream.behind > 0) {
        line.push_str(&format!(
            " Local `{base}` has diverged from `{}`: {} commit{} there {} not in it, so pushing needs them merged in first.",
            upstream.name,
            upstream.behind,
            if upstream.behind == 1 { "" } else { "s" },
            if upstream.behind == 1 { "is" } else { "are" },
        ));
    }
    if let Some(why) = fetch_failed {
        line.push_str(&match upstream {
            Some(upstream) => format!(
                " Fetching `{}` failed ({why}), so this is as last fetched.",
                upstream.name
            ),
            None => format!(
                " Fetching `{base}`'s remote failed ({why}), so whether `{base}` is behind it isn't known."
            ),
        });
    }
    line
}

fn not_consented(why: String) -> Error {
    Error::Invalid(format!(
        "[not merged] {why}. Nothing was merged. Merge only once the user consents: [merge], or their own words asking for it. You may say once, in plain text, that the work is ready."
    ))
}

/// What the code reviews of the work merged now say, for the thread to tell the user: those it
/// hears, started since the session's previous merge. It counts only what the thread recorded
/// (`record_findings`), never a sum of reviews, and lists the open findings in its words.
///
/// `review_due`: a landing's review isn't stored yet, so it counts as running.
pub(super) fn merged_findings(board: &crate::board::Board, review_due: bool) -> String {
    use crate::work::{FindingStatus, ReviewState};
    let reviews = reviews_since_merge(board);
    if reviews.is_empty() && !review_due {
        return String::new();
    }
    // A review still running doesn't hold the merge (#242): it is said, with what the others
    // found.
    let running = if review_due
        || reviews
            .iter()
            .any(|review| review.state == ReviewState::Running)
    {
        " Its review still runs, so this work isn't reviewed yet: tell the user. Its findings come as a [review …] message; record them and tell the user then."
    } else {
        ""
    };
    // A review stored before findings were items has a count and no items: its findings were
    // never recorded one by one, so they can't count as fixed.
    let untracked: u32 = reviews
        .iter()
        .filter(|review| review.items.is_empty())
        .filter_map(|review| match review.state {
            ReviewState::Findings { count } => Some(count),
            _ => None,
        })
        .sum();
    let untracked = match untracked {
        0 => String::new(),
        1 => " A review from before findings were recorded found 1 thing, never recorded: tell the user it may still be open.".into(),
        n => format!(" A review from before findings were recorded found {n} things, never recorded: tell the user they may still be open."),
    };
    let items: Vec<_> = reviews.iter().flat_map(|review| &review.items).collect();
    if items.is_empty() && !untracked.is_empty() {
        return format!("{running}{untracked}");
    }
    if items.is_empty() && !running.is_empty() {
        return running.into();
    }
    if items.is_empty() {
        return if reviews
            .iter()
            .all(|review| matches!(review.state, ReviewState::Failed { .. }))
        {
            " Its review couldn't run.".into()
        } else {
            " Its review is clean.".into()
        };
    }
    let open: Vec<String> = items
        .iter()
        .filter_map(|item| match &item.status {
            FindingStatus::Open { why } => Some(match why {
                Some(why) => format!("{} {} ({why})", item.id, item.title),
                None => format!("{} {}", item.id, item.title),
            }),
            _ => None,
        })
        .collect();
    let new: Vec<&str> = items
        .iter()
        .filter(|item| item.status == FindingStatus::New)
        .map(|item| item.id.as_str())
        .collect();
    let unverified = unverified_findings(&items);
    let mut text = running.to_owned();
    if !open.is_empty() {
        text.push_str(&format!(
            " Open review findings, as you recorded them (not fixed): {}. Tell the user, as a list.",
            open.join("; ")
        ));
    }
    if !new.is_empty() {
        text.push_str(&format!(
            " Findings you haven't recorded: {}. Record them with record_findings, and tell the user which stay open.",
            new.join(", ")
        ));
    }
    if !unverified.is_empty() {
        text.push_str(&unverified_text(&unverified));
    }
    if open.is_empty() && new.is_empty() && unverified.is_empty() {
        text.push_str(if running.is_empty() {
            " Its review's findings are all fixed or dismissed, as you recorded them."
        } else {
            " The findings of its finished reviews are all fixed or dismissed, as you recorded them."
        });
    }
    text.push_str(&untracked);
    text
}

/// The findings recorded fixed without a check in the merged code, with their titles.
fn unverified_findings(items: &[&crate::work::Finding]) -> Vec<String> {
    items
        .iter()
        .filter(|item| super::review_runs::unverified(&item.status))
        .map(|item| format!("{} {}", item.id, item.title))
        .collect()
}

/// What the thread is told of findings recorded fixed without a check in the merged code.
fn unverified_text(unverified: &[String]) -> String {
    format!(
        " Recorded fixed but never checked in the merged code (unverified): {}. Tell the user plainly that these are unverified, never that the review is clean; or check each in the merged code and record it again with `checked`.",
        unverified.join("; ")
    )
}

pub(super) fn diff_stat_of(stat: &brigadier_git::DiffStat) -> DiffStat {
    DiffStat {
        files: stat
            .files
            .iter()
            .map(|file| FileStat {
                path: file.path.clone(),
                insertions: file.insertions,
                deletions: file.deletions,
                binary: file.binary,
                change: None,
                from: None,
            })
            .collect(),
        insertions: stat.insertions,
        deletions: stat.deletions,
    }
}

/// A repo-relative path as reported (`./a/b`, `a/b/`, `a/b: what it is`) → `a/b`.
fn normalize(path: &str) -> String {
    let mut path = unbulleted(path);
    // A path in backticks is the whole name, whatever follows it ("- `src/a.ts` (new)").
    if let Some(quoted) = backticked(path) {
        return bare(quoted);
    }
    // What a line says about its file follows the path ("src/a.ts: the parser").
    for note in [": ", " (", " \u{2014} ", " \u{2013} "] {
        if let Some((head, _)) = path.split_once(note) {
            path = head;
        }
    }
    bare(path)
}

/// The paths a reported line can name: the line read as a path whole (`docs/Design (draft).md`)
/// and without what it says about its file, so neither a note nor a name with one in it is lost.
fn reported_paths(path: &str) -> Vec<String> {
    let (whole, short) = (bare(unbulleted(path)), normalize(path));
    if whole == short || backticked(unbulleted(path)).is_some() {
        vec![short]
    } else {
        vec![whole, short]
    }
}

fn unbulleted(path: &str) -> &str {
    let path = path.trim();
    path.strip_prefix("- ")
        .or_else(|| path.strip_prefix("* "))
        .unwrap_or(path)
}

fn backticked(path: &str) -> Option<&str> {
    let (quoted, _) = path.strip_prefix('`')?.split_once('`')?;
    Some(quoted)
}

fn bare(path: &str) -> String {
    let path = path
        .trim()
        .trim_end_matches(':')
        .trim_matches('`')
        .trim_start_matches("./")
        .trim_end_matches('/');
    Path::new(path).to_string_lossy().into_owned()
}

/// "Landed task-3 “Add the flag” on `main`", for "Decided for you". An overnight run's tasks
/// all land on its branch, which the run's card and report already name, so theirs stop at
/// the title.
fn landed_line(task: &Task, target: &str) -> String {
    let landed = format!("Landed task-{} \u{201c}{}\u{201d}", task.number, task.title);
    if task.run.is_some() {
        landed
    } else {
        format!("{landed} on `{target}`")
    }
}

/// How a merge worker merges `other` into its branch, whose side is `ours` (Q2).
fn merge_steps(other: &str, ours: &str) -> String {
    format!(
        "Run `git merge {other}` yourself on this branch. Resolve every conflict keeping both sides' intent: in a text conflict the first side (after `<<<<<<<`) is {ours}, the second (before `>>>>>>>`) is `{other}`'s; leave no marker behind. A file one side changed and the other deleted: keep a version, or remove it and list it under `deleted` with why, as for any file you remove on purpose. Make sure it builds and its tests pass, then `git commit` the merge on this branch; a fix after it is a normal commit on top. Never rebase, reset or amend the merge away, and never move any other branch: Brigadier checks that both sides are merged and nothing of either is missing, then moves the target itself."
    )
}

/// What the orchestrator does about a task whose work conflicts with the target.
fn conflict_step(task: &Task, target: &str) -> String {
    match &task.workspace {
        Some(w) if w.on_snapshot => return snapshot_conflict(task.number),
        _ => {}
    }
    format!(
        "Delegate a merge task with subject task-{}: its worker merges `{target}` into task-{}'s work with git and resolves the conflicts.",
        task.number, task.number
    )
}

/// Why a merge task gets no merge task of its own: its worker merges, and is sent back when
/// its target moves.
pub(super) fn merge_of_merge(number: u32, target: &str) -> String {
    format!(
        "task-{number} is a merge task: its own worker merges with git, and is sent back on its own when `{target}` moves. Send it what to change with message_worker instead of delegating another merge task."
    )
}

/// Why a task that started from the user's uncommitted changes gets no merge task.
fn snapshot_conflict(number: u32) -> String {
    format!(
        "task-{number} started from a snapshot of the user's uncommitted changes, which a merge would carry into the commit. Delegate a new implement task for the same change on the current target instead."
    )
}

fn short(oid: &Oid) -> String {
    oid.0.chars().take(10).collect()
}

fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// "1 commit", "3 commits".
fn commits_word(commits: u32) -> String {
    if commits == 1 {
        "1 commit".to_owned()
    } else {
        format!("{commits} commits")
    }
}

/// The commit `tip` landed as: itself, or what rebuilt it without its litter, followed through
/// later rebuilds (each step at most once, so a cycle can't hold it).
fn landed_as<'a>(tip: &'a Oid, rebuilt: &'a HashMap<Oid, Oid>) -> &'a Oid {
    let mut landed = tip;
    for _ in 0..rebuilt.len() {
        match rebuilt.get(landed) {
            Some(next) => landed = next,
            None => break,
        }
    }
    landed
}

/// What the litter guard left out, as a sentence (empty when nothing).
fn litter_note(excluded: &[ExcludedFile]) -> String {
    if excluded.is_empty() {
        return String::new();
    }
    format!(
        " Left out as litter (still in its worktree): {}.",
        excluded
            .iter()
            .map(|e| format!("{} ({})", e.path, e.reason))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// The changes to commit from `changes`; what the litter guard leaves out goes to `excluded`.
fn keep_paths(
    changes: &[brigadier_git::Change],
    reported: &[String],
    excluded: &mut Vec<ExcludedFile>,
) -> Vec<String> {
    let mut include = Vec::new();
    for (change, verdict) in litter::classify(changes, reported) {
        match verdict {
            litter::Verdict::Keep => include.push(change.path),
            litter::Verdict::Exclude { reason } => excluded.push(ExcludedFile {
                path: change.path,
                reason,
            }),
        }
    }
    include
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reported_path_is_read_without_what_the_line_says_about_it() {
        for line in [
            "src/a.ts",
            "./src/a.ts",
            "src/a.ts: the parser",
            "- `src/a.ts` (new)",
            "src/a.ts \u{2014} added",
            "`src/a.ts`:",
        ] {
            assert_eq!(normalize(line), "src/a.ts", "{line}");
        }
        assert_eq!(normalize("docs/my notes.md"), "docs/my notes.md");
        assert_eq!(normalize("src/"), "src");
        assert_eq!(reported_paths("- `src/a.ts` (new)"), ["src/a.ts"]);
        assert_eq!(
            reported_paths("docs/Design (draft).md"),
            ["docs/Design (draft).md", "docs/Design"]
        );
        assert_eq!(
            reported_paths("- `docs/Design (draft).md` (new)"),
            ["docs/Design (draft).md"]
        );
    }

    #[test]
    fn a_failed_fetch_is_said_with_or_without_an_upstream() {
        let upstream = MergedUpstream {
            name: "origin/main".into(),
            behind: 2,
        };
        assert_eq!(
            pushed_line("main", Some(&upstream), Some("no network")),
            " It is merged on this Mac only, not pushed to `origin/main`. Local `main` has diverged from `origin/main`: 2 commits there are not in it, so pushing needs them merged in first. Fetching `origin/main` failed (no network), so this is as last fetched."
        );
        assert_eq!(
            pushed_line("main", None, Some("no network")),
            " Fetching `main`'s remote failed (no network), so whether `main` is behind it isn't known."
        );
        assert_eq!(pushed_line("main", None, None), "");
    }

    #[test]
    fn a_merge_held_again_on_stopped_consent_is_pending_again() {
        let mut board = crate::board::Board::default();
        let mut seq = 0;
        let mut step = |board: &mut crate::board::Board, kind: OrchestratorStepKind| {
            seq += 1;
            let step = crate::work::OrchestratorStep {
                request_id: None,
                kind,
                at_ms: seq,
                position: 0,
            };
            board.apply(
                &crate::model::DomainEvent::OrchestratorStepped { step },
                seq,
            );
        };
        let held = |tip: &str| OrchestratorStepKind::MergeHeld {
            branch: "session".into(),
            base: "main".into(),
            asked_in: "m1".into(),
            consented_at_ms: 0,
            condition: crate::work::MergeCondition::WorkersFinish,
            waiting: Vec::new(),
            tip: tip.into(),
        };
        step(&mut board, held("a"));
        // A later tip of the same hold stays in its place.
        step(&mut board, held("b"));
        assert_eq!(board.orchestrator_steps.len(), 1);
        assert_eq!(pending_merge(&board).unwrap().tip, "b");
        step(
            &mut board,
            OrchestratorStepKind::MergeStopped {
                asked_in: "m1".into(),
                reason: "a writer failed".into(),
            },
        );
        assert!(pending_merge(&board).is_none());
        // Held again on the same consent: a new hold, pending, after the stop.
        step(&mut board, held("c"));
        assert_eq!(board.orchestrator_steps.len(), 3);
        assert_eq!(pending_merge(&board).unwrap().tip, "c");
        step(
            &mut board,
            OrchestratorStepKind::MergeCancelled {
                asked_in: "m1".into(),
            },
        );
        assert!(pending_merge(&board).is_none());
        // A tip update told late, after the Cancel, doesn't bring it back: cancelled stays
        // spent.
        step(&mut board, held("d"));
        assert!(pending_merge(&board).is_none());
    }

    /// #242: a review still running and the findings of those that ended inform the merge,
    /// and its result says all of them; none hides another.
    #[test]
    fn a_merge_result_names_a_running_review_with_the_open_unrecorded_and_unverified_findings() {
        use crate::work::{Finding, FindingStatus, ReviewFor, ReviewKind, ReviewRun, ReviewState};
        let finding = |id: &str, status: FindingStatus| Finding {
            id: id.into(),
            severity: Some("P2".into()),
            title: format!("Problem {id}"),
            location: None,
            status,
            high: false,
        };
        let review = |number: u32, state: ReviewState, items: Vec<Finding>| ReviewRun {
            id: format!("review-{number}"),
            conversation_id: ConversationId("c".into()),
            request_id: None,
            task_id: None,
            kind: ReviewKind::Code,
            base: "a".into(),
            tip: "b".into(),
            author: brigadier_providers::ProviderKind::Claude,
            reviewer: brigadier_providers::ProviderKind::Codex,
            reviewer_model: None,
            notify: ReviewFor::Orchestrator,
            state,
            started_at_ms: i64::from(number),
            ended_at_ms: None,
            findings: None,
            number,
            items,
            landing: None,
            answer_waits: false,
        };
        let mut board = crate::board::Board::default();
        board.reviews.insert(
            "review-1".into(),
            review(
                1,
                ReviewState::Findings { count: 3 },
                vec![
                    finding("R1.1", FindingStatus::Open { why: None }),
                    finding("R1.2", FindingStatus::New),
                    finding(
                        "R1.3",
                        FindingStatus::Fixed {
                            task: None,
                            why: None,
                            checked: None,
                        },
                    ),
                ],
            ),
        );
        board.reviews.insert(
            "review-2".into(),
            review(2, ReviewState::Running, Vec::new()),
        );
        let text = merged_findings(&board, false);
        assert!(
            text.contains("Its review still runs, so this work isn't reviewed yet"),
            "{text}"
        );
        assert!(text.contains("Open review findings"), "{text}");
        assert!(text.contains("R1.1 Problem R1.1"), "{text}");
        assert!(
            text.contains("Findings you haven't recorded: R1.2"),
            "{text}"
        );
        assert!(text.contains("(unverified): R1.3"), "{text}");
        // Only the running review: it says so, never that the review is clean.
        board.reviews.remove("review-1");
        let text = merged_findings(&board, false);
        assert!(
            text.contains("still runs") && !text.contains("clean"),
            "{text}"
        );
        // A landing's review not stored yet runs too, though only a clean one is in.
        board.reviews.remove("review-2");
        board
            .reviews
            .insert("review-3".into(), review(3, ReviewState::Clean, Vec::new()));
        assert_eq!(merged_findings(&board, false), " Its review is clean.");
        let text = merged_findings(&board, true);
        assert!(
            text.contains("still runs") && !text.contains("clean"),
            "{text}"
        );
        board.reviews.clear();
        assert!(merged_findings(&board, true).contains("still runs"));
    }

    #[test]
    fn a_held_tip_follows_landings_told_out_of_order() {
        let mut moves = HashMap::new();
        // H1 → H2 is told first, while the tip is still H0: it waits.
        moves.insert("h1".to_owned(), "h2".to_owned());
        assert_eq!(advance_tip("h0", &mut moves), "h0");
        // H0 → H1 joins them up.
        moves.insert("h0".to_owned(), "h1".to_owned());
        assert_eq!(advance_tip("h0", &mut moves), "h2");
        assert!(moves.is_empty());
        // A move that starts elsewhere is not followed.
        moves.insert("x0".to_owned(), "x1".to_owned());
        assert_eq!(advance_tip("h2", &mut moves), "h2");
    }

    #[test]
    fn a_tip_lands_as_its_last_rebuild() {
        let oid = |c: char| Oid(c.to_string().repeat(40));
        let rebuilt: HashMap<Oid, Oid> = [(oid('a'), oid('b')), (oid('b'), oid('c'))].into();
        // Rebuilt by one merge's start, then again by a later merge's.
        assert_eq!(landed_as(&oid('a'), &rebuilt), &oid('c'));
        assert_eq!(landed_as(&oid('d'), &rebuilt), &oid('d'));
        // A cycle ends.
        let cycle: HashMap<Oid, Oid> = [(oid('a'), oid('b')), (oid('b'), oid('a'))].into();
        landed_as(&oid('a'), &cycle);
    }

    #[test]
    fn old_merge_cards_still_load() {
        let old: QuestionKind = serde_json::from_value(serde_json::json!({
            "type": "merge", "branch": "session", "base": "main"
        }))
        .unwrap();
        assert!(
            matches!(old, QuestionKind::Merge { conflicted: false, conflicts, .. } if conflicts.is_empty())
        );
    }

    #[test]
    fn only_git_conflict_markers_count_not_heading_underlines() {
        let unresolved = |content: &str| {
            content
                .split('\n')
                .any(|line| conflict_marker(line.as_bytes()))
        };
        // Default markers, and 32 wide from `conflict-marker-size=32`.
        for width in [7, 32] {
            let (open, split, close) = ("<".repeat(width), "=".repeat(width), ">".repeat(width));
            let content = format!("a\n{open} ours\nb\n{split}\nc\n{close} theirs\nd\n");
            assert!(unresolved(&content), "{content}");
            let content = format!(
                "{open}\r\nb\r\n|||||||{}\r\n{split}\r\n{close}\r\n",
                "|".repeat(width - 7)
            );
            assert!(unresolved(&content), "{content}");
        }
        // A resolution with heading underlines of every width.
        let resolved =
            "Title\n=======\n\nPart\n==========\n\nMore\n================================\n";
        assert!(!unresolved(resolved));
        for line in [
            "<<<<<<<",
            "<<<<<<< ours",
            ">>>>>>> theirs\r",
            "||||||| base",
            "<<<<<<<<",
            ">>>>>>>>> theirs",
        ] {
            assert!(conflict_marker(line.as_bytes()), "{line:?}");
        }
        for line in [
            "=======",
            "==========",
            "<<<<<<",
            "<<<<<<<\tours",
            ">>>>>>>>x",
            "<<< <<<<",
            "",
        ] {
            assert!(!conflict_marker(line.as_bytes()), "{line:?}");
        }
    }

    #[test]
    fn a_runs_landing_line_leaves_out_the_branch_its_report_already_names() {
        let mut task: Task = serde_json::from_value(serde_json::json!({
            "id": "t1",
            "conversationId": "c1",
            "number": 3,
            "position": 0,
            "title": "Add avg2",
            "kind": "implement",
            "spec": "Add avg2 to src/math.js.",
            "access": { "repo": "write", "network": false, "unsandboxed": false },
            "route": { "choice": { "provider": "claude", "model": null, "effort": null }, "reason": "" },
            "state": "readyToLand",
            "attachments": [],
            "createdAtMs": 0,
            "updatedAtMs": 0
        }))
        .expect("a task");
        assert_eq!(
            landed_line(&task, "main"),
            "Landed task-3 \u{201c}Add avg2\u{201d} on `main`"
        );
        task.run = Some(
            serde_json::from_value(serde_json::json!({
                "runId": "r1",
                "segment": 1,
                "generation": 1,
                "role": "worker",
                "rulesHash": ""
            }))
            .expect("a run context"),
        );
        assert_eq!(
            landed_line(&task, "overnight/2026-10-04-textkit-1234"),
            "Landed task-3 \u{201c}Add avg2\u{201d}"
        );
    }
}
