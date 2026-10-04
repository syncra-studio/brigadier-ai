//! Mid-task fallback (PLAN.md §6 Phase 5): a worker whose provider refuses more work, or that
//! keeps failing in a way another model may not, hands its task to the best eligible model in
//! the same worktree, with the task spec, a progress log and the current diff. When no model
//! it may use is available, the task waits for the earliest reset while other tasks go on.
//!
//! - **What cuts a model off.** A usage limit (the CLI's usage-limit error, or a rate-limit
//!   notification refusing work), an auth or billing error, a context window too small for the
//!   conversation, and transient errors (overloaded, server, network) twice in a row. The cut
//!   happens when the model's turn is over, so nothing is left half-run. A worker that stalls
//!   a second time in one attempt is cut at once ([`super::watchdog`]).
//! - **The hand-off.** The old CLI session is closed (its own files are cleaned up as usual);
//!   the worktree, the task branch and the scratch folder stay. Brigadier writes
//!   `<scratch>/handoff/`: `spec.md` (the task and every later instruction from the
//!   orchestrator), `progress.md` (what the model did, in order, from the task's transcript)
//!   and, for write tasks, `diff.patch` (the worktree's changes against the task's base,
//!   untracked files included). The successor starts fresh in the same place with the same
//!   access a worker of its vendor and task kind always gets: nothing is widened.
//! - **Waiting.** With nothing eligible, the task is paused with what it waits for; a timer
//!   routes it again after the earliest known reset (or every half hour when none is known).
//!   Resuming it by hand routes it again at once.
//! - **Quiet.** The orchestrator is not woken for any of this: the task's report (or failure)
//!   says which models ran it and why they changed.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use brigadier_providers::{
    ErrorKind, LimitHit, LimitKind, ProviderError, ProviderEvent, TurnInput,
};
use brigadier_store::StreamPage;

use super::SessionManager;
use super::workers::TaskLive;
use crate::model::{DomainEvent, ModelChoice, streams};
use crate::work::{Attempt, AttemptEnd, QuotaWait, Route, Task, TaskState};
use crate::{Error, Result, now_ms};

/// Handoff files: the progress log is cut to about this many bytes (about 6k tokens), newest
/// kept.
const PROGRESS_BYTES: usize = 24_000;
/// Worker events read back for the progress log.
const PROGRESS_EVENTS: u32 = 2_000;
/// A task waiting for quota with no known reset is routed again this often.
const WAIT_RETRY: Duration = Duration::from_secs(30 * 60);
/// A reset is past this long before the task is routed again.
const AFTER_RESET: Duration = Duration::from_secs(45);
/// Provider checks this close together make one retry of the work waiting for quota.
const PROVIDER_CHECK_SETTLE: Duration = Duration::from_secs(2);
/// Hand-offs for errors (not limits) per task, before the task fails as it always did.
const ERROR_HANDOFFS: usize = 3;

/// What a worker's error means for its task.
pub(crate) enum ErrorVerdict {
    /// Hand the task on when the turn is over.
    HandOff(AttemptEnd),
    /// A transient error: retry on the same model once, hand on the next time.
    Transient,
    /// Handle it as before (the task's error, a nudge, then failure).
    Keep,
}

/// Classifies an error a worker's CLI gave up on.
pub(crate) fn verdict(error: &ProviderError) -> ErrorVerdict {
    if error.will_retry {
        return ErrorVerdict::Keep;
    }
    match error.kind {
        ErrorKind::UsageLimit => ErrorVerdict::HandOff(AttemptEnd::Limit {
            limit: error.limit.clone().unwrap_or(LimitHit {
                window: None,
                resets_at_ms: None,
                kind: LimitKind::UsageWindow,
            }),
        }),
        ErrorKind::Billing => ErrorVerdict::HandOff(AttemptEnd::Limit {
            limit: LimitHit {
                window: None,
                resets_at_ms: None,
                kind: LimitKind::Credits,
            },
        }),
        ErrorKind::Auth | ErrorKind::ContextWindow => ErrorVerdict::HandOff(AttemptEnd::Error {
            kind: error.kind,
            message: error.message.clone(),
        }),
        ErrorKind::Overloaded | ErrorKind::Server | ErrorKind::Network => ErrorVerdict::Transient,
        _ => ErrorVerdict::Keep,
    }
}

impl SessionManager {
    /// Ends the running model's part of `task` for `end` and hands the task on (or makes it
    /// wait). `from` is the CLI session the hand-off was decided for
    /// ([`TaskLive::generation`]): once another hand-on or hand-over has moved the task to a
    /// fresh session, that session is left alone. Runs outside the worker's event pump:
    /// closing the CLI waits for the pump.
    pub(crate) async fn hand_off(&self, live: &Arc<TaskLive>, end: AttemptEnd, from: u64) {
        // One hand-on or hand-over at a time, each checking the session it was decided for.
        let _handing = live.reroute.lock().await;
        if live.generation().await != from {
            tracing::info!(task = %live.id, "the task was already handed on");
            return;
        }
        self.hand_off_held(live, end).await;
    }

    /// [`Self::hand_off`] for a caller that holds the task's `reroute` lock and checked the
    /// CLI session itself (the stall watchdog).
    pub(crate) async fn hand_off_held(&self, live: &Arc<TaskLive>, end: AttemptEnd) {
        let Ok(task) = self.task_by_id(&live.conversation_id, &live.id).await else {
            return;
        };
        // Ended, or its attempt already ended (another hand-on made it wait for quota).
        let ended = task
            .attempts
            .last()
            .is_some_and(|attempt| attempt.ended_at_ms.is_some());
        if task.state.is_final() || ended {
            return;
        }
        let errors = task
            .attempts
            .iter()
            .filter(|attempt| matches!(attempt.end, Some(AttemptEnd::Error { .. })))
            .count();
        if matches!(end, AttemptEnd::Error { .. }) && errors + 1 >= ERROR_HANDOFFS {
            let reason = match &end {
                AttemptEnd::Error { message, .. } => message.clone(),
                AttemptEnd::Limit { .. } => String::new(),
            };
            self.worker_failed(&task, &reason).await;
            return;
        }
        live.close_cli().await;
        // A new attempt: its hand-overs and stalls are counted afresh.
        live.set_handovers(None);
        live.reset_stalls().await;
        let from = task.route.choice.clone();
        if let AttemptEnd::Limit { limit } = &end {
            self.runtime.note_limit(from.provider, limit.clone()).await;
        }
        if matches!(
            &end,
            AttemptEnd::Error {
                kind: ErrorKind::Auth,
                ..
            }
        ) {
            self.runtime.refresh_providers(Some(from.provider));
        }
        let ended = self
            .update_task(&task.conversation_id, &task.id, |task| {
                end_attempt(task, Some(end.clone()));
                task.error = None;
            })
            .await;
        let Ok(task) = ended else {
            return;
        };
        if let Some(attempt) = task.attempts.last() {
            // A limit says something about the model's quota; an error (an outage, a logout,
            // a crashed CLI) nothing about the model.
            let result = match &end {
                AttemptEnd::Limit { .. } => brigadier_router::OutcomeResult::HandedOff,
                AttemptEnd::Error { .. } => brigadier_router::OutcomeResult::Stopped,
            };
            self.record_outcome(&task, attempt, result).await;
        }
        tracing::info!(task = %task.id, from = %from.provider, "handing the task on");
        self.continue_held(live, task).await;
    }

    /// Routes a task whose model stopped (or that waits for quota) and starts the chosen
    /// model on it, or makes it wait. One at a time per task: a caller that finds a model
    /// already at work (another caller started it) leaves it be. Boxed: the worker it starts
    /// can come back here, and a recursive future must name its `Send` bound.
    pub(crate) fn continue_task<'a>(
        &'a self,
        live: &'a Arc<TaskLive>,
        task: Task,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let _rerouting = live.reroute.lock().await;
            self.continue_held(live, task).await;
        })
    }

    /// [`Self::continue_task`] for a caller that holds the task's `reroute` lock.
    fn continue_held<'a>(
        &'a self,
        live: &'a Arc<TaskLive>,
        task: Task,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let Ok(task) = self.task_by_id(&task.conversation_id, &task.id).await else {
                return;
            };
            let running = task
                .attempts
                .last()
                .is_some_and(|attempt| attempt.ended_at_ms.is_none());
            if task.state.is_final() || running {
                return;
            }
            let route = match self.reroute(&task).await {
                Ok(route) => route,
                Err(waiting) => {
                    self.wait_for_quota(live, &task, waiting).await;
                    return;
                }
            };
            // A task that waited from the start begins as a new task does.
            let fresh = task.attempts.is_empty() && task.workspace.is_none();
            let started = self
                .update_task(&task.conversation_id, &task.id, |task| {
                    start_attempt(task, route.clone());
                    task.state = TaskState::Running;
                    task.quota_wait = None;
                    task.trial_slot = false;
                    task.blocked_reason = None;
                })
                .await;
            let task = match started {
                Ok(task) => task,
                Err(err) => {
                    tracing::warn!(task = %task.id, error = %err, "could not record the hand-off");
                    return;
                }
            };
            if fresh {
                tracing::info!(task = %task.id, "a waiting task starts");
                let subject = match &task.subject {
                    Some(id) => self.task_by_id(&task.conversation_id, id).await.ok(),
                    None => None,
                };
                if let Err(err) = self.start_worker(live, task.clone(), subject).await {
                    self.worker_failed(&task, &err.to_string()).await;
                }
                return;
            }
            let launched = async {
                let first = self.write_task_handoff(&task).await?;
                let subject = match &task.subject {
                    Some(id) => self.task_by_id(&task.conversation_id, id).await.ok(),
                    None => None,
                };
                let workspace = task
                    .workspace
                    .as_ref()
                    .ok_or_else(|| Error::Invalid("the task has no workspace".into()))?;
                let files = self
                    .worker_files(&task, &PathBuf::from(&workspace.scratch))
                    .await;
                live.allow_revival().await;
                live.clear_transient().await;
                self.launch_worker(
                    live,
                    &task,
                    subject.as_ref(),
                    brigadier_providers::Origin::New,
                    TurnInput::with_files(first, files),
                )
                .await
            }
            .await;
            if let Err(err) = launched {
                let reason = format!("The task could not be handed on: {err}");
                self.worker_failed(&task, &reason).await;
            }
        })
    }

    /// Who may take `task` over now: the router's choice with the same floor, areas, needs and
    /// rules the task started with, and its pin binding (a task pinned to a vendor stays with
    /// it, or waits), less the models that stopped on an error (a limit leaves its
    /// provider or bucket unavailable by itself). A context window too small asks for a bigger
    /// one.
    async fn reroute(&self, task: &Task) -> std::result::Result<Route, brigadier_router::Waiting> {
        let question = self.task_question(task).await;
        let decision = self
            .decide(&question.ask(
                task,
                // A pin binds once a model has worked on the task.
                !task.attempts.is_empty(),
                // A task that never started keeps the trial slot it was created with.
                if task.attempts.is_empty() && task.trial_slot {
                    super::routing::Trial::Held
                } else {
                    super::routing::Trial::Never
                },
            ))
            .await;
        match decision {
            brigadier_router::Decision::Run(routed) => Ok(super::routing::route_from(routed)),
            brigadier_router::Decision::Wait(waiting) => Err(waiting),
        }
    }

    /// Pauses `task` until quota lets it continue, and schedules the next try.
    async fn wait_for_quota(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        waiting: brigadier_router::Waiting,
    ) {
        let wait = QuotaWait {
            reason: waiting.reason.clone(),
            resets_at_ms: waiting.resets_at_ms,
            rule: waiting.rule.clone(),
            ranking: waiting.ranking.clone(),
            since_ms: task
                .quota_wait
                .as_ref()
                .map_or_else(now_ms, |known| known.since_ms),
            messages: Vec::new(),
        };
        // Waiting as before for the same reason: nothing to record again.
        let unchanged = task.state == TaskState::Paused
            && task.quota_wait.as_ref().is_some_and(|known| {
                known.reason == wait.reason
                    && known.resets_at_ms == wait.resets_at_ms
                    && known.rule == wait.rule
                    && known.ranking == wait.ranking
            });
        if !unchanged {
            let paused = self
                .update_task(&task.conversation_id, &task.id, |task| {
                    task.state = TaskState::Paused;
                    task.blocked_reason = Some(format!("Waiting for quota: {}", wait.reason));
                    task.quota_wait = Some(wait.clone());
                })
                .await;
            if let Err(err) = paused {
                tracing::warn!(task = %task.id, error = %err, "could not pause the task for quota");
                return;
            }
        }
        tracing::info!(task = %task.id, reason = %wait.reason, "task waits for quota");
        let delay = retry_delay(waiting.resets_at_ms);
        // Only the newest timer retries: an earlier one (before a rule change or a provider
        // check tried again) has nothing left to do.
        let timer = live.next_wait_timer();
        let manager = self.arc();
        let live = live.clone();
        self.spawn(async move {
            tokio::time::sleep(delay).await;
            if live.is_wait_timer(timer) {
                manager.retry_waiting(&live).await;
            }
        });
    }

    /// Whenever a provider's state is recorded (a login, a limit, a fresh usage read), work
    /// waiting for quota looks again. Checks that come close together count once.
    pub(super) fn retry_waiting_on_provider_checks(&self) {
        let mut checks = self.runtime.provider_checks();
        let manager = self.me.clone();
        self.spawn(async move {
            while checks.changed().await.is_ok() {
                tokio::time::sleep(PROVIDER_CHECK_SETTLE).await;
                checks.borrow_and_update();
                let Some(manager) = manager.upgrade() else {
                    return;
                };
                if manager.admit().is_err() {
                    return;
                }
                manager.retry_waiting_work().await;
            }
        });
    }

    /// Routes everything that waits for quota again, now: after the user changed their rules
    /// or rankings, or a provider's state changed (a login, a fresh usage read).
    pub async fn retry_waiting_work(&self) {
        let tasks: Vec<Arc<TaskLive>> = self.tasks_lock().values().cloned().collect();
        for live in tasks {
            self.retry_waiting(&live).await;
        }
        self.retry_waiting_conversations().await;
    }

    /// After a restart: a task that waited for quota keeps waiting, with its timer set again.
    pub(super) async fn keep_waiting(&self, task: &Task) {
        let Some(wait) = task.quota_wait.clone() else {
            return;
        };
        let live = self.task_live(task);
        let waiting = brigadier_router::Waiting {
            reason: wait.reason,
            resets_at_ms: wait.resets_at_ms,
            rule: wait.rule,
            ranking: wait.ranking,
        };
        self.wait_for_quota(&live, task, waiting).await;
    }

    /// Routes a task waiting for quota again, if it still waits.
    pub(crate) async fn retry_waiting(&self, live: &Arc<TaskLive>) {
        if self.existing_task_live(&live.id).is_none() {
            return;
        }
        let Ok(task) = self.task_by_id(&live.conversation_id, &live.id).await else {
            return;
        };
        if task.state != TaskState::Paused || task.quota_wait.is_none() {
            return;
        }
        self.continue_task(live, task).await;
    }

    /// Writes the successor's hand-off files and answers its first message.
    async fn write_task_handoff(&self, task: &Task) -> Result<String> {
        let (dir, has_diff) = self.write_handoff_files(task).await?;
        let before = task
            .attempts
            .iter()
            .rev()
            .find(|attempt| attempt.end.is_some());
        let (who, why) = match before {
            Some(attempt) => (
                model_label(&attempt.route.choice),
                attempt.end.as_ref().map_or_else(String::new, end_phrase),
            ),
            None => ("another model".to_owned(), String::new()),
        };
        let mut text = format!(
            "You are taking over task-{} from {who}, which stopped{why}. Its hand-off is in {}: \
             spec.md (the task and every later instruction from the orchestrator) and \
             progress.md (what it did, in order)",
            task.number,
            dir.display()
        );
        if task.kind.writes() {
            text.push_str(if has_diff {
                ", and diff.patch (its changes against the task's base, as the worktree holds \
                 them now). Read them first and check the worktree: keep its work unless it is \
                 wrong, don't redo what is done, then finish the task and verify it"
            } else {
                ". It left no changes in the worktree yet. Read them first, then do the task \
                 and verify it"
            });
        } else {
            text.push_str(". Read them first, don't redo what is done, then finish the task");
        }
        text.push_str(". Report with submit_report as your rules say.");
        Ok(text)
    }

    /// Writes `<scratch>/handoff/`: spec.md, progress.md and, for a write task, diff.patch.
    /// Answers the folder and whether the worktree has changes.
    pub(crate) async fn write_handoff_files(&self, task: &Task) -> Result<(PathBuf, bool)> {
        let workspace = task
            .workspace
            .clone()
            .ok_or_else(|| Error::Invalid("the task has no workspace".into()))?;
        let dir = PathBuf::from(&workspace.scratch).join("handoff");
        let mut spec = format!("# task-{}: {}\n\n{}\n", task.number, task.title, task.spec);
        if !task.messages.is_empty() {
            spec.push_str("\n## Later instructions from the orchestrator, oldest first\n");
            for message in &task.messages {
                let _ = write!(spec, "\n- {}", message.replace('\n', "\n  "));
            }
            spec.push('\n');
        }
        let progress = self.progress_log(task).await;
        let diff = match (&workspace.worktree, &workspace.base, task.kind.writes()) {
            (Some(worktree), Some(base), true) => {
                let (git, path, base) = (
                    self.git.clone(),
                    PathBuf::from(worktree),
                    brigadier_git::Oid(base.clone()),
                );
                let diff = super::blocking(move || {
                    let worktree = git.open_worktree(&path).map_err(super::git_error)?;
                    worktree.diff_from(&base).map_err(super::git_error)
                })
                .await?;
                Some(diff)
            }
            _ => None,
        };
        let has_diff = diff.as_ref().is_some_and(|diff| !diff.is_empty());
        {
            let dir = dir.clone();
            super::blocking(move || {
                let io =
                    |err: std::io::Error| Error::Invalid(format!("writing the hand-off: {err}"));
                std::fs::create_dir_all(&dir).map_err(io)?;
                std::fs::write(dir.join("spec.md"), spec).map_err(io)?;
                std::fs::write(dir.join("progress.md"), progress).map_err(io)?;
                if let Some(diff) = diff {
                    std::fs::write(dir.join("diff.patch"), diff).map_err(io)?;
                }
                Ok(())
            })
            .await?;
        }
        Ok((dir, has_diff))
    }

    /// What the models before did on `task`, oldest first, from its transcript.
    async fn progress_log(&self, task: &Task) -> String {
        let page = self
            .core
            .store()
            .read_stream(
                streams::task(&task.id),
                StreamPage {
                    before: None,
                    kinds: vec!["worker.event".into()],
                    limit: PROGRESS_EVENTS,
                },
            )
            .await
            .unwrap_or_default();
        let mut lines: Vec<String> = page
            .iter()
            .rev()
            .filter_map(|stored| serde_json::from_str::<DomainEvent>(stored.payload.get()).ok())
            .filter_map(|event| match event {
                DomainEvent::WorkerEvent { event, .. } => progress_line(&event),
                _ => None,
            })
            .collect();
        let mut size: usize = lines.iter().map(|line| line.len() + 1).sum();
        let mut cut = 0;
        while size > PROGRESS_BYTES && cut < lines.len() {
            size -= lines[cut].len() + 1;
            cut += 1;
        }
        let mut text = format!("# What happened on task-{} so far\n\n", task.number);
        if cut > 0 {
            let _ = writeln!(
                text,
                "(The first {cut} entries are left out; the newest are kept.)\n"
            );
        }
        lines.drain(..cut);
        for line in lines {
            text.push_str(&line);
            text.push('\n');
        }
        text
    }
}

/// One progress-log line for a worker event worth knowing about.
fn progress_line(event: &ProviderEvent) -> Option<String> {
    use brigadier_providers::{ItemStatus, Role};
    let one = |text: &str, max: usize| {
        let text = text.trim().replace('\n', " ⏎ ");
        if text.chars().count() > max {
            format!("{}…", text.chars().take(max).collect::<String>())
        } else {
            text
        }
    };
    Some(match event {
        ProviderEvent::SessionStarted { model, .. } => {
            format!(
                "## A model started ({})",
                model.as_deref().unwrap_or("its default model")
            )
        }
        ProviderEvent::Message {
            role: Role::Assistant,
            text,
            ..
        } => format!("- Said: {}", one(text, 800)),
        ProviderEvent::Message {
            role: Role::User,
            text,
            ..
        } => format!("- Was told: {}", one(text, 400)),
        ProviderEvent::Command {
            command,
            status: ItemStatus::Completed | ItemStatus::Failed,
            exit_code,
            output,
            ..
        } => {
            let mut line = format!("- Ran `{}`", one(command, 300));
            if let Some(code) = exit_code {
                let _ = write!(line, " (exit {code})");
            }
            if let Some(output) = output.as_deref().filter(|output| !output.trim().is_empty()) {
                let tail: String = output
                    .chars()
                    .rev()
                    .take(300)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                let _ = write!(line, ": …{}", one(&tail, 300));
            }
            line
        }
        ProviderEvent::ToolCall {
            name,
            input,
            status: ItemStatus::Completed | ItemStatus::Failed,
            ..
        } => format!(
            "- Used {name}{}",
            input
                .as_deref()
                .map(|input| format!(" {}", one(input, 240)))
                .unwrap_or_default()
        ),
        ProviderEvent::FileChanges {
            changes,
            status: ItemStatus::Completed,
            ..
        } => format!(
            "- Changed files: {}",
            changes
                .iter()
                .map(|change| change.path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ProviderEvent::Error { error } if !error.will_retry => {
            format!("- Stopped by an error: {}", one(&error.message, 400))
        }
        ProviderEvent::Notice { message, .. } => format!("- Note: {}", one(message, 300)),
        _ => return None,
    })
}

/// How long work waiting for quota sleeps before it is routed again: until just after the
/// reset that could free it, and at most [`WAIT_RETRY`].
pub(super) fn retry_delay(resets_at_ms: Option<i64>) -> Duration {
    match resets_at_ms {
        Some(at) => Duration::from_millis(u64::try_from(at - now_ms()).unwrap_or(0)) + AFTER_RESET,
        None => WAIT_RETRY,
    }
    .min(WAIT_RETRY)
}

/// Closes the running attempt of `task` (or its first, never recorded one) with `end`. A task
/// that waited from the start and never began has none.
pub(crate) fn end_attempt(task: &mut Task, end: Option<AttemptEnd>) {
    let now = now_ms();
    if task.attempts.is_empty() && task.workspace.is_some() {
        task.attempts.push(Attempt {
            route: task.route.clone(),
            started_at_ms: task.created_at_ms,
            ended_at_ms: None,
            end: None,
        });
    }
    if let Some(last) = task.attempts.last_mut()
        && last.ended_at_ms.is_none()
    {
        last.ended_at_ms = Some(now);
        last.end = end;
    }
}

/// Starts a new attempt of `task` on `route`, which becomes the task's route.
fn start_attempt(task: &mut Task, route: Route) {
    task.attempts.push(Attempt {
        route: route.clone(),
        started_at_ms: now_ms(),
        ended_at_ms: None,
        end: None,
    });
    task.route = route;
}

pub(crate) fn model_label(choice: &ModelChoice) -> String {
    match &choice.model {
        Some(model) => format!("{} {model}", choice.provider.label()),
        None => format!("{}'s default model", choice.provider.label()),
    }
}

/// Why an attempt ended, in a few words ("5-hour limit reached").
pub(crate) fn end_reason(end: &AttemptEnd) -> String {
    match end {
        AttemptEnd::Limit { limit } => match limit.kind {
            LimitKind::UsageWindow => match limit.window.as_deref() {
                Some(window) => format!("{} limit reached", window_name(window)),
                None => "usage limit reached".into(),
            },
            LimitKind::SpendControl => "stopped by a spend control".into(),
            LimitKind::Credits => "out of credits".into(),
        },
        AttemptEnd::Error { kind, .. } => match kind {
            ErrorKind::ContextWindow => "context window full".into(),
            ErrorKind::Auth => "logged out".into(),
            ErrorKind::Process => "its CLI exited".into(),
            ErrorKind::Stalled => "went silent".into(),
            _ => "kept failing".into(),
        },
    }
}

/// Why an attempt ended, as a clause (" because Claude's 5-hour limit was reached").
fn end_phrase(end: &AttemptEnd) -> String {
    match end {
        AttemptEnd::Limit { limit } => match limit.kind {
            LimitKind::UsageWindow => match limit.window.as_deref() {
                Some(window) => format!(
                    " because its {} usage limit was reached",
                    window_name(window)
                ),
                None => " because its usage limit was reached".into(),
            },
            LimitKind::SpendControl => " because a spend control stopped it".into(),
            LimitKind::Credits => " because its account ran out of credits".into(),
        },
        AttemptEnd::Error { message, .. } => format!(" on an error ({message})"),
    }
}

/// A window id as people say it.
fn window_name(id: &str) -> &str {
    match id {
        "five_hour" => "5-hour",
        "seven_day" => "weekly",
        "primary" => "primary",
        "secondary" => "secondary",
        other => other,
    }
}
