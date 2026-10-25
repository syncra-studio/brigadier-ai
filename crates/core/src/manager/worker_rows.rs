//! A session's own workers as their CLIs and git show them (STRIP-DOWN-PLAN S14, G19): the
//! rows its Workers row, its rail and its worker cards read, sent with its thread
//! ([`ThreadView::workers`], [`ThreadNews::workers`]) as Background is.
//!
//! - What a worker's CLI does (its turn open or how it ended, a usage limit, a compaction, when
//!   it last said anything, the calls it has at work, its background tasks) is followed from
//!   its events, live only.
//! - What it waits on the user for is its CLI's open permission prompts and questions and its
//!   MCP servers that want a sign-in.
//! - Its +/−, commits ahead and whether it merged are git's, against its session's branch
//!   (G21): read again at most every [`GIT_EVERY_MS`], and right away when a turn of the
//!   worker or a command of the session ends.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use brigadier_providers::{
    BackgroundTask, ItemStatus, LimitHit, McpStatus, ProviderEvent, Role, TurnStatus,
};

use super::cli_thread::update_background;
use super::thread_items::step_detail;
use super::{SessionManager, blocking};
use crate::model::ConversationId;
use crate::thread_view::{
    self, StepAtWork, ThreadNews, TurnEnd, TurnRecord, WorkerRow, WorkerTask, WorkerWait,
    WorkerWaitKind,
};
use crate::work::{ApprovalSubject, CardState, DiffStat, Task, TaskId, TaskState};
use crate::{Result, now_ms};

/// How long changes gather before a session's rows are sent again.
const SETTLE_MS: u64 = 250;
/// The least time between two git reads of a session's workers, unless one is due at once.
pub(crate) const GIT_EVERY_MS: i64 = 2_000;
/// How stale a row's `last_event_at_ms` may grow before it is sent again for that alone: the
/// app counts a worker quiet after 20 minutes of nothing.
const EVENT_STALE_MS: i64 = 60_000;
/// How many commands of calls not (yet) a background task's are kept for the tasks they start.
const COMMANDS_KEPT: usize = 32;

/// What a worker's CLI did, as far as its rows tell.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct WorkerCli {
    pub open: bool,
    pub last_end: Option<TurnEnd>,
    pub last_event_ms: Option<i64>,
    /// `last_event_ms` as last sent.
    sent_event_ms: Option<i64>,
    pub limit: Option<LimitHit>,
    pub fresh_context: bool,
    /// The calls it has at work in the open turn, by item id, oldest first.
    steps: Vec<(String, StepAtWork)>,
    /// Its background tasks, as its CLI says them, and those that ended lately.
    background: Vec<BackgroundTask>,
    /// The commands of its latest calls that may start a background task, by item id: a Claude
    /// task is labelled with its description, and only its call says what it runs.
    commands: Vec<(String, String)>,
    /// The context it last said it holds, and its window when it said one. Not part of its
    /// rows: `brigadier workers` reads it.
    pub context: Option<(i64, Option<i64>)>,
    /// The open turn had an error the CLI won't retry.
    errored: bool,
    /// The open turn's user message, once its CLI said it.
    user_message: Option<String>,
    /// The line the last event adds to the worker's `record/turns.jsonl`: its turn's start, or
    /// its end, which its CLI's file doesn't say (#556).
    line: Option<TurnRecord>,
}

/// What git says of a worker's work against its session's branch.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct WorkerGit {
    pub diff: Option<DiffStat>,
    pub ahead: u32,
    pub merged: bool,
}

/// The rows' live state: per worker, its CLI and its git; per session, what is due.
#[derive(Default)]
pub(crate) struct WorkerRows {
    cli: std::sync::Mutex<HashMap<TaskId, WorkerCli>>,
    git: std::sync::Mutex<HashMap<TaskId, WorkerGit>>,
    /// Sessions whose rows are to be sent, and whether git is to be read for them at once.
    due: std::sync::Mutex<HashMap<ConversationId, bool>>,
    /// When each session's workers were last read in git.
    git_read_ms: std::sync::Mutex<HashMap<ConversationId, i64>>,
    /// Each session's rows as last sent.
    sent: std::sync::Mutex<HashMap<ConversationId, Vec<WorkerRow>>>,
}

/// Follows `event` of a worker's CLI into `cli`; whether its rows changed.
pub(crate) fn follow(cli: &mut WorkerCli, event: &ProviderEvent, at: i64) -> bool {
    cli.line = None;
    let before = WorkerCli {
        user_message: None,
        context: None,
        commands: Vec::new(),
        ..cli.clone()
    };
    cli.last_event_ms = Some(at);
    match event {
        ProviderEvent::TurnStarted { .. } => {
            if !cli.open {
                cli.open = true;
                cli.fresh_context = false;
                cli.limit = None;
                cli.errored = false;
                cli.user_message = None;
                cli.steps.clear();
            }
        }
        // A turn's own message (Codex says it once its turn started); a steer's adds nothing.
        ProviderEvent::Message {
            item_id,
            role: Role::User,
            ..
        } if !cli.open || cli.user_message.is_none() => {
            if !cli.open {
                cli.open = true;
                cli.fresh_context = false;
                cli.limit = None;
                cli.errored = false;
                cli.steps.clear();
            }
            cli.user_message = Some(item_id.clone());
            cli.line = Some(turn_line(cli, None, None));
        }
        ProviderEvent::Error { error } if !error.will_retry => {
            cli.errored = true;
            if let Some(limit) = thread_view::limit_of(error) {
                cli.limit = Some(limit);
            }
        }
        ProviderEvent::CompactionStarted { .. } => cli.fresh_context = true,
        ProviderEvent::ContextSize {
            used_tokens,
            window_tokens,
        } => cli.context = Some((*used_tokens, *window_tokens)),
        ProviderEvent::TurnCompleted { status, .. } if cli.open => {
            cli.open = false;
            cli.steps.clear();
            cli.last_end = Some(match status {
                TurnStatus::Completed if cli.errored => TurnEnd::Failed,
                TurnStatus::Completed => TurnEnd::Done,
                TurnStatus::Interrupted => TurnEnd::Stopped,
                TurnStatus::Failed => TurnEnd::Failed,
            });
            cli.line = Some(turn_line(cli, cli.last_end, Some(at)));
        }
        ProviderEvent::Exited { .. } => {
            // Its background tasks end with its CLI.
            cli.background.clear();
            cli.commands.clear();
            if cli.open {
                cli.open = false;
                cli.steps.clear();
                cli.last_end = Some(TurnEnd::Interrupted);
                cli.line = Some(turn_line(cli, cli.last_end, Some(at)));
            }
        }
        ProviderEvent::BackgroundTask { task } => {
            update_background(&mut cli.background, task, at);
        }
        _ => {
            if let Some((item_id, command)) = background_call(event) {
                note_command(cli, item_id, command);
            }
            if let Some((item_id, status, step)) = step_of(event) {
                follow_step(&mut cli.steps, item_id, status, step);
            }
        }
    }
    let mut now = WorkerCli {
        user_message: None,
        line: None,
        context: None,
        commands: Vec::new(),
        ..cli.clone()
    };
    now.last_event_ms = before.last_event_ms;
    let changed = now != before
        || cli
            .sent_event_ms
            .is_none_or(|sent| at.saturating_sub(sent) >= EVENT_STALE_MS);
    if changed {
        cli.sent_event_ms = Some(at);
    }
    changed
}

/// A call that may run a command in the background: its item id and its command. Claude's
/// `Bash` is a `Command` item that doesn't say whether it runs in the background (D38); a tool
/// call says so with `run_in_background`.
fn background_call(event: &ProviderEvent) -> Option<(&str, String)> {
    match event {
        ProviderEvent::Command {
            item_id, command, ..
        } if !command.is_empty() => Some((item_id, command.clone())),
        ProviderEvent::ToolCall {
            item_id,
            input: Some(input),
            ..
        } => {
            let args: serde_json::Value = serde_json::from_str(input).ok()?;
            if args
                .get("run_in_background")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
            {
                return None;
            }
            let command = args.get("command")?.as_str()?;
            Some((item_id, command.to_owned()))
        }
        _ => None,
    }
}

/// Keeps `command` of call `item_id`; past [`COMMANDS_KEPT`] the oldest no background task is
/// of leaves, so a long-running task keeps its command.
fn note_command(cli: &mut WorkerCli, item_id: &str, command: String) {
    cli.commands.retain(|(id, _)| id != item_id);
    cli.commands.push((item_id.to_owned(), command));
    let tasks: Vec<&str> = cli
        .background
        .iter()
        .filter_map(|task| task.item_id.as_deref())
        .collect();
    if cli
        .commands
        .iter()
        .filter(|(id, _)| !tasks.contains(&id.as_str()))
        .count()
        > COMMANDS_KEPT
        && let Some(at) = cli
            .commands
            .iter()
            .position(|(id, _)| !tasks.contains(&id.as_str()))
    {
        cli.commands.remove(at);
    }
}

/// A call `event` says of, by its item id, with how it stands and, when it says, the step it is.
fn step_of(event: &ProviderEvent) -> Option<(&str, ItemStatus, Option<StepAtWork>)> {
    let step = |name: &str, detail: Option<String>| {
        Some(StepAtWork {
            name: name.to_owned(),
            detail,
        })
    };
    Some(match event {
        ProviderEvent::Command {
            item_id,
            command,
            status,
            ..
        } => (
            item_id,
            *status,
            (!command.is_empty())
                .then(|| step("shell", Some(command.chars().take(240).collect())))
                .flatten(),
        ),
        ProviderEvent::ToolCall {
            item_id,
            name,
            input,
            status,
            ..
        } => {
            let args = input
                .as_deref()
                .and_then(|input| serde_json::from_str::<serde_json::Value>(input).ok());
            (
                item_id,
                *status,
                step(name, args.as_ref().and_then(step_detail)),
            )
        }
        // As the app's `changesDetail` says a worker's file changes.
        ProviderEvent::FileChanges {
            item_id,
            changes,
            status,
        } => {
            let detail = changes.first().map(|first| match changes.len() {
                1 => first.path.clone(),
                n => format!("{} and {} more", first.path, n - 1),
            });
            (item_id, *status, step("apply_patch", detail))
        }
        ProviderEvent::Image {
            item_id,
            status,
            prompt,
            ..
        } => (item_id, *status, step("imagegen", prompt.clone())),
        _ => return None,
    })
}

/// Keeps `steps` to the calls at work: one in progress joins them, or takes its own place with
/// what more it says (a detail said once is kept); one that ended leaves.
fn follow_step(
    steps: &mut Vec<(String, StepAtWork)>,
    item_id: &str,
    status: ItemStatus,
    step: Option<StepAtWork>,
) {
    let at = steps.iter().position(|(id, _)| id == item_id);
    match (status, at, step) {
        (ItemStatus::InProgress, Some(at), Some(step)) => {
            let held = &mut steps[at].1;
            held.detail = step.detail.or_else(|| held.detail.take());
            held.name = step.name;
        }
        (ItemStatus::InProgress, None, Some(step)) => steps.push((item_id.to_owned(), step)),
        (ItemStatus::InProgress, _, None) => {}
        (_, Some(at), _) => {
            steps.remove(at);
        }
        (_, None, _) => {}
    }
}

/// The `record/turns.jsonl` line of `cli`'s turn: its start, or its `end` at `ended_at_ms`.
fn turn_line(cli: &WorkerCli, end: Option<TurnEnd>, ended_at_ms: Option<i64>) -> TurnRecord {
    TurnRecord {
        user_message: cli.user_message.clone(),
        end,
        error: None,
        ended_at_ms,
        requests: Vec::new(),
        limit: None,
    }
}

/// The sidebar and worker row agree on whether this worker has work under way.
fn working(task: &Task, cli: Option<&WorkerCli>) -> bool {
    cli.is_some_and(|cli| cli.open)
        && !task.state.is_final()
        && !task.plain.as_ref().is_some_and(|plain| plain.closed)
}

/// A worker's row from its task (its link to the session, its title and model, its branch),
/// its CLI and its git.
pub(crate) fn row(
    task: &Task,
    cli: Option<&WorkerCli>,
    git: Option<&WorkerGit>,
    waiting: Vec<WorkerWait>,
    mcp: Vec<brigadier_providers::McpServerState>,
    base: Option<String>,
) -> WorkerRow {
    let gone = task.state.is_final() || task.plain.as_ref().is_some_and(|plain| plain.closed);
    let cli = cli.cloned().unwrap_or_default();
    // A worker no CLI event was heard of since a restart: how its task ended.
    let last_end = cli.last_end.or(match task.state {
        TaskState::Failed => Some(TurnEnd::Failed),
        TaskState::Stopped | TaskState::Rejected => Some(TurnEnd::Stopped),
        state if state.is_final() => Some(TurnEnd::Done),
        TaskState::Reported => Some(TurnEnd::Done),
        _ => None,
    });
    let git = git.cloned().unwrap_or_default();
    let background = if gone {
        Vec::new()
    } else {
        background_of(&cli)
    };
    WorkerRow {
        id: task.id.clone(),
        number: task.number,
        title: task.title.clone(),
        agent: task.route.choice.provider,
        model: task.route.choice.model.clone(),
        started_at_ms: task.created_at_ms,
        working: working(task, Some(&cli)),
        last_end,
        gone,
        last_event_at_ms: cli.last_event_ms,
        limit: cli.limit.filter(|_| !gone),
        fresh_context: cli.fresh_context && cli.open,
        waiting: if gone { Vec::new() } else { waiting },
        mcp,
        branch: task.workspace.as_ref().and_then(|w| w.branch.clone()),
        base,
        diff: git.diff,
        ahead: git.ahead,
        merged: git.merged,
        steps: if cli.open && !gone {
            cli.steps.iter().map(|(_, step)| step.clone()).collect()
        } else {
            Vec::new()
        },
        background,
    }
}

/// `cli`'s background tasks, each with the command its call ran when its CLI said one.
fn background_of(cli: &WorkerCli) -> Vec<WorkerTask> {
    cli.background
        .iter()
        .map(|task| WorkerTask {
            task: task.clone(),
            command: task.item_id.as_ref().and_then(|item| {
                cli.commands
                    .iter()
                    .find(|(id, _)| id == item)
                    .map(|(_, command)| command.clone())
            }),
        })
        .collect()
}

impl SessionManager {
    /// What `shell` finds among worker `worker`'s background tasks: a command's output file.
    pub(super) fn worker_background_file(
        &self,
        worker: &TaskId,
        shell: impl Fn(&BackgroundTask) -> Option<String>,
    ) -> Option<String> {
        self.worker_rows
            .cli
            .lock()
            .unwrap()
            .get(worker)?
            .background
            .iter()
            .find_map(shell)
    }

    /// The sidebar's activity bit, from the same CLI state as the workers' rows, without git.
    pub(super) fn workers_working(&self, tasks: &[Task]) -> bool {
        let cli = self.worker_rows.cli.lock().unwrap();
        tasks.iter().any(|task| working(task, cli.get(&task.id)))
    }

    /// How full worker `task`'s context is, as its CLI last said, with its model's window when
    /// known; `None` before it said, and once it ended or was closed.
    pub(crate) fn worker_context(
        &self,
        task: &Task,
    ) -> Option<brigadier_cli::protocol::ContextSize> {
        if task.state.is_final() || task.plain.as_ref().is_some_and(|plain| plain.closed) {
            return None;
        }
        let (used_tokens, reported) = self
            .worker_rows
            .cli
            .lock()
            .unwrap()
            .get(&task.id)?
            .context?;
        let choice = &task.route.choice;
        Some(brigadier_cli::protocol::ContextSize {
            used_tokens,
            window_tokens: self.model_window(choice.provider, choice.model.as_deref(), reported),
        })
    }

    /// Follows an event of worker `task`'s CLI (of session `conversation`) into its row.
    /// Answers the line it adds to the worker's `record/turns.jsonl` ([`Self::note_worker_turn`]).
    pub(super) fn note_worker_cli(
        &self,
        conversation: &ConversationId,
        task: &TaskId,
        event: &ProviderEvent,
    ) -> Option<TurnRecord> {
        let (changed, line) = {
            let mut held = self.worker_rows.cli.lock().unwrap();
            let cli = held.entry(task.clone()).or_default();
            let changed = follow(cli, event, now_ms());
            (changed, cli.line.take())
        };
        let git_now = matches!(
            event,
            ProviderEvent::TurnCompleted { .. } | ProviderEvent::Exited { .. }
        );
        let git_soon = matches!(
            event,
            ProviderEvent::Command { status, .. } | ProviderEvent::ToolCall { status, .. }
                if *status != ItemStatus::InProgress
        ) || matches!(event, ProviderEvent::FileChanges { .. });
        if changed || git_now || git_soon {
            self.workers_changed(conversation, git_now);
        }
        line
    }

    /// Adds `line` to worker `task`'s `record/turns.jsonl`, as a session's thread notes its
    /// turns: a Claude worker's file says no turn's end.
    pub(super) async fn note_worker_turn(
        &self,
        conversation: &ConversationId,
        task: &TaskId,
        line: &TurnRecord,
    ) {
        if line.user_message.is_none() {
            return;
        }
        let Ok(task) = self.task_by_id(conversation, task).await else {
            return;
        };
        let record = self.session_dir(conversation).worker(&task).join("record");
        if let Err(err) = thread_view::append_turn(&record, line) {
            tracing::warn!(task = %task.id, error = %err, "could not note a worker's turn");
        }
    }

    /// Session `conversation`'s worker rows are to be sent again, soon; `git_now`: with git
    /// read again whatever its last read.
    pub(crate) fn workers_changed(&self, conversation: &ConversationId, git_now: bool) {
        let first = {
            let mut due = self.worker_rows.due.lock().unwrap();
            let first = !due.contains_key(conversation);
            *due.entry(conversation.clone()).or_default() |= git_now;
            first
        };
        if !first {
            return;
        }
        let (manager, id) = (self.arc(), conversation.clone());
        self.spawn(async move {
            tokio::time::sleep(Duration::from_millis(SETTLE_MS)).await;
            let git_now = manager
                .worker_rows
                .due
                .lock()
                .unwrap()
                .remove(&id)
                .unwrap_or_default();
            match manager.worker_rows_of(&id, git_now).await {
                Ok(rows) => manager.send_worker_rows(&id, rows),
                Err(err) => tracing::debug!(conversation = %id, error = %err, "no worker rows"),
            }
        });
    }

    fn send_worker_rows(&self, id: &ConversationId, rows: Vec<WorkerRow>) {
        {
            let mut sent = self.worker_rows.sent.lock().unwrap();
            if sent.get(id).is_some_and(|held| *held == rows)
                || (rows.is_empty() && !sent.contains_key(id))
            {
                return;
            }
            sent.insert(id.clone(), rows.clone());
        }
        let _ = self.thread_news.send(ThreadNews {
            conversation_id: id.clone(),
            entries: Vec::new(),
            opened: false,
            ended: None,
            turn_ended_at_ms: None,
            context: None,
            background: None,
            mcp: None,
            workers: Some(rows),
            jobs: None,
        });
    }

    /// Session `id`'s worker rows now, newest first. `git_now`: git is read again whatever
    /// its last read; else only when that is older than [`GIT_EVERY_MS`].
    pub(crate) async fn worker_rows_of(
        &self,
        id: &ConversationId,
        git_now: bool,
    ) -> Result<Vec<WorkerRow>> {
        let board = self.core.board(id).await?;
        if board.tasks.is_empty() {
            return Ok(Vec::new());
        }
        let (parent_checkout, parent_branch) = self
            .changes_checkout(id)
            .map_or((None, None), |(path, branch)| (Some(path), branch));
        let read_git = git_now || {
            let last = self
                .worker_rows
                .git_read_ms
                .lock()
                .unwrap()
                .get(id)
                .copied();
            last.is_none_or(|last| now_ms().saturating_sub(last) >= GIT_EVERY_MS)
        };
        if read_git {
            self.worker_rows
                .git_read_ms
                .lock()
                .unwrap()
                .insert(id.clone(), now_ms());
            self.read_workers_git(&board.tasks, parent_branch.clone(), parent_checkout)
                .await;
        }
        let mut waiting: HashMap<&TaskId, Vec<WorkerWait>> = HashMap::new();
        let mut approvals: Vec<_> = board.approvals.values().collect();
        approvals.sort_by_key(|card| card.position);
        for card in approvals {
            if let (Some(task), CardState::Pending, ApprovalSubject::Cli { request }) =
                (&card.task_id, &card.state, &card.subject)
            {
                waiting.entry(task).or_default().push(WorkerWait {
                    kind: WorkerWaitKind::Permission,
                    what: request
                        .command
                        .clone()
                        .unwrap_or_else(|| request.tool.clone()),
                    card: Some(card.id.clone()),
                });
            }
        }
        let mut questions: Vec<_> = board.questions.values().collect();
        questions.sort_by_key(|card| card.position);
        for card in questions {
            if let Some(task) = &card.task_id
                && card.answer.is_none()
                && card.answered_at_ms.is_none()
            {
                waiting.entry(task).or_default().push(WorkerWait {
                    kind: WorkerWaitKind::Question,
                    what: card.text.lines().next().unwrap_or_default().to_owned(),
                    card: Some(card.id.clone()),
                });
            }
        }
        let cli = self.worker_rows.cli.lock().unwrap().clone();
        let git = self.worker_rows.git.lock().unwrap().clone();
        let mut tasks: Vec<&Task> = board.tasks.values().collect();
        tasks.sort_by_key(|task| std::cmp::Reverse(task.created_at_ms));
        Ok(tasks
            .into_iter()
            .map(|task| {
                let mcp = board
                    .mcp_servers
                    .get(&Some(task.id.0.clone()))
                    .map(|(_, agent)| agent.servers.clone())
                    .unwrap_or_default();
                let mut waits = waiting.remove(&task.id).unwrap_or_default();
                waits.extend(
                    mcp.iter()
                        .filter(|server| server.status == McpStatus::NeedsAuth)
                        .map(|server| WorkerWait {
                            kind: WorkerWaitKind::SignIn,
                            what: server.name.clone(),
                            card: None,
                        }),
                );
                let base = task
                    .plain
                    .as_ref()
                    .and_then(|plain| plain.base.clone())
                    .or_else(|| parent_branch.clone());
                row(task, cli.get(&task.id), git.get(&task.id), waits, mcp, base)
            })
            .collect())
    }

    /// Reads each worker's work against its recorded base branch in git: its +/− (what it
    /// changed, committed or not), its commits ahead and whether they all merged. A worker whose
    /// worktree is gone (closed) is read from the session's checkout `parent_checkout`, which
    /// shares its branches: its commits ahead and whether they merged, with no +/−.
    async fn read_workers_git(
        &self,
        tasks: &HashMap<TaskId, Task>,
        parent: Option<String>,
        parent_checkout: Option<PathBuf>,
    ) {
        let checkouts: Vec<_> = tasks
            .values()
            .filter(|task| !task.state.is_final() || task.plain.is_some())
            .filter_map(|task| {
                let workspace = task.workspace.as_ref()?;
                Some((
                    task.id.clone(),
                    PathBuf::from(workspace.worktree.as_ref()?),
                    workspace.branch.clone(),
                    workspace.base.clone(),
                    workspace.target.clone().or_else(|| parent.clone())?,
                    task.plain
                        .as_ref()
                        .and_then(|plain| plain.repo.clone())
                        .map(PathBuf::from)
                        .or_else(|| parent_checkout.clone()),
                ))
            })
            .collect();
        if checkouts.is_empty() {
            return;
        }
        let git = self.git.clone();
        let read = blocking(move || {
            Ok(checkouts
                .into_iter()
                .map(|(task, path, branch, start, parent, fallback)| {
                    let (repo, own) = match git.open(&path) {
                        Ok(repo) => (repo, true),
                        Err(_) => match fallback.as_ref().map(|dir| git.open(dir)) {
                            Some(Ok(repo)) if !path.exists() => (repo, false),
                            _ => return (task, None),
                        },
                    };
                    let diff = own
                        .then(|| repo.worker_diff(&parent).ok())
                        .flatten()
                        .map(|stat| super::diff_stat_of(&stat));
                    let (ahead, merged) = branch
                        .as_deref()
                        .and_then(|branch| {
                            let tip = repo.branch_tip(branch).ok()??;
                            let into = repo.branch_tip(&parent).ok()??;
                            let ahead = repo.count_commits(&into, &tip).unwrap_or(0);
                            let moved = start.as_deref().is_none_or(|start| start != tip.0);
                            let merged = moved && repo.ancestor(&tip, &into).unwrap_or(false);
                            Some((ahead, merged))
                        })
                        .unwrap_or_default();
                    (
                        task,
                        Some(WorkerGit {
                            diff,
                            ahead,
                            merged,
                        }),
                    )
                })
                .collect::<Vec<_>>())
        })
        .await
        .unwrap_or_default();
        let mut held = self.worker_rows.git.lock().unwrap();
        for (task, git) in read {
            // A worktree being set up or removed keeps its last read.
            if let Some(git) = git {
                held.insert(task, git);
            }
        }
    }

    /// Whether worker `task`'s CLI is idle after a turn that ended done, as its events tell.
    pub(crate) fn worker_turn_done(&self, task: &TaskId) -> bool {
        self.worker_rows
            .cli
            .lock()
            .unwrap()
            .get(task)
            .is_some_and(|cli| !cli.open && cli.last_end == Some(TurnEnd::Done))
    }

    /// Drops every worker's CLI and git reads held in memory, as a daemon restart does.
    #[cfg(all(test, unix))]
    pub(crate) fn forget_worker_reads(&self) {
        self.worker_rows.cli.lock().unwrap().clear();
        self.worker_rows.git.lock().unwrap().clear();
    }

    /// Worker `task` of session `id`'s thread, as its CLI keeps it: its CLI session files, read
    /// back (S14, G19.Q6: read-only).
    pub async fn worker_thread(
        &self,
        id: &ConversationId,
        task: &TaskId,
    ) -> Result<crate::thread_view::ThreadView> {
        let task = self.task_by_id(id, task).await?;
        let record = self.session_dir(id).worker(&task).join("record");
        let files = super::session_dir::read_lineage(&record)
            .into_iter()
            .filter_map(|session| {
                let file = session.file?;
                Some(
                    self.session_files
                        .read(session.provider, std::path::Path::new(&file)),
                )
            })
            .collect();
        // Its turn that is open reads as running, not cut off.
        let open = self
            .worker_rows
            .cli
            .lock()
            .unwrap()
            .get(&task.id)
            .is_some_and(|cli| cli.open);
        let live = open.then(|| thread_view::LiveTurn {
            open: true,
            ..Default::default()
        });
        let records = thread_view::read_turns(&record);
        let mut view = thread_view::view(files, live.as_ref(), &records);
        // Its hand-offs to fresh CLI sessions (D14): a divider each, the nudge and the
        // hand-off hidden, as a session's.
        view.seams = super::seam::marks(&record, &mut view.entries);
        Ok(view)
    }

    /// Whether session `id` has had worker rows sent.
    pub(crate) fn has_worker_rows(&self, id: &ConversationId) -> bool {
        self.worker_rows
            .sent
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|rows| !rows.is_empty())
    }

    /// Forgets the live state of a deleted session's workers.
    pub(crate) fn forget_worker_rows(&self, conv: &ConversationId, tasks: &[Task]) {
        let tasks: HashSet<&TaskId> = tasks.iter().map(|task| &task.id).collect();
        let rows = &self.worker_rows;
        rows.cli
            .lock()
            .unwrap()
            .retain(|task, _| !tasks.contains(task));
        rows.git
            .lock()
            .unwrap()
            .retain(|task, _| !tasks.contains(task));
        rows.due.lock().unwrap().remove(conv);
        rows.git_read_ms.lock().unwrap().remove(conv);
        rows.sent.lock().unwrap().remove(conv);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brigadier_providers::{ErrorKind, LimitKind, ProviderError};

    fn error(kind: ErrorKind, limit: Option<LimitHit>) -> ProviderEvent {
        ProviderEvent::Error {
            error: ProviderError {
                kind,
                message: "stopped".into(),
                will_retry: false,
                limit,
                code: None,
            },
        }
    }

    fn turn_end(status: TurnStatus) -> ProviderEvent {
        ProviderEvent::TurnCompleted {
            status,
            usage: None,
            duration_ms: None,
            turn_id: None,
        }
    }

    #[test]
    fn a_turn_opens_and_ends_as_its_cli_says() {
        let mut cli = WorkerCli::default();
        assert!(follow(
            &mut cli,
            &ProviderEvent::TurnStarted { turn_id: None },
            1
        ));
        assert!(cli.open);
        assert!(follow(&mut cli, &turn_end(TurnStatus::Completed), 2));
        assert_eq!((cli.open, cli.last_end), (false, Some(TurnEnd::Done)));
        follow(&mut cli, &ProviderEvent::TurnStarted { turn_id: None }, 3);
        follow(&mut cli, &turn_end(TurnStatus::Interrupted), 4);
        assert_eq!(cli.last_end, Some(TurnEnd::Stopped));
        follow(&mut cli, &ProviderEvent::TurnStarted { turn_id: None }, 5);
        follow(&mut cli, &error(ErrorKind::Overloaded, None), 6);
        follow(&mut cli, &turn_end(TurnStatus::Completed), 7);
        assert_eq!(cli.last_end, Some(TurnEnd::Failed));
    }

    fn command(id: &str, command: &str, status: ItemStatus) -> ProviderEvent {
        ProviderEvent::Command {
            item_id: id.into(),
            command: command.into(),
            cwd: None,
            status,
            exit_code: None,
            output: None,
            duration_ms: None,
            kept: None,
        }
    }

    fn tool(id: &str, input: Option<&str>, status: ItemStatus) -> ProviderEvent {
        ProviderEvent::ToolCall {
            item_id: id.into(),
            name: "Edit".into(),
            input: input.map(str::to_owned),
            status,
            output: None,
            kept: None,
        }
    }

    fn steps(cli: &WorkerCli) -> Vec<(&str, Option<&str>)> {
        cli.steps
            .iter()
            .map(|(_, step)| (step.name.as_str(), step.detail.as_deref()))
            .collect()
    }

    /// The calls a worker has at work, as its row names them: each joins as it starts, keeps
    /// the detail its start said, and leaves as it ends; a turn's end leaves none.
    #[test]
    fn a_workers_steps_at_work_follow_its_calls() {
        let mut cli = WorkerCli::default();
        follow(&mut cli, &ProviderEvent::TurnStarted { turn_id: None }, 1);
        assert!(follow(
            &mut cli,
            &command("c1", "pnpm build", ItemStatus::InProgress),
            2
        ));
        assert!(follow(
            &mut cli,
            &tool(
                "t1",
                Some(r#"{"file_path":"/w/foo.ts","old_string":"a"}"#),
                ItemStatus::InProgress
            ),
            3
        ));
        assert_eq!(
            steps(&cli),
            [("shell", Some("pnpm build")), ("Edit", Some("/w/foo.ts"))]
        );
        assert!(
            !follow(&mut cli, &tool("t1", None, ItemStatus::InProgress), 4),
            "nothing new"
        );
        assert!(follow(
            &mut cli,
            &tool("t1", None, ItemStatus::Completed),
            5
        ));
        assert_eq!(steps(&cli), [("shell", Some("pnpm build"))]);
        follow(&mut cli, &turn_end(TurnStatus::Completed), 6);
        assert!(cli.steps.is_empty());
    }

    /// A worker's background tasks follow its CLI's word, each with the command its call ran,
    /// and end with its CLI.
    #[test]
    fn a_workers_background_tasks_follow_its_cli() {
        use brigadier_providers::{BackgroundTaskKind, BackgroundTaskStatus};
        let mut cli = WorkerCli::default();
        follow(&mut cli, &ProviderEvent::TurnStarted { turn_id: None }, 1);
        let call = ProviderEvent::ToolCall {
            item_id: "b1".into(),
            name: "Bash".into(),
            input: Some(r#"{"command":"pnpm tauri:dev","run_in_background":true}"#.into()),
            status: ItemStatus::Completed,
            output: None,
            kept: None,
        };
        follow(&mut cli, &call, 2);
        let task = |status| BackgroundTask {
            id: "t1".into(),
            item_id: Some("b1".into()),
            kind: BackgroundTaskKind::Shell,
            label: "Run the dev app".into(),
            status,
            started_at_ms: 3,
            ended_at_ms: None,
            output_file: Some("/tmp/t1.output".into()),
            summary: None,
            last_line: None,
        };
        assert!(follow(
            &mut cli,
            &ProviderEvent::BackgroundTask {
                task: task(BackgroundTaskStatus::Running)
            },
            3
        ));
        assert_eq!(
            background_of(&cli),
            [WorkerTask {
                task: task(BackgroundTaskStatus::Running),
                command: Some("pnpm tauri:dev".into()),
            }]
        );
        follow(&mut cli, &turn_end(TurnStatus::Completed), 4);
        assert_eq!(background_of(&cli).len(), 1, "it outlives the turn");
        follow(
            &mut cli,
            &ProviderEvent::Exited {
                code: Some(0),
                signal: None,
                stderr_tail: None,
            },
            5,
        );
        assert!(background_of(&cli).is_empty());
    }

    /// Claude's background `Bash` is a `Command` item, its task labelled with its description:
    /// the row still knows what it runs, so its wait has no row (D38). A task's command outlives
    /// the many calls after it.
    #[test]
    fn a_claude_workers_background_bash_keeps_its_command() {
        use brigadier_providers::{BackgroundTaskKind, BackgroundTaskStatus};
        let mut cli = WorkerCli::default();
        follow(&mut cli, &ProviderEvent::TurnStarted { turn_id: None }, 1);
        let bash = |id: &str, command: &str| ProviderEvent::Command {
            item_id: id.into(),
            command: command.into(),
            cwd: None,
            status: ItemStatus::InProgress,
            exit_code: None,
            output: None,
            duration_ms: None,
            kept: None,
        };
        follow(&mut cli, &bash("toolu_wait", "brigadier wait"), 2);
        let task = BackgroundTask {
            id: "b8ea764yr".into(),
            item_id: Some("toolu_wait".into()),
            kind: BackgroundTaskKind::Shell,
            label: "Wait for the next worker event".into(),
            status: BackgroundTaskStatus::Running,
            started_at_ms: 3,
            ended_at_ms: None,
            output_file: None,
            summary: None,
            last_line: None,
        };
        follow(&mut cli, &ProviderEvent::BackgroundTask { task }, 3);
        for at in 0..2 * COMMANDS_KEPT {
            follow(&mut cli, &bash(&format!("c{at}"), "pnpm test"), 4);
        }
        assert_eq!(
            background_of(&cli)[0].command.as_deref(),
            Some("brigadier wait")
        );
        assert!(cli.commands.len() <= COMMANDS_KEPT + 1);
    }

    /// A worker's context size is kept for `brigadier workers` (run2-D42) without sending its
    /// rows again: they don't show it.
    #[test]
    fn a_workers_context_size_is_kept_without_changing_its_row() {
        let mut cli = WorkerCli::default();
        follow(&mut cli, &ProviderEvent::TurnStarted { turn_id: None }, 1);
        let size = |used| ProviderEvent::ContextSize {
            used_tokens: used,
            window_tokens: Some(1_000_000),
        };
        assert!(!follow(&mut cli, &size(120_000), 2));
        assert_eq!(cli.context, Some((120_000, Some(1_000_000))));
        assert!(!follow(&mut cli, &size(130_000), 3));
        assert_eq!(cli.context, Some((130_000, Some(1_000_000))));
    }

    fn said(id: &str) -> ProviderEvent {
        ProviderEvent::Message {
            item_id: id.into(),
            role: Role::User,
            text: "…".into(),
        }
    }

    /// The lines of a worker's `record/turns.jsonl`: its turn's start once its message came
    /// (after its start, as Codex says it, or as it opens, as Claude does), and its end (#556).
    #[test]
    fn a_turn_notes_its_start_and_end_for_its_record() {
        let mut cli = WorkerCli::default();
        follow(&mut cli, &said("u1"), 1);
        let start = cli.line.take().expect("its start");
        assert_eq!(
            (start.user_message.as_deref(), start.end),
            (Some("u1"), None)
        );
        follow(&mut cli, &said("steer"), 2);
        assert_eq!(cli.line, None, "a steer starts no turn");
        follow(&mut cli, &turn_end(TurnStatus::Completed), 3);
        let end = cli.line.take().expect("its end");
        assert_eq!(
            (end.user_message.as_deref(), end.end, end.ended_at_ms),
            (Some("u1"), Some(TurnEnd::Done), Some(3))
        );
        follow(&mut cli, &ProviderEvent::TurnStarted { turn_id: None }, 4);
        assert_eq!(cli.line, None);
        assert!(!follow(&mut cli, &said("u2"), 5), "nothing the row shows");
        assert_eq!(cli.line.take().unwrap().user_message.as_deref(), Some("u2"));
        follow(
            &mut cli,
            &ProviderEvent::Exited {
                code: Some(1),
                signal: None,
                stderr_tail: None,
            },
            6,
        );
        let end = cli.line.take().expect("its end");
        assert_eq!(
            (end.user_message.as_deref(), end.end),
            (Some("u2"), Some(TurnEnd::Interrupted))
        );
    }

    #[test]
    fn a_limit_and_a_compaction_hold_until_the_next_turn() {
        let mut cli = WorkerCli::default();
        follow(&mut cli, &ProviderEvent::TurnStarted { turn_id: None }, 1);
        follow(
            &mut cli,
            &ProviderEvent::CompactionStarted { automatic: true },
            2,
        );
        assert!(cli.fresh_context);
        let limit = LimitHit {
            window: Some("five_hour".into()),
            resets_at_ms: Some(9_000),
            kind: LimitKind::UsageWindow,
        };
        follow(
            &mut cli,
            &error(ErrorKind::UsageLimit, Some(limit.clone())),
            3,
        );
        follow(&mut cli, &turn_end(TurnStatus::Failed), 4);
        assert_eq!(cli.limit, Some(limit));
        follow(&mut cli, &ProviderEvent::TurnStarted { turn_id: None }, 5);
        assert_eq!((cli.limit.clone(), cli.fresh_context), (None, false));
    }

    #[test]
    fn a_cli_that_ends_mid_turn_reads_interrupted() {
        let mut cli = WorkerCli::default();
        follow(&mut cli, &ProviderEvent::TurnStarted { turn_id: None }, 1);
        follow(
            &mut cli,
            &ProviderEvent::Exited {
                code: Some(1),
                signal: None,
                stderr_tail: None,
            },
            2,
        );
        assert_eq!(
            (cli.open, cli.last_end),
            (false, Some(TurnEnd::Interrupted))
        );
    }

    #[test]
    fn chatter_is_sent_again_only_once_it_went_stale() {
        let mut cli = WorkerCli::default();
        follow(
            &mut cli,
            &ProviderEvent::TurnStarted { turn_id: None },
            1_000,
        );
        let progress = ProviderEvent::Notice {
            level: brigadier_providers::NoticeLevel::Info,
            message: "…".into(),
        };
        assert!(!follow(&mut cli, &progress, 2_000));
        assert!(follow(&mut cli, &progress, 1_000 + EVENT_STALE_MS));
        assert_eq!(cli.last_event_ms, Some(1_000 + EVENT_STALE_MS));
    }
}
