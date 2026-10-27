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

#[path = "envelope_provenance.rs"]
mod envelope_provenance;

impl SessionManager {
    /// Producer calls immediately before a daemon-only send/steer, after final construction.
    /// Mixed user input must never be registered. Peer/seam presentation remains unchanged.
    pub(super) fn register_internal_input(
        &self,
        id: &ConversationId,
        kinds: &[crate::InjectionKind],
        input: &mut brigadier_providers::TurnInput,
    ) -> Result<()> {
        envelope_provenance::register(&self.session_dir(id).record(), kinds, input)
    }

    async fn note_daemon_context(
        &self,
        id: &ConversationId,
        event: Option<&ProviderEvent>,
        native_hint: Option<&str>,
    ) -> bool {
        let Some(ProviderEvent::Message {
            role: Role::User,
            item_id,
            text,
            ..
        }) = event
        else {
            return false;
        };
        let record = self.session_dir(id).record();
        let native = native_hint.map(str::to_owned).or_else(|| {
            read_lineage(&record)
                .last()
                .map(|entry| entry.session_id.clone())
        });
        let Some(native) = native else {
            return false;
        };
        let key = format!("{native}:{item_id}");
        let known = daemon_context_ids(&record);
        if known.contains(&key) {
            return true;
        }
        let Ok(messages) = self.core.all_messages(id).await else {
            return false;
        };
        if messages.iter().any(|message| {
            message.role == crate::MessageRole::User
                && (message.text == *text
                    || (message.blob.is_some() && text.starts_with(&message.text)))
        }) {
            return false;
        }
        if envelope_provenance::bind(&record, text, &key).unwrap_or(false) {
            return true;
        }
        // Old unmarked inputs require an exact typed inbox record, not a phrase/prefix.
        let mut after = 0;
        let mut proven = false;
        loop {
            let Ok(events) = self
                .core
                .store()
                .read_stream_since(crate::model::streams::inbox(id), after, 500)
                .await
            else {
                return false;
            };
            let full = events.len() == 500;
            for stored in events {
                after = stored.stream_seq;
                if stored.kind != "inbox.queued" {
                    continue;
                }
                let Ok(value) = serde_json::from_str::<serde_json::Value>(stored.payload.get())
                else {
                    continue;
                };
                let Some(value) = value.get("envelope") else {
                    continue;
                };
                let Ok(envelope) =
                    serde_json::from_value::<super::conversation::Envelope>(value.clone())
                else {
                    continue;
                };
                if envelope.text == *text && envelope_provenance::internal(&[envelope.kind]) {
                    proven = envelope_provenance::bind_legacy(
                        &record,
                        stored.stream_seq,
                        envelope.kind,
                        text,
                        &key,
                    )
                    .unwrap_or(false);
                    if proven {
                        break;
                    }
                }
            }
            if proven || !full {
                break;
            }
        }
        proven
    }

    /// Follows the running turn of `conv`'s CLI through one of its events (`None`: the CLI's
    /// events ended): its items, and its end, written to `record/turns.jsonl`.
    pub(super) async fn follow_cli_thread(&self, conv: &ConvLive, event: Option<&ProviderEvent>) {
        self.follow_background_history(&conv.id, None, event).await;
        let daemon_context = self.note_daemon_context(&conv.id, event, None).await;
        let at = now_ms();
        // Brigadier's requests the turn carries, as the conversation knows them now, and whether
        // it joins the work before it: when the CLI says a user message (a turn's, or a steer's)
        // and when the turn ends.
        let carried = match event {
            Some(
                ProviderEvent::Message {
                    role: Role::User, ..
                }
                | ProviderEvent::TurnCompleted { .. }
                | ProviderEvent::Exited { .. },
            )
            | None => Some(conv.carried_turn().await),
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
                        && !live.persistent_task(task)
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
                            } | ProviderEvent::CompactionStarted { .. }
                                | ProviderEvent::CompactionEnded { .. }
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
                }) if !daemon_context => {
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
                        live.joins = carried.as_ref().is_some_and(|(_, joins)| *joins);
                    }
                    if let Some((carried, _)) = &carried {
                        thread_view::add_requests(&mut live.requests, carried);
                        noted = Some(TurnRecord {
                            user_message: live.user_message.clone(),
                            end: None,
                            error: None,
                            ended_at_ms: None,
                            requests: live.requests.clone(),
                            limit: None,
                            context_window: None,
                            joins: live.joins,
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
                Some(event) if shown_live(event) && !daemon_context => {
                    news.push(entry(Some(event.clone()), at))
                }
                _ => {}
            }
            if !news.is_empty() {
                live.entries = session_file::merge(std::mem::take(&mut live.entries), news.clone());
            }
            if let Some(end) = ended {
                live.open = false;
                if let Some((carried, _)) = &carried {
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
                    joins: live.joins,
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

    async fn backfill_daemon_context(&self, id: &ConversationId) {
        let mut after = 0;
        let mut legacy = std::collections::HashSet::new();
        loop {
            let Ok(events) = self
                .core
                .store()
                .read_stream_since(crate::model::streams::inbox(id), after, 500)
                .await
            else {
                return;
            };
            let full = events.len() == 500;
            for stored in events {
                after = stored.stream_seq;
                if stored.kind != "inbox.queued" {
                    continue;
                }
                let Ok(value) = serde_json::from_str::<serde_json::Value>(stored.payload.get())
                else {
                    continue;
                };
                let Some(value) = value.get("envelope") else {
                    continue;
                };
                let Ok(envelope) =
                    serde_json::from_value::<super::conversation::Envelope>(value.clone())
                else {
                    continue;
                };
                if envelope_provenance::internal(&[envelope.kind]) {
                    legacy.insert(envelope.text);
                }
            }
            if !full {
                break;
            }
        }
        let record = self.session_dir(id).record();
        for lineage in read_lineage(&record) {
            let Some(file) = &lineage.file else {
                continue;
            };
            let entries = self
                .session_files
                .read(lineage.provider, std::path::Path::new(file));
            for entry in entries.iter().filter(|entry| entry.parent.is_none()) {
                if let ProviderEvent::Message {
                    role: Role::User,
                    text,
                    ..
                } = &entry.event
                    && (text.starts_with(envelope_provenance::MARK) || legacy.contains(text))
                {
                    self.note_daemon_context(id, Some(&entry.event), Some(&lineage.session_id))
                        .await;
                }
            }
        }
    }

    /// Conversation `id`'s thread as its CLI keeps it, with its workers' rows.
    pub async fn thread_view_with_workers(&self, id: &ConversationId) -> Result<ThreadView> {
        self.backfill_daemon_context(id).await;
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
        let daemon_ids = daemon_context_ids(&record);
        // A fork's thread starts with its source's, up to the answer it forks (#648).
        let forked = thread_view::read_forked(&record);
        let files = std::iter::once(std::sync::Arc::new(forked.entries))
            .chain(read_lineage(&record).into_iter().filter_map(|session| {
                let file = session.file?;
                let scoped_ids = daemon_ids
                    .iter()
                    .filter_map(|key| {
                        key.strip_prefix(&format!("{}:", session.session_id))
                            .map(str::to_owned)
                    })
                    .collect();
                let entries = self
                    .session_files
                    .read(session.provider, std::path::Path::new(&file));
                Some(std::sync::Arc::new(
                    entries
                        .iter()
                        .filter(|entry| !thread_view::daemon_context(&entry.event, &scoped_ids))
                        .cloned()
                        .collect(),
                ))
            }))
            .collect();
        let mut records = forked.turns;
        records.extend(thread_view::read_turns(&record));
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

    /// Proven app task ids from the same command evidence used by live Background placement.
    pub(super) fn persistent_cli_tasks(
        &self,
        id: &ConversationId,
    ) -> std::collections::HashSet<String> {
        self.cli_threads
            .lock()
            .unwrap()
            .get(id)
            .map(|live| live.persistent_tasks())
            .unwrap_or_default()
    }

    pub(super) fn running_cli_task_ids(
        &self,
        id: &ConversationId,
    ) -> std::collections::HashSet<String> {
        self.cli_threads
            .lock()
            .unwrap()
            .get(id)
            .map(|live| {
                live.background
                    .iter()
                    .filter(|task| task.status == BackgroundTaskStatus::Running)
                    .map(|task| task.id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Stops background task `task` of conversation `id`'s CLI, or of its worker `worker`'s,
    /// as the CLI's own Stop does.
    pub async fn stop_background_task(
        &self,
        id: &ConversationId,
        worker: Option<&TaskId>,
        task: String,
        started_at_ms: Option<i64>,
    ) -> Result<()> {
        let cli = match worker {
            None => self.conv(id)?.live_cli().await,
            Some(worker) => match self.existing_task_live(worker) {
                Some(live) if live.conversation_id == *id => live.cli().await,
                _ => None,
            },
        };
        let cli = cli.ok_or_else(|| Error::Invalid("its CLI isn't running".into()))?;
        if let Some(started) = started_at_ms {
            let matches = |held: &BackgroundTask| {
                (held.id == task
                    && held.started_at_ms == started
                    && held.status == BackgroundTaskStatus::Running)
                    .then(String::new)
            };
            let current = match worker {
                None => self
                    .cli_threads
                    .lock()
                    .unwrap()
                    .get(id)
                    .and_then(|live| live.background.iter().find_map(matches)),
                Some(worker) => self.worker_background_file(worker, matches),
            };
            if current.is_none() {
                return Err(Error::Invalid(
                    "That background invocation is no longer running".into(),
                ));
            }
        }
        cli.session
            .stop_task(task.clone())
            .await
            .map_err(|err| Error::Provider(err.to_string()))?;
        self.note_history_stop(id, worker, &task);
        Ok(())
    }

    /// What background command `task` of conversation `id`'s CLI, or of its worker `worker`'s,
    /// printed so far: the end of its output file, as much as a row's output shows.
    pub fn background_task_output(
        &self,
        id: &ConversationId,
        worker: Option<&TaskId>,
        task: &str,
        started_at_ms: Option<i64>,
    ) -> Result<String> {
        use std::io::{Read, Seek, SeekFrom};
        let shell = |held: &BackgroundTask| {
            (held.id == task
                && started_at_ms.is_none_or(|started| held.started_at_ms == started)
                && matches!(
                    held.kind,
                    BackgroundTaskKind::Shell | BackgroundTaskKind::Terminal
                ))
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

fn daemon_context_ids(record: &std::path::Path) -> std::collections::HashSet<String> {
    envelope_provenance::ids(record)
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

/// What the live thread shows of the stream: its items and their deltas, and a compaction while
/// it runs (no file keeps that: its end, which the file keeps, takes its place).
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
            | ProviderEvent::CompactionStarted { .. }
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
    #[tokio::test]
    async fn stale_background_invocation_cannot_read_or_stop_a_reused_native_id() {
        use crate::manager::flow::{Flow, Options, Reply, Script};
        let script: Script = std::sync::Arc::new(|_| Box::pin(async { Reply::text("Done") }));
        let flow = Flow::start("reused-background-actions", Options::default(), script).await;
        flow.say("Start the native CLI").await;
        flow.settled().await;
        let conv = flow.manager.conv(&flow.conversation).unwrap();
        let output = flow
            .manager
            .session_dir(&flow.conversation)
            .record()
            .join("replacement.output");
        std::fs::write(&output, "replacement invocation output").unwrap();
        let task = BackgroundTask {
            id: "reused".into(),
            item_id: Some("replacement-call".into()),
            kind: BackgroundTaskKind::Shell,
            label: "Replacement".into(),
            status: BackgroundTaskStatus::Running,
            started_at_ms: 200,
            ended_at_ms: None,
            output_file: Some(output.to_string_lossy().into_owned()),
            summary: None,
            last_line: None,
        };
        flow.manager
            .follow_cli_thread(&conv, Some(&ProviderEvent::BackgroundTask { task }))
            .await;
        assert!(
            flow.manager
                .background_task_output(&flow.conversation, None, "reused", Some(100))
                .is_err()
        );
        assert_eq!(
            flow.manager
                .background_task_output(&flow.conversation, None, "reused", Some(200))
                .unwrap(),
            "replacement invocation output"
        );
        let error = flow
            .manager
            .stop_background_task(&flow.conversation, None, "reused".into(), Some(100))
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("invocation is no longer running"),
            "{error}"
        );
        assert!(
            flow.manager
                .thread_view(&flow.conversation)
                .unwrap()
                .background
                .iter()
                .any(|task| task.id == "reused" && task.status == BackgroundTaskStatus::Running)
        );
        flow.stop().await;
    }

    #[tokio::test]
    async fn typed_internal_result_and_reminder_inputs_hide_only_the_bound_native_message() {
        use crate::manager::flow::{Flow, Options, Reply, Script};
        use crate::manager::session_dir::{LineageEntry, note_lineage_at};
        use brigadier_providers::{InputPart, ProviderKind, TurnInput};
        let script: Script = std::sync::Arc::new(|_| Box::pin(async { Reply::text("Done") }));
        let flow = Flow::start("internal-input-provenance", Options::default(), script).await;
        let id = &flow.conversation;
        let record = flow.manager.session_dir(id).record();
        note_lineage_at(
            record.clone(),
            LineageEntry {
                provider: ProviderKind::Claude,
                session_id: "native-session".into(),
                file: None,
                started_at: now_ms(),
                forgotten: false,
            },
        )
        .await
        .unwrap();
        flow.core
            .append_message(id.clone(), "Real request".into())
            .await
            .unwrap();
        let conv = flow.manager.conv(id).unwrap();
        let message = |item: &str, role, text: &str| ProviderEvent::Message {
            item_id: item.into(),
            role,
            text: text.into(),
        };
        flow.manager
            .follow_cli_thread(
                &conv,
                Some(&message("user-real", Role::User, "Real request")),
            )
            .await;
        flow.manager
            .follow_cli_thread(
                &conv,
                Some(&message(
                    "assistant-before",
                    Role::Assistant,
                    "First answer",
                )),
            )
            .await;
        flow.manager
            .follow_cli_thread(
                &conv,
                Some(&ProviderEvent::TurnCompleted {
                    turn_id: None,
                    status: TurnStatus::Completed,
                    duration_ms: None,
                    usage: None,
                }),
            )
            .await;
        for (item, kind) in [
            ("internal-result", crate::InjectionKind::BackgroundRun),
            ("internal-reminder", crate::InjectionKind::Reminder),
        ] {
            let mut input = TurnInput::text("Background command finished after your turn · exit 0");
            flow.manager
                .register_internal_input(id, &[kind], &mut input)
                .unwrap();
            let InputPart::Text(wire) = &input.parts[0] else {
                panic!("wire text");
            };
            flow.manager
                .follow_cli_thread(&conv, Some(&message(item, Role::User, wire)))
                .await;
        }
        flow.manager
            .follow_cli_thread(
                &conv,
                Some(&message(
                    "assistant-after",
                    Role::Assistant,
                    "The real follow-up is preserved",
                )),
            )
            .await;
        let view = flow.manager.thread_view(id).unwrap();
        assert_eq!(
            view.turns.len(),
            1,
            "internal messages add no phantom request/workblock"
        );
        assert!(view.entries.iter().any(|entry| matches!(&entry.event, ProviderEvent::Message { item_id, .. } if item_id == "assistant-after")));
        assert!(!view.entries.iter().any(|entry| matches!(&entry.event, ProviderEvent::Message { item_id, .. } if item_id.starts_with("internal-"))));
        let known = daemon_context_ids(&record);
        assert!(known.contains("native-session:internal-result"));
        assert!(known.contains("native-session:internal-reminder"));
        // Identical human wording is still authored. Even a stored legacy Reminder is not authority over it.
        let human = "Background command finished after your turn · exit 0";
        let authored = flow
            .core
            .append_message(id.clone(), human.into())
            .await
            .unwrap();
        flow.manager
            .keep_queued(
                id,
                &super::super::conversation::Envelope {
                    kind: crate::InjectionKind::Reminder,
                    label: "reminder".into(),
                    task_id: None,
                    text: human.into(),
                },
                Some(&authored.id),
                None,
            )
            .await;
        assert!(
            !flow
                .manager
                .note_daemon_context(
                    id,
                    Some(&message("user-identical", Role::User, human)),
                    None
                )
                .await
        );
        flow.manager.shutdown().await;
    }
}
