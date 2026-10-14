//! The session's full checks, run once on the integrated tree (SESSION-FIXES-PLAN.md §11).
//!
//! Workers run only the checks of their own change and hand back; the thread lands their work
//! at once. What the work reaches beyond each change (the packages that depend on it, the
//! workspace's own files, a build of the app) is checked here, once, on the branch the work
//! landed on, instead of once per worker in every worktree.
//!
//! - **Due.** A landing, or commits of the thread's own, make the branch's new tip due
//!   ([`SessionManager::checks_due`]). Landings close together make one due check: its tip
//!   moves with each, and it starts once they settle for [`SETTLE`], or after the run before
//!   it. A tip a check already ran on, or runs on, is not made due again.
//! - **One run at a time per session**, in a checkout of its own (`checks/<id>/tree`, owned by
//!   [`checks_owner`]): made warm once, then moved to each tip, so its build output stays and
//!   no landing changes files under a running build. Its dependency installs are made again
//!   when the files that pin them change. The commands are the project's own full checks
//!   (`checks::full_checks`), each through `run_check`'s keyed cache, so the same files under
//!   the same inputs never run again unless the thread asks (`rerun_checks`), and always with
//!   the build lease.
//! - **The result** reaches the thread: a failure with its command, output and the tasks that
//!   landed since the checks last passed, for the thread to send back to the worker that broke
//!   it (a fix task resumes that worker's session, kept until a pass covers its landing); a
//!   pass when a request's answer waits for it; a run that couldn't finish as such, never as a
//!   verdict. A merge of the session's work needs a pass at the exact tip it merges.
//! - **Recorded** as [`IntegratedCheck`]s from the moment they are due: a restart runs a due or
//!   cut-off check again; a session archived or deleted ends its run and removes its checkout,
//!   and hears nothing after.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use brigadier_git::{Oid, WorktreeSpec};
use brigadier_providers::Access;
use tokio_util::sync::CancellationToken;

use super::conversation::Envelope;
use super::{SessionManager, blocking, git_error};
use crate::model::{ConversationId, DomainEvent};
use crate::work::{
    InjectionKind, IntegratedCheck, IntegratedCheckState, RequestState, Task, TaskId, TaskKind,
};
use crate::{Error, Result, now_ms};

/// How long landings must settle before a due check starts.
#[cfg_attr(test, allow(dead_code))]
const SETTLE: Duration = Duration::from_secs(15);
/// How long a merge (or a phase settled done) waits in its call for the checks of its tip,
/// started at once: a project's quick or kept checks answer it there, others hold it.
/// Tests wait less for a check they hold on purpose.
pub(crate) const MERGE_WAIT: Duration = Duration::from_secs(if cfg!(test) { 10 } else { 30 });
/// How long one command of the full checks may run.
const FULL_CHECK_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// How long an install of the checkout's dependencies may run.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// The most of a failure's output its message carries; `read_artifact` has the rest.
const EXCERPT_MAX: usize = 4_000;

/// The cleanup-ledger owner of a session's check checkout and the processes run there.
pub(crate) fn checks_owner(id: &ConversationId) -> String {
    format!("checks:{id}")
}

/// A check's number, from its id (`check-7`); 0 for one that has none.
pub(crate) fn integrated_check_number(id: &str) -> u64 {
    id.strip_prefix("check-")
        .and_then(|number| number.parse().ok())
        .unwrap_or(0)
}

/// What the sessions' full checks need while the daemon runs.
#[derive(Default)]
pub(crate) struct IntegratedChecks {
    /// Each session's queue runner, while it has a due check.
    jobs: tokio::sync::Mutex<HashMap<ConversationId, Arc<Job>>>,
    /// Held while a session's checks are read, changed and recorded.
    records: tokio::sync::Mutex<()>,
    /// Woken each time a check ends.
    ended: tokio::sync::Notify,
    /// Tests: the settle wait instead of [`SETTLE`] (100 ms when unset).
    #[cfg(test)]
    pub(crate) settle: std::sync::Mutex<Option<Duration>>,
}

/// A session's queue runner.
struct Job {
    wake: tokio::sync::Notify,
    /// Ends it, and the command it runs: the session closed.
    cancel: CancellationToken,
}

/// How a check is made due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Due {
    /// Once landings settle.
    Settled,
    /// Now: a merge waits for it.
    AtOnce,
    /// Now, without kept results.
    Rerun,
}

/// Where the full checks stand at one tip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TipChecked {
    Passed,
    /// Due or running.
    Pending,
    Failed {
        command: String,
        status: String,
    },
    /// The run couldn't finish: no verdict.
    Couldnt {
        reason: String,
    },
    /// No check was ever due at it.
    Never,
}

/// Where the full checks of `branch` stand at `tip`: the newest check of that tip decides.
pub(crate) fn tip_checked(board: &crate::board::Board, branch: &str, tip: &str) -> TipChecked {
    let newest = board
        .integrated_checks
        .values()
        .filter(|check| check.branch == branch && check.tip == tip)
        .max_by_key(|check| integrated_check_number(&check.id));
    match newest.map(|check| &check.state) {
        None => TipChecked::Never,
        Some(IntegratedCheckState::Passed { .. }) => TipChecked::Passed,
        Some(IntegratedCheckState::Due | IntegratedCheckState::Running { .. }) => {
            TipChecked::Pending
        }
        Some(IntegratedCheckState::Failed {
            command, status, ..
        }) => TipChecked::Failed {
            command: command.clone(),
            status: status.clone(),
        },
        Some(IntegratedCheckState::Interrupted { reason }) => TipChecked::Couldnt {
            reason: reason.clone(),
        },
    }
}

/// Whether a check of the session runs, or is due, for `request`: its answer waits for it.
pub(crate) fn checking_for(board: &crate::board::Board, request: &str) -> bool {
    board
        .integrated_checks
        .values()
        .any(|check| !check.state.is_final() && check.requests.iter().any(|asked| asked == request))
}

/// The line a request's notes end with while its full checks run.
pub(crate) fn checking_line(board: &crate::board::Board, request: &str) -> Option<String> {
    let check = board
        .integrated_checks
        .values()
        .filter(|check| {
            !check.state.is_final() && check.requests.iter().any(|asked| asked == request)
        })
        .min_by_key(|check| integrated_check_number(&check.id))?;
    Some(format!(
        "[the session's full checks {} on `{}` at {}, in the background: a failure comes as a message. Answer now, or reply {} to answer with their result.]",
        match check.state {
            IntegratedCheckState::Due => "are due",
            _ => "run",
        },
        check.branch,
        short(&check.tip),
        super::prompts::QUIET
    ))
}

/// Whether `branch` may merge at its tip: a merge needs the session's full checks passed at
/// the exact tip it merges.
pub(crate) enum ChecksGate {
    /// They passed at `0`, the tip it merges.
    Passed(Oid),
    /// They are due or run there (made due, or woken, by the look): why, for the thread.
    Waiting(String),
    /// They failed there, or couldn't run: why.
    Failed(String),
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(10)]
}

impl SessionManager {
    /// The settle wait of a due check.
    #[cfg(not(test))]
    fn check_settle(&self) -> Duration {
        SETTLE
    }

    /// The settle wait of a due check in tests: a short one, unless a test sets its own.
    #[cfg(test)]
    fn check_settle(&self) -> Duration {
        self.integrated
            .settle
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .unwrap_or(Duration::from_millis(100))
    }

    /// `branch` of `repo` moved to `tip` with the session's work (a landing, or the thread's
    /// own commits): its full checks are due there, for `request`.
    pub(crate) async fn checks_due(
        &self,
        id: &ConversationId,
        repo: &Path,
        branch: &str,
        tip: &Oid,
        request: Option<String>,
    ) {
        self.make_due(id, repo, branch, tip, request, Due::Settled)
            .await;
        self.run_due_checks(id).await;
    }

    /// Where the full checks of `branch` of `repo` stand for a merge of its tip: a tip never
    /// checked is made due and a due one started, both at once (a merge is asked: no landing
    /// is waited for), and their verdict waited for up to `wait`.
    pub(crate) async fn checks_gate(
        &self,
        id: &ConversationId,
        repo: &Path,
        branch: &str,
        wait: Duration,
    ) -> Result<ChecksGate> {
        let (git, at, name) = (self.git.clone(), repo.to_owned(), branch.to_owned());
        let tip = blocking(move || {
            git.open(&at)
                .map_err(git_error)?
                .branch_tip(&name)
                .map_err(git_error)?
                .ok_or_else(|| Error::Invalid(format!("branch {name} is gone")))
        })
        .await?;
        let at = short(&tip.0).to_owned();
        let deadline = tokio::time::Instant::now() + wait;
        let mut started = false;
        let checked = loop {
            let ended = self.integrated.ended.notified();
            let board = self.core.board(id).await?;
            let checked = tip_checked(&board, branch, &tip.0);
            if !matches!(checked, TipChecked::Never | TipChecked::Pending) {
                break checked;
            }
            if !started {
                started = true;
                let request = self.request_for(id, None).await;
                self.make_due(id, repo, branch, &tip, request, Due::AtOnce)
                    .await;
                self.run_due_checks(id).await;
                continue;
            }
            if tokio::time::timeout_at(deadline, ended).await.is_err() {
                break checked;
            }
        };
        Ok(match checked {
            TipChecked::Passed => ChecksGate::Passed(tip),
            TipChecked::Never => ChecksGate::Waiting(format!("are due at {at}")),
            TipChecked::Pending => ChecksGate::Waiting(format!("still run at {at}")),
            TipChecked::Failed { command, status } => {
                ChecksGate::Failed(format!("failed at {at}: `{command}` ({status})"))
            }
            TipChecked::Couldnt { reason } => {
                ChecksGate::Failed(format!("couldn't run at {at}: {reason}"))
            }
        })
    }

    /// `rerun_checks`: the session's full checks run again at its tip, at once, without
    /// kept results.
    pub(crate) async fn rerun_checks(&self, id: &ConversationId) -> Result<String> {
        let workspace = self
            .recorded_workspace(id)
            .ok_or_else(|| Error::Invalid("this session has no checkout to check yet".into()))?;
        let (git, repo, branch) = (
            self.git.clone(),
            workspace.repo.clone(),
            workspace.branch.clone(),
        );
        let tip = blocking(move || {
            git.open(&repo)
                .map_err(git_error)?
                .branch_tip(&branch)
                .map_err(git_error)?
                .ok_or_else(|| Error::Invalid(format!("branch {branch} is gone")))
        })
        .await?;
        let request = self.request_for(id, None).await;
        self.make_due(
            id,
            &workspace.repo,
            &workspace.branch,
            &tip,
            request,
            Due::Rerun,
        )
        .await;
        self.run_due_checks(id).await;
        Ok(format!(
            "The session's full checks run again on `{}` at {}, without kept results. Their result comes as a message; reply [quiet] meanwhile.",
            workspace.branch,
            short(&tip.0)
        ))
    }

    /// Records `tip` of `branch` as due: the branch's due check moves to it, else a new one
    /// is due, unless a check already ran or runs there (a rerun: always one).
    async fn make_due(
        &self,
        id: &ConversationId,
        repo: &Path,
        branch: &str,
        tip: &Oid,
        request: Option<String>,
        how: Due,
    ) {
        let rerun = how == Due::Rerun;
        let at_once = how != Due::Settled;
        let _records = self.integrated.records.lock().await;
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        let now = now_ms();
        let due = board
            .integrated_checks
            .values()
            .find(|check| check.branch == branch && check.state == IntegratedCheckState::Due)
            .cloned();
        let check = match due {
            Some(mut due) => {
                due.tip = tip.0.clone();
                due.repo = repo.to_string_lossy().into_owned();
                due.due_at_ms = if at_once { now - i64::MAX / 4 } else { now };
                due.rerun |= rerun;
                if let Some(request) = request
                    && !due.requests.contains(&request)
                {
                    due.requests.push(request);
                }
                due
            }
            None => {
                let checked = tip_checked(&board, branch, &tip.0);
                if !rerun
                    && matches!(
                        checked,
                        TipChecked::Passed | TipChecked::Pending | TipChecked::Failed { .. }
                    )
                {
                    // Already checked there, or being checked: a request still waits for it
                    // if it runs.
                    if let (TipChecked::Pending, Some(request)) = (&checked, request) {
                        self.ask_running_check(&board, id, branch, &tip.0, request)
                            .await;
                    }
                    return;
                }
                let number = board
                    .integrated_checks
                    .keys()
                    .map(|id| integrated_check_number(id))
                    .max()
                    .unwrap_or(0)
                    + 1;
                IntegratedCheck {
                    id: format!("check-{number}"),
                    conversation_id: id.clone(),
                    repo: repo.to_string_lossy().into_owned(),
                    branch: branch.to_owned(),
                    tip: tip.0.clone(),
                    state: IntegratedCheckState::Due,
                    due_at_ms: if at_once { now - i64::MAX / 4 } else { now },
                    started_at_ms: None,
                    ended_at_ms: None,
                    requests: request.into_iter().collect(),
                    tasks: Vec::new(),
                    rerun,
                }
            }
        };
        self.store_check(&check).await;
    }

    /// A request now waits for the check that runs at `tip` too.
    async fn ask_running_check(
        &self,
        board: &crate::board::Board,
        id: &ConversationId,
        branch: &str,
        tip: &str,
        request: String,
    ) {
        let running = board
            .integrated_checks
            .values()
            .filter(|check| check.branch == branch && check.tip == tip)
            .max_by_key(|check| integrated_check_number(&check.id))
            .filter(|check| !check.requests.contains(&request))
            .cloned();
        if let Some(mut check) = running {
            check.requests.push(request);
            debug_assert_eq!(&check.conversation_id, id);
            self.store_check(&check).await;
        }
    }

    async fn store_check(&self, check: &IntegratedCheck) {
        if let Err(err) = self
            .core
            .record_conversation(
                &check.conversation_id,
                vec![DomainEvent::IntegratedCheckUpdated {
                    check: check.clone(),
                }],
            )
            .await
        {
            tracing::warn!(check = %check.id, error = %err, "could not record a full check");
        }
        if check.state.is_final() {
            self.integrated.ended.notify_waiters();
        }
    }

    /// Makes sure the session's queue runner runs while it has a due check.
    pub(crate) async fn run_due_checks(&self, id: &ConversationId) {
        let mut jobs = self.integrated.jobs.lock().await;
        if let Some(job) = jobs.get(id) {
            job.wake.notify_one();
            return;
        }
        let job = Arc::new(Job {
            wake: tokio::sync::Notify::new(),
            cancel: CancellationToken::new(),
        });
        jobs.insert(id.clone(), job.clone());
        self.spawn(queue(self.arc(), id.clone(), job));
    }

    /// The session's queue: each due check, oldest first, once landings settled; ends when
    /// none is due.
    async fn checks_loop(&self, id: &ConversationId, job: &Arc<Job>) {
        loop {
            if job.cancel.is_cancelled() {
                break;
            }
            let next = {
                let mut jobs = self.integrated.jobs.lock().await;
                let next = match self.core.board(id).await {
                    Ok(board) => board
                        .integrated_checks
                        .values()
                        .filter(|check| check.state == IntegratedCheckState::Due)
                        .min_by_key(|check| integrated_check_number(&check.id))
                        .cloned(),
                    Err(_) => None,
                };
                if next.is_none() {
                    if jobs.get(id).is_some_and(|now| Arc::ptr_eq(now, job)) {
                        jobs.remove(id);
                    }
                    return;
                }
                next
            };
            let Some(due) = next else {
                return;
            };
            let settle = i64::try_from(self.check_settle().as_millis()).unwrap_or(i64::MAX);
            let wait = due.due_at_ms.saturating_add(settle) - now_ms();
            if wait > 0 {
                let wait = Duration::from_millis(u64::try_from(wait).unwrap_or(0));
                tokio::select! {
                    () = tokio::time::sleep(wait) => {}
                    () = job.wake.notified() => {}
                    () = job.cancel.cancelled() => break,
                }
                continue;
            }
            Box::pin(self.run_integrated_check(id, due, &job.cancel)).await;
        }
        let mut jobs = self.integrated.jobs.lock().await;
        if jobs.get(id).is_some_and(|now| Arc::ptr_eq(now, job)) {
            jobs.remove(id);
        }
    }

    /// Runs one due check to its end and tells the thread.
    async fn run_integrated_check(
        &self,
        id: &ConversationId,
        due: IntegratedCheck,
        cancel: &CancellationToken,
    ) {
        let Ok(_fence) = self.enter(id) else {
            return;
        };
        let repo = PathBuf::from(&due.repo);
        let (git, at, branch) = (self.git.clone(), repo.clone(), due.branch.clone());
        let tip = blocking(move || {
            git.open(&at)
                .map_err(git_error)?
                .branch_tip(&branch)
                .map_err(git_error)
        })
        .await;
        let mut check = {
            let _records = self.integrated.records.lock().await;
            let Ok(board) = self.core.board(id).await else {
                return;
            };
            // Moved on meanwhile (a landing between the read and now moves a due check).
            let Some(mut check) = board.integrated_checks.get(&due.id).cloned() else {
                return;
            };
            if check.state != IntegratedCheckState::Due {
                return;
            }
            match &tip {
                // Run at the branch's tip now: a landing that made it moved the due check too.
                Ok(Some(tip)) => check.tip = tip.0.clone(),
                Ok(None) => {
                    check.state = IntegratedCheckState::Interrupted {
                        reason: format!("its branch `{}` is gone", check.branch),
                    };
                }
                Err(err) => {
                    check.state = IntegratedCheckState::Interrupted {
                        reason: format!("git could not read `{}`: {err}", check.branch),
                    };
                }
            }
            if check.state == IntegratedCheckState::Due {
                check.state = IntegratedCheckState::Running { command: None };
            }
            check.started_at_ms = Some(now_ms());
            self.store_check(&check).await;
            check
        };
        if check.state.is_final() {
            check.ended_at_ms = Some(now_ms());
            self.store_check(&check).await;
            self.after_check(&check).await;
            return;
        }
        check.tasks = self.unchecked_tasks(id, &check).await;
        self.store_check(&check).await;
        let state = self.full_check_run(&mut check, cancel).await;
        // The session closed under it: nothing is told, nothing is kept as a verdict.
        if cancel.is_cancelled() || self.is_closing(id) {
            return;
        }
        check.state = state;
        check.ended_at_ms = Some(now_ms());
        self.store_check(&check).await;
        self.after_check(&check).await;
    }

    /// The full checks' commands in the session's check checkout, at `check.tip`; how they
    /// ended. Running, `check` names the command that runs.
    async fn full_check_run(
        &self,
        check: &mut IntegratedCheck,
        cancel: &CancellationToken,
    ) -> IntegratedCheckState {
        let id = check.conversation_id.clone();
        let interrupted = |reason: String| IntegratedCheckState::Interrupted { reason };
        let checkout = match self
            .check_checkout(&id, Path::new(&check.repo), &Oid(check.tip.clone()))
            .await
        {
            Ok(checkout) => checkout,
            Err(err) => return interrupted(format!("its checkout couldn't be made: {err}")),
        };
        let scratch = self.owned_dir("checks", &id.0).join("scratch");
        let access = self.full_check_access(&id, &checkout, &scratch);
        let caller = self
            .full_check_caller(
                &id,
                checkout.clone(),
                scratch.clone(),
                access,
                checks_owner(&id),
                cancel.clone(),
            )
            .await;
        // Dependencies installed again when what pins them changed.
        match self.refresh_installs(&caller, &checkout, &scratch).await {
            Ok(None) => {}
            Ok(Some((command, done))) => {
                if let Some(state) = ended(&command, &done) {
                    return state;
                }
            }
            Err(err) => {
                return interrupted(format!("its dependencies couldn't be installed: {err}"));
            }
        }
        let commands = match self.full_check_commands(&caller).await {
            Ok(commands) => commands,
            Err(err) => return interrupted(format!("its checks couldn't be listed: {err}")),
        };
        let mut passed = Vec::new();
        for (command, workdir) in commands {
            check.state = IntegratedCheckState::Running {
                command: Some(command.clone()),
            };
            self.store_check(check).await;
            let done = match self
                .run_full_check(
                    &caller,
                    &command,
                    &workdir,
                    FULL_CHECK_TIMEOUT,
                    check.rerun,
                    true,
                )
                .await
            {
                Ok(done) => done,
                Err(err) => return interrupted(format!("`{command}` couldn't run: {err}")),
            };
            if let Some(state) = ended(&command, &done) {
                return state;
            }
            passed.push(command);
        }
        IntegratedCheckState::Passed { commands: passed }
    }

    /// The session's check checkout, at `tip`: made warm the first time, moved after.
    async fn check_checkout(&self, id: &ConversationId, repo: &Path, tip: &Oid) -> Result<PathBuf> {
        let owner = checks_owner(id);
        let dir = self.owned_dir("checks", &id.0);
        let (tree, scratch) = (dir.join("tree"), dir.join("scratch"));
        self.prepare_owned_dir(&owner, &scratch).await?;
        if tree.exists() {
            let (git, at, tip) = (self.git.clone(), tree.clone(), tip.clone());
            let moved = blocking(move || {
                git.open_worktree(&at)
                    .and_then(|worktree| worktree.check_out_detached(&tip))
                    .map_err(git_error)
            })
            .await;
            match moved {
                Ok(()) => return Ok(tree),
                Err(err) => {
                    // Made again rather than trusted: its ignored folders go with it.
                    tracing::warn!(conversation = %id, error = %err, "the check checkout is made again");
                    let leftovers = self.runtime.ledger().dispose(&owner).await;
                    if !leftovers.is_clean() || tree.exists() {
                        return Err(Error::Invalid(format!(
                            "the old check checkout at {} couldn't be removed",
                            tree.display()
                        )));
                    }
                    self.prepare_owned_dir(&owner, &scratch).await?;
                }
            }
        }
        self.make_worktree(
            &owner,
            repo,
            &tree,
            WorktreeSpec::Detached { at: tip.clone() },
            true,
            &TaskId(format!("checks-{id}")),
        )
        .await?;
        Ok(tree)
    }

    /// The full checks' access: the session's permission level, as the thread's, writing only
    /// the check checkout and its scratch folder.
    fn full_check_access(&self, id: &ConversationId, checkout: &Path, scratch: &Path) -> Access {
        let level = super::workers::access_for(TaskKind::Implement, self.permission(id));
        if level.unsandboxed {
            return Access::Full;
        }
        let mut roots = vec![checkout.to_owned(), scratch.to_owned()];
        if let Ok(worktree) = self.git.open(checkout) {
            for root in super::workers::commit_roots(&worktree, None, true) {
                if !roots.contains(&root) {
                    roots.push(root);
                }
            }
        }
        self.sandboxed(true, roots, level.network)
    }

    /// Installs the checkout's Node dependencies again, with the package manager its lockfile
    /// names, when the files that pin them differ from the last install's. `None`: nothing to
    /// install.
    async fn refresh_installs(
        &self,
        caller: &super::checks::FullCheckCaller,
        checkout: &Path,
        scratch: &Path,
    ) -> Result<Option<(String, super::checks::CheckDone)>> {
        let (root, mark) = (checkout.to_owned(), scratch.join("installed"));
        let found = blocking(move || {
            let Some(command) = install_command(&root) else {
                return Ok(None);
            };
            let fingerprint = install_fingerprint(&root)?;
            let installed = std::fs::read_to_string(&mark).unwrap_or_default();
            Ok((installed != fingerprint).then_some((command, fingerprint, mark)))
        })
        .await?;
        let Some((command, fingerprint, mark)) = found else {
            return Ok(None);
        };
        let done = self
            .run_full_check(caller, command, "", INSTALL_TIMEOUT, true, false)
            .await?;
        if done.status == "exit 0" {
            blocking(move || {
                std::fs::write(&mark, fingerprint).map_err(|err| Error::Invalid(err.to_string()))
            })
            .await?;
        }
        Ok(Some((command.to_owned(), done)))
    }

    /// The landed tasks on `check`'s branch, up to its tip, that no passing check covers yet.
    async fn unchecked_tasks(&self, id: &ConversationId, check: &IntegratedCheck) -> Vec<TaskId> {
        let Ok(board) = self.core.board(id).await else {
            return Vec::new();
        };
        let landed: Vec<(TaskId, u32, String)> = board
            .tasks
            .values()
            .filter(|task| {
                task.workspace.as_ref().and_then(|w| w.target.as_deref())
                    == Some(check.branch.as_str())
            })
            .filter_map(|task| Some((task.id.clone(), task.number, task.landed.clone()?)))
            .collect();
        let passed: Vec<String> = board
            .integrated_checks
            .values()
            .filter(|other| {
                other.branch == check.branch
                    && matches!(other.state, IntegratedCheckState::Passed { .. })
            })
            .map(|other| other.tip.clone())
            .collect();
        let (git, repo, tip) = (
            self.git.clone(),
            PathBuf::from(&check.repo),
            Oid(check.tip.clone()),
        );
        let mut found = blocking(move || {
            let repo = git.open(&repo).map_err(git_error)?;
            let mut found = Vec::new();
            for (task, number, at) in landed {
                let at = Oid(at);
                if !repo.ancestor(&at, &tip).unwrap_or(false) {
                    continue;
                }
                let covered = passed
                    .iter()
                    .any(|passed| repo.ancestor(&at, &Oid(passed.clone())).unwrap_or(false));
                if !covered {
                    found.push((number, task));
                }
            }
            Ok(found)
        })
        .await
        .unwrap_or_default();
        found.sort();
        found.into_iter().map(|(_, task)| task).collect()
    }

    /// Whether a passing check covers `task`'s landing.
    pub(crate) async fn landing_checked(&self, task: &Task) -> bool {
        let (Some(at), Some(branch)) = (
            task.landed.clone(),
            task.workspace.as_ref().and_then(|w| w.target.clone()),
        ) else {
            return true;
        };
        let Ok(board) = self.core.board(&task.conversation_id).await else {
            return false;
        };
        let passed: Vec<(String, String)> = board
            .integrated_checks
            .values()
            .filter(|check| {
                check.branch == branch && matches!(check.state, IntegratedCheckState::Passed { .. })
            })
            .map(|check| (check.repo.clone(), check.tip.clone()))
            .collect();
        let git = self.git.clone();
        blocking(move || {
            Ok(passed.into_iter().any(|(repo, tip)| {
                git.open(Path::new(&repo))
                    .and_then(|repo| repo.ancestor(&Oid(at.clone()), &Oid(tip)))
                    .unwrap_or(false)
            }))
        })
        .await
        .unwrap_or(false)
    }

    /// What follows a check's end: the thread hears it, kept workers its pass covers are let
    /// go, a held merge looks again, and the requests it held settle.
    async fn after_check(&self, check: &IntegratedCheck) {
        let id = &check.conversation_id;
        if self.is_closing(id) {
            return;
        }
        if matches!(check.state, IntegratedCheckState::Passed { .. }) {
            self.release_checked_holds(id).await;
        }
        if let Some(text) = self.check_message(check).await {
            let task_id = check.tasks.last().cloned();
            let request = check.requests.last().cloned();
            let envelope = Envelope {
                kind: InjectionKind::Decision,
                label: format!("checks {}", check.id),
                task_id,
                text,
            };
            match request {
                Some(request) => self.deliver_for(id, envelope, Some(request)).await,
                None => self.deliver(id, envelope).await,
            }
        }
        self.check_held_merge(id).await;
        self.settle_requests(id).await;
    }

    /// What the thread hears of a check's end; none for a pass no answer waits for.
    async fn check_message(&self, check: &IntegratedCheck) -> Option<String> {
        let board = self.core.board(&check.conversation_id).await.ok()?;
        let tasks: Vec<&Task> = check
            .tasks
            .iter()
            .filter_map(|task| board.tasks.get(task))
            .collect();
        let at = short(&check.tip);
        match &check.state {
            IntegratedCheckState::Passed { commands } => {
                // Told when the thread left a request's answer to it ([quiet]).
                let deferred = match self.conv(&check.conversation_id) {
                    Ok(conv) => conv.request_activity().await.deferred,
                    Err(_) => Default::default(),
                };
                let waited = check.requests.iter().any(|request| {
                    deferred.contains(request)
                        && board.requests.get(request).is_some_and(|request| {
                            matches!(request.state, RequestState::Working | RequestState::Waiting)
                        })
                });
                let look = live_look_line(&tasks);
                if !waited && look.is_empty() {
                    return None;
                }
                let covered = if tasks.is_empty() {
                    String::new()
                } else {
                    format!(
                        " with {}",
                        tasks
                            .iter()
                            .map(|task| format!("task-{}", task.number))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                Some(format!(
                    "[checks passed at {at}] The session's full checks ({}) pass on `{}`{covered}.{look}",
                    commands
                        .iter()
                        .map(|command| format!("`{command}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    check.branch,
                ))
            }
            IntegratedCheckState::Failed {
                command,
                status,
                output,
            } => {
                let excerpt = match output {
                    Some(alias) => self.output_excerpt(&check.conversation_id, alias).await,
                    None => String::new(),
                };
                let landed = if tasks.is_empty() {
                    "Nothing a worker landed is new since they last passed: your own commits, or what was there already.".to_owned()
                } else {
                    format!(
                        "Landed since they last passed: {}.",
                        tasks
                            .iter()
                            .map(|task| landed_task_line(task))
                            .collect::<Vec<_>>()
                            .join("; ")
                    )
                };
                Some(format!(
                    "[checks failed at {at}] The session's full checks failed on `{branch}`: `{command}` ({status}).{excerpt}{whole}\n{landed}\nJudge which change broke it and send it back to that task's worker in one fix task (role fix, subject that task: it resumes the worker's own session), with the failing command and the lines that show why. Don't run the full checks yourself: once the fix lands they run again on their own. A failure that isn't the code's (a flaky test, the machine): rerun_checks.",
                    branch = check.branch,
                    whole = output
                        .as_ref()
                        .map(|alias| format!("\nThe whole output: read_artifact {alias}."))
                        .unwrap_or_default(),
                ))
            }
            IntegratedCheckState::Interrupted { reason } => Some(format!(
                "[checks couldn't run at {at}] The session's full checks on `{}` couldn't finish: {reason}. That tip is unchecked, so nothing merges until they pass. Fix what stops them if you can, then call rerun_checks.",
                check.branch
            )),
            IntegratedCheckState::Due | IntegratedCheckState::Running { .. } => None,
        }
    }

    /// The end of a stored output, for a failure's message.
    async fn output_excerpt(&self, id: &ConversationId, alias: &str) -> String {
        let Ok(board) = self.core.board(id).await else {
            return String::new();
        };
        let Some(output) = board.outputs.get(alias) else {
            return String::new();
        };
        let Ok(blob) = output.blob.parse() else {
            return String::new();
        };
        let Ok(Some(bytes)) = self.core.store().blobs().get(blob).await else {
            return String::new();
        };
        let text = String::from_utf8_lossy(&bytes);
        let mut start = text.len().saturating_sub(EXCERPT_MAX);
        while !text.is_char_boundary(start) {
            start += 1;
        }
        let tail = text[start..].trim();
        if tail.is_empty() {
            return String::new();
        }
        format!(
            "\nIts last lines{}:\n{tail}",
            if start > 0 { " (cut)" } else { "" }
        )
    }

    /// Lets go of the landed workers kept until a passing check covered their landing, once
    /// their review's part has settled too.
    async fn release_checked_holds(&self, id: &ConversationId) {
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        for task in super::review_hold::batch(&board, |task| task.review_settled) {
            if self.landing_checked(&task).await {
                self.release_hold(&task).await;
            }
        }
    }

    /// At startup: a check the restart cut off is due again, and a session with a due check
    /// runs it.
    pub(crate) async fn recover_integrated_checks(&self, id: &ConversationId) {
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        let mut due = false;
        for check in board.integrated_checks.values() {
            match check.state {
                IntegratedCheckState::Running { .. } => {
                    let mut again = check.clone();
                    again.state = IntegratedCheckState::Due;
                    again.started_at_ms = None;
                    self.store_check(&again).await;
                    due = true;
                }
                IntegratedCheckState::Due => due = true,
                _ => {}
            }
        }
        if due {
            self.run_due_checks(id).await;
        }
    }

    /// The session closes (archive, delete): its run ends, and nothing more is told.
    pub(crate) async fn end_integrated_checks(&self, id: &ConversationId) {
        if let Some(job) = self.integrated.jobs.lock().await.remove(id) {
            job.cancel.cancel();
        }
    }

    /// The session's work merged: its check checkout goes, unless a check is still due there.
    pub(crate) async fn drop_check_checkout(&self, id: &ConversationId) {
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        if board
            .integrated_checks
            .values()
            .any(|check| !check.state.is_final())
        {
            return;
        }
        let leftovers = self.runtime.ledger().dispose(&checks_owner(id)).await;
        if !leftovers.is_clean() {
            tracing::warn!(conversation = %id, ?leftovers, "some of the check checkout will be removed at the next launch");
        }
    }
}

/// A session's queue runner, as a future of a named type: its end looks at a held merge,
/// which can make another check due, so the queue is reached from itself.
fn queue(
    manager: Arc<SessionManager>,
    id: ConversationId,
    job: Arc<Job>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    Box::pin(async move { manager.checks_loop(&id, &job).await })
}

/// How a command of the full checks ended, when it ends the run: a failure (a non-zero exit,
/// a timeout, a crash) is the code's; a command stopped under it is no verdict.
fn ended(command: &str, done: &super::checks::CheckDone) -> Option<IntegratedCheckState> {
    if done.status == "exit 0" {
        return None;
    }
    if done.status.starts_with("stopped") {
        return Some(IntegratedCheckState::Interrupted {
            reason: format!("`{command}` was stopped: {}", done.status),
        });
    }
    Some(IntegratedCheckState::Failed {
        command: command.to_owned(),
        status: done.status.clone(),
        output: Some(done.alias.clone()),
    })
}

/// A landed task as a failure's message names it: its number, title and the files it changed.
fn landed_task_line(task: &Task) -> String {
    let changed: Vec<&str> = task
        .report
        .as_ref()
        .map(|report| report.changes.iter().map(String::as_str).take(12).collect())
        .unwrap_or_default();
    if changed.is_empty() {
        format!("task-{} \"{}\"", task.number, task.title)
    } else {
        format!(
            "task-{} \"{}\" (changed {})",
            task.number,
            task.title,
            changed.join(", ")
        )
    }
}

/// On this Mac, the visible changes a pass newly covers get one live look at the integrated
/// tree's dev build.
fn live_look_line(tasks: &[&Task]) -> String {
    if !cfg!(target_os = "macos") {
        return String::new();
    }
    let visible: Vec<String> = tasks
        .iter()
        .filter(|task| {
            task.kind == TaskKind::Implement
                && !task.live_look
                && task.areas.contains(&brigadier_router::Area::Frontend)
        })
        .map(|task| format!("task-{}", task.number))
        .collect();
    if visible.is_empty() {
        return String::new();
    }
    format!(
        " Visible changes landed ({}): look at them live once, now, in one delegate_task with live_look: true (kind implement, areas frontend), whose spec names each change and where to see it.",
        visible.join(", ")
    )
}

/// The command that installs a checkout's Node dependencies, by its lockfile; none without
/// one.
fn install_command(root: &Path) -> Option<&'static str> {
    [
        ("pnpm-lock.yaml", "pnpm install --frozen-lockfile"),
        ("yarn.lock", "yarn install --frozen-lockfile"),
        ("bun.lock", "bun install --frozen-lockfile"),
        ("bun.lockb", "bun install --frozen-lockfile"),
        ("package-lock.json", "npm ci"),
    ]
    .into_iter()
    .find(|(lockfile, _)| root.join(lockfile).is_file())
    .map(|(_, command)| command)
}

/// What pins a checkout's Node dependencies: its lockfiles, package manager configuration and
/// every `package.json` git tracks, hashed.
fn install_fingerprint(root: &Path) -> Result<String> {
    let mut files: Vec<String> = [
        "pnpm-lock.yaml",
        "pnpm-workspace.yaml",
        "yarn.lock",
        "bun.lock",
        "bun.lockb",
        "package-lock.json",
        ".npmrc",
        ".yarnrc.yml",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let listed = std::process::Command::new("git")
        .args(["ls-files", "-z", "--", "package.json", "*/package.json"])
        .current_dir(root)
        .output()
        .map_err(|err| Error::Invalid(format!("git could not list package.json files: {err}")))?;
    if !listed.status.success() {
        return Err(Error::Invalid(
            "git could not list package.json files".into(),
        ));
    }
    files.extend(
        listed
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| String::from_utf8_lossy(path).into_owned()),
    );
    files.sort();
    files.dedup();
    let mut hasher = blake3::Hasher::new();
    for file in files {
        if let Ok(bytes) = std::fs::read(root.join(&file)) {
            hasher.update(file.as_bytes());
            hasher.update(&[0]);
            hasher.update(&(bytes.len() as u64).to_le_bytes());
            hasher.update(&bytes);
        }
    }
    Ok(hasher.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(id: &str, tip: &str, state: IntegratedCheckState) -> IntegratedCheck {
        IntegratedCheck {
            id: id.into(),
            conversation_id: ConversationId("c".into()),
            repo: "/repo".into(),
            branch: "work".into(),
            tip: tip.into(),
            state,
            due_at_ms: 0,
            started_at_ms: None,
            ended_at_ms: None,
            requests: vec!["r1".into()],
            tasks: Vec::new(),
            rerun: false,
        }
    }

    #[test]
    fn the_newest_check_of_a_tip_decides_it() {
        let mut board = crate::board::Board::default();
        assert_eq!(tip_checked(&board, "work", "a1"), TipChecked::Never);
        for check in [
            check(
                "check-1",
                "a1",
                IntegratedCheckState::Interrupted {
                    reason: "a restart".into(),
                },
            ),
            check(
                "check-2",
                "a1",
                IntegratedCheckState::Passed {
                    commands: vec!["make check".into()],
                },
            ),
            check("check-10", "b2", IntegratedCheckState::Due),
        ] {
            board.integrated_checks.insert(check.id.clone(), check);
        }
        assert_eq!(tip_checked(&board, "work", "a1"), TipChecked::Passed);
        assert_eq!(tip_checked(&board, "work", "b2"), TipChecked::Pending);
        assert_eq!(tip_checked(&board, "other", "a1"), TipChecked::Never);
        assert!(checking_for(&board, "r1"));
        assert!(!checking_for(&board, "r2"));
        assert_eq!(
            checking_line(&board, "r1").as_deref(),
            Some(
                "[the session's full checks are due on `work` at b2, in the background: a failure comes as a message. Answer now, or reply [quiet] to answer with their result.]"
            )
        );
        assert_eq!(
            board
                .sorted_integrated_checks()
                .iter()
                .map(|check| check.id.as_str())
                .collect::<Vec<_>>(),
            ["check-1", "check-2", "check-10"]
        );
    }

    #[test]
    fn a_stopped_command_is_no_verdict_and_a_failing_one_is() {
        let done = |status: &str| super::super::checks::CheckDone {
            reply: String::new(),
            status: status.into(),
            alias: "out-1".into(),
        };
        assert_eq!(ended("make", &done("exit 0")), None);
        assert!(matches!(
            ended("make", &done("exit 2")),
            Some(IntegratedCheckState::Failed { .. })
        ));
        assert!(matches!(
            ended("make", &done("timed out after 3600 s")),
            Some(IntegratedCheckState::Failed { .. })
        ));
        assert!(matches!(
            ended("make", &done("stopped: the thread's session ended")),
            Some(IntegratedCheckState::Interrupted { .. })
        ));
    }

    #[test]
    fn installs_follow_the_lockfile_and_what_pins_it() {
        let dir = std::env::temp_dir().join(format!(
            "brigadier-installs-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(dir.join("apps/web")).unwrap();
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
                .args(args)
                .current_dir(&dir)
                .status()
                .unwrap();
            assert!(status.success());
        };
        git(&["init", "-q"]);
        assert_eq!(install_command(&dir), None);
        std::fs::write(dir.join("pnpm-lock.yaml"), "lockfileVersion: 9\n").unwrap();
        std::fs::write(dir.join("package.json"), "{}\n").unwrap();
        std::fs::write(dir.join("apps/web/package.json"), "{\"name\": \"web\"}\n").unwrap();
        git(&["add", "-A"]);
        assert_eq!(
            install_command(&dir),
            Some("pnpm install --frozen-lockfile")
        );
        let first = install_fingerprint(&dir).unwrap();
        assert_eq!(install_fingerprint(&dir).unwrap(), first);
        std::fs::write(dir.join("README.md"), "unrelated\n").unwrap();
        assert_eq!(install_fingerprint(&dir).unwrap(), first);
        std::fs::write(
            dir.join("apps/web/package.json"),
            "{\"name\": \"web\", \"dependencies\": {\"left-pad\": \"1\"}}\n",
        )
        .unwrap();
        assert_ne!(install_fingerprint(&dir).unwrap(), first);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
