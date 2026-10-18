//! The changes agents made that the user hasn't reviewed yet, kept or undone change by change
//! (the file review and the turn diff card's Undo All and Keep All).
//!
//! Each file has a review base: the file as the user last accepted it, first what it held
//! before the earliest request still pending in it changed it. A request joins once it
//! settles. A file's pending changes are the diff from its base to the file in the checkout
//! now, so they build up across requests until the user keeps or undoes them. Keep moves the
//! base on and never touches a file. Undo puts lines back in the checkout and lands as a
//! request's Undo does: uncommitted, or by a commit on the session's branch when the change
//! was committed. A merge takes its requests' changes out of the review: the merge is their
//! record now, and its Revert takes them back.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use brigadier_git::{FileBlob, FileHunk, Repo};
use brigadier_store::{NewEvent, StreamPage};
use serde::{Deserialize, Serialize};

use super::request_changes::Blob;
use super::{SessionManager, blocking, git_error};
use crate::model::ConversationId;
use crate::work::{FileReview, HeldPath, PendingChanges, PendingFile, PendingHunk, UndoOutcome};
use crate::{Error, Result};

/// A file whose text is longer than this is kept or undone whole, never shown.
const TEXT_LIMIT: usize = 1 << 20;

/// The refusal when a Keep or Undo names a change the file no longer has.
const CHANGED: &str = "The file changed since; review it again";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Pending {
    files: BTreeMap<String, PendingPath>,
    /// The requests whose changes joined: each joins once, so a file it changed that the user
    /// kept or undid never comes back.
    absorbed: BTreeSet<String>,
    /// Where the review left each request's files.
    reviews: BTreeMap<String, BTreeMap<String, FileReview>>,
    /// The requests a merge took, each with the session's head that merged.
    #[serde(default)]
    merged: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingPath {
    base: Option<Blob>,
    /// The requests whose changes are in it, oldest first.
    requests: Vec<String>,
    /// Why Undo leaves it: someone else changed it during a request too.
    held: Option<String>,
    /// What the agents left in it last, or the user's Undo of some of its changes: a whole
    /// file's Undo from anything else would take the user's own later edits with it.
    last: Option<Blob>,
    /// The requests whose changes in it the user kept some of.
    kept: BTreeSet<String>,
}

/// Which changes a Keep or Undo takes: every file, one file, or some of one file's changes.
#[derive(Debug, Clone)]
enum ReviewPick {
    All,
    File(String),
    Hunks(String, Vec<String>),
}

impl ReviewPick {
    fn new(path: Option<String>, hunks: Option<Vec<String>>) -> Result<Self> {
        match (path, hunks) {
            (None, None) => Ok(Self::All),
            (Some(path), None) => Ok(Self::File(path)),
            (Some(path), Some(hunks)) if !hunks.is_empty() => Ok(Self::Hunks(path, hunks)),
            (_, Some(_)) => Err(Error::Invalid("changes need their file".into())),
        }
    }

    fn path(&self) -> Option<&str> {
        match self {
            Self::All => None,
            Self::File(path) | Self::Hunks(path, _) => Some(path),
        }
    }
}

fn stream(id: &ConversationId) -> String {
    format!("pending:{id}")
}
/// Where the review bases are kept from Git's garbage collection, with the request records.
fn bases_ref(id: &ConversationId) -> String {
    format!("refs/brigadier/changes/{id}/review")
}
fn edited_reason(path: &str) -> String {
    format!("You changed {path} after the agents did, so undo its changes one at a time.")
}

/// A file's lines, each with its line ending, as git counts them.
fn lines(bytes: &[u8]) -> Vec<&[u8]> {
    bytes.split_inclusive(|byte| *byte == b'\n').collect()
}

/// Where a hunk's lines sit, from a start and count in git's numbering.
fn span(start: u32, count: u32) -> std::ops::Range<usize> {
    let from = if count == 0 { start } else { start - 1 } as usize;
    from..from + count as usize
}

fn hunk_id(hunk: &FileHunk, old: &[&[u8]], new: &[&[u8]]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&hunk.old_start.to_le_bytes());
    for (side, lines) in [(b'-', old), (b'+', new)] {
        hasher.update(&[side]);
        hasher.update(&(lines.len() as u64).to_le_bytes());
        for line in lines {
            hasher.update(&(line.len() as u64).to_le_bytes());
            hasher.update(line);
        }
    }
    hasher.finalize().to_hex()[..16].to_owned()
}

/// One pending file read from the checkout: its versions, their bytes and its hunks.
struct Read {
    base: Option<FileBlob>,
    current: Option<FileBlob>,
    old: Vec<u8>,
    new: Vec<u8>,
    /// `None`: git calls the change binary.
    hunks: Option<Vec<(String, FileHunk)>>,
}

impl Read {
    fn of(repo: &Repo, path: &str, base: Option<&Blob>) -> Result<Self> {
        let base = base.cloned().map(FileBlob::from);
        let current = repo.checkout_blob(path).map_err(git_error)?;
        let bytes = |blob: &Option<FileBlob>| {
            blob.as_ref()
                .map_or(Ok(Vec::new()), |blob| repo.blob_bytes(blob))
                .map_err(git_error)
        };
        let (old, new) = (bytes(&base)?, bytes(&current)?);
        let hunks = if base == current {
            Some(Vec::new())
        } else {
            repo.file_hunks(path, base.as_ref(), current.as_ref())
                .map_err(git_error)?
        };
        let hunks = hunks.map(|hunks| {
            let (old_lines, new_lines) = (lines(&old), lines(&new));
            hunks
                .into_iter()
                .map(|hunk| {
                    let id = hunk_id(
                        &hunk,
                        &old_lines[span(hunk.old_start, hunk.old_lines)],
                        &new_lines[span(hunk.new_start, hunk.new_lines)],
                    );
                    (id, hunk)
                })
                .collect()
        });
        Ok(Self {
            base,
            current,
            old,
            new,
            hunks,
        })
    }

    fn text(&self) -> Option<(&str, &str)> {
        self.hunks.as_ref()?;
        if self.old.len() > TEXT_LIMIT || self.new.len() > TEXT_LIMIT {
            return None;
        }
        Some((
            std::str::from_utf8(&self.old).ok()?,
            std::str::from_utf8(&self.new).ok()?,
        ))
    }

    /// The hunks `ids` name, last first, or the refusal when one is gone.
    fn picked(&self, ids: &[String]) -> Result<Vec<FileHunk>> {
        let hunks = self
            .hunks
            .as_ref()
            .filter(|_| self.text().is_some())
            .ok_or_else(|| Error::Invalid(CHANGED.into()))?;
        let mut picked = ids
            .iter()
            .map(|id| {
                hunks
                    .iter()
                    .find(|(each, _)| each == id)
                    .map(|(_, hunk)| *hunk)
                    .ok_or_else(|| Error::Invalid(CHANGED.into()))
            })
            .collect::<Result<Vec<_>>>()?;
        picked.sort_by_key(|hunk| std::cmp::Reverse(hunk.old_start));
        picked.dedup();
        Ok(picked)
    }

    /// The base with these hunks' new lines in it.
    fn kept(&self, hunks: &[FileHunk]) -> Vec<u8> {
        let (mut old, new) = (lines(&self.old), lines(&self.new));
        for hunk in hunks {
            old.splice(
                span(hunk.old_start, hunk.old_lines),
                new[span(hunk.new_start, hunk.new_lines)].iter().copied(),
            );
        }
        old.concat()
    }

    /// The file now with these hunks' old lines back.
    fn undone(&self, hunks: &[FileHunk]) -> Vec<u8> {
        let (old, mut new) = (lines(&self.old), lines(&self.new));
        let mut hunks = hunks.to_vec();
        hunks.sort_by_key(|hunk| std::cmp::Reverse(hunk.new_start));
        for hunk in hunks {
            new.splice(
                span(hunk.new_start, hunk.new_lines),
                old[span(hunk.old_start, hunk.old_lines)].iter().copied(),
            );
        }
        new.concat()
    }

    /// A blob for these bytes: one of its own versions (no file, for an added file all undone
    /// or a deleted one all kept) when they are that version's.
    fn blob(&self, repo: &Repo, bytes: &[u8]) -> Result<Option<FileBlob>> {
        if bytes == self.new.as_slice() {
            return Ok(self.current.clone());
        }
        if bytes == self.old.as_slice() {
            return Ok(self.base.clone());
        }
        let mode = self
            .current
            .as_ref()
            .or(self.base.as_ref())
            .map_or("100644", |blob| blob.mode.as_str());
        repo.store_file_blob(bytes, mode)
            .map(Some)
            .map_err(git_error)
    }
}

/// Puts files back in the checkout as a request's Undo does: a file whose change was committed
/// (it equals HEAD's) by a commit on the session's branch, every other one in the working files.
fn land(
    repo: &Repo,
    branch: Option<&str>,
    mut restores: BTreeMap<String, Option<FileBlob>>,
    message: &str,
) -> Result<()> {
    if let Some(branch) = branch {
        let head = repo.resolve("HEAD").map_err(git_error)?;
        let head_files = repo.file_blobs(&head).map_err(git_error)?;
        let mut committed = BTreeMap::new();
        for (path, target) in &restores {
            let current = repo.checkout_blob(path).map_err(git_error)?;
            if current.as_ref() == head_files.get(path) {
                committed.insert(path.clone(), target.clone());
            }
        }
        if !committed.is_empty() {
            match repo
                .land_file_restore(branch, &head, &committed, message)
                .map_err(git_error)?
            {
                brigadier_git::LandOutcome::Landed { .. } => {}
                brigadier_git::LandOutcome::Blocked(block) => {
                    return Err(Error::Invalid(block.to_string()));
                }
            }
            restores.retain(|path, _| !committed.contains_key(path));
        }
    }
    repo.restore_file_blobs(&restores).map_err(git_error)
}

impl Pending {
    /// Drops the files back at their base and settles where the review left every request's
    /// files; returns the requests whose files moved.
    fn settle(&mut self, now: &BTreeMap<String, Option<Blob>>) -> BTreeSet<String> {
        let mut touched = BTreeSet::new();
        self.files.retain(|path, file| {
            let done = now.get(path).is_some_and(|current| *current == file.base);
            let latest = file.requests.len().saturating_sub(1);
            for (index, request) in file.requests.iter().enumerate() {
                let state = match (done, file.kept.contains(request)) {
                    (true, true) => FileReview::Kept,
                    (true, false) => FileReview::Undone,
                    (false, _) if index == latest => FileReview::Pending,
                    (false, _) => FileReview::Later,
                };
                let review = self.reviews.entry(request.clone()).or_default();
                if review.insert(path.clone(), state) != Some(state) {
                    touched.insert(request.clone());
                }
            }
            !done
        });
        touched
    }
}

impl SessionManager {
    async fn pending(&self, id: &ConversationId) -> Result<Pending> {
        let events = self
            .core
            .store()
            .read_stream(
                stream(id),
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
            .map(Option::unwrap_or_default)
    }

    /// Reads where every pending file is now, drops the ones back at their base, stores the
    /// record and tells each request's card.
    async fn save_pending(
        &self,
        id: &ConversationId,
        checkout: &Path,
        mut pending: Pending,
        changed: bool,
        mut touched: BTreeSet<String>,
    ) -> Result<Pending> {
        let (git, checkout, bases, id_owned) = (
            self.git.clone(),
            checkout.to_owned(),
            pending
                .files
                .iter()
                .map(|(path, file)| (path.clone(), file.base.clone().map(FileBlob::from)))
                .collect::<BTreeMap<_, _>>(),
            id.clone(),
        );
        let now = blocking(move || {
            let repo = git.open(&checkout).map_err(git_error)?;
            // A base Keep made of some changes is in no other record.
            if changed {
                repo.retain_files(&bases_ref(&id_owned), "bases", &bases)
                    .map_err(git_error)?;
            }
            bases
                .into_keys()
                .map(|path| {
                    let blob = repo.checkout_blob(&path).map_err(git_error)?;
                    Ok((path, blob.map(Blob::from)))
                })
                .collect::<Result<BTreeMap<_, _>>>()
        })
        .await?;
        touched.extend(pending.settle(&now));
        if changed || !touched.is_empty() {
            self.core
                .store()
                .append(vec![NewEvent::new(
                    stream(id),
                    "request.pending_changes",
                    crate::now_ms(),
                    &pending,
                )?])
                .await?;
        }
        for request in touched {
            let review = pending.reviews.get(&request).cloned().unwrap_or_default();
            if let Err(err) = self.core.set_request_review(id, &request, review).await {
                tracing::debug!(conversation = %id, error = %err, "could not show a request's review");
            }
        }
        Ok(pending)
    }

    /// A settled request's changes join the review, under the session's change lock.
    pub(super) async fn absorb_pending(
        &self,
        id: &ConversationId,
        request: &str,
        checkout: &Path,
    ) -> Result<()> {
        let Some(record) = self.change_record(id, request).await? else {
            return Ok(());
        };
        let mut pending = self.pending(id).await?;
        if !pending.absorbed.insert(request.to_owned()) {
            return Ok(());
        }
        // A request a merge took before it settled: what the merge has is merged, and only what
        // it changed since waits, from the merged file (even a file it put back as it was).
        let merged = match pending.merged.get(request) {
            Some(head) => Some(self.merged_files(checkout, head).await),
            None => None,
        };
        let mut touched = BTreeSet::new();
        for (path, first, last, held) in record.touched() {
            let changed = first != last;
            let mut first = first.cloned();
            if let Some(files) = &merged {
                let at_merge = files.as_ref().map(|files| files.get(path).cloned());
                if at_merge
                    .as_ref()
                    .is_none_or(|at_merge| at_merge.as_ref() == last)
                {
                    if changed {
                        pending
                            .reviews
                            .entry(request.to_owned())
                            .or_default()
                            .insert(path.to_owned(), FileReview::Merged);
                        touched.insert(request.to_owned());
                    }
                    continue;
                }
                first = at_merge.flatten();
            } else if !changed {
                continue;
            }
            let file = pending
                .files
                .entry(path.to_owned())
                .or_insert_with(|| PendingPath {
                    base: first,
                    requests: Vec::new(),
                    held: None,
                    last: None,
                    kept: BTreeSet::new(),
                });
            file.requests.retain(|each| each != request);
            file.requests.push(request.to_owned());
            file.last = last.cloned();
            if let Some(held) = held {
                file.held = Some(held.to_owned());
            }
        }
        self.save_pending(id, checkout, pending, true, touched)
            .await?;
        Ok(())
    }

    /// The files in `head`, the session's head a merge took, by path; None when the checkout
    /// can't read it.
    async fn merged_files(&self, checkout: &Path, head: &str) -> Option<BTreeMap<String, Blob>> {
        let (git, checkout, head) = (self.git.clone(), checkout.to_owned(), head.to_owned());
        blocking(move || {
            let repo = git.open(&checkout).map_err(git_error)?;
            let head = repo.resolve(&head).map_err(git_error)?;
            let files = repo.file_blobs(&head).map_err(git_error)?;
            Ok(files
                .into_iter()
                .map(|(path, blob)| (path, Blob::from(blob)))
                .collect())
        })
        .await
        .inspect_err(|err| tracing::debug!(error = %err, "could not read a merged head"))
        .ok()
    }

    /// A merge took `requests`' work at `head`, what the session's checkout has as merged (its
    /// head that merged, or the merge it moved to): every waiting file the checkout has as
    /// merged leaves the review (Undo and Keep are done with; the merge's Revert takes them
    /// back), and a file of theirs with more than the merge waits with only that, from the
    /// merged file.
    pub(super) async fn merged_pending(
        &self,
        id: &ConversationId,
        requests: &[String],
        head: &str,
    ) {
        if let Err(err) = self.merged_pending_inner(id, requests, head).await {
            tracing::warn!(conversation = %id, error = %err, "could not take merged changes out of the review");
        }
    }

    async fn merged_pending_inner(
        &self,
        id: &ConversationId,
        requests: &[String],
        head: &str,
    ) -> Result<()> {
        let Some((checkout, _)) = self.changes_checkout(id) else {
            return Ok(());
        };
        let lock = self.change_records.lock(id);
        let mut clock = lock.lock().await;
        self.drain_changes(id, &checkout, &mut clock).await;
        let mut pending = self.pending(id).await?;
        for request in requests {
            pending.merged.insert(request.clone(), head.to_owned());
        }
        let mut touched = BTreeSet::new();
        // Every file that waits: one an earlier merge left (uncommitted then) may be in this one.
        let paths: Vec<String> = pending.files.keys().cloned().collect();
        if !paths.is_empty()
            && let Some(files) = self.merged_files(&checkout, head).await
        {
            let (git, root) = (self.git.clone(), checkout.clone());
            let now = blocking(move || {
                let repo = git.open(&root).map_err(git_error)?;
                paths
                    .into_iter()
                    .map(|path| {
                        let blob = repo.checkout_blob(&path).map_err(git_error)?;
                        Ok((path, blob.map(Blob::from)))
                    })
                    .collect::<Result<BTreeMap<_, _>>>()
            })
            .await?;
            for (path, now) in now {
                let at_merge = files.get(&path).cloned();
                if now != at_merge {
                    // What the checkout has beyond the merge (an edit left uncommitted, or made
                    // since) still waits, from the merged file; a file the user kept some of
                    // keeps its base, so what they kept stays kept.
                    if let Some(file) = pending.files.get_mut(&path)
                        && file.requests.iter().any(|each| requests.contains(each))
                        && file.kept.is_empty()
                    {
                        file.base = at_merge;
                    }
                    continue;
                }
                let Some(file) = pending.files.remove(&path) else {
                    continue;
                };
                for request in file.requests {
                    pending
                        .reviews
                        .entry(request.clone())
                        .or_default()
                        .insert(path.clone(), FileReview::Merged);
                    touched.insert(request);
                }
            }
        }
        self.save_pending(id, &checkout, pending, true, touched)
            .await?;
        Ok(())
    }

    fn review_checkout(&self, id: &ConversationId) -> Result<(PathBuf, Option<String>)> {
        self.changes_checkout(id)
            .ok_or_else(|| Error::Invalid("the session's checkout is unavailable".into()))
    }

    /// What waits for the user's review: every file, or with `path` that one.
    pub async fn pending_changes(
        &self,
        id: ConversationId,
        path: Option<String>,
    ) -> Result<PendingChanges> {
        let Some((checkout, _)) = self.changes_checkout(&id) else {
            return Ok(PendingChanges::default());
        };
        let lock = self.change_records.lock(&id);
        let _clock = lock.lock().await;
        let pending = self.pending(&id).await?;
        let pending = self
            .save_pending(&id, &checkout, pending, false, BTreeSet::new())
            .await?;
        let (git, files) = (self.git.clone(), pending.files);
        blocking(move || {
            let repo = git.open(&checkout).map_err(git_error)?;
            let mut out = Vec::new();
            for (name, file) in files {
                if path.as_ref().is_some_and(|path| *path != name) {
                    continue;
                }
                let read = Read::of(&repo, &name, file.base.as_ref())?;
                let text = read.text();
                let hunks: Vec<_> = read
                    .hunks
                    .iter()
                    .flatten()
                    .map(|(id, hunk)| PendingHunk {
                        id: id.clone(),
                        old_start: hunk.old_start,
                        old_lines: hunk.old_lines,
                        new_start: hunk.new_start,
                        new_lines: hunk.new_lines,
                    })
                    .collect();
                out.push(PendingFile {
                    request_id: file.requests.last().cloned().unwrap_or_default(),
                    base: text
                        .filter(|_| read.base.is_some())
                        .map(|(old, _)| old.to_owned()),
                    current: text
                        .filter(|_| read.current.is_some())
                        .map(|(_, new)| new.to_owned()),
                    text: text.is_some(),
                    insertions: hunks.iter().map(|hunk| hunk.new_lines).sum(),
                    deletions: hunks.iter().map(|hunk| hunk.old_lines).sum(),
                    hunks,
                    held: file.held,
                    path: name,
                });
            }
            Ok(PendingChanges { files: out })
        })
        .await
    }

    /// The user keeps changes: every file's, one file's (`path`) or some of its (`hunks`). Their
    /// file's review base moves on, and no file changes.
    pub async fn keep_changes(
        &self,
        id: ConversationId,
        path: Option<String>,
        hunks: Option<Vec<String>>,
    ) -> Result<()> {
        let pick = ReviewPick::new(path, hunks)?;
        let (checkout, _) = self.review_checkout(&id)?;
        let lock = self.change_records.lock(&id);
        let mut clock = lock.lock().await;
        self.drain_changes(&id, &checkout, &mut clock).await;
        let mut pending = self.pending(&id).await?;
        let files = Self::picked_files(&pending, &pick)?;
        let (git, root) = (self.git.clone(), checkout.clone());
        let bases = blocking(move || {
            let repo = git.open(&root).map_err(git_error)?;
            files
                .into_iter()
                .map(|(path, base)| {
                    let read = Read::of(&repo, &path, base.as_ref())?;
                    let base = match &pick {
                        ReviewPick::Hunks(_, ids) => {
                            let hunks = read.picked(ids)?;
                            read.blob(&repo, &read.kept(&hunks))?
                        }
                        _ => read.current.clone(),
                    };
                    Ok((path, base.map(Blob::from)))
                })
                .collect::<Result<Vec<_>>>()
        })
        .await?;
        for (path, base) in bases {
            if let Some(file) = pending.files.get_mut(&path) {
                file.base = base;
                file.kept.extend(file.requests.iter().cloned());
            }
        }
        self.save_pending(&id, &checkout, pending, true, BTreeSet::new())
            .await?;
        Ok(())
    }

    /// The user undoes changes, picked as Keep picks them: their lines go back in the checkout,
    /// landed as a request's Undo lands. A file someone else changed too is left as it is.
    pub async fn undo_pending_changes(
        &self,
        id: ConversationId,
        path: Option<String>,
        hunks: Option<Vec<String>>,
    ) -> Result<UndoOutcome> {
        let pick = ReviewPick::new(path, hunks)?;
        let (checkout, branch) = self.review_checkout(&id)?;
        let lock = self.change_records.lock(&id);
        let mut clock = lock.lock().await;
        self.drain_changes(&id, &checkout, &mut clock).await;
        let mut pending = self.pending(&id).await?;
        let mut held = Vec::new();
        let mut files = Vec::new();
        for (path, base) in Self::picked_files(&pending, &pick)? {
            let file = &pending.files[&path];
            match file.held.clone() {
                Some(reason) if pick.path().is_some() => return Err(Error::Invalid(reason)),
                Some(reason) => held.push(HeldPath { path, reason }),
                None => files.push((path, base, file.last.clone().map(FileBlob::from))),
            }
        }
        let some = matches!(pick, ReviewPick::Hunks(..));
        let (git, root) = (self.git.clone(), checkout.clone());
        let (restores, edited) = blocking(move || {
            let repo = git.open(&root).map_err(git_error)?;
            let mut restores = BTreeMap::new();
            let mut edited = Vec::new();
            for (path, base, last) in files {
                let read = Read::of(&repo, &path, base.as_ref())?;
                let target = match &pick {
                    ReviewPick::Hunks(_, ids) => {
                        let hunks = read.picked(ids)?;
                        read.blob(&repo, &read.undone(&hunks))?
                    }
                    // Changes the user hasn't seen are the user's own: the whole file waits.
                    _ if read.current != last => {
                        if pick.path().is_some() {
                            return Err(Error::Invalid(edited_reason(&path)));
                        }
                        edited.push(HeldPath {
                            reason: edited_reason(&path),
                            path,
                        });
                        continue;
                    }
                    _ => read.base.clone(),
                };
                if target != read.current {
                    restores.insert(path, target);
                }
            }
            if !restores.is_empty() {
                land(
                    &repo,
                    branch.as_deref(),
                    restores.clone(),
                    &format!(
                        "Undo changes in {}",
                        restores.keys().cloned().collect::<Vec<_>>().join(", ")
                    ),
                )?;
            }
            Ok((restores, edited))
        })
        .await?;
        held.extend(edited);
        let restored = restores.keys().cloned().collect::<Vec<_>>();
        for (path, target) in restores {
            if let Some(file) = pending.files.get_mut(&path) {
                file.last = target.map(Blob::from);
            }
        }
        self.save_pending(&id, &checkout, pending, true, BTreeSet::new())
            .await?;
        if !restored.is_empty() {
            self.conv(&id)?
                .note(format!(
                    "The user undid {} in {}.",
                    if some {
                        "some of the changes"
                    } else {
                        "the changes"
                    },
                    restored.join(", ")
                ))
                .await;
        }
        Ok(UndoOutcome { restored, held })
    }

    /// The files a pick names, each with its review base.
    fn picked_files(pending: &Pending, pick: &ReviewPick) -> Result<Vec<(String, Option<Blob>)>> {
        match pick.path() {
            None => Ok(pending
                .files
                .iter()
                .map(|(path, file)| (path.clone(), file.base.clone()))
                .collect()),
            Some(path) => pending
                .files
                .get(path)
                .map(|file| vec![(path.to_owned(), file.base.clone())])
                .ok_or_else(|| Error::Invalid(CHANGED.into())),
        }
    }

    pub(super) async fn delete_pending(&self, id: &ConversationId) -> Result<()> {
        self.core.store().delete_streams(vec![stream(id)]).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(old: &str, new: &str, hunks: &[(u32, u32, u32, u32)]) -> Read {
        Read {
            base: None,
            current: None,
            old: old.as_bytes().to_vec(),
            new: new.as_bytes().to_vec(),
            hunks: Some(
                hunks
                    .iter()
                    .map(|&(old_start, old_lines, new_start, new_lines)| {
                        (
                            String::new(),
                            FileHunk {
                                old_start,
                                old_lines,
                                new_start,
                                new_lines,
                            },
                        )
                    })
                    .collect(),
            ),
        }
    }

    fn hunks(read: &Read) -> Vec<FileHunk> {
        read.hunks.iter().flatten().map(|(_, hunk)| *hunk).collect()
    }

    /// Keeping or undoing one run of lines moves only those lines: an added, a removed and a
    /// changed run, each alone.
    #[test]
    fn pending_changes_keep_and_undo_move_only_their_lines() {
        let read = read(
            "one\ntwo\nthree\nfour\n",
            "zero\none\nthree\nFOUR\n",
            &[(0, 0, 1, 1), (2, 1, 2, 0), (4, 1, 4, 1)],
        );
        let [added, removed, changed] = hunks(&read)[..] else {
            unreachable!()
        };
        assert_eq!(read.kept(&[added]), b"zero\none\ntwo\nthree\nfour\n");
        assert_eq!(read.kept(&[removed]), b"one\nthree\nfour\n");
        assert_eq!(
            read.kept(&[changed, added]),
            b"zero\none\ntwo\nthree\nFOUR\n"
        );
        assert_eq!(read.undone(&[added]), b"one\nthree\nFOUR\n");
        assert_eq!(read.undone(&[removed]), b"zero\none\ntwo\nthree\nFOUR\n");
        assert_eq!(read.undone(&[added, changed]), b"one\nthree\nfour\n");
        assert_eq!(
            read.undone(&[added, removed, changed]),
            b"one\ntwo\nthree\nfour\n"
        );
    }

    /// A last line without its line ending stays one line, kept or undone.
    #[test]
    fn pending_changes_keep_a_last_line_without_its_ending() {
        let read = read("a\nb", "a\nc\n", &[(2, 1, 2, 1)]);
        let hunks = hunks(&read);
        assert_eq!(read.kept(&hunks), b"a\nc\n");
        assert_eq!(read.undone(&hunks), b"a\nb");
    }

    #[test]
    fn pending_changes_settle_says_where_each_requests_file_is() {
        let blob = |oid: &str| {
            Some(Blob::from(FileBlob {
                oid: oid.into(),
                mode: "100644".into(),
            }))
        };
        let mut pending = Pending::default();
        pending.files.insert(
            "a".into(),
            PendingPath {
                base: blob("1"),
                requests: vec!["r1".into(), "r2".into()],
                held: None,
                last: blob("2"),
                kept: BTreeSet::new(),
            },
        );
        let touched = pending.settle(&BTreeMap::from([("a".into(), blob("2"))]));
        assert_eq!(touched, BTreeSet::from(["r1".into(), "r2".into()]));
        assert_eq!(pending.reviews["r1"]["a"], FileReview::Later);
        assert_eq!(pending.reviews["r2"]["a"], FileReview::Pending);
        assert!(
            pending
                .settle(&BTreeMap::from([("a".into(), blob("2"))]))
                .is_empty()
        );
        pending.files.get_mut("a").unwrap().kept.insert("r1".into());
        pending.files.get_mut("a").unwrap().base = blob("2");
        pending.settle(&BTreeMap::from([("a".into(), blob("2"))]));
        assert!(pending.files.is_empty());
        assert_eq!(pending.reviews["r1"]["a"], FileReview::Kept);
        assert_eq!(pending.reviews["r2"]["a"], FileReview::Undone);
    }
}
