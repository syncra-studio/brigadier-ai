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

/// What ends each running background run, by conversation and id.
#[derive(Default)]
pub(crate) struct BackgroundRuns {
    running: std::sync::Mutex<HashMap<(ConversationId, String), CancellationToken>>,
    /// The runs the user stopped one by one: the thread hears it.
    stopped_alone: std::sync::Mutex<HashSet<(ConversationId, String)>>,
    /// Held while a run's number is taken and its start recorded.
    numbering: tokio::sync::Mutex<()>,
}

#[cfg(all(test, unix))]
static FOREGROUND_GATES: std::sync::LazyLock<
    std::sync::Mutex<HashMap<ConversationId, std::sync::Arc<tokio::sync::Notify>>>,
> = std::sync::LazyLock::new(Default::default);

/// Tests can hold a foreground call while pipe gates are inspected, regardless of machine load.
#[cfg(all(test, unix))]
pub(super) fn hold_foreground(id: &ConversationId, gate: std::sync::Arc<tokio::sync::Notify>) {
    FOREGROUND_GATES.lock().unwrap().insert(id.clone(), gate);
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
        let gate = FOREGROUND_GATES.lock().unwrap().remove(id);
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
            // Its end wakes the thread: the turn that sent it there gets no guess.
            self.hold_guesses(id).await;
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
            let result = handle
                .await
                .unwrap_or_else(|err| Err(crate::Error::Invalid(err.to_string())));
            manager.background_run_ended(run, result, cancel).await;
        });
    }

    /// A background run ended: recorded, and unless the user's Stop or the session's close
    /// ended it, its result goes to the thread.
    async fn background_run_ended(
        &self,
        mut run: BackgroundRun,
        result: Result<(String, String)>,
        cancel: CancellationToken,
    ) {
        self.background_runs
            .running
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&(run.conversation_id.clone(), run.id.clone()));
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
                self.end_with_message(run, envelope).await;
            } else {
                self.store_background_run(&run).await;
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
        self.end_with_message(run, envelope).await;
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
    async fn end_with_message(&self, run: BackgroundRun, envelope: Envelope) {
        let id = run.conversation_id.clone();
        let conv = self
            .queue_envelope(&id, envelope, run.request_id.clone())
            .await;
        self.store_background_run(&run).await;
        // The request works again until the orchestrator has read it.
        self.settle_requests(&id).await;
        if let Some(conv) = conv {
            self.kick(&conv);
        }
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
        let running = self
            .background_runs
            .running
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let Some(cancel) = running.get(&(id.clone(), run.clone())) else {
            return Err(crate::Error::Invalid(format!("{run} isn't running")));
        };
        self.background_runs
            .stopped_alone
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert((id, run));
        cancel.cancel();
        Ok(())
    }

    /// At startup: a background run the restart cut off is recorded so, and the thread hears
    /// it in the run's request, so it can run it again.
    pub(crate) async fn recover_background_runs(&self, id: &ConversationId) {
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        let mut cut: Vec<BackgroundRun> = board
            .background_runs
            .values()
            .filter(|run| run.state == BackgroundRunState::Running)
            .cloned()
            .collect();
        cut.sort_by_key(|run| background_run_number(&run.id));
        for mut run in cut {
            run.state = BackgroundRunState::Interrupted {
                reason: "a restart".into(),
            };
            run.ended_at_ms = Some(now_ms());
            self.store_background_run(&run).await;
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

    async fn store_background_run(&self, run: &BackgroundRun) {
        if let Err(err) = self
            .core
            .record_conversation(
                &run.conversation_id,
                vec![DomainEvent::BackgroundRunUpdated { run: run.clone() }],
            )
            .await
        {
            tracing::warn!(run = %run.id, error = %err, "could not record a background run");
        }
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
