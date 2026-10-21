//! The session's full checks, run on the integrated tree when an agent asks (`run_checks`,
//! SESSION-FIXES-PLAN.md §11). Brigadier never starts them by itself: not after a landing, a
//! commit or a restart.
//!
//! Workers run only the checks of their own change and hand back; the thread lands their work
//! at once. What the work reaches beyond each change (the packages that depend on it, the
//! workspace's own files, a build of the app) is checked here, on the branch the work landed
//! on, instead of once per worker in every worktree.
//!
//! - **One run at a time per session**, in a checkout of its own (`checks/<id>/tree`, owned by
//!   [`checks_owner`]): made warm once, then moved to each tip, so its build output stays and
//!   no landing changes files under a running build. Its dependency installs are made again
//!   when the files that pin them change. The commands are the project's own full checks
//!   (`checks::full_checks`), each through `run_check`'s keyed cache, so the same files under
//!   the same inputs never run again unless the thread asks (`run_checks` with `rerun`), and always with
//!   the build lease.
//! - **The result** reaches the thread: a failure with its command, output and the tasks that
//!   landed since the checks last passed, for the thread to fix or send back to the worker that
//!   broke it (a fix task resumes that worker's session, kept until a pass covers its landing);
//!   a run that couldn't finish as such, never as a verdict. A merge into main needs a pass of
//!   the thread's own `run_checks` at the exact commit it merges.
//! - **Recorded** as [`IntegratedCheck`]s from the moment they are asked for. A restart or an
//!   unarchive starts nothing: a run it cut off, or one still waiting to start, is recorded
//!   interrupted, for the agent to ask again; a session archived or deleted ends its run and
//!   removes its checkout, and hears nothing after.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use brigadier_git::{Oid, WorktreeSpec};
use brigadier_providers::Access;
use tokio_util::sync::CancellationToken;

use super::conversation::Envelope;
use super::workers::{SessionTemp, session_temp_dir};
use super::{SessionManager, blocking, git_error};
use crate::model::{ConversationId, DomainEvent};
use crate::work::{
    InjectionKind, IntegratedCheck, IntegratedCheckState, RequestState, Task, TaskId, TaskKind,
};
use crate::{Error, Result, now_ms};

/// How long `run_checks` holds the thread's call for the verdict before it goes on in the
/// background, its result then coming as a message.
const CALL_WAIT: Duration = Duration::from_secs(if cfg!(test) { 5 } else { 60 });
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

/// The label of the message that tells check `id`'s end: `checks passed check-7` for a pass
/// only a deferred answer waits for, `run_checks passed check-7` for one owed to the thread
/// (#513), `checks check-7` for the rest.
fn check_label(id: &str, pass: bool, owed: bool) -> String {
    match (pass, owed) {
        (true, true) => format!("{}{id}", super::conversation::OWED_PASS),
        (true, false) => format!("checks passed {id}"),
        (false, _) => format!("checks {id}"),
    }
}

/// What the sessions' full checks need while the daemon runs.
#[derive(Default)]
pub(crate) struct IntegratedChecks {
    /// Each session's queue runner, while it has a due check.
    jobs: tokio::sync::Mutex<HashMap<ConversationId, Arc<Job>>>,
    /// Held while a session's checks are read, changed and recorded.
    pub(super) records: tokio::sync::Mutex<()>,
    /// Woken each time a check ends.
    ended: tokio::sync::Notify,
    /// The `run_checks` runs whose verdict the thread's call still waits for, by session and
    /// check (a check's id is its session's own): their end is told in the call, not as a
    /// message. Held while a check's end is told.
    in_call: tokio::sync::Mutex<std::collections::HashSet<(ConversationId, String)>>,
}

/// A session's queue runner.
struct Job {
    wake: tokio::sync::Notify,
    /// Ends it, and the command it runs: the session closed.
    cancel: CancellationToken,
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
    newest_at(board, branch, tip, |_| true)
}

/// Where the thread's own run_checks of `branch` stand at `tip`, the only runs the merge rule
/// counts ([`full_by_thread`]): runs Brigadier started by itself before never do.
pub(crate) fn tip_checked_by_thread(
    board: &crate::board::Board,
    branch: &str,
    tip: &str,
) -> TipChecked {
    newest_at(board, branch, tip, full_by_thread)
}

fn newest_at(
    board: &crate::board::Board,
    branch: &str,
    tip: &str,
    counts: fn(&IntegratedCheck) -> bool,
) -> TipChecked {
    let newest = board
        .integrated_checks
        .values()
        .filter(|check| check.branch == branch && check.tip == tip && counts(check))
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

/// The failing tests a full-checks output names: a `path::to::test` on a line that says it
/// failed (cargo, nextest, pytest), cargo's `test name ... FAILED`, nextest's
/// `FAIL [time] crate name`, or Go's `--- FAIL: Name`.
pub(crate) fn failing_tests(output: &str) -> Vec<String> {
    let mut tests: Vec<String> = Vec::new();
    for line in output.lines().filter(|line| line.contains("FAIL")) {
        let trimmed = line.trim_start();
        let named = line
            .split_once("--- FAIL: ")
            .map(|(_, rest)| rest)
            .or_else(|| {
                trimmed
                    .strip_prefix("test ")
                    .filter(|_| trimmed.ends_with("... FAILED"))
            })
            .or_else(|| {
                trimmed
                    .strip_prefix("FAIL [")
                    .and_then(|rest| rest.split_whitespace().last())
            });
        let words = named
            .into_iter()
            .chain(line.split_whitespace().filter(|word| word.contains("::")));
        for word in words {
            let name = word
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .trim_matches(|c: char| !(c.is_alphanumeric() || c == '_'));
            if !name.is_empty() && !tests.iter().any(|known| known == name) {
                tests.push(name.to_owned());
            }
        }
    }
    tests
}

/// The failing `tests` an open issue names (`issues`: number and text), as "`test` (#230)".
pub(crate) fn named_in_issues(tests: &[String], issues: &[(u64, String)]) -> Vec<String> {
    tests
        .iter()
        .filter_map(|test| {
            let last = test.rsplit("::").next().unwrap_or(test);
            let key = if last.len() >= 8 { last } else { test.as_str() };
            let numbers: Vec<String> = issues
                .iter()
                .filter(|(_, text)| text.contains(key))
                .map(|(number, _)| format!("#{number}"))
                .collect();
            (!numbers.is_empty()).then(|| format!("`{test}` ({})", numbers.join(", ")))
        })
        .collect()
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
        "[the session's full checks {} on `{}` at {}, in the background: {} comes as a message. Answer now, or reply {} to answer with their result.]",
        match check.state {
            IntegratedCheckState::Due => "are due",
            _ => "run",
        },
        check.branch,
        short(&check.tip),
        // A run_checks call that went on is owed the verdict, a pass too (#513).
        if check.owed {
            "their result"
        } else {
            "a failure"
        },
        super::prompts::QUIET
    ))
}

/// Where the session's newest full checks and the thread's background runs stand, in plain
/// lines for the thread at `now_ms` (`list_tasks`): what it needs to say whether the work is
/// ready, and how long what runs has run.
pub(crate) fn standing_lines(board: &crate::board::Board, now_ms: i64) -> Vec<String> {
    let minutes = |from: Option<i64>| {
        let mins = (now_ms - from.unwrap_or(now_ms)).max(0) / 60_000;
        if mins == 0 {
            "under a minute".to_owned()
        } else {
            format!("{mins} min")
        }
    };
    let mut lines = Vec::new();
    let newest = board
        .integrated_checks
        .values()
        .max_by_key(|check| integrated_check_number(&check.id));
    lines.push(match newest {
        None => "Full checks: none ran in this session yet.".to_owned(),
        Some(check) => {
            let at = format!("`{}` at {}", check.branch, short(&check.tip));
            match &check.state {
                IntegratedCheckState::Due => {
                    format!("Full checks: about to start on {at}, as run_checks asked.")
                }
                IntegratedCheckState::Running { .. } => format!(
                    "Full checks: running on {at} for {}, in the background; their result comes as a message.",
                    minutes(check.started_at_ms)
                ),
                IntegratedCheckState::Passed { .. } => format!(
                    "Full checks: passed on {at}, {} ago. A newer commit needs run_checks again.",
                    minutes(check.ended_at_ms)
                ),
                IntegratedCheckState::Failed {
                    command, status, ..
                } => format!("Full checks: failed on {at}: `{command}` ({status})."),
                IntegratedCheckState::Interrupted { reason } => {
                    format!("Full checks: couldn't finish on {at} ({reason}); run_checks with rerun true runs them again.")
                }
            }
        }
    });
    let mut runs: Vec<&crate::work::BackgroundRun> = board
        .background_runs
        .values()
        .filter(|run| run.state == crate::work::BackgroundRunState::Running)
        .collect();
    runs.sort_by_key(|run| super::background_runs::background_run_number(&run.id));
    for run in runs {
        lines.push(format!(
            "{} running in the background for {}: {} `{}`.",
            run.id,
            minutes(Some(run.started_at_ms)),
            run.tool,
            run.command
        ));
    }
    lines
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(10)]
}

impl SessionManager {
    /// The thread's `run_checks`: the project's full checks on the session branch's commit
    /// now, started at once. A pass of an earlier call at that commit answers it (unless
    /// `rerun`), one running there is waited for; the call holds up to [`CALL_WAIT`] for the
    /// verdict, then it goes on in the background and its result comes as a message.
    pub(crate) async fn run_checks_tool(&self, id: &ConversationId, rerun: bool) -> Result<String> {
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
        let check = self
            .ask_full_checks(id, &workspace.repo, &workspace.branch, &tip, request, rerun)
            .await?;
        let claim = (id.clone(), check.clone());
        self.integrated.in_call.lock().await.insert(claim.clone());
        self.run_due_checks(id).await;
        let deadline = tokio::time::Instant::now() + CALL_WAIT;
        loop {
            let ended = self.integrated.ended.notified();
            let board = self.core.board(id).await?;
            if let Some(done) = board
                .integrated_checks
                .get(&check)
                .filter(|done| done.state.is_final())
            {
                // Its end is told here: after_check leaves it out of the messages.
                return Ok(self.checks_verdict(done).await);
            }
            if tokio::time::timeout_at(deadline, ended).await.is_err() {
                let mut waiting = self.integrated.in_call.lock().await;
                let board = self.core.board(id).await?;
                let Some(now) = board.integrated_checks.get(&check) else {
                    waiting.remove(&claim);
                    return Err(Error::Invalid("the checks' record is gone".into()));
                };
                if now.state.is_final() {
                    drop(waiting);
                    return Ok(self.checks_verdict(now).await);
                }
                waiting.remove(&claim);
                // Under the claims' lock, so its end, told as a message, sees it owed.
                self.set_owed(id, &check, true).await;
                drop(waiting);
                return Ok(format!(
                    "The full checks still run on `{}` at {} ({check}), in the background: their result comes as a message [checks … at {}]. Carry on with what doesn't depend on them, or reply {}; don't call run_checks again for it.",
                    now.branch,
                    short(&now.tip),
                    short(&now.tip),
                    super::prompts::QUIET,
                ));
            }
        }
    }

    /// A `run_checks` run of the full checks at `tip` of `branch`, by its id: an earlier
    /// call's pass there (unless `rerun`), one due or running there, or a new one, due at once.
    async fn ask_full_checks(
        &self,
        id: &ConversationId,
        repo: &Path,
        branch: &str,
        tip: &Oid,
        request: Option<String>,
        rerun: bool,
    ) -> Result<String> {
        let _records = self.integrated.records.lock().await;
        let board = self.core.board(id).await?;
        let asked_here = board
            .integrated_checks
            .values()
            .filter(|check| check.branch == branch && check.tip == tip.0 && full_by_thread(check))
            .max_by_key(|check| integrated_check_number(&check.id));
        match asked_here.map(|check| (&check.state, check)) {
            Some((IntegratedCheckState::Passed { .. }, check)) if !rerun => {
                return Ok(check.id.clone());
            }
            Some((IntegratedCheckState::Due | IntegratedCheckState::Running { .. }, check)) => {
                // This request waits for it too: its verdict comes to it if the call doesn't
                // tell it (the runner's next record keeps the request, see `store_run`).
                if let Some(request) = request
                    && !check.requests.contains(&request)
                {
                    let mut joined = check.clone();
                    joined.requests.push(request);
                    self.store_check(&joined).await;
                }
                return Ok(check.id.clone());
            }
            _ => {}
        }
        let check = IntegratedCheck {
            id: next_check_id(&board),
            conversation_id: id.clone(),
            repo: repo.to_string_lossy().into_owned(),
            branch: branch.to_owned(),
            tip: tip.0.clone(),
            state: IntegratedCheckState::Due,
            due_at_ms: now_ms(),
            started_at_ms: None,
            ended_at_ms: None,
            requests: request.into_iter().collect(),
            tasks: Vec::new(),
            rerun,
            asked: Some(crate::work::ChecksAsked {
                task_id: None,
                scope: crate::work::ChecksScope::Full,
            }),
            owed: false,
        };
        self.store_check(&check).await;
        Ok(check.id)
    }

    /// What the thread is told of a `run_checks` run's end, in its call.
    async fn checks_verdict(&self, check: &IntegratedCheck) -> String {
        match self.check_message(check, true, true).await {
            Some((text, _)) => text,
            None => format!("[checks at {}] They ended.", short(&check.tip)),
        }
    }

    /// Records the runner's copy of `check`, with the requests that joined it meanwhile and
    /// whether its verdict is owed: the record holds them, the runner's copy may not.
    async fn store_run(&self, check: &mut IntegratedCheck) {
        let _records = self.integrated.records.lock().await;
        if let Ok(board) = self.core.board(&check.conversation_id).await
            && let Some(stored) = board.integrated_checks.get(&check.id)
        {
            for request in &stored.requests {
                if !check.requests.contains(request) {
                    check.requests.push(request.clone());
                }
            }
            check.owed = stored.owed;
        }
        self.store_check(check).await;
    }

    /// Records whether `check`'s verdict is owed to the thread as a message (#513).
    async fn set_owed(&self, id: &ConversationId, check: &str, owed: bool) {
        let _records = self.integrated.records.lock().await;
        let Some(mut stored) = self
            .core
            .board(id)
            .await
            .ok()
            .and_then(|board| board.integrated_checks.get(check).cloned())
            .filter(|stored| stored.owed != owed)
        else {
            return;
        };
        stored.owed = owed;
        self.store_check(&stored).await;
    }

    pub(super) async fn store_check(&self, check: &IntegratedCheck) {
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

    /// The session's queue: each due check, oldest first; ends when none is due.
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
                if next.is_none() && jobs.get(id).is_some_and(|now| Arc::ptr_eq(now, job)) {
                    // Nothing more is due: a merge made while they ran takes their checkout
                    // now, before another runner can start and use it.
                    if self.merged_since_checks(id).await {
                        self.drop_check_checkout(id).await;
                    }
                    jobs.remove(id);
                }
                next
            };
            let Some(due) = next else {
                return;
            };
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
        // A local checkout's changes stay uncommitted: its checks run on a snapshot of its
        // files as they stand now, never in the user's checkout (BUILD-PLAN B2).
        let local = self
            .local_checkout(id)
            .is_some_and(|(_, branch)| branch == due.branch);
        let tip = if local {
            crate::heap(|| self.snapshot_local(&repo)).await.map(Some)
        } else {
            let (git, at, branch) = (self.git.clone(), repo.clone(), due.branch.clone());
            blocking(move || {
                git.open(&at)
                    .map_err(git_error)?
                    .branch_tip(&branch)
                    .map_err(git_error)
            })
            .await
        };
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
                // `run_checks` checks the commit it was asked at, or a local checkout's files as
                // they stand now.
                Ok(Some(tip)) if local => check.tip = tip.0.clone(),
                Ok(Some(_)) => {}
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
                check.state = IntegratedCheckState::Running {
                    command: None,
                    done: 0,
                    total: 0,
                };
            }
            check.started_at_ms = Some(now_ms());
            self.store_check(&check).await;
            check
        };
        if check.state.is_final() {
            check.ended_at_ms = Some(now_ms());
            self.store_run(&mut check).await;
            self.after_check(&check).await;
            return;
        }
        check.tasks = self.unchecked_tasks(id, &check).await;
        self.store_run(&mut check).await;
        let state = self.full_check_run(&mut check, cancel).await;
        // The session closed under it: nothing is told, nothing is kept as a verdict.
        if cancel.is_cancelled() || self.is_closing(id) {
            return;
        }
        check.state = state;
        check.ended_at_ms = Some(now_ms());
        self.store_run(&mut check).await;
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
        let tmp = session_temp_dir(&id, SessionTemp::Checks);
        if let Err(err) = self.prepare_owned_dir(&checks_owner(&id), &tmp).await {
            return interrupted(format!("its temporary folder couldn't be made: {err}"));
        }
        let access = self.full_check_access(&id, &checkout, &[scratch.clone(), tmp.clone()]);
        let caller = self
            .full_check_caller(
                &id,
                checkout.clone(),
                scratch.clone(),
                tmp,
                access,
                checks_owner(&id),
                cancel.clone(),
            )
            .await;
        // Dependencies installed again when what pins them changed.
        match self.refresh_installs(&caller, &checkout, &scratch).await {
            Ok(None) => {}
            // An install that fails says nothing of the code: the tip stays unchecked.
            Ok(Some((command, done))) => {
                if done.status != "exit 0" {
                    return interrupted(format!(
                        "its dependencies couldn't be installed: `{command}` ended {} (its output: read_artifact {})",
                        done.status, done.alias
                    ));
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
        let total = u32::try_from(commands.len()).unwrap_or(u32::MAX);
        for (command, workdir) in commands {
            check.state = IntegratedCheckState::Running {
                command: Some(command.clone()),
                done: u32::try_from(passed.len()).unwrap_or(u32::MAX),
                total,
            };
            self.store_run(check).await;
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
        // The app runs here as in the session's worktree: the checkout's gitignored env files
        // and what `.worktreeinclude` names are copied in, and stay as the tree moves.
        let secret_files = self
            .core
            .conversation(id)
            .ok()
            .and_then(|conversation| conversation.project_id)
            .and_then(|project| self.core.project(&project).ok())
            .map(|project| project.prefs.secret_files)
            .unwrap_or_default();
        if let Err(err) =
            super::secrets::copy_secrets(self, &owner, repo, &tree, &secret_files).await
        {
            tracing::warn!(conversation = %id, %err, "couldn't copy the checkout's env files into the check checkout");
        }
        Ok(tree)
    }

    /// The full checks' access: the session's permission level, as the thread's, writing only
    /// the check checkout and its `own` folders (its scratch and temporary folders).
    fn full_check_access(&self, id: &ConversationId, checkout: &Path, own: &[PathBuf]) -> Access {
        let level = super::workers::access_for(TaskKind::Implement, self.permission(id));
        if level.unsandboxed {
            return Access::Full;
        }
        let mut roots = vec![checkout.to_owned()];
        roots.extend(own.iter().cloned());
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
        // A `run_checks` call still waiting tells its verdict itself, owed to an earlier call
        // that went on or not; else whether one went on is read under the claims' lock, which
        // that call recorded it under.
        let claim = (id.clone(), check.id.clone());
        let owed = {
            let mut waiting = self.integrated.in_call.lock().await;
            if waiting.remove(&claim) {
                drop(waiting);
                self.set_owed(id, &check.id, false).await;
                self.check_held_merge(id).await;
                self.settle_requests(id).await;
                return;
            }
            self.core.board(id).await.ok().is_some_and(|board| {
                board
                    .integrated_checks
                    .get(&check.id)
                    .is_some_and(|stored| stored.owed)
            })
        };
        // Every independent request that deferred to this run gets its own outcome.
        let mut requests: Vec<_> = self
            .waiting_requests(check)
            .await
            .into_iter()
            .map(Some)
            .collect();
        let waited = !requests.is_empty();
        if requests.is_empty() {
            requests.push(check.requests.last().cloned());
        }
        // Queue the whole batch before settling or starting a turn, so delivering the
        // first result cannot settle the other requests before their messages are queued.
        let mut queued = None;
        let mut wait = false;
        let count = requests.len();
        for (index, request) in requests.into_iter().enumerate() {
            // A live look is work for the thread once, on the newest waiting request.
            let Some((text, optional_pass)) =
                self.check_message(check, waited, index + 1 == count).await
            else {
                continue;
            };
            let envelope = Envelope {
                kind: InjectionKind::Decision,
                label: check_label(&check.id, optional_pass, owed),
                task_id: check.tasks.last().cloned(),
                text,
            };
            let request = match request {
                Some(request) => Some(request),
                None => self.request_for(id, envelope.task_id.as_ref()).await,
            };
            // A pass waits a little while more of its request runs (BUILD-PLAN C3).
            if let Some(request) = &request
                && envelope.quiet()
                && crate::heap(|| self.more_coming(id, request, envelope.task_id.as_ref())).await
            {
                wait = true;
            }
            if let Some(conv) = self.queue_envelope(id, envelope, request).await {
                queued = Some(conv);
            }
        }
        if let Some(conv) = queued {
            if wait {
                self.wait_quietly(&conv).await;
            }
            self.settle_requests(id).await;
            self.kick(&conv);
        }
        self.check_held_merge(id).await;
        self.settle_requests(id).await;
    }

    /// All of `check`'s requests whose answers the thread left to it ([quiet]) and
    /// that still work or wait.
    async fn waiting_requests(&self, check: &IntegratedCheck) -> Vec<String> {
        let Ok(board) = self.core.board(&check.conversation_id).await else {
            return Vec::new();
        };
        let Ok(conv) = self.conv(&check.conversation_id) else {
            return Vec::new();
        };
        let deferred = conv.request_activity().await.deferred;
        check
            .requests
            .iter()
            .filter(|request| {
                deferred.contains(*request)
                    && board.requests.get(*request).is_some_and(|request| {
                        matches!(request.state, RequestState::Working | RequestState::Waiting)
                    })
            })
            .cloned()
            .collect()
    }

    /// What the thread hears of a check's end; none for a pass no answer waits for.
    async fn check_message(
        &self,
        check: &IntegratedCheck,
        waited: bool,
        include_live_look: bool,
    ) -> Option<(String, bool)> {
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
                // Only computer use's tools can look at it live.
                let look = if include_live_look && self.core.settings().computer_use {
                    live_look_line(&tasks)
                } else {
                    String::new()
                };
                // A run_checks run's pass is told even when no answer waits for it.
                if !waited && look.is_empty() && check.asked.is_none() {
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
                Some((
                    format!(
                        "[checks passed at {at}] The session's full checks ({}) pass on `{}`{covered}.{look}",
                        commands
                            .iter()
                            .map(|command| format!("`{command}`"))
                            .collect::<Vec<_>>()
                            .join(", "),
                        check.branch,
                    ),
                    look.is_empty(),
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
                let known = self
                    .known_failures(&check.conversation_id, output.as_deref())
                    .await;
                let known = if known.is_empty() {
                    String::new()
                } else {
                    format!("\nOpen issues name failing tests: {}.", known.join(", "))
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
                Some((
                    format!(
                        "[checks failed at {at}] The session's full checks failed on `{branch}`: `{command}` ({status}).{excerpt}{whole}\n{landed}{known}\nA merge into main needs them to pass on its exact commit. A break in the code is fixed by you, or goes back to the task that broke it in one fix task (role fix, subject that task: it resumes the worker's own session) with the failing command and the lines that show why; then call run_checks again. A failure that isn't the code's (a flaky test, the machine): run_checks with rerun true.",
                        branch = check.branch,
                        whole = output
                            .as_ref()
                            .map(|alias| format!("\nThe whole output: read_artifact {alias}."))
                            .unwrap_or_default(),
                    ),
                    false,
                ))
            }
            IntegratedCheckState::Interrupted { reason } => Some((
                format!(
                    "[checks couldn't run at {at}] The session's full checks on `{}` couldn't finish: {reason}. That tip is unchecked. Fix what stops them if you can, then call run_checks with rerun true.",
                    check.branch
                ),
                false,
            )),
            IntegratedCheckState::Due | IntegratedCheckState::Running { .. } => None,
        }
    }

    /// A stored output's whole text.
    async fn output_text(&self, id: &ConversationId, alias: &str) -> Option<String> {
        let board = self.core.board(id).await.ok()?;
        let blob = board.outputs.get(alias)?.blob.parse().ok()?;
        let bytes = self.core.store().blobs().get(blob).await.ok()??;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// The failing tests in a check's output (`alias`) that open issues of the session's
    /// repository name, as "`test` (#230)"; none when there are none or `gh` can't say.
    async fn known_failures(&self, id: &ConversationId, alias: Option<&str>) -> Vec<String> {
        let Some(alias) = alias else {
            return Vec::new();
        };
        let Some(text) = self.output_text(id, alias).await else {
            return Vec::new();
        };
        let tests = failing_tests(&text);
        if tests.is_empty() {
            return Vec::new();
        }
        let Some(workspace) = self.recorded_workspace(id) else {
            return Vec::new();
        };
        match self.open_issues(&workspace.repo).await {
            Some(issues) => named_in_issues(&tests, &issues),
            None => Vec::new(),
        }
    }

    /// Where the full checks of `branch` stand at `tip`, for a merge's result: they inform the
    /// merge, they don't hold it (#241).
    pub(crate) async fn checks_line(&self, id: &ConversationId, branch: &str, tip: &Oid) -> String {
        let Ok(board) = self.core.board(id).await else {
            return String::new();
        };
        let at = short(&tip.0);
        let state = match tip_checked(&board, branch, &tip.0) {
            TipChecked::Passed => format!("passed at {at}"),
            TipChecked::Never => format!("not run at {at}"),
            TipChecked::Pending => format!("still running at {at}"),
            TipChecked::Couldnt { reason } => format!("couldn't run at {at}: {reason}"),
            TipChecked::Failed { command, status } => {
                let output = board
                    .integrated_checks
                    .values()
                    .filter(|check| check.branch == branch && check.tip == tip.0)
                    .max_by_key(|check| integrated_check_number(&check.id))
                    .and_then(|check| match &check.state {
                        IntegratedCheckState::Failed { output, .. } => output.clone(),
                        _ => None,
                    });
                let known = self.known_failures(id, output.as_deref()).await;
                let known = if known.is_empty() {
                    String::new()
                } else {
                    format!("; open issues name {}", known.join(", "))
                };
                format!("failed at {at}: `{command}` ({status}){known}")
            }
        };
        format!(" The session's full checks: {state}.")
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

    /// At startup (`cause`: "Brigadier quit") or an unarchive ("The session was archived"):
    /// nothing runs again by itself. A check cut off while it ran, or one still waiting to
    /// start, is recorded interrupted, for the agent to call run_checks again. At startup
    /// (`tell`), each verdict owed to the thread that no turn took goes to it now, once: one
    /// the stop cut off says so (#513). After an unarchive none is owed any more: the archive
    /// ended the session's work.
    pub(crate) async fn recover_integrated_checks(
        &self,
        id: &ConversationId,
        cause: &str,
        tell: bool,
    ) {
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        for check in board.integrated_checks.values() {
            let when = match check.state {
                IntegratedCheckState::Running { .. } => "while they ran",
                IntegratedCheckState::Due => "before they started",
                _ => continue,
            };
            let mut cut = check.clone();
            cut.state = IntegratedCheckState::Interrupted {
                reason: format!("{cause} {when}: call run_checks again"),
            };
            cut.ended_at_ms = Some(now_ms());
            self.store_check(&cut).await;
        }
        self.tell_owed_checks(id, tell).await;
    }

    /// See [`Self::recover_integrated_checks`]. One a turn took since it ended (its message is
    /// logged in the thread's stream) is owed no more; one in the inbox again already (the
    /// inbox's own recovery) isn't told twice.
    async fn tell_owed_checks(&self, id: &ConversationId, tell: bool) {
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        let mut owed: Vec<IntegratedCheck> = board
            .integrated_checks
            .values()
            .filter(|check| check.owed && check.state.is_final())
            .cloned()
            .collect();
        if owed.is_empty() {
            return;
        }
        if !tell {
            for check in &owed {
                self.set_owed(id, &check.id, false).await;
            }
            return;
        }
        owed.sort_by_key(|check| integrated_check_number(&check.id));
        let since = owed
            .iter()
            .map(|check| check.ended_at_ms.unwrap_or(0))
            .min()
            .unwrap_or(0);
        let taken = self
            .injections_taken_since(id, since, InjectionKind::Decision)
            .await;
        for check in owed {
            let labels = [
                check_label(&check.id, true, true),
                check_label(&check.id, true, false),
                check_label(&check.id, false, false),
            ];
            let ended = check.ended_at_ms.unwrap_or(0);
            if taken
                .iter()
                .any(|(at_ms, label, _)| *at_ms >= ended && labels.contains(label))
            {
                self.set_owed(id, &check.id, false).await;
                continue;
            }
            let request = check.requests.last().cloned();
            let task = check.tasks.last().cloned();
            let mut queued = false;
            for label in &labels {
                queued |= self
                    .inbox_holds(
                        id,
                        InjectionKind::Decision,
                        label,
                        task.as_ref(),
                        request.as_deref(),
                    )
                    .await;
            }
            if queued {
                continue;
            }
            let Some((text, pass)) = self.check_message(&check, false, true).await else {
                continue;
            };
            tracing::info!(conversation = %id, check = %check.id, "a check's verdict the thread was owed goes to it after the restart");
            self.deliver_for(
                id,
                Envelope {
                    kind: InjectionKind::Decision,
                    label: check_label(&check.id, pass, true),
                    task_id: task,
                    text,
                },
                request,
            )
            .await;
        }
    }

    /// The session closes (archive, delete): its run ends, and nothing more is told.
    pub(crate) async fn end_integrated_checks(&self, id: &ConversationId) {
        if let Some(job) = self.integrated.jobs.lock().await.remove(id) {
            job.cancel.cancel();
        }
    }

    /// Whether the session merged after its newest check became due: its checkout then has
    /// nothing left to check.
    async fn merged_since_checks(&self, id: &ConversationId) -> bool {
        let Ok(board) = self.core.board(id).await else {
            return false;
        };
        let merged = board
            .orchestrator_steps
            .iter()
            .filter(|step| matches!(step.kind, crate::work::OrchestratorStepKind::Merged { .. }))
            .map(|step| step.at_ms)
            .max();
        let due = board
            .integrated_checks
            .values()
            .map(|check| check.due_at_ms)
            .max();
        merged.is_some_and(|merged| due.is_none_or(|due| merged > due))
    }

    /// The session's work merged: its check checkout goes, or once the checks due or running
    /// there end.
    pub(crate) async fn drop_check_checkout(&self, id: &ConversationId) {
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        if board
            .integrated_checks
            .values()
            .any(|check| !check.state.is_final())
        {
            // It goes once they end: their queue runner sees the merge.
            return;
        }
        let leftovers = self.runtime.ledger().dispose(&checks_owner(id)).await;
        if !leftovers.is_clean() {
            tracing::warn!(conversation = %id, ?leftovers, "some of the check checkout will be removed at the next launch");
        }
    }
}

/// The next free check id of a conversation's board (`check-<n>`).
pub(super) fn next_check_id(board: &crate::board::Board) -> String {
    let number = board
        .integrated_checks
        .keys()
        .map(|id| integrated_check_number(id))
        .max()
        .unwrap_or(0)
        + 1;
    format!("check-{number}")
}

/// Whether `check` is a run of the full checks the thread asked for with `run_checks`: the
/// only kind a merge into main counts.
pub(crate) fn full_by_thread(check: &IntegratedCheck) -> bool {
    check.asked.as_ref().is_some_and(|asked| {
        asked.task_id.is_none() && asked.scope == crate::work::ChecksScope::Full
    })
}

/// A session's queue runner, as a future of a named type: its end looks at a held merge, so
/// the queue is reached from itself.
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

    #[test]
    fn failing_tests_that_open_issues_name_are_listed_with_their_numbers() {
        let output = "\
    PASS [   0.4s] brigadier-core manager::flow::tests::fine
    FAIL [   0.0s] brigadier-providers claude::tests::a_denied_session_refuses_every_tool
test manager::flow::peer_chain_tests::the_ninth_message_waits ... FAILED
test root_level_test ... FAILED
    FAIL [   0.1s] brigadier-cli another_root_test
--- FAIL: TestUpload (0.01s)
FAILED tests/test_api.py::test_login - assert 1 == 2
test result: FAILED. 1 passed; 3 failed";
        let tests = failing_tests(output);
        assert_eq!(
            tests,
            [
                "claude::tests::a_denied_session_refuses_every_tool",
                "manager::flow::peer_chain_tests::the_ninth_message_waits",
                "root_level_test",
                "another_root_test",
                "TestUpload",
                "tests/test_api.py::test_login",
            ]
        );
        let issues = [
            (
                230,
                "Flaky: the_ninth_message_waits times out on CI\n".to_owned(),
            ),
            (12, "Something else\ntest_login".to_owned()),
        ];
        assert_eq!(
            named_in_issues(&tests, &issues),
            [
                "`manager::flow::peer_chain_tests::the_ninth_message_waits` (#230)",
                "`tests/test_api.py::test_login` (#12)",
            ]
        );
    }

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
            asked: None,
            owed: false,
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
        board.integrated_checks.get_mut("check-10").unwrap().owed = true;
        assert_eq!(
            checking_line(&board, "r1").as_deref(),
            Some(
                "[the session's full checks are due on `work` at b2, in the background: their result comes as a message. Answer now, or reply [quiet] to answer with their result.]"
            ),
            "a run_checks call that went on is owed its pass too"
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
    fn the_thread_reads_where_the_full_checks_and_its_background_runs_stand() {
        let mut board = crate::board::Board::default();
        assert_eq!(
            standing_lines(&board, 0),
            ["Full checks: none ran in this session yet."]
        );
        let mut passed = check(
            "check-1",
            "a1",
            IntegratedCheckState::Passed {
                commands: Vec::new(),
            },
        );
        passed.ended_at_ms = Some(0);
        board.integrated_checks.insert(passed.id.clone(), passed);
        assert_eq!(
            standing_lines(&board, 5 * 60_000),
            [
                "Full checks: passed on `work` at a1, 5 min ago. A newer commit needs run_checks again."
            ]
        );
        let mut running = check(
            "check-2",
            "b2",
            IntegratedCheckState::Running {
                command: None,
                done: 0,
                total: 0,
            },
        );
        running.started_at_ms = Some(60_000);
        board.integrated_checks.insert(running.id.clone(), running);
        board.background_runs.insert(
            "run-1".into(),
            crate::work::BackgroundRun {
                id: "run-1".into(),
                conversation_id: ConversationId("c".into()),
                request_id: None,
                tool: "run_check".into(),
                command: "cargo test".into(),
                started_at_ms: 0,
                ended_at_ms: None,
                state: crate::work::BackgroundRunState::Running,
                item_id: None,
                name: None,
            },
        );
        assert_eq!(
            standing_lines(&board, 7 * 60_000),
            [
                "Full checks: running on `work` at b2 for 6 min, in the background; their result comes as a message.",
                "run-1 running in the background for 7 min: run_check `cargo test`."
            ]
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
            let status = crate::test_git_command()
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
