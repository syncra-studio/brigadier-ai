//! The thread's commands that outlast a tool call (`run`, `run_check`).
//!
//! A tool call holds the thread: while it runs, the thread can't sort a follow-up or read a
//! message steered to it, and the user waits on a silent row. So a command of the thread's
//! that is still going after [`FOREGROUND`] goes to the background: the call returns at once
//! with its id, the command goes on, owned by the session rather than the thread's CLI (a
//! closed or replaced CLI doesn't end it), and its result comes to the thread as a message in
//! the request that started it, which works until then. The whole operation moves, waits
//! included: a check waiting for the same check or for the build lease goes too.
//!
//! - **Recorded** as a [`BackgroundRun`] when it goes to the background, so the request works
//!   and the app shows it; a restart that cuts one off records it interrupted and tells the
//!   thread, which may run it again.
//! - **Stopped** with the user's Stop and the session's archive or delete, telling nothing; one
//!   the user stops on its own (the context card's Stop) tells the thread, which answers.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::SessionManager;
use super::conversation::Envelope;
use crate::Result;
use crate::model::{ConversationId, DomainEvent};
use crate::now_ms;
use crate::work::{BackgroundRun, BackgroundRunState, InjectionKind};

/// How long a command of the thread's may hold its tool call before it goes to the
/// background. Tests wait less.
pub(crate) const FOREGROUND: Duration = Duration::from_secs(if cfg!(test) { 2 } else { 60 });
/// How long before its move to the background a `run_check` call stops waiting for its
/// review, so the wait never sends it there.
const REVIEW_MARGIN: Duration = FOREGROUND.checked_div(10).expect("not zero");

/// Until when a `run_check` call of session `id` starting now may wait for a review of the
/// thread's change in the foreground: the rest of its [`FOREGROUND`] minute, less a margin
/// (#193). In tests only when the test waits (`wait_for_reviews`): their reviewers answer at
/// once, and the rest hear of a review in a message, as most do in the app. A test whose
/// commands never go to the background ([`let_background`]) holds its call to the review's
/// end, however loaded the machine (#442).
pub(crate) fn review_hold(id: &ConversationId) -> Option<tokio::time::Instant> {
    #[cfg(test)]
    if !REVIEW_WAITS.lock().unwrap().contains(id) {
        return None;
    }
    #[cfg(all(test, unix))]
    if !BACKGROUNDED.lock().unwrap().contains(id) {
        return Some(tokio::time::Instant::now() + Duration::from_secs(600));
    }
    #[cfg(not(test))]
    let _ = id;
    Some(tokio::time::Instant::now() + FOREGROUND - REVIEW_MARGIN)
}

#[cfg(test)]
static REVIEW_WAITS: std::sync::LazyLock<std::sync::Mutex<HashSet<ConversationId>>> =
    std::sync::LazyLock::new(Default::default);

/// Session `id`'s `run_check` calls wait for their review as in the app ([`review_hold`]).
#[cfg(all(test, unix))]
pub(super) fn wait_for_reviews(id: &ConversationId) {
    REVIEW_WAITS.lock().unwrap().insert(id.clone());
}

/// How long a stopped run's work may take to end before it is ended for it: Stop always ends
/// the run, even when its work doesn't heed it.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// `run-<n>`'s number (0 for anything else).
pub(crate) fn background_run_number(id: &str) -> u64 {
    id.strip_prefix("run-")
        .and_then(|number| number.parse().ok())
        .unwrap_or(0)
}

/// The thread's name for a run, trimmed to at most 40 characters; none when blank.
fn run_name(name: Option<&str>) -> Option<String> {
    let name = name?.trim();
    if name.is_empty() {
        return None;
    }
    if name.chars().count() <= 40 {
        return Some(name.to_owned());
    }
    let cut: String = name.chars().take(39).collect();
    Some(format!("{}…", cut.trim_end()))
}

/// The cleanup-ledger owner of a session's background runs: they outlive the thread's CLI and
/// end with the session.
pub(crate) fn background_owner(id: &ConversationId) -> String {
    format!("background:{id}")
}

/// A command's work while its tool call holds it ([`SessionManager::in_foreground`]): dropped
/// before the background watcher takes it, the work is cancelled, and a run already recorded
/// running is still watched to its end, so it reads stopped.
struct Handoff {
    manager: std::sync::Arc<SessionManager>,
    handle: Option<tokio::task::JoinHandle<Result<(String, String)>>>,
    cancel: CancellationToken,
    run: Option<BackgroundRun>,
}

impl Drop for Handoff {
    fn drop(&mut self) {
        let Some(handle) = self.handle.take() else {
            return;
        };
        self.cancel.cancel();
        if let Some(run) = self.run.take() {
            self.manager
                .watch_background_run(handle, run, self.cancel.clone());
        }
    }
}

/// Work the session's Stop ends ([`SessionManager::stoppable`]), until dropped.
pub(super) struct Stoppable<'a> {
    runs: &'a BackgroundRuns,
    key: (ConversationId, String),
    pub(super) cancel: CancellationToken,
}

impl Drop for Stoppable<'_> {
    fn drop(&mut self) {
        self.runs
            .running
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.key);
    }
}

/// A run whose end is being recorded: taken from the running ones into the ended ones as its
/// end is chosen, so a Stop then is refused rather than lost and no look takes it for missed.
/// Dropped before its end is stored (`stored`), its watcher was cut off: the end is free for
/// the look at missed ends to take.
struct Ending<'a> {
    manager: &'a SessionManager,
    key: (ConversationId, String),
    stored: bool,
}

impl<'a> Ending<'a> {
    fn take(manager: &'a SessionManager, key: (ConversationId, String)) -> Self {
        let runs = &manager.background_runs;
        let mut running = runs.running.lock().unwrap_or_else(|p| p.into_inner());
        running.remove(&key);
        runs.ended
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key.clone());
        Self {
            manager,
            key,
            stored: false,
        }
    }
}

impl<'a> Ending<'a> {
    /// Takes the end of a run nothing watches whose end no one recorded or took (its end was
    /// missed): then only the caller records it (a Stop and the watchdog's look can come
    /// together).
    fn claim(manager: &'a SessionManager, id: &ConversationId, run: &str) -> Option<Self> {
        let key = (id.clone(), run.to_owned());
        let runs = &manager.background_runs;
        let running = runs.running.lock().unwrap_or_else(|p| p.into_inner());
        (!running.contains_key(&key)
            && runs
                .ended
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(key.clone()))
        .then(|| Self {
            manager,
            key,
            stored: false,
        })
    }
}

impl Ending<'_> {
    /// Its end is recorded (`stored`), or free again for the look at missed ends.
    fn finish(mut self, stored: bool) {
        self.stored = stored;
    }
}

impl Drop for Ending<'_> {
    fn drop(&mut self) {
        if !self.stored {
            self.manager
                .background_runs
                .ended
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&self.key);
        }
    }
}

/// What ends each running background run, by conversation and id.
#[derive(Default)]
pub(crate) struct BackgroundRuns {
    running: std::sync::Mutex<HashMap<(ConversationId, String), CancellationToken>>,
    /// The runs whose end this process recorded or is recording, so a look at an older board
    /// doesn't take one for missed, nor two looks the same one. Locked after `running`.
    ended: std::sync::Mutex<HashSet<(ConversationId, String)>>,
    /// The runs the user stopped one by one: the thread hears it.
    stopped_alone: std::sync::Mutex<HashSet<(ConversationId, String)>>,
    /// Held while a run's number is taken and its start recorded.
    numbering: tokio::sync::Mutex<()>,
}

#[cfg(all(test, unix))]
static FOREGROUND_GATES: std::sync::LazyLock<
    std::sync::Mutex<HashMap<ConversationId, std::sync::Arc<tokio::sync::Notify>>>,
> = std::sync::LazyLock::new(Default::default);

/// The sessions whose commands go to the background past [`FOREGROUND`] in tests
/// ([`let_background`]); every other session's are held in their calls to their end.
#[cfg(all(test, unix))]
static BACKGROUNDED: std::sync::LazyLock<std::sync::Mutex<HashSet<ConversationId>>> =
    std::sync::LazyLock::new(Default::default);

/// Tests can hold a foreground call while pipe gates are inspected, regardless of machine load.
#[cfg(all(test, unix))]
pub(super) fn hold_foreground(id: &ConversationId, gate: std::sync::Arc<tokio::sync::Notify>) {
    FOREGROUND_GATES.lock().unwrap().insert(id.clone(), gate);
}

/// In tests a command answers in its own call, however loaded the machine, unless its test
/// lets session `id`'s go to the background past [`FOREGROUND`], as they do in the app: a
/// quick command on a loaded machine can take longer than that.
#[cfg(all(test, unix))]
pub(super) fn let_background(id: &ConversationId) {
    BACKGROUNDED.lock().unwrap().insert(id.clone());
}

/// What the thread hears of a run the user stopped alone: how long it ran, and what it printed
/// until then, as its row keeps it (`kept`, the digest of its stored output), or that it printed
/// nothing; the thread has no time of its own to guess (#89).
fn stopped_text(run: &BackgroundRun, kept: &str) -> String {
    let ask = "Don't run it again unless they ask; say in one line where that leaves the work.";
    let ran = super::watchdog::spoken(run.ended_at_ms.unwrap_or_else(now_ms) - run.started_at_ms);
    if kept.trim().is_empty() {
        return format!(
            "[The user stopped {} after {ran}, before it finished · {} `{}`: it printed nothing \
             and no result came. {ask}]",
            run.id, run.tool, run.command
        );
    }
    format!(
        "[The user stopped {} after {ran}, before it finished · {} `{}`: no result came, but \
         what it printed until then is kept, and its row shows it. {ask}]\n{kept}",
        run.id, run.tool, run.command
    )
}

/// What the thread hears of the user's Restart of a failed run (`ended`: how it ended).
fn restart_text(run: &BackgroundRun, ended: &str) -> String {
    format!(
        "[The user pressed Restart on {} ({ended}) · {} `{}`: run it again now with {}, the same \
         command, then say in one line how it went.]",
        run.id, run.tool, run.command, run.tool
    )
}

/// What the thread hears when its call goes on in the background: no guess at how long it has
/// left, which would stay under its row once it stopped or finished (#89).
fn backgrounded_text(id: &str) -> String {
    format!(
        "[{id} still runs after {secs} s, so it goes on in the background: its result comes to \
         you as a message in this request, which works until then. Don't wait for it or run it \
         again. Tell the user in one line what runs, with no guess at how long it has left, then \
         carry on or reply {quiet}.]",
        secs = FOREGROUND.as_secs(),
        quiet = super::prompts::QUIET,
    )
}

impl SessionManager {
    /// Runs `work`, the thread's `tool` call of `command` (its status and reply), in its tool
    /// call while it ends within [`FOREGROUND`], else in the background: its reply then comes
    /// as a message. `tool` comes with the short name the thread gave the command, if any.
    /// `cancel` ends the work; the thread's CLI ending (`cli_ended`) cancels it only while it
    /// still holds the call.
    pub(super) async fn in_foreground<F>(
        &self,
        id: &ConversationId,
        (tool, name): (&str, Option<&str>),
        command: &str,
        cli_ended: CancellationToken,
        cancel: CancellationToken,
        work: F,
    ) -> Result<String>
    where
        F: Future<Output = Result<(String, String)>> + Send + 'static,
    {
        // When the command really started: a run's time counts its foreground minute too.
        let started_at_ms = now_ms();
        // Whose answer it serves: the turn's request, taken before the call can outlive it.
        let request = match self.conv(id) {
            Ok(conv) => conv.running_request().await,
            Err(_) => None,
        };
        // Until the background watcher owns the work, a dropped call (its MCP request cancelled,
        // its bridge gone) ends the work rather than leaving it running unowned.
        let mut handoff = Handoff {
            manager: self.arc(),
            handle: Some(tokio::spawn(work)),
            cancel: cancel.clone(),
            run: None,
        };
        let handle = handoff.handle.as_mut().expect("held until handed off");
        #[cfg(all(test, unix))]
        let gate = FOREGROUND_GATES.lock().unwrap().remove(id).or_else(|| {
            (!BACKGROUNDED.lock().unwrap().contains(id))
                .then(|| std::sync::Arc::new(tokio::sync::Notify::new()))
        });
        let background_due = async {
            tokio::time::sleep(FOREGROUND).await;
            #[cfg(all(test, unix))]
            if let Some(gate) = gate {
                gate.notified().await;
            }
        };
        let joined = tokio::select! {
            joined = &mut *handle => Some(joined),
            () = cli_ended.cancelled() => {
                cancel.cancel();
                Some(handle.await)
            }
            () = background_due => None,
        };
        if let Some(joined) = joined {
            handoff.handle = None;
            return joined
                .map_err(|err| crate::Error::Invalid(err.to_string()))?
                .map(|(_, reply)| reply);
        }
        let run = {
            let _numbering = self.background_runs.numbering.lock().await;
            let number = self
                .core
                .board(id)
                .await
                .map(|board| {
                    board
                        .background_runs
                        .keys()
                        .map(|id| background_run_number(id))
                        .max()
                        .unwrap_or(0)
                })
                .unwrap_or(0)
                + 1;
            let run = BackgroundRun {
                id: format!("run-{number}"),
                conversation_id: id.clone(),
                request_id: request,
                tool: tool.to_owned(),
                command: command.to_owned(),
                started_at_ms,
                ended_at_ms: None,
                state: BackgroundRunState::Running,
                item_id: self.own_command_item(id, tool, command).await,
                name: run_name(name),
            };
            if let Some(item) = &run.item_id
                && let Ok(conv) = self.conv(id)
            {
                conv.items().background_started(item, &run.id);
            }
            self.background_runs
                .running
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert((id.clone(), run.id.clone()), cancel.clone());
            // Recorded running from here: a dropped call still records how it ended.
            handoff.run = Some(run.clone());
            self.store_background_run(&run).await;
            run
        };
        let reply = backgrounded_text(&run.id);
        handoff.run = None;
        let handle = handoff.handle.take().expect("held until handed off");
        self.watch_background_run(handle, run, cancel);
        Ok(reply)
    }

    /// Waits for a background run's work and records how it ended.
    fn watch_background_run(
        &self,
        handle: tokio::task::JoinHandle<Result<(String, String)>>,
        run: BackgroundRun,
        cancel: CancellationToken,
    ) {
        let manager = self.arc();
        self.spawn(async move {
            let mut handle = handle;
            let stopped = async {
                cancel.cancelled().await;
                tokio::time::sleep(STOP_GRACE).await;
            };
            let joined = tokio::select! {
                joined = &mut handle => joined,
                () = stopped => {
                    handle.abort();
                    handle.await
                }
            };
            let result = joined.unwrap_or_else(|err| Err(crate::Error::Invalid(err.to_string())));
            manager.background_run_ended(run, result, cancel).await;
        });
    }

    /// Ends each of session `id`'s runs its board still shows running though nothing watches
    /// it any more (its end was missed): recorded as such, and the thread hears it, so no row
    /// stays running for ever (#220).
    pub(crate) async fn end_missed_runs(&self, id: &ConversationId) {
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        let missed: Vec<(BackgroundRun, Ending)> = board
            .background_runs
            .values()
            .filter(|run| run.state == BackgroundRunState::Running)
            .filter_map(|run| Some((run.clone(), Ending::claim(self, id, &run.id)?)))
            .collect();
        for (mut run, ended) in missed {
            run.state = BackgroundRunState::Interrupted {
                reason: "its end was missed".into(),
            };
            run.ended_at_ms = Some(now_ms());
            let envelope = Envelope {
                kind: InjectionKind::BackgroundRun,
                label: format!("{} missed", run.id),
                task_id: None,
                text: format!(
                    "[{} ended without Brigadier seeing how · {} `{}`: no result came. Run it \
                     again if you still need it.]",
                    run.id, run.tool, run.command
                ),
            };
            ended.finish(self.end_with_message(run, envelope).await);
        }
    }

    /// A background run ended: recorded, and unless the user's Stop or the session's close
    /// ended it, its result goes to the thread.
    async fn background_run_ended(
        &self,
        mut run: BackgroundRun,
        result: Result<(String, String)>,
        cancel: CancellationToken,
    ) {
        let ended = Ending::take(self, (run.conversation_id.clone(), run.id.clone()));
        self.background_command_changes(&run).await;
        // The call it started may have been linked after the handoff, on the board only.
        if run.item_id.is_none()
            && let Ok(board) = self.core.board(&run.conversation_id).await
        {
            run.item_id = board
                .background_runs
                .get(&run.id)
                .and_then(|stored| stored.item_id.clone());
        }
        run.ended_at_ms = Some(now_ms());
        if cancel.is_cancelled() {
            // What it printed before the Stop stays readable from its row, and the thread hears
            // of it as kept.
            let (_, kept) = self
                .retain_background_output(&run, "stopped", String::new())
                .await;
            run.state = BackgroundRunState::Stopped;
            let alone = self
                .background_runs
                .stopped_alone
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&(run.conversation_id.clone(), run.id.clone()));
            if alone {
                let envelope = Envelope {
                    kind: InjectionKind::BackgroundRun,
                    label: format!("{} stopped", run.id),
                    task_id: None,
                    text: stopped_text(&run, &kept),
                };
                ended.finish(self.end_with_message(run, envelope).await);
            } else {
                ended.finish(self.store_background_run(&run).await);
                self.settle_requests(&run.conversation_id).await;
            }
            return;
        }
        let (status, text) = match result {
            Ok(done) => done,
            Err(err) => ("couldn't run".to_owned(), format!("[couldn't run: {err}]")),
        };
        let (text, _) = self.retain_background_output(&run, &status, text).await;
        run.state = BackgroundRunState::Finished { status };
        let envelope = Envelope {
            kind: InjectionKind::BackgroundRun,
            label: format!("{} finished", run.id),
            task_id: None,
            text: format!(
                "[{} finished · {} `{}`]\n{text}",
                run.id, run.tool, run.command
            ),
        };
        ended.finish(self.end_with_message(run, envelope).await);
    }

    /// Stores what a background run printed as its row's final output, `text` when its live
    /// buffer holds nothing more, and releases the buffer: the stored output then answers the
    /// row's reads. Returns `text`, the result the thread hears as it always did (a check's
    /// evidence and commit lines are in it, not in what the pipes printed), and what was
    /// stored (the digest of what the pipes printed, else `text`).
    async fn retain_background_output(
        &self,
        run: &BackgroundRun,
        status: &str,
        text: String,
    ) -> (String, String) {
        let mut stored = text.clone();
        let live_output = if crate::digest::digest_alias(&text).is_none() {
            self.conv(&run.conversation_id)
                .ok()
                .and_then(|conv| {
                    conv.items()
                        .background_output(&run.id, run.item_id.as_deref())
                })
                .filter(|(output, _)| !output.is_empty())
                .map(|(output, _)| output)
        } else {
            None
        };
        if let Some(output) = live_output {
            match self
                .store_output(
                    &run.conversation_id,
                    crate::work::OutputSource::Run,
                    status,
                    output,
                    false,
                )
                .await
            {
                Ok((_, digest)) => stored = digest,
                Err(err) => {
                    tracing::warn!(run = %run.id, error = %err, "could not retain background output")
                }
            }
        }
        if !stored.is_empty()
            && let Ok(event) = brigadier_store::NewEvent::new(
                format!("background-output:{}:{}", run.conversation_id, run.id),
                "background.output",
                now_ms(),
                &stored,
            )
            && let Err(err) = self.core.store().append(vec![event]).await
        {
            tracing::warn!(run = %run.id, error = %err, "could not store background output");
        }
        // Keep the live buffer readable until its final stored result can answer the same read.
        if let Ok(conv) = self.conv(&run.conversation_id) {
            conv.items().background_ended(&run.id);
        }
        (text, stored)
    }

    /// Records a background run's end with the message the thread hears of it: the message is
    /// queued first, so its request is carried by one or the other throughout and a settling
    /// in between can't end it before the message comes.
    /// Whether its end was stored.
    async fn end_with_message(&self, run: BackgroundRun, envelope: Envelope) -> bool {
        let id = run.conversation_id.clone();
        let conv = self
            .queue_envelope(&id, envelope, run.request_id.clone())
            .await;
        let stored = self.store_background_run(&run).await;
        // The request works again until the orchestrator has read it.
        self.settle_requests(&id).await;
        if let Some(conv) = conv {
            self.kick(&conv);
        }
        stored
    }

    /// Links a call whose background reply arrived before its start row was observed.
    pub(super) async fn background_item(
        &self,
        id: &ConversationId,
        event: &brigadier_providers::ProviderEvent,
    ) {
        let brigadier_providers::ProviderEvent::ToolCall {
            item_id,
            output: Some(output),
            ..
        } = event
        else {
            return;
        };
        let Some(start) = output.find("[run-") else {
            return;
        };
        let Some(run_id) = output[start + 1..]
            .split_whitespace()
            .next()
            .filter(|id| id.starts_with("run-"))
        else {
            return;
        };
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        if let Some(mut run) = board.background_runs.get(run_id).cloned()
            && run.item_id.as_deref() != Some(item_id)
        {
            run.item_id = Some(item_id.clone());
            if run.state == BackgroundRunState::Running
                && let Ok(conv) = self.conv(id)
            {
                conv.items().background_started(item_id, &run.id);
            }
            self.store_background_run(&run).await;
        }
    }

    /// Reads a running command's shared thread buffer, then its retained result after it ends.
    /// Redaction sees the full buffer before the tail is cut, including secrets across chunks.
    pub async fn read_background_run(
        &self,
        id: &ConversationId,
        run_id: &str,
    ) -> Result<(String, u64)> {
        let board = self.core.board(id).await?;
        let run = board
            .background_runs
            .get(run_id)
            .ok_or_else(|| crate::Error::NotFound(format!("background run {run_id}")))?;
        let live = self.conv(id).ok().and_then(|conv| {
            conv.items()
                .background_output(run_id, run.item_id.as_deref())
        });
        if let Some((output, bytes)) = live {
            return Ok(self
                .command_tail(id, output, bytes, run.state == BackgroundRunState::Running)
                .await);
        }
        let events = self
            .core
            .store()
            .read_stream(
                format!("background-output:{id}:{run_id}"),
                brigadier_store::StreamPage {
                    limit: 1,
                    ..Default::default()
                },
            )
            .await?;
        let Some(event) = events.first() else {
            return Ok((String::new(), 0));
        };
        let mut output: String =
            serde_json::from_str(event.payload.get()).map_err(|source| crate::Error::Corrupt {
                seq: event.seq,
                source,
            })?;
        let mut bytes = output.len() as u64;
        if let Some(alias) = crate::digest::digest_alias(&output)
            && let Some(whole) = self.core.stored_output_text(id, alias).await
        {
            bytes = board
                .outputs
                .get(alias)
                .map_or(whole.len() as u64, |stored| stored.bytes);
            output = whole;
        }
        Ok(self
            .command_tail(id, output.into_bytes(), bytes, false)
            .await)
    }

    pub(super) async fn delete_background_output(&self, id: &ConversationId) -> Result<()> {
        let board = self.core.board(id).await?;
        self.core
            .store()
            .delete_streams(
                board
                    .background_runs
                    .keys()
                    .map(|run| format!("background-output:{id}:{run}"))
                    .collect(),
            )
            .await?;
        Ok(())
    }

    /// Work of the session's, not a background run, that its Stop ends as it ends them (a new
    /// worktree's setup script): `name` keeps it apart from their ids. It is stoppable until
    /// the returned guard drops.
    pub(super) fn stoppable(&self, id: &ConversationId, name: &str) -> Stoppable<'_> {
        let key = (id.clone(), name.to_owned());
        let cancel = CancellationToken::new();
        self.background_runs
            .running
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key.clone(), cancel.clone());
        Stoppable {
            runs: &self.background_runs,
            key,
            cancel,
        }
    }

    /// Ends the session's background runs (the user's Stop, an archive or delete): each is
    /// recorded stopped and tells nothing.
    pub(crate) fn stop_background_runs(&self, id: &ConversationId) {
        let running = self
            .background_runs
            .running
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        for ((of, _), cancel) in running.iter() {
            if of == id {
                cancel.cancel();
            }
        }
    }

    /// The user's Stop of one background run (the context card's): it ends, and the thread
    /// hears it in the run's request. A run already over is refused.
    pub async fn stop_background_run(&self, id: ConversationId, run: String) -> Result<()> {
        // Marked and cancelled under the lock: its end can't be chosen in between.
        let stopped = {
            let running = self
                .background_runs
                .running
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let key = (id.clone(), run.clone());
            running.get(&key).map(|cancel| {
                self.background_runs
                    .stopped_alone
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(key);
                cancel.cancel();
            })
        };
        match stopped {
            Some(()) => Ok(()),
            None => self.stop_missed_run(&id, &run).await,
        }
    }

    /// The user's × on an ended preview's or run's Background row (Q94): it leaves the rows;
    /// its record and output stay. One still running is refused.
    pub async fn dismiss_background(&self, id: ConversationId, row: String) -> Result<()> {
        let board = self.core.board(&id).await?;
        let ended = match (board.previews.get(&row), board.background_runs.get(&row)) {
            (Some(preview), _) => !preview.state.is_live(),
            (None, Some(run)) => run.state != BackgroundRunState::Running,
            (None, None) => {
                return Err(crate::Error::Invalid(format!(
                    "there is no {row} in this session"
                )));
            }
        };
        if !ended {
            return Err(crate::Error::Invalid(format!("{row} still runs")));
        }
        self.core
            .record_conversation(
                &id,
                vec![DomainEvent::BackgroundCleared {
                    conversation_id: id.clone(),
                    ids: vec![row],
                }],
            )
            .await?;
        Ok(())
    }

    /// The user's Restart on a failed run's Background row (Q94): the row leaves, and the
    /// thread is asked to run the same command again with the same tool, as only it may (its
    /// access, its checks); its new run, if it outlasts a tool call, is a row of its own. A run
    /// still going, or one that ended well or was stopped, is refused.
    pub async fn restart_background_run(&self, id: ConversationId, run: String) -> Result<()> {
        let board = self.core.board(&id).await?;
        let Some(run) = board.background_runs.get(&run).cloned() else {
            return Err(crate::Error::Invalid(format!(
                "there is no {run} in this session"
            )));
        };
        let ended = match &run.state {
            BackgroundRunState::Finished { status } if status != "exit 0" => status.clone(),
            BackgroundRunState::Interrupted { reason } => reason.clone(),
            _ => {
                return Err(crate::Error::Invalid(format!(
                    "{} didn't fail: only a failed run restarts",
                    run.id
                )));
            }
        };
        self.core
            .record_conversation(
                &id,
                vec![DomainEvent::BackgroundCleared {
                    conversation_id: id.clone(),
                    ids: vec![run.id.clone()],
                }],
            )
            .await?;
        let envelope = Envelope {
            kind: InjectionKind::BackgroundRun,
            label: format!("{} restart", run.id),
            task_id: None,
            text: restart_text(&run, &ended),
        };
        let conv = self
            .queue_envelope(&id, envelope, run.request_id.clone())
            .await;
        self.settle_requests(&id).await;
        if let Some(conv) = conv {
            self.kick(&conv);
        }
        Ok(())
    }

    /// The user's Stop of a run its row shows running though nothing watches it any more (its
    /// end was missed): it is recorded stopped and the thread hears it, as for any Stop.
    async fn stop_missed_run(&self, id: &ConversationId, run: &str) -> Result<()> {
        let board = self.core.board(id).await?;
        let Some((mut run, ended)) = board
            .background_runs
            .get(run)
            .filter(|stored| stored.state == BackgroundRunState::Running)
            .and_then(|stored| Some((stored.clone(), Ending::claim(self, id, &stored.id)?)))
        else {
            return Err(crate::Error::Invalid(format!("{run} isn't running")));
        };
        run.ended_at_ms = Some(now_ms());
        let (_, kept) = self
            .retain_background_output(&run, "stopped", String::new())
            .await;
        run.state = BackgroundRunState::Stopped;
        let envelope = Envelope {
            kind: InjectionKind::BackgroundRun,
            label: format!("{} stopped", run.id),
            task_id: None,
            text: stopped_text(&run, &kept),
        };
        ended.finish(self.end_with_message(run, envelope).await);
        Ok(())
    }

    /// At startup: a background run the restart cut off is recorded so, and the thread hears
    /// it in the run's request, so it can run it again.
    pub(crate) async fn recover_background_runs(&self, id: &ConversationId) {
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        let mut cut: Vec<(BackgroundRun, Ending)> = board
            .background_runs
            .values()
            .filter(|run| run.state == BackgroundRunState::Running)
            .filter_map(|run| Some((run.clone(), Ending::claim(self, id, &run.id)?)))
            .collect();
        cut.sort_by_key(|(run, _)| background_run_number(&run.id));
        for (mut run, ended) in cut {
            run.state = BackgroundRunState::Interrupted {
                reason: "a restart".into(),
            };
            run.ended_at_ms = Some(now_ms());
            ended.finish(self.store_background_run(&run).await);
            let envelope = Envelope {
                kind: InjectionKind::BackgroundRun,
                label: format!("{} cut off", run.id),
                task_id: None,
                text: format!(
                    "[{} was cut off by a restart before it finished · {} `{}`: no result came. \
                     Run it again if you still need it.]",
                    run.id, run.tool, run.command
                ),
            };
            self.deliver_for(id, envelope, run.request_id.clone()).await;
        }
    }

    /// Whether it was stored.
    async fn store_background_run(&self, run: &BackgroundRun) -> bool {
        let stored = self
            .core
            .record_conversation(
                &run.conversation_id,
                vec![DomainEvent::BackgroundRunUpdated { run: run.clone() }],
            )
            .await;
        if let Err(err) = &stored {
            tracing::warn!(run = %run.id, error = %err, "could not record a background run");
        }
        stored.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_runs_number_is_read_from_its_id() {
        assert_eq!(background_run_number("run-12"), 12);
        assert_eq!(background_run_number("check-1"), 0);
    }

    /// #89: the thread's lines about a background run rest on what Brigadier tells it: how long
    /// a stopped run really ran, and no time guess when a call goes on in the background.
    #[test]
    fn a_background_runs_words_carry_no_time_guess() {
        let backgrounded = backgrounded_text("run-3");
        assert!(!backgrounded.contains("how long, then"));
        assert!(backgrounded.contains("with no guess at how long it has left"));
        let run = BackgroundRun {
            id: "run-3".into(),
            conversation_id: ConversationId("c".into()),
            request_id: None,
            tool: "run".into(),
            name: None,
            command: "cargo test".into(),
            started_at_ms: 1_000,
            ended_at_ms: Some(111_000),
            state: BackgroundRunState::Stopped,
            item_id: None,
        };
        assert!(
            stopped_text(&run, "108 lines").starts_with("[The user stopped run-3 after 2 minutes,")
        );
        assert!(stopped_text(&run, "").contains("after 2 minutes, before it finished"));
    }
}
