//! The thread as the CLI keeps it (S9, G11): the session's CLI session files, listed in its
//! `record/lineage.json`, read back as items ([`brigadier_providers::session_file`]), the live
//! stream's items after them, and its turns. A turn is a user message up to the CLI's end of
//! that turn. The files hold no turn ends of their own (Claude's only says the user's Stop),
//! so Brigadier writes the least the files lack, from the live stream, to
//! `record/turns.jsonl`: per turn its user message's id, how it ended, the error and when.
//! Nothing else of the conversation is copied. The app draws the thread from it alone (S10).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use brigadier_providers::session_file::{self, FileEntry};
use brigadier_providers::{
    BackgroundTask, BackgroundTaskKind, BackgroundTaskStatus, ErrorKind, LimitHit, McpServerState,
    ProviderEvent, ProviderKind, Role, policy,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::ConversationId;
use crate::work::{CardId, DiffStat, TaskId};

#[cfg(test)]
#[path = "version_fixture_tests.rs"]
mod version_fixture_tests;

/// A conversation's thread as its CLI keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ThreadView {
    /// Every item, in the order the CLI shows them, each once.
    pub entries: Vec<FileEntry>,
    /// Its turns, oldest first.
    pub turns: Vec<ThreadTurn>,
    /// What its CLI runs in the background now, as the CLI says it, and those that ended
    /// lately: the app shows them by the CLI's own rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<BackgroundTask>>", optional)]
    pub background: Vec<BackgroundTask>,
    /// The MCP servers its CLI has, as the CLI last said.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<McpServerState>>", optional)]
    pub mcp: Vec<McpServerState>,
    /// Its own workers (`brigadier start`), as their CLIs and git show them (S14).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<WorkerRow>>", optional)]
    pub workers: Vec<WorkerRow>,
    /// Its seams (G12): where a fresh context took over, each drawn as a divider.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<SeamMark>>", optional)]
    pub seams: Vec<SeamMark>,
    /// What of its CLI's background work keeps its turn going (D21).
    #[serde(default, skip_serializing_if = "CliJobs::is_empty")]
    #[ts(as = "Option<CliJobs>", optional)]
    pub jobs: CliJobs,
    /// The last [`ThreadNews::seq`] it holds: news up to it is in it, news after it isn't,
    /// whichever reaches the app first.
    #[serde(default, skip_serializing_if = "unnumbered")]
    #[ts(as = "Option<u64>", optional)]
    pub seq: u64,
}

fn unnumbered(seq: &u64) -> bool {
    *seq == 0
}

/// What of a session CLI's background work keeps the turn that started it going (D21): the
/// turn's work stays open and live, and the sidebar says Working, as for its workers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CliJobs {
    /// The background commands its CLI runs now that hold their turn, by task id: not a
    /// subagent, a `brigadier wait` (its workers hold the turn) or a `brigadier preview` call
    /// (a preview has its own row and runs on).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<String>>", optional)]
    pub holding: Vec<String>,
    /// One ended while no turn of its CLI's was open, and the turn its end starts hasn't yet:
    /// until it does, or a grace of a minute passes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub waking: bool,
    /// A subagent its CLI started still runs while no turn of its CLI's is open: it holds the
    /// work as its block's header reads it, so a message sent meanwhile goes on in that block (D113).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub subagents: bool,
}

impl CliJobs {
    pub fn is_empty(&self) -> bool {
        self.holding.is_empty() && !self.waking && !self.subagents
    }
}

/// The first words of the one message Brigadier sends at a seam (G12): write your hand-off.
/// Only a native id previously bound to registered typed input can hide a user-shaped event.
pub(crate) fn daemon_context(
    event: &ProviderEvent,
    known: &std::collections::HashSet<String>,
) -> bool {
    matches!(event, ProviderEvent::Message { role: Role::User, item_id, .. } if known.contains(item_id))
}

pub(crate) const NUDGE_MARK: &str = "[Brigadier] A fresh session will continue this work.";
/// How a successor's hand-off begins (`[Brigadier hand-off <n> · <session>]`) and ends; the
/// first message of the successor follows it.
pub(crate) const HANDOFF_MARK: &str = "[Brigadier hand-off ";
pub(crate) const HANDOFF_END: &str = "[End of the hand-off]";

/// How Brigadier's word to a CLI that a command it left running ended after its turn begins
/// (`conversation.rs`): ``[your background command `<command>` finished after your turn · <how>]``.
pub(crate) const WAKE_MARK: &str = "[your background command `";

/// The job's end an item tells: the CLI's notice of it, or Brigadier's own word ([`WAKE_MARK`]).
pub(crate) fn wake_of(event: &ProviderEvent) -> Option<brigadier_providers::Wake> {
    use brigadier_providers::{Wake, WakeEnd, WakeJob};
    let text = match event {
        ProviderEvent::TaskNotice { wake, .. } => return wake.clone(),
        ProviderEvent::Message {
            role: Role::User,
            text,
            ..
        } => text,
        _ => return None,
    };
    let (command, rest) = text
        .strip_prefix(WAKE_MARK)?
        .split_once("` finished after your turn · ")?;
    let how = rest.split(']').next()?.trim();
    let exit = how.strip_prefix("exit ").and_then(|code| code.parse().ok());
    let end = match (exit, how) {
        (Some(0), _) | (None, "completed") => WakeEnd::Finished,
        (Some(_), _) | (None, "failed") => WakeEnd::Failed,
        _ => WakeEnd::Stopped,
    };
    Some(Wake {
        job: WakeJob::Command,
        label: command.to_owned(),
        command: Some(command.to_owned()),
        end,
        exit,
    })
}

/// A seam of the thread: the divider "Fresh context · hand-off note" (G12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SeamMark {
    /// The generation it ended, from 1.
    pub n: u32,
    /// It is under way: the thread is writing its note, and no divider shows yet.
    pub open: bool,
    /// The successor's first message, which the divider stands before; none until the
    /// successor started (the divider then ends the thread).
    pub at: Option<String>,
    /// The successor started with no message of the user's: its first message is Brigadier's
    /// word to carry on, shown as no bubble of the user's.
    pub carried: bool,
    /// Brigadier's message asking for the hand-off, shown as Brigadier's line, not the user's.
    pub nudge: Option<String>,
    /// The hand-off note, once written.
    pub note: Option<String>,
    /// What the successor started from.
    pub start: Option<String>,
    /// Why the session ended without a note.
    pub death: Option<String>,
    /// The other CLI the session continued on, as asked (S38c).
    pub moved_to: Option<ProviderKind>,
}

/// One of a session's own workers, as its CLI and git show it: what the session's Workers
/// row, its rail and its worker cards read (S14, G19). Nothing of it is stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkerRow {
    pub id: TaskId,
    pub number: u32,
    pub title: String,
    pub agent: ProviderKind,
    pub model: Option<String>,
    pub started_at_ms: i64,
    /// Its CLI's turn is open.
    pub working: bool,
    /// How its last turn ended; none before one did.
    pub last_end: Option<TurnEnd>,
    /// Its CLI ended, or its session closed it.
    pub gone: bool,
    /// When its CLI last said anything.
    pub last_event_at_ms: Option<i64>,
    /// The usage limit its model hit, while it holds.
    pub limit: Option<LimitHit>,
    /// Its CLI compacted its context in the turn that is open.
    pub fresh_context: bool,
    /// What its CLI waits on the user for.
    pub waiting: Vec<WorkerWait>,
    /// Its MCP servers, as its CLI last said.
    pub mcp: Vec<McpServerState>,
    /// Its own branch; none in its session's checkout.
    pub branch: Option<String>,
    /// The session's branch it merges back into (G21).
    pub base: Option<String>,
    /// What it changed against that branch, committed or not.
    pub diff: Option<DiffStat>,
    /// Commits of its branch that branch lacks.
    pub ahead: u32,
    /// It has commits, and that branch has them all.
    pub merged: bool,
    /// What its CLI has at work in its open turn, oldest first; none between its steps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<StepAtWork>>", optional)]
    pub steps: Vec<StepAtWork>,
    /// What its CLI last did in its open turn, kept until it does something else: the line the
    /// rail and the context card show between its calls (D39). None outside a turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub doing: Option<WorkerDoing>,
    /// Its CLI's background tasks, as the CLI says them, and those that ended lately; none once
    /// it ended or was closed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<WorkerTask>>", optional)]
    pub background: Vec<WorkerTask>,
}

/// One of a worker's CLI's background tasks, with the command that started it when its CLI
/// labels it otherwise (Claude's background Bash, by its description).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkerTask {
    pub task: BackgroundTask,
    pub command: Option<String>,
}

/// A call a worker's CLI has at work, as the lead's step would record it: its tool's name and
/// its one-line detail (the path, command, pattern or query). The app says it in the thread's
/// words ("Running pnpm build").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct StepAtWork {
    pub name: String,
    pub detail: Option<String>,
}

/// What a worker's CLI last did in its open turn, as its own events say it (D39).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum WorkerDoing {
    /// A call it started, at work or ended since.
    Call { step: StepAtWork },
    /// It thinks.
    Thinking,
    /// It writes a reply.
    Writing,
    /// The first line of the reply it last wrote.
    Said { line: String },
}

/// Something a worker's CLI waits on the user for: the session relays the answer (G19.Q6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkerWait {
    pub kind: WorkerWaitKind,
    /// What it asks, in a line.
    pub what: String,
    /// Its card, when it has one.
    pub card: Option<CardId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum WorkerWaitKind {
    /// A question its CLI asks the user.
    Question,
    /// One of its MCP servers wants the user to sign in.
    SignIn,
}

/// One turn: a user message up to the CLI's end of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTurn {
    /// The id of the user message that started it.
    pub user_message: String,
    /// When it started and ended, by the CLI's timestamps; no end while it runs.
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
    /// How it ended; none while it runs.
    pub end: Option<TurnEnd>,
    pub error: Option<String>,
    /// What kind of error `error` is, as its CLI's adapter classed it; none for an error of
    /// Brigadier's own (a quit) or one recorded before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error_kind: Option<ErrorKind>,
    /// The CLI that ran it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub provider: Option<ProviderKind>,
    /// Brigadier's requests it carried: its user message's, then those steered into it. Their
    /// cards, workers and changes show in the turn.
    pub requests: Vec<String>,
    /// The usage, spend or credits limit that ended it, as its CLI said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<LimitHit>,
    /// It started with a message the user sent while the work before it still held (its
    /// workers or its background commands still ran): it goes on with that work, in its block.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub joins: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TurnEnd {
    Done,
    /// The user's Stop.
    Stopped,
    Failed,
    /// The CLI ended mid-turn (it was killed, or Brigadier quit under it).
    Interrupted,
}

/// A line of `record/turns.jsonl`: a turn's requests once its user message came, and its end.
/// A turn's lines add up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TurnRecord {
    pub user_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<TurnEnd>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<ErrorKind>,
    /// The CLI that ran it, when its session file isn't the thread's own (a fork's source's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requests: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<LimitHit>,
    /// The model's context window its CLI last said, at the turn's end: Claude's session file
    /// never records it (#540).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
    /// Its user message came while the work before it still held ([`ThreadTurn::joins`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub joins: bool,
}

/// What the live stream added for a conversation, sent as it comes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ThreadNews {
    pub conversation_id: ConversationId,
    pub entries: Vec<FileEntry>,
    /// A turn opened.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub opened: bool,
    /// A turn ended, and how: the turns read again (`threadView`) say the rest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ended: Option<TurnEnd>,
    /// When that turn ended, from the same clock as its turn record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub turn_ended_at_ms: Option<i64>,
    /// How full its model's context is, as its CLI just said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub context: Option<crate::model::ContextUsage>,
    /// Its CLI's background tasks, all of them, when one changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub background: Option<Vec<BackgroundTask>>,
    /// Its CLI's MCP servers, all of them, when one changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub mcp: Option<Vec<McpServerState>>,
    /// Its workers, all of them, when one changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub workers: Option<Vec<WorkerRow>>,
    /// What of its CLI's background work holds its turn, when that changed (D21).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub jobs: Option<CliJobs>,
    /// Its number, rising, given as it changed what a read of the thread holds
    /// ([`ThreadView::seq`]); none (0) for news no read holds the same way (workers' rows).
    #[serde(default, skip_serializing_if = "unnumbered")]
    #[ts(as = "Option<u64>", optional)]
    pub seq: u64,
}

/// Where a conversation's CLI turn stands, as the sidebar reads it: open now, or how its last
/// turn ended. Nothing of it is stored but `record/turns.jsonl`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliTurn {
    #[serde(default)]
    pub open: bool,
    /// How the last turn ended; none when there is none, or it began and never ended (a crash).
    #[serde(default)]
    pub last_end: Option<TurnEnd>,
    #[serde(default)]
    pub turn_ended_at_ms: Option<i64>,
}

#[path = "persistent_apps.rs"]
mod persistent_apps;
pub(crate) use persistent_apps::persistent_app;

/// A conversation's live turn: the stream's items since it began.
#[derive(Debug, Default)]
pub(crate) struct LiveTurn {
    pub entries: Vec<FileEntry>,
    /// Earlier turns' stream items their files may lack (a stopped turn's streamed text,
    /// which Codex never writes): kept past those turns, each with the item before it.
    pub earlier: Vec<FileEntry>,
    /// The user message that started it, once the stream said it.
    pub user_message: Option<String>,
    /// Its CLI's turn started and its user message is still to come (Codex says it after).
    pub awaiting_user: bool,
    /// It goes on with the turn before it: its CLI began it on its own for a background job's
    /// end (its notice), with no user message of its own. That turn reads as open until it
    /// ends, and its end is that turn's (`user_message` is that turn's).
    pub continues: bool,
    pub open: bool,
    pub error: Option<String>,
    pub error_kind: Option<ErrorKind>,
    /// The limit its CLI hit in it.
    pub limit: Option<LimitHit>,
    /// Brigadier's requests it carries.
    pub requests: Vec<String>,
    /// Its user message came while the work before it still held ([`ThreadTurn::joins`]).
    pub joins: bool,
    /// Its CLI began it on its own (no turn of Brigadier's ran): it is the open block's.
    pub own: bool,
    /// The CLI's background tasks: they outlast the turn that started them, so a turn that
    /// opens keeps them.
    pub background: Vec<BackgroundTask>,
    /// A command's output is being followed for its last line.
    pub reading_output: bool,
    /// Its CLI's MCP servers: they outlast the turn too.
    pub mcp: Vec<McpServerState>,
    /// The commands of its CLI's latest calls, by item id, whether or not they began in the
    /// background (Claude moves a running command there): a background task is named by its
    /// call. Those of its background tasks are kept while the tasks are.
    pub commands: Vec<(String, String)>,
    /// Its CLI's subagent calls still running, by item id: they outlast the turn too.
    pub subagents: Vec<String>,
    /// A background command ended while no turn was open: until when the turn its end starts
    /// is owed (`CliJobs::waking`), a turn begun meanwhile included until it says whose it is.
    pub waking_until: Option<i64>,
    /// What of its background work was last said to hold its turn.
    pub jobs_said: CliJobs,
    /// The model's context window its CLI last said: it outlasts the turn.
    pub context_window: Option<i64>,
}

/// How many commands of calls not (yet) a background task's are kept.
const COMMANDS_KEPT: usize = 64;

impl LiveTurn {
    /// Notes the command of a call `event` says, if any: Claude's shell call (its streamed
    /// arguments say it, maybe after its start), a Codex command.
    pub(crate) fn note_command(&mut self, event: &ProviderEvent) {
        let (item, command) = match event {
            ProviderEvent::ToolCall {
                item_id,
                input: Some(input),
                ..
            } => {
                let Some(command) = serde_json::from_str::<serde_json::Value>(input)
                    .ok()
                    .and_then(|args| args.get("command")?.as_str().map(str::to_owned))
                else {
                    return;
                };
                (item_id, command)
            }
            ProviderEvent::Command {
                item_id, command, ..
            } => (item_id, command.clone()),
            _ => return,
        };
        self.commands.retain(|(id, _)| id != item);
        self.commands.push((item.clone(), command));
        let tasks: Vec<&str> = self
            .background
            .iter()
            .filter_map(|task| task.item_id.as_deref())
            .collect();
        let others = self
            .commands
            .iter()
            .filter(|(id, _)| !tasks.contains(&id.as_str()))
            .count();
        if others > COMMANDS_KEPT
            && let Some(at) = self
                .commands
                .iter()
                .position(|(id, _)| !tasks.contains(&id.as_str()))
        {
            self.commands.remove(at);
        }
    }

    /// The command background task `task` runs, when its CLI said it.
    fn command_of<'a>(&'a self, task: &'a BackgroundTask) -> Option<&'a str> {
        task.item_id
            .as_deref()
            .and_then(|item| {
                self.commands
                    .iter()
                    .find(|(id, _)| id == item)
                    .map(|(_, command)| command.as_str())
            })
            .or_else(|| (task.kind == BackgroundTaskKind::Terminal).then_some(task.label.as_str()))
    }

    pub(crate) fn persistent_task(&self, task: &BackgroundTask) -> bool {
        matches!(
            task.kind,
            BackgroundTaskKind::Shell | BackgroundTaskKind::Terminal
        ) && self.command_of(task).is_some_and(persistent_app)
    }

    pub(crate) fn persistent_tasks(&self) -> std::collections::HashSet<String> {
        self.background
            .iter()
            .filter(|task| {
                task.status == BackgroundTaskStatus::Running && self.persistent_task(task)
            })
            .map(|task| task.id.clone())
            .collect()
    }

    /// A subagent its CLI started that still runs after the turn that started it ended, its
    /// CLI listing no task for it (Codex): it holds the turn's work as a background task does,
    /// as the thread draws it (D24, D97), past later turns too.
    pub(crate) fn subagent_running(&self) -> bool {
        !self.open && !self.subagents.is_empty()
    }

    /// Notes a subagent call's start or end ([`Self::subagents`]).
    pub(crate) fn note_subagent(&mut self, event: &ProviderEvent) {
        if let ProviderEvent::ToolCall {
            item_id,
            name,
            status,
            ..
        } = event
            && brigadier_providers::adapters::ADAPTERS.iter().any(|cli| {
                cli.subagent_tools
                    .contains(&crate::digest::bare_tool_name(name))
            })
        {
            self.subagents.retain(|id| id != item_id);
            if *status == brigadier_providers::ItemStatus::InProgress {
                self.subagents.push(item_id.clone());
            }
        }
    }

    /// What of its CLI's background work holds its turn now (`CliJobs`).
    pub(crate) fn jobs(&self, now: i64) -> CliJobs {
        let holding = self
            .background
            .iter()
            .filter(|task| {
                task.status == BackgroundTaskStatus::Running
                    && task.kind != BackgroundTaskKind::Agent
                    && !self.persistent_task(task)
                    && !self.command_of(task).is_some_and(|command| {
                        policy::runs_only_brigadier(command)
                            && (policy::runs_brigadier(command, "wait")
                                || policy::runs_brigadier(command, "preview"))
                    })
            })
            .map(|task| task.id.clone())
            .collect();
        CliJobs {
            holding,
            waking: (!self.open || self.awaiting_user)
                && self.waking_until.is_some_and(|until| now < until),
            subagents: self.subagent_running()
                || !self.open
                    && self.background.iter().any(|task| {
                        task.kind == BackgroundTaskKind::Agent
                            && task.status == BackgroundTaskStatus::Running
                    }),
        }
    }
}

/// A session file's items, with the length and change time of it and its subagents' files
/// they were read at, and its reading, to read only what they add next.
struct CachedFile {
    stamp: FileStamp,
    entries: Arc<Vec<FileEntry>>,
    /// None while a read of it runs.
    reading: Option<session_file::Reading>,
}

type FileStamp = Vec<(PathBuf, u64, std::time::SystemTime)>;

fn file_stamp(kind: ProviderKind, path: &Path) -> Option<FileStamp> {
    let stamp = |path: PathBuf| {
        let meta = std::fs::metadata(&path).ok()?;
        Some((path, meta.len(), meta.modified().ok()?))
    };
    // The session's own file must be there; a subagent's that went or can't be read is left
    // out, not the whole thread.
    let mut stamps = vec![stamp(path.to_owned())?];
    if brigadier_providers::adapters::adapter(kind)
        .is_some_and(|adapter| adapter.subagent_transcripts)
        && let Ok(children) = std::fs::read_dir(path.with_extension("").join("subagents"))
    {
        let mut children: Vec<PathBuf> = children
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|ext| ext == "jsonl" || ext == "json")
            })
            .collect();
        children.sort();
        stamps.extend(children.into_iter().filter_map(stamp));
    }
    Some(stamps)
}

/// Session files read, kept while they don't change.
#[derive(Default)]
pub(crate) struct FileCache(Mutex<HashMap<PathBuf, CachedFile>>);

impl FileCache {
    pub(crate) fn read(&self, kind: ProviderKind, path: &Path) -> Arc<Vec<FileEntry>> {
        let path = &session_file::locate(kind, path);
        let Some(stamp) = file_stamp(kind, path) else {
            return Arc::default();
        };
        let held = match self.0.lock().unwrap().get_mut(path) {
            Some(file) if file.stamp == stamp => {
                return file.entries.clone();
            }
            Some(file) => file.reading.take(),
            None => None,
        };
        let mut reading = held.unwrap_or_else(|| session_file::Reading::new(kind, path));
        let entries = Arc::new(reading.read().unwrap_or_default());
        self.0.lock().unwrap().insert(
            path.to_owned(),
            CachedFile {
                stamp,
                entries: entries.clone(),
                reading: Some(reading),
            },
        );
        entries
    }
}

/// The thread from its CLI session files (`files`, oldest first), what the live stream added
/// and the turn ends recorded.
pub(crate) fn view(
    files: Vec<Arc<Vec<FileEntry>>>,
    live: Option<&LiveTurn>,
    records: &[TurnRecord],
) -> ThreadView {
    let mut entries = Vec::new();
    for file in files {
        entries = session_file::merge(entries, file.as_ref().clone());
    }
    let running = live.is_some_and(|live| live.open);
    if let Some(live) = live {
        entries = add_unheld(entries, &live.earlier);
        if running {
            entries = session_file::merge(entries, live.entries.clone());
        } else {
            entries = add_unheld(entries, &live.entries);
        }
    }
    for entry in entries.iter_mut().filter(|entry| entry.parent.is_none()) {
        entry.wake = wake_of(&entry.event);
    }
    let awaiting = live.is_some_and(|live| live.awaiting_user);
    let mut turns = turns(&entries, records, running, awaiting);
    if running
        && let Some(live) = live
        && let Some(turn) = turns.last_mut()
    {
        // The turn it goes on with ended before: it runs again, until this one ends.
        if live.continues && live.user_message.as_deref() == Some(turn.user_message.as_str()) {
            turn.end = None;
            turn.ended_at_ms = None;
            turn.error = None;
            turn.error_kind = None;
        }
        if turn.error.is_none() {
            turn.error.clone_from(&live.error);
            turn.error_kind = live.error_kind;
        }
        if turn.limit.is_none() {
            turn.limit.clone_from(&live.limit);
        }
        add_requests(&mut turn.requests, &live.requests);
        // Still awaiting its own message, the live turn is the one read after the turn before.
        if !live.awaiting_user || live.user_message.as_deref() != Some(turn.user_message.as_str()) {
            turn.joins |= live.joins;
        }
        // A turn its file opened that the live turn, the CLI's own, hasn't said yet: a message
        // steered into that turn, which is the open block's (D25, D74).
        if (live.continues || live.own)
            && live
                .user_message
                .as_deref()
                .is_some_and(|own| own != turn.user_message)
        {
            turn.joins = true;
        }
    }
    ThreadView {
        entries,
        turns,
        background: live.map(|live| live.background.clone()).unwrap_or_default(),
        mcp: live.map(|live| live.mcp.clone()).unwrap_or_default(),
        workers: Vec::new(),
        seams: Vec::new(),
        jobs: live
            .map(|live| live.jobs(crate::now_ms()))
            .unwrap_or_default(),
        seq: 0,
    }
}

/// Once the turn ended its file holds it: what the stream said adds only items the file
/// still lacks (by id), so the thread reads as it will after a restart. What has no id of its
/// own (an error) would show twice. Each goes after the item the stream had before it, not
/// after the turn's end the file holds (#560).
fn add_unheld(mut entries: Vec<FileEntry>, live: &[FileEntry]) -> Vec<FileEntry> {
    let mut held: std::collections::HashSet<String> = entries
        .iter()
        .filter_map(|entry| session_file::item_id(&entry.event).map(str::to_owned))
        .collect();
    let mut before: Option<String> = None;
    for entry in live {
        let Some(id) = session_file::item_id(&entry.event).map(str::to_owned) else {
            continue;
        };
        if held.insert(id.clone()) {
            let at = before
                .as_deref()
                .and_then(|before| {
                    entries
                        .iter()
                        .position(|held| session_file::item_id(&held.event) == Some(before))
                })
                .map_or(entries.len(), |index| index + 1);
            entries.insert(at, entry.clone());
        }
        before = Some(id);
    }
    entries
}

/// Drops from `earlier` what `files` now hold, but the item before each they lack: it says
/// where that one goes ([`add_unheld`]).
pub(crate) fn keep_unheld(earlier: &mut Vec<FileEntry>, files: &[Arc<Vec<FileEntry>>]) {
    let held: std::collections::HashSet<&str> = files
        .iter()
        .flat_map(|file| file.iter())
        .filter_map(|entry| session_file::item_id(&entry.event))
        .collect();
    earlier.retain(|entry| session_file::item_id(&entry.event).is_some());
    let lacked: Vec<bool> = earlier
        .iter()
        .map(|entry| session_file::item_id(&entry.event).is_some_and(|id| !held.contains(id)))
        .collect();
    let mut index = 0;
    earlier.retain(|_| {
        let keep = lacked[index] || lacked.get(index + 1) == Some(&true);
        index += 1;
        keep
    });
}

/// Where each turn of `entries` starts: at each user message that isn't a steer.
fn turn_starts(entries: &[FileEntry]) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            entry.parent.is_none() && !entry.steer && user_message(entry).is_some()
        })
        .map(|(index, _)| index)
        .collect()
}

/// The turns `entries` hold: each user message that isn't a steer starts one.
fn turns(
    entries: &[FileEntry],
    records: &[TurnRecord],
    running: bool,
    awaiting: bool,
) -> Vec<ThreadTurn> {
    let starts = turn_starts(entries);
    // A turn's lines, added up: its requests, and its end. A turn with lines but no end never
    // ended: the CLI or Brigadier went under it (#557).
    let mut recorded: HashMap<&str, (Vec<String>, Option<&TurnRecord>, bool)> = HashMap::new();
    // Each turn's ends, oldest first: a turn the stream said went on (a background job's end,
    // or a compaction the daemon took for one) ends more than once.
    let mut ends: HashMap<&str, Vec<&TurnRecord>> = HashMap::new();
    for record in records {
        let Some(id) = record.user_message.as_deref() else {
            continue;
        };
        let (requests, end, joins) = recorded.entry(id).or_default();
        add_requests(requests, &record.requests);
        *joins |= record.joins;
        if record.end.is_some() {
            *end = Some(record);
            ends.entry(id).or_default().push(record);
        }
    }
    let count = starts.len();
    // The end the turn before was recorded with, as the daemon followed it.
    let mut before: Option<&TurnRecord> = None;
    let mut turns: Vec<(ThreadTurn, bool)> = starts
        .iter()
        .enumerate()
        .map(|(n, &start)| {
            let end = starts.get(n + 1).copied().unwrap_or(entries.len());
            let inside = &entries[start..end];
            let id = user_message(&entries[start]).unwrap_or_default().to_owned();
            let last = n + 1 == count;
            let mut turn = ThreadTurn {
                user_message: id.clone(),
                started_at_ms: entries[start].at_ms,
                ended_at_ms: inside.iter().rev().find_map(|entry| entry.at_ms),
                end: None,
                error: None,
                error_kind: None,
                provider: None,
                requests: Vec::new(),
                limit: None,
                joins: false,
            };
            let marker = inside.iter().find_map(|entry| match &entry.event {
                ProviderEvent::TurnCompleted { status, .. } if entry.parent.is_none() => {
                    Some(match status {
                        brigadier_providers::TurnStatus::Completed => TurnEnd::Done,
                        brigadier_providers::TurnStatus::Interrupted => TurnEnd::Stopped,
                        brigadier_providers::TurnStatus::Failed => TurnEnd::Failed,
                    })
                }
                _ => None,
            });
            let noted = recorded.remove(id.as_str());
            // A turn no record names (or names no end of) that began before the turn before it
            // ended ran inside that one, as the stream said it: Claude writes Brigadier's
            // `/compact` after the compaction, which the stream said in the turn before, and a
            // message steered into the CLI's own turn as a turn's opening one. It ended as that
            // one did.
            let within = before.filter(|record| {
                noted.as_ref().is_none_or(|(_, end, _)| end.is_none())
                    && record
                        .ended_at_ms
                        .zip(turn.started_at_ms)
                        .is_some_and(|(ended, started)| started <= ended)
            });
            let cut = noted.as_ref().is_some_and(|(_, end, _)| end.is_none());
            let (requests, record, joins) = noted.unwrap_or_default();
            // Internal inputs and background notifications continue the same visible turn.
            // A prior attempt's error stays in the native file after an account switch;
            // only errors since the preceding end can describe this continuation.
            // A turn its record ended isn't the one running while the live turn awaits its own
            // message: that turn comes after it (a limit's carry-on keeps the cut turn's error).
            let running =
                last && running && !(awaiting && record.is_some_and(|record| record.end.is_some()));
            let completed = record.is_some_and(|record| record.end == Some(TurnEnd::Done));
            let since = ends.get(id.as_str()).and_then(|ends| {
                ends.iter()
                    .rev()
                    .find(|prior| {
                        running || completed || record.is_none_or(|end| !std::ptr::eq(**prior, end))
                    })
                    .and_then(|prior| prior.ended_at_ms)
            });
            let current_errors = || {
                inside
                    .iter()
                    .filter(|entry| {
                        entry.parent.is_none()
                            && entry.at_ms.zip(since).is_none_or(|(at, since)| at > since)
                    })
                    .filter_map(|entry| match &entry.event {
                        ProviderEvent::Error { error } => Some(error),
                        _ => None,
                    })
            };
            let first = current_errors().next();
            let error = first.map(|error| error.message.clone());
            let limit = current_errors().rev().find_map(limit_of);
            if within.is_none() {
                before = record;
            }
            turn.requests = requests;
            turn.joins = joins;
            turn.provider = record.and_then(|record| record.provider);
            turn.limit = record
                .filter(|_| !running)
                .and_then(|record| record.limit.clone())
                .or(limit);
            if let Some(record) = record {
                turn.end = record.end;
                turn.error = record.error.clone().filter(|_| !running).or(error);
                turn.ended_at_ms = record.ended_at_ms;
            } else if last && running {
                turn.ended_at_ms = None;
            } else if let Some(record) = within {
                turn.end = record.end;
                turn.error = error;
                turn.ended_at_ms = record.ended_at_ms;
            } else if cut {
                turn.end = Some(TurnEnd::Interrupted);
                turn.error = error;
            } else if let Some(marker) = marker {
                turn.end = Some(marker);
                turn.error = error;
            } else if error.is_some() {
                turn.end = Some(TurnEnd::Failed);
                turn.error = error;
            } else if last {
                // The CLI is gone and never said the turn ended.
                turn.end = Some(TurnEnd::Interrupted);
            } else {
                turn.end = Some(TurnEnd::Done);
            }
            // The error's kind: the CLI's error's, or its record's when the record said it.
            turn.error_kind = if turn.error.is_some()
                && turn.error.as_deref() == first.map(|error| error.message.as_str())
            {
                first.map(|error| error.kind)
            } else {
                record
                    .filter(|record| record.error == turn.error)
                    .and_then(|record| record.error_kind)
            };
            (turn, within.is_some())
        })
        .collect();
    // The turn before one that ran inside it ended where it ended before that one began, not
    // with it: Brigadier's `/compact` after an answer adds nothing to the answer's time.
    for n in 1..turns.len() {
        if !turns[n].1 || turns[n - 1].1 {
            continue;
        }
        let started = turns[n].0.started_at_ms;
        let previous = &mut turns[n - 1].0;
        let own = ends.get(previous.user_message.as_str()).and_then(|ends| {
            ends.iter().rev().find(|record| {
                record
                    .ended_at_ms
                    .zip(started)
                    .is_some_and(|(ended, started)| ended <= started)
            })
        });
        if let Some(record) = own {
            previous.end = record.end;
            previous.ended_at_ms = record.ended_at_ms;
            if record.error.is_some() {
                previous.error.clone_from(&record.error);
            }
        }
    }
    turns.into_iter().map(|(turn, _)| turn).collect()
}

/// The limit an error of the CLI says it hit: a usage window, a spend control or credits.
/// Only `view`'s newest `turns` turns, from the first one's user message on: what a thread shows
/// at once when it opens, its older turns coming after with the whole view (D46). A seam whose
/// message is cut goes with it; a view whose first kept turn's message isn't found stays whole.
pub fn newest_turns(view: &mut ThreadView, turns: usize) {
    let Some(first) = view.turns.len().checked_sub(turns).filter(|&cut| cut > 0) else {
        return;
    };
    let message = view.turns[first].user_message.as_str();
    let Some(at) = view.entries.iter().position(|entry| {
        entry.parent.is_none()
            && matches!(&entry.event, ProviderEvent::Message { item_id, role: Role::User, .. } if item_id == message)
    }) else {
        return;
    };
    view.entries.drain(..at);
    view.turns.drain(..first);
    let kept: std::collections::HashSet<&str> = view
        .entries
        .iter()
        .filter_map(|entry| session_file::item_id(&entry.event))
        .collect();
    view.seams
        .retain(|seam| seam.at.as_deref().is_none_or(|at| kept.contains(at)));
}

pub(crate) fn limit_of(error: &brigadier_providers::ProviderError) -> Option<LimitHit> {
    match (&error.limit, error.kind) {
        (Some(limit), _) => Some(limit.clone()),
        (None, ErrorKind::UsageLimit) => Some(LimitHit {
            window: None,
            resets_at_ms: None,
            kind: Default::default(),
        }),
        _ => None,
    }
}

/// Adds `more` to `requests`, each once, in order.
pub(crate) fn add_requests(requests: &mut Vec<String>, more: &[String]) {
    for request in more {
        if !requests.contains(request) {
            requests.push(request.clone());
        }
    }
}

/// The id of a user message.
/// Names each turn's CLI: the one whose session file (`files`) holds its user message, else its
/// record's (a fork's source's), else `current` (the live stream's turn).
pub(crate) fn name_providers(
    view: &mut ThreadView,
    files: &[(ProviderKind, Arc<Vec<FileEntry>>)],
    current: Option<ProviderKind>,
) {
    let mut by_message: HashMap<&str, ProviderKind> = HashMap::new();
    for (kind, entries) in files {
        for id in entries.iter().filter_map(user_message) {
            by_message.insert(id, *kind);
        }
    }
    for turn in &mut view.turns {
        turn.provider = by_message
            .get(turn.user_message.as_str())
            .copied()
            .or(turn.provider)
            .or(current);
    }
}

fn user_message(entry: &FileEntry) -> Option<&str> {
    match &entry.event {
        ProviderEvent::Message {
            item_id,
            role: Role::User,
            ..
        } => Some(item_id),
        _ => None,
    }
}

/// A fork's thread before its own CLI's (#648), in its `record/forked.json`: its source's, as
/// the source's CLI kept it, up to the answer it forks. Its turns are records that end each as
/// it ended in the source.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Forked {
    pub entries: Vec<FileEntry>,
    pub turns: Vec<TurnRecord>,
}

/// `view` up to the end of its last turn that carried `request`; none when no turn did.
pub(crate) fn forked_at(view: &ThreadView, request: &str) -> Option<Forked> {
    let last = view
        .turns
        .iter()
        .rposition(|turn| turn.requests.iter().any(|id| id == request))?;
    let end = turn_starts(&view.entries)
        .get(last + 1)
        .copied()
        .unwrap_or(view.entries.len());
    Some(Forked {
        entries: view.entries[..end].to_vec(),
        turns: view.turns[..=last]
            .iter()
            .map(|turn| TurnRecord {
                user_message: Some(turn.user_message.clone()),
                end: turn.end,
                error: turn.error.clone(),
                error_kind: turn.error_kind,
                provider: turn.provider,
                ended_at_ms: turn.ended_at_ms,
                requests: turn.requests.clone(),
                limit: turn.limit.clone(),
                context_window: None,
                joins: turn.joins,
            })
            .collect(),
    })
}

/// The fork's thread before its own CLI's, from `record/forked.json`; none when it is no fork.
pub(crate) fn read_forked(record: &Path) -> Forked {
    std::fs::read(record.join("forked.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// The user message of the last turn `record/turns.jsonl` names, if any.
pub(crate) fn last_user_message(record: &Path) -> Option<String> {
    read_turns(record)
        .into_iter()
        .rev()
        .find_map(|turn| turn.user_message)
}

/// `record/turns.jsonl`'s turns; none when it is missing.
pub(crate) fn read_turns(record: &Path) -> Vec<TurnRecord> {
    std::fs::read_to_string(record.join("turns.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// The last line of `record/turns.jsonl`: its turn's start or end and recorded timestamp.
pub(crate) fn last_turn(record: &Path) -> Option<TurnRecord> {
    let turns = std::fs::read_to_string(record.join("turns.jsonl")).ok()?;
    let last = turns.lines().rev().find(|line| !line.trim().is_empty())?;
    serde_json::from_str::<TurnRecord>(last).ok()
}

/// Adds a turn's end to `record/turns.jsonl`.
pub(crate) fn append_turn(record: &Path, turn: &TurnRecord) -> std::io::Result<()> {
    use std::io::Write as _;
    std::fs::create_dir_all(record)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(record.join("turns.jsonl"))?;
    let line = serde_json::to_string(turn).map_err(std::io::Error::other)?;
    writeln!(file, "{line}")
}

#[cfg(test)]
mod tests {
    use brigadier_providers::{ItemStatus, TurnStatus};

    use super::*;

    /// A background subagent holds its work once its turn ended, as a background command does:
    /// a task the CLI lists (Claude) or a call still running (Codex), every CLI alike (demo8
    /// item 2: what was sent meanwhile drew a second block).
    #[test]
    fn a_background_subagent_holds_its_work() {
        let agent = BackgroundTask {
            command: None,
            provider: None,
            id: "agent".into(),
            item_id: Some("call".into()),
            kind: BackgroundTaskKind::Agent,
            label: "background calculation".into(),
            status: BackgroundTaskStatus::Running,
            started_at_ms: 1,
            ended_at_ms: None,
            output_file: None,
            summary: None,
            last_line: None,
        };
        let mut listed = LiveTurn {
            background: vec![agent.clone()],
            ..Default::default()
        };
        assert!(listed.jobs(2).subagents && !listed.jobs(2).is_empty());
        assert!(
            listed.jobs(2).holding.is_empty(),
            "a subagent is no command"
        );
        listed.open = true;
        assert!(listed.jobs(2).is_empty(), "its open turn holds it");
        listed.open = false;
        listed.background[0].status = BackgroundTaskStatus::Completed;
        assert!(listed.jobs(2).is_empty());
        let called = LiveTurn {
            subagents: vec!["call".into()],
            ..Default::default()
        };
        assert!(called.jobs(2).subagents);
    }

    /// Brigadier's own word that a command ended after its turn says the job's end, as a CLI's
    /// notice does; a message of the user's says none.
    #[test]
    fn a_view_cut_to_its_newest_turns_starts_at_the_first_kept_turns_message() {
        let turn = |id: &str| ThreadTurn {
            user_message: id.into(),
            started_at_ms: None,
            ended_at_ms: None,
            end: Some(TurnEnd::Done),
            error: None,
            error_kind: None,
            provider: None,
            requests: Vec::new(),
            limit: None,
            joins: false,
        };
        let seam = |at: &str| SeamMark {
            n: 1,
            open: false,
            at: Some(at.into()),
            carried: false,
            nudge: None,
            note: None,
            start: None,
            death: None,
            moved_to: None,
        };
        let whole = ThreadView {
            entries: vec![
                message("u1", Role::User),
                message("a1", Role::Assistant),
                message("u2", Role::User),
                message("a2", Role::Assistant),
                message("u3", Role::User),
                message("a3", Role::Assistant),
            ],
            turns: vec![turn("u1"), turn("u2"), turn("u3")],
            background: vec![],
            mcp: vec![],
            workers: vec![],
            seams: vec![seam("u2"), seam("u3")],
            jobs: CliJobs::default(),
            seq: 7,
        };
        let ids = |view: &ThreadView| -> Vec<String> {
            view.entries
                .iter()
                .filter_map(|entry| session_file::item_id(&entry.event).map(str::to_owned))
                .collect()
        };
        let mut cut = whole.clone();
        newest_turns(&mut cut, 2);
        assert_eq!(ids(&cut), ["u2", "a2", "u3", "a3"]);
        assert_eq!(cut.turns, vec![turn("u2"), turn("u3")]);
        assert_eq!(cut.seams.len(), 2);
        assert_eq!(cut.seq, 7);
        let mut one = whole.clone();
        newest_turns(&mut one, 1);
        assert_eq!(ids(&one), ["u3", "a3"]);
        assert_eq!(one.seams, vec![seam("u3")]);
        // As many turns as it has or more, or a first kept message not found: whole.
        let mut all = whole.clone();
        newest_turns(&mut all, 3);
        assert_eq!(all, whole);
        let mut lost = whole.clone();
        lost.turns[1].user_message = "gone".into();
        newest_turns(&mut lost, 2);
        assert_eq!(lost.entries.len(), 6);
    }

    #[test]
    fn brigadiers_word_of_a_commands_end_is_its_wake() {
        use brigadier_providers::{WakeEnd, WakeJob};
        let said = |text: &str| {
            wake_of(&ProviderEvent::Message {
                item_id: "m".into(),
                role: Role::User,
                text: text.into(),
            })
            .map(|wake| (wake.job, wake.command, wake.end, wake.exit))
        };
        let words =
            |how: &str| format!("{WAKE_MARK}pnpm test` finished after your turn · {how}]\nFAIL");
        assert_eq!(
            said(&words("exit 1")),
            Some((
                WakeJob::Command,
                Some("pnpm test".into()),
                WakeEnd::Failed,
                Some(1)
            ))
        );
        assert_eq!(
            said(&words("completed")),
            Some((
                WakeJob::Command,
                Some("pnpm test".into()),
                WakeEnd::Finished,
                None
            ))
        );
        assert_eq!(said("Run pnpm test."), None);
    }

    fn entry(event: ProviderEvent) -> FileEntry {
        FileEntry {
            wake: None,
            event,
            at_ms: Some(1),
            updated_at_ms: None,
            parent: None,
            steer: false,
            images: Vec::new(),
            phase: None,
            said: None,
            requests: Vec::new(),
        }
    }

    #[test]
    fn daemon_provenance_requires_known_native_id_and_respects_authored_markers() {
        let entry = message("native", Role::User);
        assert!(!daemon_context(&entry.event, &Default::default()));
        assert!(daemon_context(
            &entry.event,
            &std::collections::HashSet::from(["native".into()])
        ));
    }

    #[test]
    fn reconstructed_projection_keeps_assistant_followup_without_an_internal_workblock() {
        let raw = [
            message("real-user", Role::User),
            message("answer-before", Role::Assistant),
            message("daemon-input", Role::User),
            message("answer-after", Role::Assistant),
        ];
        let known = std::collections::HashSet::from(["daemon-input".to_owned()]);
        let projected: Vec<_> = raw
            .iter()
            .filter(|entry| !daemon_context(&entry.event, &known))
            .cloned()
            .collect();
        let reconstructed = view(vec![Arc::new(projected)], None, &[]);
        assert_eq!(reconstructed.turns.len(), 1);
        assert_eq!(
            ids(&reconstructed),
            ["real-user", "answer-before", "answer-after"]
        );
        assert_eq!(raw.len(), 4, "native diagnostics/history remain untouched");
        assert!(!daemon_context(
            &message("authored-identical", Role::User).event,
            &known
        ));
    }

    /// A Claude session's thread still reads when one of its subagents' files can't be (#619 C4).
    #[cfg(unix)]
    #[test]
    fn a_subagent_file_that_cannot_be_read_leaves_the_thread_whole() {
        let dir = std::env::temp_dir().join(format!("bg-thread-stamp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("s1/subagents")).unwrap();
        let file = dir.join("s1.jsonl");
        std::fs::write(&file, "").unwrap();
        std::fs::write(dir.join("s1/subagents/agent-a.jsonl"), "").unwrap();
        // Listed by the folder, gone by the time it is looked at.
        std::os::unix::fs::symlink(dir.join("nowhere"), dir.join("s1/subagents/agent-b.jsonl"))
            .unwrap();
        let stamp = file_stamp(ProviderKind::Claude, &file).expect("the thread's own file reads");
        let names: Vec<_> = stamp
            .iter()
            .map(|(path, ..)| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["s1.jsonl", "agent-a.jsonl"]);
        assert!(file_stamp(ProviderKind::Claude, &dir.join("gone.jsonl")).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn message(id: &str, role: Role) -> FileEntry {
        entry(ProviderEvent::Message {
            item_id: id.into(),
            role,
            text: id.into(),
        })
    }

    #[test]
    fn a_recovered_account_limit_does_not_describe_a_later_continuation() {
        let at = |mut entry: FileEntry, at| {
            entry.at_ms = Some(at);
            entry
        };
        let limited = at(
            entry(ProviderEvent::Error {
                error: brigadier_providers::ProviderError {
                    kind: ErrorKind::UsageLimit,
                    message: "You've hit your session limit".into(),
                    will_retry: false,
                    limit: Some(LimitHit {
                        window: Some("five_hour".into()),
                        resets_at_ms: Some(1793100600000),
                        kind: Default::default(),
                    }),
                    code: Some("rate_limit".into()),
                },
            }),
            1793090676772,
        );
        // Background notifications and internal account-resume messages join the same
        // visible user turn. Work's error remains in the shared native session file.
        let entries = Arc::new(vec![
            at(message("user", Role::User), 1793088360000),
            limited,
            at(
                message("resumed-on-personal-2", Role::Assistant),
                1793090870000,
            ),
            at(message("working", Role::Assistant), 1793092796552),
        ]);
        let record = |end, at| TurnRecord {
            user_message: Some("user".into()),
            end: Some(end),
            ended_at_ms: Some(at),
            error: None,
            error_kind: None,
            provider: None,
            limit: None,
            requests: Vec::new(),
            context_window: None,
            joins: false,
        };
        let recovered = record(TurnEnd::Done, 1793090876447);
        let recovered_view = view(
            vec![entries.clone()],
            None,
            std::slice::from_ref(&recovered),
        );
        assert_eq!(recovered_view.turns[0].error, None);
        assert_eq!(recovered_view.turns[0].limit, None);
        for end in [TurnEnd::Done, TurnEnd::Interrupted] {
            let view = view(
                vec![entries.clone()],
                None,
                &[recovered.clone(), record(end, 1793092845464)],
            );
            assert_eq!(view.turns[0].end, Some(end));
            assert_eq!(view.turns[0].error, None, "{view:#?}");
            assert_eq!(view.turns[0].limit, None, "{view:#?}");
        }
        let live = LiveTurn {
            open: true,
            continues: true,
            user_message: Some("user".into()),
            ..Default::default()
        };
        let running = view(
            vec![entries.clone()],
            Some(&live),
            std::slice::from_ref(&recovered),
        );
        assert_eq!(running.turns[0].end, None);
        assert_eq!(running.turns[0].error, None);
        assert_eq!(running.turns[0].limit, None, "{running:#?}");
        let mut fresh = entries.as_ref().clone();
        fresh.push(at(
            entry(ProviderEvent::Error {
                error: brigadier_providers::ProviderError {
                    kind: ErrorKind::Network,
                    message: "Connection lost.".into(),
                    will_retry: false,
                    limit: None,
                    code: None,
                },
            }),
            1793092800000,
        ));
        let failed = view(
            vec![Arc::new(fresh)],
            None,
            &[recovered, record(TurnEnd::Interrupted, 1793092845464)],
        );
        assert_eq!(failed.turns[0].error.as_deref(), Some("Connection lost."));
        assert_eq!(failed.turns[0].limit, None);
    }

    /// The CLI's next turn started and hasn't said its message yet (a limit's carry-on on the
    /// account or CLI switched to): the cut turn still reads as the limit ended it, never as
    /// failed with no word of why (demo8 item 4, seen on Codex for a second).
    #[test]
    fn a_cut_turn_keeps_its_limit_while_the_next_awaits_its_message() {
        let limit = LimitHit {
            window: Some("primary".into()),
            resets_at_ms: Some(1793100600000),
            kind: Default::default(),
        };
        let entries = Arc::new(vec![message("user", Role::User)]);
        let cut = TurnRecord {
            user_message: Some("user".into()),
            end: Some(TurnEnd::Failed),
            ended_at_ms: Some(2),
            error: Some("Codex's primary usage limit was reached".into()),
            error_kind: Some(ErrorKind::UsageLimit),
            provider: None,
            limit: Some(limit.clone()),
            requests: vec!["r1".into()],
            context_window: None,
            joins: false,
        };
        let live = LiveTurn {
            open: true,
            awaiting_user: true,
            user_message: Some("user".into()),
            ..Default::default()
        };
        let view = view(vec![entries], Some(&live), std::slice::from_ref(&cut));
        assert_eq!(view.turns.len(), 1);
        assert_eq!(view.turns[0].end, Some(TurnEnd::Failed));
        assert_eq!(
            view.turns[0].error_kind,
            Some(ErrorKind::UsageLimit),
            "{view:#?}"
        );
        assert_eq!(view.turns[0].limit, Some(limit));
    }

    fn command(id: &str, status: ItemStatus) -> FileEntry {
        entry(ProviderEvent::Command {
            item_id: id.into(),
            command: "curl -sI x".into(),
            cwd: None,
            status,
            exit_code: None,
            output: None,
            duration_ms: None,
            kept: None,
        })
    }

    fn ids(view: &ThreadView) -> Vec<String> {
        view.entries
            .iter()
            .map(|entry| match &entry.event {
                ProviderEvent::TurnCompleted { .. } => "end".to_owned(),
                event => session_file::item_id(event).unwrap_or("?").to_owned(),
            })
            .collect()
    }

    /// A command only the stream said (Codex keeps none of a declined or killed one) reads
    /// where the stream had it once the turn ended, not after the turn's end (#560).
    #[test]
    fn an_item_only_the_stream_said_reads_in_its_place_after_the_turn() {
        let end = entry(ProviderEvent::TurnCompleted {
            turn_id: None,
            status: TurnStatus::Completed,
            duration_ms: None,
            usage: None,
        });
        let file = vec![
            message("u1", Role::User),
            message("a1", Role::Assistant),
            message("a2", Role::Assistant),
            end,
        ];
        let live = LiveTurn {
            entries: vec![
                message("u1", Role::User),
                message("a1", Role::Assistant),
                command("c1", ItemStatus::Declined),
                command("c2", ItemStatus::InProgress),
                message("a2", Role::Assistant),
            ],
            ..LiveTurn::default()
        };
        let view = view(vec![Arc::new(file)], Some(&live), &[]);
        assert_eq!(ids(&view), ["u1", "a1", "c1", "c2", "a2", "end"]);
        assert_eq!(view.turns.len(), 1);
    }

    /// A stopped turn's streamed answer, which Codex never writes to its file, still reads in
    /// that turn while later turns run and after they end; what the file holds isn't kept.
    #[test]
    fn a_stopped_turns_streamed_text_stays_after_later_turns() {
        let end = || {
            entry(ProviderEvent::TurnCompleted {
                turn_id: None,
                status: TurnStatus::Interrupted,
                duration_ms: None,
                usage: None,
            })
        };
        let file = Arc::new(vec![
            message("u0", Role::User),
            message("a0", Role::Assistant),
            end(),
            message("u1", Role::User),
            end(),
            message("u2", Role::User),
        ]);
        let mut live = LiveTurn {
            earlier: vec![
                message("u0", Role::User),
                message("a0", Role::Assistant),
                message("u1", Role::User),
                message("a1", Role::Assistant),
            ],
            entries: vec![message("u2", Role::User), message("a2", Role::Assistant)],
            open: true,
            ..LiveTurn::default()
        };
        keep_unheld(&mut live.earlier, std::slice::from_ref(&file));
        assert_eq!(live.earlier.len(), 2, "only a1 and the item before it");
        let running = view(vec![file.clone()], Some(&live), &[]);
        assert_eq!(
            ids(&running),
            ["u0", "a0", "end", "u1", "a1", "end", "u2", "a2"]
        );
        live.open = false;
        let ended = view(vec![file], Some(&live), &[]);
        assert_eq!(ids(&ended), ids(&running));
        assert_eq!(ended.turns.len(), 3);
    }

    /// A turn the CLI began on its own for a background job's end goes on with the turn before
    /// it: that turn reads as open while it runs, and ends when it does.
    #[test]
    fn a_turn_the_cli_began_for_a_background_jobs_end_keeps_the_turn_before_it_open() {
        let notice = entry(ProviderEvent::TaskNotice {
            wake: None,
            item_id: "n1".into(),
            text: r#"Background command "brigadier wait" completed (exit code 0)"#.into(),
        });
        let file = vec![message("u1", Role::User), message("a1", Role::Assistant)];
        let ended = |at: i64| TurnRecord {
            user_message: Some("u1".into()),
            end: Some(TurnEnd::Done),
            error: None,
            error_kind: None,
            provider: None,
            ended_at_ms: Some(at),
            requests: Vec::new(),
            limit: None,
            context_window: None,
            joins: false,
        };
        let live = LiveTurn {
            entries: vec![notice.clone(), message("a2", Role::Assistant)],
            user_message: Some("u1".into()),
            continues: true,
            open: true,
            ..LiveTurn::default()
        };
        let running = view(vec![Arc::new(file.clone())], Some(&live), &[ended(10)]);
        assert_eq!(ids(&running), ["u1", "a1", "n1", "a2"]);
        assert_eq!(running.turns.len(), 1);
        assert_eq!(running.turns[0].end, None);
        assert_eq!(running.turns[0].ended_at_ms, None);
        // Ended: its end is the turn's, the later of the two.
        let mut all = file;
        all.extend([notice, message("a2", Role::Assistant)]);
        let done = view(vec![Arc::new(all)], None, &[ended(10), ended(30)]);
        assert_eq!(done.turns.len(), 1);
        assert_eq!(done.turns[0].end, Some(TurnEnd::Done));
        assert_eq!(done.turns[0].ended_at_ms, Some(30));
        // A turn of a new user message still to come never reopens the one before it.
        let starting = LiveTurn {
            user_message: Some("u1".into()),
            awaiting_user: true,
            open: true,
            ..LiveTurn::default()
        };
        let next = view(
            vec![Arc::new(vec![message("u1", Role::User)])],
            Some(&starting),
            &[ended(10)],
        );
        assert_eq!(next.turns[0].end, Some(TurnEnd::Done));
        // Sent while the work before held, it joins that work from its start (goal 8): the turn
        // the file reads before the stream gives its message, never the one before it.
        let joining = LiveTurn {
            joins: true,
            ..starting
        };
        let before = view(
            vec![Arc::new(vec![message("u1", Role::User)])],
            Some(&joining),
            &[ended(10)],
        );
        assert!(!before.turns[0].joins);
        let read = view(
            vec![Arc::new(vec![
                message("u1", Role::User),
                message("u2", Role::User),
            ])],
            Some(&joining),
            &[ended(10)],
        );
        assert!(!read.turns[0].joins);
        assert!(read.turns[1].joins);
    }

    /// Brigadier's `/compact` after a turn: the stream said the compaction in that turn, so its
    /// record names that turn alone; Claude's file then holds the `/compact` as a turn of its
    /// own. It ended with that turn, and never reads as cut off. A turn begun after that one's
    /// end that nothing recorded still does.
    #[test]
    fn a_turn_the_records_never_named_inside_the_turn_before_ends_as_that_one_did() {
        let at = |at: i64, entry: FileEntry| FileEntry {
            at_ms: Some(at),
            ..entry
        };
        let ended = |at: i64| TurnRecord {
            user_message: Some("u1".into()),
            end: Some(TurnEnd::Done),
            error: None,
            error_kind: None,
            provider: None,
            ended_at_ms: Some(at),
            requests: vec!["r1".into()],
            limit: None,
            context_window: None,
            joins: false,
        };
        let compaction = entry(ProviderEvent::CompactionEnded {
            automatic: false,
            tokens_before: Some(200),
            tokens_after: Some(20),
            error: None,
            native_id: None,
        });
        let file = vec![
            at(1, message("u1", Role::User)),
            at(2, message("a1", Role::Assistant)),
            at(9, compaction),
            at(3, message("/compact", Role::User)),
        ];
        let records = [ended(2), ended(10)];
        let view = view(vec![Arc::new(file.clone())], None, &records);
        assert_eq!(view.turns.len(), 2);
        // The turn before ended where it did, before the `/compact` began: not with it.
        assert_eq!(view.turns[0].end, Some(TurnEnd::Done));
        assert_eq!(view.turns[0].ended_at_ms, Some(2));
        assert_eq!(view.turns[1].end, Some(TurnEnd::Done));
        assert_eq!(view.turns[1].error, None);
        assert_eq!(view.turns[1].ended_at_ms, Some(10));
        assert!(view.turns[1].requests.is_empty());
        let mut later = file;
        later.push(at(20, message("u3", Role::User)));
        let cut = super::view(vec![Arc::new(later)], None, &records);
        assert_eq!(cut.turns[2].end, Some(TurnEnd::Interrupted));
    }

    /// O38: while the CLI's own turn runs on with the turn before, its file opens a turn at a
    /// message steered into it before the stream says that message. That turn is the open
    /// block's from the first frame: it joins, never reads as a block of its own.
    #[test]
    fn a_turn_the_file_opens_inside_the_clis_own_running_turn_joins_it() {
        let file = vec![
            message("u1", Role::User),
            message("a1", Role::Assistant),
            message("a2", Role::Assistant),
            message("u2", Role::User),
        ];
        let records = [TurnRecord {
            user_message: Some("u1".into()),
            end: Some(TurnEnd::Done),
            error: None,
            error_kind: None,
            provider: None,
            ended_at_ms: Some(2),
            requests: vec!["r1".into()],
            limit: None,
            context_window: None,
            joins: false,
        }];
        for (continues, own) in [(true, false), (false, true)] {
            let live = LiveTurn {
                open: true,
                continues,
                own,
                awaiting_user: own,
                user_message: Some("u1".into()),
                ..LiveTurn::default()
            };
            let view = view(vec![Arc::new(file.clone())], Some(&live), &records);
            assert_eq!(view.turns.len(), 2, "{view:#?}");
            assert!(view.turns[1].joins, "{view:#?}");
        }
    }
}
