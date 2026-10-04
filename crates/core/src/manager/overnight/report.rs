//! The morning report (PLAN.md §10.11): one message at the end of the run, rendered from the
//! run's records, never from a model's memory of it. What shows is short. First come three
//! lines, in the run card's words: the outcome, what Merge takes and what waits on the user.
//! Then one line per phase, what waits on the user and what got in the way. Everything after
//! the `### Details` heading the app shows folded: the branch, the commits, what was decided
//! for the user (one short line each), each phase's criteria with their evidence, its checks
//! and the lead's answers, who did the work, and the usage.
//!
//! It is written once: its message has a stable id per run segment, looked for before it is
//! appended, and the run records it after. A crash in between finds the message on recovery
//! and only records it. The notification the report comes with is queued on the run at the
//! same time; the app delivers it as Brigadier and acknowledges it.
//!
//! A finished run whose report has an older shape is rendered again once, from its records,
//! into `report_text`, which the app shows in place of the message. Its commits end at the tip
//! recorded with the report, or, for a report from before that was recorded, they come from
//! its tasks' landings. When those don't add up, the run keeps the report it has.

use super::super::decisions::{named, short_words, waiting_run};
use super::super::gates::{criterion_evidence, without_marker};
use super::super::{SessionManager, blocking, git_error};
use super::directives::{Clock, parse};
use super::wind_down::{CUT_OFF, to_you};
use crate::board::Board;
use crate::model::{DomainEvent, OvernightRunId, Setup};
use crate::now_ms;
use crate::overnight::{
    CriterionStatus, Deadline, OvernightPhase, OvernightRun, PhaseState, REPORT_VERSION,
    RunNotification, RunRole, StopReason,
};
use crate::sessions::one_line;
use crate::work::{
    Decision, DecisionKind, DecisionSource, GateOutcome, RequestState, Task, TaskKind, TaskState,
    UserRequest,
};
use brigadier_providers::ProviderKind;

/// Commits listed in the report, at most.
const COMMITS: usize = 60;
/// A criterion's evidence in the details, at most (the full text is on the phase).
const EVIDENCE: usize = 160;
/// A decision's line, at most.
const DECIDED: usize = 140;
/// A missing criterion or a gap on a phase's line, at most.
const MISSING: usize = 90;
/// Check names listed per kind of check that couldn't run.
const CHECK_NAMES: usize = 3;
/// The heading after which the app folds the report.
pub const DETAILS: &str = "### Details";

/// A commit on the run branch.
#[derive(Debug, Clone, PartialEq)]
struct RunCommit {
    commit: String,
    subject: String,
}

/// Recorded tokens per provider; `None` when its records can't be read.
type Usage = Vec<(ProviderKind, Option<u64>)>;

impl SessionManager {
    /// Writes the run's report into its conversation, once, and queues its notification.
    pub(crate) async fn write_run_report(&self, run: &OvernightRun) {
        let _held = self.overnight.reporting.lock().await;
        let id = &run.conversation_id;
        let board = match self.core.board(id).await {
            Ok(board) => board,
            Err(err) => {
                tracing::warn!(run = %run.id, error = %err, "could not read the run for its report");
                return;
            }
        };
        let Some(run) = board.runs.get(&run.id) else {
            return;
        };
        if run.report_message_id.is_some() {
            return;
        }
        let message_id = format!("run-report-{}", run.id);
        // Search every branch, including messages older than the active thread's page. A
        // crash after append must not duplicate the report after a branch switch.
        let written = match self.core.all_messages(id).await {
            Ok(messages) => messages
                .into_iter()
                .find(|message| message.id == message_id),
            Err(err) => {
                tracing::warn!(run = %run.id, error = %err, "could not reconcile the report");
                return;
            }
        };
        let now = now_ms();
        let tip = self.branch_tip(run).await;
        let commits = match &tip {
            Some(tip) => self.commits_to(run, tip).await,
            None => None,
        }
        .unwrap_or_else(|| landed_commits(run, &board));
        let usage = self.run_usage(run, &board, now).await;
        let mut text = render(run, &board, &commits, &usage, now);
        if let Some(message) = &written {
            // Reconciliation uses the text already posted, not newly rendered facts.
            text = self.full_text(message).await;
        }
        let outcome = outcome_of(&text);
        if written.is_none() {
            let request_id = format!("run-{}-report", run.id.short());
            let now = now_ms();
            let request = self
                .core
                .record_conversation(
                    id,
                    vec![DomainEvent::RequestUpdated {
                        request: UserRequest {
                            id: request_id.clone(),
                            conversation_id: id.clone(),
                            preview: format!("Overnight report · {}", run.name),
                            state: RequestState::Done,
                            started_at_ms: now,
                            ended_at_ms: Some(now),
                            steered_into: None,
                            steered_after: None,
                            undo: None,
                        },
                    }],
                )
                .await;
            if let Err(err) = request {
                tracing::warn!(run = %run.id, error = %err, "could not file the run's report");
                return;
            }
            if let Err(err) = self
                .core
                .append_assistant_message(
                    id.clone(),
                    message_id.clone(),
                    text.clone(),
                    None,
                    Some(request_id),
                )
                .await
            {
                tracing::warn!(run = %run.id, error = %err, "could not write the run's report");
                return;
            }
        }
        let notification = RunNotification {
            id: format!("run-notice-{}", run.id),
            title: notification_title(run, now),
            body: notification_body(run, &board),
            created_at_ms: now_ms(),
            delivered_at_ms: None,
            delivery_error: None,
        };
        let _change = self.overnight.changes.lock().await;
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        let Some(mut now) = board.runs.get(&run.id).cloned() else {
            return;
        };
        if now.generation != run.generation || now.report_message_id.is_some() {
            return;
        }
        now.report_message_id = Some(message_id);
        now.report_outcome = outcome;
        // A message found on reconciliation may be in an older shape: it is rendered again.
        now.report_version = if written.is_none() { REPORT_VERSION } else { 0 };
        now.end_commit = tip;
        if now.notification.is_none() {
            now.notification = Some(notification);
        }
        if let Err(err) = self.record_run(&now).await {
            tracing::warn!(run = %run.id, error = %err, "could not record the report/outbox; will reconcile it");
        }
    }

    /// Renders a finished run's report in the current shape, once, when it was written in an
    /// older one. Nothing changes when its commits can't be rebuilt from its records.
    async fn rerender_run_report(&self, run: &OvernightRun) {
        let fresh = self
            .overnight
            .rerendered
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(run.id.clone());
        if !fresh {
            return;
        }
        let _held = self.overnight.reporting.lock().await;
        let Ok(board) = self.core.board(&run.conversation_id).await else {
            return;
        };
        let Some(run) = board.runs.get(&run.id) else {
            return;
        };
        if run.report_message_id.is_none() || run.report_version >= REPORT_VERSION {
            return;
        }
        let commits = match &run.end_commit {
            Some(end) => self.commits_to(run, end).await,
            None => None,
        }
        .unwrap_or_else(|| landed_commits(run, &board));
        let end = run.finished_at_ms.unwrap_or_else(now_ms);
        let usage = self.run_usage(run, &board, end).await;
        let Some(text) = rebuilt(run, &board, &commits, &usage) else {
            tracing::warn!(run = %run.id, "the run's commits don't reach its verified tip; its report stays as written");
            return;
        };
        let outcome = outcome_of(&text);
        let _change = self.overnight.changes.lock().await;
        let Ok(board) = self.core.board(&run.conversation_id).await else {
            return;
        };
        let Some(mut now) = board.runs.get(&run.id).cloned() else {
            return;
        };
        if now.report_message_id.is_none() || now.report_version >= REPORT_VERSION {
            return;
        }
        now.report_text = Some(text);
        now.report_outcome = outcome;
        now.report_version = REPORT_VERSION;
        if let Err(err) = self.record_run(&now).await {
            tracing::warn!(run = %run.id, error = %err, "could not record the report rendered again");
        }
    }

    /// Recorded tokens per provider for this segment's tasks and lead. Only turns belonging to
    /// them, between its start and `end`, are counted.
    async fn run_usage(&self, run: &OvernightRun, board: &Board, end: i64) -> Usage {
        let start = run.started_at_ms.unwrap_or(run.created_at_ms);
        let end = run.finished_at_ms.unwrap_or(end);
        let mut usage = Vec::new();
        for provider in ProviderKind::ALL {
            let turns = match self.runtime.routing_store() {
                Some(store) => store.turns_since(provider, start).await.ok(),
                None => None,
            };
            let tokens = turns.map(|turns| {
                turns
                    .into_iter()
                    .filter(|turn| turn.at_ms <= end)
                    .filter(|turn| match turn.task_id.as_ref() {
                        Some(task_id) => board.tasks.values().any(|task| {
                            &task.id.0 == task_id
                                && task
                                    .run
                                    .as_ref()
                                    .is_some_and(|context| context.run_id == run.id)
                        }),
                        None => {
                            turn.conversation_id.as_deref() == Some(run.conversation_id.0.as_str())
                        }
                    })
                    .map(|turn| turn.input + turn.cached_input + turn.cache_write + turn.output)
                    .sum::<i64>()
            });
            usage.push((
                provider,
                tokens.map(|tokens| u64::try_from(tokens).unwrap_or(0)),
            ));
        }
        usage
    }

    /// The app showed a run's notification.
    pub async fn ack_overnight_notification(
        &self,
        conversation_id: crate::model::ConversationId,
        run_id: crate::model::OvernightRunId,
        notification_id: String,
    ) -> crate::Result<()> {
        let board = self.core.board(&conversation_id).await?;
        let run = board
            .runs
            .get(&run_id)
            .cloned()
            .ok_or_else(|| crate::Error::NotFound(format!("overnight run {run_id}")))?;
        let _held = self.overnight.changes.lock().await;
        let board = self.core.board(&conversation_id).await?;
        let Some(mut now) = board.runs.get(&run.id).cloned() else {
            return Ok(());
        };
        match now.notification.as_mut() {
            Some(notice) if notice.id == notification_id && notice.delivered_at_ms.is_none() => {
                notice.delivered_at_ms = Some(now_ms());
                notice.delivery_error = None;
            }
            _ => return Ok(()),
        }
        self.record_run(&now).await
    }

    /// An OS refusal is durable and visible while the notification stays pending.
    pub async fn fail_overnight_notification(
        &self,
        conversation_id: crate::model::ConversationId,
        run_id: crate::model::OvernightRunId,
        notification_id: String,
        error: String,
    ) -> crate::Result<()> {
        let _held = self.overnight.changes.lock().await;
        let board = self.core.board(&conversation_id).await?;
        let Some(mut run) = board.runs.get(&run_id).cloned() else {
            return Ok(());
        };
        let Some(notice) = run.notification.as_mut() else {
            return Ok(());
        };
        if notice.id != notification_id
            || notice.delivered_at_ms.is_some()
            || notice.delivery_error.as_ref() == Some(&error)
        {
            return Ok(());
        }
        notice.delivery_error = Some(error.chars().take(1000).collect());
        self.record_run(&run).await
    }

    /// Notifications of finished runs the app hasn't shown yet, in every session.
    pub async fn pending_overnight_notifications(
        &self,
    ) -> Vec<(
        crate::model::ConversationId,
        crate::model::OvernightRunId,
        RunNotification,
    )> {
        let mut pending = Vec::new();
        for conversation in self.core.catalog().conversations {
            if !matches!(conversation.setup, Some(Setup::Session { .. })) {
                continue;
            }
            let Ok(board) = self.core.board(&conversation.id).await else {
                continue;
            };
            for saved in board.runs.values() {
                // Also reconcile terminal setup failures and a report cut off by persistence,
                // and render a report in an older shape again.
                if saved.state != crate::overnight::OvernightState::Finished {
                    continue;
                }
                if saved.report_message_id.is_none() {
                    self.write_run_report(saved).await;
                } else if saved.report_version < REPORT_VERSION {
                    self.rerender_run_report(saved).await;
                }
            }
            let Ok(board) = self.core.board(&conversation.id).await else {
                continue;
            };
            for run in board.runs.values() {
                if let Some(notice) = &run.notification
                    && notice.delivered_at_ms.is_none()
                {
                    pending.push((conversation.id.clone(), run.id.clone(), notice.clone()));
                }
            }
        }
        pending
    }

    /// The run branch's tip now; at report time, that is where the run's commits end.
    async fn branch_tip(&self, run: &OvernightRun) -> Option<String> {
        let workspace = run.workspace.clone()?;
        let repo = self.run_repo(run)?;
        let git = self.git.clone();
        blocking(move || {
            let repo = git.open(std::path::Path::new(&repo)).map_err(git_error)?;
            Ok(repo
                .branch_tip(&workspace.branch)
                .map_err(git_error)?
                .map(|tip| tip.0))
        })
        .await
        .ok()
        .flatten()
    }

    /// The run's commits from its base up to `end`, newest first. `None` when git can't read
    /// them.
    async fn commits_to(&self, run: &OvernightRun, end: &str) -> Option<Vec<RunCommit>> {
        let workspace = run.workspace.clone()?;
        let repo = self.run_repo(run)?;
        let git = self.git.clone();
        let end = brigadier_git::Oid(end.to_owned());
        let logged = blocking(move || {
            let repo = git.open(std::path::Path::new(&repo)).map_err(git_error)?;
            let mut commits = Vec::new();
            for commit in repo.log(&end, COMMITS).map_err(git_error)? {
                if commit.commit.0 == workspace.base_commit {
                    break;
                }
                commits.push((commit.commit.0, commit.subject));
            }
            Ok(commits)
        })
        .await
        .ok()?;
        Some(
            logged
                .into_iter()
                .map(|(commit, subject)| RunCommit { commit, subject })
                .collect(),
        )
    }

    /// The repository of the run's session.
    fn run_repo(&self, run: &OvernightRun) -> Option<String> {
        match self.core.conversation(&run.conversation_id).ok()?.setup {
            Some(Setup::Session { repo, .. }) => Some(repo),
            _ => None,
        }
    }
}

/// A finished run's report in the current shape, from its records; `None` when its commits
/// don't reach the tip its Merge takes (they can't be the run's).
fn rebuilt(
    run: &OvernightRun,
    board: &Board,
    commits: &[RunCommit],
    usage: &Usage,
) -> Option<String> {
    let reaches = match (&run.verified_commit, &run.workspace) {
        (Some(tip), Some(workspace)) if tip != &workspace.base_commit => {
            commits.iter().any(|commit| &commit.commit == tip)
        }
        _ => true,
    };
    reaches.then(|| {
        render(
            run,
            board,
            commits,
            usage,
            run.finished_at_ms.unwrap_or_else(now_ms),
        )
    })
}

/// The report's three opening paragraphs, which the run card shows too.
fn outcome_of(text: &str) -> Option<[String; 3]> {
    let outcome: Vec<String> = text.split("\n\n").take(3).map(str::to_owned).collect();
    outcome.try_into().ok()
}

/// The run and the segments it continues, which all work on its branch.
fn chain<'a>(run: &'a OvernightRun, board: &'a Board) -> Vec<&'a OvernightRunId> {
    let mut ids = vec![&run.id];
    let mut at = run;
    while let Some(previous) = at.predecessor.as_ref().and_then(|id| board.runs.get(id)) {
        if ids.contains(&&previous.id) {
            break;
        }
        ids.push(&previous.id);
        at = previous;
    }
    ids
}

/// The run's commits from its tasks' landings (its own and the segments it continues), newest
/// first: the record of what is on its branch, whatever the branch holds now.
fn landed_commits(run: &OvernightRun, board: &Board) -> Vec<RunCommit> {
    let segments = chain(run, board);
    let landed_at = |task: &Task| {
        board
            .decisions
            .iter()
            .find(|decision| {
                matches!(&decision.source, DecisionSource::Task { task_id } if task_id == &task.id)
                    && decision.what.starts_with("Landed ")
            })
            .map_or(task.updated_at_ms, |decision| decision.at_ms)
    };
    let mut tasks: Vec<&Task> = board
        .tasks
        .values()
        .filter(|task| {
            task.landed.is_some()
                && task
                    .run
                    .as_ref()
                    .is_some_and(|context| segments.contains(&&context.run_id))
        })
        .collect();
    tasks.sort_by_key(|task| std::cmp::Reverse((landed_at(task), task.number)));
    tasks
        .into_iter()
        .take(COMMITS)
        .map(|task| RunCommit {
            commit: task.landed.clone().unwrap_or_default(),
            subject: task
                .candidate
                .as_ref()
                .and_then(|candidate| candidate.message.lines().next())
                .map_or_else(|| task.title.clone(), |line| line.trim().to_owned()),
        })
        .collect()
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(7)]
}

fn counts(run: &OvernightRun) -> (usize, usize) {
    let worked: Vec<&OvernightPhase> = run
        .phases
        .iter()
        .filter(|phase| phase.state != PhaseState::Skipped)
        .collect();
    let verified = worked
        .iter()
        .filter(|p| p.state == PhaseState::Verified)
        .count();
    (worked.len(), verified)
}

/// A local time of day, "07:04".
fn clock_time(at_ms: i64) -> String {
    jiff::Timestamp::from_millisecond(at_ms).map_or_else(
        |_| "?".into(),
        |at| {
            at.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%H:%M")
                .to_string()
        },
    )
}

/// How the run ended, and when (its end, or `now` while the report is being written).
fn ending(run: &OvernightRun, now: i64) -> String {
    let at = format!(" at {}", clock_time(run.finished_at_ms.unwrap_or(now)));
    match &run.stop {
        Some(StopReason::Done) => format!("finished{at}"),
        Some(StopReason::Stopped) => format!("stopped by you{at}"),
        Some(StopReason::Deadline) => match &run.directives.deadline {
            Deadline::At { time } => format!("stopped at its {} deadline", time.local_time),
            _ => format!("stopped at the deadline{at}"),
        },
        Some(StopReason::StopDirective) => format!("stopped where you asked{at}"),
        Some(StopReason::Blocked { phase_id }) => {
            let number = run
                .phase(phase_id)
                .map_or_else(|| "0".to_owned(), |phase| phase.number.to_string());
            format!("stopped early{at}: phase {number} needs you")
        }
        Some(StopReason::Failed { message }) => {
            format!("could not run: {}", message.trim_end_matches('.'))
        }
        None => format!("ended{at}"),
    }
}

/// The run's own Waiting items, and those of its tasks.
fn waiting<'a>(run: &OvernightRun, board: &'a Board) -> Vec<&'a str> {
    let mut items: Vec<_> = board
        .waiting
        .values()
        .filter(|item| {
            waiting_run(&item.source, item.request_id.as_deref(), board).as_ref() == Some(&run.id)
        })
        .collect();
    items.sort_by_key(|item| item.created_at_ms);
    items.iter().map(|item| item.what.as_str()).collect()
}

fn notification_title(run: &OvernightRun, now: i64) -> String {
    match &run.stop {
        Some(StopReason::Blocked { .. }) => format!("{}: {}", run.name, ending(run, now)),
        _ => format!("{} {}", run.name, ending(run, now)),
    }
}

fn notification_body(run: &OvernightRun, board: &Board) -> String {
    let (worked, verified) = counts(run);
    let waits = waiting(run, board).len();
    let mut body = format!("{verified} of {worked} phases verified");
    if waits > 0 {
        body.push_str(&format!(" · {waits} waiting on you"));
    }
    body
}

/// "1 task", "2 tasks".
fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// "a", "a and b", "a, b and c".
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// A line ending in a full stop.
fn sentence(line: &str) -> String {
    let line = line.trim();
    if line.ends_with(['.', '!', '?', '…']) {
        line.to_owned()
    } else {
        format!("{line}.")
    }
}

/// The report, from the records alone, as of `now`.
fn render(
    run: &OvernightRun,
    board: &Board,
    commits: &[RunCommit],
    usage: &Usage,
    now: i64,
) -> String {
    let waits = waiting(run, board);
    let mut text = String::new();
    // The three lines the run card shows too.
    text.push_str(&outcome_line(run, now));
    text.push_str("\n\n");
    text.push_str(&merge_line(run, commits));
    text.push_str("\n\n");
    text.push_str(&match waits.len() {
        0 => "Nothing waits on you.".to_owned(),
        1 => "1 thing waits on you.".to_owned(),
        n => format!("{n} things wait on you."),
    });
    text.push_str("\n\n");
    for line in phase_lines(run, board) {
        text.push_str(&format!("- {line}\n"));
    }
    if !waits.is_empty() {
        text.push_str("\nWaiting on you:\n");
        for item in &waits {
            text.push_str(&format!("- {}\n", item.trim()));
        }
        text.push_str("Then say \u{201c}continue\u{201d} to pick the run up on the same branch.\n");
    }
    let in_the_way = in_the_way(run, board, now);
    match in_the_way.as_slice() {
        [] => {}
        [one] => text.push_str(&format!("\nGot in the way: {one}\n")),
        lines => {
            text.push_str("\nGot in the way:\n");
            for line in lines {
                text.push_str(&format!("- {line}\n"));
            }
        }
    }
    // The details, folded by the app: everything after this heading.
    text.push_str(&format!("\n{DETAILS}\n"));
    text.push_str(&details(run, board, commits, usage));
    // Workers by the names the app shows, also in text written before they were; the run's
    // words about the user said to the user.
    named(&to_you(&text), board)
}

/// "**Name**: stopped by you at 07:04. 1 of 3 phases verified."
fn outcome_line(run: &OvernightRun, now: i64) -> String {
    let ending = ending(run, now);
    if matches!(run.stop, Some(StopReason::Failed { .. })) {
        return format!("**{}**: {ending}.", run.name);
    }
    let (worked, verified) = counts(run);
    if worked == 0 {
        return format!("**{}**: {ending}. No phases were planned.", run.name);
    }
    format!(
        "**{}**: {ending}. {verified} of {} verified.",
        run.name,
        plural(worked, "phase", "phases")
    )
}

/// Which verified phases Merge takes: "phase 1", "phases 1 and 2" (as the run card says it).
fn merged_phases(run: &OvernightRun) -> Option<String> {
    let numbers: Vec<String> = run
        .phases
        .iter()
        .filter(|phase| phase.state == PhaseState::Verified)
        .map(|phase| phase.number.to_string())
        .collect();
    match numbers.len() {
        0 => None,
        1 => Some(format!("phase {}", numbers[0])),
        _ => Some(format!("phases {}", and_list(&numbers))),
    }
}

/// What Merge takes, and what stays unverified on the branch.
fn merge_line(run: &OvernightRun, commits: &[RunCommit]) -> String {
    let Some(workspace) = &run.workspace else {
        return "No branch was made.".into();
    };
    if run.merged.is_some() {
        return format!("Merged into `{}`.", workspace.base);
    }
    let stay = |count: usize, later: &str| match count {
        0 => String::new(),
        1 => format!(" 1 {later}commit stays unverified on the branch."),
        n => format!(" {n} {later}commits stay unverified on the branch."),
    };
    match (&run.verified_commit, merged_phases(run)) {
        (Some(tip), Some(phases)) if tip != &workspace.base_commit => {
            let later = commits
                .iter()
                .take_while(|commit| &commit.commit != tip)
                .count();
            format!(
                "Merge takes {phases} (`{}`).{}",
                short(tip),
                stay(later, "later ")
            )
        }
        _ => format!("Nothing verified to merge yet.{}", stay(commits.len(), "")),
    }
}

/// A phase's mark and name, as the run card and the thread's phase headers show them once the
/// run is over: "✓ Phase 1 · Measure", "Phase 3 · Re-measure" for one not reached.
fn phase_head(state: PhaseState, title: &str) -> String {
    let mark = match state {
        PhaseState::Verified => "✓ ",
        PhaseState::Partial | PhaseState::Running | PhaseState::Checking => "◐ ",
        PhaseState::Blocked => "✕ ",
        PhaseState::Skipped => "– ",
        PhaseState::Pending => "",
    };
    format!("{mark}{title}")
}

/// A phase's state in a word or two once the run is over, the run card's and the thread's
/// word for it (`phaseWord` in the app).
fn phase_word(state: PhaseState) -> &'static str {
    match state {
        PhaseState::Pending => "not reached",
        PhaseState::Running | PhaseState::Checking => "unfinished",
        PhaseState::Verified => "verified",
        PhaseState::Partial => "partial",
        PhaseState::Blocked => "blocked",
        PhaseState::Skipped => "skipped",
    }
}

/// One line per phase: its state in the run card's word, why when it isn't verified, and
/// what landed.
fn phase_lines(run: &OvernightRun, board: &Board) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(planning) = &run.planning {
        let head = phase_head(planning.state, "Phase 0 · Write the plan");
        let word = phase_word(planning.state);
        lines.push(match (planning.state, planning.gaps.first()) {
            (PhaseState::Verified, _) | (_, None) => format!("{head}: {word}."),
            (_, Some(gap)) => format!("{head}: {word}, {}", sentence(&one_line(gap, MISSING))),
        });
    }
    let segments = chain(run, board);
    for phase in &run.phases {
        let head = phase_head(
            phase.state,
            &format!("Phase {} · {}", phase.number, phase.name),
        );
        let word = phase_word(phase.state);
        let (landed, not_landed) = landings(phase, board, &segments);
        let tasks = |verified: bool| match (landed, not_landed) {
            (0, 0) => String::new(),
            (0, n) if !verified => format!(" {} land.", plural(n, "task didn't", "tasks didn't")),
            (n, m) if m > 0 && !verified => {
                format!(" {} landed, {m} didn't.", plural(n, "task", "tasks"))
            }
            (0, _) => String::new(),
            (n, _) => format!(" {} landed.", plural(n, "task", "tasks")),
        };
        lines.push(match phase.state {
            PhaseState::Verified => format!("{head}: {word}.{}", tasks(true)),
            PhaseState::Partial => format!(
                "{head}: {word}, {}.{}",
                unverified_why(run, phase),
                tasks(false)
            ),
            PhaseState::Blocked => format!(
                "{head}: {word}, it needs you: {}",
                sentence(&one_line(
                    phase
                        .gaps
                        .first()
                        .map_or("see Waiting on you", String::as_str),
                    MISSING
                ))
            ),
            PhaseState::Skipped | PhaseState::Pending => format!("{head}: {word}."),
            PhaseState::Running | PhaseState::Checking => {
                format!("{head}: {word}.{}", tasks(false))
            }
        });
    }
    lines
}

/// The phase's write tasks that landed, and those that ended without landing.
fn landings(phase: &OvernightPhase, board: &Board, segments: &[&OvernightRunId]) -> (usize, usize) {
    let tasks = board.tasks.values().filter(|task| {
        task.kind.writes()
            && task.run.as_ref().is_some_and(|context| {
                context.phase_id.as_deref() == Some(phase.id.as_str())
                    && segments.contains(&&context.run_id)
            })
    });
    let (mut landed, mut not_landed) = (0, 0);
    for task in tasks {
        if task.landed.is_some() {
            landed += 1;
        } else if matches!(
            task.state,
            TaskState::Stopped | TaskState::Failed | TaskState::Rejected
        ) {
            not_landed += 1;
        }
    }
    (landed, not_landed)
}

/// Why a partial phase isn't verified, in a few words.
fn unverified_why(run: &OvernightRun, phase: &OvernightPhase) -> String {
    let missing: Vec<&str> = phase
        .criteria
        .iter()
        .filter(|result| {
            matches!(
                result.status,
                CriterionStatus::NotMet | CriterionStatus::Blocked
            )
        })
        .filter_map(|result| {
            phase
                .done_when
                .iter()
                .find(|criterion| criterion.id == result.id)
                .map(|criterion| criterion.text.as_str())
        })
        .collect();
    if let Some(first) = missing.first() {
        let more = match missing.len() {
            1 => String::new(),
            n => format!(" and {} more", n - 1),
        };
        return format!(
            "missing: {}{more}",
            one_line(first, MISSING).trim_end_matches('.')
        );
    }
    // A phase its checks settled before the run ended says why in a gap of its own.
    if let Some(gap) = phase.gaps.iter().find(|gap| !gap.starts_with(CUT_OFF)) {
        return one_line(gap, MISSING).trim_end_matches('.').to_owned();
    }
    match &run.stop {
        Some(StopReason::Stopped) => "you stopped the run".into(),
        Some(StopReason::Deadline) => "the deadline came first".into(),
        Some(StopReason::StopDirective) => "the run stopped where you asked".into(),
        Some(StopReason::Blocked { .. }) => "the run stopped early".into(),
        _ => phase.gaps.first().map_or_else(
            || "its checks never passed".into(),
            |gap| one_line(gap, MISSING).trim_end_matches('.').to_owned(),
        ),
    }
}

/// What got in the way of the run, from its records, grouped: what the overnight rules
/// declined, resumes that failed, changes held unverified, checks that couldn't run, work
/// stopped before it finished, interruptions and lateness. A phase not reached says so on its
/// own line.
fn in_the_way(run: &OvernightRun, board: &Board, now: i64) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for obstacle in &run.obstacles {
        lines.push(format!(
            "{}{}",
            obstacle.text.trim_end_matches('.'),
            times_in(obstacle.count, &obstacle.tasks)
        ));
    }
    let mut held: Vec<u32> = run_decisions(run, board)
        .filter(|decision| decision.what.starts_with("Held "))
        .filter_map(|decision| decided_task(decision, board).map(|task| task.number))
        .collect();
    held.sort_unstable();
    held.dedup();
    if !held.is_empty() {
        lines.push(format!(
            "Held because {} change couldn't be verified: {}",
            if held.len() == 1 { "its" } else { "their" },
            task_list(&held)
        ));
    }
    lines.extend(unrun_checks(run, board));
    let stopped: Vec<u32> = run_tasks(run, board)
        .filter(|task| {
            task.kind.writes() && task.state == TaskState::Stopped && task.candidate.is_none()
        })
        .map(|task| task.number)
        .collect();
    if !stopped.is_empty() {
        lines.push(format!(
            "Stopped before it finished: {}",
            task_list(&stopped)
        ));
    }
    for gap in &run.gaps {
        lines.push(format!("{}; no work happened then", gap.cause));
    }
    if let Deadline::At { time } = &run.directives.deadline {
        let finished = run.finished_at_ms.unwrap_or(now);
        if finished > time.at_ms {
            lines.push(format!(
                "The report is {} minutes late (due at {})",
                (finished - time.at_ms) / 60_000,
                time.local_time
            ));
        }
    }
    lines.iter().map(|line| sentence(line)).collect()
}

/// This segment's tasks, in the order they were made.
fn run_tasks<'a>(run: &'a OvernightRun, board: &'a Board) -> impl Iterator<Item = &'a Task> {
    let mut tasks: Vec<&Task> = board
        .tasks
        .values()
        .filter(|task| {
            task.run
                .as_ref()
                .is_some_and(|context| context.run_id == run.id)
        })
        .collect();
    tasks.sort_by_key(|task| task.number);
    tasks.into_iter()
}

/// "task-3, task-5".
fn task_list(numbers: &[u32]) -> String {
    numbers
        .iter()
        .map(|n| format!("task-{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Checks the run's verifiers couldn't run, one line per kind: the most often named checks
/// and how many tasks' checks named any.
fn unrun_checks(run: &OvernightRun, board: &Board) -> Vec<String> {
    const KINDS: [(&str, &str); 3] = [
        ("[not run]", "Not run"),
        ("[excluded]", "Excluded by a rule or the environment"),
        ("[pre-existing]", "Already failing before the run"),
    ];
    let mut lines = Vec::new();
    for (marker, label) in KINDS {
        // Each check by its name as first written, with how often it was named.
        let mut names: Vec<(String, String, usize)> = Vec::new();
        let mut tasks: Vec<u32> = Vec::new();
        for verifier in run_tasks(run, board).filter(|task| task.kind == TaskKind::Verify) {
            let Some(report) = &verifier.report else {
                continue;
            };
            let checked = verifier
                .subject
                .as_ref()
                .and_then(|id| board.tasks.get(id))
                .map_or(verifier.number, |task| task.number);
            for line in &report.risks {
                let Some(check) = unrun_check(line, marker) else {
                    continue;
                };
                let key = check.to_lowercase();
                match names.iter_mut().find(|(known, _, _)| *known == key) {
                    Some(entry) => entry.2 += 1,
                    None => names.push((key, check.to_owned(), 1)),
                }
                if !tasks.contains(&checked) {
                    tasks.push(checked);
                }
            }
        }
        if names.is_empty() {
            continue;
        }
        // Most often first; the first named first among equals (the sort is stable).
        names.sort_by_key(|(_, _, count)| std::cmp::Reverse(*count));
        let mut shown: Vec<String> = names
            .iter()
            .take(CHECK_NAMES)
            .map(|(_, name, _)| name.clone())
            .collect();
        if names.len() > CHECK_NAMES {
            shown.push(format!("{} more", names.len() - CHECK_NAMES));
        }
        lines.push(format!(
            "{label}: {} ({})",
            and_list(&shown),
            plural(tasks.len(), "task", "tasks")
        ));
    }
    lines
}

/// The check a verifier's risk line names after `marker` ("[not run] GUI smoke: why"), without
/// its reason and quoting.
fn unrun_check<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    let line = without_marker(line).trim();
    let rest = line
        .get(..marker.len())
        .filter(|head| head.eq_ignore_ascii_case(marker))
        .map(|_| line[marker.len()..].trim())?;
    let check = rest.split_once(':').map_or(rest, |(check, _)| check).trim();
    let check = check.trim_matches('`').trim();
    (!check.is_empty()).then_some(check)
}

/// " (3 times, task-2, task-5)": how often and for which tasks, when it says anything.
fn times_in(count: u32, tasks: &[u32]) -> String {
    let mut parts = Vec::new();
    if count > 1 {
        parts.push(format!("{count} times"));
    }
    parts.extend(tasks.iter().map(|n| format!("task-{n}")));
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    }
}

/// The folded part: where the work is, the commits, what was decided, each phase's evidence,
/// who did the work, the usage and what Brigadier ignored in the user's words.
fn details(run: &OvernightRun, board: &Board, commits: &[RunCommit], usage: &Usage) -> String {
    let mut text = String::new();
    if let Some(workspace) = &run.workspace {
        text.push_str(&format!(
            "Branch `{}` from `{}`. Worktree and handoffs: `{}`.\n",
            workspace.branch, workspace.base, workspace.path
        ));
    }
    if !commits.is_empty() {
        text.push_str("\nCommits:\n");
        let mut verified = false;
        for commit in commits {
            if Some(&commit.commit) == run.verified_commit.as_ref() {
                verified = true;
            }
            text.push_str(&format!(
                "- `{}` {}{}\n",
                short(&commit.commit),
                commit.subject,
                if verified { "" } else { " (unverified)" }
            ));
        }
        if commits.len() == COMMITS {
            text.push_str("Only the latest 60 are listed; the branch has them all.\n");
        }
    }
    let decided = decision_lines(run, board);
    if !decided.is_empty() {
        text.push_str("\nDecided for you:\n");
        for line in decided {
            text.push_str(&format!("- {line}\n"));
        }
    }
    for phase in &run.phases {
        if matches!(phase.state, PhaseState::Pending | PhaseState::Skipped) {
            continue;
        }
        text.push_str(&format!("\nPhase {} · {}:\n", phase.number, phase.name));
        for line in evidence_lines(phase, board) {
            text.push_str(&format!("- {line}\n"));
        }
    }
    let workers = worker_lines(run, board);
    if !workers.is_empty() {
        text.push_str("\nWho worked:\n");
        for line in workers {
            text.push_str(&format!("- {line}\n"));
        }
    }
    text.push_str(&format!("\n{}\n", usage_line(usage)));
    let ignored = ignored(run);
    if !ignored.is_empty() {
        text.push_str(&format!(
            "Ignored in your words (Brigadier picks these itself): {}.\n",
            and_list(&ignored)
        ));
    }
    text
}

/// Each criterion with its status, who checked it and its evidence; the whole-phase checks;
/// their findings and the lead's answers. One short line each.
fn evidence_lines(phase: &OvernightPhase, board: &Board) -> Vec<String> {
    let mut lines = Vec::new();
    for criterion in &phase.done_when {
        let result = phase.criteria.iter().find(|c| c.id == criterion.id);
        let line = match result {
            Some(result) => {
                let by = result
                    .by
                    .as_ref()
                    .and_then(|id| board.tasks.get(id))
                    .map(|task| format!(", checked by task-{}", task.number))
                    .unwrap_or_default();
                let evidence = evidence_text(&result.evidence);
                let evidence = evidence.strip_prefix("Not checked: ").unwrap_or(evidence);
                format!(
                    "{} {}{by}: {}",
                    criterion.id,
                    status_word(result.status),
                    one_line(evidence, EVIDENCE)
                )
            }
            None => format!("{} not checked", criterion.id),
        };
        lines.push(sentence(&line));
    }
    match &phase.gate {
        Some(gate) => {
            let on = gate
                .commit
                .as_deref()
                .map(|commit| format!(" on `{}`", short(commit)))
                .unwrap_or_default();
            let fixes = match phase.fix_rounds {
                0 => String::new(),
                n => format!(", after {}", plural(n as usize, "fix round", "fix rounds")),
            };
            lines.push(match gate.outcome {
                Some(GateOutcome::Passed) => {
                    format!(
                        "Whole-phase checks passed in round {}{on}{fixes}.",
                        gate.round
                    )
                }
                Some(_) => format!(
                    "Whole-phase checks found gaps in round {}{on}{fixes}.",
                    gate.round
                ),
                None => format!(
                    "Whole-phase checks didn't finish round {}{on}{fixes}.",
                    gate.round
                ),
            });
            if gate.outcome != Some(GateOutcome::Passed) {
                for finding in &gate.findings {
                    lines.push(sentence(&format!(
                        "Finding {}: {}",
                        finding.id,
                        one_line(&finding.text, DECIDED)
                    )));
                }
            }
        }
        None if phase.state == PhaseState::Partial => {
            lines.push("Whole-phase checks never ran.".into());
        }
        None => {}
    }
    for response in &phase.responses {
        lines.push(sentence(&format!(
            "Lead's answer: {}",
            one_line(response, DECIDED)
        )));
    }
    lines
}

fn status_word(status: CriterionStatus) -> &'static str {
    match status {
        CriterionStatus::Met => "met",
        CriterionStatus::NotMet => "not met",
        CriterionStatus::NotRun => "not checked",
        CriterionStatus::Blocked => "needs you",
    }
}

/// The decisions taken for the run: its own, its tasks', and those of its requests.
fn run_decisions<'a>(
    run: &'a OvernightRun,
    board: &'a Board,
) -> impl Iterator<Item = &'a Decision> {
    let prefix = format!("run-{}-", run.id.short());
    board.decisions.iter().filter(move |decision| {
        let ours = match &decision.source {
            DecisionSource::Run { run_id, .. } => return run_id == &run.id,
            DecisionSource::Task { task_id } => board
                .tasks
                .get(task_id)
                .and_then(|task| task.run.as_ref())
                .is_some_and(|context| context.run_id == run.id),
            _ => false,
        };
        ours || decision
            .request_id
            .as_deref()
            .is_some_and(|request| request.starts_with(&prefix))
    })
}

fn decided_task<'a>(decision: &Decision, board: &'a Board) -> Option<&'a Task> {
    match &decision.source {
        DecisionSource::Task { task_id } => board.tasks.get(task_id),
        _ => None,
    }
}

/// What was decided for the user, one short line each. A phase's outcome is its line already,
/// held changes are under what got in the way, and a landing on the run branch is its commit;
/// declined permissions are counted on one line.
fn decision_lines(run: &OvernightRun, board: &Board) -> Vec<String> {
    let segments = chain(run, board);
    let mut lines = Vec::new();
    let mut declined: (usize, Vec<u32>) = (0, Vec::new());
    for decision in run_decisions(run, board) {
        let what = decision.what.trim();
        let task = decided_task(decision, board);
        if decision.kind == DecisionKind::PhaseOutcome || what.starts_with("Held ") {
            continue;
        }
        if what.starts_with("Declined ") {
            declined.0 += 1;
            if let Some(task) = task
                && !declined.1.contains(&task.number)
            {
                declined.1.push(task.number);
            }
            continue;
        }
        if what.starts_with("Landed ") && !what.ends_with("on the user's word") {
            let on_branch = task.is_some_and(|task| {
                task.run
                    .as_ref()
                    .is_some_and(|context| segments.contains(&&context.run_id))
            });
            if !on_branch {
                lines.push(sentence(&without_titles(what)));
            }
            continue;
        }
        lines.push(sentence(&one_line(
            &short_words(&decision.what, &decision.why).what,
            DECIDED,
        )));
    }
    if declined.0 > 0 {
        declined.1.sort_unstable();
        lines.push(format!(
            "Declined {} outside a task's sandbox{}.",
            plural(declined.0, "permission request", "permission requests"),
            if declined.1.is_empty() {
                String::new()
            } else {
                format!(" ({})", task_list(&declined.1))
            }
        ));
    }
    lines
}

/// A line without its quoted titles: "Landed task-41 on `b`" from "Landed task-41 “Fix it” on
/// `b`".
fn without_titles(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(open) = rest.find('\u{201c}') {
        let Some(close) = rest[open..].find('\u{201d}') else {
            break;
        };
        out.push_str(rest[..open].trim_end());
        rest = &rest[open + close + '\u{201d}'.len_utf8()..];
    }
    out.push_str(rest);
    out
}

/// Whether a task of a role and kind is in a group of who worked.
type Belongs = fn(RunRole, TaskKind) -> bool;

/// Who did the work: per kind of work, how many and on which models.
fn worker_lines(run: &OvernightRun, board: &Board) -> Vec<String> {
    let groups: [(&str, Belongs); 8] = [
        ("Workers", |role, kind| {
            role == RunRole::Worker && kind.writes()
        }),
        ("Scouts and research", |role, kind| {
            role == RunRole::Worker && matches!(kind, TaskKind::Scout | TaskKind::Research)
        }),
        ("Plan reviews", |role, kind| {
            role == RunRole::Worker && kind == TaskKind::Review
        }),
        ("Task reviews", |role, kind| {
            role == RunRole::Check && kind == TaskKind::Review
        }),
        ("Task verifications", |role, kind| {
            (role == RunRole::Check || role == RunRole::Worker) && kind == TaskKind::Verify
        }),
        ("Phase verifiers", |role, _| role == RunRole::PhaseVerifier),
        ("Phase reviewers", |role, _| role == RunRole::PhaseReviewer),
        ("Judges", |role, _| role == RunRole::Judge),
    ];
    let mut tasks: Vec<(&Task, RunRole)> = run_tasks(run, board)
        .filter_map(|task| task.run.as_ref().map(|context| (task, context.role)))
        .collect();
    tasks.sort_by_key(|(task, _)| task.number);
    groups
        .iter()
        .filter_map(|(label, belongs)| {
            let members: Vec<&Task> = tasks
                .iter()
                .filter(|(task, role)| belongs(*role, task.kind))
                .map(|(task, _)| *task)
                .collect();
            (!members.is_empty())
                .then(|| format!("{label}: {}, {}.", members.len(), models(&members)))
        })
        .collect()
}

/// "Claude opus", or "Codex gpt-6-astra 10, gpt-6.1-sol 6": the models, most used first.
fn models(tasks: &[&Task]) -> String {
    let mut counted: Vec<(ProviderKind, String, usize)> = Vec::new();
    for task in tasks {
        let provider = task.route.choice.provider;
        let model = task
            .route
            .choice
            .model
            .clone()
            .unwrap_or_else(|| "default".into());
        match counted
            .iter_mut()
            .find(|(p, m, _)| *p == provider && *m == model)
        {
            Some(entry) => entry.2 += 1,
            None => counted.push((provider, model, 1)),
        }
    }
    let first = counted_first(tasks);
    counted.sort_by_key(|(provider, _, count)| (*provider != first, std::cmp::Reverse(*count)));
    if let [(provider, model, _)] = counted.as_slice() {
        return format!("{} {model}", provider_name(*provider));
    }
    let mut parts = Vec::new();
    let mut last: Option<ProviderKind> = None;
    for (provider, model, count) in counted {
        if last == Some(provider) {
            parts.push(format!("{model} {count}"));
        } else {
            parts.push(format!("{} {model} {count}", provider_name(provider)));
        }
        last = Some(provider);
    }
    parts.join(", ")
}

/// The provider of the first of `tasks`, whose models are listed first.
fn counted_first(tasks: &[&Task]) -> ProviderKind {
    tasks
        .first()
        .map_or(ProviderKind::Claude, |task| task.route.choice.provider)
}

/// A provider in one word.
fn provider_name(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Claude => "Claude",
        ProviderKind::Codex => "Codex",
    }
}

/// "Usage: Claude 26.6M · Codex 55.7M tokens."
fn usage_line(usage: &Usage) -> String {
    let known: Vec<String> = usage
        .iter()
        .filter_map(|(provider, tokens)| {
            tokens
                .filter(|tokens| *tokens > 0)
                .map(|tokens| format!("{} {}", provider_name(*provider), human_count(tokens)))
        })
        .collect();
    if !known.is_empty() {
        return format!("Usage: {} tokens.", known.join(" · "));
    }
    if usage.iter().any(|(_, tokens)| tokens.is_some()) {
        "Usage: none recorded.".into()
    } else {
        "Usage: its records couldn't be read.".into()
    }
}

/// What Brigadier ignored in the user's words, as the current reading of them has it: a rule
/// Brigadier keeps anyway ("never Fable") isn't listed, even in a run recorded before it read
/// it so.
fn ignored(run: &OvernightRun) -> Vec<String> {
    let current = parse(&run.words, &Clock::system()).directives.ignored;
    let words = run.words.to_lowercase();
    run.directives
        .ignored
        .iter()
        .filter(|line| {
            let what = line
                .strip_prefix("Ignored: ")
                .and_then(|rest| rest.split_once(" (").map(|(what, _)| what))
                .unwrap_or(line);
            // Read from the words the run started with: listed only if still read so. From
            // words given later: kept.
            current.contains(line) || !words.contains(&what.to_lowercase())
        })
        .map(|line| {
            line.strip_prefix("Ignored: ")
                .and_then(|rest| rest.split_once(" (").map(|(what, _)| what))
                .unwrap_or(line)
                .to_owned()
        })
        .collect()
}

/// A count of tokens as people read it: 950, 12.3k, 26.6M, 1.2B.
fn human_count(count: u64) -> String {
    let scaled = |div: f64, unit: &str| {
        let value = count as f64 / div;
        let text = format!("{value:.1}");
        format!("{}{unit}", text.trim_end_matches(".0"))
    };
    match count {
        0..1_000 => count.to_string(),
        1_000..1_000_000 => scaled(1e3, "k"),
        1_000_000..1_000_000_000 => scaled(1e6, "M"),
        _ => scaled(1e9, "B"),
    }
}

/// A checker's "[met] p1-c1: evidence" line without its status and id, which the report
/// already shows; any other text as it is.
pub(super) fn evidence_text(line: &str) -> &str {
    let line = line.trim();
    if without_marker(line).starts_with('[') {
        criterion_evidence(line).unwrap_or(line)
    } else {
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ConversationId;
    use crate::overnight::{ObstacleKind, OvernightState};

    /// The night of 2026-10-03 as its records were (the app's fixture of it), with the risk
    /// lines two of its verifiers wrote (the fixture keeps no reports).
    fn night() -> (OvernightRun, Board) {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../apps/desktop/src/fixtures/boards/overnight-2026-10-03.json"
        ))
        .expect("the fixture");
        let mut board = Board::default();
        for (_, task) in fixture["tasks"].as_object().expect("tasks") {
            let mut task = task.clone();
            task["report"] = serde_json::Value::Null;
            let task: Task = serde_json::from_value(task).expect("a task");
            board.tasks.insert(task.id.clone(), task);
        }
        for decision in fixture["decisions"].as_array().expect("decisions") {
            board
                .decisions
                .push(serde_json::from_value(decision.clone()).expect("a decision"));
        }
        for (id, item) in fixture["waiting"].as_object().expect("waiting") {
            board.waiting.insert(
                id.clone(),
                serde_json::from_value(item.clone()).expect("an item"),
            );
        }
        let mut run: OvernightRun = serde_json::from_value(
            fixture["overnight"]
                .as_object()
                .and_then(|runs| runs.values().next())
                .cloned()
                .expect("the run"),
        )
        .expect("a run");
        // The fixture holds the report as rendered again (for the app's page); the run wrote
        // the first shape of it.
        run.report_version = 0;
        run.report_text = None;
        board.runs.insert(run.id.clone(), run.clone());
        let risks = |number: u32, risks: &[&str]| {
            let task = board
                .tasks
                .values()
                .find(|task| task.number == number)
                .expect("the verifier")
                .id
                .clone();
            (
                task,
                risks
                    .iter()
                    .map(|line| (*line).to_owned())
                    .collect::<Vec<_>>(),
            )
        };
        for (task, risks) in [
            risks(
                4,
                &[
                    "[pre-existing] Provider doctests: parent reproduced 11 failures",
                    "[pre-existing] Dependency installation: ERR_PNPM_STORE_DIR_OPEN_OPERATION_LOCK",
                    "[not run] Full app smoke: launching it violates the no-daemon instruction",
                ],
            ),
            risks(
                16,
                &[
                    "[pre-existing] Provider doctests: the same 11 failures",
                    "[not run] Desktop --smoke: the task says start no daemon",
                ],
            ),
        ] {
            board.tasks.get_mut(&task).expect("the verifier").report = Some(
                serde_json::from_value(serde_json::json!({
                    "summary": "", "changes": [], "decisions": [], "verification": [],
                    "openQuestions": [], "risks": risks, "verdict": null, "artifacts": [],
                    "submittedAtMs": 0
                }))
                .expect("a report"),
            );
        }
        (run, board)
    }

    fn usage() -> Usage {
        vec![
            (ProviderKind::Claude, Some(26_570_557)),
            (ProviderKind::Codex, Some(55_667_191)),
        ]
    }

    #[test]
    fn the_night_of_october_3_reads_in_twenty_seconds() {
        let (mut run, board) = night();
        // The fixture cuts the user's words short; these are theirs.
        run.words
            .push_str(" Never Fable; effort no higher than high.");
        let commits = landed_commits(&run, &board);
        let text = render(&run, &board, &commits, &usage(), 0);
        // `cargo test … -- --nocapture` shows it whole.
        println!("{text}");
        let (shown, details) = text.split_once(DETAILS).expect("details");
        let at = clock_time(run.finished_at_ms.expect("finished"));
        assert_eq!(
            shown,
            format!(
                "**Faster, leaner overnight runs**: stopped by you at {at}. 1 of 3 phases verified.

Merge takes phase 1 (`1dcda64`). 2 later commits stay unverified on the branch.

1 thing waits on you.

- ✓ Phase 1 · Measure: verified. 1 task landed.
- ◐ Phase 2 · Fix: partial, you stopped the run. 2 tasks landed, 1 didn't.
- Phase 3 · Re-measure: not reached.

Waiting on you:
- Bring the phase 2 note commit daf8ac1f4b from brigadier/4158464b/session onto overnight/2026-10-03-faster-leaner-overnight-runs-09cc7d53. It adds the whole note, so on the run branch keep its version of docs/evidence/2026-10-03-overnight-ab-breakdown.md; that version only appends a section.
Then say “continue” to pick the run up on the same branch.

Got in the way:
- Held because their change couldn't be verified: “Make cause 1's fix in the A/B note obey…”, “Router: quota forecast penalty starts…”.
- Not run: Full app smoke and Desktop --smoke (2 tasks).
- Already failing before the run: Provider doctests and Dependency installation (2 tasks).
- Stopped before it finished: “Gate members take run slots in a fixed…”.

"
            )
        );
        // The run card's three lines are the report's first three.
        assert_eq!(
            outcome_of(&text).expect("three lines")[1],
            "Merge takes phase 1 (`1dcda64`). 2 later commits stay unverified on the branch."
        );
        for line in [
            "- `d1453e7` Read 'path: description' report entries as the path when landing (unverified)\n",
            "- `bdd536d` Start the quota forecast penalty at 100% for check work (unverified)\n",
            "- `1dcda64` docs: break down the overnight A/B run by role\n",
            "- Didn't land “Break down the overnight A/B time and…”: problems left after 1 fix round.\n",
            "- Sent “Fix the A/B breakdown note after review” back after review (fix 1 of 2).\n",
            "- Sent the plan “Phase 2 · Fix the three ranked causes from the A/B breakdown” back after review.\n",
            "- Landed “Findings note: record what phase 2…” on `brigadier/4158464b/session`.\n",
            "- Rejected cause 3 (gate members take run slots in priority order) instead of retrying it tonight; “Gate members take run slots in a fixed…” did not land.\n",
            "- p1-c1 met, checked by “Judge phase 1”: docs/evidence/2026-10-03-overnight-ab-breakdown.md has the per-role table",
            "- Whole-phase checks passed in round 1 on `1dcda64`.\n",
            "- p2-c1 not checked: you stopped the run.\n",
            "- Workers: 6, Claude opus.\n",
            "- Task reviews: 16, Codex gpt-6-astra 10, gpt-6.1-sol 6.\n",
            "- Task verifications: 13, Codex gpt-6.1-sol 10, gpt-6-sol 3.\n",
            "- Judges: 1, Claude opus.\n",
            "Usage: Claude 26.6M · Codex 55.7M tokens.\n",
        ] {
            assert!(details.contains(line), "{line}\n{details}");
        }
        // No reviewer's words, no criterion echoed back, and "never Fable" is kept, not ignored.
        for gone in [
            "P2 —",
            "From the review",
            "(met):",
            "Ignored",
            "Landed “Make cause",
        ] {
            assert!(!text.contains(gone), "{gone}\n{text}");
        }
        // Workers by the names the app shows them by, never "task-N" (branches keep theirs).
        assert_no_task_numbers(&text);
        assert!(shown.lines().count() <= 20, "{shown}");
    }

    /// No "task-N" outside a branch or path name.
    fn assert_no_task_numbers(text: &str) {
        let mut rest = text;
        while let Some(at) = rest.find("task-") {
            let before = rest[..at].chars().next_back();
            let digit = rest[at + 5..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit());
            assert!(
                !digit || before.is_some_and(|c| c == '/' || c.is_alphanumeric()),
                "a worker named by its number: {}\n{text}",
                &rest[at.saturating_sub(40)..(at + 12).min(rest.len())]
            );
            rest = &rest[at + 5..];
        }
    }

    #[test]
    fn an_old_report_is_rendered_again_only_when_its_records_add_up() {
        let (run, board) = night();
        assert_eq!(run.report_version, 0, "the fixture is an old report");
        let commits = landed_commits(&run, &board);
        let text = rebuilt(&run, &board, &commits, &usage()).expect("rendered again");
        assert!(
            text.starts_with("**Faster, leaner overnight runs**: stopped by you at "),
            "{text}"
        );
        assert_eq!(
            outcome_of(&text).expect("three lines")[2],
            "1 thing waits on you."
        );
        // A branch Continue moved on, or one that is gone: the records still say what was on it.
        let mut later = board.clone();
        for task in later.tasks.values_mut() {
            if task.number == 32 {
                task.landed = None;
            }
        }
        assert_eq!(landed_commits(&run, &later).len(), 2);
        // Records that don't reach the verified tip aren't the run's: the report stays.
        for task in later.tasks.values_mut() {
            if task.number == 13 {
                task.landed = None;
            }
        }
        assert!(rebuilt(&run, &later, &landed_commits(&run, &later), &usage()).is_none());
    }

    #[test]
    fn a_continued_segment_lists_only_its_branch_up_to_its_end() {
        let (first, mut board) = night();
        let mut second = first.clone();
        second.id = OvernightRunId::generate();
        second.predecessor = Some(first.id.clone());
        second.segment = 2;
        board.runs.insert(second.id.clone(), second.clone());
        // A task of the second segment landed after the first segment's report.
        let mut task = board
            .tasks
            .values()
            .find(|task| task.number == 32)
            .cloned()
            .expect("task-32");
        task.id = crate::model::TaskId("t-later".into());
        task.number = 50;
        task.landed = Some("feedface".into());
        task.updated_at_ms = i64::MAX;
        if let Some(context) = task.run.as_mut() {
            context.run_id = second.id.clone();
        }
        board.tasks.insert(task.id.clone(), task);
        let commits = landed_commits(&first, &board);
        assert_eq!(commits.len(), 3, "{commits:?}");
        assert!(commits.iter().all(|commit| commit.commit != "feedface"));
        // The second segment's branch holds both segments' commits.
        assert_eq!(landed_commits(&second, &board)[0].commit, "feedface");
    }

    #[test]
    fn decisions_read_short_in_old_and_new_words() {
        for (what, why, short) in [
            (
                "Sent task-32 back to fix what its checks found (fix 1 of 2)",
                "From the review (task-33, Codex gpt-6-astra): P2 — landing.rs:1483: …",
                "Sent task-32 back after review (fix 1 of 2)",
            ),
            (
                "Sent task-8 back to fix what its checks found (fix 2 of 2)",
                "From the review (task-9): … From the verification (task-10): …",
                "Sent task-8 back after review and verification (fix 2 of 2)",
            ),
            (
                "Did not land task-5: the orchestrator decides what happens next",
                "The problems were still there after 2 fix rounds. From the verification …",
                "Didn't land task-5: problems left after 2 fix rounds",
            ),
            (
                "Did not land task-3: the orchestrator decides what happens next",
                "Its change is the same one its checks found these problems in. …",
                "Didn't land task-3: the fix changed nothing",
            ),
            (
                "Did not land task-4: its checks could not finish",
                "- it failed",
                "Didn't land task-4: its checks couldn't finish",
            ),
            (
                "Sent the plan “Fix it” back for revision",
                "Its independent review asked for changes. F1: …",
                "Sent the plan “Fix it” back after review",
            ),
            (
                "Sent task-6 back: 2 review findings (fix 1 of 2)",
                "",
                "Sent task-6 back: 2 review findings (fix 1 of 2)",
            ),
        ] {
            assert_eq!(short_words(what, why).what, short);
        }
        assert_eq!(
            without_titles("Landed task-41 “Record phase 2” on `brigadier/x/session`"),
            "Landed task-41 on `brigadier/x/session`"
        );
    }

    #[test]
    fn a_phase_its_checks_settled_says_its_own_gap_not_the_runs_stop() {
        let (mut run, board) = night();
        let cut_off = |run: &OvernightRun| {
            phase_lines(run, &board)
                .into_iter()
                .find(|line| line.contains("Phase 2"))
                .expect("phase 2's line")
        };
        assert!(
            cut_off(&run).contains("you stopped the run"),
            "{}",
            cut_off(&run)
        );
        // Settled partial by its own checks before the Stop, the run going on to another phase.
        run.phases[1].gaps = vec!["No reviewer of another vendor was free.".into()];
        let line = cut_off(&run);
        assert!(
            line.contains("No reviewer of another vendor was free"),
            "{line}"
        );
        assert!(!line.contains("stopped the run"), "{line}");
    }

    #[test]
    fn a_rule_brigadier_keeps_is_not_listed_as_ignored() {
        let mut run = OvernightRun::for_test(ConversationId("c".into()), "Speed", Vec::new());
        run.words = "/overnight Speed. Never Fable; use ultracode.".into();
        run.directives.ignored = vec![
            "Ignored: Fable (Brigadier picks this itself)".into(),
            "Ignored: ultracode (Brigadier picks this itself)".into(),
            "Ignored: effort max (Brigadier picks this itself)".into(),
        ];
        // Fable: the run's words rule it out, which the reading now keeps as a rule. ultracode:
        // still ignored. Effort max: from words given later, kept as recorded.
        assert_eq!(ignored(&run), ["ultracode", "effort max"]);
    }

    #[test]
    fn the_report_says_what_got_in_the_way_and_folds_the_details() {
        let mut phases = vec![
            OvernightPhase::new(1, "Measure", "", &["It is measured.".into()], &[]),
            OvernightPhase::new(2, "Re-measure", "", &["Again.".into()], &[]),
        ];
        phases[0].state = PhaseState::Verified;
        let mut run = OvernightRun::for_test(ConversationId("c".into()), "Speed", phases);
        run.state = OvernightState::Finished;
        run.stop = Some(StopReason::Deadline);
        for number in [3, 5, 3] {
            run.note_obstacle(
                ObstacleKind::Declined,
                "`git push` was declined by the overnight rules: it acts outside this machine",
                Some(number),
                1,
            );
        }
        let text = render(&run, &Board::default(), &[], &usage(), 0);
        let (shown, details) = text.split_once(DETAILS).expect("details");
        assert!(
            shown.contains("- Phase 2 · Re-measure: not reached.\n"),
            "{text}"
        );
        assert!(
            shown.contains("\nGot in the way: `git push` was declined by the overnight rules: it acts outside this machine (3 times, task-3, task-5).\n"),
            "{text}"
        );
        assert!(!shown.contains("Not reached"), "{text}");
        assert!(details.contains("- p1-c1 not checked.\n"), "{text}");
        assert!(
            shown.starts_with("**Speed**: stopped at the deadline at "),
            "{text}"
        );
        assert!(
            shown.contains("\n\nNo branch was made.\n\nNothing waits on you.\n\n"),
            "{text}"
        );
    }

    #[test]
    fn numbers_read_as_people_say_them() {
        assert_eq!(human_count(950), "950");
        assert_eq!(human_count(12_340), "12.3k");
        assert_eq!(human_count(26_570_557), "26.6M");
        assert_eq!(human_count(3_000_000), "3M");
        assert_eq!(human_count(1_234_000_000), "1.2B");
        assert_eq!(
            usage_line(&vec![
                (ProviderKind::Claude, None),
                (ProviderKind::Codex, Some(0))
            ]),
            "Usage: none recorded."
        );
    }
}
