//! The morning report (PLAN.md §10.11): one message at the end of the run, rendered from the
//! run's records, never from a model's memory of it, in the sections a delegated run reports
//! with. First come three lines, in the run card's words: the outcome, what Merge takes and
//! what waits on the user. Then each phase of the run's plan as the thread settled it, the
//! commits with their reviews (a review still running says so), what was decided for the
//! user, what waits on them, problems and risks. Everything after the `### Details` heading
//! the app shows folded: how each phase was checked (the thread's settlement, its workers'
//! reports and what each review found), the branch, who did the work on which model (the
//! worker lineage), and the usage.
//!
//! It is written once: its message has a stable id per run segment, looked for before it is
//! appended, and the run records it after. A crash in between finds the message on recovery
//! and only records it. The notification the report comes with is queued on the run at the
//! same time; the app delivers it as Brigadier and acknowledges it.

use super::super::decisions::{named, short_words, waiting_run};
use super::super::{SessionManager, blocking, git_error};
use super::directives::{Clock, parse};
use super::wind_down::to_you;
use crate::board::Board;
use crate::model::{DomainEvent, OvernightRunId, Setup};
use crate::now_ms;
use crate::overnight::{Deadline, OvernightRun, RunNotification, RunRole, StopReason};
use crate::routing::TurnUsage;
use crate::sessions::one_line;
use crate::work::{
    Decision, DecisionKind, DecisionSource, PhaseStage, RequestState, ReviewKind, ReviewRun,
    ReviewState, StepOutcome, StepSettlement, Task, TaskKind, TaskState, UserRequest, WorkerRole,
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
/// A verifier's "done when" lines listed per phase, at most (its report has them all).
const DONE_WHEN: usize = 8;
/// Lines of how a phase was checked, what is left of it, and review findings, listed per
/// phase, at most.
const HOW: usize = 4;
/// Risks a phase's verifier named, listed per phase under problems and risks, at most.
const RISKS: usize = 3;
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
    /// Writes the run's report into its conversation, once, and queues its notification (one
    /// that doesn't `notify` is recorded as delivered: the user is here, archiving it).
    pub(crate) async fn write_run_report(&self, run: &OvernightRun, notify: bool) {
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
        // Summaries are shortened by render, so replace quoted image tokens first.
        let mut shown_board = board.clone();
        for task in shown_board.tasks.values_mut() {
            if let Some(report) = &mut task.report {
                report.summary = self.core.display_quote(id, &report.summary).await;
            }
        }
        for plan in shown_board.plans.values_mut() {
            for step in &mut plan.steps {
                if let Some(settled) = &mut step.settled {
                    settled.summary = self.core.display_quote(id, &settled.summary).await;
                }
            }
        }
        let mut text = self
            .core
            .display_quote(id, &render(run, &shown_board, &commits, &usage, now))
            .await;
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
                            worked: Vec::new(),
                            quota_wait: false,
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
            delivered_at_ms: (!notify).then(now_ms),
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
        now.end_commit = tip;
        if now.notification.is_none() {
            now.notification = Some(notification);
        }
        if let Err(err) = self.record_run(&now).await {
            tracing::warn!(run = %run.id, error = %err, "could not record the report/outbox; will reconcile it");
        }
    }

    /// Recorded tokens per provider for this segment's tasks, its thread and the reviews of its
    /// commits. Only their turns between its start and `end` are counted.
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
                    .filter(|turn| turn.at_ms <= end && counts_for(turn, run, board))
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
                // Also reconcile terminal setup failures and a report cut off by persistence.
                if saved.state != crate::overnight::OvernightState::Finished {
                    continue;
                }
                if saved.report_message_id.is_none() {
                    let notify = conversation.lifecycle != crate::model::Lifecycle::Archived;
                    self.write_run_report(saved, notify).await;
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

/// Whether a turn is the run's: one of its tasks', or its session's own (the thread's, and a
/// review of a commit, counted under `review:<id>`).
fn counts_for(turn: &TurnUsage, run: &OvernightRun, board: &Board) -> bool {
    let session = turn.conversation_id.as_deref() == Some(run.conversation_id.0.as_str());
    match turn.task_id.as_deref() {
        Some(task_id) if task_id.starts_with("review:") => session,
        Some(task_id) => board.tasks.values().any(|task| {
            task.id.0 == task_id
                && task
                    .run
                    .as_ref()
                    .is_some_and(|context| context.run_id == run.id)
        }),
        None => session,
    }
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

/// How a phase of the run's plan stands in the report and on the run card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    Done,
    Partial,
    Blocked,
    Skipped,
    NotReached,
    Unfinished,
}

/// A phase of the run's plan as the report reads it.
struct Step<'a> {
    number: u32,
    name: &'a str,
    mark: Mark,
    settled: Option<&'a StepSettlement>,
}

/// The run's phases: its plan's steps, each by its source number and as the thread settled
/// it; else (no plan was recorded) the phases it was started with, none of them reached.
fn steps<'a>(run: &'a OvernightRun, board: &'a Board) -> Vec<Step<'a>> {
    if let Some(plan) = super::run::run_plan(board, run) {
        return plan
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| Step {
                number: step.number_at(index),
                name: &step.title,
                mark: match (&step.settled, step.stage) {
                    (Some(settled), _) => match settled.outcome {
                        StepOutcome::Done => Mark::Done,
                        StepOutcome::Partial => Mark::Partial,
                        StepOutcome::Blocked => Mark::Blocked,
                    },
                    (None, PhaseStage::Skipped) => Mark::Skipped,
                    (None, PhaseStage::Pending) => Mark::NotReached,
                    (None, _) => Mark::Unfinished,
                },
                settled: step.settled.as_ref(),
            })
            .collect();
    }
    run.phases
        .iter()
        .map(|phase| Step {
            number: phase.number,
            name: &phase.name,
            mark: if run.selects(phase.number) {
                Mark::NotReached
            } else {
                Mark::Skipped
            },
            settled: None,
        })
        .collect()
}

/// The phases worked on (not left out), and those settled done.
fn counts(steps: &[Step]) -> (usize, usize) {
    let worked = steps
        .iter()
        .filter(|step| step.mark != Mark::Skipped)
        .count();
    let done = steps.iter().filter(|step| step.mark == Mark::Done).count();
    (worked, done)
}

/// The phases Merge takes: those settled done, in order, up to the first that isn't (a phase
/// left out doesn't count either way).
fn accepted(steps: &[Step]) -> Vec<u32> {
    steps
        .iter()
        .filter(|step| step.mark != Mark::Skipped)
        .take_while(|step| step.mark == Mark::Done)
        .map(|step| step.number)
        .collect()
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
        Some(StopReason::Blocked { phase_id }) => match phase_id.strip_prefix("phase-") {
            Some(number) => format!("stopped early{at}: phase {number} needs you"),
            None => format!("stopped early{at}: what is left needs you"),
        },
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
    let (worked, done) = counts(&steps(run, board));
    let waits = waiting(run, board).len();
    let mut body = format!("{done} of {worked} phases done");
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
    let steps = steps(run, board);
    let mut text = String::new();
    // The outcome in three lines, which the run card shows too.
    text.push_str(&outcome_line(run, &steps, now));
    text.push_str("\n\n");
    text.push_str(&merge_line(run, &steps, commits));
    text.push_str("\n\n");
    text.push_str(&match waits.len() {
        0 => "Nothing waits on you.".to_owned(),
        1 => "1 thing waits on you.".to_owned(),
        n => format!("{n} things wait on you."),
    });
    if !steps.is_empty() {
        text.push_str("\n\n### Phases\n");
        for line in phase_lines(run, board, &steps) {
            text.push_str(&format!("- {line}\n"));
        }
    }
    if !commits.is_empty() {
        text.push_str("\n### Commits\n");
        let mut accepted = false;
        for commit in commits {
            if Some(&commit.commit) == run.verified_commit.as_ref() {
                accepted = true;
            }
            text.push_str(&format!(
                "- `{}` {}{}{}\n",
                short(&commit.commit),
                commit.subject,
                review_of(&commit.commit, board),
                if accepted { "" } else { " (not in the merge)" }
            ));
        }
        if commits.len() == COMMITS {
            text.push_str("Only the latest 60 are listed; the branch has them all.\n");
        }
    }
    let decided = decision_lines(run, board);
    if !decided.is_empty() {
        text.push_str("\n### Decided for you\n");
        for line in decided {
            text.push_str(&format!("- {line}\n"));
        }
    }
    if !waits.is_empty() {
        text.push_str("\n### Waiting on you\n");
        for item in &waits {
            text.push_str(&format!("- {}\n", item.trim()));
        }
        text.push_str("Then say \u{201c}continue\u{201d} to pick the run up on the same branch.\n");
    }
    let problems = problems(run, board, &steps, now);
    if !problems.is_empty() {
        text.push_str("\n### Problems and risks\n");
        for line in problems {
            text.push_str(&format!("- {line}\n"));
        }
    }
    // The details, folded by the app: everything after this heading.
    text.push_str(&format!("\n{DETAILS}\n"));
    text.push_str(&details(run, board, &steps, usage));
    // Workers by the names the app shows, also in text written before they were; the run's
    // words about the user said to the user.
    named(&to_you(&text), board)
}

/// " · review: no findings": the newest review of the change that ends at `commit`.
fn review_of(commit: &str, board: &Board) -> String {
    let Some(review) = board
        .reviews
        .values()
        .filter(|review| review.kind == ReviewKind::Code && review.tip == commit)
        .max_by_key(|review| review.started_at_ms)
    else {
        return String::new();
    };
    match &review.state {
        ReviewState::Running => " \u{b7} review still running".into(),
        ReviewState::Clean => " \u{b7} review: no findings".into(),
        ReviewState::Findings { count } => format!(
            " \u{b7} review: {}",
            plural(*count as usize, "finding", "findings")
        ),
        ReviewState::Failed { .. } => " \u{b7} review couldn't run".into(),
    }
}

/// "**Name**: stopped by you at 07:04. 1 of 3 phases done."
fn outcome_line(run: &OvernightRun, steps: &[Step], now: i64) -> String {
    let ending = ending(run, now);
    if matches!(run.stop, Some(StopReason::Failed { .. })) {
        return format!("**{}**: {ending}.", run.name);
    }
    let (worked, done) = counts(steps);
    if worked == 0 {
        return format!("**{}**: {ending}. No phases were planned.", run.name);
    }
    format!(
        "**{}**: {ending}. {done} of {} done.",
        run.name,
        plural(worked, "phase", "phases")
    )
}

/// What Merge takes (the accepted tip: the branch when the thread settled the last of the
/// phases done in a row), and what stays on the branch past it.
fn merge_line(run: &OvernightRun, steps: &[Step], commits: &[RunCommit]) -> String {
    let Some(workspace) = &run.workspace else {
        return "No branch was made.".into();
    };
    if run.merged.is_some() {
        return format!("Merged into `{}`.", workspace.base);
    }
    let stay = |count: usize, later: &str| match count {
        0 => String::new(),
        1 => format!(" 1 {later}commit isn't in the merge."),
        n => format!(" {n} {later}commits aren't in the merge."),
    };
    let numbers: Vec<String> = accepted(steps).iter().map(u32::to_string).collect();
    let phases = match numbers.len() {
        0 => None,
        1 => Some(format!("phase {}", numbers[0])),
        _ => Some(format!("phases {}", and_list(&numbers))),
    };
    match (&run.verified_commit, phases) {
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
        _ => format!(
            "Nothing settled done to merge yet.{}",
            stay(commits.len(), "")
        ),
    }
}

/// A phase's mark and name, as the run card and the thread's phase headers show them once the
/// run is over: "✓ Phase 1 · Measure", "Phase 3 · Re-measure" for one not reached.
fn phase_head(step: &Step) -> String {
    let mark = match step.mark {
        Mark::Done => "✓ ",
        Mark::Partial | Mark::Unfinished => "◐ ",
        Mark::Blocked => "✕ ",
        Mark::Skipped => "– ",
        Mark::NotReached => "",
    };
    format!("{mark}Phase {} · {}", step.number, step.name)
}

/// A phase's state in a word or two once the run is over, the run card's and the thread's
/// word for it (`phaseWord` in the app).
fn phase_word(mark: Mark) -> &'static str {
    match mark {
        Mark::NotReached => "not reached",
        Mark::Unfinished => "unfinished",
        Mark::Done => "done",
        Mark::Partial => "partial",
        Mark::Blocked => "blocked",
        Mark::Skipped => "skipped",
    }
}

/// One line per phase: its state in the run card's word, why when it isn't done, and what
/// landed.
fn phase_lines(run: &OvernightRun, board: &Board, steps: &[Step]) -> Vec<String> {
    let segments = chain(run, board);
    steps
        .iter()
        .map(|step| {
            let head = phase_head(step);
            let word = phase_word(step.mark);
            let (landed, not_landed) = landings(step.number, board, &segments);
            let tasks = |done: bool| match (landed, not_landed) {
                (0, 0) => String::new(),
                (0, n) if !done => {
                    format!(" {} land.", plural(n, "task didn't", "tasks didn't"))
                }
                (n, m) if m > 0 && !done => {
                    format!(" {} landed, {m} didn't.", plural(n, "task", "tasks"))
                }
                (0, _) => String::new(),
                (n, _) => format!(" {} landed.", plural(n, "task", "tasks")),
            };
            match step.mark {
                Mark::Done => format!("{head}: {word}.{}", tasks(true)),
                Mark::Partial | Mark::Unfinished => format!(
                    "{head}: {word}, {}.{}",
                    unfinished_why(run, step),
                    tasks(false)
                ),
                Mark::Blocked => format!(
                    "{head}: {word}, it needs you: {}",
                    sentence(&one_line(
                        step.settled
                            .and_then(|settled| settled.left.first())
                            .map_or("see Waiting on you", String::as_str),
                        MISSING
                    ))
                ),
                Mark::Skipped | Mark::NotReached => format!("{head}: {word}."),
            }
        })
        .collect()
}

/// The tasks of phase `number` in the run and the segments it continues, in the order they
/// were made.
fn phase_tasks<'a>(number: u32, board: &'a Board, segments: &[&OvernightRunId]) -> Vec<&'a Task> {
    let mut tasks: Vec<&Task> = board
        .tasks
        .values()
        .filter(|task| {
            task.phase == Some(number)
                && task
                    .run
                    .as_ref()
                    .is_some_and(|context| segments.contains(&&context.run_id))
        })
        .collect();
    tasks.sort_by_key(|task| task.number);
    tasks
}

/// How a phase was checked, for the details: the thread's settlement (what it changed, how
/// each "done when" was checked, what is left), its verifier's or lead's report, what each
/// review found and what was done about it. One short line each.
fn phase_evidence(step: &Step, board: &Board, segments: &[&OvernightRunId]) -> Vec<String> {
    if matches!(step.mark, Mark::NotReached | Mark::Skipped) {
        return Vec::new();
    }
    let mut lines = Vec::new();
    if let Some(settled) = step.settled {
        lines.push(sentence(&format!(
            "Settled: {}",
            one_line(settled.summary.trim(), EVIDENCE).trim_end_matches('.')
        )));
        for gap in settled.left.iter().take(HOW) {
            lines.push(sentence(&format!(
                "Left: {}",
                one_line(gap, EVIDENCE).trim_end_matches('.')
            )));
        }
    }
    let tasks = phase_tasks(step.number, board, segments);
    let reported = |task: &&&Task| task.report.is_some();
    let verifier = tasks
        .iter()
        .rev()
        .filter(reported)
        .find(|task| task.role == Some(WorkerRole::Verifier));
    let lead = tasks.iter().rev().filter(reported).find(|task| {
        task.kind == TaskKind::Implement
            && matches!(
                task.role,
                None | Some(WorkerRole::Lead | WorkerRole::Parallel | WorkerRole::Fix)
            )
    });
    match (verifier, lead) {
        (Some(task), _) | (None, Some(task)) => {
            let report = task.report.as_ref().expect("reported");
            let who = if verifier.is_some() {
                format!("Verified by task-{}", task.number)
            } else {
                format!("task-{} checked its own work", task.number)
            };
            lines.push(sentence(&format!(
                "{who}: {}",
                one_line(report.summary.trim(), EVIDENCE).trim_end_matches('.')
            )));
            for line in report.done_when.iter().take(DONE_WHEN) {
                lines.push(sentence(&one_line(without_marker(line), EVIDENCE)));
            }
            if report.done_when.len() > DONE_WHEN {
                lines.push(format!(
                    "{} more \u{201c}done when\u{201d} lines in its report.",
                    report.done_when.len() - DONE_WHEN
                ));
            }
            for line in report.verification.iter().take(HOW) {
                lines.push(sentence(&format!("How: {}", one_line(line, EVIDENCE))));
            }
        }
        (None, None) if step.settled.is_none() => lines.push("Nothing checked it.".into()),
        (None, None) => {}
    }
    // The phase's one-shot reviews: an outline's, a worker's own and each landing's.
    let mut reviews: Vec<&ReviewRun> = board
        .reviews
        .values()
        .filter(|review| {
            review
                .task_id
                .as_ref()
                .is_some_and(|id| tasks.iter().any(|task| &task.id == id))
        })
        .collect();
    reviews.sort_by_key(|review| review.started_at_ms);
    for review in &reviews {
        let what = match review.kind {
            ReviewKind::Plan => "Outline review",
            ReviewKind::Code => "Code review",
        };
        lines.push(match &review.state {
            ReviewState::Running => format!("{what}: still running."),
            ReviewState::Clean => format!("{what}: no findings."),
            ReviewState::Findings { count } => sentence(&format!(
                "{what}: {}",
                plural(*count as usize, "finding", "findings")
            )),
            ReviewState::Failed { reason } => sentence(&format!(
                "{what} could not run: {}",
                one_line(reason, DECIDED).trim_end_matches('.')
            )),
        });
    }
    // What was done about the review: whoever asked for it fixed what it agreed with and said
    // so in its report.
    if let Some(task) = verifier.or(lead)
        && let Some(report) = &task.report
        && reviews.iter().any(|review| {
            review.task_id.as_ref() == Some(&task.id)
                && matches!(review.state, ReviewState::Findings { .. })
        })
    {
        let answers: Vec<String> = report
            .decisions
            .iter()
            .take(HOW)
            .map(|line| one_line(line, DECIDED).trim_end_matches('.').to_owned())
            .collect();
        lines.push(sentence(&format!(
            "Done about it: {}",
            if answers.is_empty() {
                one_line(report.summary.trim(), DECIDED)
                    .trim_end_matches('.')
                    .to_owned()
            } else {
                answers.join("; ")
            }
        )));
    }
    lines
}

/// Phase `number`'s write tasks that landed, and those that ended without landing.
fn landings(number: u32, board: &Board, segments: &[&OvernightRunId]) -> (usize, usize) {
    let (mut landed, mut not_landed) = (0, 0);
    for task in phase_tasks(number, board, segments)
        .into_iter()
        .filter(|task| task.kind.writes())
    {
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

/// Why a phase isn't done, in a few words: what the thread said is left of it, else how the
/// run ended.
fn unfinished_why(run: &OvernightRun, step: &Step) -> String {
    if let Some(gap) = step.settled.and_then(|settled| settled.left.first()) {
        return one_line(gap, MISSING).trim_end_matches('.').to_owned();
    }
    match &run.stop {
        Some(StopReason::Stopped) => "you stopped the run".into(),
        Some(StopReason::Deadline) => "the deadline came first".into(),
        Some(StopReason::StopDirective) => "the run stopped where you asked".into(),
        Some(StopReason::Blocked { .. }) => "the run stopped early".into(),
        _ => "it wasn't finished".into(),
    }
}

/// Problems and risks, from the run's records, grouped: what got in the way (declined
/// permissions, resumes that failed, changes held unverified, checks that couldn't run, work
/// stopped before it finished, interruptions and lateness), then the risks each phase's
/// verifier named.
fn problems(run: &OvernightRun, board: &Board, steps: &[Step], now: i64) -> Vec<String> {
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
    let segments = chain(run, board);
    for step in steps {
        let risks: Vec<&String> = phase_tasks(step.number, board, &segments)
            .into_iter()
            .rev()
            .find(|task| task.role == Some(WorkerRole::Verifier) && task.report.is_some())
            .and_then(|task| task.report.as_ref())
            .map(|report| report.risks.iter().take(RISKS).collect())
            .unwrap_or_default();
        for risk in risks {
            lines.push(format!(
                "Phase {}: {}",
                step.number,
                one_line(without_marker(risk), DECIDED).trim_end_matches('.')
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

/// The folded part: how each phase was checked, where the work is, who did it on which model,
/// the usage and what Brigadier ignored in the user's words.
fn details(run: &OvernightRun, board: &Board, steps: &[Step], usage: &Usage) -> String {
    let mut text = String::new();
    let segments = chain(run, board);
    let mut checked = String::new();
    for step in steps {
        let lines = phase_evidence(step, board, &segments);
        if lines.is_empty() {
            continue;
        }
        checked.push_str(&format!("- Phase {}:\n", step.number));
        for line in lines {
            checked.push_str(&format!("  - {line}\n"));
        }
    }
    if !checked.is_empty() {
        text.push_str(&format!("How each phase was checked:\n{checked}\n"));
    }
    if let Some(workspace) = &run.workspace {
        text.push_str(&format!(
            "Branch `{}` from `{}`. Worktree and handoffs: `{}`.\n",
            workspace.branch, workspace.base, workspace.path
        ));
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

/// Who did the work (the worker lineage), one line per phase: its leads and verifiers by name
/// and model, in the order they started, then its reviews, scouts and judges counted by model.
fn worker_lines(run: &OvernightRun, board: &Board) -> Vec<String> {
    let tasks: Vec<&Task> = run_tasks(run, board).collect();
    let mut phases: Vec<(String, Option<u32>)> = steps(run, board)
        .iter()
        .map(|step| (format!("Phase {}", step.number), Some(step.number)))
        .collect();
    phases.push(("Outside a phase".into(), None));
    let mut lines = Vec::new();
    for (label, id) in phases {
        let of: Vec<&Task> = tasks
            .iter()
            .copied()
            .filter(|task| task.phase == id)
            .collect();
        if of.is_empty() {
            continue;
        }
        let role = |task: &Task| match (task.role, task.run.as_ref().map(|c| c.role)) {
            (Some(WorkerRole::Verifier), _) => "verifier",
            (Some(WorkerRole::Reviewer), _) | (_, Some(RunRole::Check)) => "review",
            _ => match task.kind {
                TaskKind::Implement | TaskKind::Merge => "lead",
                TaskKind::Scout | TaskKind::Research => "scout",
                TaskKind::Review => "review",
                TaskKind::Verify => "verifier",
                TaskKind::Operate => "operator",
            },
        };
        let named = |task: &Task| {
            format!(
                "task-{} ({} {})",
                task.number,
                provider_name(task.route.choice.provider),
                task.route.choice.model.as_deref().unwrap_or("default")
            )
        };
        let mut parts = Vec::new();
        for (kind, many) in [("lead", "leads"), ("verifier", "verified by")] {
            let members: Vec<String> = of
                .iter()
                .filter(|task| role(task) == kind)
                .map(|task| named(task))
                .collect();
            // An old run checked every task on its own: those many verifiers are counted.
            if kind == "verifier" && members.len() > 3 {
                let many: Vec<&Task> = of
                    .iter()
                    .copied()
                    .filter(|task| role(task) == kind)
                    .collect();
                parts.push(format!("{} verifiers ({})", many.len(), models(&many)));
                continue;
            }
            if !members.is_empty() {
                let word = if kind == "lead" && members.len() == 1 {
                    "lead"
                } else {
                    many
                };
                parts.push(format!("{word} {}", members.join(", ")));
            }
        }
        for (kind, one, many) in [
            ("review", "review", "reviews"),
            ("scout", "scout", "scouts"),
            ("operator", "app run", "app runs"),
            ("judge", "judge", "judges"),
        ] {
            let members: Vec<&Task> = of
                .iter()
                .copied()
                .filter(|task| role(task) == kind)
                .collect();
            if !members.is_empty() {
                parts.push(format!(
                    "{} ({})",
                    plural(members.len(), one, many),
                    models(&members)
                ));
            }
        }
        lines.push(sentence(&format!("{label}: {}", parts.join("; "))));
    }
    lines
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
    let first = tasks
        .first()
        .map_or(ProviderKind::Claude, |task| task.route.choice.provider);
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

/// A line without its list marker: "- ", "* ", "• ", "2. " or "2) ".
fn without_marker(line: &str) -> &str {
    let line = line.trim_start_matches(['-', '*', '•', ' ', '\t']);
    let number = line.trim_start_matches(|c: char| c.is_ascii_digit());
    match number.strip_prefix(['.', ')']) {
        Some(rest) if number.len() < line.len() => rest.trim_start(),
        _ => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ConversationId;
    use crate::overnight::{ObstacleKind, OvernightPhase, OvernightState};

    #[test]
    fn usage_counts_the_session_s_reviews_with_its_thread() {
        let run = OvernightRun::for_test(ConversationId("c".into()), "Speed", Vec::new());
        let turn = |conversation: &str, task: Option<&str>| TurnUsage {
            at_ms: 1,
            provider: ProviderKind::Codex,
            model: "m".into(),
            conversation_id: Some(conversation.into()),
            project_id: None,
            task_id: task.map(str::to_owned),
            input: 1,
            cached_input: 0,
            cache_write: 0,
            output: 1,
            step: None,
            duration_ms: None,
            request_id: None,
            context: None,
            child_thread: None,
            cost_usd: None,
            account: None,
        };
        let board = Board::default();
        assert!(counts_for(&turn("c", None), &run, &board));
        assert!(counts_for(&turn("c", Some("review:01a1")), &run, &board));
        assert!(!counts_for(&turn("d", Some("review:01a1")), &run, &board));
        assert!(!counts_for(&turn("c", Some("task-9")), &run, &board));
    }

    fn usage() -> Usage {
        vec![
            (ProviderKind::Claude, Some(26_570_557)),
            (ProviderKind::Codex, Some(55_667_191)),
        ]
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

    /// A run of three phases: phase 1 settled done at `a1`, phase 2 settled partial, phase 3
    /// not reached; the thread committed once more after phase 1, with its review running.
    fn settled_run() -> (OvernightRun, Board) {
        let phases = vec![
            OvernightPhase::new(1, "Measure", "", &["It is measured.".into()], &[]),
            OvernightPhase::new(2, "Re-measure", "", &["Again.".into()], &[]),
            OvernightPhase::new(3, "Report", "", &["Written.".into()], &[]),
        ];
        let mut run = OvernightRun::for_test(ConversationId("c".into()), "Speed", phases);
        run.state = OvernightState::Finished;
        run.stop = Some(StopReason::Deadline);
        run.workspace = Some(crate::overnight::RunWorkspace {
            base: "main".into(),
            base_commit: "base000".into(),
            branch: "overnight/speed".into(),
            path: "/tmp/run".into(),
        });
        run.verified_commit = Some("a1a1a1a1".into());
        let settled = |outcome, summary: &str, left: &[&str], tip: &str| {
            Some(StepSettlement {
                outcome,
                summary: summary.into(),
                left: left.iter().map(|line| (*line).to_owned()).collect(),
                tip: Some(tip.into()),
                at_ms: 1,
            })
        };
        let plan = crate::work::Plan {
            id: crate::work::CardId::generate(),
            conversation_id: run.conversation_id.clone(),
            request_id: Some(super::super::run::run_request(&run)),
            position: 0,
            title: "Speed".into(),
            steps: vec![
                crate::work::PlanStep {
                    title: "Measure".into(),
                    number: Some(1),
                    stage: PhaseStage::Done,
                    settled: settled(
                        StepOutcome::Done,
                        "Measured it: `just bench` printed 3.1 s.",
                        &[],
                        "a1a1a1a1",
                    ),
                    ..Default::default()
                },
                crate::work::PlanStep {
                    title: "Re-measure".into(),
                    number: Some(2),
                    stage: PhaseStage::Failed,
                    settled: settled(
                        StepOutcome::Partial,
                        "Half of it.",
                        &["The cold run isn't measured."],
                        "b2b2b2b2",
                    ),
                    ..Default::default()
                },
                crate::work::PlanStep {
                    title: "Report".into(),
                    number: Some(3),
                    ..Default::default()
                },
            ],
            state: crate::work::PlanState::Approved {
                by: crate::work::PlanApprover::Orchestrator,
            },
            created_at_ms: 0,
            decided_at_ms: None,
        };
        run.plan_id = Some(plan.id.clone());
        let mut board = Board::default();
        board.plans.insert(plan.id.clone(), plan);
        for (id, tip, state) in [
            ("r1", "a1a1a1a1", ReviewState::Clean),
            ("r2", "c3c3c3c3", ReviewState::Running),
        ] {
            board.reviews.insert(
                id.into(),
                ReviewRun {
                    id: id.into(),
                    conversation_id: run.conversation_id.clone(),
                    request_id: None,
                    task_id: None,
                    kind: ReviewKind::Code,
                    base: "base000".into(),
                    tip: tip.into(),
                    author: ProviderKind::Claude,
                    reviewer: ProviderKind::Codex,
                    reviewer_model: None,
                    notify: crate::work::ReviewFor::Orchestrator,
                    state,
                    started_at_ms: 0,
                    ended_at_ms: None,
                    findings: None,
                },
            );
        }
        board.runs.insert(run.id.clone(), run.clone());
        (run, board)
    }

    #[test]
    fn the_report_reads_the_threads_settlements_and_merges_the_accepted_tip() {
        let (run, board) = settled_run();
        let commits = [
            RunCommit {
                commit: "c3c3c3c3".into(),
                subject: "Tidy the bench".into(),
            },
            RunCommit {
                commit: "b2b2b2b2".into(),
                subject: "Re-measure warm".into(),
            },
            RunCommit {
                commit: "a1a1a1a1".into(),
                subject: "Measure".into(),
            },
        ];
        let text = render(&run, &board, &commits, &usage(), 0);
        println!("{text}");
        let (shown, details) = text.split_once(DETAILS).expect("details");
        assert!(
            shown.starts_with("**Speed**: stopped at the deadline at ")
                && shown.contains(". 1 of 3 phases done.\n\n"),
            "{text}"
        );
        assert!(
            shown.contains(
                "\n\nMerge takes phase 1 (`a1a1a1a`). 2 later commits aren't in the merge.\n\n"
            ),
            "{text}"
        );
        for line in [
            "- ✓ Phase 1 · Measure: done.\n",
            "- ◐ Phase 2 · Re-measure: partial, The cold run isn't measured.\n",
            "- Phase 3 · Report: not reached.\n",
            "- `c3c3c3c` Tidy the bench \u{b7} review still running (not in the merge)\n",
            "- `b2b2b2b` Re-measure warm (not in the merge)\n",
            "- `a1a1a1a` Measure \u{b7} review: no findings\n",
        ] {
            assert!(shown.contains(line), "{line}\n{text}");
        }
        // The evidence is kept, folded.
        assert!(!shown.contains("Settled:"), "{text}");
        for line in [
            "How each phase was checked:\n- Phase 1:\n  - Settled: Measured it: `just bench` printed 3.1 s.\n",
            "- Phase 2:\n  - Settled: Half of it.\n  - Left: The cold run isn't measured.\n",
            "Usage: ",
        ] {
            assert!(details.contains(line), "{line}\n{text}");
        }
        assert!(!details.contains("Phase 3:"), "{text}");
        assert_eq!(notification_body(&run, &board), "1 of 3 phases done");
    }

    #[test]
    fn the_report_lists_problems_and_folds_the_details() {
        let phases = vec![
            OvernightPhase::new(1, "Measure", "", &["It is measured.".into()], &[]),
            OvernightPhase::new(2, "Re-measure", "", &["Again.".into()], &[]),
        ];
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
            shown.contains("\n### Problems and risks\n- `git push` was declined by the overnight rules: it acts outside this machine (3 times, task-3, task-5).\n"),
            "{text}"
        );
        assert!(details.contains("Usage: "), "{text}");
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
