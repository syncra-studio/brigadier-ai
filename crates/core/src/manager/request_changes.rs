//! Evidence for the files each request changed, retained independently of its checkout's history.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use brigadier_git::{FileBlob, Oid};
use brigadier_providers::{FileChangeKind, ItemStatus, ProviderEvent};
use brigadier_store::{NewEvent, StreamPage};
use serde::{Deserialize, Serialize};

use super::shell_changes::{ShellCaptures, ShellEvidence, ShellKey};
use super::{SessionManager, blocking, git_error};
use crate::model::{ConversationId, Environment, Setup};
use crate::work::{HeldPath, RequestChanges, RequestState};
use crate::{Error, Result};

#[derive(Default)]
pub(super) struct ChangeRecords {
    locks: std::sync::Mutex<HashMap<ConversationId, Arc<tokio::sync::Mutex<SnapshotClock>>>>,
    /// Thread events waiting for a snapshot, in the order the thread saw them. Whoever holds
    /// the session's lock takes them first, so a later refresh never overtakes them.
    queued: std::sync::Mutex<HashMap<ConversationId, VecDeque<Queued>>>,
    /// The files shell commands named, read around them, until their calls are reported.
    pub(super) shells: ShellCaptures,
}

#[derive(Default)]
pub(super) struct SnapshotClock {
    at: Option<tokio::time::Instant>,
    slow: bool,
}

impl ChangeRecords {
    pub(super) fn lock(&self, id: &ConversationId) -> Arc<tokio::sync::Mutex<SnapshotClock>> {
        self.locks
            .lock()
            .unwrap()
            .entry(id.clone())
            .or_default()
            .clone()
    }

    fn push(&self, id: &ConversationId, queued: Queued) {
        self.queued
            .lock()
            .unwrap()
            .entry(id.clone())
            .or_default()
            .push_back(queued);
    }

    pub(super) fn pending(&self, id: &ConversationId) -> bool {
        self.queued
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|queue| !queue.is_empty())
    }

    /// The front run of queued events for one request.
    fn take(&self, id: &ConversationId) -> Option<(String, Vec<Queued>)> {
        let mut queues = self.queued.lock().unwrap();
        let queue = queues.get_mut(id)?;
        let request = queue.front()?.request.clone();
        let mut taken = Vec::new();
        while queue
            .front()
            .is_some_and(|queued| queued.request == request)
        {
            taken.extend(queue.pop_front());
        }
        if queue.is_empty() {
            queues.remove(id);
        }
        Some((request, taken))
    }
}

/// One thread event's part in a request's changes, read when the thread reported it.
pub(super) struct Queued {
    request: String,
    key: Option<String>,
    edits: Vec<Evidence>,
    command: bool,
    /// What a shell command changed, by the files it names read around it.
    shell: Option<ShellEvidence>,
}

/// What one reported edit did to one file. Its `after` is read when the edit is reported, so a
/// later edit to the same file can't be mistaken for someone else's.
struct Evidence {
    path: String,
    before: Before,
    after: Option<Blob>,
}

/// One part of a snapshot's evidence, in the order the thread reported it, with whether a
/// command reported before it may explain what its files held before it.
enum Step {
    Edit(Evidence, bool),
    Shell(ShellEvidence, bool),
}

enum Before {
    Known(Option<Blob>),
    /// The edit gave nothing to check its original against: the last snapshot stands in.
    Unverified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Blob {
    oid: String,
    mode: String,
}
impl From<FileBlob> for Blob {
    fn from(blob: FileBlob) -> Self {
        Self {
            oid: blob.oid,
            mode: blob.mode,
        }
    }
}
impl From<Blob> for FileBlob {
    fn from(blob: Blob) -> Self {
        Self {
            oid: blob.oid,
            mode: blob.mode,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OwnedPath {
    first: Option<Blob>,
    last: Option<Blob>,
    by: BTreeSet<String>,
    held: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Record {
    latest: String,
    pub(super) commit: String,
    paths: BTreeMap<String, OwnedPath>,
    restored: Vec<String>,
    /// What Undo put back by a commit (its change was committed): Reapply commits those back
    /// and leaves the rest in the working files, as they were.
    #[serde(default)]
    committed: BTreeSet<String>,
    settled: bool,
    seen: BTreeSet<String>,
    /// The files one shell command changed: Undo puts them back together or leaves them all,
    /// so a move (`mv a b`) never loses its one copy.
    #[serde(default)]
    together: Vec<BTreeSet<String>>,
    /// What a command left in a file no snapshot saw, before a later one moved it on.
    #[serde(default)]
    unseen: BTreeMap<String, Option<Blob>>,
    /// Ignored files of the user's a command moved, removed or changed: no record keeps them,
    /// so the files changed with them stay as they are.
    #[serde(default)]
    kept_out: BTreeSet<String>,
}

pub(super) enum Refresh<'a> {
    Start,
    Event(&'a ProviderEvent),
    Landing { from: &'a Oid, tip: &'a Oid },
    Settle,
    Recover,
}

enum Pass {
    Start,
    Events(Vec<Queued>),
    Landing(Oid, Oid),
    Settle,
    Recover,
}

/// After one slow snapshot, the next waits a second from the last, so a burst of edits on a big
/// checkout costs a snapshot a second rather than one each.
async fn pace(clock: &mut SnapshotClock) {
    if clock.slow
        && let Some(at) = clock.at
    {
        tokio::time::sleep_until(at + std::time::Duration::from_secs(1)).await;
    }
}

/// Drops the whole file an edit came from (Claude's `originalFile`) from an event that is
/// stored or sent: only the request's record reads it, from the live event, and whole files
/// would make stored events, and the pages and messages made of them, too big to send.
pub(super) fn without_originals(mut event: ProviderEvent) -> ProviderEvent {
    if let ProviderEvent::FileChanges { changes, .. } = &mut event {
        for change in changes {
            change.original = None;
        }
    }
    event
}

fn stream(id: &ConversationId, request: &str) -> String {
    format!("changes:{id}:{request}")
}
fn prefix(id: &ConversationId, request: &str) -> String {
    format!("refs/brigadier/changes/{id}/{request}")
}
fn user_reason(path: &str) -> String {
    format!("You changed {path} during this request too, so Undo leaves it as it is.")
}
fn together_reason(path: &str, held: &str) -> String {
    format!(
        "Commands changed {path} together with {held}, which Undo leaves as it is, so it leaves both."
    )
}
fn command_reason(path: &str) -> String {
    format!(
        "A command changed {path}, and Brigadier can't tell that change from yours, so Undo leaves it as it is."
    )
}

impl Record {
    #[allow(clippy::too_many_arguments)]
    fn own(
        &mut self,
        path: &str,
        before: Option<Blob>,
        after: Option<Blob>,
        previous: Option<Blob>,
        by: &str,
        weak: bool,
        after_command: bool,
    ) {
        // A command's snapshot runs behind the thread, so it can read an edit or a worker's
        // landing reported just after the command and claim it. Their own evidence of that
        // same change takes the file back.
        if (by == "edit" || by == "landing" || by == "shell")
            && !weak
            && let Some(owned) = self.paths.get_mut(path)
            && owned.by.len() == 1
            && owned.by.contains("command")
            && owned.first == before
            && owned.last == after
        {
            owned.by = BTreeSet::from([by.to_owned()]);
            owned.held = None;
            return;
        }
        let owned = self
            .paths
            .entry(path.to_owned())
            .or_insert_with(|| OwnedPath {
                first: before.clone(),
                last: previous,
                by: BTreeSet::new(),
                held: None,
            });
        if owned.last != before {
            owned.held = Some(if after_command {
                command_reason(path)
            } else {
                user_reason(path)
            });
        }
        if by == "command" {
            owned.held = Some(command_reason(path));
        }
        if weak && owned.held.is_none() {
            owned.held = Some(format!(
                "Brigadier could not verify the original contents of {path}, so Undo leaves it as it is."
            ));
        }
        owned.last = after;
        owned.by.insert(by.to_owned());
    }

    fn trees(
        &self,
    ) -> (
        BTreeMap<String, Option<FileBlob>>,
        BTreeMap<String, Option<FileBlob>>,
    ) {
        let before = self
            .paths
            .iter()
            .map(|(path, owned)| (path.clone(), owned.first.clone().map(Into::into)))
            .collect();
        let after = self
            .paths
            .iter()
            .map(|(path, owned)| (path.clone(), owned.last.clone().map(Into::into)))
            .collect();
        (before, after)
    }
    /// One shell command's files go together: one left as it is leaves them all, and so on
    /// through every command that shares a file with them (`mv a b`, then `mv b c`).
    fn hold_together(&mut self) {
        // Each file left as it is, and the held file that leaves it.
        let mut held: BTreeMap<String, String> = self
            .paths
            .iter()
            .filter(|(_, owned)| owned.held.is_some())
            .map(|(path, _)| path)
            .chain(&self.kept_out)
            .map(|path| (path.clone(), path.clone()))
            .collect();
        loop {
            let mut grew = false;
            for group in &self.together {
                let Some(from) = group.iter().find_map(|path| held.get(path).cloned()) else {
                    continue;
                };
                for path in group {
                    if !held.contains_key(path) {
                        held.insert(path.clone(), from.clone());
                        grew = true;
                    }
                }
            }
            if !grew {
                break;
            }
        }
        for (path, from) in held {
            if let Some(owned) = self.paths.get_mut(&path)
                && owned.held.is_none()
            {
                owned.held = Some(together_reason(&path, &from));
            }
        }
    }

    fn held(&self) -> Vec<HeldPath> {
        self.paths
            .iter()
            .filter(|(_, owned)| owned.first != owned.last)
            .filter_map(|(path, owned)| {
                owned.held.as_ref().map(|reason| HeldPath {
                    path: path.clone(),
                    reason: reason.clone(),
                })
            })
            .collect()
    }
}

impl Record {
    /// Each file the request changed: what it held before and after, and why Undo leaves it,
    /// if it does.
    #[allow(clippy::type_complexity)]
    pub(super) fn changed(
        &self,
    ) -> impl Iterator<Item = (&str, Option<&Blob>, Option<&Blob>, Option<&str>)> {
        self.paths
            .iter()
            .filter(|(_, owned)| owned.first != owned.last)
            .map(|(path, owned)| {
                (
                    path.as_str(),
                    owned.first.as_ref(),
                    owned.last.as_ref(),
                    owned.held.as_deref(),
                )
            })
    }
}

impl SessionManager {
    /// Both session environments count, but overnight workspaces and Chats do not.
    pub(super) fn changes_checkout(
        &self,
        id: &ConversationId,
    ) -> Option<(PathBuf, Option<String>)> {
        if self
            .overnight
            .active
            .get(id)
            .and_then(|active| active.workspace)
            .is_some()
        {
            return None;
        }
        match self.core.conversation(id).ok()?.setup? {
            Setup::Session {
                repo,
                environment: Environment::LocalCheckout { .. },
                ..
            } => Some((repo.into(), None)),
            Setup::Session {
                environment:
                    Environment::NewWorktree {
                        path: Some(path),
                        branch,
                        ..
                    },
                ..
            } => Some((path.into(), Some(branch))),
            _ => None,
        }
    }

    pub(super) async fn change_record(
        &self,
        id: &ConversationId,
        request: &str,
    ) -> Result<Option<Record>> {
        let events = self
            .core
            .store()
            .read_stream(
                stream(id, request),
                StreamPage {
                    limit: 1,
                    ..Default::default()
                },
            )
            .await?;
        events
            .first()
            .map(|event| {
                serde_json::from_str(event.payload.get()).map_err(|source| Error::Corrupt {
                    seq: event.seq,
                    source,
                })
            })
            .transpose()
    }

    async fn save_change_record(
        &self,
        id: &ConversationId,
        request: &str,
        record: &Record,
    ) -> Result<()> {
        self.core
            .store()
            .append(vec![NewEvent::new(
                stream(id, request),
                "request.change_record",
                crate::now_ms(),
                record,
            )?])
            .await?;
        Ok(())
    }

    /// One lock per session serializes snapshots and the evidence they explain.
    pub(super) async fn refresh_changes(
        &self,
        id: &ConversationId,
        request: Option<&str>,
        refresh: Refresh<'_>,
    ) {
        let Some(request) = request else { return };
        if let Err(err) = self.refresh_changes_inner(id, request, refresh).await {
            tracing::warn!(conversation = %id, error = %err, "could not refresh request changes");
        }
    }

    async fn refresh_changes_inner(
        &self,
        id: &ConversationId,
        request: &str,
        refresh: Refresh<'_>,
    ) -> Result<()> {
        let Some((checkout, _)) = self.changes_checkout(id) else {
            return Ok(());
        };
        let pass = match refresh {
            Refresh::Start => Pass::Start,
            Refresh::Event(event) => match self.queued_change(id, &checkout, request, event).await?
            {
                Some(queued) => Pass::Events(vec![queued]),
                None => return Ok(()),
            },
            Refresh::Landing { from, tip } => Pass::Landing(from.clone(), tip.clone()),
            Refresh::Settle => Pass::Settle,
            Refresh::Recover => Pass::Recover,
        };
        let settle = matches!(pass, Pass::Settle);
        let lock = self.change_records.lock(id);
        let mut clock = lock.lock().await;
        self.drain_changes(id, &checkout, &mut clock).await;
        pace(&mut clock).await;
        self.apply_changes(id, request, &checkout, &mut clock, pass)
            .await?;
        if settle {
            self.absorb_pending(id, request, &checkout).await?;
        }
        Ok(())
    }

    /// Applies what the thread reported before anything that comes after it.
    pub(super) async fn drain_changes(
        &self,
        id: &ConversationId,
        checkout: &Path,
        clock: &mut SnapshotClock,
    ) {
        while self.change_records.pending(id) {
            // Events reported while this waits join the same snapshot.
            pace(clock).await;
            let Some((request, queued)) = self.change_records.take(id) else {
                break;
            };
            if let Err(err) = self
                .apply_changes(id, &request, checkout, clock, Pass::Events(queued))
                .await
            {
                tracing::warn!(conversation = %id, error = %err, "could not refresh request changes");
            }
        }
    }

    /// The part of one thread event the request's record needs, or None for an event that
    /// changes nothing. An edit's files are read now; the snapshot waits.
    async fn queued_change(
        &self,
        id: &ConversationId,
        checkout: &Path,
        request: &str,
        event: &ProviderEvent,
    ) -> Result<Option<Queued>> {
        let command = match event {
            ProviderEvent::Command { status, .. } => *status != ItemStatus::InProgress,
            ProviderEvent::ToolCall { name, status, .. } => {
                *status != ItemStatus::InProgress
                    && (crate::digest::is_shell_tool(name) || crate::digest::own_run(name))
            }
            _ => false,
        };
        let key = match event {
            ProviderEvent::FileChanges {
                item_id,
                status: ItemStatus::Completed,
                ..
            } => Some(format!("edit:{item_id}")),
            ProviderEvent::Command { item_id, .. } | ProviderEvent::ToolCall { item_id, .. }
                if command =>
            {
                Some(format!("command:{item_id}"))
            }
            ProviderEvent::TurnCompleted { .. } => None,
            _ => return Ok(None),
        };
        let edits = match event {
            ProviderEvent::FileChanges { changes, .. } => {
                let (git, checkout, changes) =
                    (self.git.clone(), checkout.to_owned(), changes.clone());
                blocking(move || {
                    let repo = git.open(&checkout).map_err(git_error)?;
                    let relative = |given: &str| {
                        let given = Path::new(given);
                        let relative = if given.is_absolute() {
                            given
                                .strip_prefix(repo.root())
                                .ok()
                                .or_else(|| given.strip_prefix(&checkout).ok())
                        } else {
                            Some(given)
                        };
                        relative.and_then(Path::to_str).map(str::to_owned)
                    };
                    let mut edits = Vec::new();
                    for change in changes {
                        let Some(path) = relative(&change.path) else {
                            continue;
                        };
                        let path = path.as_str();
                        // A move: the file is new where it went, and was where it came from
                        // what the diff turns back from it (the same bytes when nothing else
                        // changed).
                        if let Some(moved) = change.moved_to.as_deref().and_then(relative) {
                            let after = repo.checkout_blob(&moved).map_err(git_error)?;
                            // A diff without hunks (a pure rename, +0 −0) changed nothing.
                            let diff = change.diff.as_deref().filter(|diff| diff.contains("@@ "));
                            let before = match diff {
                                None => Before::Known(after.clone().map(Into::into)),
                                Some(diff) if !diff.contains(" more bytes]") => repo
                                    .before_patch(&moved, after.as_ref(), diff)
                                    .map_or(Before::Unverified, |before| {
                                        Before::Known(before.map(Into::into))
                                    }),
                                Some(_) => Before::Unverified,
                            };
                            // Undo moves both ends back or neither: a source it can't verify
                            // holds the file where it went too, so it never removes the one copy.
                            let destination = match before {
                                Before::Unverified => Before::Unverified,
                                Before::Known(_) => Before::Known(None),
                            };
                            edits.push(Evidence {
                                path: path.to_owned(),
                                before,
                                after: repo.checkout_blob(path).map_err(git_error)?.map(Into::into),
                            });
                            edits.push(Evidence {
                                path: moved,
                                before: destination,
                                after: after.map(Into::into),
                            });
                            continue;
                        }
                        let after = repo.checkout_blob(path).map_err(git_error)?;
                        // An original with a secret in it came redacted: it isn't what the
                        // file held, so the diff tells it instead.
                        let original = change.original.as_ref().filter(|original| {
                            !original.contains(brigadier_providers::redact::REDACTED)
                        });
                        let before = if let Some(original) = original {
                            let mode = after.as_ref().map_or("100644", |blob| blob.mode.as_str());
                            Before::Known(Some(
                                repo.store_checkout_blob(original.as_bytes(), mode, path)
                                    .map_err(git_error)?
                                    .into(),
                            ))
                        } else if change.kind == FileChangeKind::Add {
                            Before::Known(None)
                        } else if let Some(before) = change
                            .diff
                            .as_deref()
                            .filter(|diff| !diff.contains(" more bytes]"))
                            .and_then(|diff| repo.before_patch(path, after.as_ref(), diff).ok())
                        {
                            Before::Known(before.map(Into::into))
                        } else {
                            Before::Unverified
                        };
                        edits.push(Evidence {
                            path: path.to_owned(),
                            before,
                            after: after.map(Into::into),
                        });
                    }
                    Ok(edits)
                })
                .await?
            }
            _ => Vec::new(),
        };
        // A shell call's own evidence, when its files were read around it.
        let shell = match event {
            ProviderEvent::Command { item_id, .. } if command => {
                self.change_records
                    .shells
                    .take(id, &ShellKey::Call(item_id.clone()), item_id)
            }
            ProviderEvent::ToolCall {
                item_id,
                name,
                input,
                ..
            } if command => {
                let key = if matches!(
                    crate::digest::bare_tool_name(name),
                    "run" | "run_unsandboxed"
                ) {
                    input
                        .as_deref()
                        .and_then(|input| serde_json::from_str::<serde_json::Value>(input).ok())
                        .and_then(|input| Some(input.get("command")?.as_str()?.to_owned()))
                        .map(ShellKey::Run)
                } else {
                    Some(ShellKey::Call(item_id.clone()))
                };
                key.and_then(|key| self.change_records.shells.take(id, &key, item_id))
            }
            _ => None,
        };
        Ok(Some(Queued {
            request: request.to_owned(),
            key,
            edits,
            command,
            shell,
        }))
    }

    async fn apply_changes(
        &self,
        id: &ConversationId,
        request: &str,
        checkout: &Path,
        clock: &mut SnapshotClock,
        pass: Pass,
    ) -> Result<()> {
        let stored = self.change_record(id, request).await?;
        if matches!(pass, Pass::Start) && stored.is_some() {
            return Ok(());
        }
        if stored.is_none() && !matches!(pass, Pass::Start) {
            return Ok(());
        }
        let mut record = stored.unwrap_or_default();
        if matches!(pass, Pass::Settle) && record.settled {
            return Ok(());
        }
        let mut keys = Vec::new();
        // Each edit, and whether a command reported before it in this snapshot may explain
        // what its file held before it; and each shell command's own evidence, in order.
        let mut edits = Vec::new();
        let mut command = false;
        let mut landing = None;
        let recovering = matches!(pass, Pass::Recover);
        let settling = matches!(pass, Pass::Settle);
        match pass {
            Pass::Events(queued) => {
                let mut any = false;
                for queued in queued {
                    if let Some(key) = queued.key {
                        if record.seen.contains(&key) || keys.contains(&key) {
                            continue;
                        }
                        keys.push(key);
                    }
                    any = true;
                    edits.extend(
                        queued
                            .edits
                            .into_iter()
                            .map(|edit| Step::Edit(edit, command)),
                    );
                    if let Some(shell) = queued.shell {
                        edits.push(Step::Shell(shell, command));
                    }
                    command |= queued.command;
                }
                if !any {
                    return Ok(());
                }
            }
            Pass::Landing(from, tip) => {
                keys.push(format!("landing:{}", tip.0));
                if record.seen.contains(&keys[0]) {
                    return Ok(());
                }
                landing = Some((from, tip));
            }
            _ => {}
        }
        let settled = settling
            || (recovering
                && self
                    .core
                    .board(id)
                    .await?
                    .requests
                    .get(request)
                    .is_some_and(|request| {
                        !matches!(request.state, RequestState::Working | RequestState::Waiting)
                    }));
        clock.at = Some(tokio::time::Instant::now());
        let evidence_seen = !keys.is_empty();
        let previous_paths = record.paths.clone();
        // The branch the session's work goes onto, and whether that branch is the checkout's
        // own: a merge brings in other work only from it or its upstream, and only from the
        // upstream when the checkout's branch is the base itself.
        let base = match self.core.conversation(id).ok().and_then(|c| c.setup) {
            Some(Setup::Session {
                environment: Environment::NewWorktree { base, .. },
                ..
            }) => Some((base, false)),
            Some(Setup::Session {
                environment: Environment::LocalCheckout { branch },
                ..
            }) => Some((branch, true)),
            _ => None,
        };
        let (git, checkout, id_owned, request_owned) = (
            self.git.clone(),
            checkout.to_owned(),
            id.clone(),
            request.to_owned(),
        );
        let (record, changes, changed, cost, rows) = blocking(move || {
            let repo = git.open(&checkout).map_err(git_error)?;
            let started = std::time::Instant::now();
            let latest = repo.snapshot_files().map_err(git_error)?;
            let cost = started.elapsed();
            let previous = if record.latest.is_empty() {
                BTreeMap::new()
            } else {
                repo.file_blobs(&Oid(record.latest.clone()))
                    .map_err(git_error)?
            };
            let current = repo.file_blobs(&latest).map_err(git_error)?;
            // What a merge of the base brought in (`git merge main`) is the base's work, not
            // this request's change (#233): per merge, the files the base changed since the
            // merge base, at their version there (none: deleted). A merge of anything else (a
            // worker's topic branch) is the request's own.
            let bases: Vec<Oid> = base
                .iter()
                .flat_map(|(base, checked_out)| {
                    let local = (!checked_out)
                        .then(|| repo.resolve(&format!("refs/heads/{base}")).ok())
                        .flatten();
                    let upstream = repo.upstream_tip(base).ok().flatten();
                    local.into_iter().chain(upstream)
                })
                .collect();
            let from_base = |commit: &Oid| {
                bases
                    .iter()
                    .any(|tip| repo.ancestor(commit, tip).unwrap_or(false))
            };
            type Changed = BTreeMap<String, Option<FileBlob>>;
            let incoming = |from: &Oid, to: &Oid| -> Vec<Changed> {
                repo.merged_in(from, to)
                    .unwrap_or_default()
                    .iter()
                    .filter(|(_, theirs)| from_base(theirs))
                    .filter_map(|(first, theirs)| {
                        let fork = repo
                            .file_blobs(&repo.merge_base(first, theirs).ok()?)
                            .ok()?;
                        let theirs = repo.file_blobs(theirs).ok()?;
                        Some(
                            fork.keys()
                                .chain(theirs.keys())
                                .filter(|path| fork.get(*path) != theirs.get(*path))
                                .map(|path| (path.clone(), theirs.get(path).cloned()))
                                .collect(),
                        )
                    })
                    .collect()
            };
            // The other side changed `path` to the version it has now, deleted included.
            let brought_in = |sides: &[Changed],
                              path: &str,
                              before: Option<&FileBlob>,
                              after: Option<&FileBlob>| {
                sides.iter().any(|changed| {
                    changed
                        .get(path)
                        .is_some_and(|theirs| theirs.as_ref() == after && after != before)
                })
            };
            let merged = if record.latest.is_empty() {
                Vec::new()
            } else {
                incoming(&Oid(record.latest.clone()), &latest)
            };
            let mut explained = BTreeSet::new();
            // Each shell command's item and the files its evidence gave the record.
            let mut shells = Vec::new();
            // What this pass's steps left in each file they changed.
            let mut made: BTreeMap<String, Option<Blob>> = BTreeMap::new();
            for step in edits {
                let (edit, after_command) = match step {
                    Step::Edit(edit, after_command) => (edit, after_command),
                    Step::Shell(shell, after_command) => {
                        let mut group = BTreeMap::new();
                        let mut together = BTreeSet::new();
                        for (path, before, after) in shell.changes {
                            let previous_blob = previous.get(&path).cloned().map(Blob::from);
                            let current_blob = current.get(&path).cloned().map(Blob::from);
                            together.insert(path.clone());
                            let made_before = made.insert(path.clone(), after.clone());
                            // A file no snapshot keeps (an ignored one, or one a later command
                            // moved on) is no part of the request's changes, though it still
                            // ties this command's files to that later command's. Unless it
                            // held what no step here made: a file of the user's since the last
                            // snapshot that the command moved or removed, which leaves the
                            // command's files as they are with it.
                            if previous_blob.is_none() && current_blob.is_none() {
                                let unseen = record.unseen.insert(path.clone(), after.clone());
                                if before.is_none()
                                    || made_before.as_ref() == Some(&before)
                                    || unseen.as_ref() == Some(&before)
                                {
                                    continue;
                                }
                                if repo.is_ignored(&path).unwrap_or(true) {
                                    record.kept_out.insert(path);
                                } else {
                                    record.own(
                                        &path,
                                        before.clone(),
                                        None,
                                        None,
                                        "shell",
                                        false,
                                        after_command,
                                    );
                                    group.insert(path, (before, None));
                                }
                                continue;
                            }
                            if current_blob == after {
                                explained.insert(path.clone());
                            } else {
                                explained.remove(&path);
                            }
                            record.own(
                                &path,
                                before.clone(),
                                after.clone(),
                                previous_blob,
                                "shell",
                                false,
                                after_command,
                            );
                            group.insert(path, (before, after));
                        }
                        if together.len() > 1 {
                            record.together.push(together);
                        }
                        if !group.is_empty() {
                            shells.push((shell.item, group));
                        }
                        continue;
                    }
                };
                let path = edit.path.as_str();
                let previous_blob = previous.get(path).cloned().map(Blob::from);
                let (before, weak) = match edit.before {
                    // A file rebuilt from a patch (a deleted one) gets a plain file's mode: the
                    // same bytes as the snapshot's are that file, with its mode.
                    Before::Known(Some(before))
                        if previous_blob.as_ref().is_some_and(|previous| {
                            previous.oid == before.oid && previous.mode != before.mode
                        }) =>
                    {
                        (previous_blob.clone(), false)
                    }
                    Before::Known(before) => (before, false),
                    Before::Unverified => (previous_blob.clone(), true),
                };
                made.insert(path.to_owned(), edit.after.clone());
                // The last edit to a file decides whether it explains what the file holds.
                if current.get(path).cloned().map(Blob::from) == edit.after {
                    explained.insert(path.to_owned());
                } else {
                    explained.remove(path);
                }
                record.own(
                    path,
                    before,
                    edit.after,
                    previous_blob,
                    "edit",
                    weak,
                    after_command,
                );
            }
            if let Some((from, tip)) = landing {
                let before = repo.file_blobs(&from).map_err(git_error)?;
                let after = repo.file_blobs(&tip).map_err(git_error)?;
                let landed_in = incoming(&from, &tip);
                for path in before.keys().chain(after.keys()).collect::<BTreeSet<_>>() {
                    if before.get(path) == after.get(path) {
                        continue;
                    }
                    if !record.paths.contains_key(path)
                        && brought_in(&landed_in, path, before.get(path), after.get(path))
                    {
                        if current.get(path) == after.get(path) {
                            explained.insert(path.to_owned());
                        }
                        continue;
                    }
                    record.own(
                        path,
                        before.get(path).cloned().map(Into::into),
                        after.get(path).cloned().map(Into::into),
                        previous.get(path).cloned().map(Into::into),
                        "landing",
                        false,
                        false,
                    );
                    if current.get(path) == after.get(path) {
                        explained.insert(path.to_owned());
                    }
                }
            }
            // Files gone that nothing here explains: the other end of a move a command's words
            // didn't name (`mv old*.txt new.txt`).
            let mut gone = BTreeSet::new();
            if !record.latest.is_empty() {
                for path in previous
                    .keys()
                    .chain(current.keys())
                    .collect::<BTreeSet<_>>()
                {
                    if previous.get(path) == current.get(path) || explained.contains(path) {
                        continue;
                    }
                    // A file this request already changed is reconciled below, whoever
                    // changed it since.
                    if !record.paths.contains_key(path)
                        && brought_in(&merged, path, previous.get(path), current.get(path))
                    {
                        continue;
                    }
                    // A prior edit or landing can already explain this version.
                    if record.paths.get(path).is_some_and(|owned| {
                        owned.last == current.get(path).cloned().map(Into::into)
                    }) {
                        continue;
                    }
                    if !current.contains_key(path) {
                        gone.insert(path.clone());
                    }
                    if command {
                        record.own(
                            path,
                            previous.get(path).cloned().map(Into::into),
                            current.get(path).cloned().map(Into::into),
                            previous.get(path).cloned().map(Into::into),
                            "command",
                            false,
                            false,
                        );
                    } else if let Some(owned) = record.paths.get_mut(path) {
                        owned.held = Some(user_reason(path));
                    }
                }
            }
            // Its content may be in one of this snapshot's commands' files, which Undo would
            // then take away: they stay as they are with it.
            if !gone.is_empty() && !shells.is_empty() {
                let mut together = gone;
                for (_, group) in &shells {
                    together.extend(group.keys().cloned());
                }
                record.together.push(together);
            }
            record.hold_together();
            // A command whose files all stay the thread's shows them as its edits.
            let mut rows = Vec::new();
            for (item, group) in shells {
                let owned = group.keys().all(|path| {
                    record
                        .paths
                        .get(path)
                        .is_some_and(|owned| owned.held.is_none())
                });
                if !owned {
                    continue;
                }
                let mut files = Vec::new();
                for (path, (before, after)) in group {
                    // Only what the row shows: a diff git can't give as text shows none.
                    let diff = repo
                        .file_diff(
                            &path,
                            before.clone().map(Into::into).as_ref(),
                            after.clone().map(Into::into).as_ref(),
                        )
                        .unwrap_or_default();
                    let kind = match (&before, &after) {
                        (None, _) => FileChangeKind::Add,
                        (_, None) => FileChangeKind::Delete,
                        _ => FileChangeKind::Update,
                    };
                    files.push((repo.root().join(&path), kind, diff));
                }
                rows.push((item, files));
            }
            let changed = record.latest != latest.0
                || record.paths != previous_paths
                || record.settled != settled
                || record.commit.is_empty();
            record.seen.extend(keys);
            record.latest = latest.0.clone();
            record.settled = settled;
            if changed {
                let (before, after) = record.trees();
                record.commit = repo
                    .retain_change_record(
                        &prefix(&id_owned, &request_owned),
                        &before,
                        &after,
                        &latest,
                    )
                    .map_err(git_error)?
                    .0;
            }
            let from = repo
                .resolve(&format!("{}^", record.commit))
                .map_err(git_error)?;
            let stat = repo
                .diff_stat(&from, &Oid(record.commit.clone()))
                .map_err(git_error)?;
            let changes = RequestChanges {
                stat: super::landing::diff_stat_of(&stat),
                held: record.held(),
                settled,
            };
            Ok((record, changes, changed, cost, rows))
        })
        .await?;
        clock.slow |= cost > std::time::Duration::from_millis(300);
        if changed || evidence_seen {
            self.save_change_record(id, request, &record).await?;
        }
        if changed || recovering {
            self.core.set_request_changes(id, request, changes).await?;
        }
        for (item, files) in rows {
            self.shell_edit_row(id, request, &item, files).await;
        }
        Ok(())
    }

    /// Old local-checkout requests have landing evidence but no record. Turning that evidence
    /// into a record keeps their Undo in working files, as every local-checkout restore must.
    pub(super) async fn record_legacy_local_changes(
        &self,
        id: &ConversationId,
        request: &crate::work::UserRequest,
        commits: Vec<Oid>,
    ) -> Result<()> {
        let Some((checkout, None)) = self.changes_checkout(id) else {
            return Ok(());
        };
        let lock = self.change_records.lock(id);
        let _guard = lock.lock().await;
        if self.change_record(id, &request.id).await?.is_some() {
            return Ok(());
        }
        let (git, id_owned, request_owned) = (self.git.clone(), id.clone(), request.id.clone());
        let reverted = request.undo.as_ref().is_some_and(|undo| undo.reverted);
        let record = blocking(move || {
            let repo = git.open(&checkout).map_err(git_error)?;
            let (from, to) = repo.changes_of(&commits).map_err(git_error)?;
            let before = repo.file_blobs(&from).map_err(git_error)?;
            let after = repo.file_blobs(&to).map_err(git_error)?;
            let mut record = Record {
                settled: true,
                latest: repo.snapshot_files().map_err(git_error)?.0,
                ..Default::default()
            };
            for path in before.keys().chain(after.keys()).collect::<BTreeSet<_>>() {
                if before.get(path) == after.get(path) {
                    continue;
                }
                record.own(
                    path,
                    before.get(path).cloned().map(Into::into),
                    after.get(path).cloned().map(Into::into),
                    before.get(path).cloned().map(Into::into),
                    "landing",
                    false,
                    false,
                );
            }
            if reverted {
                record.restored = record.paths.keys().cloned().collect();
            }
            let (before, after) = record.trees();
            record.commit = repo
                .retain_change_record(
                    &prefix(&id_owned, &request_owned),
                    &before,
                    &after,
                    &Oid(record.latest.clone()),
                )
                .map_err(git_error)?
                .0;
            Ok(record)
        })
        .await?;
        self.save_change_record(id, &request.id, &record).await
    }

    pub(super) async fn undo_record(
        &self,
        id: &ConversationId,
        request_id: &str,
        reapply: bool,
    ) -> Result<crate::work::UndoOutcome> {
        let lock = self.change_records.lock(id);
        let mut clock = lock.lock().await;
        if let Some((checkout, _)) = self.changes_checkout(id) {
            self.drain_changes(id, &checkout, &mut clock).await;
        }
        let mut record = self
            .change_record(id, request_id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("request changes {request_id}")))?;
        let board = self.core.board(id).await?;
        let request = board
            .requests
            .get(request_id)
            .ok_or_else(|| Error::NotFound(format!("request {request_id}")))?;
        if matches!(request.state, RequestState::Working | RequestState::Waiting) {
            return Err(Error::Invalid("its work is still going".into()));
        }
        let reverted = request.undo.as_ref().is_some_and(|undo| undo.reverted);
        if reverted != reapply {
            return Err(Error::Invalid(
                if reapply {
                    "its changes are not reverted"
                } else {
                    "its changes are already reverted"
                }
                .into(),
            ));
        }
        let (checkout, branch) = self
            .changes_checkout(id)
            .ok_or_else(|| Error::Invalid("the session's checkout is unavailable".into()))?;
        let selected: BTreeMap<_, _> = record
            .paths
            .iter()
            .filter(|(path, owned)| {
                owned.held.is_none()
                    && owned.first != owned.last
                    && (!reapply || record.restored.contains(path))
            })
            .map(|(path, owned)| {
                (
                    path.clone(),
                    if reapply {
                        (owned.first.clone(), owned.last.clone())
                    } else {
                        (owned.last.clone(), owned.first.clone())
                    },
                )
            })
            .collect();
        let verb = if reapply { "Reapply" } else { "Undo" };
        let message = format!("{verb} \"{}\"", request.preview);
        let git = self.git.clone();
        let undone_by_commit = record.committed.clone();
        let (restored, committed, commit) = blocking(move || {
            let repo = git.open(&checkout).map_err(git_error)?;
            let mut refused = Vec::new();
            let mut restores = BTreeMap::new();
            for (path, (expected, target)) in selected {
                let current = repo
                    .checkout_blob(&path)
                    .map_err(git_error)?
                    .map(Blob::from);
                if current == expected {
                    restores.insert(path, target.map(FileBlob::from));
                } else if current != target {
                    refused.push(path);
                }
            }
            if !refused.is_empty() {
                return Err(Error::Invalid(format!(
                    "Later changes touch the same files ({})",
                    refused.join(", ")
                )));
            }
            let restored = restores.keys().cloned().collect();
            let mut commit = None;
            let mut committed_paths = BTreeSet::new();
            if let Some(branch) = branch {
                let head = repo.resolve("HEAD").map_err(git_error)?;
                let head_files = repo.file_blobs(&head).map_err(git_error)?;
                let mut committed = BTreeMap::new();
                for (path, target) in &restores {
                    // After an Undo every file equals HEAD again, committed change or not: only
                    // what Undo committed is committed back.
                    if reapply && !undone_by_commit.contains(path) {
                        continue;
                    }
                    let current = repo.checkout_blob(path).map_err(git_error)?;
                    if current.as_ref() == head_files.get(path) {
                        committed.insert(path.clone(), target.clone());
                    }
                }
                if !committed.is_empty() {
                    match repo
                        .land_file_restore(&branch, &head, &committed, &message)
                        .map_err(git_error)?
                    {
                        brigadier_git::LandOutcome::Landed { new_tip } => {
                            commit = Some(new_tip.0);
                        }
                        brigadier_git::LandOutcome::Blocked(block) => {
                            return Err(Error::Invalid(block.to_string()));
                        }
                    }
                    restores.retain(|path, _| !committed.contains_key(path));
                }
                committed_paths = committed.into_keys().collect();
            }
            repo.restore_file_blobs(&restores).map_err(git_error)?;
            Ok((restored, committed_paths, commit))
        })
        .await?;
        let outcome = crate::work::UndoOutcome {
            restored,
            held: record.held(),
        };
        if !reapply {
            record.restored.clone_from(&outcome.restored);
            record.committed = committed;
        }
        self.save_change_record(id, request_id, &record).await?;
        let mut undo = request.undo.clone().unwrap_or(crate::work::RequestUndo {
            reverted: false,
            commits: Vec::new(),
        });
        undo.reverted = !reapply;
        if let Some(commit) = commit {
            undo.commits.push(commit);
        }
        self.core.set_request_undo(id, request_id, undo).await?;
        self.conv(id)?
            .note(format!(
                "The user {} request \"{}\". Restored: {}. Left as they are: {}.",
                if reapply { "reapplied" } else { "undid" },
                request.preview,
                if outcome.restored.is_empty() {
                    "none".to_owned()
                } else {
                    outcome.restored.join(", ")
                },
                if outcome.held.is_empty() {
                    "none".to_owned()
                } else {
                    outcome
                        .held
                        .iter()
                        .map(|held| held.path.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ))
            .await;
        Ok(outcome)
    }

    /// A background call's tool result is only its handoff. Its eventual end gets a command
    /// refresh too, so files written after that handoff remain counted and held.
    pub(super) async fn background_command_changes(&self, run: &crate::work::BackgroundRun) {
        let event = ProviderEvent::Command {
            kept: None,
            item_id: format!("background-run/{}", run.id),
            command: run.command.clone(),
            cwd: None,
            status: ItemStatus::Completed,
            exit_code: None,
            output: None,
            duration_ms: None,
        };
        self.refresh_changes(
            &run.conversation_id,
            run.request_id.as_deref(),
            Refresh::Event(&event),
        )
        .await;
    }

    /// The thread's event loop never waits for a snapshot: the event's own evidence is read
    /// now and the snapshot runs behind it, in order.
    pub(super) async fn changes_event(&self, id: &ConversationId, event: &ProviderEvent) {
        if !matches!(
            event,
            ProviderEvent::FileChanges { .. }
                | ProviderEvent::Command { .. }
                | ProviderEvent::ToolCall { .. }
                | ProviderEvent::TurnCompleted { .. }
        ) {
            return;
        }
        let Some(request) = (match self.conv(id).ok() {
            Some(conv) => conv.running_request().await,
            None => None,
        }) else {
            return;
        };
        let Some((checkout, _)) = self.changes_checkout(id) else {
            return;
        };
        let queued = match self.queued_change(id, &checkout, &request, event).await {
            Ok(Some(queued)) => queued,
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(conversation = %id, error = %err, "could not read an edit's changes");
                return;
            }
        };
        self.change_records.push(id, queued);
        if matches!(event, ProviderEvent::TurnCompleted { .. }) {
            self.change_records.shells.clear(id);
        }
        let Some(manager) = self.me.upgrade() else {
            return;
        };
        let id = id.clone();
        tokio::spawn(async move {
            let lock = manager.change_records.lock(&id);
            let mut clock = lock.lock().await;
            manager.drain_changes(&id, &checkout, &mut clock).await;
        });
    }

    /// Of `files` uncommitted in `checkout`, those the thread's own edits left as they are now
    /// (#88): going back from the file as it is, each request record that has it has it from
    /// the thread's edits alone, with nobody else's change mixed in, back to the last commit's
    /// version, so a change the user made before the thread's edits keeps the file theirs.
    pub(super) async fn threads_uncommitted(
        &self,
        id: &ConversationId,
        checkout: &Path,
        files: &[String],
    ) -> BTreeSet<String> {
        // Evidence the thread reported but not yet applied would leave its edit the user's.
        if let Some((changes_checkout, _)) = self.changes_checkout(id) {
            let lock = self.change_records.lock(id);
            let mut clock = lock.lock().await;
            self.drain_changes(id, &changes_checkout, &mut clock).await;
        }
        let Ok(board) = self.core.board(id).await else {
            return BTreeSet::new();
        };
        let mut requests: Vec<_> = board.requests.values().collect();
        requests.sort_by_key(|request| request.started_at_ms);
        // Each file's part in each request's record, oldest first.
        let mut chains: BTreeMap<String, Vec<OwnedPath>> = files
            .iter()
            .map(|path| (path.clone(), Vec::new()))
            .collect();
        for request in requests {
            let Ok(Some(record)) = self.change_record(id, &request.id).await else {
                continue;
            };
            for (path, chain) in &mut chains {
                if let Some(owned) = record.paths.get(path) {
                    chain.push(owned.clone());
                }
            }
        }
        let (git, checkout) = (self.git.clone(), checkout.to_owned());
        blocking(move || {
            let repo = git.open(&checkout).map_err(git_error)?;
            let committed = match repo.resolve("HEAD") {
                Ok(head) => repo.file_blobs(&head).map_err(git_error)?,
                Err(_) => BTreeMap::new(),
            };
            Ok(chains
                .into_iter()
                .filter_map(|(path, chain)| {
                    let base = committed.get(&path).cloned().map(Blob::from);
                    let mut expected = repo.checkout_blob(&path).ok()?.map(Blob::from);
                    for owned in chain.iter().rev() {
                        let edits_only = owned.held.is_none()
                            && owned.by.iter().all(|by| by == "edit" || by == "shell");
                        if !edits_only || owned.last != expected {
                            return None;
                        }
                        if owned.first == base {
                            return Some(path);
                        }
                        expected = owned.first.clone();
                    }
                    None
                })
                .collect())
        })
        .await
        .unwrap_or_default()
    }

    pub(super) async fn recover_change_records(&self, id: &ConversationId) {
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        for request in board.requests.values() {
            let Ok(Some(record)) = self.change_record(id, &request.id).await else {
                continue;
            };
            if !record.settled
                || request
                    .changes
                    .as_ref()
                    .is_none_or(|changes| !changes.settled)
            {
                self.refresh_changes(id, Some(&request.id), Refresh::Recover)
                    .await;
            }
        }
    }

    pub(super) async fn removed_worktree_changes(&self, checkout: &Path) {
        let ids: Vec<_> = self
            .core
            .catalog()
            .conversations
            .into_iter()
            .filter_map(|conversation| match conversation.setup {
                Some(Setup::Session {
                    environment:
                        Environment::NewWorktree {
                            path: Some(path), ..
                        },
                    ..
                }) if Path::new(&path) == checkout
                    || Path::new(&path)
                        .canonicalize()
                        .ok()
                        .zip(checkout.canonicalize().ok())
                        .is_some_and(|(a, b)| a == b) =>
                {
                    Some(conversation.id)
                }
                _ => None,
            })
            .collect();
        for id in ids {
            if let Err(err) = self.delete_change_records(&id).await {
                tracing::warn!(conversation = %id, error = %err, "could not remove a worktree's request records");
            }
        }
    }

    pub(super) async fn delete_change_records(&self, id: &ConversationId) -> Result<()> {
        self.delete_pending(id).await?;
        if let Some(Setup::Session { repo, .. }) = self.core.conversation(id)?.setup
            && Path::new(&repo).exists()
        {
            let (git, id) = (self.git.clone(), id.to_string());
            blocking(move || {
                git.open(Path::new(&repo))
                    .and_then(|repo| repo.delete_change_records(&id))
                    .map_err(git_error)
            })
            .await?;
        }
        let board = self.core.board(id).await?;
        self.core
            .store()
            .delete_streams(
                board
                    .requests
                    .keys()
                    .map(|request| stream(id, request))
                    .collect(),
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(oid: &str) -> Option<Blob> {
        Some(Blob {
            oid: oid.into(),
            mode: "100644".into(),
        })
    }

    /// A command's late snapshot claimed a change; the edit's or landing's own evidence of
    /// the same change takes it back, while evidence of another change is still held.
    #[test]
    fn evidence_of_the_same_change_takes_a_file_back_from_a_command() {
        for by in ["edit", "landing"] {
            let mut record = Record::default();
            record.own(
                "a",
                blob("1"),
                blob("2"),
                blob("1"),
                "command",
                false,
                false,
            );
            assert!(record.paths["a"].held.is_some());
            record.own("a", blob("1"), blob("2"), blob("2"), by, false, false);
            assert_eq!(record.paths["a"].held, None, "{by}");
            assert_eq!(record.paths["a"].by, BTreeSet::from([by.to_owned()]));

            let mut record = Record::default();
            record.own(
                "a",
                blob("1"),
                blob("2"),
                blob("1"),
                "command",
                false,
                false,
            );
            record.own("a", blob("1"), blob("3"), blob("2"), by, false, false);
            assert!(record.paths["a"].held.is_some(), "{by}");
        }
    }

    /// A held file holds every file a command changed with it, and on through commands that
    /// share a file, whatever order their groups came in.
    #[test]
    fn a_held_file_holds_every_command_group_it_reaches() {
        let mut record = Record::default();
        for path in ["a", "c", "d"] {
            record.own(path, blob("1"), blob("2"), blob("1"), "shell", false, false);
        }
        record.paths.get_mut("a").unwrap().held = Some(user_reason("a"));
        record.together = vec![
            BTreeSet::from(["b".to_owned(), "c".to_owned()]),
            BTreeSet::from(["a".to_owned(), "b".to_owned()]),
        ];
        record.hold_together();
        assert!(record.paths["c"].held.as_ref().unwrap().contains("with a"));
        assert_eq!(record.paths["d"].held, None);
    }
}
