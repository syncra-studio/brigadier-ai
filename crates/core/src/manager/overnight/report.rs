//! The morning report (PLAN.md §10.11): one message at the end of the run, rendered from the
//! run's records, never from a model's memory of it. Its first three lines say the outcome,
//! where the work is and what waits on the user; then each phase with one line per criterion,
//! the commits (which are verified, which are partial work), what was decided for the user
//! (one plain line each), what waits on them, and what got in the way, built from the
//! conductor's records. Everything after the `### Details` heading (the whole-phase findings,
//! workers and models, provider usage) the app shows folded.
//!
//! It is written once: its message has a stable id per run segment, looked for before it is
//! appended, and the run records it after. A crash in between finds the message on recovery
//! and only records it. The notification the report comes with is queued on the run at the
//! same time; the app delivers it as Brigadier and acknowledges it.

use super::super::decisions::waiting_run;
use super::super::gates::{criterion_evidence, without_marker};
use super::super::{SessionManager, blocking, git_error};
use crate::board::Board;
use crate::model::{DomainEvent, Setup};
use crate::now_ms;
use crate::overnight::{
    CriterionStatus, Deadline, OvernightPhase, OvernightRun, PhaseState, RunNotification, RunRole,
    StopReason,
};
use crate::sessions::one_line;
use crate::work::{DecisionKind, DecisionSource, RequestState, TaskKind, TaskState, UserRequest};

/// Commits listed in the report, at most.
const COMMITS: usize = 60;
/// A criterion's evidence in the report, at most (the full text is on the phase).
const EVIDENCE: usize = 240;
/// A decision's line, at most.
const DECIDED: usize = 160;
/// The heading after which the app folds the report.
pub const DETAILS: &str = "### Details";

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
        let commits = self.run_commits(run).await;
        let now = now_ms();
        let mut text = render(run, &board, &commits, now);
        text.push_str(&self.run_usage(run, &board, now).await);
        if let Some(message) = &written {
            // Reconciliation uses the text already posted, not newly rendered facts.
            text = self.full_text(message).await;
        }
        let outcome: Vec<String> = text.split("\n\n").take(3).map(str::to_owned).collect();
        let outcome: Option<[String; 3]> = outcome.try_into().ok();
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
            title: notification_title(run),
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
        if now.notification.is_none() {
            now.notification = Some(notification);
        }
        if let Err(err) = self.record_run(&now).await {
            tracing::warn!(run = %run.id, error = %err, "could not record the report/outbox; will reconcile it");
        }
    }

    /// Provider-separated observed usage. Window percentages are account snapshots, not
    /// this run's bill. Only turns belonging to this segment's tasks/lead are counted.
    async fn run_usage(&self, run: &OvernightRun, board: &Board, now: i64) -> String {
        let mut text = String::from("\n#### Provider usage\n");
        let start = run.started_at_ms.unwrap_or(run.created_at_ms);
        let end = run.finished_at_ms.unwrap_or_else(now_ms);
        for provider in brigadier_providers::ProviderKind::ALL {
            let turns = match self.runtime.routing_store() {
                Some(store) => store.turns_since(provider, start).await.ok(),
                None => None,
            };
            let Some(turns) = turns else {
                text.push_str(&format!(
                    "- {}: usage records unavailable.\n",
                    provider.label()
                ));
                continue;
            };
            let mut tokens: i64 = 0;
            let mut counted = 0;
            for turn in turns.into_iter().filter(|turn| turn.at_ms <= end) {
                let belongs = match turn.task_id.as_ref() {
                    Some(task_id) => board.tasks.values().any(|task| {
                        &task.id.0 == task_id
                            && task
                                .run
                                .as_ref()
                                .is_some_and(|context| context.run_id == run.id)
                    }),
                    None => turn.conversation_id.as_deref() == Some(run.conversation_id.0.as_str()),
                };
                if belongs {
                    tokens += turn.input + turn.cached_input + turn.cache_write + turn.output;
                    counted += 1;
                }
            }
            text.push_str(&format!(
                "- {}: {} tokens in {counted} turns (including cached input).\n",
                provider.label(),
                human_count(u64::try_from(tokens).unwrap_or(0))
            ));
        }
        if let Ok(view) = self.usage_view(None).await {
            // A blank line first: without it the line joins the last bullet above.
            text.push_str("\nAccount windows at report time (include other activity):\n");
            for provider in view.providers {
                if let Some(quota) = provider.quota {
                    for window in quota.windows {
                        text.push_str(&format!(
                            "- {} · {}: {:.1}% used; reset {}.\n",
                            provider.provider.label(),
                            window.window.label,
                            window.window.used_percent,
                            window
                                .window
                                .resets_at_ms
                                .map_or_else(|| "unknown".into(), |at| day_time(at, now))
                        ));
                    }
                }
            }
        }
        text
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
                if saved.state == crate::overnight::OvernightState::Finished
                    && saved.report_message_id.is_none()
                {
                    self.write_run_report(saved).await;
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

    /// The run branch's commits since the run's base, newest first.
    async fn run_commits(&self, run: &OvernightRun) -> Vec<(String, String)> {
        let (Some(workspace), Ok(conversation)) = (
            run.workspace.clone(),
            self.core.conversation(&run.conversation_id),
        ) else {
            return Vec::new();
        };
        let Some(Setup::Session { repo, .. }) = conversation.setup else {
            return Vec::new();
        };
        let git = self.git.clone();
        blocking(move || {
            let repo = git.open(std::path::Path::new(&repo)).map_err(git_error)?;
            let Some(tip) = repo.branch_tip(&workspace.branch).map_err(git_error)? else {
                return Ok(Vec::new());
            };
            let mut commits = Vec::new();
            for commit in repo.log(&tip, COMMITS).map_err(git_error)? {
                if commit.commit.0 == workspace.base_commit {
                    break;
                }
                commits.push((commit.commit.0, commit.subject));
            }
            Ok(commits)
        })
        .await
        .unwrap_or_default()
    }
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(10)]
}

fn counts(run: &OvernightRun) -> (usize, usize, usize, usize) {
    let worked: Vec<&OvernightPhase> = run
        .phases
        .iter()
        .filter(|phase| phase.state != PhaseState::Skipped)
        .collect();
    let of = |state: PhaseState| worked.iter().filter(|p| p.state == state).count();
    (
        worked.len(),
        of(PhaseState::Verified),
        of(PhaseState::Partial),
        of(PhaseState::Blocked),
    )
}

fn ending(run: &OvernightRun) -> String {
    match &run.stop {
        Some(StopReason::Done) => "finished".into(),
        Some(StopReason::Stopped) => "stopped by you".into(),
        Some(StopReason::Deadline) => "stopped at the deadline".into(),
        Some(StopReason::StopDirective) => "stopped where you asked".into(),
        Some(StopReason::Blocked { phase_id }) => {
            let number = run
                .phase(phase_id)
                .map_or_else(|| "0".to_owned(), |phase| phase.number.to_string());
            format!("stopped early: phase {number} needs you")
        }
        Some(StopReason::Failed { message }) => format!("could not run: {message}"),
        None => "ended".into(),
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

fn notification_title(run: &OvernightRun) -> String {
    match &run.stop {
        Some(StopReason::Blocked { .. }) => format!("{}: {}", run.name, ending(run)),
        _ => format!("{} {}", run.name, ending(run)),
    }
}

fn notification_body(run: &OvernightRun, board: &Board) -> String {
    let (worked, verified, _, _) = counts(run);
    let waits = waiting(run, board).len();
    let mut body = format!("{verified} of {worked} phases verified");
    if waits > 0 {
        body.push_str(&format!(" · {waits} waiting on you"));
    }
    body
}

/// The report, from the records alone, as of `now`.
fn render(run: &OvernightRun, board: &Board, commits: &[(String, String)], now: i64) -> String {
    let (worked, verified, partial, blocked) = counts(run);
    let waits = waiting(run, board);
    let mut text = String::new();
    // 1. The outcome.
    let mut outcome = format!(
        "**{}** {}: {verified} of {worked} phases verified",
        run.name,
        ending(run)
    );
    if partial > 0 {
        outcome.push_str(&format!(", {partial} partial"));
    }
    if blocked > 0 {
        outcome.push_str(&format!(", {blocked} blocked"));
    }
    text.push_str(&outcome);
    text.push_str(".\n\n");
    // 2. Where the work is.
    match &run.workspace {
        Some(workspace) => {
            let after = run.verified_commit.as_ref().map_or(commits.len(), |tip| {
                commits
                    .iter()
                    .take_while(|(commit, _)| commit != tip)
                    .count()
            });
            match &run.verified_commit {
                Some(tip) => text.push_str(&format!(
                    "The work is on `{}` (from `{}`). Merge takes the verified tip `{}`{}.\n\n",
                    workspace.branch,
                    workspace.base,
                    short(tip),
                    if after > 0 {
                        format!(
                            "; the {after} later commit{} {} unverified work and stay{} on the branch",
                            if after == 1 { "" } else { "s" },
                            if after == 1 { "is" } else { "are" },
                            if after == 1 { "s" } else { "" }
                        )
                    } else {
                        String::new()
                    }
                )),
                None => text.push_str(&format!(
                    "The work is on `{}` (from `{}`). Nothing is verified yet, so there is nothing to merge.\n\n",
                    workspace.branch, workspace.base
                )),
            }
        }
        None => text.push_str("No branch was made.\n\n"),
    }
    // 3. What waits on the user.
    match waits.len() {
        0 => text.push_str("Nothing waits on you.\n"),
        1 => text.push_str(&format!("Waiting on you: {}\n", waits[0])),
        n => text.push_str(&format!(
            "{n} things wait on you (listed below); first: {}\n",
            waits[0]
        )),
    }
    // Each phase: how it ended, and one line per criterion.
    if let Some(planning) = &run.planning {
        text.push_str(&format!(
            "\n### Phase 0 · Write the plan — {}\n",
            symbol(planning.state)
        ));
        for gap in &planning.gaps {
            text.push_str(&format!("- {gap}\n"));
        }
    }
    for phase in &run.phases {
        text.push_str(&format!(
            "\n### Phase {} · {} — {}\n",
            phase.number,
            phase.name,
            symbol(phase.state)
        ));
        if phase.state == PhaseState::Skipped || phase.state == PhaseState::Pending {
            continue;
        }
        for criterion in &phase.done_when {
            let result = phase.criteria.iter().find(|c| c.id == criterion.id);
            let (status, evidence) = match result {
                Some(result) => (status_word(result.status), evidence_text(&result.evidence)),
                None => ("not checked", "No check reached it."),
            };
            text.push_str(&format!(
                "- {} {} ({status}): {}\n",
                criterion.id,
                criterion.text,
                one_line(evidence, EVIDENCE)
            ));
        }
        for gap in &phase.gaps {
            text.push_str(&format!("- Still missing: {}\n", one_line(gap, EVIDENCE)));
        }
    }
    // Commits.
    if !commits.is_empty() {
        text.push_str("\n### Commits on the run branch\n");
        let mut verified = run.verified_commit.is_none();
        for (commit, subject) in commits {
            if Some(commit) == run.verified_commit.as_ref() {
                verified = true;
            }
            text.push_str(&format!(
                "- `{}` {}{}\n",
                short(commit),
                subject,
                if verified && run.verified_commit.is_some() {
                    ""
                } else {
                    " (unverified)"
                }
            ));
        }
        if commits.len() == COMMITS {
            text.push_str("The list is limited to the latest 60; the run branch has them all.\n");
        }
    }
    // Decided for you: one plain line each; the findings behind them are in the tasks. A
    // phase's outcome is its section's heading already.
    let decided: Vec<String> = run_decisions(run, board)
        .filter(|decision| decision.kind != DecisionKind::PhaseOutcome)
        .map(|decision| format!("- {}", one_line(&decision.what, DECIDED)))
        .collect();
    if !decided.is_empty() {
        text.push_str("\n### Decided for you\n");
        text.push_str(&decided.join("\n"));
        text.push('\n');
    }
    if !waits.is_empty() {
        text.push_str("\n### Waiting on you\n");
        for item in &waits {
            text.push_str(&format!("- {item}\n"));
        }
        text.push_str("Answer them, then say \"continue\" (or press Continue) to pick the run up on the same branch.\n");
    }
    let in_the_way = in_the_way(run, board, now);
    if !in_the_way.is_empty() {
        text.push_str("\n### What got in the way\n");
        for line in &in_the_way {
            text.push_str(&format!("- {line}\n"));
        }
    }
    // The details, folded by the app: everything after this heading.
    text.push_str(&format!("\n{DETAILS}\n"));
    if let Some(workspace) = &run.workspace {
        text.push_str(&format!("- Run worktree and handoffs: `{}`; each worker's kept work and handoff are linked from its task.\n", workspace.path));
    }
    for line in &run.directives.ignored {
        text.push_str(&format!("- {line}.\n"));
    }
    for phase in &run.phases {
        let Some(gate) = &phase.gate else {
            continue;
        };
        text.push_str(&format!(
            "\n#### Phase {} whole-phase checks\n{} round{}, last on `{}`; {} fix round{}.\n",
            phase.number,
            gate.round,
            if gate.round == 1 { "" } else { "s" },
            gate.commit.as_deref().map_or("?", short),
            phase.fix_rounds,
            if phase.fix_rounds == 1 { "" } else { "s" }
        ));
        for finding in &gate.findings {
            text.push_str(&format!("- Finding {}: {}\n", finding.id, finding.text));
        }
        for response in &phase.responses {
            text.push_str(&format!("- Lead's answer: {response}\n"));
        }
    }
    // Who did the work.
    let mut tasks: Vec<_> = board
        .tasks
        .values()
        .filter_map(|task| {
            task.run
                .as_ref()
                .filter(|context| context.run_id == run.id)
                .map(|context| (task, context.role))
        })
        .collect();
    tasks.sort_by_key(|(task, _)| task.number);
    if !tasks.is_empty() {
        text.push_str("\n#### Workers and models\n");
        for (task, role) in tasks {
            text.push_str(&format!(
                "- task-{} {} ({}, {} {}): {}\n",
                task.number,
                task.title,
                role_word(role),
                task.route.choice.provider.label(),
                task.route.choice.model.as_deref().unwrap_or("default"),
                state_word(task)
            ));
        }
    }
    text
}

/// The decisions taken for the run: its own, its tasks', and those of its requests.
fn run_decisions<'a>(
    run: &'a OvernightRun,
    board: &'a Board,
) -> impl Iterator<Item = &'a crate::work::Decision> {
    board
        .decisions
        .iter()
        .filter(move |decision| match &decision.source {
            DecisionSource::Run { run_id, .. } => run_id == &run.id,
            DecisionSource::Task { task_id } => board
                .tasks
                .get(task_id)
                .and_then(|task| task.run.as_ref())
                .is_some_and(|context| context.run_id == run.id),
            _ => decision
                .request_id
                .as_deref()
                .is_some_and(|request| request.starts_with(&format!("run-{}-", run.id.short()))),
        })
}

/// What got in the way of the run, from its records, grouped: what the overnight rules
/// declined, resumes that failed, checks that couldn't run, changes held unverified, work
/// stopped before it finished, interruptions, lateness and phases not reached.
fn in_the_way(run: &OvernightRun, board: &Board, now: i64) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for obstacle in &run.obstacles {
        lines.push(format!(
            "{}{}",
            obstacle.text.trim_end_matches('.'),
            times_in(obstacle.count, &obstacle.tasks)
        ));
    }
    // Checks the run's verifiers couldn't run, by check, for the tasks they checked.
    let mut checks: std::collections::BTreeMap<(&str, String), (Vec<u32>, String)> =
        Default::default();
    let run_tasks = || {
        board.tasks.values().filter(|task| {
            task.run
                .as_ref()
                .is_some_and(|context| context.run_id == run.id)
        })
    };
    for verifier in run_tasks().filter(|task| task.kind == TaskKind::Verify) {
        let Some(report) = &verifier.report else {
            continue;
        };
        let checked = verifier
            .subject
            .as_ref()
            .and_then(|id| board.tasks.get(id))
            .map_or(verifier.number, |task| task.number);
        for line in &report.risks {
            let Some((kind, check, why)) = unrun_check(line) else {
                continue;
            };
            let entry = checks
                .entry((kind, check.to_lowercase()))
                .or_insert_with(|| (Vec::new(), format!("{check}: {}", one_line(why, 120))));
            if !entry.0.contains(&checked) {
                entry.0.push(checked);
            }
        }
    }
    for ((kind, _), (mut tasks, what)) in checks {
        tasks.sort_unstable();
        lines.push(format!(
            "{} ({kind}){}",
            what.trim_end_matches('.'),
            times_in(0, &tasks)
        ));
    }
    for decision in run_decisions(run, board) {
        if decision.what.starts_with("Held task-") {
            lines.push(one_line(&decision.what, DECIDED));
        }
    }
    let stopped: Vec<String> = run_tasks()
        .filter(|task| task.kind.writes() && task.state == TaskState::Stopped)
        .map(|task| format!("task-{} \u{201c}{}\u{201d}", task.number, task.title))
        .collect();
    if !stopped.is_empty() {
        lines.push(format!(
            "Stopped before it finished: {}",
            stopped.join(", ")
        ));
    }
    for gap in &run.gaps {
        lines.push(format!("{}; no work happened then", gap.cause));
    }
    if let Deadline::At { time } = &run.directives.deadline {
        let finished = run.finished_at_ms.unwrap_or(now);
        if finished > time.at_ms {
            lines.push(format!(
                "This report is late: it was due at {} and written {} minutes after",
                time.local_time,
                (finished - time.at_ms) / 60_000
            ));
        }
    }
    let remaining: Vec<String> = run
        .phases
        .iter()
        .filter(|phase| phase.state == PhaseState::Pending && run.selects(phase.number))
        .map(|phase| format!("phase {}", phase.number))
        .collect();
    if !remaining.is_empty() {
        lines.push(format!("Not reached: {}", remaining.join(", ")));
    }
    lines.iter_mut().for_each(|line| {
        if !line.ends_with(['.', '!', '?']) {
            line.push('.');
        }
    });
    lines
}

/// A verifier's risk line about a check it couldn't run: what kind of gap, the check, and why.
fn unrun_check(line: &str) -> Option<(&'static str, &str, &str)> {
    let line = without_marker(line).trim();
    let (kind, rest) = [
        ("[excluded]", "excluded by a rule or the environment"),
        ("[pre-existing]", "fails before this change too"),
        ("[not run]", "not run"),
    ]
    .into_iter()
    .find_map(|(marker, kind)| {
        line.get(..marker.len())
            .filter(|head| head.eq_ignore_ascii_case(marker))
            .map(|_| (kind, line[marker.len()..].trim()))
    })?;
    let (check, why) = rest.split_once(':').unwrap_or((rest, ""));
    let check = check.trim();
    (!check.is_empty()).then_some((kind, check, why.trim()))
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

fn symbol(state: PhaseState) -> &'static str {
    match state {
        PhaseState::Verified => "✓ verified",
        PhaseState::Partial => "◐ partial",
        PhaseState::Blocked => "✕ blocked",
        PhaseState::Skipped => "– skipped",
        PhaseState::Pending => "not reached",
        PhaseState::Running | PhaseState::Checking => "◐ unfinished",
    }
}

/// A task's state in the words its row in the thread uses (`rowWords.ts`).
fn state_word(task: &crate::work::Task) -> &'static str {
    match task.state {
        TaskState::Queued | TaskState::Starting => "Starting",
        TaskState::Running | TaskState::Blocked => "Working",
        TaskState::Paused => "Paused",
        TaskState::Reported if task.kind.writes() => "Finished",
        TaskState::Reported => "Reported",
        TaskState::Reviewing => "Checking",
        TaskState::AwaitingApproval => "Waiting for you",
        TaskState::ReadyToLand => "Held",
        TaskState::Landed => "Landed",
        TaskState::Done => "Done",
        TaskState::Rejected => "Turned down",
        TaskState::Stopped if task.candidate.is_some() && task.landed.is_none() => "Not landed",
        TaskState::Stopped => "Stopped",
        TaskState::Failed => "Failed",
    }
}

fn role_word(role: RunRole) -> &'static str {
    match role {
        RunRole::Worker => "worker",
        RunRole::Check => "task check",
        RunRole::PhaseVerifier => "phase verifier",
        RunRole::PhaseReviewer => "phase reviewer",
        RunRole::Judge => "judge",
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

/// A local time as people read it next to `now`: "today 07:00", "tomorrow 00:13", or with
/// its day and date when further ("Sat 10 Oct 00:13").
fn day_time(at_ms: i64, now_ms: i64) -> String {
    let zone = jiff::tz::TimeZone::system();
    let (Ok(at), Ok(now)) = (
        jiff::Timestamp::from_millisecond(at_ms),
        jiff::Timestamp::from_millisecond(now_ms),
    ) else {
        return "?".into();
    };
    let (at, now) = (at.to_zoned(zone.clone()), now.to_zoned(zone));
    let days = (at.date() - now.date()).get_days();
    let time = at.strftime("%H:%M");
    match days {
        0 => format!("today {time}"),
        1 => format!("tomorrow {time}"),
        _ => at.strftime("%a %-d %b %H:%M").to_string(),
    }
}

fn status_word(status: CriterionStatus) -> &'static str {
    match status {
        CriterionStatus::Met => "met",
        CriterionStatus::NotMet => "not met",
        CriterionStatus::NotRun => "not checked",
        CriterionStatus::Blocked => "needs you",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ConversationId;
    use crate::overnight::{ObstacleKind, OvernightState};
    use crate::work::Task;

    fn task(id: &str, number: u32, kind: &str, run: &OvernightRun, subject: Option<&str>) -> Task {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "conversationId": "c",
            "number": number,
            "position": 0,
            "title": format!("Task {number}"),
            "kind": kind,
            "spec": "",
            "access": { "repo": "write", "network": false, "unsandboxed": false },
            "route": { "choice": { "provider": "claude", "model": null, "effort": null }, "reason": "" },
            "state": "done",
            "attachments": [],
            "createdAtMs": 0,
            "updatedAtMs": 0,
            "subject": subject,
            "run": {
                "runId": run.id.0,
                "segment": 1,
                "generation": 1,
                "role": "check",
                "rulesHash": ""
            }
        }))
        .expect("a task")
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
        let mut board = Board::default();
        let worker = task("t3", 3, "implement", &run, None);
        let mut verifier = task("t4", 4, "verify", &run, Some("t3"));
        verifier.report = Some(
            serde_json::from_value(serde_json::json!({
                "summary": "",
                "changes": [],
                "decisions": [],
                "verification": [],
                "openQuestions": [],
                "risks": [
                    "[excluded] GUI smoke: the sandbox can't open windows",
                    "- [not run] e2e: no browser installed",
                    "The docs are thin."
                ],
                "verdict": null,
                "artifacts": [],
                "submittedAtMs": 0
            }))
            .expect("a report"),
        );
        let mut worker = worker;
        worker.state = TaskState::ReadyToLand;
        for (n, (what, kind)) in [
            ("Verified phase 1 “Measure”", DecisionKind::PhaseOutcome),
            ("Landed task-3 “Task 3”", DecisionKind::Routine),
        ]
        .into_iter()
        .enumerate()
        {
            board.decisions.push(crate::work::Decision {
                id: format!("d{n}"),
                request_id: None,
                source: DecisionSource::Task {
                    task_id: worker.id.clone(),
                },
                what: what.into(),
                kind,
                why: String::new(),
                at_ms: 0,
                position: n as i64,
            });
        }
        board.tasks.insert(worker.id.clone(), worker);
        board.tasks.insert(verifier.id.clone(), verifier);
        board.runs.insert(run.id.clone(), run.clone());
        let text = render(&run, &board, &[], 0);
        assert!(!text.contains("— –"), "{text}");
        // A phase's outcome is its section's heading, not a decision line too.
        assert!(
            text.contains("### Decided for you\n- Landed task-3 “Task 3”\n"),
            "{text}"
        );
        assert!(!text.contains("- Verified phase 1"), "{text}");
        // States in the words of the thread's rows.
        assert!(
            text.contains("(task check, Claude Code default): Held\n"),
            "{text}"
        );
        assert!(
            text.contains("(task check, Claude Code default): Done\n"),
            "{text}"
        );
        assert!(
            text.contains("### Phase 2 · Re-measure — not reached"),
            "{text}"
        );
        let in_the_way = text
            .find("### What got in the way")
            .expect("what got in the way");
        let details = text.find(DETAILS).expect("details");
        assert!(in_the_way < details, "{text}");
        assert!(text.contains(
            "- `git push` was declined by the overnight rules: it acts outside this machine (3 times, task-3, task-5).\n"
        ), "{text}");
        assert!(text.contains(
            "- GUI smoke: the sandbox can't open windows (excluded by a rule or the environment) (task-3).\n"
        ), "{text}");
        assert!(
            text.contains("- e2e: no browser installed (not run) (task-3).\n"),
            "{text}"
        );
        assert!(!text.contains("The docs are thin"), "{text}");
        assert!(text.contains("- Not reached: phase 2.\n"), "{text}");
        assert!(
            text[details..].contains("#### Workers and models"),
            "{text}"
        );
        assert!(!text[..details].contains("task-4 Task 4"), "{text}");
    }

    #[test]
    fn numbers_and_times_read_as_people_say_them() {
        assert_eq!(human_count(950), "950");
        assert_eq!(human_count(12_340), "12.3k");
        assert_eq!(human_count(26_570_557), "26.6M");
        assert_eq!(human_count(3_000_000), "3M");
        assert_eq!(human_count(1_234_000_000), "1.2B");
        let zone = jiff::tz::TimeZone::system();
        let at = |date: jiff::civil::DateTime| {
            date.to_zoned(zone.clone())
                .expect("a local time")
                .timestamp()
                .as_millisecond()
        };
        let now = at(jiff::civil::date(2026, 10, 4).at(7, 30, 0, 0));
        assert_eq!(
            day_time(at(jiff::civil::date(2026, 10, 4).at(9, 5, 0, 0)), now),
            "today 09:05"
        );
        assert_eq!(
            day_time(at(jiff::civil::date(2026, 10, 5).at(0, 13, 0, 0)), now),
            "tomorrow 00:13"
        );
        assert_eq!(
            day_time(at(jiff::civil::date(2026, 10, 10).at(0, 13, 0, 0)), now),
            "Sat 10 Oct 00:13"
        );
    }
}
