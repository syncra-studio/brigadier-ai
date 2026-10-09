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

use std::path::{Path, PathBuf};

use brigadier_git::{
    ChangeKind, CommitOutcome, LandBlock, LandOutcome, LandRequest, MergeOutcome, Oid,
    PrepareOutcome, SeriesOutcome, litter,
};

use super::cards::{merge_chosen, merge_label};
use super::conversation::Envelope;
use super::workers::Workspace;
use super::{SessionManager, blocking, git_error, merge_consent};
use crate::model::{ConversationId, Environment, MessageRole, Setup};
use crate::work::{
    DecisionKind, DecisionSource, DiffStat, ExcludedFile, FileStat, InjectionKind,
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
        match self.move_commits(&task).await {
            Ok(moved) => Ok(self.settle_landing(&task, moved, false).await.1),
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
        let (landed, text) = match self.move_commits(&task).await {
            Ok(moved) => self.settle_landing(&task, moved, true).await,
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
        let squash = task.kind == crate::work::TaskKind::Merge;
        let on_snapshot = workspace.on_snapshot;
        let leftovers = self.commit_message(format!(
            "{}\n\nWhat task-{} left uncommitted, committed as it landed.",
            task.title, task.number
        ));
        let omit_ai_coauthors = self.core.settings().omit_ai_coauthors;
        let git = self.git.clone();
        let moved = blocking(move || {
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
                let (tip, commits, rebased) = if squash {
                    // A merge task's work is one merge of the target into the conflicting
                    // work: it lands as one commit, built from the files.
                    let changes = match worktree
                        .prepare_candidate(&base, &onto)
                        .map_err(git_error)?
                    {
                        PrepareOutcome::Prepared { changes } => changes,
                        PrepareOutcome::Conflicts { paths } => {
                            return Ok(Moved::Conflicts { onto, paths });
                        }
                    };
                    let include = keep_paths(&changes, &reported, &mut excluded);
                    match worktree
                        .commit_candidate(&include, &leftovers)
                        .map_err(git_error)?
                    {
                        CommitOutcome::Committed { commit, .. } => (commit, 1, false),
                        CommitOutcome::HookFailed { output } => {
                            return Ok(Moved::HookFailed { output });
                        }
                        CommitOutcome::Empty => return Ok(Moved::Nothing { excluded }),
                    }
                } else {
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
                    // 3. Onto the target's tip.
                    match worktree
                        .replay_series(&base, &onto, &drop)
                        .map_err(git_error)?
                    {
                        SeriesOutcome::Conflicts { paths } => {
                            return Ok(Moved::Conflicts { onto, paths });
                        }
                        SeriesOutcome::Replayed { tip, commits, .. } => {
                            // Work started on a snapshot of the user's uncommitted files sits
                            // on the commit the snapshot was taken on.
                            let started = if on_snapshot {
                                repo.resolve(&format!("{}^", base.0)).map_err(git_error)?
                            } else {
                                base.clone()
                            };
                            (tip, commits, started != onto)
                        }
                    }
                };
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
                // The messages as the user wants them, before they land: the worker's branch
                // moves to the rewritten commits, so it is found merged once they have landed.
                let tip = if omit_ai_coauthors {
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
        .await?;
        Ok(moved)
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
        let problem = |reason: String, state: TaskState| async move {
            if on_its_own {
                self.landing_problem(task, &reason, state).await;
            } else {
                self.hand_back(task, state, Some(&reason)).await;
            }
            format!("[not landed task-{}] {reason}", task.number)
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
        };
        (false, text)
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
        // Its one review, by the other vendor, starts now and runs on its own.
        {
            let manager = self.arc();
            let (task, from, tip) = (task.clone(), from.clone(), new_tip.clone());
            self.spawn(async move { manager.review_landing(&task, from, tip).await });
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
        // too.
        for landed in &tasks {
            let branch = landed.workspace.as_ref().and_then(|w| w.branch.clone());
            self.dispose_task(landed, TaskState::Landed).await;
            if let (Some(branch), Ok(repo)) = (branch, self.task_repo(landed)) {
                let git = self.git.clone();
                let into = target.to_owned();
                let deleted = blocking(move || {
                    let repo = git.open(&repo).map_err(git_error)?;
                    // `git branch -d` would compare with the main checkout's HEAD, which is
                    // not the target in a new-worktree session; the merge check here is the
                    // real one. Deleted only at the tip found merged: a commit added since
                    // then stays.
                    if let Some(tip) = repo.branch_tip(&branch).map_err(git_error)?
                        && repo.is_merged(&branch, &into).map_err(git_error)?
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

    /// What a merge worker reads: the task whose work it finishes merging and the conflicts
    /// Brigadier left in its worktree.
    pub(crate) async fn merge_brief(&self, subject: &Task, workspace: &Workspace) -> String {
        let target = workspace.target.clone().unwrap_or_default();
        let merge = match (
            workspace.worktree.clone(),
            subject.workspace.as_ref().and_then(|w| w.base.clone()),
        ) {
            (Some(path), Some(base)) => {
                let git = self.git.clone();
                blocking(move || {
                    let repo = git.open(&path).map_err(git_error)?;
                    let head = repo.resolve("HEAD").map_err(git_error)?;
                    repo.merge_at(&head, &Oid(base)).map_err(git_error)
                })
                .await
                .ok()
                .flatten()
            }
            _ => None,
        };
        let number = subject.number;
        let conflicts = match &merge {
            Some(merge) if merge.clean => {
                "It merged without conflicts: check that both sides still fit together.".to_owned()
            }
            Some(merge) => {
                let sides = format!(
                    "`git show {}:<path>` shows `{target}`'s side and `git show {}:<path>` task-{number}'s (reading is fine)",
                    merge.onto.0, merge.work.0
                );
                if merge.conflicts.is_empty() {
                    format!(
                        "Git reported a conflict it could not pin to one file (for example a folder renamed on one side). Compare both sides: {sides}. Resolve it by editing the files, keeping both sides' intent."
                    )
                } else {
                    format!(
                        "Conflicts in: {}. Text conflicts are marked in the file: the first side (after `<<<<<<<`) is `{target}`'s, the second (before `>>>>>>>`) is task-{number}'s. Binary files, a file deleted on one side, a mode change or a file against a folder carry no markers: {sides}. Resolve each by editing the files, keeping both sides' intent, and leave no marker behind.",
                        merge.conflicts.join(", ")
                    )
                }
            }
            None => "Resolve every conflict it left by editing the files, keeping both sides' intent, and leave no marker behind.".to_owned(),
        };
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
            "\n\nBrigadier has merged the current `{target}` into task-{number}'s work (\"{}\") in this worktree. {conflicts} Make sure it builds and its tests pass. Run no git command that changes anything (no merge, commit, rebase, checkout or reset): Brigadier builds the commit from the files.{reported}\n\nThe original task:\n{}",
            subject.title, subject.spec
        )
    }

    /// A merge task starts from the conflicting task's work (kept as a WIP commit) with the
    /// target's current tip already merged in by Brigadier, outside any sandbox: conflict
    /// markers are left in the files and that tip becomes the task's base, so only the
    /// resolution lands. The worker never needs to write the repository's git directory.
    /// Files the landing's litter guard would leave out stay out of the merge too.
    pub(crate) async fn merge_start(&self, subject: &Task) -> Result<(Oid, Oid)> {
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
        let target = workspace.target.ok_or_else(|| {
            Error::Invalid(format!("task-{} has no target branch", subject.number))
        })?;
        let reported: Vec<String> = subject
            .report
            .as_ref()
            .map(|r| r.changes.iter().map(|p| normalize(p)).collect())
            .unwrap_or_default();
        let git = self.git.clone();
        let wip = format!("WIP: task-{} before merging", subject.number);
        let merged = format!("WIP: task-{} with `{target}` merged in", subject.number);
        let start = blocking(move || {
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
            git.open(&path)
                .map_err(git_error)?
                .merge_for_resolution(&base, &work, &leave_out, &target, &merged)
                .map_err(git_error)
        })
        .await?;
        Ok((start.onto, start.commit))
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
                return Err(Error::Invalid(format!("`{}` moved since this work was verified. Nothing merged; review and verify the new base before merging.", workspace.base)));
            }
            // A base already included in this phase-verified candidate is safe. Otherwise
            // its change must be verified again before it merges.
            let approved_base_tip = base_tip;
            let message = format!("Merge verified overnight work into {}", workspace.base);
            let (commit, base_tip) = match repo.prepare_merge_commit(&workspace.base, &tip, &message).map_err(git_error)? {
                MergeOutcome::Ready { commit, base_tip, .. } => (commit, base_tip),
                MergeOutcome::Conflicts { paths } => return Err(Error::Invalid(format!("The verified work conflicts in: {}. Resolve and verify these files before merging.", paths.join(", ")))),
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
            environment: Environment::NewWorktree { base, branch, .. },
            ..
        }) = self.core.conversation(id)?.setup
        else {
            return Err(Error::Invalid(
                "this session works on a local checkout: its commits are already on the picked branch, and there is nothing to merge".into(),
            ));
        };
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
            let used = board.orchestrator_steps.iter().any(|step| {
                matches!(&step.kind, OrchestratorStepKind::Merged { asked_in: Some(asked), .. } if *asked == card.id.to_string())
            });
            if merge && !used {
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
        let item = QuestionItem {
            text: format!("Merge `{branch}` into `{base}`?"),
            options: vec![
                QuestionOption {
                    label: merge_label(&base),
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
        };
        self.open_round(id, None, kind, vec![item]).await?;
        Ok(format!(
            "Asked the user whether to merge `{branch}` into `{base}`. The answer arrives as an [answer] message; reply with exactly {} or your final answer now.",
            super::prompts::QUIET
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
        let (asked_in, wrote) = {
            let wrote = conv.user_wrote.lock().await;
            let asked_in = self.merge_consent(id, user_words, &branch, &base).await?;
            (asked_in, *wrote)
        };
        // What the thread committed itself gets its review before the branch is merged.
        self.scan_thread_commits(id, self.thread_turn_running(id).await)
            .await;
        let open: Vec<String> = self
            .core
            .tasks(id)
            .await?
            .iter()
            .filter(|t| t.kind.writes() && !t.state.is_final())
            .map(|t| format!("task-{}", t.number))
            .collect();
        if !open.is_empty() {
            return Err(Error::Invalid(format!(
                "write tasks are still open: {}. Land or stop them first.",
                open.join(", ")
            )));
        }
        let message = self.commit_message(
            message
                .filter(|m| !m.trim().is_empty())
                .unwrap_or_else(|| format!("Merge {branch} into {base}")),
        );
        let (git, repo_path, base_name, branch_name) = (
            self.git.clone(),
            PathBuf::from(&repo),
            base.clone(),
            branch.clone(),
        );
        let prepared = blocking(move || {
            let repo = git.open(&repo_path).map_err(git_error)?;
            let outcome = repo
                .prepare_merge(&base_name, &branch_name, &message)
                .map_err(git_error)?;
            match outcome {
                MergeOutcome::Ready {
                    commit, base_tip, ..
                } => {
                    let tip = repo
                        .branch_tip(&branch_name)
                        .map_err(git_error)?
                        .ok_or_else(|| Error::Invalid("the session branch is gone".into()))?;
                    let commits = repo.count_commits(&base_tip, &tip).map_err(git_error)?;
                    Ok(Ok((commit, base_tip, tip, commits)))
                }
                MergeOutcome::Conflicts { paths } => Ok(Err(paths)),
            }
        })
        .await?;
        let (commit, base_tip, session_tip, commits) = match prepared {
            Ok(ready) => ready,
            Err(paths) => {
                return Err(Error::Invalid(format!(
                    "`{branch}` conflicts with the current `{base}` in: {}. Nothing was merged. Brigadier resolves conflicts only between a task and the session branch, not between the session branch and `{base}`: tell the user which files conflict, so they can merge the two branches themselves; call finish_session again if they ask.",
                    paths.join(", ")
                )));
            }
        };
        if commits == 0 {
            return Err(Error::Invalid(format!(
                "`{branch}` has no commits that `{base}` lacks"
            )));
        }
        #[cfg(test)]
        {
            let pause = conv.merge_pause.lock().unwrap().clone();
            if let Some((reached, release)) = pause {
                reached.notify_one();
                release.notified().await;
            }
        }
        // The last look at consent and the landing are one step for what the user writes: a
        // "wait" sent while the merge was prepared stops it; one sent now comes after it.
        let landing = {
            let now = conv.user_wrote.lock().await;
            if *now != wrote {
                return Err(Error::Invalid(
                    "[not merged] The user wrote again while the merge was being prepared: read what they said. Nothing was merged; call finish_session again only if their latest message asks for it.".into(),
                ));
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
                merged_reviews(&board)
            }
            Err(_) => String::new(),
        };
        self.orchestrator_step(
            id,
            OrchestratorStepKind::Merged {
                branch: branch.clone(),
                base: base.clone(),
                commits,
                asked_in: Some(asked_in),
            },
        )
        .await;
        // The session's work is merged: what showed it stops, and its worktree and branch go
        // (THREAD-PLAN.md Q9).
        self.stop_previews(id, "the session was merged").await;
        let released = self.release_merged_session(id).await;
        Ok(format!(
            "[finished] As the user asked: `{branch}` ({commits} commit{}) is merged into `{base}` at {}.{reviews}{released}",
            if commits == 1 { "" } else { "s" },
            short(&new_tip)
        ))
    }

    /// The user message that gives consent to the session's merge: the latest on the branch
    /// shown, with `user_words` in it ([`merge_consent::check`]), nothing the user wrote waiting
    /// after it, and no merge asked in it already.
    async fn merge_consent(
        &self,
        id: &ConversationId,
        user_words: &str,
        branch: &str,
        base: &str,
    ) -> Result<String> {
        let refuse = |why: String| {
            Error::Invalid(format!(
                "[not merged] {why}. Nothing was merged. Merge only once the user consents: ask with propose_merge and wait for their answer, or when their latest message asks for it in words."
            ))
        };
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
        let used = |asked: &str| {
            board.orchestrator_steps.iter().any(|step| {
                matches!(&step.kind, OrchestratorStepKind::Merged { asked_in: Some(done), .. } if done == asked)
            })
        };
        // The user's latest answer on a merge card, after their latest message, decides: its
        // "Merge" is consent, anything else revokes what their message said.
        let card = board
            .questions
            .values()
            .filter(|question| {
                matches!(&question.kind, QuestionKind::Merge { branch: b, base: a } if b == branch && a == base)
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
            return Ok(asked);
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
        merge_consent::check(user_words, &latest.text, before, branch, base).map_err(refuse)?;
        Ok(latest.id.clone())
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

/// What the code reviews of the work merged now say, for the thread to tell the user: those it
/// hears, started since the session's previous merge (the context card shows the same line).
fn merged_reviews(board: &crate::board::Board) -> String {
    use crate::work::{ReviewFor, ReviewKind, ReviewState};
    let since = board
        .orchestrator_steps
        .iter()
        .filter(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
        .map(|step| step.at_ms)
        .max()
        .unwrap_or(0);
    let reviews: Vec<_> = board
        .reviews
        .values()
        .filter(|review| {
            review.kind == ReviewKind::Code
                && review.notify == ReviewFor::Orchestrator
                && review.started_at_ms >= since
        })
        .collect();
    if reviews.is_empty() {
        return String::new();
    }
    let findings: u32 = reviews
        .iter()
        .map(|review| match review.state {
            ReviewState::Findings { count } => count,
            _ => 0,
        })
        .sum();
    if reviews
        .iter()
        .any(|review| review.state == ReviewState::Running)
    {
        " Its review still runs; if it finds anything, you hear it as a [review …] message: tell the user then.".into()
    } else if findings > 0 {
        format!(
            " Its review found {findings}; if you haven't told the user what, and what you did about it, do."
        )
    } else if reviews
        .iter()
        .all(|review| matches!(review.state, ReviewState::Failed { .. }))
    {
        " Its review couldn't run.".into()
    } else {
        " Its review is clean.".into()
    }
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

/// What the orchestrator does about a task whose work conflicts with the target.
fn conflict_step(task: &Task, target: &str) -> String {
    if task.workspace.as_ref().is_some_and(|w| w.on_snapshot) {
        return snapshot_conflict(task.number);
    }
    format!(
        "Delegate a merge task with subject task-{}: Brigadier merges `{target}` into its work and the merge worker resolves the conflicts.",
        task.number
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
