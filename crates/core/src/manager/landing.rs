//! Landing a phase's work (the delegator flow, §2.4 of the flow spec): the worker that ends a
//! phase (its lead, or a verifier the orchestrator started on top of the lead's work)
//! committed its work in small steps in its own worktree, branched from the session's tip. `land_phase` moves those
//! commits onto the target branch, with no card and no checks of its own:
//!
//! 1. **Leftovers**: anything the worker left uncommitted is committed, litter left out.
//! 2. **Litter guard over the whole range** (phase start..HEAD): logs, scratch files and new
//!    files no report of the phase names are dropped from every commit and listed.
//! 3. **Rebase** onto the target's tip when it moved (each commit replayed, authors and
//!    messages kept). After a real rebase the worker runs a quick self-check (build and the
//!    tests it touched) before anything lands; its next report then lands on its own,
//!    fast-forward only, never reviewed again. Conflicts go to a merge worker.
//! 4. **Fast-forward** the target, never over uncommitted, untracked or ignored files, and
//!    only if the target is still where it was (the git engine checks it right before
//!    mutating). Anything unsafe leaves the task "ready to land" with nothing changed.
//!
//! Every landing then gets one read-only review by the other vendor in the background
//! ([`super::review_runs`]); nothing waits for it.
//!
//! Finishing a new-worktree session merges the session branch into its base the same way, when
//! the user's latest message asks for it in words (`merge_consent`): no card.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use brigadier_git::{
    ChangeKind, CommitOutcome, LandBlock, LandOutcome, LandRequest, MergeOutcome, Oid,
    SeriesOutcome, Side, litter,
};

use super::cards::{MERGE_RESOLVING, merge_chosen, merge_label};
use super::conversation::Envelope;
use super::workers::Workspace;
use super::{SessionManager, blocking, git_error, merge_consent};
use crate::model::{ConversationId, Environment, MessageRole, Setup};
use crate::work::{
    DecisionKind, DecisionSource, DiffStat, ExcludedFile, FileStat, InjectionKind, MergeInputs,
    OrchestratorStepKind, PhaseStage, QuestionItem, QuestionKind, QuestionOption, Task, TaskState,
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
    /// The target moved: the commits were rebased onto `onto` and wait for the worker's
    /// self-check.
    Rebased {
        onto: Oid,
        commits: u32,
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
        // A verifier the orchestrator started on this work lands it with its own.
        if let Some(verifier) = self.verifier_of(&task).await {
            return Err(Error::Invalid(format!(
                "task-{}'s work is verified by task-{}, which you started on top of its commits: land task-{} once it reports.",
                task.number, verifier.number, verifier.number
            )));
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

    /// A worker rebased during its landing reported after its self-check: its commits land
    /// now, on their own (fast-forward, or another rebase and self-check if the target moved
    /// again). The orchestrator hears the outcome.
    pub(crate) async fn land_after_self_check(&self, task: &Task) {
        let Ok(_fence) = self.enter(&task.conversation_id) else {
            return;
        };
        let Ok(task) = self
            .update_task(&task.conversation_id, &task.id, |t| {
                t.state = TaskState::Landing;
                t.blocked_reason = None;
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
            self.land_commits(
                task, git, worktree, repo, base, target, reported, &workspace,
            )
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
        let on_snapshot = workspace.on_snapshot;
        let leftovers = self.commit_message(format!(
            "{}\n\nWhat task-{} left uncommitted, committed as it landed.",
            task.title, task.number
        ));
        let omit_ai_coauthors = self.core.settings().omit_ai_coauthors;
        let (records, branch) = (self.ref_records.clone(), workspace.branch.clone());
        #[cfg(test)]
        let rewritten = self.landing_rewritten.lock().unwrap().clone();
        blocking(move || {
            let repo = git.open(&repo).map_err(git_error)?;
            let worktree = git.open_worktree(&worktree).map_err(git_error)?;
            let mut tries = 0;
            loop {
                tries += 1;
                let mut excluded = Vec::new();
                let onto = repo
                    .branch_tip(&target)
                    .map_err(git_error)?
                    .ok_or_else(|| Error::Invalid(format!("branch {target} does not exist")))?;
                let (tip, commits, rebased) = {
                    // 1. What it left uncommitted.
                    let head = worktree.head().map_err(git_error)?;
                    let left = worktree.changes(&head).map_err(git_error)?;
                    let include = keep_paths(&left, &reported, &mut excluded);
                    if !include.is_empty()
                        && let CommitOutcome::HookFailed { output } = worktree
                            .commit_candidate(&include, &leftovers)
                            .map_err(git_error)?
                    {
                        return Ok(Moved::HookFailed { output });
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
                    let rewrite = || -> Result<std::result::Result<(Oid, u32, bool), Moved>> {
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
                        // Work started on a snapshot of the user's uncommitted files sits on
                        // the commit the snapshot was taken on.
                        let started = if on_snapshot {
                            repo.resolve(&format!("{}^", base.0)).map_err(git_error)?
                        } else {
                            base.clone()
                        };
                        let rebased = started != onto;
                        let tip = if omit_ai_coauthors && commits > 0 && !rebased {
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
                        Ok(Ok((tip, commits, rebased)))
                    };
                    let rewritten_now = match &branch {
                        Some(branch) => super::refcheck::engine_rewrite(&repo, &records, branch, rewrite),
                        None => rewrite(),
                    };
                    match rewritten_now? {
                        Ok(done) => done,
                        Err(conflicts) => return Ok(conflicts),
                    }
                };
                #[cfg(test)]
                if let Some(rewritten) = &rewritten {
                    rewritten();
                }
                if commits == 0 {
                    return Ok(Moved::Nothing { excluded });
                }
                if rebased {
                    return Ok(Moved::Rebased {
                        onto,
                        commits,
                        excluded,
                    });
                }
                // 4. Fast-forward.
                let request = LandRequest {
                    branch: target.clone(),
                    expected_tip: onto.clone(),
                    commit: tip,
                };
                match repo.land(&request).map_err(git_error)? {
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
        })
        .await
    }

    /// Acts on what moving the commits came to; returns the outcome for the orchestrator.
    /// `on_its_own`: Brigadier lands it after a self-check, so a problem reaches the
    /// orchestrator as a message (the caller announces a landing). Whether it landed comes
    /// with it.
    async fn settle_landing(&self, task: &Task, moved: Moved, on_its_own: bool) -> (bool, String) {
        let target = task
            .workspace
            .as_ref()
            .and_then(|w| w.target.clone())
            .unwrap_or_default();
        // Boxed, like each merge_again below: each call's future is otherwise made in this
        // frame, a slot apiece in an unoptimized build.
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
                self.landed(task, &target, &from, &tip, commits).await;
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
                for done in self.landed_with(task).await {
                    self.dispose_task(&done, TaskState::Done).await;
                    self.set_phase_stage(&done, PhaseStage::Done).await;
                }
                format!(
                    "[nothing to land task-{}] It has no commits to land.{}",
                    task.number,
                    litter_note(&excluded)
                )
            }
            Moved::Rebased {
                onto,
                commits,
                excluded,
            } => {
                let updated = self
                    .update_task(&task.conversation_id, &task.id, |t| {
                        if let Some(w) = t.workspace.as_mut() {
                            w.base = Some(onto.0.clone());
                            w.on_snapshot = false;
                        }
                    })
                    .await;
                let text = format!(
                    "`{target}` moved since you started, so Brigadier rebased your {} onto its tip ({}).{} Run a quick self-check now: build, and run the tests of the crates or packages you touched. Fix and commit anything that broke, then call submit_report again. Your work lands on its own once your report is in; it is not reviewed again.",
                    commits_word(commits),
                    short(&onto),
                    litter_note(&excluded)
                );
                let sent = match updated {
                    Ok(task) => self
                        .message_worker(&task.conversation_id, &task, text, "Brigadier")
                        .await
                        .map(|_| ()),
                    Err(err) => Err(err),
                };
                match sent {
                    Ok(()) => format!(
                        "`{target}` moved, so task-{}'s {} were rebased onto it. It runs a quick self-check, and its work lands on its own after its report; you hear when it has landed.",
                        task.number,
                        commits_word(commits)
                    ),
                    Err(err) => {
                        problem(
                            format!("Its commits were rebased onto `{target}`, which moved, but its worker could not be asked to check them: {err}"),
                            TaskState::Reported,
                        )
                        .await
                    }
                }
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
    /// its next report lands on its own (`land_after_self_check`).
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
        reported.extend(extra.iter().map(|p| normalize(p)));
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

    /// Every file a report of `task`'s phase names: the provenance a new file needs to land.
    async fn phase_reported(&self, task: &Task) -> Vec<String> {
        let mut reported: Vec<String> = Vec::new();
        let mut add = |t: &Task| {
            if let Some(report) = &t.report {
                reported.extend(report.changes.iter().map(|p| normalize(p)));
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

    async fn landed(&self, task: &Task, target: &str, from: &Oid, new_tip: &Oid, commits: u32) {
        // The thread's own commits below the landing get their review; the landing has its own.
        if let Ok(repo) = self.task_repo(task) {
            self.thread_branch_landed(&task.conversation_id, &repo, target, from, new_tip)
                .await;
        }
        let tasks = self.landed_with(task).await;
        // The session's own landing on a held merge's branch is covered by its consent.
        if let Ok(board) = self.core.board(&task.conversation_id).await
            && let Some(held) = pending_merge(&board)
            && held.branch == target
            && held.tip == from.0
        {
            self.orchestrator_step(
                &task.conversation_id,
                OrchestratorStepKind::MergeHeld {
                    branch: held.branch,
                    base: held.base,
                    asked_in: held.asked_in,
                    consented_at_ms: held.consented_at_ms,
                    condition: held.condition,
                    waiting: held.waiting,
                    tip: new_tip.0.clone(),
                },
            )
            .await;
        }
        // A fix of landed work's review findings gets no review of its own, only the
        // targeted re-check of a finding the thread marked high (SESSION-FIXES-PLAN.md Q6.1).
        let fixed = match (&task.role, &task.subject) {
            (Some(crate::work::WorkerRole::Fix), Some(subject)) => self
                .task_by_id(&task.conversation_id, subject)
                .await
                .ok()
                .filter(|subject| subject.landed.is_some()),
            _ => None,
        };
        // Otherwise the landed workers stay resumable for their review's findings, kept
        // before the review starts so that its end always finds them.
        let mut held = std::collections::HashSet::new();
        if fixed.is_none() {
            for landed in &tasks {
                if self.hold_for_review(landed, target).await {
                    held.insert(landed.id.clone());
                }
            }
        }
        for landed in &tasks {
            let updated = self
                .update_task(&landed.conversation_id, &landed.id, |t| {
                    t.landed = Some(new_tip.0.clone());
                    t.landing = None;
                })
                .await;
            // Its report enters the Brain now, at the commit that landed.
            if let Ok(updated) = &updated
                && let Some(report) = &updated.report
            {
                self.learn_report(updated, report, None);
            }
        }
        self.orchestrator_step(
            &task.conversation_id,
            OrchestratorStepKind::Landed {
                task_ids: tasks.iter().map(|t| t.id.clone()).collect(),
                commits,
                branch: target.to_owned(),
                head: new_tip.0.clone(),
            },
        )
        .await;
        let request = self
            .request_for(&task.conversation_id, Some(&task.id))
            .await;
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
        .await;
        for landed in &tasks {
            self.set_phase_stage(landed, PhaseStage::Done).await;
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
            self.dispose_task(landed, TaskState::Landed).await;
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
            match fixed {
                Some(subject) => manager.recheck_fix(&task, &subject, from, tip).await,
                None => manager.review_landing(&task, from, tip).await,
            }
            manager.check_held_merge(&task.conversation_id).await;
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
                "\n\nWhat the orchestrator told the worker after that, oldest first (it changes the task where it differs):",
            );
            for message in &subject.messages {
                text.push_str(&format!("\n---\n{message}"));
            }
        }
        if let Some(report) = &subject.report {
            text.push_str(&format!(
                "\n\nThe worker's report:\n{}\n{}",
                report.summary,
                report
                    .verification
                    .iter()
                    .map(|v| format!("- verified: {v}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
            for (heading, lines) in [
                ("Done when, as the worker reported it", &report.done_when),
                ("Risks the worker named", &report.risks),
            ] {
                if !lines.is_empty() {
                    text.push_str(&format!("\n\n{heading}:"));
                    for line in lines {
                        text.push_str(&format!("\n- {line}"));
                    }
                }
            }
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
            .map(|r| r.changes.iter().map(|p| normalize(p)).collect())
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

    /// `propose_merge`: asks the user once, on a card, whether to merge the session branch into
    /// its base. A second card is refused while one is open, after "Merge" (finish_session is
    /// what's next), or after "Not yet" until the user writes again.
    pub(crate) async fn propose_merge(
        &self,
        id: &ConversationId,
        note: Option<String>,
    ) -> Result<String> {
        if self.overnight.active.get(id).is_some() {
            return Err(Error::Invalid(
                "An overnight run is going in this session: its verified work merges by the user's Merge on the run's card, in the morning.".into(),
            ));
        }
        let Some(Setup::Session {
            repo,
            environment: Environment::NewWorktree { base, branch, .. },
            ..
        }) = self.core.conversation(id)?.setup
        else {
            return Err(Error::Invalid(
                "this session works on a local checkout: its commits are already on the picked branch, and there is nothing to merge".into(),
            ));
        };
        // The merge is offered only once no worker writes (SESSION-FIXES-PLAN.md Q3).
        let writing = self.writing(id).await?;
        if !writing.is_empty() {
            return Err(Error::Invalid(format!(
                "Workers are still writing ({}): offer the merge once they land.",
                writing.join(", ")
            )));
        }
        let board = self.core.board(id).await?;
        let latest = self.latest_user_message_at(id, &board).await?;
        let cards: Vec<_> = board
            .questions
            .values()
            .filter(|question| matches!(&question.kind, QuestionKind::Merge { .. }))
            .collect();
        if cards.iter().any(|card| card.is_open()) {
            return Err(Error::Invalid(
                "A merge card is already open: wait for the user's answer, which arrives as a message.".into(),
            ));
        }
        // A card withdrawn (its request was edited) has no answer and doesn't count.
        let since = |card: &&crate::work::Question| {
            card.answer.is_some()
                && card
                    .answered_at_ms
                    .is_some_and(|answered| latest.is_none_or(|latest| answered >= latest))
        };
        if let Some(card) = cards
            .iter()
            .copied()
            .filter(since)
            .max_by_key(|card| card.answered_at_ms)
        {
            let merge = card
                .answer
                .as_deref()
                .is_some_and(|answer| merge_chosen(answer, &base));
            if merge && !spent(&board, &card.id.to_string()) {
                return Err(Error::Invalid(
                    "The user already chose \"Merge\" on the card: call finish_session, without user_words.".into(),
                ));
            }
            if !merge {
                return Err(Error::Invalid(
                    "The user answered \"Not yet\" on the merge card and hasn't written since: don't ask again until they bring it up.".into(),
                ));
            }
        }
        let note = note
            .map(|line| line.trim().to_owned())
            .filter(|line| !line.is_empty());
        // Whether the merge would stop at conflicts, looked at ahead: the card then offers to
        // resolve them. A look that fails shows the plain card.
        let (git, repo_path, base_name, branch_name) = (
            self.git.clone(),
            PathBuf::from(&repo),
            base.clone(),
            branch.clone(),
        );
        let conflicts = blocking(move || {
            let repo = git.open(&repo_path).map_err(git_error)?;
            match repo
                .prepare_merge(&base_name, &branch_name, "Merge")
                .map_err(git_error)?
            {
                MergeOutcome::Conflicts { paths } => Ok(Some(paths)),
                MergeOutcome::Ready { .. } => Ok(None),
            }
        })
        .await
        .unwrap_or_else(|err| {
            tracing::warn!(conversation = %id, error = %err, "could not look ahead at the session merge");
            None
        });
        let (text, label) = merge_offer(&branch, &base, conflicts.as_deref());
        let item = QuestionItem {
            text,
            options: vec![
                QuestionOption {
                    label,
                    description: note,
                },
                QuestionOption {
                    label: "Not yet".into(),
                    description: Some(format!("The work stays on `{branch}`.")),
                },
            ],
            recommended: None,
        };
        let kind = QuestionKind::Merge {
            branch: branch.clone(),
            base: base.clone(),
            conflicted: conflicts.is_some(),
            conflicts: conflicts
                .unwrap_or_default()
                .into_iter()
                .take(CONFLICTS_SHOWN)
                .collect(),
        };
        self.open_round(id, None, kind, vec![item]).await?;
        Ok(format!(
            "Asked the user whether to merge `{branch}` into `{base}`. The card shows under your final answer: write it now, unless you already did. The answer arrives as an [answer] message."
        ))
    }

    /// When the user last wrote on the branch shown, if ever.
    async fn latest_user_message_at(
        &self,
        id: &ConversationId,
        board: &crate::board::Board,
    ) -> Result<Option<i64>> {
        let messages = match &board.head {
            Some((head, _)) => self.core.branch(id, head).await?,
            None => self.core.all_messages(id).await?,
        };
        Ok(messages
            .iter()
            .rev()
            .find(|message| message.role == MessageRole::User)
            .map(|message| message.created_at_ms))
    }

    /// `finish_session`: merges the session branch into its base once the user consented: on the
    /// thread's merge card, or in their latest message's words (`user_words`, quoted from it;
    /// [`merge_consent`]).
    pub(crate) async fn finish_session(
        &self,
        id: &ConversationId,
        user_words: &str,
        message: Option<String>,
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
        let (consent, wrote) = {
            let wrote = conv.user_wrote.lock().await;
            let consent = self.merge_consent(id, user_words, &branch, &base).await?;
            (consent, *wrote)
        };
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
        // Consent given while workers write holds until they finish (SESSION-FIXES-PLAN.md
        // Q3): stored, so it survives a restart.
        if !writing.is_empty() {
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
                        condition: consent.condition,
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
                "[merge held] Merging after {} worker{} finish ({}). Nothing is merged yet, and the user's consent holds: Brigadier tells you [merge due] once they have landed, or [merge stopped] if one fails or a finding you record stays open. Tell the user in one line, then reply {} while they work.",
                writing.len(),
                if writing.len() == 1 { "" } else { "s" },
                names.join(", "),
                super::prompts::QUIET,
            ));
        }
        if let Some(held) = &pending {
            match held_gate(held, &tasks, &board) {
                Gate::Stop(reason) => {
                    self.stop_held_merge(id, &conv, held, &reason).await;
                    return Err(Error::Invalid(stopped_text(&reason)));
                }
                // A review of the session's work still runs: an open finding it brings
                // would stop the merge, so it waits for the review's end.
                Gate::Wait(new) if new.is_empty() => {
                    // A [merge due] told before that review started is told again at its end.
                    *conv.merge_told.lock().unwrap() = Some((held.asked_in.clone(), "waiting"));
                    return Err(Error::Invalid(
                        "[not merged] A review of the session's work still runs, and the user's held merge waits for it: Brigadier tells you [merge due] when it ends, or [merge stopped]. Nothing was merged; reply [quiet] meanwhile.".into(),
                    ));
                }
                Gate::Wait(_) | Gate::Due => {}
            }
        }
        // Every finding on the session's work is recorded before it merges (Q3, Q7).
        let unrecorded = findings_now(&board).new;
        if !unrecorded.is_empty() {
            return Err(Error::Invalid(format!(
                "[not merged] Record the findings {} with record_findings first (fixed, open, or dismissed and why); then call finish_session again. Nothing was merged.",
                unrecorded.join(", ")
            )));
        }
        let message = self.commit_message(
            message
                .filter(|m| !m.trim().is_empty())
                .unwrap_or_else(|| format!("Merge {branch} into {base}")),
        );
        enum Prepared {
            Ready(Oid, Oid, Oid, u32),
            Conflicts(Vec<String>, Oid),
            /// Other work landed on the branch since the held consent.
            Moved,
        }
        let (git, repo_path, base_name, branch_name, covered) = (
            self.git.clone(),
            PathBuf::from(&repo),
            base.clone(),
            branch.clone(),
            pending.as_ref().map(|held| held.tip.clone()),
        );
        // A held consent covers the branch as it was then, moved on by the session's own
        // landings since: only the user's words take it back (Q3); other work isn't covered.
        let prepared = blocking(move || {
            let repo = git.open(&repo_path).map_err(git_error)?;
            let tip = repo
                .branch_tip(&branch_name)
                .map_err(git_error)?
                .ok_or_else(|| Error::Invalid("the session branch is gone".into()))?;
            if covered.is_some_and(|covered| covered != tip.0) {
                return Ok(Prepared::Moved);
            }
            let outcome = repo
                .prepare_merge_commit(&base_name, &tip, &message)
                .map_err(git_error)?;
            match outcome {
                MergeOutcome::Ready {
                    commit, base_tip, ..
                } => {
                    let commits = repo.count_commits(&base_tip, &tip).map_err(git_error)?;
                    Ok(Prepared::Ready(commit, base_tip, tip, commits))
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
        let (commit, base_tip, session_tip, commits) = match prepared {
            Prepared::Ready(commit, base_tip, tip, commits) => (commit, base_tip, tip, commits),
            Prepared::Moved => {
                return Err(Error::Invalid(format!(
                    "[not merged] Other work landed on `{branch}` since the user consented, and their consent doesn't cover it. Nothing was merged. Ask with propose_merge."
                )));
            }
            Prepared::Conflicts(paths, tip) => {
                // The merge used no consent: it is held, stored, until the merge task that
                // merges the base into the branch lands, if what the user wrote since the
                // first look still allows it.
                let now = conv.user_wrote.lock().await;
                if *now != wrote {
                    return Err(wrote_meanwhile());
                }
                let consent = self.merge_consent(id, user_words, &branch, &base).await?;
                if pending.is_none() {
                    self.orchestrator_step(
                        id,
                        OrchestratorStepKind::MergeHeld {
                            branch: branch.clone(),
                            base: base.clone(),
                            asked_in: consent.asked_in,
                            consented_at_ms: consent.at_ms,
                            condition: consent.condition,
                            waiting: Vec::new(),
                            tip: tip.0,
                        },
                    )
                    .await;
                }
                return Err(Error::Invalid(format!(
                    "`{branch}` conflicts with the current `{base}` in: {}. Nothing was merged, and the user's consent to this merge still holds. Don't ask the user: delegate a merge task without `subject` now. Its worker merges `{base}` into `{branch}` with git and resolves the conflicts. Once it lands, Brigadier tells you [merge due]: then call finish_session again. It merges on that consent unless the user writes \"wait\", \"no\" or a condition meanwhile.",
                    paths.join(", ")
                )));
            }
        };
        if commits == 0 {
            return Err(Error::Invalid(format!(
                "`{branch}` has no commits that `{base}` lacks"
            )));
        }
        // The last look at consent and the landing are one step for what the user writes: a
        // "wait" sent while the merge was prepared stops it; one sent now comes after it.
        let landing = {
            let now = conv.user_wrote.lock().await;
            if *now != wrote {
                return Err(wrote_meanwhile());
            }
            self.merge_consent(id, user_words, &branch, &base).await?;
            let (git, repo_path, session_branch) =
                (self.git.clone(), PathBuf::from(&repo), branch.clone());
            let request = LandRequest {
                branch: base.clone(),
                expected_tip: base_tip,
                commit,
            };
            // What the user asked for is the session branch as it was: work that landed on it
            // meanwhile would be left out.
            blocking(move || {
                let repo = git.open(&repo_path).map_err(git_error)?;
                if repo.branch_tip(&session_branch).map_err(git_error)? != Some(session_tip) {
                    return Ok(None);
                }
                repo.land(&request).map(Some).map_err(git_error)
            })
            .await
        };
        let new_tip = match landing {
            Ok(Some(LandOutcome::Landed { new_tip })) => new_tip,
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
                merged_findings(&board)
            }
            Err(_) => String::new(),
        };
        self.orchestrator_step(
            id,
            OrchestratorStepKind::Merged {
                branch: branch.clone(),
                base: base.clone(),
                commits,
                asked_in: Some(consent.asked_in),
            },
        )
        .await;
        *conv.merge_told.lock().unwrap() = None;
        drop(landing_lock);
        // The session's work is merged: what showed it stops, and its worktree and branch go
        // (THREAD-PLAN.md Q9).
        self.stop_previews(id, "the session was merged").await;
        // The landed workers kept for their reviews' findings are done with too.
        self.settle_all_holds(id).await;
        let released = self.release_merged_session(id).await;
        Ok(format!(
            "[finished] As the user asked: `{branch}` ({commits} commit{}) is merged into `{base}` at {}.{reviews}{released}",
            if commits == 1 { "" } else { "s" },
            short(&new_tip)
        ))
    }

    /// The session's write tasks that aren't final, as `task-N`.
    async fn writing(&self, id: &ConversationId) -> Result<Vec<String>> {
        Ok(self
            .core
            .tasks(id)
            .await?
            .iter()
            .filter(|t| t.kind.writes() && !t.state.is_final())
            .map(|t| format!("task-{}", t.number))
            .collect())
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
            match held_gate(&held, &tasks, &board) {
                Gate::Stop(reason) => {
                    self.stop_held_merge(id, &conv, &held, &reason).await;
                    stopped_text(&reason)
                }
                // A review still runs: its end looks again.
                Gate::Wait(new) if new.is_empty() => return,
                Gate::Wait(new) => {
                    if told("waiting") {
                        return;
                    }
                    format!(
                        "[merge waiting] The workers have finished, and the user's merge waits on the findings {}: record each with record_findings (fixed, open, or dismissed and why). Brigadier tells you when it is due; an open one stops it.",
                        new.join(", ")
                    )
                }
                // A message the user wrote since waits to be read: it decides first, and the
                // thread's turn on it looks again when it ends.
                Gate::Due if wrote_since(&board, held.consented_at_ms) => return,
                Gate::Due => {
                    // Their words since took it back: it stops rather than come due.
                    if let Ok(Some(why)) = self.taken_back(id, &board, &held).await {
                        let reason = format!("{}{}.", why[..1].to_uppercase(), &why[1..]);
                        self.stop_held_merge(id, &conv, &held, &reason).await;
                        stopped_text(&reason)
                    } else {
                        if told("due") {
                            return;
                        }
                        "[merge due] The workers have finished and nothing stops it: the user's merge is due. Call finish_session now; their held consent covers it, so don't ask them again.".to_owned()
                    }
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
                text: "[merge cancelled by the user] The user cancelled the merge they had asked for. Don't merge: their consent is spent, and only new words or a new Merge on a card give it again. Reply in one line at most.".into(),
            },
        )
        .await;
        Ok("Merge cancelled.".into())
    }

    /// The user's consent to the session's merge: given now ([`Self::fresh_consent`]), or held
    /// (while workers wrote, or a merge stopped at conflicts with the base), while the user
    /// took nothing back since they gave it ([`Self::held_consent`]).
    async fn merge_consent(
        &self,
        id: &ConversationId,
        user_words: &str,
        branch: &str,
        base: &str,
    ) -> Result<Consent> {
        let fresh = self.fresh_consent(id, user_words, branch, base).await;
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

    /// Consent `held` (a pending merge), unless it was used or cancelled, or the user since
    /// they gave it wrote something that waits, says "no", "wait", "not yet" or "don't", holds
    /// the merge or puts a condition on it ([`merge_consent::takes_back`]), or answered "Not
    /// yet" on a merge card. `refused` is why fresh consent was refused.
    async fn held_consent(
        &self,
        id: &ConversationId,
        held: PendingMerge,
        refused: Error,
    ) -> Result<Consent> {
        let board = self.core.board(id).await?;
        if spent(&board, &held.asked_in) {
            return Err(refused);
        }
        if wrote_since(&board, held.consented_at_ms) {
            return Err(not_consented(
                "the user wrote again since (it waits in the queue): read it first".into(),
            ));
        }
        if let Some(why) = self.taken_back(id, &board, &held).await? {
            return Err(not_consented(why));
        }
        Ok(Consent {
            asked_in: held.asked_in,
            at_ms: held.consented_at_ms,
            condition: held.condition,
        })
    }

    /// Why the user took back the consent `held` since they gave it: a "Not yet" on a merge
    /// card, or a message that waits, says "no", "wait", "not yet" or "don't", holds the merge
    /// or puts a condition on it ([`merge_consent::takes_back`]). `None` while it holds.
    async fn taken_back(
        &self,
        id: &ConversationId,
        board: &crate::board::Board,
        held: &PendingMerge,
    ) -> Result<Option<String>> {
        let not_yet = board.questions.values().any(|question| {
            matches!(&question.kind, QuestionKind::Merge { branch, base, .. } if *branch == held.branch && *base == held.base)
                && question.id.to_string() != held.asked_in
                && question.answered_at_ms.is_some_and(|at| at >= held.consented_at_ms)
                && !question
                    .answer
                    .as_deref()
                    .is_some_and(|answer| merge_chosen(answer, &held.base))
        });
        if not_yet {
            return Ok(Some(
                "the user answered \"Not yet\" on the merge card since they consented; don't ask again until they bring it up".into(),
            ));
        }
        let messages = match &board.head {
            Some((head, _)) => self.core.branch(id, head).await?,
            None => self.core.all_messages(id).await?,
        };
        Ok(messages
            .iter()
            .filter(|message| {
                message.role == MessageRole::User
                    && message.id != held.asked_in
                    && message.created_at_ms >= held.consented_at_ms
            })
            .find_map(|message| merge_consent::takes_back(&message.text))
            .map(|why| format!("the user wrote since they consented, and their message {why}")))
    }

    /// The user message that gives consent to the session's merge: the latest on the branch
    /// shown, with `user_words` in it ([`merge_consent::check`]), nothing the user wrote waiting
    /// after it, and no merge asked in it already.
    async fn fresh_consent(
        &self,
        id: &ConversationId,
        user_words: &str,
        branch: &str,
        base: &str,
    ) -> Result<Consent> {
        let refuse = not_consented;
        let board = self.core.board(id).await?;
        let messages = match &board.head {
            Some((head, _)) => self.core.branch(id, head).await?,
            None => self.core.all_messages(id).await?,
        };
        let at = messages
            .iter()
            .rposition(|message| message.role == MessageRole::User);
        let latest = at.map(|at| &messages[at]);
        let wrote_since = |since: i64| {
            board
                .queue
                .items
                .iter()
                .any(|item| item.queued_at_ms.max(item.edited_at_ms.unwrap_or(0)) >= since)
        };
        let used = |asked: &str| spent(&board, asked);
        // The user's latest answer on a merge card, after their latest message, decides: its
        // "Merge" is consent, anything else revokes what their message said.
        let card = board
            .questions
            .values()
            .filter(|question| {
                matches!(&question.kind, QuestionKind::Merge { branch: b, base: a, .. } if b == branch && a == base)
                    && question.answer.is_some()
            })
            .max_by_key(|question| question.answered_at_ms);
        if let Some(card) = card
            && let Some(answered) = card.answered_at_ms
            && latest.is_none_or(|latest| answered >= latest.created_at_ms)
        {
            if !card
                .answer
                .as_deref()
                .is_some_and(|answer| merge_chosen(answer, base))
            {
                return Err(refuse(
                    "the user answered \"Not yet\" on the merge card after their latest message; don't ask again until they bring it up".into(),
                ));
            }
            let asked = card.id.to_string();
            if used(&asked) {
                return Err(refuse(
                    "the user's Merge on the card was already used for a merge; another merge needs their fresh consent".into(),
                ));
            }
            if wrote_since(answered) {
                return Err(refuse(
                    "the user wrote again since (it waits in the queue): read it first".into(),
                ));
            }
            return Ok(Consent {
                asked_in: asked,
                at_ms: answered,
                condition: crate::work::MergeCondition::Now,
            });
        }
        let Some(latest) = latest else {
            return Err(refuse("the user hasn't written anything".into()));
        };
        if wrote_since(latest.created_at_ms) {
            return Err(refuse(
                "the user wrote again since (it waits in the queue): read it first".into(),
            ));
        }
        if used(&latest.id) {
            return Err(refuse(
                "the user's latest message already asked for a merge, and it is done; another merge needs their fresh words".into(),
            ));
        }
        let before = at
            .and_then(|at| at.checked_sub(1))
            .map(|before| &messages[before])
            .filter(|message| message.role == MessageRole::Assistant)
            .map(|message| message.text.as_str());
        let condition =
            merge_consent::check(user_words, &latest.text, before, branch, base).map_err(refuse)?;
        Ok(Consent {
            asked_in: latest.id.clone(),
            at_ms: latest.created_at_ms,
            condition,
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
pub(super) struct Consent {
    /// The user message, or merge card, that gave it.
    asked_in: String,
    /// When they gave it.
    at_ms: i64,
    /// What it waits for ("merge when done").
    condition: crate::work::MergeCondition,
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
}

/// Whether a message the user wrote at or after `at_ms` still waits in the queue.
fn wrote_since(board: &crate::board::Board, at_ms: i64) -> bool {
    board
        .queue
        .items
        .iter()
        .any(|item| item.queued_at_ms.max(item.edited_at_ms.unwrap_or(0)) >= at_ms)
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
    })
    .filter(|held| !spent(board, &held.asked_in))
}

/// Whether the consent given in `asked` was used for a merge, cancelled, or stopped.
pub(super) fn spent(board: &crate::board::Board, asked: &str) -> bool {
    board
        .orchestrator_steps
        .iter()
        .any(|step| match &step.kind {
            OrchestratorStepKind::Merged {
                asked_in: Some(done),
                ..
            } => done == asked,
            OrchestratorStepKind::MergeCancelled { asked_in }
            | OrchestratorStepKind::MergeStopped { asked_in, .. } => asked_in == asked,
            _ => false,
        })
}

/// What a held merge does once no worker writes (SESSION-FIXES-PLAN.md Q3 step 3).
#[derive(Debug, PartialEq)]
pub(super) enum Gate {
    /// Nothing merges: a writer failed or was stopped, or a finding is recorded open.
    Stop(String),
    /// A review still runs, or findings wait to be recorded (their ids).
    Wait(Vec<String>),
    Due,
}

/// [`Gate`] of `held` with every write task final.
pub(super) fn held_gate(held: &PendingMerge, tasks: &[Task], board: &crate::board::Board) -> Gate {
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
    let findings = findings_now(board);
    if !findings.open.is_empty() {
        let open: Vec<String> = findings
            .open
            .iter()
            .map(|(id, title)| format!("{id} is open: {title}"))
            .collect();
        return Gate::Stop(format!("{}.", open.join("; ")));
    }
    if findings.running || !findings.new.is_empty() {
        return Gate::Wait(findings.new);
    }
    Gate::Due
}

fn stopped_text(reason: &str) -> String {
    format!(
        "[merge stopped] {reason} Nothing was merged, and the user's consent is spent: tell them why, and ask what to do."
    )
}

/// The code reviews of the session's work since its previous merge, as the thread recorded
/// their findings.
pub(super) struct FindingsNow {
    /// One still runs.
    pub running: bool,
    /// Recorded open: id and title.
    pub open: Vec<(String, String)>,
    /// Not recorded yet: ids.
    pub new: Vec<String>,
}

pub(super) fn findings_now(board: &crate::board::Board) -> FindingsNow {
    use crate::work::{FindingStatus, ReviewState};
    let reviews = reviews_since_merge(board);
    let items = reviews.iter().flat_map(|review| &review.items);
    FindingsNow {
        running: reviews
            .iter()
            .any(|review| review.state == ReviewState::Running),
        open: items
            .clone()
            .filter(|item| matches!(item.status, FindingStatus::Open { .. }))
            .map(|item| (item.id.clone(), item.title.clone()))
            .collect(),
        new: items
            .filter(|item| item.status == FindingStatus::New)
            .map(|item| item.id.clone())
            .collect(),
    }
}

/// The code reviews the thread hears, started since the session's previous merge.
fn reviews_since_merge(board: &crate::board::Board) -> Vec<&crate::work::ReviewRun> {
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

fn not_consented(why: String) -> Error {
    Error::Invalid(format!(
        "[not merged] {why}. Nothing was merged. Merge only once the user consents: ask with propose_merge and wait for their answer, or when their latest message asks for it in words."
    ))
}

fn wrote_meanwhile() -> Error {
    Error::Invalid(
        "[not merged] The user wrote again while the merge was being prepared: read what they said. Nothing was merged; call finish_session again only if their latest message asks for it.".into(),
    )
}

/// What the code reviews of the work merged now say, for the thread to tell the user: those it
/// hears, started since the session's previous merge. It counts only what the thread recorded
/// (`record_findings`), never a sum of reviews, and lists the open findings in its words.
pub(super) fn merged_findings(board: &crate::board::Board) -> String {
    use crate::work::{FindingStatus, ReviewState};
    let reviews = reviews_since_merge(board);
    if reviews.is_empty() {
        return String::new();
    }
    if reviews
        .iter()
        .any(|review| review.state == ReviewState::Running)
    {
        return " Its review still runs; if it finds anything, you hear it as a [review …] message: record its findings and tell the user then.".into();
    }
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
        return untracked;
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
    let mut text = String::new();
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
    if text.is_empty() {
        text.push_str(" Its review's findings are all fixed or dismissed, as you recorded them.");
    }
    text.push_str(&untracked);
    text
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
            })
            .collect(),
        insertions: stat.insertions,
        deletions: stat.deletions,
    }
}

/// A repo-relative path as reported (`./a/b`, `a/b/`) → `a/b`.
fn normalize(path: &str) -> String {
    let path = path.trim().trim_start_matches("./").trim_end_matches('/');
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

/// The most conflicting files a merge card names.
const CONFLICTS_SHOWN: usize = 3;

/// "3 files conflict with `main`: a.rs, b.rs, c.rs.", the first few named.
fn merge_offer(branch: &str, base: &str, conflicts: Option<&[String]>) -> (String, String) {
    match conflicts {
        None => (
            format!("Merge `{branch}` into `{base}`?"),
            merge_label(base),
        ),
        Some(paths) => (
            format!(
                "Merge `{branch}` into `{base}`? {}",
                conflicts_line(paths, base)
            ),
            MERGE_RESOLVING.to_owned(),
        ),
    }
}

fn conflicts_line(paths: &[String], base: &str) -> String {
    let shown: Vec<String> = paths
        .iter()
        .take(CONFLICTS_SHOWN)
        .map(|path| format!("`{path}`"))
        .collect();
    let more = paths.len().saturating_sub(CONFLICTS_SHOWN);
    match paths.len() {
        0 => format!("It conflicts with `{base}`."),
        1 => format!("1 file conflicts with `{base}`: {}.", shown[0]),
        count if more == 0 => format!(
            "{count} files conflict with `{base}`: {}.",
            shown.join(", ")
        ),
        count => format!(
            "{count} files conflict with `{base}`: {} and {more} more.",
            shown.join(", ")
        ),
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
    fn a_conflict_without_paths_still_offers_resolution_and_old_cards_load() {
        let (text, label) = merge_offer("session", "main", Some(&[]));
        assert!(text.contains("conflicts with `main`"));
        assert_eq!(label, MERGE_RESOLVING);
        let old: QuestionKind = serde_json::from_value(serde_json::json!({
            "type": "merge", "branch": "session", "base": "main"
        }))
        .unwrap();
        assert!(
            matches!(old, QuestionKind::Merge { conflicted: false, conflicts, .. } if conflicts.is_empty())
        );
        let paths = (0..20).map(|i| format!("file-{i}.rs")).collect::<Vec<_>>();
        let (text, _) = merge_offer("session", "main", Some(&paths));
        assert!(text.contains("20 files conflict"));
        assert!(!text.contains("file-19.rs"));
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
