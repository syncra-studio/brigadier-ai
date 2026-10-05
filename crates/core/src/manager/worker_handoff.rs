//! Worker hand-off by size (PLAN.md §7): a worker whose context passes the hand-off size
//! continues in a fresh session of the same model, in the same worktree, instead of sending an
//! ever larger context with every request.
//!
//! - **Only between turns.** When a worker's context passes the size mid-turn, it is asked (a
//!   steer) to finish the step it is in and end its turn with a handoff note. The fresh session
//!   starts when that turn ends. A message for a worker that is between turns (or hibernated)
//!   with a context past the size starts the fresh session at once, without a new note: the
//!   worker's report and transcript carry what it did.
//! - **Nothing is lost.** Brigadier writes `<scratch>/handoff/`: note.md (the worker's own note),
//!   spec.md, progress.md and diff.patch as a fallback hand-off writes them, and transcript.md,
//!   every message, command and tool call of the task in full, as Brigadier recorded them. The
//!   fresh session gets the note and the last messages word for word, and searches the rest.
//! - **Same model, same attempt.** It is not a reroute: the task keeps its model, attempt and
//!   access. A limit or error hand-on still wins when both are due.
//! - **Stalls too.** The stall watchdog ([`super::watchdog`]) hands a worker that went silent
//!   mid-turn, and didn't answer a nudge, over the same way, closing its stuck turn.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use brigadier_providers::{ItemStatus, NoticeLevel, ProviderEvent, Role, TurnInput};
use brigadier_store::StreamPage;

use super::SessionManager;
use super::conversation::Cli;
use super::workers::TaskLive;
use crate::model::{DomainEvent, TaskId, streams};
use crate::work::{Task, TaskState};
use crate::{Error, Result, knowledge};

/// Worker events read per page for the transcript.
const EVENTS_PAGE: u32 = 2_000;
/// The previous session's last messages the fresh one gets word for word.
const TAIL_MESSAGES: usize = 6;
/// … each up to this many bytes (the rest is in transcript.md).
const TAIL_MESSAGE_BYTES: usize = 8_000;
/// How the notice of a hand-over starts; hand-overs are counted by it.
const HANDED_OVER: &str = "Continued in a fresh session of the same model";

/// The steer that asks a worker to end its turn with a handoff note.
fn wrap_up_prompt(tokens: i64) -> String {
    format!(
        "[Brigadier] Your context is now about {}k tokens, so Brigadier will continue this task in \
a fresh session of the same model, in the same worktree, with your full transcript kept on disk. \
If the task is done apart from its report, call submit_report now as usual. Otherwise stop \
starting new work: finish only the step you are in, commit the finished steps (not broken work), \
and end your turn with a handoff note for the fresh session instead of a report, written so that \
someone with no memory of this session can carry on. Write it in plain text under these \
headings: Goal and where it stands; Done (with commit hashes); In progress (exact files and \
state, and anything uncommitted); Next steps (ordered and concrete); Decisions and approvals \
already given (including the orchestrator's answers and go-aheads, so they aren't asked again); \
Gotchas learned (versions, commands that work, traps); How to verify (the exact commands and \
what passing looks like). Include anything you found or ruled out that isn't in the code. At \
most about 800 words. Don't call submit_report for this.",
        tokens / 1_000
    )
}

impl SessionManager {
    /// A worker reported its context size: past the hand-off size mid-turn, it is asked to wrap
    /// up with a handoff note.
    pub(crate) async fn worker_context(
        &self,
        live: &Arc<TaskLive>,
        cli: &Arc<Cli>,
        tokens: i64,
        window: Option<i64>,
    ) {
        let start = live.note_context(tokens, window).await;
        if !self.worker_handoff_due(tokens, Some(start), live.window().await)
            || !live.ask_handoff().await
        {
            return;
        }
        tracing::info!(task = %live.id, tokens, "asking the worker to wrap up for a hand-off");
        self.record_worker_event(
            &live.id,
            ProviderEvent::Notice {
                level: NoticeLevel::Info,
                message: format!(
                    "Context at about {}k tokens: asked the worker to end its turn with a \
                     handoff note, so a fresh session can continue.",
                    tokens / 1_000
                ),
            },
        )
        .await;
        // Not from inside the worker's event pump: a steer may wait on the CLI.
        let cli = cli.clone();
        self.spawn(async move {
            if let Err(err) = cli
                .session
                .steer(TurnInput::text(wrap_up_prompt(tokens)))
                .await
            {
                tracing::warn!(error = %err, "could not ask the worker to wrap up");
            }
        });
    }

    /// Whether the worker's session, at `tokens` and started at `start`, is handed over now
    /// (see [`knowledge::worker_handoff_due`]).
    /// `window` is its model's context window, when known.
    pub(crate) fn worker_handoff_due(
        &self,
        tokens: i64,
        start: Option<i64>,
        window: Option<i64>,
    ) -> bool {
        knowledge::worker_handoff_due(tokens, start, knowledge::worker_handoff_tokens(window))
    }

    /// The worker's context is past the hand-off size (after a restart, from its recorded
    /// events).
    pub(crate) async fn worker_over_handoff(&self, live: &Arc<TaskLive>, id: &TaskId) -> bool {
        let (tokens, start) = match live.context().await {
            (Some(tokens), start) => (Some(tokens), start),
            (None, _) => self.last_worker_context(id).await,
        };
        match tokens {
            Some(tokens) => self.worker_handoff_due(tokens, start, live.window().await),
            None => false,
        }
    }

    /// The last context size the worker's latest CLI session reported, and its first.
    pub(crate) async fn last_worker_context(&self, id: &TaskId) -> (Option<i64>, Option<i64>) {
        let Ok(page) = self
            .core
            .store()
            .read_stream(
                streams::task(id),
                StreamPage {
                    before: None,
                    kinds: vec!["worker.event".into()],
                    limit: EVENTS_PAGE,
                },
            )
            .await
        else {
            return (None, None);
        };
        let history_complete = page.len() < EVENTS_PAGE as usize;
        let events: Vec<ProviderEvent> = page
            .iter()
            .filter_map(
                |stored| match serde_json::from_str::<DomainEvent>(stored.payload.get()) {
                    Ok(DomainEvent::WorkerEvent { event, .. }) => Some(event),
                    _ => None,
                },
            )
            .collect();
        session_sizes(&events, history_complete)
    }

    /// Continues `task` in a fresh session of the same model: closes its CLI (between turns,
    /// or mid-turn when it stalled), writes the hand-off files, and starts the new session with
    /// `note`, the last messages and `pending` (a message still to act on). `from` is the CLI
    /// session the hand-over was decided for ([`TaskLive::generation`]). Boxed: the worker it
    /// starts can come back here, and a recursive future must name its `Send` bound.
    pub(crate) fn hand_over_worker<'a>(
        &'a self,
        live: &'a Arc<TaskLive>,
        task: &'a Task,
        from: u64,
        note: Option<String>,
        pending: Option<String>,
        why: Handover,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let _handing = live.reroute.lock().await;
            self.hand_over(live, task, from, note, pending, why).await
        })
    }

    /// [`Self::hand_over_worker`] for a caller that holds the task's `reroute` lock (the
    /// stall watchdog, which closed the stalled session itself).
    pub(crate) fn hand_over_held<'a>(
        &'a self,
        live: &'a Arc<TaskLive>,
        task: &'a Task,
        from: u64,
        why: Handover,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(self.hand_over(live, task, from, None, None, why))
    }

    async fn hand_over(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        from: u64,
        note: Option<String>,
        pending: Option<String>,
        why: Handover,
    ) -> Result<()> {
        // Another hand-over came first (a message arrived while the turn that asked for a
        // note was ending, or the other way round): its fresh session must not be closed
        // mid-turn. It has the old session's last messages, the note among them; a pending
        // message goes to it.
        if live.generation().await != from {
            tracing::info!(task = %task.id, "the worker already continues in a fresh session");
            if let Some(pending) = pending {
                // A turn it starts counts against its overnight run's workers.
                self.admit_run_task(task).await?;
                live.deliver(TurnInput::text(pending)).await?;
                self.release_if_idle(task).await;
            }
            return Ok(());
        }
        // Handed over while paused, and now a message came: it starts the fresh session
        // prepared then, as a message starts a paused worker.
        if let Some(mut first) = live.take_held_handover().await {
            if let Some(pending) = pending {
                first.append_text(&format!("\n\nWaiting for you now:\n{pending}"));
            }
            return self.start_fresh(live, task, first).await;
        }
        // Stopped meanwhile: its session is closed with it, not handed over.
        if self
            .task_by_id(&task.conversation_id, &task.id)
            .await?
            .state
            .is_final()
        {
            return Ok(());
        }
        let tokens = match live.context().await {
            (Some(tokens), _) => Some(tokens),
            (None, _) => self.last_worker_context(&task.id).await.0,
        };
        live.close_cli().await;
        let task = self.task_by_id(&task.conversation_id, &task.id).await?;
        if task.state.is_final() {
            return Ok(());
        }
        let (dir, has_diff) = self.write_handoff_files(&task).await?;
        let (transcript, tail) = self.worker_transcript(&task.id).await;
        // A turn that ended before the ask reached it has no note: its last message is in the
        // tail anyway.
        let note = note.filter(|note| note.to_lowercase().contains("next steps"));
        {
            let (dir, note) = (dir.clone(), note.clone());
            super::blocking(move || {
                let io =
                    |err: std::io::Error| Error::Invalid(format!("writing the hand-off: {err}"));
                std::fs::write(dir.join("transcript.md"), transcript).map_err(io)?;
                match note {
                    Some(note) => std::fs::write(dir.join("note.md"), note).map_err(io)?,
                    None => match std::fs::remove_file(dir.join("note.md")) {
                        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                            return Err(io(err));
                        }
                        _ => {}
                    },
                }
                Ok(())
            })
            .await?;
        }
        let waits = pending.is_some();
        let text = handover_text(
            &task,
            &dir,
            HandoverFacts {
                why,
                tokens,
                has_note: note.is_some(),
                has_diff,
            },
            &tail,
            pending,
        );
        let size = match why {
            Handover::Size => tokens.map_or_else(String::new, |tokens| {
                format!(" at about {}k tokens", tokens / 1_000)
            }),
            Handover::Stalled { silent } => {
                format!(
                    " after {} without activity",
                    super::watchdog::spoken(silent)
                )
            }
        };
        self.record_worker_event(
            &task.id,
            ProviderEvent::Notice {
                level: NoticeLevel::Info,
                message: format!("{HANDED_OVER}{size}; the hand-off is in {}.", dir.display()),
            },
        )
        .await;
        tracing::info!(task = %task.id, ?tokens, ?why, with_note = note.is_some(), "worker handed over to a fresh session");
        let workspace = task
            .workspace
            .as_ref()
            .ok_or_else(|| Error::Invalid("the task has no workspace".into()))?;
        let files = self
            .worker_files(&task, &PathBuf::from(&workspace.scratch))
            .await;
        let first = TurnInput::with_files(text, files);
        match self.task_by_id(&task.conversation_id, &task.id).await {
            // Stopped meanwhile (the user's stop button): nothing starts again.
            Ok(now) if now.state.is_final() => Ok(()),
            // Paused meanwhile, and no message waits: the fresh session starts on resume.
            Ok(now) if now.state == TaskState::Paused && !waits => {
                live.hold_handover(first).await;
                Ok(())
            }
            _ => self.start_fresh(live, &task, first).await,
        }
    }

    /// Starts the fresh session a hand-over prepared, with its first message.
    pub(crate) async fn start_fresh(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        first: TurnInput,
    ) -> Result<()> {
        let task = self.task_by_id(&task.conversation_id, &task.id).await?;
        if task.state.is_final() {
            return Ok(());
        }
        let subject = match &task.subject {
            Some(id) => self.task_by_id(&task.conversation_id, id).await.ok(),
            None => None,
        };
        live.allow_revival().await;
        live.clear_transient().await;
        self.launch_worker(
            live,
            &task,
            subject.as_ref(),
            brigadier_providers::Origin::New,
            first,
        )
        .await
    }

    /// Every recorded event of the task's workers, in full, oldest first, and their last
    /// messages.
    async fn worker_transcript(&self, id: &TaskId) -> (String, Vec<String>) {
        let mut events: Vec<ProviderEvent> = Vec::new();
        let mut before = None;
        loop {
            let page = self
                .core
                .store()
                .read_stream(
                    streams::task(id),
                    StreamPage {
                        before,
                        kinds: vec!["worker.event".into()],
                        limit: EVENTS_PAGE,
                    },
                )
                .await
                .unwrap_or_default();
            let Some(oldest) = page.last() else {
                break;
            };
            before = Some(oldest.stream_seq);
            let full = page.len() == EVENTS_PAGE as usize;
            events.extend(page.iter().filter_map(|stored| {
                match serde_json::from_str::<DomainEvent>(stored.payload.get()) {
                    Ok(DomainEvent::WorkerEvent { event, .. }) => Some(event),
                    _ => None,
                }
            }));
            if !full {
                break;
            }
        }
        events.reverse();
        let mut text = String::from(
            "# The task's full transcript\n\nEvery message, command and tool call so far, oldest \
             first, as Brigadier recorded them (long command and tool outputs are clipped where \
             the CLI clipped them).\n",
        );
        let mut tail: Vec<String> = Vec::new();
        for event in &events {
            if let ProviderEvent::Message {
                role: Role::Assistant,
                text: said,
                ..
            } = event
            {
                tail.push(said.clone());
            }
            transcript_entry(&mut text, event);
        }
        let tail = tail.split_off(tail.len().saturating_sub(TAIL_MESSAGES));
        (text, tail)
    }
}

/// One transcript entry, in full.
fn transcript_entry(text: &mut String, event: &ProviderEvent) {
    match event {
        ProviderEvent::SessionStarted { model, .. } => {
            let _ = write!(
                text,
                "\n## A session started ({})\n",
                model.as_deref().unwrap_or("its default model")
            );
        }
        ProviderEvent::Message {
            role, text: said, ..
        } => {
            let who = match role {
                Role::Assistant => "The worker",
                _ => "The worker was told",
            };
            let _ = write!(text, "\n### {who}\n\n{said}\n");
        }
        ProviderEvent::Command {
            command,
            status: ItemStatus::Completed | ItemStatus::Failed,
            exit_code,
            output,
            ..
        } => {
            let code = exit_code.map_or_else(String::new, |code| format!(" (exit {code})"));
            let _ = write!(text, "\n### Ran `{command}`{code}\n");
            if let Some(output) = output.as_deref().filter(|output| !output.trim().is_empty()) {
                let _ = write!(text, "\n```\n{}\n```\n", output.trim_end());
            }
        }
        ProviderEvent::ToolCall {
            name,
            input,
            status: ItemStatus::Completed | ItemStatus::Failed,
            output,
            ..
        } => {
            let _ = write!(text, "\n### Used {name}\n");
            if let Some(input) = input {
                let _ = write!(text, "\nInput: {input}\n");
            }
            if let Some(output) = output.as_deref().filter(|output| !output.trim().is_empty()) {
                let _ = write!(text, "\n```\n{}\n```\n", output.trim_end());
            }
        }
        ProviderEvent::FileChanges {
            changes,
            status: ItemStatus::Completed,
            ..
        } => {
            let paths: Vec<&str> = changes.iter().map(|change| change.path.as_str()).collect();
            let _ = write!(text, "\n### Changed files: {}\n", paths.join(", "));
        }
        ProviderEvent::Error { error } if !error.will_retry => {
            let _ = write!(text, "\n### Stopped by an error\n\n{}\n", error.message);
        }
        ProviderEvent::Notice { message, .. } => {
            let _ = write!(text, "\n### Brigadier\n\n{message}\n");
        }
        _ => {}
    }
}

/// Why a worker continues in a fresh session of the same model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Handover {
    /// Its context passed the hand-off size.
    Size,
    /// It went silent mid-turn (for `silent` ms) and didn't answer a nudge.
    Stalled { silent: i64 },
}

/// What the fresh session's first message says about the hand-over.
struct HandoverFacts {
    why: Handover,
    /// The earlier session's context size, when known.
    tokens: Option<i64>,
    has_note: bool,
    has_diff: bool,
}

/// The fresh session's first message.
fn handover_text(
    task: &Task,
    dir: &std::path::Path,
    facts: HandoverFacts,
    tail: &[String],
    pending: Option<String>,
) -> String {
    let HandoverFacts {
        why,
        tokens,
        has_note,
        has_diff,
    } = facts;
    let why = match why {
        Handover::Size => {
            let size = tokens.map_or_else(String::new, |tokens| {
                format!(" (about {}k tokens)", tokens / 1_000)
            });
            format!(
                "Your earlier session's context had grown large{size}, so it was closed between \
                 turns to give you room"
            )
        }
        Handover::Stalled { silent } => format!(
            "Your earlier session went silent mid-turn for {} and didn't answer a nudge (a \
             command or the CLI may have hung), so it was closed. Don't run a command that may \
             hang without a timeout, or start a server in the foreground",
            super::watchdog::spoken(silent)
        ),
    };
    let mut text = format!(
        "[Brigadier] You are continuing task-{} in a fresh session. {why}; you are the same \
         model, in the same place, and nothing in the worktree was touched. Its hand-off is in {}: ",
        task.number,
        dir.display()
    );
    if has_note {
        text.push_str(
            "note.md (its own handoff note: read it first, including its traps learned and how \
             to verify), ",
        );
    }
    text.push_str(
        "spec.md (the task and every later instruction from the orchestrator), progress.md (a \
         short log of what it did) and transcript.md (the whole transcript, word for word: search \
         it with rg for any detail instead of redoing work)",
    );
    if task.kind.writes() {
        text.push_str(if has_diff {
            ", and diff.patch (the changes so far, as the worktree holds them). Keep the work \
             already done unless it is wrong, and don't redo it"
        } else {
            ". There are no changes in the worktree yet"
        });
    } else {
        text.push_str(". Don't redo what is done");
    }
    text.push_str(
        ". Decisions and commitments made earlier still hold. Then carry on with the task and \
         report with submit_report as your rules say.",
    );
    if !tail.is_empty() {
        text.push_str("\n\nThe earlier session's last messages, oldest first, word for word:");
        for said in tail {
            let said = if said.len() > TAIL_MESSAGE_BYTES {
                let mut end = TAIL_MESSAGE_BYTES;
                while !said.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{} […cut; the rest is in transcript.md]", &said[..end])
            } else {
                said.clone()
            };
            let _ = write!(text, "\n\n---\n{said}");
        }
        text.push_str("\n\n---");
    }
    if let Some(pending) = pending {
        let _ = write!(text, "\n\nWaiting for you now:\n{pending}");
    }
    text
}

/// The last context size of the latest CLI session in `events` (newest first), and its first:
/// the session may have been resumed, which starts it again under the same id. Without any
/// session start in `events` (a long session), only the last size is known. A start under the
/// same id may be a resume: its first size is trusted only when an older session or the start
/// of the stream proves that the original start is present.
fn session_sizes(events: &[ProviderEvent], history_complete: bool) -> (Option<i64>, Option<i64>) {
    let (mut last, mut first) = (None, None);
    // The sizes since the previous session start seen (newest first, so the later ones).
    let (mut run_last, mut run_first) = (None, None);
    let mut session: Option<&str> = None;
    for event in events {
        match event {
            ProviderEvent::ContextSize { used_tokens, .. } => {
                run_last.get_or_insert(*used_tokens);
                run_first = Some(*used_tokens);
            }
            ProviderEvent::SessionStarted { native_id, .. } => {
                if session.is_some_and(|id| id != native_id.as_str()) {
                    return (last, first);
                }
                session = Some(native_id);
                last = last.or(run_last.take());
                first = run_first.take().or(first);
            }
            _ => {}
        }
    }
    if session.is_none() {
        return (run_last, None);
    }
    (last, if history_complete { first } else { None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use brigadier_providers::NoticeLevel;

    fn started(id: &str) -> ProviderEvent {
        ProviderEvent::SessionStarted {
            native_id: id.into(),
            model: None,
            cwd: None,
            cli_version: None,
        }
    }

    fn size(tokens: i64) -> ProviderEvent {
        ProviderEvent::ContextSize {
            used_tokens: tokens,
            window_tokens: None,
        }
    }

    #[test]
    fn sizes_come_from_the_latest_session_across_resumes() {
        // Oldest first: a session, a fresh one after a hand-over, the fresh one resumed.
        let mut events = vec![
            started("a"),
            size(10_000),
            size(170_000),
            started("b"),
            size(28_000),
            size(60_000),
            started("b"),
            size(61_000),
        ];
        events.reverse();
        assert_eq!(session_sizes(&events, true), (Some(61_000), Some(28_000)));
    }

    #[test]
    fn a_session_without_sizes_has_none() {
        let mut events = vec![started("a"), size(10_000), started("b")];
        events.reverse();
        assert_eq!(session_sizes(&events, true), (None, None));
    }

    #[test]
    fn a_long_session_has_only_its_last_size() {
        let mut events = vec![size(90_000), size(95_000)];
        events.reverse();
        assert_eq!(session_sizes(&events, true), (Some(95_000), None));
    }

    #[test]
    fn an_incomplete_history_cannot_use_a_resume_as_the_baseline() {
        // The original start at 10k fell outside the replay page. The start still visible
        // is a resume of the same session at 110k, not its original baseline.
        let mut events = vec![size(115_000), size(110_000), started("a")];
        events.resize_with(EVENTS_PAGE as usize, || {
            notice("An older event in the same session".into())
        });
        let (last, first) = session_sizes(&events, false);
        assert_eq!((last, first), (Some(115_000), None));
        // Use the plain 100k threshold, not the resume's 110k + 50k guard.
        assert!(knowledge::worker_handoff_due(last.unwrap(), first, 100_000));
    }

    #[test]
    fn an_older_session_proves_the_baseline_even_in_an_incomplete_history() {
        let mut events = vec![
            started("a"),
            size(170_000),
            started("b"),
            size(28_000),
            started("b"),
            size(61_000),
        ];
        events.reverse();
        assert_eq!(session_sizes(&events, false), (Some(61_000), Some(28_000)));
    }

    fn notice(message: String) -> ProviderEvent {
        ProviderEvent::Notice {
            level: NoticeLevel::Info,
            message,
        }
    }
}
