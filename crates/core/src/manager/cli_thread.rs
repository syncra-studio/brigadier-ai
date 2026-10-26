//! The thread from the CLI's own files (S9, [`crate::thread_view`]): the live stream's items
//! of the running turn, the turn ends written to `record/turns.jsonl`, and the view read back.

use brigadier_providers::session_file::{self, FileEntry, FileImage};
use brigadier_providers::{
    BackgroundTask, BackgroundTaskKind, BackgroundTaskStatus, ProviderEvent, Role, TurnStatus,
};

use super::SessionManager;
use super::conversation::{BACKGROUND_TURN_GRACE, ConvLive};
use super::session_dir::read_lineage;
use crate::model::{ContextUsage, ConversationId, ConversationView};
use crate::thread_view::{self, CliTurn, LiveTurn, ThreadNews, ThreadView, TurnEnd, TurnRecord};
use crate::work::{ConversationActivity, TaskId};
use crate::{Error, Result, now_ms};

impl SessionManager {
    /// Follows the running turn of `conv`'s CLI through one of its events (`None`: the CLI's
    /// events ended): its items, and its end, written to `record/turns.jsonl`.
    pub(super) async fn follow_cli_thread(&self, conv: &ConvLive, event: Option<&ProviderEvent>) {
        let at = now_ms();
        // Brigadier's requests the turn carries, as the conversation knows them now: when the
        // CLI says a user message (a turn's, or a steer's) and when the turn ends.
        let carried = match event {
            Some(
                ProviderEvent::Message {
                    role: Role::User, ..
                }
                | ProviderEvent::TurnCompleted { .. }
                | ProviderEvent::Exited { .. },
            )
            | None => Some(conv.carried_requests().await),
            _ => None,
        };
        let mut news = Vec::new();
        let mut ended = None;
        let mut opened = false;
        let mut context = None;
        let mut noted = None;
        let mut background = None;
        let mut mcp = None;
        let mut read_output = false;
        let jobs;
        let mut wake_owed = None;
        {
            let mut threads = self.cli_threads.lock().unwrap();
            let live = threads.entry(conv.id.clone()).or_default();
            if let Some(event) = event {
                live.note_command(event);
            }
            match event {
                Some(ProviderEvent::BackgroundTask { task }) => {
                    // A command that ends while no turn is open starts one of its CLI's: it is
                    // owed until it starts, or the grace passes (D21).
                    let ended = task.status != BackgroundTaskStatus::Running
                        && task.kind != BackgroundTaskKind::Agent
                        && live.background.iter().any(|held| {
                            held.id == task.id && held.status == BackgroundTaskStatus::Running
                        });
                    background = Some(update_background(&mut live.background, task, at));
                    if ended && !live.open {
                        let until = at
                            + i64::try_from(BACKGROUND_TURN_GRACE.as_millis()).unwrap_or(i64::MAX);
                        live.waking_until = Some(until);
                        wake_owed = Some(until);
                    }
                    read_output = !live.reading_output && reads_output(&live.background);
                    live.reading_output |= read_output;
                }
                Some(ProviderEvent::McpServers { servers, .. }) => {
                    live.mcp.clone_from(servers);
                    mcp = Some(live.mcp.clone());
                }
                Some(ProviderEvent::McpServerChanged { server }) => {
                    match live.mcp.iter_mut().find(|held| held.name == server.name) {
                        Some(held) => *held = server.clone(),
                        None => live.mcp.push(server.clone()),
                    }
                    mcp = Some(live.mcp.clone());
                }
                // The CLI's background tasks end with it.
                Some(ProviderEvent::Exited { .. }) | None if !live.background.is_empty() => {
                    live.background.clear();
                    live.waking_until = None;
                    background = Some(Vec::new());
                }
                _ => {}
            }
            // So do its MCP servers.
            if matches!(event, Some(ProviderEvent::Exited { .. }) | None) && !live.mcp.is_empty() {
                live.mcp.clear();
                mcp = Some(Vec::new());
            }
            // A turn the CLI began on its own for a background job's end: its notice, or the
            // first item of its own before any user message (Claude's stream doesn't say the
            // notice its file keeps). It goes on with the turn before, which reads as open until
            // it ends, and its end is that turn's; a message the user sends meanwhile is a steer
            // of that turn, its request one more of that turn's. A compaction is no such item:
            // Claude says `/compact`'s (or the message it compacted for) only after it, and
            // that message starts the turn.
            let on_its_own = live.open
                && live.awaiting_user
                && event.is_some_and(|event| {
                    shown_live(event)
                        && !matches!(
                            event,
                            ProviderEvent::Message {
                                role: Role::User,
                                ..
                            } | ProviderEvent::CompactionEnded { .. }
                        )
                });
            if on_its_own {
                live.awaiting_user = false;
                live.continues = true;
                live.waking_until = None;
                // The thread reads its turns again: that turn is open once more.
                opened = true;
                if live.user_message.is_none() {
                    live.user_message =
                        thread_view::last_user_message(&self.session_dir(&conv.id).record());
                }
            }
            match event {
                Some(ProviderEvent::TurnStarted { .. }) => {
                    if !live.open {
                        opened = true;
                        *live = LiveTurn {
                            open: true,
                            awaiting_user: true,
                            // The turn before's, until its own user message comes: a turn with
                            // none goes on with that turn, as the thread reads it.
                            user_message: live.user_message.take(),
                            background: std::mem::take(&mut live.background),
                            mcp: std::mem::take(&mut live.mcp),
                            commands: std::mem::take(&mut live.commands),
                            jobs_said: std::mem::take(&mut live.jobs_said),
                            reading_output: live.reading_output,
                            context_window: live.context_window,
                            ..LiveTurn::default()
                        };
                    }
                }
                Some(ProviderEvent::Message {
                    item_id,
                    role: Role::User,
                    ..
                }) => {
                    let steer = live.open && !live.awaiting_user;
                    if !live.open {
                        opened = true;
                        *live = LiveTurn {
                            open: true,
                            background: std::mem::take(&mut live.background),
                            mcp: std::mem::take(&mut live.mcp),
                            commands: std::mem::take(&mut live.commands),
                            jobs_said: std::mem::take(&mut live.jobs_said),
                            reading_output: live.reading_output,
                            ..LiveTurn::default()
                        };
                    }
                    if !steer {
                        live.awaiting_user = false;
                        live.user_message = Some(item_id.clone());
                    }
                    if let Some(carried) = &carried {
                        thread_view::add_requests(&mut live.requests, carried);
                        noted = Some(TurnRecord {
                            user_message: live.user_message.clone(),
                            end: None,
                            error: None,
                            ended_at_ms: None,
                            requests: live.requests.clone(),
                            limit: None,
                            context_window: None,
                        });
                    }
                    let entry = FileEntry {
                        steer,
                        ..entry(event.cloned(), at)
                    };
                    news.push(entry);
                }
                Some(ProviderEvent::Error { error }) if live.open => {
                    live.error = Some(error.message.clone());
                    if let Some(limit) = thread_view::limit_of(error) {
                        live.limit = Some(limit);
                    }
                    news.push(entry(event.cloned(), at));
                }
                Some(ProviderEvent::TurnCompleted { status, .. }) if live.open => {
                    let end = match status {
                        TurnStatus::Completed if live.error.is_some() => TurnEnd::Failed,
                        TurnStatus::Completed => TurnEnd::Done,
                        TurnStatus::Interrupted => TurnEnd::Stopped,
                        TurnStatus::Failed => TurnEnd::Failed,
                    };
                    ended = Some(end);
                }
                Some(ProviderEvent::Exited { .. }) | None if live.open => {
                    ended = Some(TurnEnd::Interrupted);
                }
                Some(ProviderEvent::ContextSize {
                    used_tokens,
                    window_tokens,
                }) => {
                    if window_tokens.is_some() {
                        live.context_window = *window_tokens;
                    }
                    context = Some(ContextUsage {
                        used_tokens: *used_tokens,
                        window_tokens: *window_tokens,
                    });
                }
                Some(event) if shown_live(event) => news.push(entry(Some(event.clone()), at)),
                _ => {}
            }
            if !news.is_empty() {
                live.entries = session_file::merge(std::mem::take(&mut live.entries), news.clone());
            }
            if let Some(end) = ended {
                live.open = false;
                if let Some(carried) = &carried {
                    thread_view::add_requests(&mut live.requests, carried);
                }
                noted = Some(TurnRecord {
                    user_message: live.user_message.clone(),
                    end: Some(end),
                    error: live.error.take(),
                    ended_at_ms: Some(at),
                    requests: live.requests.clone(),
                    limit: live.limit.take(),
                    context_window: live.context_window,
                });
            }
            if let Some(record) = &noted
                && record.user_message.is_some()
            {
                let dir = self.session_dir(&conv.id).record();
                if let Err(err) = thread_view::append_turn(&dir, record) {
                    crate::logging::private_trace!(conv.private_logs, warn, "could not note a turn"; conversation = %conv.id, error = %err);
                }
            }
            jobs = said_jobs(live, at);
        }
        if !news.is_empty()
            || opened
            || ended.is_some()
            || context.is_some()
            || background.is_some()
            || mcp.is_some()
            || jobs.is_some()
        {
            let _ = self.thread_news.send(ThreadNews {
                conversation_id: conv.id.clone(),
                entries: news,
                opened,
                ended,
                turn_ended_at_ms: ended.map(|_| at),
                context,
                background,
                mcp,
                workers: None,
                jobs,
            });
        }
        if let Some(until) = wake_owed {
            self.wake_grace(conv.id.clone(), until);
        }
        // A command of the session may merge a worker's branch, or move its own.
        if matches!(
            event,
            Some(ProviderEvent::Command { status, .. }) if *status != brigadier_providers::ItemStatus::InProgress
        ) && self.has_worker_rows(&conv.id)
        {
            self.workers_changed(&conv.id, true);
        }
        if read_output {
            let manager = self.arc();
            let id = conv.id.clone();
            tokio::spawn(async move { manager.follow_task_output(id).await });
        }
    }

    /// Follows the last line each running background command of conversation `id` prints,
    /// from its output file, while it has one: its row shows it, as the CLI's own does.
    async fn follow_task_output(&self, id: ConversationId) {
        loop {
            tokio::time::sleep(LAST_LINE_EVERY).await;
            let files: Vec<(String, String)> = {
                let mut threads = self.cli_threads.lock().unwrap();
                let Some(live) = threads.get_mut(&id) else {
                    return;
                };
                if !reads_output(&live.background) {
                    live.reading_output = false;
                    return;
                }
                live.background
                    .iter()
                    .filter(|task| reads_output(std::slice::from_ref(*task)))
                    .filter_map(|task| Some((task.id.clone(), task.output_file.clone()?)))
                    .collect()
            };
            let lines: Vec<(String, Option<String>)> = files
                .into_iter()
                .map(|(task, file)| (task, last_line(std::path::Path::new(&file))))
                .collect();
            let changed = {
                let mut threads = self.cli_threads.lock().unwrap();
                let Some(live) = threads.get_mut(&id) else {
                    return;
                };
                let mut changed = false;
                for (task, line) in lines {
                    if let Some(task) = live.background.iter_mut().find(|held| held.id == task)
                        && line.is_some()
                        && task.last_line != line
                    {
                        task.last_line = line;
                        changed = true;
                    }
                }
                changed.then(|| live.background.clone())
            };
            if let Some(background) = changed {
                let _ = self.thread_news.send(ThreadNews {
                    conversation_id: id.clone(),
                    entries: Vec::new(),
                    opened: false,
                    ended: None,
                    turn_ended_at_ms: None,
                    context: None,
                    background: Some(background),
                    mcp: None,
                    workers: None,
                    jobs: None,
                });
            }
        }
    }

    /// The turn a background command's end owes conversation `id` (`CliJobs::waking`) that
    /// hasn't started by `until` won't: its sidebar row and the turn that started the command
    /// stop waiting for it.
    fn wake_grace(&self, id: ConversationId, until: i64) {
        let manager = self.arc();
        tokio::spawn(async move {
            let wait = until.saturating_sub(now_ms()).max(0);
            tokio::time::sleep(std::time::Duration::from_millis(wait.unsigned_abs())).await;
            let jobs = {
                let mut threads = manager.cli_threads.lock().unwrap();
                let Some(live) = threads.get_mut(&id) else {
                    return;
                };
                if live.waking_until != Some(until) {
                    return;
                }
                live.waking_until = None;
                said_jobs(live, now_ms())
            };
            if let Some(jobs) = jobs {
                let _ = manager.thread_news.send(ThreadNews {
                    conversation_id: id,
                    entries: Vec::new(),
                    opened: false,
                    ended: None,
                    turn_ended_at_ms: None,
                    context: None,
                    background: None,
                    mcp: None,
                    workers: None,
                    jobs: Some(jobs),
                });
            }
        });
    }

    /// Where conversation `id`'s CLI turn stands: open now, else how its last turn ended.
    pub(crate) fn cli_turn(&self, id: &ConversationId) -> CliTurn {
        let open = self
            .cli_threads
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|live| live.open);
        let last = thread_view::last_turn(&self.session_dir(id).record());
        CliTurn {
            open,
            last_end: last.as_ref().and_then(|turn| turn.end),
            turn_ended_at_ms: last
                .filter(|turn| turn.end.is_some())
                .and_then(|turn| turn.ended_at_ms),
        }
    }

    /// What runs and what waits for the user, per conversation ([`crate::Core::activity`]),
    /// with each one's CLI turn.
    pub async fn activity(&self) -> Vec<ConversationActivity> {
        let turns = self
            .core
            .activity_ids()
            .await
            .into_iter()
            .map(|id| {
                let turn = self.cli_turn(&id);
                (id, turn)
            })
            .collect();
        let mut activity = self.core.activity(&turns).await;
        for entry in &mut activity {
            if let Ok(tasks) = self.core.tasks(&entry.conversation_id).await {
                entry.workers_working = self.workers_working(&tasks);
            }
            entry.commands_working = self
                .cli_threads
                .lock()
                .unwrap()
                .get(&entry.conversation_id)
                .is_some_and(|live| !live.jobs(now_ms()).is_empty());
        }
        activity
    }

    /// Conversation `id`'s thread as its CLI keeps it, with its workers' rows.
    pub async fn thread_view_with_workers(&self, id: &ConversationId) -> Result<ThreadView> {
        let mut view = self.thread_view(id)?;
        view.workers = self.worker_rows_of(id, false).await?;
        Ok(view)
    }

    /// Everything conversation `id`'s view shows, with how full its context is as its CLI's
    /// own file says (#540).
    pub async fn conversation_view(
        &self,
        id: ConversationId,
        limit: u32,
    ) -> Result<ConversationView> {
        let mut view = self.core.conversation_view(id.clone(), limit).await?;
        view.context = self.context_usage(&id).await;
        Ok(view)
    }

    /// How full conversation `id`'s context is: what its newest CLI session's file last said is
    /// in use, with the window that file names (Codex's) or else the one the newest turn in
    /// `record/turns.jsonl` noted (Claude's) (#540).
    pub(crate) async fn context_usage(&self, id: &ConversationId) -> Option<ContextUsage> {
        let record = self.session_dir(id).record();
        super::blocking(move || Ok(context_in(&record)))
            .await
            .ok()
            .flatten()
    }

    /// Conversation `id`'s thread as its CLI keeps it.
    pub fn thread_view(&self, id: &ConversationId) -> Result<ThreadView> {
        self.core.conversation(id)?;
        let record = self.session_dir(id).record();
        let files = read_lineage(&record)
            .into_iter()
            .filter_map(|session| {
                let file = session.file?;
                Some(
                    self.session_files
                        .read(session.provider, std::path::Path::new(&file)),
                )
            })
            .collect();
        let records = thread_view::read_turns(&record);
        let threads = self.cli_threads.lock().unwrap();
        let mut view = thread_view::view(files, threads.get(id), &records);
        view.seams = super::seam::marks(&record, &mut view.entries);
        Ok(view)
    }

    /// An image a CLI session file of conversation `id` holds.
    /// Base64, with its media type.
    pub fn thread_image(&self, id: &ConversationId, image: &FileImage) -> Result<(String, String)> {
        let ours = read_lineage(&self.session_dir(id).record())
            .iter()
            .filter_map(|session| session.file.as_deref())
            .any(|file| {
                let file = std::path::Path::new(file);
                let image = std::path::Path::new(&image.file);
                image == file || image.starts_with(file.with_extension("").join("subagents"))
            });
        if !ours {
            return Err(Error::NotFound(format!("image in {}", image.file)));
        }
        use base64::Engine as _;
        let bytes =
            session_file::read_image(image).map_err(|err| Error::Invalid(err.to_string()))?;
        Ok((
            base64::engine::general_purpose::STANDARD.encode(bytes),
            image.mime.clone(),
        ))
    }

    /// Stops background task `task` of conversation `id`'s CLI, or of its worker `worker`'s,
    /// as the CLI's own Stop does.
    pub async fn stop_background_task(
        &self,
        id: &ConversationId,
        worker: Option<&TaskId>,
        task: String,
    ) -> Result<()> {
        let cli = match worker {
            None => self.conv(id)?.live_cli().await,
            Some(worker) => match self.existing_task_live(worker) {
                Some(live) if live.conversation_id == *id => live.cli().await,
                _ => None,
            },
        };
        let cli = cli.ok_or_else(|| Error::Invalid("its CLI isn't running".into()))?;
        cli.session
            .stop_task(task)
            .await
            .map_err(|err| Error::Provider(err.to_string()))
    }

    /// What background command `task` of conversation `id`'s CLI, or of its worker `worker`'s,
    /// printed so far: the end of its output file, as much as a row's output shows.
    pub fn background_task_output(
        &self,
        id: &ConversationId,
        worker: Option<&TaskId>,
        task: &str,
    ) -> Result<String> {
        use std::io::{Read, Seek, SeekFrom};
        let shell = |held: &BackgroundTask| {
            (held.id == task && held.kind == BackgroundTaskKind::Shell)
                .then(|| held.output_file.clone())
                .flatten()
        };
        let file = match worker {
            None => self
                .cli_threads
                .lock()
                .unwrap()
                .get(id)
                .and_then(|live| live.background.iter().find_map(shell)),
            Some(worker) => self.worker_background_file(worker, shell),
        }
        .ok_or_else(|| Error::NotFound(format!("output of background task {task}")))?;
        let mut file = std::fs::File::open(&file).map_err(|err| Error::Invalid(err.to_string()))?;
        let len = file
            .metadata()
            .map_err(|err| Error::Invalid(err.to_string()))?
            .len();
        file.seek(SeekFrom::Start(len.saturating_sub(OUTPUT_SHOWN)))
            .map_err(|err| Error::Invalid(err.to_string()))?;
        let mut tail = Vec::new();
        file.read_to_end(&mut tail)
            .map_err(|err| Error::Invalid(err.to_string()))?;
        Ok(String::from_utf8_lossy(&tail).into_owned())
    }

    /// What live streams add to threads, as it comes.
    pub fn subscribe_thread_news(&self) -> tokio::sync::broadcast::Receiver<ThreadNews> {
        self.thread_news.subscribe()
    }
}

/// How often a running background command's last output line is read again.
const LAST_LINE_EVERY: std::time::Duration = std::time::Duration::from_millis(1500);
/// How long an ended background task is kept: the longest a row stays (a command's, about
/// 5 minutes after it exits, as Claude Code keeps it).
const ENDED_KEPT_MS: i64 = 5 * 60_000;
/// The most of a background command's output its row opens to: its end.
const OUTPUT_SHOWN: u64 = 256 * 1024;
/// The most of a line a row shows.
const LINE_MAX: usize = 200;

/// `task` as it stands now among `tasks`, those ended long ago dropped; the tasks after.
/// What of `live`'s background work holds its turn at `at`, when that changed since it was
/// last said.
/// How full the context of the thread whose `record/` is `record` is ([`SessionManager::context_usage`]):
/// from the CLI session file written to last, of those its lineage lists and hasn't forgotten.
fn context_in(record: &std::path::Path) -> Option<ContextUsage> {
    let (_, provider, file) = read_lineage(record)
        .into_iter()
        .filter(|session| !session.forgotten)
        .filter_map(|session| {
            let file = session_file::locate(
                session.provider,
                std::path::Path::new(session.file.as_deref()?),
            );
            let written = std::fs::metadata(&file)
                .and_then(|meta| meta.modified())
                .ok()?;
            Some((written, session.provider, file))
        })
        .max_by_key(|(written, ..)| *written)?;
    let said = session_file::context(provider, &file).ok().flatten()?;
    let window_tokens = said.window_tokens.or_else(|| {
        thread_view::read_turns(record)
            .into_iter()
            .rev()
            .find_map(|turn| turn.context_window)
    });
    Some(ContextUsage {
        used_tokens: said.used_tokens,
        window_tokens,
    })
}

fn said_jobs(live: &mut LiveTurn, at: i64) -> Option<thread_view::CliJobs> {
    let jobs = live.jobs(at);
    (jobs != live.jobs_said).then(|| {
        live.jobs_said = jobs.clone();
        jobs
    })
}

pub(super) fn update_background(
    tasks: &mut Vec<BackgroundTask>,
    task: &BackgroundTask,
    at: i64,
) -> Vec<BackgroundTask> {
    match tasks.iter_mut().find(|held| held.id == task.id) {
        Some(held) => {
            let line = held.last_line.take();
            *held = task.clone();
            held.last_line = task.last_line.clone().or(line);
        }
        None => tasks.push(task.clone()),
    }
    tasks.retain(|task| {
        task.ended_at_ms
            .is_none_or(|ended| at - ended < ENDED_KEPT_MS)
    });
    if task.status != BackgroundTaskStatus::Running
        && let Some(file) = &task.output_file
        && task.kind == BackgroundTaskKind::Shell
        && let Some(held) = tasks.iter_mut().find(|held| held.id == task.id)
    {
        // Its last words, once more, as it ended.
        if let Some(line) = last_line(std::path::Path::new(file)) {
            held.last_line = Some(line);
        }
    }
    tasks.clone()
}

/// Whether any of `tasks` is a running command whose output file is known.
fn reads_output(tasks: &[BackgroundTask]) -> bool {
    tasks.iter().any(|task| {
        task.kind == BackgroundTaskKind::Shell
            && task.status == BackgroundTaskStatus::Running
            && task.output_file.is_some()
    })
}

/// The last line with text in the file at `path`, read from its end.
fn last_line(path: &std::path::Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(4096))).ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    let text = String::from_utf8_lossy(&tail);
    let line = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    Some(line.chars().take(LINE_MAX).collect())
}

fn entry(event: Option<ProviderEvent>, at: i64) -> FileEntry {
    FileEntry {
        event: event.expect("an event"),
        at_ms: Some(at),
        updated_at_ms: None,
        parent: None,
        steer: false,
        images: Vec::new(),
        phase: None,
    }
}

/// What the live thread shows of the stream: its items and their deltas.
fn shown_live(event: &ProviderEvent) -> bool {
    matches!(
        event,
        ProviderEvent::Message { .. }
            | ProviderEvent::MessageDelta { .. }
            | ProviderEvent::Reasoning { .. }
            | ProviderEvent::ReasoningDelta { .. }
            | ProviderEvent::ToolCall { .. }
            | ProviderEvent::Command { .. }
            | ProviderEvent::CommandOutputDelta { .. }
            | ProviderEvent::FileChanges { .. }
            | ProviderEvent::Image { .. }
            | ProviderEvent::CompactionEnded { .. }
            | ProviderEvent::TaskNotice { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, kind: BackgroundTaskKind, status: BackgroundTaskStatus) -> BackgroundTask {
        BackgroundTask {
            id: id.into(),
            item_id: None,
            kind,
            label: format!("task {id}"),
            status,
            started_at_ms: 1_000,
            ended_at_ms: (status != BackgroundTaskStatus::Running).then_some(2_000),
            output_file: None,
            summary: None,
            last_line: None,
        }
    }

    #[test]
    fn a_task_is_updated_in_place_and_dropped_long_after_it_ends() {
        let mut tasks = Vec::new();
        let shell = task(
            "b1",
            BackgroundTaskKind::Shell,
            BackgroundTaskStatus::Running,
        );
        let agent = task(
            "a1",
            BackgroundTaskKind::Agent,
            BackgroundTaskStatus::Running,
        );
        update_background(&mut tasks, &shell, 1_500);
        let after = update_background(&mut tasks, &agent, 1_500);
        assert_eq!(after.len(), 2);

        // A line read earlier is kept when the CLI's update carries none.
        tasks[0].last_line = Some("compiling".into());
        let done = task(
            "b1",
            BackgroundTaskKind::Shell,
            BackgroundTaskStatus::Completed,
        );
        let after = update_background(&mut tasks, &done, 2_500);
        assert_eq!(after[0].status, BackgroundTaskStatus::Completed);
        assert_eq!(after[0].last_line.as_deref(), Some("compiling"));
        assert_eq!(after[1].id, "a1");

        // Five minutes after it ended, the next update drops it.
        let still = task(
            "a1",
            BackgroundTaskKind::Agent,
            BackgroundTaskStatus::Running,
        );
        let after = update_background(&mut tasks, &still, 2_000 + ENDED_KEPT_MS);
        assert_eq!(
            after
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>(),
            ["a1"]
        );
    }

    #[test]
    fn an_ended_command_reads_its_last_line_once_more() {
        let file =
            std::env::temp_dir().join(format!("brigadier-task-{}.output", uuid::Uuid::now_v7()));
        std::fs::write(&file, "one\ntwo\n\n  done  \n\n").unwrap();
        let mut tasks = Vec::new();
        let mut running = task(
            "b1",
            BackgroundTaskKind::Shell,
            BackgroundTaskStatus::Running,
        );
        running.output_file = Some(file.to_string_lossy().into_owned());
        assert!(reads_output(&[running.clone()]));
        update_background(&mut tasks, &running, 1_500);
        assert_eq!(tasks[0].last_line, None);

        let mut done = running.clone();
        done.status = BackgroundTaskStatus::Completed;
        done.ended_at_ms = Some(2_000);
        assert!(!reads_output(&[done.clone()]));
        let after = update_background(&mut tasks, &done, 2_000);
        assert_eq!(after[0].last_line.as_deref(), Some("done"));
        std::fs::remove_file(&file).unwrap();
    }

    #[test]
    fn the_last_line_is_the_last_with_text_and_kept_short() {
        let file =
            std::env::temp_dir().join(format!("brigadier-line-{}.output", uuid::Uuid::now_v7()));
        std::fs::write(&file, format!("start\n{}\n   \n", "x".repeat(500))).unwrap();
        assert_eq!(last_line(&file).map(|line| line.len()), Some(LINE_MAX));
        std::fs::write(&file, "").unwrap();
        assert_eq!(last_line(&file), None);
        std::fs::remove_file(&file).unwrap();
        assert_eq!(last_line(&file), None);
    }

    /// The CLI's notice line for a background job's end is in the live thread, as in its file.
    #[test]
    fn a_background_jobs_notice_line_shows_live() {
        assert!(shown_live(&ProviderEvent::TaskNotice {
            item_id: "n1".into(),
            text: r#"Background command "sleep 20" completed (exit code 0)"#.into(),
        }));
    }
}
