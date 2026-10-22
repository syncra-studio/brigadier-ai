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

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use brigadier_providers::{NoticeLevel, ProviderEvent, TurnInput};
use brigadier_store::StreamPage;

use super::SessionManager;
use super::conversation::Cli;
use super::workers::TaskLive;
use crate::model::{DomainEvent, TaskId, streams};
use crate::work::{Task, TaskState};
use crate::{Error, Result, knowledge};

/// Worker events read per page for the transcript.
const EVENTS_PAGE: u32 = 2_000;
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
already given (including the thread's answers and go-aheads, so they aren't asked again); \
Gotchas learned (versions, commands that work, traps); Checks that passed (each command \
and the commit it passed at); How to verify (the exact commands and what passing looks like). Include anything you found or ruled out that isn't in the code. At \
most about 800 words. Don't call submit_report for this.",
        tokens / 1_000
    )
}

impl SessionManager {
    /// A worker reported its context size: past the hand-off size mid-turn, it is asked to wrap
    /// up with a handoff note, again after each further stretch of growth; past the hard line,
    /// its turn is stopped and a fresh session takes over (BUILD-PLAN D2.3).
    pub(crate) async fn worker_context(
        &self,
        live: &Arc<TaskLive>,
        cli: &Arc<Cli>,
        tokens: i64,
        window: Option<i64>,
    ) {
        let start = live.note_context(tokens, window).await;
        let window = live.window().await;
        let hard = knowledge::hard_line(cli.provider, window);
        // A session that starts past the line can't be helped by another: only growth counts.
        // Without its model's window the line isn't known: a guess (the 200k default) would
        // stop a big-window worker far too early, so only the wrap-up steer applies then.
        if window.is_some() && tokens >= hard && tokens > start {
            self.force_worker_handoff(live, NearLimit::HardLine { tokens })
                .await;
            return;
        }
        let at = knowledge::worker_handoff_at(window);
        if !self.worker_handoff_due(tokens, Some(start), window)
            || !live
                .ask_handoff(tokens, knowledge::steer_stretch(at, hard))
                .await
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

    /// Stops the worker's running turn and continues the task in a fresh session of the same
    /// model, without waiting for the turn to end: its context passed the hard line, or its CLI
    /// began compacting and the session's hook refused it (BUILD-PLAN D2.3).
    pub(crate) async fn force_worker_handoff(&self, live: &Arc<TaskLive>, why: NearLimit) {
        let Some(from) = live.force_handoff().await else {
            return;
        };
        let Ok(task) = self.task_by_id(&live.conversation_id, &live.id).await else {
            return;
        };
        let words = match why {
            NearLimit::HardLine { tokens } => format!(
                "Context at about {}k tokens, near the model's limit: stopped the turn there, and a fresh session continues the task.",
                tokens / 1_000
            ),
            NearLimit::CompactionRefused => "Its CLI began compacting its context; that was refused, and a fresh session continues the task instead."
                .to_owned(),
        };
        tracing::info!(task = %live.id, ?why, "stopping a worker's turn for a hand-off");
        self.record_worker_event(
            &live.id,
            ProviderEvent::Notice {
                level: NoticeLevel::Info,
                message: words,
            },
        )
        .await;
        // Not from inside the worker's event pump: closing its CLI waits for the pump.
        let manager = self.arc();
        let live = live.clone();
        self.spawn(async move {
            if let Err(err) = manager
                .hand_over_worker(&live, &task, from, None, None, Handover::NearLimit(why))
                .await
            {
                let reason = format!("The task could not continue in a fresh session: {err}");
                manager.worker_failed(&task, &reason).await;
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
        // The provider isn't known here: the lower line of the two.
        let cap = knowledge::worker_hard_line(window);
        knowledge::worker_handoff_due(tokens, start, knowledge::worker_handoff_at(window), cap)
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
    /// or after its process ended), writes the hand-off files, and starts the new session with
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
    /// recovery path, which closed the previous session itself).
    pub(crate) fn hand_over_held<'a>(
        &'a self,
        live: &'a Arc<TaskLive>,
        task: &'a Task,
        from: u64,
        why: Handover,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(self.hand_over(live, task, from, None, None, why))
    }

    /// [`Self::hand_over_worker`] for a caller that holds the task's `reroute` lock.
    pub(crate) async fn hand_over(
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
        live.next_start(crate::routing::WriteCause::Handoff);
        live.close_cli().await;
        let task = self.task_by_id(&task.conversation_id, &task.id).await?;
        if task.state.is_final() {
            return Ok(());
        }
        // A turn that ended before the ask reached it has no note: its last message is in the
        // tail anyway.
        let note = note.filter(|note| note.to_lowercase().contains("next steps"));
        let files = self.write_successor_files(&task, note).await?;
        let dir = files.dir.clone();
        let waits = pending.is_some();
        let text = handover_text(&task, HandoverFacts { why, tokens }, &files, pending);
        let text = self.with_constraints(&task.conversation_id, text).await;
        let size = match why {
            Handover::Size => tokens.map_or_else(String::new, |tokens| {
                format!(" at about {}k tokens", tokens / 1_000)
            }),
            Handover::Interrupted => " after its process ended".to_owned(),
            Handover::Restarted => {
                " after Brigadier restarted and its session couldn't be resumed".to_owned()
            }
            Handover::ContextWindow => " after its context passed the model's limit".to_owned(),
            Handover::NearLimit(_) => {
                " after its turn was stopped near the model's limit".to_owned()
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
        tracing::info!(task = %task.id, ?tokens, ?why, with_note = files.has_note, "worker handed over to a fresh session");
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

    /// Writes a successor's whole hand-off (BUILD-PLAN D2.4), the same for a size hand-off, a
    /// crash, a restart without the CLI's files and a move to another model: spec.md,
    /// progress.md and diff.patch ([`Self::write_handoff_files`]), the streamed transcript
    /// with every kept output, decisions.md, and `note` as note.md, or without one
    /// state.md, the state of work built from the transcript and marked unchecked.
    pub(crate) async fn write_successor_files(
        &self,
        task: &Task,
        note: Option<String>,
    ) -> Result<SuccessorFiles> {
        // The model before may be on another provider, whose files go with its session.
        self.keep_saved_files(&task.id).await;
        let (dir, has_diff) = self.write_handoff_files(task).await?;
        let export = self.export_handoff_transcript(task, &dir).await;
        let decisions = self.decision_index_text(&task.conversation_id).await;
        let state = match &note {
            Some(_) => None,
            None => {
                let commit = match task
                    .workspace
                    .as_ref()
                    .and_then(|workspace| workspace.worktree.clone())
                {
                    Some(worktree) => self.last_commit(PathBuf::from(worktree)).await,
                    None => None,
                };
                Some(state_text(task, &export, commit, has_diff))
            }
        };
        let files = SuccessorFiles {
            dir: dir.clone(),
            has_diff,
            export,
            has_note: note.is_some(),
            has_state: state.is_some(),
            has_decisions: !decisions.is_empty(),
        };
        let decisions = (!decisions.is_empty()).then(|| {
            format!(
                "# The session's decisions, oldest first\n\nReplaced ones are marked; the active \
                 ones still hold.\n\n{decisions}\n"
            )
        });
        super::blocking(move || {
            let io = |err: std::io::Error| Error::Invalid(format!("writing the hand-off: {err}"));
            // Each file is this hand-off's, or gone: one an earlier hand-off left would mislead.
            for (name, text) in [
                ("note.md", note),
                ("state.md", state),
                ("decisions.md", decisions),
            ] {
                match text {
                    Some(text) => std::fs::write(dir.join(name), text).map_err(io)?,
                    None => match std::fs::remove_file(dir.join(name)) {
                        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                            return Err(io(err));
                        }
                        _ => {}
                    },
                }
            }
            Ok(())
        })
        .await?;
        Ok(files)
    }

    /// The last commit at `path` (a worktree), in a line: its short hash and subject.
    pub(crate) async fn last_commit(&self, path: PathBuf) -> Option<String> {
        let git = self.git.clone();
        super::blocking(move || {
            let worktree = git.open_worktree(&path).map_err(super::git_error)?;
            let head = worktree.head().map_err(super::git_error)?;
            let repo = git.open(&path).map_err(super::git_error)?;
            let log = repo.log(&head, 1).map_err(super::git_error)?;
            Ok(log.first().map(|commit| {
                let hash = &commit.commit.0;
                format!("{} {}", &hash[..12.min(hash.len())], commit.subject)
            }))
        })
        .await
        .ok()
        .flatten()
    }
}

/// Why a worker continues in a fresh session of the same model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Handover {
    /// Its context passed the hand-off size.
    Size,
    /// Its process ended before it reported, and its session couldn't be resumed.
    Interrupted,
    /// Brigadier restarted while it worked, and its session couldn't be resumed (its CLI's
    /// files were gone, BUILD-PLAN D2.4).
    Restarted,
    /// Its model refused the context as too long ("prompt is too long"): the same model goes
    /// on in a fresh session, never another model (BUILD-PLAN D1.3).
    ContextWindow,
    /// Its turn was stopped near the model's limit (BUILD-PLAN D2.3).
    NearLimit(NearLimit),
}

/// Why a worker's turn was stopped for a hand-off before it ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NearLimit {
    /// Its context passed the hard line ([`knowledge::hard_line`]).
    HardLine { tokens: i64 },
    /// Its CLI began compacting on its own, and the session's hook refused it.
    CompactionRefused,
}

/// What the fresh session's first message says about the hand-over.
struct HandoverFacts {
    why: Handover,
    /// The earlier session's context size, when known.
    tokens: Option<i64>,
}

/// A successor's hand-off folder, as [`SessionManager::write_successor_files`] wrote it.
pub(crate) struct SuccessorFiles {
    pub dir: PathBuf,
    /// diff.patch holds changes.
    pub has_diff: bool,
    /// transcript.md and outputs/, and what couldn't be exported.
    pub export: super::export::Export,
    /// note.md is the earlier session's own note.
    pub has_note: bool,
    /// state.md is the state of work Brigadier built from the transcript (no note).
    pub has_state: bool,
    /// decisions.md lists the session's decisions.
    pub has_decisions: bool,
}

impl SuccessorFiles {
    /// The folder's files, as a successor's first message names them: "spec.md (…), … and
    /// diff.patch (…)".
    pub(crate) fn listing(&self, writes: bool) -> String {
        let mut text = String::new();
        if self.has_note {
            text.push_str(
                "note.md (its own handoff note: read it first, including its traps learned and \
                 how to verify), ",
            );
        }
        if self.has_state {
            text.push_str(
                "state.md (where the work stood, built by Brigadier from the transcript because \
                 there is no note: unchecked, so check it against the worktree), ",
            );
        }
        text.push_str(
            "spec.md (the task and every later instruction from the thread), progress.md (a \
             short log of what it did), transcript.md (the whole transcript, word for word: search \
             it with rg for any detail instead of redoing work; each tool output cut there at 8 \
             KiB names its whole copy in outputs/)",
        );
        if self.has_decisions {
            text.push_str(
                ", decisions.md (the session's decisions, replaced ones marked: the active ones \
                 still hold)",
            );
        }
        if writes {
            text.push_str(if self.has_diff {
                " and diff.patch (the changes so far, as the worktree holds them)"
            } else {
                ". There are no changes in the worktree yet"
            });
        }
        text
    }

    /// The transcript's completeness and the earlier session's last messages, word for word,
    /// for the end of a successor's first message.
    pub(crate) fn push_tail(&self, text: &mut String) {
        let _ = write!(text, "\n\n{}", self.export.summary("transcript.md"));
        let tail = &self.export.tail;
        if tail.is_empty() {
            return;
        }
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
}

/// What every successor's first message says about its room: a successor that read how full
/// the earlier session was cut its own reads to one token and lost every one (#114).
pub(crate) const FULL_READS: &str = " Your own context starts nearly empty, so read what you \
     need in full: never cap a read or a command's output (max_output_tokens, a line limit) at a \
     few tokens to save room, as a cut read loses what it holds.";

/// The fresh session's first message.
fn handover_text(
    task: &Task,
    facts: HandoverFacts,
    files: &SuccessorFiles,
    pending: Option<String>,
) -> String {
    let HandoverFacts { why, tokens } = facts;
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
        Handover::Interrupted => "Your earlier session's process ended before it reported, and \
             the session couldn't be resumed. Check what was running (its output may be lost)"
            .to_owned(),
        Handover::Restarted => "Brigadier restarted while your earlier session was working, \
             and that session couldn't be resumed (its CLI's files were gone). Check what was \
             running (its output may be lost)"
            .to_owned(),
        Handover::ContextWindow => "Your earlier session's context passed the model's limit \
             (\"prompt is too long\"), so it was closed"
            .to_owned(),
        Handover::NearLimit(_) => "Your earlier session's context neared the model's limit in \
             the middle of a turn, so Brigadier stopped it there. The step it was in may be \
             unfinished: check the worktree (git status, git diff) and the end of transcript.md \
             before you go on"
            .to_owned(),
    };
    let mut text = format!(
        "[Brigadier] You are continuing task-{} in a fresh session. {why}; you are the same \
         model, in the same place, and nothing in the worktree was touched. Its hand-off is in {}: \
         {}",
        task.number,
        files.dir.display(),
        files.listing(task.kind.writes()),
    );
    text.push_str(if task.kind.writes() && files.has_diff {
        ". Keep the work already done unless it is wrong, and don't redo it"
    } else if task.kind.writes() {
        ""
    } else {
        ". Don't redo what is done"
    });
    text.push_str(
        ". Decisions and commitments made earlier still hold. Then carry on with the task and \
         report with submit_report as your rules say.",
    );
    text.push_str(FULL_READS);
    files.push_tail(&mut text);
    if let Some(pending) = pending {
        let _ = write!(text, "\n\nWaiting for you now:\n{pending}");
    }
    text
}

/// state.md: where the work stood, from the transcript, for a successor with no note.
fn state_text(
    task: &Task,
    export: &super::export::Export,
    commit: Option<String>,
    has_diff: bool,
) -> String {
    let mut text = format!(
        "# Where task-{} stood\n\nBuilt by Brigadier from the transcript, with no note from the \
         session that did the work: unchecked. Check it against the worktree before you rely on \
         it.\n\n",
        task.number
    );
    let _ = writeln!(
        text,
        "- The last step it took: {}",
        export.last_step.as_deref().unwrap_or("unknown")
    );
    if task.kind.writes() {
        let _ = writeln!(
            text,
            "- The last commit in the worktree: {}",
            commit.as_deref().unwrap_or("unknown")
        );
        text.push_str(if has_diff {
            "- Changes since the task's base: in diff.patch (committed or not).\n"
        } else {
            "- Changes since the task's base: none.\n"
        });
    }
    if let Some(last) = export.tail.last() {
        let mut end = last.len().min(TAIL_MESSAGE_BYTES);
        while !last.is_char_boundary(end) {
            end -= 1;
        }
        let _ = write!(text, "\n## Its last message\n\n{}\n", &last[..end]);
    }
    if !export.complete() {
        text.push_str(
            "\nThe transcript is not complete: see the first message for what is missing.\n",
        );
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
            file: None,
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
        assert!(knowledge::worker_handoff_due(
            last.unwrap(),
            first,
            100_000,
            i64::MAX
        ));
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
