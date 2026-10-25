//! What a parent's workers did that the parent has not heard yet (STRIP-DOWN-PLAN S29, O1),
//! kept in its `record/worker-events.jsonl` so none is lost across a restart or a seam and none
//! is heard twice.
//!
//! - Each event has a stable id (`ev-<8 hex>`) and is pending until one holder takes it: a
//!   `brigadier wait` of the parent's, or the safety net (one message into the parent's
//!   thread). Only a pending event can be held, and by one holder at a time.
//! - A wait's hold is done once its `brigadier` printed the events and said so; a wait that
//!   hangs up first frees them. A restart frees every wait's hold: that `brigadier` was gone
//!   with its parent's CLI before it could say so.
//! - The net's hold is done once the parent's CLI took the turn that carries it. After a
//!   restart it is done when the parent's CLI session file holds the message, else freed.
//! - The file only grows by a line per change, and is read back once per daemon, then written
//!   again without what was done.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use std::time::Duration;

use brigadier_cli::protocol::{EventKind, WaitEvent};
use brigadier_providers::{ItemStatus, ProviderEvent};
use serde::{Deserialize, Serialize};

use super::conversation::Envelope;
use super::session_dir::read_lineage;
use super::{SessionManager, blocking};
use crate::model::{ConversationId, Lifecycle};
use crate::now_ms;
use crate::work::{CardId, CardState, InjectionKind, TaskId};

/// The parent's events' file, in its `record/`.
pub(crate) const FILE: &str = "worker-events.jsonl";

/// A done line count past which the file is written again without them at its next read.
const COMPACT_AFTER: usize = 200;

/// Who holds an event: a `brigadier wait` of this daemon's (`wait:<n>`) or the net's message
/// (`net:<id>`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Holder {
    Wait(u64),
    Net(String),
}

impl Holder {
    fn word(&self) -> String {
        match self {
            Self::Wait(n) => format!("wait:{n}"),
            Self::Net(id) => format!("net:{id}"),
        }
    }

    fn parse(word: &str) -> Option<Self> {
        match word.split_once(':')? {
            ("wait", n) => n.parse().ok().map(Self::Wait),
            ("net", id) => Some(Self::Net(id.to_owned())),
            _ => None,
        }
    }
}

/// One line of the file.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "line", rename_all = "kebab-case")]
enum Line {
    Event {
        id: String,
        task: TaskId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        card: Option<CardId>,
        event: Box<WaitEvent>,
    },
    Held {
        ids: Vec<String>,
        by: String,
    },
    /// Heard (`by` its holder), or nobody will be: its worker was closed, or the prompt it
    /// stood for was answered meanwhile.
    Done {
        ids: Vec<String>,
        by: String,
    },
    Freed {
        ids: Vec<String>,
    },
}

/// An event not heard yet.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Kept {
    pub id: String,
    pub task: TaskId,
    /// The card a `needs-input` event stands for: once it is answered, the event is stale.
    pub card: Option<CardId>,
    pub event: WaitEvent,
    pub held: Option<Holder>,
}

/// The file read back: its events not heard yet, oldest first, and how many lines it had.
#[derive(Debug, Default)]
struct Fold {
    events: Vec<Kept>,
    lines: usize,
}

impl Fold {
    fn apply(&mut self, line: Line) {
        self.lines += 1;
        match line {
            Line::Event {
                id,
                task,
                card,
                event,
            } => {
                if self.events.iter().all(|kept| kept.id != id) {
                    self.events.push(Kept {
                        id,
                        task,
                        card,
                        event: *event,
                        held: None,
                    });
                }
            }
            Line::Held { ids, by } => {
                let by = Holder::parse(&by);
                for kept in self.events.iter_mut().filter(|kept| ids.contains(&kept.id)) {
                    kept.held.clone_from(&by);
                }
            }
            Line::Done { ids, .. } => self.events.retain(|kept| !ids.contains(&kept.id)),
            Line::Freed { ids } => {
                for kept in self.events.iter_mut().filter(|kept| ids.contains(&kept.id)) {
                    kept.held = None;
                }
            }
        }
    }

    fn read(file: &Path) -> Self {
        let mut fold = Self::default();
        let Ok(text) = std::fs::read_to_string(file) else {
            return fold;
        };
        // A crash cut the last line short: cut it off, so the next line starts on its own.
        if !text.is_empty() && !text.ends_with('\n') {
            let whole = text.rfind('\n').map_or(0, |end| end + 1);
            let cut = std::fs::OpenOptions::new()
                .write(true)
                .open(file)
                .and_then(|kept| kept.set_len(whole as u64));
            if let Err(err) = cut {
                tracing::warn!(file = %file.display(), error = %err, "could not cut a torn worker event line");
            }
        }
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            match serde_json::from_str(line) {
                Ok(line) => fold.apply(line),
                // A line a crash cut short: what it said never happened.
                Err(err) => {
                    tracing::warn!(file = %file.display(), error = %err, "skipped a worker event line")
                }
            }
        }
        fold
    }
}

/// One parent's events.
struct Parent {
    file: PathBuf,
    fold: Mutex<Fold>,
    /// Its `brigadier wait`s running now.
    waiters: AtomicUsize,
}

impl Parent {
    fn fold(&self) -> MutexGuard<'_, Fold> {
        self.fold.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn append(&self, fold: &mut Fold, lines: &[Line]) {
        let mut text = String::new();
        for line in lines {
            match serde_json::to_string(line) {
                Ok(json) => {
                    text.push_str(&json);
                    text.push('\n');
                }
                Err(err) => tracing::warn!(error = %err, "a worker event could not be written"),
            }
        }
        fold.lines += lines.len();
        let written = self
            .file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.file)
            })
            .and_then(|mut file| file.write_all(text.as_bytes()));
        if let Err(err) = written {
            tracing::warn!(file = %self.file.display(), error = %err, "could not keep a worker event");
        }
    }
}

/// Every parent's events, each read from its file at its first use.
#[derive(Default)]
pub(crate) struct WorkerEvents {
    parents: Mutex<HashMap<ConversationId, Arc<Parent>>>,
    /// Wakes every waiting `brigadier wait` (an event came or was freed).
    pub(crate) added: tokio::sync::Notify,
    /// Numbers this daemon's waits' holds.
    next_wait: AtomicU64,
    /// Workers closed by their parent: what they still do is heard by nobody.
    closed: Mutex<HashSet<TaskId>>,
}

/// A running `brigadier wait`: its parent has a waiter while it lives. Once it ends, however
/// it ends (its `brigadier` hung up too), `then` runs: the net may be due.
pub(crate) struct Waiter {
    parent: Arc<Parent>,
    then: Option<Box<dyn FnOnce() + Send>>,
}

impl Drop for Waiter {
    fn drop(&mut self) {
        self.parent.waiters.fetch_sub(1, Ordering::AcqRel);
        if let Some(then) = self.then.take() {
            then();
        }
    }
}

/// A new event's id.
fn new_id() -> String {
    format!("ev-{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
}

/// A new net message's id.
pub(crate) fn new_net_id() -> String {
    format!("wn-{}", &uuid::Uuid::new_v4().simple().to_string()[..12])
}

impl WorkerEvents {
    /// `conversation`'s events, read from `record` (its `record/` folder) at the first use.
    fn parent(&self, conversation: &ConversationId, record: &Path) -> Arc<Parent> {
        let mut parents = self.parents.lock().unwrap_or_else(PoisonError::into_inner);
        parents
            .entry(conversation.clone())
            .or_insert_with(|| {
                let file = record.join(FILE);
                let fold = Fold::read(&file);
                Arc::new(Parent {
                    file,
                    fold: Mutex::new(fold),
                    waiters: AtomicUsize::new(0),
                })
            })
            .clone()
    }

    /// Reads `conversation`'s file back after a restart: its waits' holds are freed (their
    /// `brigadier` is gone), and the file is written again without what was done when it grew
    /// long. The net's holds still open come back, by message, for the caller to settle.
    pub(crate) fn recover(
        &self,
        conversation: &ConversationId,
        record: &Path,
    ) -> Vec<(String, Vec<String>)> {
        let parent = self.parent(conversation, record);
        let mut fold = parent.fold();
        let waits: Vec<String> = fold
            .events
            .iter()
            .filter(|kept| matches!(kept.held, Some(Holder::Wait(_))))
            .map(|kept| kept.id.clone())
            .collect();
        for kept in &mut fold.events {
            if matches!(kept.held, Some(Holder::Wait(_))) {
                kept.held = None;
            }
        }
        if fold.lines > fold.events.len() + COMPACT_AFTER {
            let mut lines = Vec::new();
            for kept in &fold.events {
                lines.push(Line::Event {
                    id: kept.id.clone(),
                    task: kept.task.clone(),
                    card: kept.card.clone(),
                    event: Box::new(kept.event.clone()),
                });
                if let Some(by) = &kept.held {
                    lines.push(Line::Held {
                        ids: vec![kept.id.clone()],
                        by: by.word(),
                    });
                }
            }
            let mut text = String::new();
            for line in &lines {
                if let Ok(json) = serde_json::to_string(line) {
                    text.push_str(&json);
                    text.push('\n');
                }
            }
            match super::session_dir::write_atomic(&parent.file, text.as_bytes()) {
                Ok(()) => fold.lines = lines.len(),
                Err(err) => tracing::warn!(error = %err, "could not compact the worker events"),
            }
        } else if !waits.is_empty() {
            parent.append(&mut fold, &[Line::Freed { ids: waits }]);
        }
        let mut nets: Vec<(String, Vec<String>)> = Vec::new();
        for kept in &fold.events {
            if let Some(Holder::Net(message)) = &kept.held {
                match nets.iter_mut().find(|(id, _)| id == message) {
                    Some((_, ids)) => ids.push(kept.id.clone()),
                    None => nets.push((message.clone(), vec![kept.id.clone()])),
                }
            }
        }
        nets
    }

    /// Keeps a new event of `task`'s for its parent `conversation`, and wakes the waits.
    pub(crate) fn push(
        &self,
        conversation: &ConversationId,
        record: &Path,
        task: &TaskId,
        card: Option<CardId>,
        event: WaitEvent,
    ) -> Option<String> {
        if self
            .closed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(task)
        {
            return None;
        }
        let parent = self.parent(conversation, record);
        let id = new_id();
        let mut event = event;
        event.event_id.clone_from(&id);
        {
            let mut fold = parent.fold();
            let line = Line::Event {
                id: id.clone(),
                task: task.clone(),
                card: card.clone(),
                event: Box::new(event.clone()),
            };
            parent.append(&mut fold, &[line]);
            fold.events.push(Kept {
                id: id.clone(),
                task: task.clone(),
                card,
                event,
                held: None,
            });
        }
        self.added.notify_waiters();
        Some(id)
    }

    /// A new hold for a `brigadier wait`.
    pub(crate) fn wait_holder(&self) -> Holder {
        Holder::Wait(self.next_wait.fetch_add(1, Ordering::Relaxed))
    }

    /// Counts a `brigadier wait` of `conversation`'s as running while the result lives.
    pub(crate) fn waiter(&self, conversation: &ConversationId, record: &Path) -> Waiter {
        let parent = self.parent(conversation, record);
        parent.waiters.fetch_add(1, Ordering::AcqRel);
        Waiter { parent, then: None }
    }

    /// Whether a `brigadier wait` of `conversation`'s runs now.
    pub(crate) fn waiting(&self, conversation: &ConversationId, record: &Path) -> bool {
        self.parent(conversation, record)
            .waiters
            .load(Ordering::Acquire)
            > 0
    }

    /// Holds `conversation`'s pending events `wanted` picks for `by`, oldest first. Those
    /// `stale` picks (a prompt answered meanwhile) are done instead, for nobody.
    pub(crate) fn hold(
        &self,
        conversation: &ConversationId,
        record: &Path,
        by: &Holder,
        wanted: impl Fn(&Kept) -> bool,
        stale: impl Fn(&Kept) -> bool,
    ) -> Vec<Kept> {
        let parent = self.parent(conversation, record);
        let mut fold = parent.fold();
        let (mut taken, mut gone) = (Vec::new(), Vec::new());
        for kept in fold
            .events
            .iter()
            .filter(|kept| kept.held.is_none() && wanted(kept))
        {
            if stale(kept) {
                gone.push(kept.id.clone());
            } else {
                taken.push(kept.id.clone());
            }
        }
        let mut lines = Vec::new();
        if !gone.is_empty() {
            lines.push(Line::Done {
                ids: gone.clone(),
                by: "stale".into(),
            });
        }
        if !taken.is_empty() {
            lines.push(Line::Held {
                ids: taken.clone(),
                by: by.word(),
            });
        }
        if lines.is_empty() {
            return Vec::new();
        }
        parent.append(&mut fold, &lines);
        fold.events.retain(|kept| !gone.contains(&kept.id));
        let mut held = Vec::new();
        for kept in &mut fold.events {
            if taken.contains(&kept.id) {
                kept.held = Some(by.clone());
                held.push(kept.clone());
            }
        }
        held
    }

    /// What `by` holds of `conversation`'s was heard: done.
    pub(crate) fn done(&self, conversation: &ConversationId, record: &Path, by: &Holder) {
        let parent = self.parent(conversation, record);
        let mut fold = parent.fold();
        let ids: Vec<String> = fold
            .events
            .iter()
            .filter(|kept| kept.held.as_ref() == Some(by))
            .map(|kept| kept.id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        parent.append(
            &mut fold,
            &[Line::Done {
                ids: ids.clone(),
                by: by.word(),
            }],
        );
        fold.events.retain(|kept| !ids.contains(&kept.id));
    }

    /// What `by` holds of `conversation`'s was not heard: pending again, and the waits woken.
    pub(crate) fn free(&self, conversation: &ConversationId, record: &Path, by: &Holder) {
        let parent = self.parent(conversation, record);
        {
            let mut fold = parent.fold();
            let ids: Vec<String> = fold
                .events
                .iter()
                .filter(|kept| kept.held.as_ref() == Some(by))
                .map(|kept| kept.id.clone())
                .collect();
            if ids.is_empty() {
                return;
            }
            parent.append(&mut fold, &[Line::Freed { ids }]);
            for kept in &mut fold.events {
                if kept.held.as_ref() == Some(by) {
                    kept.held = None;
                }
            }
        }
        self.added.notify_waiters();
    }

    /// `conversation`'s events not heard yet, oldest first.
    pub(crate) fn kept(&self, conversation: &ConversationId, record: &Path) -> Vec<Kept> {
        self.parent(conversation, record).fold().events.clone()
    }

    /// Worker `task` of `conversation` was closed: nobody hears what it left or still does.
    pub(crate) fn forget(&self, conversation: &ConversationId, record: &Path, task: &TaskId) {
        self.closed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(task.clone());
        let parent = self.parent(conversation, record);
        let mut fold = parent.fold();
        let ids: Vec<String> = fold
            .events
            .iter()
            .filter(|kept| kept.task == *task && kept.held.is_none())
            .map(|kept| kept.id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        parent.append(
            &mut fold,
            &[Line::Done {
                ids: ids.clone(),
                by: "closed".into(),
            }],
        );
        fold.events.retain(|kept| !ids.contains(&kept.id));
    }

    /// Whether `conversation`'s events were read in this daemon and some are not heard yet.
    pub(crate) fn loaded(&self, conversation: &ConversationId) -> bool {
        let parent = self
            .parents
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(conversation)
            .cloned();
        parent.is_some_and(|parent| !parent.fold().events.is_empty())
    }
}

/// How long an event waits, its parent's turn ended and no `brigadier wait` of its running,
/// before the net sends it: long enough for a wait started as the turn ends to take it, short
/// enough that the parent hears it within 2 s (O1).
#[cfg(not(test))]
const NET_AFTER: Duration = Duration::from_millis(1500);
#[cfg(test)]
const NET_AFTER: Duration = Duration::from_millis(300);

/// How long a working worker's CLI may say nothing before its parent hears it is quiet, unless
/// [`QUIET_ENV`] sets it in seconds.
const QUIET: Duration = Duration::from_secs(20 * 60);
const QUIET_ENV: &str = "BRIGADIER_WORKER_QUIET_SECS";

/// The most of a worker's message a net line carries.
const EXCERPT_CHARS: usize = 160;

/// What a working worker's CLI did last, for its `quiet` event.
#[derive(Debug, Default)]
pub(crate) struct Watch {
    conversation: Option<ConversationId>,
    last_ms: i64,
    working: bool,
    /// Its commands and tool calls under way, by item.
    running: Vec<(String, String)>,
    /// Its parent heard it is quiet since its last event.
    told: bool,
    /// Its last event asks the user's permission: it waits on them, not quiet.
    asking: bool,
}

/// Its watch of each worker, and how long one may be quiet.
pub(crate) struct Quiet {
    watches: Mutex<HashMap<TaskId, Watch>>,
    after_ms: AtomicU64,
    /// Wakes the timer when the limit changes.
    changed: tokio::sync::Notify,
}

impl Default for Quiet {
    fn default() -> Self {
        let after = std::env::var(QUIET_ENV)
            .ok()
            .and_then(|secs| secs.trim().parse::<u64>().ok())
            .map_or(QUIET, Duration::from_secs);
        Self {
            watches: Mutex::default(),
            after_ms: AtomicU64::new(u64::try_from(after.as_millis()).unwrap_or(u64::MAX)),
            changed: tokio::sync::Notify::new(),
        }
    }
}

impl Quiet {
    /// Follows `event` of worker `task`'s CLI.
    pub(crate) fn saw(
        &self,
        conversation: &ConversationId,
        task: &TaskId,
        event: &ProviderEvent,
        at: i64,
    ) {
        let mut watches = self.watches.lock().unwrap_or_else(PoisonError::into_inner);
        let watch = watches.entry(task.clone()).or_default();
        watch.conversation = Some(conversation.clone());
        watch.last_ms = at;
        watch.told = false;
        watch.asking = matches!(event, ProviderEvent::ApprovalRequested { .. });
        match event {
            ProviderEvent::TurnStarted { .. }
            | ProviderEvent::Message {
                role: brigadier_providers::Role::User,
                ..
            } => watch.working = true,
            ProviderEvent::TurnCompleted { .. } | ProviderEvent::Exited { .. } => {
                watch.working = false;
                watch.running.clear();
            }
            ProviderEvent::Command {
                item_id,
                command,
                status,
                ..
            } => running(watch, item_id, *status, || {
                format!(
                    "`{}`",
                    brigadier_providers::policy::unwrapped_command(command)
                )
            }),
            ProviderEvent::ToolCall {
                item_id,
                name,
                status,
                ..
            } => running(watch, item_id, *status, || name.clone()),
            _ => {}
        }
    }

    /// The workers quiet past the limit, now told: each with its parent and what runs.
    fn due(&self, now: i64) -> Vec<(ConversationId, TaskId, Vec<String>, i64)> {
        let after = i64::try_from(self.after_ms.load(Ordering::Relaxed)).unwrap_or(i64::MAX);
        let mut watches = self.watches.lock().unwrap_or_else(PoisonError::into_inner);
        let mut due = Vec::new();
        for (task, watch) in watches.iter_mut() {
            if watch.working
                && !watch.told
                && !watch.asking
                && now.saturating_sub(watch.last_ms) >= after
            {
                watch.told = true;
                if let Some(conversation) = &watch.conversation {
                    let what = watch.running.iter().map(|(_, what)| what.clone()).collect();
                    due.push((conversation.clone(), task.clone(), what, after));
                }
            }
        }
        due
    }

    fn forget(&self, task: &TaskId) {
        self.watches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(task);
    }
}

fn running(watch: &mut Watch, item: &str, status: ItemStatus, what: impl FnOnce() -> String) {
    watch.running.retain(|(id, _)| id != item);
    if status == ItemStatus::InProgress {
        watch.running.push((item.to_owned(), what()));
    }
}

/// The net's word for an event.
fn net_word(kind: EventKind) -> &'static str {
    match kind {
        EventKind::TurnEnded => "finished",
        EventKind::NeedsInput => "asks",
        EventKind::Failed => "failed",
        EventKind::Interrupted => "stopped",
        EventKind::Gone => "ended",
        EventKind::UsageLimit => "hit a usage limit",
        EventKind::FreshContext => "compacted its context",
        EventKind::Quiet => "is quiet",
    }
}

/// The net's one message: a line per worker event, and their ids.
fn net_text(message: &str, held: &[Kept], titles: &HashMap<TaskId, String>) -> String {
    let lines: Vec<String> = held
        .iter()
        .map(|kept| {
            let title = titles.get(&kept.task).map_or("", String::as_str);
            let what = kept
                .event
                .asks
                .as_deref()
                .or(kept.event.message.as_deref())
                .and_then(|text| text.lines().map(str::trim).find(|line| !line.is_empty()))
                .map(|line| {
                    let cut: String = line.chars().take(EXCERPT_CHARS).collect();
                    if cut.len() < line.len() {
                        format!(" · {cut}…")
                    } else {
                        format!(" · {cut}")
                    }
                })
                .unwrap_or_default();
            format!(
                "Worker \"{title}\" ({}): {}{what}{}",
                kept.event.id,
                net_word(kept.event.event),
                super::work_facts::net_facts(&kept.event)
            )
        })
        .collect();
    let ids: Vec<&str> = held.iter().map(|kept| kept.id.as_str()).collect();
    format!(
        "[worker events {message}] {} ({})",
        lines.join("; "),
        ids.join(", ")
    )
}

/// Whether the card a `needs-input` event stands for was answered meanwhile.
fn answered(board: &crate::board::Board, kept: &Kept) -> bool {
    let Some(card) = &kept.card else {
        return false;
    };
    match board.approvals.get(card) {
        Some(approval) => approval.state != CardState::Pending,
        None => board
            .questions
            .get(card)
            .is_none_or(|question| question.answer.is_some() || question.answered_at_ms.is_some()),
    }
}

/// A `brigadier wait`'s events, held for it until its `brigadier` printed them.
pub struct HeldEvents {
    conversation: ConversationId,
    holder: Holder,
}

impl SessionManager {
    fn events_record(&self, id: &ConversationId) -> PathBuf {
        self.session_dir(id).record()
    }

    /// Keeps an event of worker `task` for its parent `conversation`: a `brigadier wait`
    /// takes it, or the net once the parent's turn ended.
    pub(super) fn worker_event(
        &self,
        conversation: &ConversationId,
        task: &TaskId,
        card: Option<CardId>,
        event: WaitEvent,
    ) {
        let record = self.events_record(conversation);
        if self
            .worker_events
            .push(conversation, &record, task, card, event)
            .is_some()
        {
            self.net_soon(conversation);
        }
    }

    /// Holds `conversation`'s events for a `brigadier wait` of its workers `tasks` (all of
    /// them when empty), oldest first.
    pub(super) async fn hold_for_wait(
        &self,
        conversation: &ConversationId,
        tasks: &HashSet<TaskId>,
        holder: &Holder,
    ) -> crate::Result<Vec<WaitEvent>> {
        let board = self.core.board(conversation).await?;
        let record = self.events_record(conversation);
        Ok(self
            .worker_events
            .hold(
                conversation,
                &record,
                holder,
                |kept| tasks.is_empty() || tasks.contains(&kept.task),
                |kept| answered(&board, kept),
            )
            .into_iter()
            .map(|kept| kept.event)
            .collect())
    }

    /// A running `brigadier wait` of `conversation`'s, while the result lives; the net looks
    /// again once it ends.
    pub(super) fn events_waiter(&self, conversation: &ConversationId) -> Waiter {
        let record = self.events_record(conversation);
        let mut waiter = self.worker_events.waiter(conversation, &record);
        let (manager, id) = (self.me.clone(), conversation.clone());
        waiter.then = Some(Box::new(move || {
            if let Some(manager) = manager.upgrade() {
                manager.net_soon(&id);
            }
        }));
        waiter
    }

    pub(super) fn held_events(&self, conversation: &ConversationId, holder: Holder) -> HeldEvents {
        HeldEvents {
            conversation: conversation.clone(),
            holder,
        }
    }

    /// The `brigadier wait` printed its events: heard.
    pub fn events_heard(&self, held: HeldEvents) {
        let record = self.events_record(&held.conversation);
        self.worker_events
            .done(&held.conversation, &record, &held.holder);
    }

    /// The `brigadier wait` ended before it printed its events: they are its parent's again.
    pub fn events_unheard(&self, held: HeldEvents) {
        let record = self.events_record(&held.conversation);
        self.worker_events
            .free(&held.conversation, &record, &held.holder);
        self.net_soon(&held.conversation);
    }

    /// Worker `task` was closed by its parent: nobody hears what it left or still does.
    pub(super) fn forget_worker_events(&self, conversation: &ConversationId, task: &TaskId) {
        let record = self.events_record(conversation);
        self.worker_events.forget(conversation, &record, task);
        self.worker_quiet.forget(task);
        self.forget_worker_commands(task);
    }

    /// The net looks at `conversation` soon: after its turn ends, an event comes, or a wait
    /// ends without taking them.
    pub(super) fn net_soon(&self, conversation: &ConversationId) {
        if !self.worker_events.loaded(conversation) {
            return;
        }
        let (manager, id) = (self.me.clone(), conversation.clone());
        self.spawn(async move {
            tokio::time::sleep(NET_AFTER).await;
            if let Some(manager) = manager.upgrade() {
                manager.net(&id).await;
            }
        });
    }

    /// The safety net (Q9): `conversation`'s turn ended, no `brigadier wait` of its runs, and
    /// events waited [`NET_AFTER`]: one message into its thread carries them all, once.
    async fn net(&self, id: &ConversationId) {
        if self.admit().is_err() || self.is_closing(id) {
            return;
        }
        let record = self.events_record(id);
        if self.worker_events.waiting(id, &record) {
            return;
        }
        let Ok(conversation) = self.core.conversation(id) else {
            return;
        };
        if conversation.lifecycle == Lifecycle::Archived {
            return;
        }
        let Ok(conv) = self.conv(id) else {
            return;
        };
        if conv.is_busy().await {
            return;
        }
        let after = i64::try_from(NET_AFTER.as_millis()).unwrap_or(i64::MAX);
        let now = now_ms();
        let due = self
            .worker_events
            .kept(id, &record)
            .iter()
            .any(|kept| kept.held.is_none() && now.saturating_sub(kept.event.at_ms) >= after);
        if !due {
            return;
        }
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        let message = new_net_id();
        let holder = Holder::Net(message.clone());
        let held = self.worker_events.hold(
            id,
            &record,
            &holder,
            |_| true,
            |kept| answered(&board, kept),
        );
        if held.is_empty() {
            return;
        }
        let titles: HashMap<TaskId, String> = board
            .tasks
            .values()
            .map(|task| (task.id.clone(), task.title.clone()))
            .collect();
        let envelope = Envelope {
            kind: InjectionKind::WorkerEvents,
            label: format!("worker events {message}"),
            task_id: None,
            text: net_text(&message, &held, &titles),
        };
        self.deliver_for(id, envelope, None).await;
    }

    /// The parent's CLI took the turn carrying the net's message `label`: its events were
    /// heard.
    pub(super) fn net_heard(&self, conversation: &ConversationId, label: &str) {
        if let Some(message) = label.strip_prefix("worker events ") {
            let record = self.events_record(conversation);
            self.worker_events
                .done(conversation, &record, &Holder::Net(message.to_owned()));
        }
    }

    /// After a restart: `conversation`'s events come back; the net's messages its CLI's session
    /// file holds were heard, the others are sent again.
    pub(super) async fn recover_worker_events(&self, id: &ConversationId) {
        let record = self.events_record(id);
        if !record.join(FILE).is_file() {
            return;
        }
        let nets = self.worker_events.recover(id, &record);
        if !nets.is_empty() {
            let (at, wanted) = (record.clone(), nets.clone());
            let heard = blocking(move || {
                let mut text = String::new();
                for session in read_lineage(&at) {
                    let Some(file) = session.file else {
                        continue;
                    };
                    let path = brigadier_providers::session_file::locate(
                        session.provider,
                        Path::new(&file),
                    );
                    if let Ok(read) = std::fs::read_to_string(path) {
                        text.push_str(&read);
                    }
                }
                Ok(wanted
                    .iter()
                    .map(|(message, _)| text.contains(message.as_str()))
                    .collect::<Vec<bool>>())
            })
            .await
            .unwrap_or_default();
            for (at, (message, _)) in nets.into_iter().enumerate() {
                let holder = Holder::Net(message);
                if heard.get(at).copied().unwrap_or(false) {
                    self.worker_events.done(id, &record, &holder);
                } else {
                    self.worker_events.free(id, &record, &holder);
                }
            }
        }
        self.net_soon(id);
    }

    /// Follows an event of worker `task`'s CLI, for its `quiet` event.
    pub(super) fn watch_worker(
        &self,
        conversation: &ConversationId,
        task: &TaskId,
        event: &ProviderEvent,
    ) {
        self.worker_quiet.saw(conversation, task, event, now_ms());
    }

    /// Looks for quiet workers every minute (more often when the limit is short).
    pub(super) fn start_quiet_timer(&self) {
        let (manager, quiet) = (self.me.clone(), self.worker_quiet.clone());
        self.spawn(async move {
            loop {
                let after = quiet.after_ms.load(Ordering::Relaxed) / 4;
                let every = Duration::from_millis(after.clamp(20, 60_000));
                tokio::select! {
                    () = tokio::time::sleep(every) => {}
                    () = quiet.changed.notified() => {}
                }
                let Some(manager) = manager.upgrade() else {
                    return;
                };
                if manager.admit().is_err() {
                    return;
                }
                manager.tell_quiet().await;
            }
        });
    }

    async fn tell_quiet(&self) {
        for (conversation, task, running, after) in self.worker_quiet.due(now_ms()) {
            let minutes = after / 60_000;
            let quiet = if minutes > 0 {
                format!("No word from its CLI for {minutes} min")
            } else {
                format!("No word from its CLI for {} s", after / 1000)
            };
            let message = if running.is_empty() {
                format!("{quiet}; nothing of its is shown running.")
            } else {
                format!("{quiet}; running: {}.", running.join(", "))
            };
            let mut event = super::plain_workers::event(&task, EventKind::Quiet);
            event.message = Some(message);
            self.worker_event(&conversation, &task, None, event);
        }
    }

    /// Tests: how long a working worker may be quiet.
    #[cfg(all(test, unix))]
    pub(crate) fn set_worker_quiet(&self, after: Duration) {
        self.worker_quiet.after_ms.store(
            u64::try_from(after.as_millis()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        self.worker_quiet.changed.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of its own, removed after the test.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let dir =
                std::env::temp_dir().join(format!("brigadier-events-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn event(kind: EventKind) -> WaitEvent {
        WaitEvent {
            id: "t-aaaa".into(),
            event_id: String::new(),
            event: kind,
            message: None,
            asks: None,
            resets_at_ms: None,
            at_ms: 1,
            git: None,
            commands: None,
        }
    }

    fn all(_: &Kept) -> bool {
        true
    }

    fn none(_: &Kept) -> bool {
        false
    }

    #[test]
    fn an_event_is_held_by_one_holder_and_done_once() {
        let dir = Scratch::new();
        let events = WorkerEvents::default();
        let (me, other) = (ConversationId("c1".into()), ConversationId("c2".into()));
        let (a, b) = (TaskId("t-a".into()), TaskId("t-b".into()));
        let (mine, theirs) = (dir.0.as_path().join("c1"), dir.0.as_path().join("c2"));
        events.push(&me, &mine, &a, None, event(EventKind::TurnEnded));
        events.push(&me, &mine, &b, None, event(EventKind::NeedsInput));
        events.push(&other, &theirs, &b, None, event(EventKind::Gone));

        let (one, two) = (events.wait_holder(), events.wait_holder());
        let only_a = |kept: &Kept| kept.task == a;
        assert_eq!(events.hold(&me, &mine, &one, only_a, none).len(), 1);
        assert!(
            events.hold(&me, &mine, &two, only_a, none).is_empty(),
            "held once"
        );
        let rest = events.hold(&me, &mine, &two, all, none);
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].event.event, EventKind::NeedsInput);
        events.done(&me, &mine, &one);
        events.free(&me, &mine, &two);
        let left = events.kept(&me, &mine);
        assert_eq!(left.len(), 1, "{left:?}");
        assert_eq!(left[0].held, None, "freed: pending again");
        assert_eq!(events.kept(&other, &theirs).len(), 1);
    }

    #[test]
    fn a_restart_frees_the_waits_holds_and_returns_the_nets() {
        let dir = Scratch::new();
        let me = ConversationId("c1".into());
        let task = TaskId("t-a".into());
        let record = dir.0.as_path().to_owned();
        let (by_wait, by_net) = {
            let events = WorkerEvents::default();
            for kind in [EventKind::TurnEnded, EventKind::Failed, EventKind::Quiet] {
                events.push(&me, &record, &task, None, event(kind));
            }
            let wait = events.wait_holder();
            let first = |kept: &Kept| kept.event.event == EventKind::TurnEnded;
            let by_wait = events.hold(&me, &record, &wait, first, none);
            let net = Holder::Net(new_net_id());
            let by_net = events.hold(&me, &record, &net, all, none);
            (by_wait, by_net)
        };
        assert_eq!((by_wait.len(), by_net.len()), (1, 2));

        let events = WorkerEvents::default();
        let nets = events.recover(&me, &record);
        assert_eq!(nets.len(), 1);
        assert_eq!(
            nets[0].1,
            by_net
                .iter()
                .map(|kept| kept.id.clone())
                .collect::<Vec<_>>()
        );
        let kept = events.kept(&me, &record);
        assert_eq!(kept.len(), 3);
        assert_eq!(kept[0].held, None, "the wait's hold is freed");
        assert!(matches!(kept[1].held, Some(Holder::Net(_))));
        // The ids stay what they were.
        assert_eq!(kept[0].id, by_wait[0].id);
    }

    #[test]
    fn stale_and_closed_events_are_done_for_nobody() {
        let dir = Scratch::new();
        let events = WorkerEvents::default();
        let me = ConversationId("c1".into());
        let (a, b) = (TaskId("t-a".into()), TaskId("t-b".into()));
        let record = dir.0.as_path().to_owned();
        let card = CardId("card-1".into());
        events.push(
            &me,
            &record,
            &a,
            Some(card.clone()),
            event(EventKind::NeedsInput),
        );
        events.push(&me, &record, &b, None, event(EventKind::TurnEnded));
        events.forget(&me, &record, &b);
        assert!(
            events
                .push(&me, &record, &b, None, event(EventKind::Gone))
                .is_none(),
            "a closed worker is heard by nobody"
        );
        let wait = events.wait_holder();
        let held = events.hold(&me, &record, &wait, all, |kept| {
            kept.card == Some(card.clone())
        });
        assert!(held.is_empty(), "{held:?}");
        assert!(events.kept(&me, &record).is_empty());
        // And so it reads after a restart.
        let again = WorkerEvents::default();
        again.recover(&me, &record);
        assert!(again.kept(&me, &record).is_empty());
    }

    #[test]
    fn a_long_file_is_written_again_without_what_was_done() {
        let dir = Scratch::new();
        let me = ConversationId("c1".into());
        let task = TaskId("t-a".into());
        let record = dir.0.as_path().to_owned();
        {
            let events = WorkerEvents::default();
            for _ in 0..COMPACT_AFTER {
                events.push(&me, &record, &task, None, event(EventKind::TurnEnded));
                let wait = events.wait_holder();
                events.hold(&me, &record, &wait, all, none);
                events.done(&me, &record, &wait);
            }
            events.push(&me, &record, &task, None, event(EventKind::Gone));
        }
        let events = WorkerEvents::default();
        events.recover(&me, &record);
        let text = std::fs::read_to_string(record.join(FILE)).unwrap();
        assert_eq!(text.lines().count(), 1, "{text}");
        assert_eq!(events.kept(&me, &record)[0].event.event, EventKind::Gone);
    }

    #[test]
    fn a_line_a_crash_cut_short_does_not_swallow_the_next() {
        let dir = Scratch::new();
        let me = ConversationId("c1".into());
        let task = TaskId("t-a".into());
        let record = dir.0.as_path().to_owned();
        {
            let events = WorkerEvents::default();
            events.push(&me, &record, &task, None, event(EventKind::TurnEnded));
        }
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(record.join(FILE))
            .unwrap();
        file.write_all(br#"{"line":"event","id":"ev-0"#).unwrap();
        drop(file);
        {
            let events = WorkerEvents::default();
            events.recover(&me, &record);
            events.push(&me, &record, &task, None, event(EventKind::Gone));
        }
        let events = WorkerEvents::default();
        events.recover(&me, &record);
        let kinds: Vec<EventKind> = events
            .kept(&me, &record)
            .into_iter()
            .map(|kept| kept.event.event)
            .collect();
        assert_eq!(kinds, [EventKind::TurnEnded, EventKind::Gone]);
    }
}
