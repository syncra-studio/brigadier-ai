//! Gates: the independent checks every accepted change passes before it lands (PLAN §6
//! Phase 6, built in, always on).
//!
//! A gate round runs, in parallel and on one candidate commit, a reviewer from another vendor
//! (two for risky work, on different models) and a verifier that runs the project's checks
//! and proves each "done when" criterion of the task. Each member's result is recorded against
//! its round, and the round is decided once every member has one:
//!
//! - **All passed:** the change lands (after the user's click under "Ask for approval").
//! - **Changes needed** (a reviewer asked for them, or a criterion is unmet): Brigadier sends
//!   the worker the findings itself and gates its next report again, at most [`FIX_ROUNDS`]
//!   times. Then the orchestrator decides.
//! - **Not verified** (checks that couldn't run, criteria left unchecked): a second verifier
//!   on another model tries once. If it can't either, nothing lands: the task waits, ready to
//!   land, with what blocks it.
//! - **No result** (a member failed or was stopped, or the verifier changed its checkout):
//!   nothing lands, and the orchestrator hears why.
//!
//! A newer candidate (a fix, or a replay onto a target that moved) opens a new round, with a
//! new verification; results of an older round are ignored. A round whose members all ended
//! but that was never decided is settled by the stall watchdog ([`super::watchdog`]).

use brigadier_git::Oid;
use brigadier_router::Author;

use super::conversation::Envelope;
use super::{SessionManager, blocking, git_error};
use crate::model::ModelChoice;
use crate::work::{
    Gate, GateLink, GateMember, GateOutcome, GateOwner, GateResult, GateRole, InjectionKind,
    Report, ReviewRecord, ReviewVerdict, Task, TaskId, TaskKind, TaskState, WaitingSource,
};
use crate::{Error, Result};

/// Times Brigadier sends a task back with a gate's findings before the orchestrator decides.
pub(crate) const FIX_ROUNDS: u32 = 2;
/// What Brigadier tells a worker it sends back with its checks' findings, before them.
pub(super) const SEND_BACK: &str = "Independent checks of your change found problems, so nothing landed. Fix each one in this worktree, verify the fix for real, then call submit_report again with a complete report (all fields, as before).";
/// Gate rounds one task may go through (fixes, retries, replays) before Brigadier gives up.
const MAX_ROUNDS: u32 = 8;

/// What a new gate round checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Recheck {
    /// Reviewers and a verifier: a new candidate.
    Full,
    /// A verifier only: the same change, already reviewed, on a new commit (a clean replay
    /// onto a target that moved) or verified again after a verifier couldn't.
    Verify,
}

/// Why a gate round could not open.
pub(crate) enum NotOpened {
    /// The worker's fix round changed nothing: the orchestrator decides.
    Unchanged,
    Error(Error),
}

impl From<Error> for NotOpened {
    fn from(err: Error) -> Self {
        Self::Error(err)
    }
}

impl SessionManager {
    /// How many reviewers check a change (user decision, 2026-10-04): none for a change to
    /// documentation only, which its verifier checks against the code; two for a change to a
    /// risky area (landing, policy, the sandbox, git); one otherwise. A fix round asks again
    /// only the reviewers that asked for changes: the others' approvals are kept, and its
    /// verifier checks the fix against them ([`kept_approvals`]).
    fn panel_size(task: &Task) -> usize {
        let Some(candidate) = &task.candidate else {
            return 1;
        };
        let files = &candidate.diff_stat.files;
        if docs_only(files) {
            return 0;
        }
        let needed = if files.iter().any(|file| risky_path(&file.path)) {
            2
        } else {
            1
        };
        if let Some(previous) = &task.gate
            && previous.outcome == Some(GateOutcome::Failed)
        {
            let reviewers = previous
                .members
                .iter()
                .filter(|m| m.role == GateRole::Review);
            // A round that had no reviewer (documentation then) approved nothing to keep.
            if reviewers.clone().count() == 0 {
                return needed;
            }
            return reviewers
                .filter(|m| matches!(m.result, Some(GateResult::Failed { .. })))
                .count();
        }
        needed
    }

    /// Opens a new gate round on the task's candidate. `unreported` lists tracked changes the
    /// worker didn't report; `retry` says why an earlier verifier couldn't check the change.
    /// `relanding`: the user already approved this change (a clean replay, and a retry of its
    /// verification); it lands as soon as the round passes.
    pub(crate) async fn open_gate(
        &self,
        task: &Task,
        unreported: Vec<String>,
        recheck: Recheck,
        retry: Option<(String, Author)>,
    ) -> std::result::Result<(), NotOpened> {
        let held = self.gates.lock().await;
        let task = self.task_by_id(&task.conversation_id, &task.id).await?;
        let candidate = task
            .candidate
            .clone()
            .ok_or_else(|| Error::Invalid("no candidate".into()))?;
        let round = task.gate.as_ref().map_or(1, |gate| gate.round + 1);
        if round > MAX_ROUNDS {
            return Err(Error::Invalid(format!(
                "Its change was checked {MAX_ROUNDS} times without landing; nothing landed. Look at the last findings and decide."
            ))
            .into());
        }
        // A fix round that changed nothing would only be checked to the same end.
        if let Some(previous) = &task.gate
            && previous.outcome == Some(GateOutcome::Failed)
            && let Some(before) = &previous.commit
            && self.same_tree(&task, before, &candidate.commit).await
        {
            return Err(NotOpened::Unchanged);
        }
        let author = Author {
            provider: task.route.choice.provider,
            model: task.route.choice.model.clone(),
        };
        let owner = GateOwner::Task {
            task_id: task.id.clone(),
        };
        // Its checkers get the session's access (see `access_for`).
        let sandboxed =
            self.permission(&task.conversation_id) != crate::model::PermissionLevel::FullAccess;
        let reviewers = match recheck {
            Recheck::Full => Self::panel_size(&task),
            Recheck::Verify => 0,
        };
        let docs = task
            .candidate
            .as_ref()
            .is_some_and(|candidate| docs_only(&candidate.diff_stat.files));
        let kept = match recheck {
            Recheck::Full => kept_approvals(&task),
            Recheck::Verify => None,
        };
        // A big change without a plan is noted once, on its first round: a fix the checks
        // asked for may make it bigger, and the plan can't be added to the change anyway.
        let plan = plan_note(&task, self.wrote_plan(&task).await, task.gate.is_none());
        let mut members: Vec<GateMember> = Vec::new();
        let mut started: Vec<Task> = Vec::new();
        let mut checking: Vec<Author> = Vec::new();
        let opened: Result<Option<ReviewRecord>> = async {
            let mut first = None;
            // The verifier first: it is admitted first, so checks of one change never wait on
            // each other's slots (PLAN.md §10.7).
            let verify = self
                .create_task(
                    &task.conversation_id,
                    format!("Verify task-{}", task.number),
                    TaskKind::Verify,
                    if docs {
                        verify_docs_spec(&task, &candidate.commit, kept.as_deref())
                    } else {
                        let mut spec = verify_spec(
                            &task,
                            &candidate.commit,
                            retry.as_ref().map(|(why, _)| why),
                            plan.as_deref(),
                            sandboxed,
                        );
                        if let Some(kept) = &kept {
                            spec.push_str(&format!("\n{kept}"));
                        }
                        spec
                    },
                    None,
                    Some(author.clone()),
                    retry.iter().map(|(_, before)| before.clone()).collect(),
                    Some(GateLink {
                        owner: owner.clone(),
                        round,
                        role: GateRole::Verify,
                    }),
                    Some(task.clone()),
                    Vec::new(),
                    Some(task.areas.clone()),
                    // Proving a change takes a capable model, not a light one.
                    Some(brigadier_router::QualityTier::Strong),
                    Vec::new(),
                )
                .await?;
            members.push(GateMember {
                task_id: verify.id.clone(),
                role: GateRole::Verify,
                result: None,
                // A hand-off of the second verifier avoids the first one too.
                avoid: retry
                    .iter()
                    .map(|(_, before)| ModelChoice {
                        provider: before.provider,
                        model: before.model.clone(),
                        effort: None,
                        fast: None,
                    })
                    .collect(),
            });
            started.push(verify);
            for index in 0..reviewers {
                let review = self
                    .create_task(
                        &task.conversation_id,
                        if index == 0 {
                            format!("Review task-{}", task.number)
                        } else {
                            format!("Second review of task-{}", task.number)
                        },
                        TaskKind::Review,
                        review_spec(&task, &candidate.commit, &unreported, plan.as_deref()),
                        None,
                        Some(author.clone()),
                        checking.clone(),
                        Some(GateLink {
                            owner: owner.clone(),
                            round,
                            role: GateRole::Review,
                        }),
                        Some(task.clone()),
                        Vec::new(),
                        // It reviews the change where the change is.
                        Some(task.areas.clone()),
                        None,
                        Vec::new(),
                    )
                    .await?;
                checking.push(Author {
                    provider: review.route.choice.provider,
                    model: review.route.choice.model.clone(),
                });
                if first.is_none() {
                    first = Some(ReviewRecord {
                        task_id: review.id.clone(),
                        commit: candidate.commit.clone(),
                        verdict: None,
                        cross_vendor: review.route.choice.provider != author.provider,
                    });
                }
                members.push(GateMember {
                    task_id: review.id.clone(),
                    role: GateRole::Review,
                    result: None,
                    avoid: Vec::new(),
                });
                started.push(review);
            }
            Ok(first)
        }
        .await;
        let first = match opened {
            Ok(first) => first,
            Err(err) => {
                // Nothing half-started keeps running. Stopped outside the lock: a stopped
                // member's missing result is recorded under it.
                drop(held);
                for member in started {
                    let _ = Box::pin(self.stop_task(member.id)).await;
                }
                return Err(err.into());
            }
        };
        let relanding = round_relanding(recheck, retry.is_some(), task.gate.as_ref());
        let retrying = retry.is_some();
        let installed = self
            .update_task_if(
                &task.conversation_id,
                &task.id,
                |now| still_checks(now, &candidate.commit),
                |t| {
                    t.gate = Some(Gate {
                        round,
                        commit: Some(candidate.commit.clone()),
                        members,
                        outcome: None,
                        relanding,
                        retry: retrying,
                        overridden: false,
                        findings: Vec::new(),
                    });
                    if let Some(first) = first {
                        t.review = Some(first);
                    }
                    t.state = TaskState::Reviewing;
                    t.blocked_reason = None;
                },
            )
            .await?;
        if installed.is_none() {
            drop(held);
            for member in started {
                let _ = Box::pin(self.stop_task(member.id)).await;
            }
        }
        Ok(())
    }

    /// Whether the worker wrote a plan (plan.md in its outputs folder) before a big change.
    async fn wrote_plan(&self, task: &Task) -> bool {
        let Some(workspace) = &task.workspace else {
            return false;
        };
        let plan =
            super::outputs::outputs_dir(std::path::Path::new(&workspace.scratch)).join("plan.md");
        tokio::fs::metadata(&plan)
            .await
            .is_ok_and(|meta| meta.is_file() && meta.len() > 0)
    }

    /// For a held change accepted again: when the rebuilt `commit` holds the same files as the
    /// round that couldn't verify it, its reviews still stand, and only a verifier checks it
    /// again. Returns why the last verifier couldn't, and who it was (the next one is another
    /// model).
    pub(super) async fn reverify_held(
        &self,
        task: &Task,
        commit: &str,
    ) -> Option<(String, Author)> {
        let gate = task.gate.as_ref()?;
        if gate.outcome != Some(GateOutcome::Unverified) {
            return None;
        }
        let before = gate.commit.as_deref()?;
        if !self.same_tree(task, before, commit).await {
            return None;
        }
        let verifier = gate.members.iter().find(|m| m.role == GateRole::Verify)?;
        let verifier = self
            .task_by_id(&task.conversation_id, &verifier.task_id)
            .await
            .ok()?;
        Some((
            unverified_reasons(gate),
            Author {
                provider: verifier.route.choice.provider,
                model: verifier.route.choice.model.clone(),
            },
        ))
    }

    /// Whether two commits of the task's repository hold the same files.
    pub(super) async fn same_tree(&self, task: &Task, a: &str, b: &str) -> bool {
        let Ok(repo) = self.task_repo(task) else {
            return false;
        };
        let (git, a, b) = (self.git.clone(), a.to_owned(), b.to_owned());
        blocking(move || {
            let repo = git.open(&repo).map_err(git_error)?;
            let a = repo.tree_of(&a).map_err(git_error)?;
            let b = repo.tree_of(&b).map_err(git_error)?;
            Ok(a == b)
        })
        .await
        .unwrap_or(false)
    }

    /// A gate member reported. Called while its report is recorded, before its turn ends, so
    /// a verifier's checkout is still there to compare with the commit it checked.
    pub(crate) async fn gate_member_reported(&self, member: &Task) {
        let Some(link) = member.gate_link.clone() else {
            return;
        };
        let task_id = match &link.owner {
            GateOwner::Task { task_id } => task_id,
            GateOwner::Plan { plan_id } => {
                self.plan_member_done(member, plan_id, &link, None).await;
                return;
            }
            GateOwner::Phase { run_id, phase_id } => {
                self.phase_member_done(member, run_id, phase_id, &link, None)
                    .await;
                return;
            }
        };
        let Some(report) = &member.report else {
            return;
        };
        let result = match link.role {
            GateRole::Review | GateRole::Judge => review_result(report),
            GateRole::Verify => match self.verifier_changes(member).await {
                Some(changed) => GateResult::NoResult {
                    reason: format!(
                        "The verifier (task-{}) changed what it checked ({changed}), so its result was discarded.",
                        member.number
                    ),
                },
                None => {
                    // It covers at least the criteria the worker listed.
                    let listed = self
                        .task_by_id(&member.conversation_id, task_id)
                        .await
                        .ok()
                        .and_then(|owner| owner.report)
                        .map_or(0, |report| report.done_when.len());
                    verify_result(report, listed)
                }
            },
        };
        self.record_gate_result(member, task_id, &link, result)
            .await;
    }

    /// A gate member failed or was stopped before its result.
    pub(crate) async fn gate_member_failed(&self, member: &Task, reason: &str) {
        let Some(link) = member.gate_link.clone() else {
            return;
        };
        let task_id = match &link.owner {
            GateOwner::Task { task_id } => task_id,
            GateOwner::Plan { plan_id } => {
                self.plan_member_done(member, plan_id, &link, Some(reason))
                    .await;
                return;
            }
            GateOwner::Phase { run_id, phase_id } => {
                self.phase_member_done(member, run_id, phase_id, &link, Some(reason))
                    .await;
                return;
            }
        };
        let result = GateResult::NoResult {
            reason: format!(
                "The {} (task-{}) gave no result: {reason}",
                role_name(link.role),
                member.number
            ),
        };
        self.record_gate_result(member, task_id, &link, result)
            .await;
    }

    /// What a verifier changed in its checkout: tracked files, a new file git doesn't ignore,
    /// or another commit. `None` when its checkout is still exactly the commit it checked.
    pub(super) async fn verifier_changes(&self, member: &Task) -> Option<String> {
        let workspace = member.workspace.as_ref()?;
        let worktree = std::path::PathBuf::from(workspace.worktree.clone()?);
        let base = Oid(workspace.base.clone()?);
        let git = self.git.clone();
        blocking(move || {
            let worktree = git.open_worktree(&worktree).map_err(git_error)?;
            if worktree.head().map_err(git_error)? != base {
                return Ok(Some("its checkout is on another commit".to_owned()));
            }
            Ok(checkout_changes(
                &worktree.changes(&base).map_err(git_error)?,
            ))
        })
        .await
        .unwrap_or_else(|err| Some(format!("its checkout could not be read: {err}")))
    }

    /// Records a member's result in its round and, once the round is decided, acts on it.
    async fn record_gate_result(
        &self,
        member: &Task,
        owner: &TaskId,
        link: &GateLink,
        result: GateResult,
    ) {
        let decided = {
            let _held = self.gates.lock().await;
            let Ok(task) = self.task_by_id(&member.conversation_id, owner).await else {
                return;
            };
            let Some(mut gate) = task.gate.clone() else {
                return;
            };
            // A result of an older or closed round, or for a task no longer waiting on it.
            if gate.round != link.round
                || gate.outcome.is_some()
                || task.state != TaskState::Reviewing
            {
                return;
            }
            let Some(slot) = gate
                .members
                .iter_mut()
                .find(|known| known.task_id == member.id)
            else {
                return;
            };
            if slot.result.is_some() {
                return;
            }
            let verdict = match &result {
                GateResult::Passed => Some(ReviewVerdict::Approve),
                GateResult::Failed { .. } => Some(ReviewVerdict::RequestChanges),
                _ => None,
            };
            slot.result = Some(result);
            if gate.members.iter().all(|m| m.result.is_some()) {
                gate.outcome = Some(outcome_of(&gate.members));
            }
            let updated = self
                .update_task(&task.conversation_id, &task.id, |t| {
                    t.gate = Some(gate.clone());
                    if let Some(review) = t.review.as_mut()
                        && review.task_id == member.id
                    {
                        review.verdict = verdict;
                    }
                })
                .await;
            match updated {
                Ok(task) if gate.outcome.is_some() => Some(task),
                _ => None,
            }
        };
        if let Some(task) = decided {
            let manager = self.arc();
            self.spawn(async move { manager.gate_decided(task).await });
        }
    }

    /// Acts on a decided round. Boxed with a named type: acting on a round can stop and
    /// start gate members, whose results come back here.
    fn gate_decided(&self, task: Task) -> brigadier_providers::BoxFuture<'_, ()> {
        Box::pin(self.act_on_gate(task))
    }

    async fn act_on_gate(&self, task: Task) {
        let Some(gate) = task.gate.clone() else {
            return;
        };
        // The task moved on meanwhile (stopped, sent back, a newer round): nothing to do.
        if !self
            .task_by_id(&task.conversation_id, &task.id)
            .await
            .is_ok_and(|now| round_current(&task, &now) && now.gate == task.gate)
        {
            return;
        }
        let members = self.member_tasks(&task, &gate).await;
        match gate.outcome {
            Some(GateOutcome::Passed) => {
                self.landing_waits(&task, &gate, &members).await;
                let landed = if gate.relanding {
                    self.land_task(&task).await
                } else {
                    self.approve_and_land(&task).await
                };
                if let Err(err) = landed {
                    self.landing_problem(&task, &err.to_string(), TaskState::Reported)
                        .await;
                }
            }
            Some(GateOutcome::Failed) => {
                self.landing_waits(&task, &gate, &members).await;
                let findings = findings_text(&gate, &members);
                let counts = finding_counts(&gate);
                self.send_back_or_escalate(&task, &findings, &counts, false)
                    .await;
            }
            Some(GateOutcome::Unverified) => {
                let reasons = unverified_reasons(&gate);
                // No second verifier (user decision, 2026-10-04): a change that couldn't be
                // verified is held, and accepting it again verifies it once more.
                let (user_only, listed) = self.landing_waits(&task, &gate, &members).await;
                let next = if listed > 0 {
                    format!(
                        "Only the user can unblock it; it is listed for them under Waiting on you:\n{}\nYou hear when they mark it done; then call accept_task for task-{} again to verify and land it.",
                        user_only
                            .iter()
                            .map(|line| format!("- {line}"))
                            .collect::<Vec<_>>()
                            .join("\n"),
                        task.number
                    )
                } else {
                    format!(
                        "If only the user can unblock this (a key, a sign-in, a tool to install), ask them. Once it is fixed, call accept_task for task-{} again to verify and land it.",
                        task.number
                    )
                };
                self.decided_for_task(
                    &task,
                    format!("Held task-{}: its change couldn't be verified", task.number),
                    format!("Nothing lands unverified. {}", reason_heads(&reasons)),
                )
                .await;
                self.landing_problem(
                    &task,
                    &format!(
                        "Its change could not be verified, so nothing landed:\n{reasons}\n{next}"
                    ),
                    TaskState::ReadyToLand,
                )
                .await;
            }
            Some(GateOutcome::NoResult) => {
                self.landing_waits(&task, &gate, &members).await;
                let reasons = no_result_reasons(&gate);
                let _ = self
                    .update_task(&task.conversation_id, &task.id, |t| t.landing = None)
                    .await;
                self.decided_for_task(
                    &task,
                    format!(
                        "Didn't land task-{}: its checks couldn't finish",
                        task.number
                    ),
                    reason_heads(&reasons),
                )
                .await;
                self.landing_problem(
                    &task,
                    &format!(
                        "The checks of its change could not finish; nothing landed.\n{reasons}\nIf that is temporary, call accept_task for task-{} again. If it needs the user (a sign-in, a key), tell them what failed instead of retrying.",
                        task.number
                    ),
                    TaskState::Reported,
                )
                .await;
            }
            Some(GateOutcome::Superseded) | None => {}
        }
    }

    /// What the round's verifiers say only the user can do before the checks can run is
    /// listed for them (see [`landing_wait_lines`]); what an earlier round listed and they no
    /// longer name is over. Returns the lines, and how many are listed.
    async fn landing_waits(
        &self,
        task: &Task,
        gate: &Gate,
        members: &[Task],
    ) -> (Vec<String>, usize) {
        let reports: Vec<&Report> = gate
            .members
            .iter()
            .filter(|m| m.role == GateRole::Verify)
            .filter_map(|m| members.iter().find(|t| t.id == m.task_id))
            .filter_map(|t| t.report.as_ref())
            .collect();
        let Some(user_only) = landing_wait_lines(gate, &reports) else {
            return (Vec::new(), 0);
        };
        let listed = self
            .sync_waiting(
                task,
                WaitingSource::Landing {
                    task_id: task.id.clone(),
                },
                &user_only,
            )
            .await;
        (user_only, listed)
    }

    /// The tasks of a round's members.
    async fn member_tasks(&self, task: &Task, gate: &Gate) -> Vec<Task> {
        let mut found = Vec::new();
        for member in &gate.members {
            if let Ok(member) = self
                .task_by_id(&task.conversation_id, &member.task_id)
                .await
            {
                found.push(member);
            }
        }
        found
    }

    /// Sends the worker the gate's findings to fix, or (rounds used up, a fix that changed
    /// nothing, or work the orchestrator took back) hands the decision to the orchestrator.
    /// `counts` names how many findings came from which check ("2 review findings"), for
    /// "Decided for you".
    pub(crate) async fn send_back_or_escalate(
        &self,
        task: &Task,
        findings: &str,
        counts: &str,
        unchanged: bool,
    ) {
        if task.landing.is_some() && task.fix_rounds < FIX_ROUNDS && !unchanged {
            let text = format!("{SEND_BACK}\n{findings}");
            let sent = match self
                .update_task(&task.conversation_id, &task.id, |t| t.fix_rounds += 1)
                .await
            {
                Ok(task) => {
                    self.message_worker(&task.conversation_id, &task, text, "Brigadier")
                        .await
                }
                Err(err) => Err(err),
            };
            match sent {
                Ok(_) => {
                    // Later checks, and the orchestrator if the fixes end without landing,
                    // read what the worker was asked to fix.
                    let _ = self
                        .update_task(&task.conversation_id, &task.id, |t| {
                            t.fixes.push(findings.to_owned());
                        })
                        .await;
                    self.decided_for_task(
                        task,
                        format!(
                            "Sent task-{} back: {counts} (fix {} of {FIX_ROUNDS})",
                            task.number,
                            task.fix_rounds + 1
                        ),
                        "The findings are on its checks.".to_owned(),
                    )
                    .await;
                    return;
                }
                Err(err) => {
                    tracing::warn!(task = %task.id, error = %err, "could not send a task back with its findings");
                }
            }
        }
        let mut addendum = None;
        let task = self
            .update_task(&task.conversation_id, &task.id, |t| {
                t.landing = None;
                t.state = TaskState::Reported;
                t.blocked_reason = None;
                addendum = t.addendum.take();
            })
            .await
            .unwrap_or_else(|_| task.clone());
        let tried = match (unchanged, task.fix_rounds) {
            (true, _) => format!(
                "Its change is the same one these findings are about (a fix that changed nothing, or the same change accepted again), so it was not checked again. Decide: send it back with guidance (message_worker), stop it, or ask the user. Only if the user explicitly told you to land it despite these findings, call accept_task for task-{} with override: true.",
                task.number
            ),
            (false, 0) => format!(
                "Send task-{} back with message_worker to fix this, then accept it again.",
                task.number
            ),
            (false, rounds) => format!(
                "Brigadier sent it back {rounds} time{} with the findings and they are still there. Decide: send it back with guidance (message_worker), stop it, or ask the user.",
                if rounds == 1 { "" } else { "s" }
            ),
        };
        let reason = match (unchanged, task.fix_rounds) {
            (true, _) => "the fix changed nothing".to_owned(),
            (false, 0) => counts.to_owned(),
            (false, rounds) => format!(
                "problems left after {rounds} fix round{}",
                if rounds == 1 { "" } else { "s" }
            ),
        };
        self.decided_for_task(
            &task,
            format!("Didn't land task-{}: {reason}", task.number),
            "The orchestrator decides what happens next; the findings are on its checks."
                .to_owned(),
        )
        .await;
        let mut text = format!(
            "[checks task-{}] Changes needed; nothing landed.\n{findings}",
            task.number
        );
        if unchanged {
            text.push_str(&fixes_text(&task.fixes));
        }
        text.push_str(&format!("\n[/checks] {tried}"));
        // What the worker wrote after its last report, held while Brigadier had the change.
        if let Some(addendum) = addendum {
            text.push_str(&format!(
                "\n{}",
                super::prompts::late_findings_envelope(&task, &addendum)
            ));
        }
        self.announcing(&task).await;
        self.deliver(
            &task.conversation_id,
            Envelope {
                kind: InjectionKind::Report,
                label: format!("checks of task-{}", task.number),
                task_id: Some(task.id.clone()),
                text,
            },
        )
        .await;
        // From now on, only the user can have this change land despite these findings.
        if let (Some(gate), Some(candidate)) = (&task.gate, &task.candidate) {
            let escalated = crate::work::Escalated {
                round: gate.round,
                commit: candidate.commit.clone(),
                at_ms: crate::now_ms(),
            };
            let _ = self
                .update_task(&task.conversation_id, &task.id, |t| {
                    t.escalated = Some(escalated);
                })
                .await;
        }
    }

    /// The worker's fix changed nothing: the orchestrator gets the last round's findings.
    pub(crate) async fn escalate_unchanged(&self, task: &Task) {
        let findings = self.gate_findings(task).await;
        let counts = task.gate.as_ref().map(finding_counts).unwrap_or_default();
        self.send_back_or_escalate(task, &findings, &counts, true)
            .await;
    }

    /// The findings of the task's last gate round, member by member.
    pub(crate) async fn gate_findings(&self, task: &Task) -> String {
        match &task.gate {
            Some(gate) => {
                let members = self.member_tasks(task, gate).await;
                findings_text(gate, &members)
            }
            None => String::new(),
        }
    }

    /// Stops a task's open gate round (the task was stopped): its members' work is moot.
    pub(crate) async fn close_gate(&self, task: &Task) {
        let open: Vec<TaskId> = {
            let _held = self.gates.lock().await;
            // As it is now: a round may have opened since the caller read it.
            let Ok(now) = self.task_by_id(&task.conversation_id, &task.id).await else {
                return;
            };
            let Some(gate) = now.gate.filter(|gate| gate.outcome.is_none()) else {
                return;
            };
            let _ = self
                .update_task(&task.conversation_id, &task.id, |t| {
                    if let Some(gate) = t.gate.as_mut() {
                        gate.outcome = Some(GateOutcome::Superseded);
                    }
                })
                .await;
            gate.members
                .iter()
                .filter(|m| m.result.is_none())
                .map(|m| m.task_id.clone())
                .collect()
        };
        for member in open {
            // Boxed: stopping a member records its missing result, which reaches this gate.
            let _ = Box::pin(self.stop_task(member)).await;
        }
    }

    /// Decides a round every member has a result in that was left without an outcome (the
    /// stall watchdog found it), as the last result would have. Returns whether it did.
    pub(crate) async fn resettle_gate(&self, task: &Task) -> bool {
        let decided = {
            let _held = self.gates.lock().await;
            let Ok(now) = self.task_by_id(&task.conversation_id, &task.id).await else {
                return false;
            };
            let Some(mut gate) = now.gate.clone() else {
                return false;
            };
            if now.state != TaskState::Reviewing
                || gate.outcome.is_some()
                || gate.members.is_empty()
                || gate.members.iter().any(|m| m.result.is_none())
            {
                return false;
            }
            gate.outcome = Some(outcome_of(&gate.members));
            match self
                .update_task(&now.conversation_id, &now.id, |t| t.gate = Some(gate))
                .await
            {
                Ok(task) => task,
                Err(err) => {
                    tracing::warn!(task = %task.id, error = %err, "could not settle a gate round");
                    return false;
                }
            }
        };
        let manager = self.arc();
        self.spawn(async move { manager.gate_decided(decided).await });
        true
    }

    /// Who a checking task must not be, for a hand-off to another model. A gate member: the
    /// author of the change, the models of the round's other reviewers, and those it was
    /// told to avoid. A review or verification the orchestrator delegated itself: the author
    /// of its subject.
    pub(crate) async fn gate_avoid(&self, member: &Task) -> (Option<Author>, Vec<Author>) {
        let Some(link) = &member.gate_link else {
            return (self.subject_author(member).await, Vec::new());
        };
        let task_id = match &link.owner {
            GateOwner::Task { task_id } => task_id,
            GateOwner::Plan { plan_id } => {
                return self.plan_gate_avoid(member, plan_id, link.round).await;
            }
            GateOwner::Phase { run_id, phase_id } => {
                return self.phase_gate_avoid(member, run_id, phase_id).await;
            }
        };
        let Ok(board) = self.core.board(&member.conversation_id).await else {
            return (None, Vec::new());
        };
        let Some(owner) = board.tasks.get(task_id) else {
            return (None, Vec::new());
        };
        let others = match owner.gate.as_ref().filter(|g| g.round == link.round) {
            Some(gate) => member_avoids(gate, &member.id, link.role, |id| {
                board.tasks.get(id).map(|t| author_of(&t.route.choice))
            }),
            None => Vec::new(),
        };
        (Some(author_of(&owner.route.choice)), others)
    }

    /// The author of the change a review or verification task checks.
    async fn subject_author(&self, task: &Task) -> Option<Author> {
        if !matches!(task.kind, TaskKind::Review | TaskKind::Verify) {
            return None;
        }
        let subject = self
            .task_by_id(&task.conversation_id, task.subject.as_ref()?)
            .await
            .ok()?;
        Some(author_of(&subject.route.choice))
    }
}

/// The models a gate member must differ from besides the author: a reviewer, the round's
/// other reviewers; any member, the models it was told to avoid when it started.
fn member_avoids(
    gate: &Gate,
    member: &TaskId,
    role: GateRole,
    model_of: impl Fn(&TaskId) -> Option<Author>,
) -> Vec<Author> {
    let mut avoid = Vec::new();
    for other in &gate.members {
        if &other.task_id == member {
            avoid.extend(other.avoid.iter().map(author_of));
        } else if role == GateRole::Review
            && other.role == GateRole::Review
            && let Some(model) = model_of(&other.task_id)
        {
            avoid.push(model);
        }
    }
    avoid
}

fn author_of(choice: &ModelChoice) -> Author {
    Author {
        provider: choice.provider,
        model: choice.model.clone(),
    }
}

/// Whether the round decided on `decided` (the task when its round ended, or when its
/// landing was approved) still speaks for the task as it is `now`: the same round on the same
/// commit, and the task still landing (not stopped, sent back or landed meanwhile).
pub(super) fn round_current(decided: &Task, now: &Task) -> bool {
    let (Some(then), Some(gate)) = (&decided.gate, &now.gate) else {
        return false;
    };
    gate.round == then.round
        && gate.commit == then.commit
        && matches!(
            now.state,
            TaskState::Reviewing | TaskState::AwaitingApproval
        )
}

/// How the task's last checks ended, when they ended without landing (changes needed, or
/// checks that could not finish) and nothing changed since: the worker was not sent back
/// (which voids the candidate), so accepting it again checks the same change.
pub(super) fn checks_stand(task: &Task) -> Option<GateOutcome> {
    let outcome = task.gate.as_ref()?.outcome.clone()?;
    (matches!(outcome, GateOutcome::Failed | GateOutcome::NoResult) && task.candidate.is_some())
        .then_some(outcome)
}

/// What a closed round's verifiers (their `reports`) say only the user can do before the
/// change can land, whatever the round's outcome but a pass: a change that passed waits on
/// nothing, so whatever was listed for it is over (a gap the parent has too is no blocker).
/// `None` for a round no verifier reported in: it says nothing about it.
fn landing_wait_lines(gate: &Gate, reports: &[&Report]) -> Option<Vec<String>> {
    if gate.outcome == Some(GateOutcome::Passed) {
        return Some(Vec::new());
    }
    if reports.is_empty() {
        return None;
    }
    Some(
        reports
            .iter()
            .flat_map(|report| report.needs_user.iter().cloned())
            .collect(),
    )
}

/// Whether the user spoke (their newest message or answer on a card at `user_at_ms`) after
/// the orchestrator was handed the failed checks of the task's change as it is now: same
/// round, same candidate. Only then may `accept_task` land it despite them (`override`);
/// the orchestrator's word alone is not the user's.
pub(super) fn user_spoke_since_checks(task: &Task, user_at_ms: Option<i64>) -> bool {
    let (Some(escalated), Some(gate), Some(candidate)) =
        (&task.escalated, &task.gate, &task.candidate)
    else {
        return false;
    };
    escalated.round == gate.round
        && escalated.commit == candidate.commit
        && user_at_ms.is_some_and(|at| at > escalated.at_ms)
}

/// Whether the task's candidate, as it is now, passed its gate (or the user had it land
/// despite the gate's findings): only that commit may land.
pub(super) fn candidate_passed(task: &Task) -> bool {
    task.gate.as_ref().is_some_and(|gate| {
        (gate.outcome == Some(GateOutcome::Passed)
            || (gate.overridden && gate.outcome == Some(GateOutcome::Failed)))
            && gate.commit.is_some()
            && gate.commit.as_deref() == task.candidate.as_ref().map(|c| c.commit.as_str())
    })
}

/// A change bigger than this (files, or lines added and removed) is planned first: the
/// implement worker writes plan.md before it starts (see `prompts::worker`).
const PLANNED_FILES: usize = 3;
const PLANNED_LINES: u32 = 150;

/// What the gate checks about the worker's plan: the change against plan.md when there is
/// one; a big change without one only on the change's first round (`first_round`), and only
/// as a risk to name (the plan lives outside the commit, so no fix can add it).
fn plan_note(task: &Task, wrote_plan: bool, first_round: bool) -> Option<String> {
    if wrote_plan {
        return Some("The worker wrote a plan first (plan.md, below): check the change against it. Each planned step should be done, and anything outside the plan needs a reason.".into());
    }
    if !first_round {
        return None;
    }
    let stat = &task.candidate.as_ref()?.diff_stat;
    let lines = stat.insertions + stat.deletions;
    (task.kind == TaskKind::Implement && (stat.files.len() > PLANNED_FILES || lines > PLANNED_LINES))
        .then(|| {
            format!(
                "A risk to name, never a finding: this change is big ({} files, {lines} lines) and the worker wrote no plan.md, which it was asked to write first for a change of more than {PLANNED_FILES} files or about {PLANNED_LINES} lines. A plan lives in the worker's outputs folder, never in the commit, so no fix of the change can add one: never make it a [not met] line, an open question or a reason to request changes. Name it under risks in your report, and check the change's scope with extra care: every part must be needed by the task.",
                stat.files.len()
            )
        })
}

/// What every member of a task's gate is told about what it judges.
const JUDGE_THE_CHANGE: &str = "Judge the change itself (the code, tests and files in the commit), not the worker's report: a problem only with the report or its wording is never a finding, since the report does not land.";

/// For a round after Brigadier sent the worker back with earlier checks' findings: they are
/// listed below the task (see `SessionManager::review_brief`).
fn fixes_note(task: &Task) -> Option<String> {
    (!task.fixes.is_empty()).then(|| {
        format!(
            "Brigadier sent the worker back {} with earlier checks' findings (listed below with the task). Check that each one is fixed, and fixed right.",
            times(task.fixes.len())
        )
    })
}

/// What a reviewer reads first.
fn review_spec(task: &Task, commit: &str, unreported: &[String], plan: Option<&str>) -> String {
    let mut spec = format!(
        "Review the candidate commit {} of task-{} (\"{}\"). Decide whether it may land: it must do what the task asked, correctly, without slop, stray files or unverified claims. {JUDGE_THE_CHANGE}",
        short(commit),
        task.number,
        task.title
    );
    if let Some(fixes) = fixes_note(task) {
        spec.push_str(&format!("\n{fixes}"));
    }
    if !unreported.is_empty() {
        spec.push_str(&format!(
            "\nThe worker did not report these tracked changes; check they belong to the task: {}.",
            unreported.join(", ")
        ));
    }
    if let Some(plan) = plan {
        spec.push_str(&format!("\n{plan}"));
    }
    spec.push_str("\nEnd with submit_report and a verdict: approve, or requestChanges with the exact issues in open_questions.");
    spec
}

/// What a verifier reads first.
fn verify_spec(
    task: &Task,
    commit: &str,
    retry: Option<&String>,
    plan: Option<&str>,
    sandboxed: bool,
) -> String {
    // Asked about later messages that don't exist, a verifier asks the orchestrator for them.
    let criteria = if task.messages.is_empty() {
        "the task's below (the orchestrator sent the worker nothing that changes it) and each one the worker listed in its report"
    } else {
        "the task's below, those in the orchestrator's later messages to the worker (below the task), and each one the worker listed in its report"
    };
    let mut spec = format!(
        "Verify the candidate commit {} of task-{} (\"{}\") independently, before it may land. Your checkout is at that commit.
1. Find every \"done when\" criterion: {criteria}. For each one, produce your own evidence: run the command and quote the decisive line, or read the code and say where. The worker's claims are not evidence.
2. Run the project's checks the way the project runs them (see its README, package scripts, Makefile and CI config): typecheck, lint, build and the existing tests. {setup}
3. Check hygiene: files the commit should not hold (logs, scratch notes, debug output, secrets, generated junk), debug code left in, and changes the task didn't ask for.
4. Change no tracked file and add no source file: build output goes only into the project's ignored folders. Brigadier compares your checkout with the commit after your report and discards a verification that changed it.
5. {not_run}
6. When a check fails, find out whether it fails the same way without this change: unpack the parent commit into your scratch folder (`mkdir <scratch>/parent && git archive HEAD~1 | tar -x -C <scratch>/parent`) and run it there, or read the code. A failure that is already there on the parent is not this change's: it is never [not met], an open question or a failed check for this change. Name it under risks.
7. When a check can't run here, try it on the parent the same way. If it can't run there either, for the same reason (the same missing key, sign-in or service), it is a gap the project already had, not this change's: name it under risks as \"[pre-existing] <the check>: <your evidence it fails the same way on the parent: the command you ran there and its error>\", one line per check. A check that can't run here but runs on the parent, or one you couldn't try there, goes under risks as \"[not run] the check: the command you tried and its error\".
{JUDGE_THE_CHANGE}",
        short(commit),
        task.number,
        task.title,
        setup = checks_setup(sandboxed),
        not_run = not_run_step(sandboxed),
    );
    if let Some(fixes) = fixes_note(task) {
        spec.push_str(&format!("\n{fixes}"));
    }
    let gave_up: Vec<&String> = task
        .report
        .iter()
        .flat_map(|report| &report.done_when)
        .filter(|line| !matches!(criterion_status(line), Some(Status::Met)))
        .collect();
    if !gave_up.is_empty() {
        spec.push_str("\nThe worker did not show these as met; check each one yourself first:");
        for line in gave_up {
            spec.push_str(&format!("\n- {line}"));
        }
    }
    if let Some(plan) = plan {
        spec.push_str(&format!("\n{plan}"));
    }
    if let Some(why) = retry {
        spec.push_str(&format!(
            "\nAn earlier verifier could not check this change:\n{why}\n{}",
            if sandboxed {
                "Run what it couldn't if your checkout lets you; what your sandbox refuses is [not run] or [excluded], with its error or rule."
            } else {
                "Find a way to run what it couldn't."
            }
        ));
    }
    spec.push_str(
        "\nEnd with submit_report. done_when: one line per criterion, for every criterion (the task's and the worker's, never fewer lines than the worker listed): \"[met] criterion: your evidence\", \"[not met] criterion: what fails\", or \"[not checked] criterion: the command you tried and its error\"; a check you did not run yourself is never [met]. checks: passed (every check you ran passed, apart from failures already on the parent, and the only checks that could not run, if any, are [pre-existing] or [excluded] ones, each named with its evidence or rule), failed (a check ran and failed because of this change), notRun (any other check could not run, after you tried; a check stopped by a missing key, sign-in or service is notRun, not failed; the change never lands on notRun), or noChecks (the project has no checks you could run, apart from [pre-existing] and [excluded] ones). Put each problem the worker must fix in open_questions, and nothing else: any line there sends the change back to the worker.",
    );
    spec.push_str(if task.run.is_some() {
        " If a \"done when\" criterion can't be shown met until the user does something only they can (a key, a sign-in, a paid account), say exactly what under needs_user. A [pre-existing] gap, an [excluded] check or an optional check goes under risks only, never under needs_user: needs_user is for what holds a criterion."
    } else {
        " If a check that is not a [pre-existing] gap can't run until the user does something only they can (a key, a sign-in, a paid account), say exactly what under needs_user. A [pre-existing] gap or an [excluded] check goes under risks only, never under needs_user: needs_user is for what holds this change."
    });
    spec
}

/// A change to documentation only: Markdown and other prose files, or anything under the
/// top-level `docs/`.
pub(crate) fn docs_only(files: &[crate::work::FileStat]) -> bool {
    // Not `.txt`: `requirements.txt` and `CMakeLists.txt` build things.
    const TEXT: &[&str] = &["md", "mdx", "markdown", "rst", "adoc"];
    !files.is_empty()
        && files.iter().all(|file| {
            let path = file.path.trim_start_matches("./");
            path.starts_with("docs/")
                || std::path::Path::new(path)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| TEXT.contains(&ext.to_ascii_lowercase().as_str()))
        })
}

/// A path in an area where a mistake costs the most (user decision, 2026-10-04): landing,
/// policy, the sandbox, git. Matched by whole words of the path (`git_actions.rs`,
/// `crates/git/`), so `.github/` or `digit.rs` don't count.
fn risky_path(path: &str) -> bool {
    path.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| matches!(word, "landing" | "policy" | "sandbox" | "git"))
}

/// What a fix round's verifier checks for the reviewers whose approvals are kept: that the fix
/// keeps what they approved. `None` when no approval is kept (a first round, or every reviewer
/// asked for changes).
fn kept_approvals(task: &Task) -> Option<String> {
    let previous = task.gate.as_ref()?;
    if previous.outcome != Some(GateOutcome::Failed) {
        return None;
    }
    let approved = previous
        .members
        .iter()
        .filter(|m| m.role == GateRole::Review && m.result == Some(GateResult::Passed))
        .count();
    let before = previous.commit.as_deref()?;
    (approved > 0).then(|| {
        format!(
            "{} of the change at {} approved it and {} not asked again: read the fix (`git diff {} HEAD`) and check that it keeps what was approved. Anything it breaks goes in open_questions.",
            if approved == 1 { "A reviewer" } else { "Its reviewers" },
            short(before),
            if approved == 1 { "is" } else { "are" },
            short(before),
        )
    })
}

/// What the verifier of a change to documentation only reads: the criteria and the docs
/// against the code, without builds, tests or smoke runs.
fn verify_docs_spec(task: &Task, commit: &str, kept: Option<&str>) -> String {
    let mut spec = format!(
        "Verify the candidate commit {} of task-{} (\"{}\") independently, before it may land. Your checkout is at that commit. The change touches documentation only.
1. Find every \"done when\" criterion: the task's below and each one the worker listed in its report. For each one, produce your own evidence: quote the document and say where, or run the command that shows it. The worker's claims are not evidence. A criterion that builds or tests pass can't change with documentation: once `git show --stat HEAD` shows only documentation files, mark it [met] with that as its evidence.
2. Check the documents against the code: every path, command, name, number and claim they state must be true of this checkout; links and file references must resolve. Don't run builds, tests or smoke checks for it.
3. Check hygiene: files the commit should not hold (logs, scratch notes, secrets) and changes the task didn't ask for.
4. Change no tracked file. Brigadier compares your checkout with the commit after your report and discards a verification that changed it.
{JUDGE_THE_CHANGE}",
        short(commit),
        task.number,
        task.title
    );
    if let Some(fixes) = fixes_note(task) {
        spec.push_str(&format!("\n{fixes}"));
    }
    if let Some(kept) = kept {
        spec.push_str(&format!("\n{kept}"));
    }
    spec.push_str(
        "\nEnd with submit_report. done_when: one line per criterion, for every criterion: \"[met] criterion: your evidence\", \"[not met] criterion: what is wrong\", or \"[not checked] criterion: why you couldn't\". checks: noChecks (documentation has no build or tests to run), or failed when a document states something the code contradicts. Put each problem the worker must fix in open_questions, and nothing else.",
    );
    spec
}

/// The setup of a verifier's checks: what is installed, where a smoke run keeps its data, and
/// how a check the sandbox or a rule forbids is given (PLAN.md §10.8).
pub(crate) fn checks_setup(sandboxed: bool) -> String {
    format!(
        "Its dependencies are copied into this checkout from the user's while its lockfiles match theirs: don't reinstall them{}. A smoke run that starts the app or a daemon keeps its data in your test data folder, never the app's real data folder{}. A check the task or the run's Rules forbid (\"don't launch the app\") is not run: give it as \"[excluded] <the check>: <the rule>\".",
        if sandboxed {
            " (a check that needs one a changed lockfile left out is [not run], with its error)"
        } else {
            " (install one only if a check says it is missing)"
        },
        if sandboxed {
            "; in your sandbox it can't open windows, so give it as \"[excluded] <the check>: the sandbox can't open windows\""
        } else {
            ""
        },
    )
}

/// How hard a verifier tries a check before calling it not run: in a sandbox, what it
/// refuses it refuses for the whole check, so there is no other way to try.
pub(crate) fn not_run_step(sandboxed: bool) -> &'static str {
    if sandboxed {
        "Before you call a check not run, run it the way the project runs it; name the command and quote its error."
    } else {
        "Workers often decide too early that a check can't run. Never do that yourself: before you call a check not run, try it, then try another way (install what is missing, use the project's own scripts, read how CI runs it). Name each command you tried and quote its error."
    }
}

/// A "done when" line's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Status {
    Met,
    NotMet,
    NotChecked,
}

/// A line without its list marker: "- ", "* ", "• ", "2. " or "2) ".
pub(super) fn without_marker(line: &str) -> &str {
    let line = line.trim_start_matches(['-', '*', '•', ' ', '\t']);
    let number = line.trim_start_matches(|c: char| c.is_ascii_digit());
    match number.strip_prefix(['.', ')']) {
        Some(rest) if number.len() < line.len() => rest.trim_start(),
        _ => line,
    }
}

/// The status a "done when" line starts with ("[met] …"), and whether evidence follows.
pub(super) fn criterion_status(line: &str) -> Option<Status> {
    let line = without_marker(line).to_lowercase();
    if line.starts_with("[met]") {
        Some(Status::Met)
    } else if line.starts_with("[not met]") {
        Some(Status::NotMet)
    } else if line.starts_with("[not checked]") {
        Some(Status::NotChecked)
    } else {
        None
    }
}

/// The text after a "done when" line's status.
pub(super) fn criterion_text(line: &str) -> &str {
    let line = without_marker(line);
    line.find(']').map_or(line, |end| line[end + 1..].trim())
}

/// The evidence a "done when" line gives after its criterion ("[met] criterion: evidence",
/// or a dash instead of the colon), if any.
pub(super) fn criterion_evidence(line: &str) -> Option<&str> {
    let text = criterion_text(line);
    let at = [": ", " — ", " – ", " - ", " -> ", " => "]
        .iter()
        .filter_map(|separator| text.find(separator).map(|at| at + separator.len()))
        .min()?;
    Some(text[at..].trim()).filter(|evidence| evidence.chars().any(char::is_alphanumeric))
}

/// A reviewer's result.
pub(super) fn review_result(report: &Report) -> GateResult {
    match report.verdict {
        Some(ReviewVerdict::Approve) => GateResult::Passed,
        Some(ReviewVerdict::RequestChanges) => GateResult::Failed {
            findings: if report.open_questions.is_empty() {
                vec![report.summary.clone()]
            } else {
                report.open_questions.clone()
            },
        },
        None => GateResult::NoResult {
            reason: "The reviewer gave no verdict.".into(),
        },
    }
}

/// A "[pre-existing] …" gap's check and the evidence that it fails the same way on the
/// parent ("[pre-existing] check: evidence", or a dash instead of the colon); `None` when
/// either is missing.
fn pre_existing_gap(line: &str) -> Option<(&str, &str)> {
    let evidence = criterion_evidence(line)?;
    let text = criterion_text(line);
    let check = text[..text.len() - evidence.len()]
        .trim_end()
        .trim_end_matches([':', '—', '–', '-', '>', '='])
        .trim();
    check
        .chars()
        .any(char::is_alphanumeric)
        .then_some((check, evidence))
}

/// The checks a verifier named under risks as unable to run: those that can't run on the
/// parent either, for the same reason ("[pre-existing] …", a gap the project already had) or
/// that a rule or the environment forbids ("[excluded] …"), and the others ("[not run] …").
pub(crate) fn unrun_checks(risks: &[String]) -> (Vec<&String>, Vec<&String>) {
    let marked = |line: &String, marker: &str| {
        without_marker(line)
            .get(..marker.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(marker))
    };
    (
        risks
            .iter()
            .filter(|line| marked(line, "[pre-existing]") || marked(line, "[excluded]"))
            .collect(),
        risks
            .iter()
            .filter(|line| marked(line, "[not run]"))
            .collect(),
    )
}

/// A verifier's result. It passed only when every "done when" criterion is shown met with
/// evidence (at least as many as the worker listed, `listed`), the project's checks passed
/// (or it has none), and it found nothing for the worker to fix. A check that can't run on
/// the parent commit either, for the same reason, is a gap the project already had: named
/// under risks as "[pre-existing] check: evidence", it doesn't hold the change ("never land
/// unverified" is about the checks this change could have run). Checks reported `notRun`
/// never pass, whatever gaps are named beside them.
pub(super) fn verify_result(report: &Report, listed: usize) -> GateResult {
    use crate::work::ChecksResult;
    let mut unmet = Vec::new();
    let mut unchecked = Vec::new();
    let mut met = 0;
    for line in &report.done_when {
        match criterion_status(line) {
            Some(Status::Met) if criterion_evidence(line).is_some() => met += 1,
            Some(Status::Met) => unchecked.push(format!("{line} (no evidence given)")),
            Some(Status::NotMet) => unmet.push(line.clone()),
            Some(Status::NotChecked) | None => unchecked.push(line.clone()),
        }
    }
    // Every criterion shown met, at least as many as the worker listed.
    let all_met = met > 0 && unchecked.is_empty() && report.done_when.len() >= listed;
    let checks_failed = report.checks == Some(ChecksResult::Failed);
    // It was told to put each problem the worker must fix in open_questions.
    if !unmet.is_empty() || !report.open_questions.is_empty() || (checks_failed && !all_met) {
        let mut findings = unmet;
        findings.extend(report.open_questions.iter().cloned());
        if findings.is_empty() {
            findings.push(format!("The checks failed: {}", report.summary));
        }
        return GateResult::Failed { findings };
    }
    let reason = if checks_failed {
        // A failing check tied to no criterion and nothing named to fix (an optional check
        // that needs a key nobody set): nothing for the worker to fix, and not proof either.
        Some(format!(
            "A check failed, though every \"done when\" criterion is met and it named nothing to fix: {}",
            report.summary
        ))
    } else if met == 0 {
        Some("It showed no \"done when\" criterion met with evidence.".to_owned())
    } else if !unchecked.is_empty() {
        Some(format!("Left unchecked: {}", unchecked.join("; ")))
    } else if report.done_when.len() < listed {
        Some(format!(
            "It covered {} \"done when\" criteria; the worker listed {listed}, so some were not checked.",
            report.done_when.len()
        ))
    } else {
        let (gaps, unrun) = unrun_checks(&report.risks);
        // A gap that names no check, or no evidence that the parent fails the same way, is
        // no proof that the change had nothing to run.
        let unproven: Vec<&str> = gaps
            .iter()
            .filter(|line| pre_existing_gap(line).is_none())
            .map(|line| without_marker(line))
            .collect();
        // Everything that didn't run is excluded by a rule or the environment, or already
        // failed on the parent: nothing this change could have run is missing.
        let excluded = gaps.iter().any(|line| {
            without_marker(line)
                .to_lowercase()
                .starts_with("[excluded]")
        });
        match report.checks {
            Some(ChecksResult::NotRun) if unrun.is_empty() && excluded && unproven.is_empty() => {
                None
            }
            Some(ChecksResult::Passed | ChecksResult::NoChecks)
                if unrun.is_empty() && !unproven.is_empty() =>
            {
                Some(format!(
                    "A check it named as failing on the parent too (or as excluded) gave no check, or no evidence from the parent (or no rule): {}",
                    unproven.join("; ")
                ))
            }
            Some(ChecksResult::Passed | ChecksResult::NoChecks) if unrun.is_empty() => None,
            Some(ChecksResult::Passed | ChecksResult::NoChecks | ChecksResult::NotRun) => {
                Some(format!(
                    "The project's checks could not run: {}",
                    if unrun.is_empty() {
                        report.summary.clone()
                    } else {
                        unrun
                            .iter()
                            .map(|line| without_marker(line))
                            .collect::<Vec<_>>()
                            .join("; ")
                    }
                ))
            }
            None => Some("It did not say whether the project's checks ran.".to_owned()),
            Some(ChecksResult::Failed) => unreachable!("handled above"),
        }
    };
    match reason {
        None => GateResult::Passed,
        Some(reason) => GateResult::Unverified { reason },
    }
}

/// How a round with every result in ended.
pub(super) fn outcome_of(members: &[GateMember]) -> GateOutcome {
    let results = || members.iter().filter_map(|m| m.result.as_ref());
    if results().any(|r| matches!(r, GateResult::NoResult { .. })) {
        GateOutcome::NoResult
    } else if results().any(|r| matches!(r, GateResult::Failed { .. })) {
        GateOutcome::Failed
    } else if results().any(|r| matches!(r, GateResult::Unverified { .. })) {
        GateOutcome::Unverified
    } else {
        GateOutcome::Passed
    }
}

/// A round's findings, member by member, for the worker or the orchestrator.
fn findings_text(gate: &Gate, members: &[Task]) -> String {
    let mut text = String::new();
    for member in &gate.members {
        let who = members.iter().find(|t| t.id == member.task_id).map_or_else(
            || role_name(member.role).to_owned(),
            |t| {
                format!(
                    "{} (task-{}, {})",
                    role_name(member.role),
                    t.number,
                    super::workers::route_label(t)
                )
            },
        );
        match &member.result {
            Some(GateResult::Failed { findings }) => {
                text.push_str(&format!("\nFrom the {who}:"));
                for finding in findings {
                    text.push_str(&format!("\n- {finding}"));
                }
            }
            Some(GateResult::Unverified { reason }) => {
                text.push_str(&format!("\nThe {who} could not check everything: {reason}"));
            }
            _ => {}
        }
    }
    text.trim_start().to_owned()
}

/// What Brigadier already sent the worker back to fix (`Task::fixes`), round by round, for
/// the orchestrator; empty when it sent nothing.
fn fixes_text(fixes: &[String]) -> String {
    if fixes.is_empty() {
        return String::new();
    }
    let mut text = format!(
        "\nBrigadier already sent the worker back {} with its checks' findings:",
        times(fixes.len())
    );
    for (index, findings) in fixes.iter().enumerate() {
        text.push_str(&format!("\nFix {}:\n{findings}", index + 1));
    }
    text
}

fn times(count: usize) -> String {
    match count {
        1 => "once".to_owned(),
        2 => "twice".to_owned(),
        count => format!("{count} times"),
    }
}

/// A round's findings on one line, for "Decided for you".
pub(super) fn one_line_findings(findings: &str) -> String {
    findings
        .lines()
        .map(|line| line.trim().trim_start_matches("- "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// How many findings a round's checks gave, by check: "2 review findings, 1 verification
/// finding" ("its checks' findings" when none is listed).
fn finding_counts(gate: &Gate) -> String {
    let mut counts: Vec<(GateRole, usize)> = Vec::new();
    for member in &gate.members {
        if let Some(GateResult::Failed { findings }) = &member.result {
            match counts.iter_mut().find(|(role, _)| *role == member.role) {
                Some((_, count)) => *count += findings.len(),
                None => counts.push((member.role, findings.len())),
            }
        }
    }
    let parts: Vec<String> = counts
        .iter()
        .filter(|(_, count)| *count > 0)
        .map(|(role, count)| {
            format!(
                "{count} {} finding{}",
                role_name(*role),
                if *count == 1 { "" } else { "s" }
            )
        })
        .collect();
    if parts.is_empty() {
        "its checks' findings".to_owned()
    } else {
        parts.join(", ")
    }
}

/// Brigadier's own words of each reason ("The project's checks could not run"), without the
/// check's text after them, as one line.
fn reason_heads(reasons: &str) -> String {
    let mut heads: Vec<String> = Vec::new();
    for line in reasons.lines() {
        let line = line.trim().trim_start_matches("- ");
        let head = line.split_once(": ").map_or(line, |(head, _)| head).trim();
        let head = head.trim_end_matches('.');
        if !head.is_empty() && !heads.iter().any(|h| h == head) {
            heads.push(head.to_owned());
        }
    }
    heads
        .iter()
        .map(|head| format!("{head}."))
        .collect::<Vec<_>>()
        .join(" ")
}

fn unverified_reasons(gate: &Gate) -> String {
    gate.members
        .iter()
        .filter_map(|m| match &m.result {
            Some(GateResult::Unverified { reason }) => Some(format!("- {reason}")),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn no_result_reasons(gate: &Gate) -> String {
    gate.members
        .iter()
        .filter_map(|m| match &m.result {
            Some(GateResult::NoResult { reason }) => Some(format!("- {reason}")),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn role_name(role: GateRole) -> &'static str {
    match role {
        GateRole::Review => "review",
        GateRole::Verify => "verification",
        GateRole::Judge => "judgement",
    }
}

/// What a verifier's checkout holds that its commit doesn't: any changed tracked file and
/// any new file git doesn't ignore, whatever its kind (a check may have used it). Build
/// output goes into ignored folders, which don't count.
fn checkout_changes(changes: &[brigadier_git::Change]) -> Option<String> {
    let paths: Vec<&str> = changes.iter().map(|change| change.path.as_str()).collect();
    (!paths.is_empty()).then(|| format!("it changed {}", paths.join(", ")))
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(10)]
}

/// Whether a new round lands its change as soon as it passes: a verify-only round on a
/// change the user already approved (a clean replay), and a retry of such a round's
/// verification, which checks the same change again.
fn round_relanding(recheck: Recheck, retry: bool, previous: Option<&Gate>) -> bool {
    match recheck {
        Recheck::Verify if retry => previous.is_some_and(|gate| gate.relanding),
        Recheck::Verify => true,
        Recheck::Full => false,
    }
}

/// Whether checks of `commit` that were starting still apply to the task as it is now: not
/// when it was stopped, or has a newer change, meanwhile.
fn still_checks(now: &Task, commit: &str) -> bool {
    !now.state.is_final()
        && now
            .candidate
            .as_ref()
            .is_some_and(|candidate| candidate.commit == commit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work::ChecksResult;

    fn report(done_when: &[&str], checks: Option<ChecksResult>) -> Report {
        Report {
            summary: "Checked.".into(),
            changes: Vec::new(),
            decisions: Vec::new(),
            verification: Vec::new(),
            done_when: done_when.iter().map(|line| (*line).to_owned()).collect(),
            open_questions: Vec::new(),
            risks: Vec::new(),
            needs_user: Vec::new(),
            verdict: None,
            checks,
            artifacts: Vec::new(),
            submitted_at_ms: 0,
        }
    }

    #[test]
    fn decided_for_you_counts_findings_and_keeps_only_brigadiers_words() {
        let member = |id: &str, role, result| GateMember {
            task_id: TaskId(id.into()),
            role,
            result,
            avoid: Vec::new(),
        };
        let failed = |findings: &[&str]| {
            Some(GateResult::Failed {
                findings: findings.iter().map(|f| (*f).to_owned()).collect(),
            })
        };
        let mut gate = Gate {
            round: 1,
            commit: Some("c1".into()),
            members: vec![
                member("r1", GateRole::Review, failed(&["a", "b"])),
                member("r2", GateRole::Review, failed(&["c"])),
                member("v", GateRole::Verify, failed(&["d"])),
            ],
            outcome: None,
            relanding: false,
            retry: false,
            overridden: false,
            findings: Vec::new(),
        };
        assert_eq!(
            finding_counts(&gate),
            "3 review findings, 1 verification finding"
        );
        gate.members = vec![member("v", GateRole::Verify, Some(GateResult::Passed))];
        assert_eq!(finding_counts(&gate), "its checks' findings");
        assert_eq!(
            reason_heads(
                "- The project's checks could not run: pnpm smoke needs a display\n- Left unchecked: c2; c3\n- The project's checks could not run: cargo test"
            ),
            "The project's checks could not run. Left unchecked."
        );
    }

    #[test]
    fn every_criterion_met_with_evidence_and_passing_checks_passes() {
        let report = report(
            &[
                "[met] `pnpm test` passes: 41 passed, 0 failed",
                "- [Met] the flag is documented: README.md line 40",
            ],
            Some(ChecksResult::Passed),
        );
        assert_eq!(verify_result(&report, 0), GateResult::Passed);
    }

    #[test]
    fn a_project_without_checks_passes_on_evidence_alone() {
        let report = report(
            &["[met] the page title reads Home: src/app.tsx:12"],
            Some(ChecksResult::NoChecks),
        );
        assert_eq!(verify_result(&report, 0), GateResult::Passed);
    }

    #[test]
    fn checks_that_could_not_run_never_pass() {
        let report = report(
            &["[met] the endpoint returns 200: curl showed 200 OK"],
            Some(ChecksResult::NotRun),
        );
        assert!(matches!(
            verify_result(&report, 0),
            GateResult::Unverified { .. }
        ));
    }

    #[test]
    fn a_check_that_cannot_run_on_the_parent_either_does_not_hold_the_change() {
        let met = [
            "[met] CONTRIBUTORS.md lists the bot: CONTRIBUTORS.md:7",
            "[met] npm test passes: 12 passed, 0 failed",
        ];
        let gap = "- [Pre-existing] npm run test:integration: on the parent too it stops at `CALC_API_TOKEN is not set`";
        let mut passed = report(&met, Some(ChecksResult::Passed));
        passed.risks = vec![gap.into()];
        assert_eq!(verify_result(&passed, 2), GateResult::Passed);
        // Checks called notRun never pass, even beside such a gap: another check may have
        // failed to run without its own line.
        let mut not_run = report(&met, Some(ChecksResult::NotRun));
        not_run.risks = vec![gap.into(), "The diff is small.".into()];
        assert!(matches!(
            verify_result(&not_run, 2),
            GateResult::Unverified { .. }
        ));
        // A check this change could have run, but didn't, holds it, and is named.
        let blocked = "[not run] npm run e2e: the browser download failed";
        not_run.risks.push(blocked.into());
        let GateResult::Unverified { reason } = verify_result(&not_run, 2) else {
            panic!("not unverified");
        };
        assert!(reason.contains("npm run e2e"), "{reason}");
        passed.risks.push(blocked.into());
        assert!(matches!(
            verify_result(&passed, 2),
            GateResult::Unverified { .. }
        ));
        // So does any criterion left unchecked.
        let mut unchecked = report(
            &[
                "[met] lint passes: 0 warnings",
                "[not checked] the API answers: no token",
            ],
            Some(ChecksResult::NotRun),
        );
        unchecked.risks = vec![gap.into()];
        assert!(matches!(
            verify_result(&unchecked, 0),
            GateResult::Unverified { .. }
        ));
    }

    #[test]
    fn a_check_a_rule_or_the_sandbox_excludes_never_holds_the_change() {
        let met = ["[met] the note exists: docs/notes.md, 210 lines"];
        let excluded = "[excluded] pnpm desktop --smoke: the sandbox can't open windows";
        for checks in [
            ChecksResult::Passed,
            ChecksResult::NotRun,
            ChecksResult::NoChecks,
        ] {
            let mut report = report(&met, Some(checks));
            report.risks = vec![
                excluded.into(),
                "[Excluded] daemon start: the task says not to".into(),
            ];
            assert_eq!(verify_result(&report, 1), GateResult::Passed, "{checks:?}");
        }
        // Without its rule it proves nothing.
        let mut bare = report(&met, Some(ChecksResult::NotRun));
        bare.risks = vec!["[excluded] pnpm desktop --smoke".into()];
        assert!(matches!(
            verify_result(&bare, 1),
            GateResult::Unverified { .. }
        ));
        // A check that didn't run for another reason still holds it.
        let mut other = report(&met, Some(ChecksResult::NotRun));
        other.risks = vec![
            excluded.into(),
            "[not run] pnpm test: vitest crashed".into(),
        ];
        assert!(matches!(
            verify_result(&other, 1),
            GateResult::Unverified { .. }
        ));
    }

    #[test]
    fn a_sandboxed_verifier_is_never_told_to_install_or_try_another_way() {
        let task = gated(TaskState::Reviewing, 1, "c1", None, "c1");
        let sandboxed = verify_spec(&task, "c1", None, None, true);
        for gone in [
            "Install missing",
            "another way",
            "install what is missing",
            "runtime smoke check",
        ] {
            assert!(!sandboxed.contains(gone), "{gone}: {sandboxed}");
        }
        assert!(
            sandboxed.contains("copied into this checkout"),
            "{sandboxed}"
        );
        // Nor when it checks again what an earlier verifier couldn't.
        let why = "[not run] pnpm test: no node_modules".to_owned();
        let again = verify_spec(&task, "c1", Some(&why), None, true);
        assert!(!again.contains("Find a way"), "{again}");
        assert!(again.contains("what your sandbox refuses is [not run] or [excluded]"));
        assert!(sandboxed.contains("[excluded] <the check>: the sandbox can't open windows"));
        assert!(sandboxed.contains("test data folder"), "{sandboxed}");
        let full = verify_spec(&task, "c1", None, None, false);
        assert!(full.contains("try another way"), "{full}");
        assert!(!full.contains("can't open windows"), "{full}");
        assert!(
            full.contains("[excluded] <the check>: <the rule>"),
            "{full}"
        );
    }

    #[test]
    fn a_pre_existing_gap_counts_only_with_its_check_and_evidence_from_the_parent() {
        let met = ["[met] npm test passes: 12 passed, 0 failed"];
        for gap in [
            "[pre-existing]",
            "[pre-existing] npm run test:integration",
            "[pre-existing] npm run test:integration:",
            "[pre-existing] : CALC_API_TOKEN is not set on the parent either",
            "[pre-existing] npm run test:integration — ",
        ] {
            let mut passed = report(&met, Some(ChecksResult::Passed));
            passed.risks = vec![gap.into()];
            let GateResult::Unverified { reason } = verify_result(&passed, 1) else {
                panic!("not unverified: {gap}");
            };
            assert!(reason.contains("no evidence from the parent"), "{reason}");
        }
        for gap in [
            "[pre-existing] npm run test:integration: the parent stops at `CALC_API_TOKEN is not set` too",
            "* [Pre-existing] cargo test --features gpu — no CUDA here or on the parent",
        ] {
            let mut passed = report(&met, Some(ChecksResult::Passed));
            passed.risks = vec![gap.into()];
            assert_eq!(verify_result(&passed, 1), GateResult::Passed, "{gap}");
            let mut none = report(&met, Some(ChecksResult::NoChecks));
            none.risks = vec![gap.into()];
            assert_eq!(verify_result(&none, 1), GateResult::Passed, "{gap}");
        }
        assert_eq!(
            pre_existing_gap("[pre-existing] npm run lint: the parent fails the same way"),
            Some(("npm run lint", "the parent fails the same way"))
        );
    }

    #[test]
    fn a_change_that_passed_its_checks_waits_on_the_user_for_nothing() {
        let mut gate = gated(TaskState::Reviewing, 1, "c1", None, "c1")
            .gate
            .expect("a gate");
        let mut verifier = report(
            &["[met] npm test passes: 12 passed"],
            Some(ChecksResult::Passed),
        );
        verifier.needs_user = vec!["Set CALC_API_TOKEN to run the integration tests".into()];
        // Passed: what the verifier listed for the user holds nothing, and what was listed
        // before is over.
        gate.outcome = Some(GateOutcome::Passed);
        assert_eq!(landing_wait_lines(&gate, &[&verifier]), Some(Vec::new()));
        assert_eq!(landing_wait_lines(&gate, &[]), Some(Vec::new()));
        // Not verified: it waits on the user.
        gate.outcome = Some(GateOutcome::Unverified);
        assert_eq!(
            landing_wait_lines(&gate, &[&verifier]),
            Some(verifier.needs_user.clone())
        );
        // No verifier reported: the round says nothing about it.
        assert_eq!(landing_wait_lines(&gate, &[]), None);
    }

    #[test]
    fn criteria_reported_all_not_run_are_unverified() {
        let report = report(
            &[
                "[not checked] tests pass: could not run pnpm",
                "[not checked] the build works: no time",
            ],
            Some(ChecksResult::NotRun),
        );
        let GateResult::Unverified { reason } = verify_result(&report, 0) else {
            panic!("not unverified");
        };
        assert!(
            reason.contains("no \"done when\" criterion met"),
            "{reason}"
        );
    }

    #[test]
    fn a_met_line_without_evidence_or_status_is_unchecked() {
        let report = report(
            &[
                "[met] tests pass: ok passing",
                "[met] ok",
                "the build works",
            ],
            Some(ChecksResult::Passed),
        );
        let GateResult::Unverified { reason } = verify_result(&report, 0) else {
            panic!("not unverified");
        };
        assert!(reason.contains("[met] ok (no evidence given)"), "{reason}");
        assert!(reason.contains("the build works"), "{reason}");
    }

    #[test]
    fn no_criteria_at_all_is_unverified() {
        assert!(matches!(
            verify_result(&report(&[], Some(ChecksResult::Passed)), 0),
            GateResult::Unverified { .. }
        ));
    }

    #[test]
    fn an_unmet_criterion_or_failed_check_fails_with_findings() {
        let mut failed = report(
            &[
                "[met] lint passes: oxlint 0 warnings",
                "[not met] tests pass: 2 failed in api.test.ts",
            ],
            Some(ChecksResult::Failed),
        );
        failed.open_questions = vec!["Fix the null check in api.ts".into()];
        assert_eq!(
            verify_result(&failed, 0),
            GateResult::Failed {
                findings: vec![
                    "[not met] tests pass: 2 failed in api.test.ts".into(),
                    "Fix the null check in api.ts".into()
                ]
            }
        );
    }

    #[test]
    fn numbered_and_bulleted_criteria_are_read() {
        for line in [
            "2. [met] tests pass: 41 passed",
            "2) [met] tests pass: 41 passed",
            "- [met] tests pass: 41 passed",
            "* [met] tests pass: 41 passed",
            "• [met] tests pass: 41 passed",
            "  10. [Met] tests pass: 41 passed",
        ] {
            assert_eq!(criterion_status(line), Some(Status::Met), "{line}");
            assert_eq!(criterion_evidence(line), Some("41 passed"), "{line}");
        }
        assert_eq!(
            criterion_status("1. [not met] lint passes: 2 errors"),
            Some(Status::NotMet)
        );
        // A number that is not a list marker is not taken for one.
        assert_eq!(criterion_status("2 [met] tests pass: ok"), None);
        let numbered = report(
            &[
                "1. [met] lerp works: test/math.test.js:14",
                "2. [not met] npm test passes: the exports test fails",
            ],
            Some(ChecksResult::Failed),
        );
        assert_eq!(
            verify_result(&numbered, 0),
            GateResult::Failed {
                findings: vec!["2. [not met] npm test passes: the exports test fails".into()]
            }
        );
    }

    #[test]
    fn a_failed_check_with_every_criterion_met_is_unverified_not_failed() {
        let mut optional = report(
            &[
                "[met] isPrime(1) is true: test/primes.test.js:4 passes",
                "[met] npm test passes: 12 passed, 0 failed",
            ],
            Some(ChecksResult::Failed),
        );
        optional.summary = "Integration could not start: CALC_API_TOKEN is missing.".into();
        let GateResult::Unverified { reason } = verify_result(&optional, 2) else {
            panic!("not unverified");
        };
        assert!(reason.contains("CALC_API_TOKEN"), "{reason}");
        // Anything named to fix still fails it.
        optional.open_questions = vec!["Remove debug.log".into()];
        assert!(matches!(
            verify_result(&optional, 2),
            GateResult::Failed { .. }
        ));
        // So does a failed check while a criterion is left unchecked.
        let unchecked = report(
            &[
                "[met] lint passes: 0 warnings",
                "[not checked] tests pass: could not start",
            ],
            Some(ChecksResult::Failed),
        );
        assert!(matches!(
            verify_result(&unchecked, 0),
            GateResult::Failed { .. }
        ));
    }

    #[test]
    fn a_review_without_a_verdict_gives_no_result() {
        let mut review = report(&[], None);
        assert!(matches!(
            review_result(&review),
            GateResult::NoResult { .. }
        ));
        review.verdict = Some(ReviewVerdict::RequestChanges);
        assert_eq!(
            review_result(&review),
            GateResult::Failed {
                findings: vec!["Checked.".into()]
            }
        );
    }

    #[test]
    fn a_round_is_as_good_as_its_worst_member() {
        let member = |result| GateMember {
            task_id: TaskId("t".into()),
            role: GateRole::Review,
            result: Some(result),
            avoid: Vec::new(),
        };
        let unverified = GateResult::Unverified { reason: "x".into() };
        let failed = GateResult::Failed {
            findings: Vec::new(),
        };
        assert_eq!(
            outcome_of(&[member(GateResult::Passed), member(unverified.clone())]),
            GateOutcome::Unverified
        );
        assert_eq!(
            outcome_of(&[member(unverified), member(failed.clone())]),
            GateOutcome::Failed
        );
        assert_eq!(
            outcome_of(&[
                member(failed),
                member(GateResult::NoResult { reason: "x".into() })
            ]),
            GateOutcome::NoResult
        );
    }

    #[test]
    fn any_new_file_counts_as_a_verifier_change() {
        let change = |path: &str, untracked| brigadier_git::Change {
            path: path.into(),
            kind: brigadier_git::ChangeKind::Added,
            untracked,
        };
        assert_eq!(checkout_changes(&[]), None);
        // Not only source: a migration, a script or a page a check could have used.
        assert_eq!(
            checkout_changes(&[change("db/001.sql", true), change("run.sh", true)]),
            Some("it changed db/001.sql, run.sh".into())
        );
        assert_eq!(
            checkout_changes(&[change("src/app.ts", false)]),
            Some("it changed src/app.ts".into())
        );
    }

    #[test]
    fn a_verifier_finding_in_open_questions_fails_an_otherwise_passing_report() {
        let mut found = report(
            &["[met] tests pass: 41 passed, 0 failed"],
            Some(ChecksResult::Passed),
        );
        found.open_questions = vec!["debug.log is committed; remove it".into()];
        assert_eq!(
            verify_result(&found, 1),
            GateResult::Failed {
                findings: vec!["debug.log is committed; remove it".into()]
            }
        );
    }

    #[test]
    fn fewer_criteria_than_the_worker_listed_is_unverified() {
        let short = report(
            &["[met] tests pass: 41 passed, 0 failed"],
            Some(ChecksResult::Passed),
        );
        let GateResult::Unverified { reason } = verify_result(&short, 2) else {
            panic!("not unverified");
        };
        assert!(reason.contains("covered 1"), "{reason}");
        assert!(reason.contains("listed 2"), "{reason}");
        // As many as the worker listed, or more (the task's own), passes.
        assert_eq!(verify_result(&short, 1), GateResult::Passed);
    }

    #[test]
    fn a_met_line_needs_evidence_after_its_criterion_not_just_length() {
        let long = report(
            &[
                "[met] tests pass: 41 passed",
                "[met] the settings page shows the new toggle for dark mode",
            ],
            Some(ChecksResult::Passed),
        );
        let GateResult::Unverified { reason } = verify_result(&long, 0) else {
            panic!("not unverified");
        };
        assert!(reason.contains("(no evidence given)"), "{reason}");
        assert_eq!(
            criterion_evidence("[met] the build works — cargo build finished in 12s"),
            Some("cargo build finished in 12s")
        );
        assert_eq!(
            criterion_evidence("- [met] tests pass - 41 passed"),
            Some("41 passed")
        );
        assert_eq!(criterion_evidence("[met] tests pass:"), None);
        assert_eq!(criterion_evidence("[met] tests pass: -"), None);
        // A colon inside a path is not a separator.
        assert_eq!(criterion_evidence("[met] see src/app.ts:12"), None);
    }

    fn author(model: &str) -> Author {
        Author {
            provider: brigadier_providers::ProviderKind::Codex,
            model: Some(model.into()),
        }
    }

    #[test]
    fn a_gate_member_keeps_avoiding_what_it_started_avoiding() {
        let member = |id: &str, role, avoid: &[&str]| GateMember {
            task_id: TaskId(id.into()),
            role,
            result: None,
            avoid: avoid
                .iter()
                .map(|model| ModelChoice {
                    provider: brigadier_providers::ProviderKind::Codex,
                    model: Some((*model).into()),
                    effort: None,
                    fast: None,
                })
                .collect(),
        };
        let gate = Gate {
            round: 2,
            commit: Some("c2".into()),
            members: vec![
                member("r1", GateRole::Review, &[]),
                member("r2", GateRole::Review, &[]),
                member("v", GateRole::Verify, &["first-verifier"]),
            ],
            outcome: None,
            relanding: false,
            retry: true,
            overridden: false,
            findings: Vec::new(),
        };
        let model_of = |id: &TaskId| Some(author(&format!("model-of-{}", id.0)));
        // The second verifier: the one that could not check, not the reviewers.
        assert_eq!(
            member_avoids(&gate, &TaskId("v".into()), GateRole::Verify, model_of),
            vec![author("first-verifier")]
        );
        // A reviewer: the other reviewer.
        assert_eq!(
            member_avoids(&gate, &TaskId("r1".into()), GateRole::Review, model_of),
            vec![author("model-of-r2")]
        );
    }

    /// A write task whose round `round` on `commit` ended with `outcome`, now at `state`
    /// with candidate `candidate`.
    fn gated(
        state: TaskState,
        round: u32,
        commit: &str,
        outcome: Option<GateOutcome>,
        candidate: &str,
    ) -> Task {
        let mut task: Task = serde_json::from_value(serde_json::json!({
            "id": "t1",
            "conversationId": "c1",
            "number": 1,
            "position": 0,
            "title": "Add the flag",
            "kind": "implement",
            "spec": "Add the flag.",
            "access": { "repo": "write", "network": false, "unsandboxed": false },
            "route": { "choice": { "provider": "claude", "model": null, "effort": null }, "reason": "" },
            "state": "reviewing",
            "attachments": [],
            "createdAtMs": 0,
            "updatedAtMs": 0
        }))
        .expect("a task");
        task.state = state;
        task.gate = Some(Gate {
            round,
            commit: Some(commit.into()),
            members: Vec::new(),
            outcome,
            relanding: false,
            retry: false,
            overridden: false,
            findings: Vec::new(),
        });
        task.candidate = Some(crate::work::Candidate {
            commit: candidate.into(),
            onto: "base".into(),
            message: "Add the flag".into(),
            diff_stat: crate::work::DiffStat {
                files: Vec::new(),
                insertions: 1,
                deletions: 0,
            },
            excluded: Vec::new(),
            diff: None,
        });
        task
    }

    #[test]
    fn an_old_approval_never_lands_a_newer_candidate() {
        let passed = Some(GateOutcome::Passed);
        let approved = gated(TaskState::AwaitingApproval, 1, "c1", passed.clone(), "c1");
        assert!(round_current(&approved, &approved));
        assert!(candidate_passed(&approved));
        // Sent back and accepted again: round 2 checks c2 while round 1's card is open.
        let newer = gated(TaskState::Reviewing, 2, "c2", None, "c2");
        assert!(!round_current(&approved, &newer));
        assert!(!candidate_passed(&newer));
        // Sent back, still working: nothing lands.
        let working = gated(TaskState::Running, 1, "c1", passed.clone(), "c1");
        assert!(!round_current(&approved, &working));
        // Stopped meanwhile.
        let stopped = gated(TaskState::Stopped, 1, "c1", passed.clone(), "c1");
        assert!(!round_current(&approved, &stopped));
        // A candidate that is not the commit the round passed.
        let moved = gated(TaskState::Reviewing, 1, "c1", passed, "c3");
        assert!(!candidate_passed(&moved));
    }

    #[test]
    fn checks_that_were_starting_lapse_once_the_task_stops_or_moves_on() {
        // Accepted, its checks starting: they apply.
        assert!(still_checks(
            &gated(TaskState::Reported, 1, "c0", None, "c1"),
            "c1"
        ));
        // Stopped meanwhile: a round must not bring it back.
        assert!(!still_checks(
            &gated(TaskState::Stopped, 1, "c0", None, "c1"),
            "c1"
        ));
        // A newer change meanwhile: the checks of the older one are moot.
        assert!(!still_checks(
            &gated(TaskState::Reported, 1, "c0", None, "c2"),
            "c1"
        ));
    }

    #[test]
    fn a_retried_verification_keeps_the_users_approval() {
        let mut previous = gated(TaskState::Reviewing, 2, "c1", None, "c1")
            .gate
            .expect("a gate");
        // A clean replay of an approved change lands once verified.
        assert!(round_relanding(Recheck::Verify, false, Some(&previous)));
        // Its verifier couldn't check it: the retry still lands without asking again.
        previous.relanding = true;
        assert!(round_relanding(Recheck::Verify, true, Some(&previous)));
        // A retry of a round the user hadn't approved yet waits for them as before.
        previous.relanding = false;
        assert!(!round_relanding(Recheck::Verify, true, Some(&previous)));
        assert!(!round_relanding(Recheck::Full, false, Some(&previous)));
    }

    #[test]
    fn checks_that_ended_on_an_unchanged_change_are_not_an_invitation_to_accept_it_again() {
        let failed = gated(
            TaskState::Reported,
            3,
            "c1",
            Some(GateOutcome::Failed),
            "c1",
        );
        assert_eq!(checks_stand(&failed), Some(GateOutcome::Failed));
        let note = super::super::prompts::undecided_note(std::slice::from_ref(&failed));
        assert!(note.contains("will not land it"), "{note}");
        let unfinished = gated(
            TaskState::Reported,
            1,
            "c1",
            Some(GateOutcome::NoResult),
            "c1",
        );
        assert_eq!(checks_stand(&unfinished), Some(GateOutcome::NoResult));
        // Sent back since (its candidate is void), then reported again: a new decision.
        let mut fixed = failed.clone();
        fixed.candidate = None;
        assert_eq!(checks_stand(&fixed), None);
        let note = super::super::prompts::undecided_note(&[fixed]);
        assert!(!note.contains("will not land"), "{note}");
        let passed = gated(
            TaskState::Reported,
            1,
            "c1",
            Some(GateOutcome::Passed),
            "c1",
        );
        assert_eq!(checks_stand(&passed), None);
    }

    #[test]
    fn a_change_the_user_said_to_land_anyway_may_land_as_it_is() {
        let mut failed = gated(
            TaskState::Reviewing,
            2,
            "c1",
            Some(GateOutcome::Failed),
            "c1",
        );
        assert!(!candidate_passed(&failed));
        failed.gate.as_mut().expect("a gate").overridden = true;
        assert!(candidate_passed(&failed));
        // Only that change: a newer candidate is checked as usual.
        let mut newer = failed.clone();
        newer.candidate.as_mut().expect("a candidate").commit = "c2".into();
        assert!(!candidate_passed(&newer));
        // Checks that could not finish are no findings to override.
        let mut unfinished = failed;
        unfinished.gate.as_mut().expect("a gate").outcome = Some(GateOutcome::NoResult);
        assert!(!candidate_passed(&unfinished));
    }

    #[test]
    fn only_the_user_speaking_after_the_findings_lets_a_change_land_despite_them() {
        let mut task = gated(
            TaskState::Reported,
            2,
            "c1",
            Some(GateOutcome::Failed),
            "c1",
        );
        // The orchestrator never handed the findings on: nothing the user said was about them.
        assert!(!user_spoke_since_checks(&task, Some(2_000)));
        task.escalated = Some(crate::work::Escalated {
            round: 2,
            commit: "c1".into(),
            at_ms: 1_000,
        });
        // Nothing from the user since, or only from before.
        assert!(!user_spoke_since_checks(&task, None));
        assert!(!user_spoke_since_checks(&task, Some(900)));
        assert!(!user_spoke_since_checks(&task, Some(1_000)));
        assert!(user_spoke_since_checks(&task, Some(1_001)));
        // Findings of an earlier round, or about another candidate, are not these.
        let mut later_round = task.clone();
        later_round.gate.as_mut().expect("a gate").round = 3;
        assert!(!user_spoke_since_checks(&later_round, Some(2_000)));
        let mut newer = task.clone();
        newer.candidate.as_mut().expect("a candidate").commit = "c2".into();
        assert!(!user_spoke_since_checks(&newer, Some(2_000)));
    }

    fn big(task: &mut Task) {
        let stat = &mut task.candidate.as_mut().expect("a candidate").diff_stat;
        stat.files = (0..4)
            .map(|n| crate::work::FileStat {
                path: format!("src/{n}.js"),
                insertions: 2,
                deletions: 0,
                binary: false,
            })
            .collect();
        stat.insertions = 8;
    }

    #[test]
    fn a_missing_plan_is_a_risk_on_the_first_round_only() {
        let mut task = gated(TaskState::Reviewing, 1, "c1", None, "c1");
        assert_eq!(plan_note(&task, false, true), None);
        big(&mut task);
        let note = plan_note(&task, false, true).expect("a note");
        assert!(note.contains("never a finding"), "{note}");
        assert!(note.contains("never make it a [not met] line"), "{note}");
        // A fix round's checks (or any later round) never hear of it again.
        assert_eq!(plan_note(&task, false, false), None);
        // A plan the worker wrote is checked against on every round.
        assert!(plan_note(&task, true, false).is_some_and(|note| note.contains("plan.md")));
    }

    #[test]
    fn a_verifier_hears_of_later_messages_only_when_there_are_some() {
        let mut task = gated(TaskState::Reviewing, 1, "c1", None, "c1");
        let spec = verify_spec(&task, "c1", None, None, true);
        assert!(!spec.contains("later messages"), "{spec}");
        assert!(spec.contains("sent the worker nothing"), "{spec}");
        task.messages = vec!["Also update the README.".into()];
        let spec = verify_spec(&task, "c1", None, None, true);
        assert!(spec.contains("the orchestrator's later messages"), "{spec}");
    }

    #[test]
    fn checks_judge_the_change_with_what_the_worker_was_sent_back_to_fix() {
        let mut task = gated(TaskState::Reviewing, 2, "c2", None, "c2");
        for spec in [
            review_spec(&task, "c2", &[], None),
            verify_spec(&task, "c2", None, None, true),
        ] {
            assert!(spec.contains(JUDGE_THE_CHANGE), "{spec}");
            assert!(!spec.contains("Brigadier sent the worker back"), "{spec}");
        }
        task.fixes = vec!["From the review (task-2):\n- Re-export avg2".into()];
        for spec in [
            review_spec(&task, "c2", &[], None),
            verify_spec(&task, "c2", None, None, true),
        ] {
            assert!(
                spec.contains("Brigadier sent the worker back once"),
                "{spec}"
            );
        }
    }

    #[test]
    fn a_verifier_checks_a_failure_against_the_parent_commit() {
        let task = gated(TaskState::Reviewing, 1, "c1", None, "c1");
        let spec = verify_spec(&task, "c1", None, None, true);
        assert!(spec.contains("git archive HEAD~1"), "{spec}");
        assert!(spec.contains("already there on the parent"), "{spec}");
        assert!(spec.contains("failed because of this change"), "{spec}");
    }

    #[test]
    fn the_orchestrator_hears_what_brigadier_already_had_fixed() {
        assert_eq!(fixes_text(&[]), "");
        let text = fixes_text(&[
            "From the review (task-2):\n- Re-export avg2".into(),
            "From the verification (task-5):\n- [not met] npm test passes".into(),
        ]);
        assert!(text.contains("sent the worker back twice"), "{text}");
        assert!(
            text.contains("Fix 1:\nFrom the review (task-2):\n- Re-export avg2"),
            "{text}"
        );
        assert!(
            text.contains("Fix 2:\nFrom the verification (task-5)"),
            "{text}"
        );
    }

    fn touching(task: &mut Task, paths: &[&str]) {
        task.candidate
            .as_mut()
            .expect("a candidate")
            .diff_stat
            .files = paths
            .iter()
            .map(|path| crate::work::FileStat {
                path: (*path).into(),
                insertions: 1,
                deletions: 0,
                binary: false,
            })
            .collect();
    }

    fn checked(id: &str, role: GateRole, result: GateResult) -> GateMember {
        GateMember {
            task_id: TaskId(id.into()),
            role,
            result: Some(result),
            avoid: Vec::new(),
        }
    }

    #[test]
    fn a_change_gets_one_reviewer_none_for_docs_and_two_where_mistakes_cost_most() {
        let mut task = gated(TaskState::Reviewing, 1, "c1", None, "c1");
        task.gate = None;
        touching(
            &mut task,
            &["src/app.ts", "src/digit.rs", ".github/workflows/ci.yml"],
        );
        assert_eq!(SessionManager::panel_size(&task), 1);
        touching(&mut task, &["README.md", "docs/PLAN.md", "docs/shot.png"]);
        assert_eq!(SessionManager::panel_size(&task), 0);
        touching(&mut task, &["README.md", "src/app.ts"]);
        assert_eq!(SessionManager::panel_size(&task), 1);
        for risky in [
            "crates/core/src/manager/landing.rs",
            "crates/providers/src/policy.rs",
            "crates/sandbox/src/lib.rs",
            "crates/git/src/lib.rs",
            "src/git_actions.rs",
        ] {
            touching(&mut task, &["src/app.ts", risky]);
            assert_eq!(SessionManager::panel_size(&task), 2, "{risky}");
        }
        assert!(!risky_path("src/digit.rs") && !risky_path(".github/x.yml"));
        assert!(!docs_only(&[]));
        // Files that build things, and code in a folder named docs, are not documentation.
        for code in [
            "requirements.txt",
            "CMakeLists.txt",
            "apps/desktop/src/app/docs/Viewer.tsx",
        ] {
            touching(&mut task, &[code]);
            assert_eq!(SessionManager::panel_size(&task), 1, "{code}");
        }
    }

    #[test]
    fn a_fix_round_asks_again_only_the_reviewers_that_asked_for_changes() {
        let mut task = gated(
            TaskState::Reviewing,
            1,
            "c1",
            Some(GateOutcome::Failed),
            "c2",
        );
        touching(&mut task, &["crates/git/src/lib.rs"]);
        let gate = task.gate.as_mut().expect("a gate");
        gate.members = vec![
            checked("v", GateRole::Verify, GateResult::Passed),
            checked("r1", GateRole::Review, GateResult::Passed),
            checked(
                "r2",
                GateRole::Review,
                GateResult::Failed {
                    findings: vec!["x".into()],
                },
            ),
        ];
        assert_eq!(SessionManager::panel_size(&task), 1);
        let kept = kept_approvals(&task).expect("an approval kept");
        assert!(
            kept.contains("A reviewer of the change at c1 approved it"),
            "{kept}"
        );
        assert!(kept.contains("git diff c1 HEAD"), "{kept}");
        // Only the verifier asked for changes: no reviewer runs again, its fresh verifier
        // checks the fix against both kept approvals.
        let gate = task.gate.as_mut().expect("a gate");
        gate.members[2].result = Some(GateResult::Passed);
        gate.members[0].result = Some(GateResult::Failed {
            findings: vec!["x".into()],
        });
        assert_eq!(SessionManager::panel_size(&task), 0);
        assert!(kept_approvals(&task).is_some_and(|k| k.starts_with("Its reviewers")));
        // A round that passed or couldn't verify keeps nothing; a first round has nothing.
        task.gate.as_mut().expect("a gate").outcome = Some(GateOutcome::Unverified);
        assert_eq!(kept_approvals(&task), None);
        assert_eq!(SessionManager::panel_size(&task), 2);
        // A documentation change whose fix touches code: its round had no reviewer, so the
        // fix gets the reviewers a code change gets.
        let gate = task.gate.as_mut().expect("a gate");
        gate.outcome = Some(GateOutcome::Failed);
        gate.members = vec![checked(
            "v",
            GateRole::Verify,
            GateResult::Failed {
                findings: vec!["x".into()],
            },
        )];
        touching(&mut task, &["README.md", "src/app.ts"]);
        assert_eq!(SessionManager::panel_size(&task), 1);
    }

    #[test]
    fn a_docs_only_verifier_runs_no_build_or_tests() {
        let task = gated(TaskState::Reviewing, 1, "c1", None, "c1");
        let spec = verify_docs_spec(&task, "c1", None);
        // A worker's "tests pass" line never holds a change to documentation only.
        assert!(spec.contains("can't change with documentation"), "{spec}");
        assert!(
            spec.contains("Don't run builds, tests or smoke checks"),
            "{spec}"
        );
        assert!(!spec.contains("install"), "{spec}");
    }
}
