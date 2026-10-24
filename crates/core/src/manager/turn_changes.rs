//! Changes cards are git diffs of retained turn trees. No tool-call ownership inference.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use brigadier_git::Oid;
use brigadier_providers::{Artifact, ProviderEvent};
use brigadier_store::{NewEvent, StreamPage};
use serde::{Deserialize, Serialize};

use super::{SessionManager, blocking, git_error};
use crate::model::{ConversationId, Environment, Setup};
use crate::work::{
    FileReview, PendingChanges, PendingFile, PendingHunk, RequestChanges, RequestUndo, TurnRevert,
    UndoOutcome,
};
use crate::{Error, Result};

#[derive(Default)]
pub(super) struct TurnSnapshots {
    locks: Mutex<HashMap<ConversationId, Arc<tokio::sync::Mutex<()>>>>,
    active: Mutex<HashMap<ConversationId, String>>,
}

impl TurnSnapshots {
    fn lock(&self, id: &ConversationId) -> Arc<tokio::sync::Mutex<()>> {
        self.locks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(id.clone())
            .or_default()
            .clone()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Turn {
    pub number: u64,
}

fn stream(id: &ConversationId, request: &str) -> String {
    format!("snapshot:{id}:{request}")
}

/// Archive leaves this owner intact. Delete disposes it even if a worktree must stay.
pub(super) fn snapshots_owner(id: &ConversationId) -> String {
    format!("session-snapshots:{id}")
}

impl SessionManager {
    pub(super) fn changes_checkout(
        &self,
        id: &ConversationId,
    ) -> Option<(PathBuf, Option<String>)> {
        match self.core.conversation(id).ok()?.setup? {
            Setup::Session {
                repo,
                environment: Environment::LocalCheckout { branch },
                ..
            } => Some((repo.into(), Some(branch))),
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

    pub(super) async fn snapshot_turn_record(
        &self,
        id: &ConversationId,
        request: &str,
    ) -> Result<Option<Turn>> {
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

    /// Called immediately before the CLI receives the user's turn.
    pub(super) async fn snapshot_start(&self, id: &ConversationId, request: Option<&str>) {
        let Some(request) = request else { return };
        let request = self
            .turn_snapshots
            .active
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(id.clone())
            .or_insert_with(|| request.to_owned())
            .clone();
        if let Err(err) = self.snapshot_boundary(id, &request, false).await {
            tracing::warn!(conversation = %id, error = %err, "could not snapshot turn start");
        }
    }

    /// The pump calls this only for the CLI's own turn end or exit, before clearing its request.
    pub(super) async fn snapshot_event(&self, id: &ConversationId, event: &ProviderEvent) {
        if !matches!(
            event,
            ProviderEvent::TurnCompleted { .. } | ProviderEvent::Exited { .. }
        ) {
            return;
        }
        self.snapshot_finish(id).await;
    }

    /// EOF is also a boundary when the provider could not emit a final event.
    pub(super) async fn snapshot_finish(&self, id: &ConversationId) {
        let request = self
            .turn_snapshots
            .active
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .cloned();
        let Some(request) = request else { return };
        match self.snapshot_boundary(id, &request, true).await {
            Ok(()) => {
                self.turn_snapshots
                    .active
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(id);
            }
            Err(err) => {
                tracing::warn!(conversation = %id, error = %err, "could not snapshot turn end")
            }
        }
    }

    async fn snapshot_boundary(&self, id: &ConversationId, request: &str, end: bool) -> Result<()> {
        let Some((checkout, _)) = self.changes_checkout(id) else {
            return Ok(());
        };
        let lock = self.turn_snapshots.lock(id);
        let _guard = lock.lock().await;
        let turn = match self.snapshot_turn_record(id, request).await? {
            Some(turn) => turn,
            None if end => return Ok(()),
            None => {
                let counter = format!("snapshots:{id}");
                let previous = self
                    .core
                    .store()
                    .read_stream(
                        counter.clone(),
                        StreamPage {
                            limit: 1,
                            ..Default::default()
                        },
                    )
                    .await?;
                let number = previous
                    .first()
                    .map(|event| {
                        serde_json::from_str::<Turn>(event.payload.get())
                            .map(|turn| turn.number + 1)
                            .map_err(|source| Error::Corrupt {
                                seq: event.seq,
                                source,
                            })
                    })
                    .transpose()?
                    .unwrap_or(1);
                let turn = Turn { number };
                self.core
                    .store()
                    .append(vec![
                        NewEvent::new(counter, "snapshot.turn", crate::now_ms(), &turn)?,
                        NewEvent::new(
                            stream(id, request),
                            "snapshot.turn",
                            crate::now_ms(),
                            &turn,
                        )?,
                    ])
                    .await?;
                turn
            }
        };
        let conversation = self.core.conversation(id)?;
        let Some(Setup::Session { repo, .. }) = conversation.setup else {
            return Ok(());
        };
        self.runtime
            .ledger()
            .record(
                &snapshots_owner(id),
                Artifact::GitRefs {
                    repo,
                    namespace: brigadier_git::snapshot::session_refs(&id.0).map_err(git_error)?,
                },
            )
            .await?;
        let (git, session, number) = (self.git.clone(), id.0.clone(), turn.number);
        let diff = blocking(move || {
            let repo = git.open(&checkout).map_err(git_error)?;
            repo.snapshot_turn(&session, number, end)
                .map_err(git_error)?;
            if end {
                repo.turn_diff(&session, number)
                    .map(Some)
                    .map_err(git_error)
            } else {
                Ok(None)
            }
        })
        .await?;
        if let Some(diff) = diff {
            let merged = diff
                .merged
                .into_iter()
                .map(|group| crate::work::MergedTurnChanges {
                    branch: group.branch,
                    tip: group.tip.0,
                    merge_commit: group.merge_commit.0,
                    stat: super::diff_stat_of(&group.stat),
                })
                .collect();
            self.core
                .set_request_changes(
                    id,
                    request,
                    RequestChanges {
                        stat: super::diff_stat_of(&diff.stat),
                        held: Vec::new(),
                        settled: true,
                        merged,
                    },
                )
                .await?;
        }
        Ok(())
    }

    /// Read the confirmation's complete list of files changed since this turn ended.
    pub async fn prepare_turn_revert(
        &self,
        id: ConversationId,
        request: String,
    ) -> Result<TurnRevert> {
        let (checkout, _) = self
            .changes_checkout(&id)
            .ok_or_else(|| Error::Invalid("the session's checkout is unavailable".into()))?;
        let turn = self
            .snapshot_turn_record(&id, &request)
            .await?
            .ok_or_else(|| Error::NotFound("turn snapshot".into()))?;
        let (git, session) = (self.git.clone(), id.0.clone());
        blocking(move || {
            let repo = git.open(&checkout).map_err(git_error)?;
            let tree = repo.snapshot_tree().map_err(git_error)?;
            let end = repo
                .turn_tree(&session, turn.number, true)
                .map_err(git_error)?;
            Ok(TurnRevert {
                expected_tree: tree.0.clone(),
                changed_after: repo.diff_scope(&end, &tree).map_err(git_error)?.1,
            })
        })
        .await
    }

    /// Restore the start tree only after the confirmation names later files and supplies the
    /// exact tree the user reviewed. A newer edit invalidates that confirmation.
    pub async fn revert_turn(
        &self,
        id: ConversationId,
        request: String,
        expected_tree: String,
    ) -> Result<UndoOutcome> {
        let lock = self.turn_snapshots.lock(&id);
        let _guard = lock.lock().await;
        if let Ok(conv) = self.conv(&id)
            && conv.is_busy().await
        {
            return Err(Error::Invalid("the session is still working".into()));
        }
        let (checkout, _) = self
            .changes_checkout(&id)
            .ok_or_else(|| Error::Invalid("the session's checkout is unavailable".into()))?;
        let turn = self
            .snapshot_turn_record(&id, &request)
            .await?
            .ok_or_else(|| Error::NotFound("turn snapshot".into()))?;
        let (git, session) = (self.git.clone(), id.0.clone());
        let restored = blocking(move || {
            git.open(&checkout)
                .map_err(git_error)?
                .revert_turn(&session, turn.number, &Oid(expected_tree))
                .map_err(git_error)
        })
        .await?;
        self.core
            .set_request_undo(
                &id,
                &request,
                RequestUndo {
                    reverted: true,
                    commits: Vec::new(),
                },
            )
            .await?;
        Ok(UndoOutcome {
            restored,
            held: Vec::new(),
        })
    }

    /// Only explicit pending review entries wait; snapshot turns alone create no review queue.
    pub async fn pending_changes(
        &self,
        id: ConversationId,
        path: Option<String>,
    ) -> Result<PendingChanges> {
        let Some((checkout, _)) = self.changes_checkout(&id) else {
            return Ok(PendingChanges::default());
        };
        let board = self.core.board(&id).await?;
        let mut requests: Vec<_> = board.requests.values().collect();
        requests.sort_by_key(|request| request.started_at_ms);
        let mut files = BTreeMap::new();
        for request in requests {
            if request.undo.as_ref().is_some_and(|undo| undo.reverted) {
                continue;
            }
            let Some(changes) = &request.changes else {
                continue;
            };
            if !changes.settled {
                continue;
            }
            let Some(turn) = self.snapshot_turn_record(&id, &request.id).await? else {
                continue;
            };
            for file in &changes.stat.files {
                if path.as_ref().is_some_and(|path| *path != file.path) {
                    continue;
                }
                files.remove(&file.path);
                if request.review.get(&file.path) != Some(&FileReview::Pending) {
                    continue;
                }
                files.insert(
                    file.path.clone(),
                    (request.id.clone(), turn.number, file.clone()),
                );
            }
        }
        let (git, session) = (self.git.clone(), id.0.clone());
        blocking(move || {
            let repo = git.open(&checkout).map_err(git_error)?;
            let mut pending = PendingChanges::default();
            for (path, (request_id, number, stat)) in files {
                let (base, current) = repo
                    .turn_review_trees(&session, number)
                    .map_err(git_error)?;
                let start = repo.file_blobs(&base).map_err(git_error)?;
                let end = repo.file_blobs(&current).map_err(git_error)?;
                let hunks = repo
                    .file_hunks(&path, start.get(&path), end.get(&path))
                    .map_err(git_error)?
                    .unwrap_or_default();
                let hunks = hunks
                    .into_iter()
                    .map(|hunk| PendingHunk {
                        id: format!(
                            "{}:{}:{}:{}:{}:{}",
                            start.get(&path).map_or("none", |blob| blob.oid.as_str()),
                            end.get(&path).map_or("none", |blob| blob.oid.as_str()),
                            hunk.old_start,
                            hunk.old_lines,
                            hunk.new_start,
                            hunk.new_lines
                        ),
                        old_start: hunk.old_start,
                        old_lines: hunk.old_lines,
                        new_start: hunk.new_start,
                        new_lines: hunk.new_lines,
                    })
                    .collect::<Vec<_>>();
                let text = |blob: Option<&brigadier_git::FileBlob>| -> Result<Option<String>> {
                    blob.map(|blob| {
                        repo.blob_bytes(blob)
                            .map_err(git_error)
                            .map(|bytes| String::from_utf8(bytes).ok())
                    })
                    .transpose()
                    .map(Option::flatten)
                };
                pending.files.push(PendingFile {
                    path: path.clone(),
                    request_id,
                    base: text(start.get(&path))?,
                    current: text(end.get(&path))?,
                    text: !stat.binary,
                    insertions: hunks.iter().map(|hunk| hunk.new_lines).sum(),
                    deletions: hunks.iter().map(|hunk| hunk.old_lines).sum(),
                    hunks,
                    held: None,
                });
            }
            Ok(pending)
        })
        .await
    }

    /// Keeping advances the file's review base in git; the card's snapshots stay immutable.
    pub async fn keep_changes(
        &self,
        id: ConversationId,
        path: Option<String>,
        hunks: Option<Vec<String>>,
    ) -> Result<()> {
        self.review_snapshot_files(id, path, hunks, true)
            .await
            .map(|_| ())
    }

    /// File Undo restores only the selected snapshot lines, while the file still equals the
    /// review's expected tree. A later user edit is left untouched and named.
    pub async fn undo_pending_changes(
        &self,
        id: ConversationId,
        path: Option<String>,
        hunks: Option<Vec<String>>,
    ) -> Result<UndoOutcome> {
        self.review_snapshot_files(id, path, hunks, false).await
    }

    async fn review_snapshot_files(
        &self,
        id: ConversationId,
        path: Option<String>,
        hunks: Option<Vec<String>>,
        keep: bool,
    ) -> Result<UndoOutcome> {
        if hunks.is_some() && path.is_none() {
            return Err(Error::Invalid("changes need their file".into()));
        }
        let lock = self.turn_snapshots.lock(&id);
        let _guard = lock.lock().await;
        let pending = self.pending_changes(id.clone(), path).await?;
        let (checkout, _) = self
            .changes_checkout(&id)
            .ok_or_else(|| Error::Invalid("the session's checkout is unavailable".into()))?;
        let mut outcome = UndoOutcome::default();
        for file in pending.files {
            let turn = self
                .snapshot_turn_record(&id, &file.request_id)
                .await?
                .ok_or_else(|| Error::NotFound("turn snapshot".into()))?;
            let (git, checkout, session, path, chosen) = (
                self.git.clone(),
                checkout.clone(),
                id.0.clone(),
                file.path.clone(),
                hunks.clone(),
            );
            let (restored, complete, kept_some) = blocking(move || {
                let repo = git.open(&checkout).map_err(git_error)?;
                let (base_tree, end_tree) = repo
                    .turn_review_trees(&session, turn.number)
                    .map_err(git_error)?;
                let base = repo.file_blobs(&base_tree).map_err(git_error)?;
                let end = repo.file_blobs(&end_tree).map_err(git_error)?;
                // A renamed file's old path, which Undo of the whole file brings back.
                let source = match (&chosen, base.contains_key(&path)) {
                    (None, false) if !keep => repo
                        .rename_source(&base_tree, &end_tree, &path)
                        .map_err(git_error)?,
                    _ => None,
                };
                if !keep
                    && std::iter::once(&path)
                        .chain(&source)
                        .map(|each| {
                            Ok(repo.checkout_blob(each).map_err(git_error)?.as_ref()
                                != end.get(each))
                        })
                        .collect::<Result<Vec<_>>>()?
                        .contains(&true)
                {
                    return Ok((false, false, false));
                }
                let target = match chosen {
                    None => {
                        if keep {
                            end.get(&path).cloned()
                        } else {
                            base.get(&path).cloned()
                        }
                    }
                    Some(ids) => {
                        if ids.is_empty() {
                            return Err(Error::Invalid("choose a change to review".into()));
                        }
                        let picked = ids
                            .iter()
                            .map(|id| {
                                file.hunks
                                    .iter()
                                    .find(|hunk| &hunk.id == id)
                                    .ok_or_else(|| {
                                        Error::Invalid("the file changed; review it again".into())
                                    })
                            })
                            .collect::<Result<Vec<_>>>()?;
                        let bytes = |blob: Option<&brigadier_git::FileBlob>| {
                            blob.map_or(Ok(Vec::new()), |blob| {
                                repo.blob_bytes(blob).map_err(git_error)
                            })
                        };
                        let old = bytes(base.get(&path))?;
                        let new = bytes(end.get(&path))?;
                        let old_lines: Vec<_> =
                            old.split_inclusive(|byte| *byte == b'\n').collect();
                        let new_lines: Vec<_> =
                            new.split_inclusive(|byte| *byte == b'\n').collect();
                        let mut result = if keep {
                            old_lines.clone()
                        } else {
                            new_lines.clone()
                        };
                        let mut picked = picked;
                        picked.sort_by_key(|hunk| {
                            std::cmp::Reverse(if keep { hunk.old_start } else { hunk.new_start })
                        });
                        picked.dedup_by(|a, b| a.id == b.id);
                        let span = |start: u32, count: u32| {
                            let from = if count == 0 { start } else { start - 1 } as usize;
                            from..from + count as usize
                        };
                        for hunk in picked {
                            let old_range = span(hunk.old_start, hunk.old_lines);
                            let new_range = span(hunk.new_start, hunk.new_lines);
                            if keep {
                                result.splice(old_range, new_lines[new_range].iter().copied());
                            } else {
                                result.splice(new_range, old_lines[old_range].iter().copied());
                            }
                        }
                        let bytes = result.concat();
                        if bytes == old {
                            base.get(&path).cloned()
                        } else if bytes == new {
                            end.get(&path).cloned()
                        } else {
                            Some(
                                repo.store_file_blob(
                                    &bytes,
                                    end.get(&path)
                                        .or(base.get(&path))
                                        .map_or("100644", |blob| blob.mode.as_str()),
                                )
                                .map_err(git_error)?,
                            )
                        }
                    }
                };
                if !keep {
                    let mut restore = BTreeMap::from([(path.clone(), target.clone())]);
                    if let Some(source) = &source {
                        restore.insert(source.clone(), base.get(source).cloned());
                    }
                    repo.restore_file_blobs(&restore).map_err(git_error)?;
                }
                repo.review_turn_file(&session, turn.number, keep, &path, target.clone())
                    .map_err(git_error)?;
                if let Some(source) = &source {
                    repo.review_turn_file(
                        &session,
                        turn.number,
                        keep,
                        source,
                        base.get(source).cloned(),
                    )
                    .map_err(git_error)?;
                }
                let complete = target.as_ref()
                    == if keep {
                        end.get(&path)
                    } else {
                        base.get(&path)
                    };
                let original = repo
                    .file_blobs(
                        &repo
                            .turn_tree(&session, turn.number, false)
                            .map_err(git_error)?,
                    )
                    .map_err(git_error)?;
                Ok((true, complete, base.get(&path) != original.get(&path)))
            })
            .await?;
            if restored {
                if !keep {
                    outcome.restored.push(file.path.clone());
                }
                let board = self.core.board(&id).await?;
                let mut review = board.requests[&file.request_id].review.clone();
                review.insert(
                    file.path,
                    if !complete {
                        FileReview::Pending
                    } else if keep || kept_some {
                        FileReview::Kept
                    } else {
                        FileReview::Undone
                    },
                );
                self.core
                    .set_request_review(&id, &file.request_id, review)
                    .await?;
            } else {
                outcome.held.push(crate::work::HeldPath {
                    path: file.path,
                    reason: "Changed after this turn".into(),
                });
            }
        }
        Ok(outcome)
    }

    pub(super) async fn delete_turn_snapshots(&self, id: &ConversationId) -> Result<()> {
        let leftovers = self.runtime.ledger().dispose(&snapshots_owner(id)).await;
        if !leftovers.is_clean() {
            return Err(Error::Invalid(format!(
                "could not remove turn snapshot refs: {leftovers:?}"
            )));
        }
        let board = self.core.board(id).await?;
        let mut streams = vec![format!("snapshots:{id}")];
        streams.extend(board.requests.keys().map(|request| stream(id, request)));
        self.core.store().delete_streams(streams).await?;
        Ok(())
    }
}
