//! Landing accepted work: each accepted write task becomes one clean, reviewed commit on the
//! right branch (PLAN §6 Phase 3; corrections B4, B5, B6, B11).
//!
//! 1. **Build the candidate on the current target tip** (`prepare_candidate`): the worker's
//!    whole work since its base, replayed in its worktree. Conflicts go back to the
//!    orchestrator, which delegates a `merge` task: Brigadier merges the target into the work
//!    with conflict markers in place, and the merge worker only edits files.
//! 2. **Litter guard**: only the worker's reported files and tracked changes are staged; logs,
//!    scratch notes, debug scripts and stray files are left out and listed. Unreported tracked
//!    changes are kept and flagged to the reviewer.
//! 3. **Commit** with a normal `git commit` (the repository's hooks run, the user's identity).
//! 4. **Review** by a model from the other vendor (a `review` task over that exact commit and
//!    the worker's verification). Enforced here, whatever the orchestrator asks. If only one
//!    vendor is available, another model of the same vendor reviews, and the card says so.
//! 5. **Approval**: under "Ask for approval" the user approves the landing on a card.
//! 6. **Land** fast-forward only, never over uncommitted, untracked or ignored files, and only
//!    if the target is still where it was (the git engine checks it right before mutating).
//!    If the target moved, the candidate is replayed on the new tip; unless that replay was
//!    clean and touched none of the same paths, it is reviewed again. Anything unsafe leaves
//!    the task "ready to land" with nothing changed.
//!
//! Finishing a new-worktree session merges the session branch into its base the same way,
//! after the user's one click.

use std::path::{Path, PathBuf};

use brigadier_git::{
    ChangeKind, CommitOutcome, LandBlock, LandOutcome, LandRequest, MergeOutcome, Oid,
    PrepareOutcome, RebaseOutcome, litter,
};
use brigadier_providers::ApprovalDecision;

use super::cards::CardAnswer;
use super::conversation::Envelope;
use super::outputs::outputs_dir;
use super::workers::Workspace;
use super::{SessionManager, blocking, git_error};
use crate::model::{ConversationId, Environment, PermissionLevel, Setup};
use crate::work::{
    ApprovalSubject, Candidate, DiffStat, ExcludedFile, FileStat, InjectionKind, Task, TaskState,
};
use crate::{Error, Result};

/// Diffs up to this size are given to the reviewer inline.
const INLINE_DIFF_BYTES: usize = 24_000;
/// A worker's plan up to this size is given to the reviewer inline.
const PLAN_INLINE_BYTES: usize = 8_000;

impl SessionManager {
    /// `accept_task`: starts the landing pipeline and returns at once. The commit message is
    /// kept: when the gate finds problems, Brigadier sends the worker back and lands its fix
    /// with it, without the orchestrator. `overriding`: the user explicitly said to land it
    /// despite its checks' findings (see [`Self::land_despite_checks`]).
    pub(crate) async fn accept_task(
        &self,
        conversation_id: &ConversationId,
        task: Task,
        message: String,
        overriding: bool,
    ) -> Result<String> {
        if overriding
            && let Some(reply) = self
                .land_despite_checks(conversation_id, &task, &message)
                .await?
        {
            return Ok(reply);
        }
        let number = task.number;
        self.begin_landing(conversation_id, task, message, true)
            .await?
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "task-{number} changed meanwhile; look at it again before accepting it"
                ))
            })
    }

    /// Lands, on the user's word, the change whose last checks found problems, as it is and
    /// without checking it again: the candidate those checks ran on (or one with the same
    /// files, a fix that changed nothing), with the commit message it was built with. Under
    /// "Ask for approval" the user still approves it on its card. `None` when the task has no
    /// such change: it is checked as usual.
    async fn land_despite_checks(
        &self,
        conversation_id: &ConversationId,
        task: &Task,
        message: &str,
    ) -> Result<Option<String>> {
        let message = message.trim().to_owned();
        if message.is_empty() {
            return Err(Error::Invalid("the commit message is empty".into()));
        }
        if task.state != TaskState::Reported
            || super::gates::checks_stand(task) != Some(crate::work::GateOutcome::Failed)
        {
            return Ok(None);
        }
        let (Some(checked), Some(candidate)) = (
            task.gate.as_ref().and_then(|gate| gate.commit.clone()),
            task.candidate.clone(),
        ) else {
            return Ok(None);
        };
        if checked != candidate.commit && !self.same_tree(task, &checked, &candidate.commit).await {
            return Ok(None);
        }
        // An overnight run never lands past failed checks: that stays the user's call, in the
        // morning (PLAN.md §10.8).
        if let Some(run) = &task.run {
            let line = format!(
                "{}: its checks failed, so the run won't land it. Read the findings and decide.",
                super::decisions::worker_name(task)
            );
            if let Err(err) = self
                .wait_on_user(
                    conversation_id,
                    task.request_id.clone(),
                    crate::work::WaitingSource::Run {
                        run_id: run.run_id.clone(),
                        task_id: Some(task.id.clone()),
                    },
                    &line,
                )
                .await
            {
                tracing::warn!(task = %task.id, error = %err, "could not list a refused override");
            }
            return Err(Error::Invalid(format!(
                "task-{n} works for the overnight run, which never lands a change despite failed checks. It is listed for the user. Send it back with the findings (message_worker), or leave it for them.",
                n = task.number
            )));
        }
        // The user's word, not the orchestrator's: they wrote after these findings reached it.
        let user_at_ms = self.user_spoke_at(conversation_id).await;
        if !super::gates::user_spoke_since_checks(task, user_at_ms) {
            return Err(Error::Invalid(format!(
                "task-{n} lands despite its checks' findings only on the user's word, and the user has not written since those findings reached you. Ask the user first, with the findings; only if they tell you to land it anyway, call accept_task for task-{n} with override: true again.",
                n = task.number
            )));
        }
        let later = self.later_request_for(conversation_id, task).await;
        let task = self
            .update_task(conversation_id, &task.id, |t| {
                t.state = TaskState::Reviewing;
                t.blocked_reason = None;
                t.landing = Some(message);
                if let Some(gate) = t.gate.as_mut() {
                    gate.overridden = true;
                    // The same files the round found problems in.
                    gate.commit = Some(candidate.commit.clone());
                }
                if later.is_some() {
                    t.request_id = later;
                }
            })
            .await?;
        // What the worker wrote after its report goes with the outcome, not before it.
        self.hold_late_findings(&task).await;
        let number = task.number;
        let manager = self.arc();
        self.spawn(async move {
            if let Err(err) = manager.approve_and_land(&task).await {
                manager
                    .landing_problem(&task, &err.to_string(), TaskState::Reported)
                    .await;
            }
        });
        Ok(Some(format!(
            "Landing task-{number} as it is, on the user's word, despite its checks' findings (its commit keeps the message it was checked with); the outcome arrives as a message."
        )))
    }

    /// When the user last spoke in the conversation: their newest message, or answer on a
    /// question card.
    async fn user_spoke_at(&self, conversation_id: &ConversationId) -> Option<i64> {
        let wrote = self
            .core
            .list_messages(conversation_id.clone(), None, 20)
            .await
            .ok()
            .and_then(|page| {
                page.messages
                    .iter()
                    .filter(|message| message.role == crate::model::MessageRole::User)
                    .map(|message| message.created_at_ms)
                    .max()
            });
        let answered = self
            .core
            .board(conversation_id)
            .await
            .ok()
            .and_then(|board| {
                board
                    .questions
                    .values()
                    .filter_map(|question| question.answered_at_ms)
                    .max()
            });
        wrote.max(answered)
    }

    /// Starts landing `task`; `fresh` when the orchestrator accepted it (its fix rounds start
    /// over), not when Brigadier lands a fix on its own. `None`, with nothing changed, when
    /// the task moved on from the state it was read in meanwhile (stopped, sent back,
    /// reported again, or no longer Brigadier's to land).
    pub(crate) async fn begin_landing(
        &self,
        conversation_id: &ConversationId,
        task: Task,
        message: String,
        fresh: bool,
    ) -> Result<Option<String>> {
        if !task.kind.writes() {
            return Err(Error::Invalid(format!(
                "task-{} is a {:?} task: only implement and merge tasks land",
                task.number, task.kind
            )));
        }
        let message = message.trim().to_owned();
        if message.is_empty() {
            return Err(Error::Invalid("the commit message is empty".into()));
        }
        match task.state {
            TaskState::Reported => {}
            TaskState::ReadyToLand if task.candidate.is_some() => {}
            state => {
                return Err(Error::Invalid(format!(
                    "task-{} is {state:?}; only a reported task can be accepted",
                    task.number
                )));
            }
        }
        // Only a candidate whose checks all passed lands as it is; one that couldn't be
        // verified is built and checked again.
        let retry = task.state == TaskState::ReadyToLand && super::gates::candidate_passed(&task);
        let later = self.later_request_for(conversation_id, &task).await;
        // Checked and changed in one step: a stop, a steer or a newer report that came after
        // `task` was read wins.
        let still = |now: &Task| {
            now.state == task.state
                && super::workers::same_report(now, &task)
                && now.candidate == task.candidate
                && (fresh || now.landing.is_some())
        };
        let Some(task) = self
            .update_task_if(conversation_id, &task.id, still, |t| {
                t.state = TaskState::Reviewing;
                t.blocked_reason = None;
                t.landing = Some(message.clone());
                if fresh {
                    t.fix_rounds = 0;
                }
                if !retry {
                    t.candidate = None;
                }
                if later.is_some() {
                    t.request_id = later;
                }
            })
            .await?
        else {
            return Ok(None);
        };
        // What the worker wrote after its report goes to its checks, and to the orchestrator
        // with their outcome, not before it.
        self.hold_late_findings(&task).await;
        let number = task.number;
        let manager = self.arc();
        self.spawn(async move {
            let result = if retry {
                manager.land_task(&task).await
            } else {
                manager.build_and_review(&task, message).await
            };
            if let Err(err) = result {
                manager
                    .landing_problem(&task, &err.to_string(), TaskState::Reported)
                    .await;
            }
        });
        Ok(Some(if retry {
            format!("Landing task-{number} again; the outcome arrives as a message.")
        } else {
            format!(
                "Accepted task-{number}. Brigadier builds its commit, has it reviewed by another vendor's model and verified, sends the worker back with any findings, and lands it; the outcome arrives as a message."
            )
        }))
    }

    /// Steps 1–4: candidate, litter guard, commit, review task.
    async fn build_and_review(&self, task: &Task, message: String) -> Result<()> {
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
        let reported = task
            .report
            .as_ref()
            .map(|r| r.changes.clone())
            .unwrap_or_default();

        let git = self.git.clone();
        let (repo_path, path, branch) = (repo.clone(), worktree.clone(), target.clone());
        let commit_message = message.clone();
        let built = blocking(move || {
            let repo = git.open(&repo_path).map_err(git_error)?;
            let onto = repo
                .branch_tip(&branch)
                .map_err(git_error)?
                .ok_or_else(|| Error::Invalid(format!("branch {branch} does not exist")))?;
            let worktree = git.open_worktree(&path).map_err(git_error)?;
            let changes = match worktree
                .prepare_candidate(&base, &onto)
                .map_err(git_error)?
            {
                PrepareOutcome::Prepared { changes } => changes,
                PrepareOutcome::Conflicts { paths } => return Ok(Built::Conflicts { onto, paths }),
            };
            let reported: Vec<String> = reported.iter().map(|p| normalize(p)).collect();
            let mut include = Vec::new();
            let mut excluded = Vec::new();
            let mut unreported = Vec::new();
            for (change, verdict) in litter::classify(&changes, &reported) {
                match verdict {
                    litter::Verdict::Keep => {
                        if !change.untracked && !is_reported(&change.path, &reported) {
                            unreported.push(change.path.clone());
                        }
                        include.push(change.path);
                    }
                    litter::Verdict::Exclude { reason } => excluded.push(ExcludedFile {
                        path: change.path,
                        reason,
                    }),
                }
            }
            match worktree
                .commit_candidate(&include, &commit_message)
                .map_err(git_error)?
            {
                CommitOutcome::Committed { commit, diff_stat } => {
                    let diff = repo.diff(&onto, &commit).map_err(git_error)?;
                    Ok(Built::Committed {
                        onto,
                        commit,
                        diff_stat,
                        excluded,
                        unreported,
                        diff,
                    })
                }
                CommitOutcome::HookFailed { output } => Ok(Built::HookFailed { output }),
                CommitOutcome::Empty => Ok(Built::Empty { excluded }),
            }
        })
        .await?;

        match built {
            Built::Conflicts { onto, paths } => {
                self.landing_problem(
                    task,
                    &format!(
                        "Its changes conflict with the current `{target}` ({}) in: {}. {}",
                        short(&onto),
                        paths.join(", "),
                        conflict_step(task, &target)
                    ),
                    TaskState::Reported,
                )
                .await;
            }
            Built::HookFailed { output } => {
                self.landing_problem(
                    task,
                    &format!(
                        "The repository's commit hooks refused the commit:\n{}\nSend task-{} back with message_worker to fix this.",
                        clip(&output, 3_000),
                        task.number
                    ),
                    TaskState::Reported,
                )
                .await;
            }
            Built::Empty { excluded } => {
                let note = if excluded.is_empty() {
                    String::new()
                } else {
                    format!(
                        " Left out as litter: {}.",
                        excluded
                            .iter()
                            .map(|e| e.path.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                self.announcing(task).await;
                self.dispose_task(task, TaskState::Done).await;
                self.deliver(
                    &task.conversation_id,
                    Envelope {
                        kind: InjectionKind::Decision,
                        label: format!("task-{} empty", task.number),
                        task_id: Some(task.id.clone()),
                        text: format!("[nothing to land task-{}] The task changed nothing that could be committed.{note}", task.number),
                    },
                )
                .await;
            }
            Built::Committed {
                onto,
                commit,
                diff_stat,
                excluded,
                unreported,
                diff,
            } => {
                let live = self.existing_task_live(&task.id);
                let redactor = match live {
                    Some(live) => live.redactor().await,
                    None => None,
                };
                let diff = match redactor {
                    Some(redactor) => redactor.redact(&diff).into_owned(),
                    None => diff,
                };
                let diff_bytes = diff.len() as u64;
                let hash = self.core.store().blobs().put(diff.into_bytes()).await?;
                let candidate = Candidate {
                    commit: commit.0.clone(),
                    onto: onto.0.clone(),
                    message: message.clone(),
                    diff_stat: diff_stat_of(&diff_stat),
                    excluded,
                    diff: Some(crate::work::ArtifactRef {
                        id: hash.to_string(),
                        title: format!("Candidate commit of task-{}", task.number),
                        kind: crate::work::ArtifactKind::Diff,
                        mime: "text/x-diff".into(),
                        bytes: diff_bytes,
                        file_name: Some(format!("task-{}-candidate.diff", task.number)),
                    }),
                };
                let task = self
                    .update_task(&task.conversation_id, &task.id, |t| {
                        t.candidate = Some(candidate);
                        // Later work of this worker is relative to the candidate's parent.
                        if let Some(workspace) = t.workspace.as_mut() {
                            workspace.base = Some(onto.0.clone());
                            workspace.on_snapshot = false;
                        }
                    })
                    .await?;
                // A held change accepted again unchanged keeps its reviews.
                let reverify = self.reverify_held(&task, &commit.0).await;
                let recheck = if reverify.is_some() {
                    super::gates::Recheck::Verify
                } else {
                    super::gates::Recheck::Full
                };
                match self.open_gate(&task, unreported, recheck, reverify).await {
                    Ok(()) => {}
                    Err(super::gates::NotOpened::Error(err)) => return Err(err),
                    Err(super::gates::NotOpened::Unchanged) => {
                        self.escalate_unchanged(&task).await;
                    }
                }
            }
        }
        Ok(())
    }

    /// What a reviewer reads about the change: the task, what the worker was told since, the
    /// worker's report, the diff. `scratch` is the reader's own scratch folder.
    pub(crate) async fn review_brief(&self, subject: &Task, scratch: &Path) -> String {
        // What the worker wrote after its report may come in after its checks were opened.
        let addendum = match &subject.addendum {
            Some(addendum) => Some(addendum.clone()),
            None => self
                .task_by_id(&subject.conversation_id, &subject.id)
                .await
                .ok()
                .filter(|now| now.candidate == subject.candidate)
                .and_then(|now| now.addendum),
        };
        let mut text = brief_history(subject);
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
            if let Some(addendum) = addendum {
                let addendum = self.late_findings_for_checker(&addendum, scratch).await;
                text.push_str(&format!(
                    "\n\nWhat the worker wrote after its report:\n{addendum}"
                ));
            }
        }
        // A worker on a big change writes its plan first; the change is checked against it.
        if let Some(workspace) = &subject.workspace {
            let plan = outputs_dir(Path::new(&workspace.scratch)).join("plan.md");
            if let Ok(plan_text) = tokio::fs::read_to_string(&plan).await
                && !plan_text.trim().is_empty()
            {
                let plan_text = if plan_text.len() > PLAN_INLINE_BYTES {
                    let mut end = PLAN_INLINE_BYTES;
                    while !plan_text.is_char_boundary(end) {
                        end -= 1;
                    }
                    format!(
                        "{}\n[… cut; the full plan is {}]",
                        &plan_text[..end],
                        plan.display()
                    )
                } else {
                    plan_text
                };
                text.push_str(&format!(
                    "\n\nThe worker's plan (plan.md), which the change should follow:\n{plan_text}"
                ));
            }
        }
        if let Some(candidate) = &subject.candidate {
            if !candidate.excluded.is_empty() {
                text.push_str("\n\nLeft out of the commit as litter:");
                for file in &candidate.excluded {
                    text.push_str(&format!("\n- {} ({})", file.path, file.reason));
                }
            }
            text.push_str(&format!(
                "\n\nYour checkout is at the candidate commit; its parent is {}. See the change with `git show --stat HEAD` and `git diff HEAD~1`.",
                short(&Oid(candidate.onto.clone()))
            ));
            if let Some(diff) = &candidate.diff
                && diff.bytes as usize <= INLINE_DIFF_BYTES
                && let Ok(text_diff) = self.core.read_blob_text(diff.id.clone()).await
            {
                text.push_str(&format!("\n\nThe diff:\n```diff\n{text_diff}\n```"));
            }
        }
        text
    }

    /// What the worker wrote after its report, as a checker reads it: the whole of each part
    /// cut to a report's size is written into the checker's scratch folder, and the cut
    /// points at that file (a checker has no read_artifact).
    async fn late_findings_for_checker(&self, held: &str, scratch: &Path) -> String {
        let mut shown = held.to_owned();
        for (n, (note, id)) in super::prompts::late_findings_cuts(held)
            .into_iter()
            .enumerate()
        {
            let Ok(whole) = self.core.read_blob_text(id.to_owned()).await else {
                continue;
            };
            let path = scratch.join(format!("after-report-{}.md", n + 1));
            if let Err(err) = tokio::fs::write(&path, whole).await {
                tracing::warn!(path = %path.display(), error = %err, "could not give a checker what the worker wrote after its report");
                continue;
            }
            shown = shown.replacen(note, &super::prompts::late_findings_file_note(&path), 1);
        }
        shown
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

    /// Step 5 (the user's approval under "Ask for approval"), then step 6.
    pub(super) async fn approve_and_land(&self, task: &Task) -> Result<()> {
        let permission = self.permission(&task.conversation_id);
        if permission == PermissionLevel::AskForApproval && task.run.is_none() {
            let candidate = task
                .candidate
                .clone()
                .ok_or_else(|| Error::Invalid("no candidate".into()))?;
            self.set_task_state(&task.conversation_id, &task.id, TaskState::AwaitingApproval)
                .await?;
            let (_, rx) = self
                .open_approval(
                    &task.conversation_id,
                    Some(task.id.clone()),
                    ApprovalSubject::Landing {
                        task_id: task.id.clone(),
                        branch: task
                            .workspace
                            .as_ref()
                            .and_then(|w| w.target.clone())
                            .unwrap_or_default(),
                        diff_stat: candidate.diff_stat.clone(),
                    },
                )
                .await?;
            let answer = rx.await;
            // The answer is about this round's commit: once the task moved on (a newer round,
            // sent back, stopped), it decides nothing.
            let now = self.task_by_id(&task.conversation_id, &task.id).await?;
            if !super::gates::round_current(task, &now) {
                return Ok(());
            }
            match answer {
                Ok(CardAnswer::Decision(ApprovalDecision::Allow)) => {}
                Ok(CardAnswer::Decision(ApprovalDecision::Deny { message })) => {
                    self.announcing(task).await;
                    let addendum = self.hand_back(task, TaskState::Reported, None).await;
                    self.deliver(
                        &task.conversation_id,
                        Envelope {
                            kind: InjectionKind::Decision,
                            label: format!("landing task-{} declined", task.number),
                            task_id: Some(task.id.clone()),
                            text: format!(
                                "[decision] The user declined landing task-{}{}. Nothing landed.{addendum}",
                                task.number,
                                if message.trim().is_empty() {
                                    String::new()
                                } else {
                                    format!(": {message}")
                                }
                            ),
                        },
                    )
                    .await;
                    return Ok(());
                }
                _ => return Err(Error::Invalid("the landing approval was withdrawn".into())),
            }
        }
        self.land_task(task).await
    }

    /// Step 6: lands the candidate. When the target moved, the candidate is replayed onto it
    /// and the new commit goes through the gate again before it lands. `decided` is the task
    /// as its round was decided (or its landing approved): only that round's commit lands,
    /// and only once it passed.
    pub(super) async fn land_task(&self, decided: &Task) -> Result<()> {
        let task = self
            .task_by_id(&decided.conversation_id, &decided.id)
            .await?;
        if !super::gates::round_current(decided, &task) {
            tracing::info!(task = %task.id, "dropped the landing of a round the task moved on from");
            return Ok(());
        }
        if !super::gates::candidate_passed(&task) {
            return Err(Error::Invalid(format!(
                "Its current change has not passed its checks, so it did not land. Call accept_task for task-{} again to check and land it.",
                task.number
            )));
        }
        let candidate = task
            .candidate
            .clone()
            .ok_or_else(|| Error::Invalid("no candidate".into()))?;
        let target = task
            .workspace
            .as_ref()
            .and_then(|w| w.target.clone())
            .ok_or_else(|| Error::Invalid("no target branch".into()))?;
        let (git, repo) = (self.git.clone(), self.task_repo(&task)?);
        let request = LandRequest {
            branch: target.clone(),
            expected_tip: Oid(candidate.onto.clone()),
            commit: Oid(candidate.commit.clone()),
        };
        let outcome = blocking(move || {
            git.open(&repo)
                .map_err(git_error)?
                .land(&request)
                .map_err(git_error)
        })
        .await?;
        match outcome {
            LandOutcome::Landed { new_tip } => {
                self.landed(&task, &target, &new_tip).await;
                Ok(())
            }
            LandOutcome::Blocked(LandBlock::TipMoved { actual }) => {
                let worktree = task
                    .workspace
                    .as_ref()
                    .and_then(|w| w.worktree.clone())
                    .map(PathBuf::from)
                    .ok_or_else(|| Error::Invalid("no worktree".into()))?;
                let (git, old, new) = (
                    self.git.clone(),
                    Oid(candidate.onto.clone()),
                    actual.clone(),
                );
                let commit = Oid(candidate.commit.clone());
                let rebased = blocking(move || {
                    git.open_worktree(&worktree)
                        .map_err(git_error)?
                        .rebase_candidate(&commit, &old, &new)
                        .map_err(git_error)
                })
                .await?;
                match rebased {
                    RebaseOutcome::Rebased { commit, clean_fast } => {
                        let task = self
                            .update_task(&task.conversation_id, &task.id, |t| {
                                if let Some(c) = t.candidate.as_mut() {
                                    c.commit = commit.0.clone();
                                    c.onto = actual.0.clone();
                                }
                                if let Some(w) = t.workspace.as_mut() {
                                    w.base = Some(actual.0.clone());
                                    w.on_snapshot = false;
                                }
                            })
                            .await?;
                        // A change the user had land despite its findings lands as it is
                        // after a clean replay too.
                        if clean_fast && task.gate.as_ref().is_some_and(|gate| gate.overridden) {
                            let task = self
                                .update_task(&task.conversation_id, &task.id, |t| {
                                    if let Some(gate) = t.gate.as_mut() {
                                        gate.commit = Some(commit.0.clone());
                                    }
                                })
                                .await?;
                            return Box::pin(self.land_task(&task)).await;
                        }
                        // A new commit is verified again before it lands, and reviewed
                        // again too when the replay touched paths the target also changed
                        // (B11).
                        let recheck = if clean_fast {
                            super::gates::Recheck::Verify
                        } else {
                            super::gates::Recheck::Full
                        };
                        match self.open_gate(&task, Vec::new(), recheck, None).await {
                            Err(super::gates::NotOpened::Error(err)) => Err(err),
                            _ => Ok(()),
                        }
                    }
                    RebaseOutcome::Conflicts { paths } => {
                        self.landing_problem(
                            &task,
                            &format!(
                                "`{target}` moved and now conflicts with it in: {}. {}",
                                paths.join(", "),
                                conflict_step(&task, &target)
                            ),
                            TaskState::Reported,
                        )
                        .await;
                        Ok(())
                    }
                }
            }
            LandOutcome::Blocked(block) => {
                self.landing_problem(
                    &task,
                    &format!(
                        "It is ready to land, but landing now is not safe: {block} Nothing was changed. Call accept_task for task-{} again once that is resolved.",
                        task.number
                    ),
                    TaskState::ReadyToLand,
                )
                .await;
                Ok(())
            }
        }
    }

    async fn landed(&self, task: &Task, target: &str, new_tip: &Oid) {
        let updated = self
            .update_task(&task.conversation_id, &task.id, |t| {
                t.landed = Some(new_tip.0.clone());
            })
            .await;
        // Its report enters the Brain now, at the commit that landed.
        if let Ok(updated) = &updated
            && let Some(report) = &updated.report
        {
            self.learn_report(updated, report, None);
        }
        // The task branch is fully on the target now; it was Brigadier's, so it goes too.
        let branch = task.workspace.as_ref().and_then(|w| w.branch.clone());
        // The landed envelope follows the cleanup below.
        self.announcing(task).await;
        self.dispose_task(task, TaskState::Landed).await;
        if let (Some(branch), Ok(repo)) = (branch, self.task_repo(task)) {
            let git = self.git.clone();
            let into = target.to_owned();
            let deleted = blocking(move || {
                let repo = git.open(&repo).map_err(git_error)?;
                // `git branch -d` would compare with the main checkout's HEAD, which is not
                // the target in a new-worktree session; the merge check here is the real one.
                // Deleted only at the tip found merged: a commit added since then stays.
                if let Some(tip) = repo.branch_tip(&branch).map_err(git_error)?
                    && repo.is_merged(&branch, &into).map_err(git_error)?
                {
                    repo.delete_branch_at(&branch, &tip).map_err(git_error)?;
                }
                Ok(())
            })
            .await;
            if let Err(err) = deleted {
                tracing::warn!(task = %task.id, error = %err, "could not delete the landed task branch");
            }
        }
        // Landed on the user's word, despite what its checks found.
        if task.gate.as_ref().is_some_and(|gate| gate.overridden) {
            let findings = super::gates::one_line_findings(&self.gate_findings(task).await);
            self.decided_for_task(
                task,
                format!("{} on the user's word", landed_line(task, target)),
                "Landed despite its checks' findings.".to_owned(),
            )
            .await;
            self.deliver(
                &task.conversation_id,
                Envelope {
                    kind: InjectionKind::Decision,
                    label: format!("landed task-{}", task.number),
                    task_id: Some(task.id.clone()),
                    text: format!(
                        "[landed task-{}] Commit {} is on `{target}`. It landed on the user's word despite its checks' findings: {findings}",
                        task.number,
                        short(new_tip),
                    ),
                },
            )
            .await;
            return;
        }
        let review = task
            .review
            .as_ref()
            .map(|r| {
                if r.cross_vendor {
                    "reviewed by another vendor"
                } else {
                    "reviewed by another model of the same vendor (only one vendor was available)"
                }
            })
            .unwrap_or("reviewed");
        let fixes = fixes_made(task);
        self.decided_for_task(
            task,
            landed_line(task, target),
            format!(
                "{} and verified{}.",
                match task.review.as_ref().map(|r| r.cross_vendor) {
                    Some(true) => "Reviewed by another vendor",
                    Some(false) => "Reviewed by the same vendor (the only one available)",
                    None => "Reviewed",
                },
                match fixes {
                    0 => String::new(),
                    1 => ", after 1 fix round".into(),
                    rounds => format!(", after {rounds} fix rounds"),
                }
            ),
        )
        .await;
        self.deliver(
            &task.conversation_id,
            Envelope {
                kind: InjectionKind::Decision,
                label: format!("landed task-{}", task.number),
                task_id: Some(task.id.clone()),
                text: format!(
                    "[landed task-{}] Commit {} is on `{target}` ({review}, and verified{}).",
                    task.number,
                    short(new_tip),
                    match fixes {
                        0 => String::new(),
                        1 => "; Brigadier had the worker fix the checks' findings once".into(),
                        rounds => format!(
                            "; Brigadier had the worker fix the checks' findings {rounds} times"
                        ),
                    }
                ),
            },
        )
        .await;
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
        let addendum = self.hand_back(task, state, Some(reason)).await;
        self.deliver(
            &task.conversation_id,
            Envelope {
                kind: InjectionKind::Decision,
                label: format!("landing task-{}", task.number),
                task_id: Some(task.id.clone()),
                text: format!(
                    "[not landed task-{} \"{}\"] {reason}{addendum}",
                    task.number, task.title
                ),
            },
        )
        .await;
    }

    /// A landing ends without landing and the orchestrator decides next: the task goes to
    /// `state` (`blocked` is why, for a task ready to land), Brigadier no longer lands it on
    /// its own, and what its worker wrote after its report, held meanwhile, is returned as a
    /// block for the orchestrator (empty when none was held).
    async fn hand_back(&self, task: &Task, state: TaskState, blocked: Option<&str>) -> String {
        let mut addendum = None;
        // A task stopped meanwhile, or being stopped, stays stopped.
        let updated = self
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
                    addendum = t.addendum.take();
                },
            )
            .await;
        match (updated, addendum) {
            (Ok(Some(updated)), Some(addendum)) => format!(
                "\n{}",
                super::prompts::late_findings_envelope(&updated, &addendum)
            ),
            _ => String::new(),
        }
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
            // its change requires a new candidate and fresh whole-phase checks.
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

    /// `finish_session`: merges the session branch into its base after the user's click.
    pub(crate) async fn finish_session(
        &self,
        id: &ConversationId,
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
        let message = message
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| format!("Merge {branch} into {base}"));
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
                    commit,
                    base_tip,
                    fast_forward,
                } => {
                    let tip = repo
                        .branch_tip(&branch_name)
                        .map_err(git_error)?
                        .ok_or_else(|| Error::Invalid("the session branch is gone".into()))?;
                    let commits = repo.count_commits(&base_tip, &tip).map_err(git_error)?;
                    let stat = repo.diff_stat(&base_tip, &commit).map_err(git_error)?;
                    Ok(Ok((commit, base_tip, tip, fast_forward, commits, stat)))
                }
                MergeOutcome::Conflicts { paths } => Ok(Err(paths)),
            }
        })
        .await?;
        let (commit, base_tip, session_tip, _fast_forward, commits, stat) = match prepared {
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
        let (approval, rx) = self
            .open_approval(
                id,
                None,
                ApprovalSubject::FinishSession {
                    branch: branch.clone(),
                    base: base.clone(),
                    commits,
                    diff_stat: diff_stat_of(&stat),
                },
            )
            .await?;
        let manager = self.arc();
        let id = id.clone();
        self.spawn(async move {
            let text = match rx.await {
                Ok(CardAnswer::Decision(ApprovalDecision::Allow)) => {
                    let (git, repo_path, session_branch) =
                        (manager.git.clone(), PathBuf::from(&repo), branch.clone());
                    let request = LandRequest {
                        branch: base.clone(),
                        expected_tip: base_tip,
                        commit,
                    };
                    // What the user approved is the session branch as it was: work that landed
                    // on it while the card was open would be left out.
                    let landing = blocking(move || {
                        let repo = git.open(&repo_path).map_err(git_error)?;
                        if repo.branch_tip(&session_branch).map_err(git_error)? != Some(session_tip) {
                            return Ok(None);
                        }
                        repo.land(&request).map(Some).map_err(git_error)
                    });
                    match landing.await {
                        Ok(None) => format!("[not finished] `{branch}` changed after the user was asked. Nothing was merged; call finish_session again."),
                        Ok(Some(LandOutcome::Landed { new_tip })) => format!(
                            "[finished] The user approved: `{branch}` ({commits} commit{}) is merged into `{base}` at {}.",
                            if commits == 1 { "" } else { "s" },
                            short(&new_tip)
                        ),
                        Ok(Some(LandOutcome::Blocked(block))) => format!("[not finished] Merging `{branch}` into `{base}` is not safe now: {block} Nothing was changed; call finish_session again."),
                        Err(err) => format!("[not finished] Merging failed: {err}. Nothing was changed."),
                    }
                }
                Ok(CardAnswer::Decision(ApprovalDecision::Deny { message })) => format!(
                    "[decision] The user did not merge `{branch}` into `{base}`{}.",
                    if message.trim().is_empty() { String::new() } else { format!(": {message}") }
                ),
                _ => {
                    manager
                        .settle_approval(&approval, crate::work::CardState::Expired { reason: "withdrawn".into() })
                        .await;
                    return;
                }
            };
            manager
                .deliver_for(
                    &id,
                    Envelope {
                        kind: InjectionKind::Decision,
                        label: "finish session".into(),
                        task_id: None,
                        text,
                    },
                    approval.request_id.clone(),
                )
                .await;
        });
        Ok("Asked the user to approve merging the session branch; the outcome arrives as a message.".into())
    }

    pub(super) fn task_repo(&self, task: &Task) -> Result<PathBuf> {
        match self.core.conversation(&task.conversation_id)?.setup {
            Some(Setup::Session { repo, .. }) => Ok(PathBuf::from(repo)),
            _ => Err(Error::Invalid("tasks belong to a session".into())),
        }
    }
}

enum Built {
    Conflicts {
        onto: Oid,
        paths: Vec<String>,
    },
    HookFailed {
        output: String,
    },
    Empty {
        excluded: Vec<ExcludedFile>,
    },
    Committed {
        onto: Oid,
        commit: Oid,
        diff_stat: brigadier_git::DiffStat,
        excluded: Vec<ExcludedFile>,
        unreported: Vec<String>,
        diff: String,
    },
}

/// The task a change implements and what its worker was told since: the orchestrator's
/// messages, and the findings Brigadier sent it back with.
fn brief_history(subject: &Task) -> String {
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
    if !subject.fixes.is_empty() {
        text.push_str(&format!(
            "\n\nWhat earlier checks of its change found, which Brigadier sent the worker back to fix, oldest first. Each time it told the worker: \"{}\"",
            super::gates::SEND_BACK
        ));
        for (index, findings) in subject.fixes.iter().enumerate() {
            text.push_str(&format!("\n--- fix {}\n{findings}", index + 1));
        }
    }
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

fn is_reported(path: &str, reported: &[String]) -> bool {
    reported.iter().any(|r| {
        path == r || path.starts_with(&format!("{r}/")) || r.ends_with(&format!("/{path}"))
    })
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

/// The fixes Brigadier had the worker make to its change, also those before the orchestrator
/// accepted it again (after a hold or a hand-back): a fresh accept starts `fix_rounds` over,
/// `fixes` keeps every one.
fn fixes_made(task: &Task) -> usize {
    task.fixes.len().max(task.fix_rounds as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_of_a_fix_read_what_the_worker_was_sent_back_with() {
        let mut task: Task = serde_json::from_value(serde_json::json!({
            "id": "t1",
            "conversationId": "c1",
            "number": 1,
            "position": 0,
            "title": "Add avg2",
            "kind": "implement",
            "spec": "Add avg2 to src/math.js.",
            "access": { "repo": "write", "network": false, "unsandboxed": false },
            "route": { "choice": { "provider": "claude", "model": null, "effort": null }, "reason": "" },
            "state": "reviewing",
            "attachments": [],
            "createdAtMs": 0,
            "updatedAtMs": 0
        }))
        .expect("a task");
        let first = brief_history(&task);
        assert!(first.contains("Add avg2 to src/math.js."), "{first}");
        assert!(!first.contains("orchestrator told"), "{first}");
        assert!(!first.contains("Brigadier sent"), "{first}");
        task.messages = vec!["src/index.js may change too.".into()];
        task.fixes = vec![
            "From the review (task-2):\n- Re-export avg2 from src/index.js".into(),
            "From the verification (task-5):\n- [not met] npm test passes".into(),
        ];
        let text = brief_history(&task);
        assert!(text.contains("src/index.js may change too."), "{text}");
        assert!(text.contains(super::super::gates::SEND_BACK), "{text}");
        assert!(
            text.contains("--- fix 1\nFrom the review (task-2):\n- Re-export avg2"),
            "{text}"
        );
        assert!(text.contains("--- fix 2\nFrom the verification"), "{text}");
        // Accepted again after a hold: its fix rounds start over, the fixes made still count.
        task.fix_rounds = 0;
        assert_eq!(fixes_made(&task), 2);
        task.fix_rounds = 1;
        task.fixes.clear();
        assert_eq!(fixes_made(&task), 1);
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
