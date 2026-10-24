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
    BackgroundTask, ErrorKind, LimitHit, McpServerState, ProviderEvent, ProviderKind, Role,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::ConversationId;
use crate::work::{CardId, DiffStat, TaskId};

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
}

/// The first words of the one message Brigadier sends at a seam (G12): write your hand-off.
pub(crate) const NUDGE_MARK: &str = "[Brigadier] A fresh session will continue this work.";
/// How a successor's hand-off begins (`[Brigadier hand-off <n> · <session>]`) and ends; the
/// first message of the successor follows it.
pub(crate) const HANDOFF_MARK: &str = "[Brigadier hand-off ";
pub(crate) const HANDOFF_END: &str = "[End of the hand-off]";

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
    /// A permission prompt of its CLI.
    Permission,
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
    /// Brigadier's requests it carried: its user message's, then those steered into it. Their
    /// cards, workers and changes show in the turn.
    pub requests: Vec<String>,
    /// The usage, spend or credits limit that ended it, as its CLI said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<LimitHit>,
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
    pub ended_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requests: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<LimitHit>,
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
}

/// A conversation's live turn: the stream's items since it began.
#[derive(Debug, Default)]
pub(crate) struct LiveTurn {
    pub entries: Vec<FileEntry>,
    /// The user message that started it, once the stream said it.
    pub user_message: Option<String>,
    /// Its CLI's turn started and its user message is still to come (Codex says it after).
    pub awaiting_user: bool,
    pub open: bool,
    pub error: Option<String>,
    /// The limit its CLI hit in it.
    pub limit: Option<LimitHit>,
    /// Brigadier's requests it carries.
    pub requests: Vec<String>,
    /// The CLI's background tasks: they outlast the turn that started them, so a turn that
    /// opens keeps them.
    pub background: Vec<BackgroundTask>,
    /// A command's output is being followed for its last line.
    pub reading_output: bool,
    /// Its CLI's MCP servers: they outlast the turn too.
    pub mcp: Vec<McpServerState>,
}

/// A session file's items, with the length and change time they were read at, and its
/// reading, to read only what it adds next.
struct CachedFile {
    len: u64,
    modified: std::time::SystemTime,
    entries: Arc<Vec<FileEntry>>,
    /// None while a read of it runs.
    reading: Option<session_file::Reading>,
}

/// Session files read, kept while they don't change.
#[derive(Default)]
pub(crate) struct FileCache(Mutex<HashMap<PathBuf, CachedFile>>);

impl FileCache {
    pub(crate) fn read(&self, kind: ProviderKind, path: &Path) -> Arc<Vec<FileEntry>> {
        let path = &session_file::locate(kind, path);
        let stamp = std::fs::metadata(path)
            .ok()
            .and_then(|meta| Some((meta.len(), meta.modified().ok()?)));
        let Some((len, modified)) = stamp else {
            return Arc::default();
        };
        let held = match self.0.lock().unwrap().get_mut(path) {
            Some(file) if file.len == len && file.modified == modified => {
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
                len,
                modified,
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
        if running {
            entries = session_file::merge(entries, live.entries.clone());
        } else {
            entries = add_unheld(entries, &live.entries);
        }
    }
    let mut turns = turns(&entries, records, running);
    if running
        && let Some(live) = live
        && let Some(turn) = turns.last_mut()
    {
        if turn.error.is_none() {
            turn.error.clone_from(&live.error);
        }
        if turn.limit.is_none() {
            turn.limit.clone_from(&live.limit);
        }
        add_requests(&mut turn.requests, &live.requests);
    }
    ThreadView {
        entries,
        turns,
        background: live.map(|live| live.background.clone()).unwrap_or_default(),
        mcp: live.map(|live| live.mcp.clone()).unwrap_or_default(),
        workers: Vec::new(),
        seams: Vec::new(),
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

/// The turns `entries` hold: each user message that isn't a steer starts one.
fn turns(entries: &[FileEntry], records: &[TurnRecord], running: bool) -> Vec<ThreadTurn> {
    let starts: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            entry.parent.is_none() && !entry.steer && user_message(entry).is_some()
        })
        .map(|(index, _)| index)
        .collect();
    // A turn's lines, added up: its requests, and its end. A turn with lines but no end never
    // ended: the CLI or Brigadier went under it (#557).
    let mut recorded: HashMap<&str, (Vec<String>, Option<&TurnRecord>)> = HashMap::new();
    for record in records {
        let Some(id) = record.user_message.as_deref() else {
            continue;
        };
        let (requests, end) = recorded.entry(id).or_default();
        add_requests(requests, &record.requests);
        if record.end.is_some() {
            *end = Some(record);
        }
    }
    let count = starts.len();
    starts
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
                requests: Vec::new(),
                limit: None,
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
            let error = inside.iter().find_map(|entry| match &entry.event {
                ProviderEvent::Error { error } if entry.parent.is_none() => {
                    Some(error.message.clone())
                }
                _ => None,
            });
            let limit = inside.iter().rev().find_map(|entry| match &entry.event {
                ProviderEvent::Error { error } if entry.parent.is_none() => limit_of(error),
                _ => None,
            });
            let noted = recorded.remove(id.as_str());
            let cut = noted.as_ref().is_some_and(|(_, end)| end.is_none());
            let (requests, record) = noted.unwrap_or_default();
            turn.requests = requests;
            turn.limit = record.and_then(|record| record.limit.clone()).or(limit);
            if let Some(record) = record {
                turn.end = record.end;
                turn.error = record.error.clone().or(error);
                turn.ended_at_ms = record.ended_at_ms;
            } else if last && running {
                turn.ended_at_ms = None;
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
            turn
        })
        .collect()
}

/// The limit an error of the CLI says it hit: a usage window, a spend control or credits.
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

/// `record/turns.jsonl`'s turns; none when it is missing.
pub(crate) fn read_turns(record: &Path) -> Vec<TurnRecord> {
    std::fs::read_to_string(record.join("turns.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// How the last turn of `record/turns.jsonl` ended: its last line's end, none when that line
/// is a turn's start (it never ended) or there is none.
pub(crate) fn last_end(record: &Path) -> Option<TurnEnd> {
    let turns = std::fs::read_to_string(record.join("turns.jsonl")).ok()?;
    let last = turns.lines().rev().find(|line| !line.trim().is_empty())?;
    serde_json::from_str::<TurnRecord>(last).ok()?.end
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

    fn entry(event: ProviderEvent) -> FileEntry {
        FileEntry {
            event,
            at_ms: Some(1),
            updated_at_ms: None,
            parent: None,
            steer: false,
            images: Vec::new(),
            phase: None,
        }
    }

    fn message(id: &str, role: Role) -> FileEntry {
        entry(ProviderEvent::Message {
            item_id: id.into(),
            role,
            text: id.into(),
        })
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
}
