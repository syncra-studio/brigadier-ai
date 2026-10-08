//! What the session's thread read and searched (THREAD-PLAN.md Q8 lever 1, phase 2 step 5),
//! for the context pack phase 3 writes for each worker.
//!
//! The thread's CLI tells it as [`ProviderEvent::Looked`], in the same words for both vendors
//! (Claude's `Read`, `Grep` and `Glob`, and the simple reads and searches of its `Bash`
//! command lines; Codex's commands as it parsed them). A worker's never
//! counts: only the thread's own conversation pump collects them. Its searches in the code
//! index (`code_search`, `code_refs`) count too. They are held for the turn and recorded once
//! when it ends (or when the CLI goes, or every [`BATCH_MAX`] calls in a long turn) as one
//! [`DomainEvent::ThreadLooked`] on the conversation's stream, so they survive a restart, a
//! rebirth and a vendor fallback like the rest of its board. Paths are made absolute (against
//! the CLI's working directory), resolved as the file system does and checked against the
//! thread's effective workspace; a search keeps the files it found that exist.

use std::collections::{HashMap, VecDeque};
use std::path::{Component, Path, PathBuf};

use brigadier_providers::policy::real_path;
use brigadier_providers::{FileSearch, LineRange, ProviderEvent};

use super::SessionManager;
use super::conversation::ConvLive;
use crate::Result;
use crate::model::{ConversationId, DomainEvent};
use crate::work::{ThreadRead, ThreadSearch};

/// Distinct files kept per conversation: the most recently read.
pub(crate) const FILES_KEPT: usize = 200;
/// Searches kept per conversation: the most recent.
pub(crate) const SEARCHES_KEPT: usize = 100;
/// Files kept per search.
pub(crate) const HITS_STORED: usize = 50;
/// Tool calls held in a turn before they are recorded anyway.
pub(super) const BATCH_MAX: usize = 64;

/// What the thread read and searched, folded from its [`DomainEvent::ThreadLooked`] events
/// (part of the conversation's board).
#[derive(Debug, Default, Clone)]
pub(crate) struct ReadLog {
    files: HashMap<String, ReadFile>,
    /// Oldest first.
    searches: VecDeque<ThreadSearch>,
    /// Counts reads, for recency.
    clock: u64,
    dropped_files: u64,
    dropped_searches: u64,
}

/// A file the thread read, its reads merged.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ReadFile {
    /// Absolute, symlinks resolved.
    pub path: String,
    /// It read all of it at least once.
    pub whole: bool,
    /// Otherwise the lines it read: sorted, overlapping and adjacent ranges merged.
    pub lines: Vec<LineRange>,
    /// Outside the thread's workspace when last read.
    pub outside: bool,
    pub(crate) recency: u64,
}

/// What [`SessionManager::thread_reads`] returns.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ThreadReads {
    /// Most recently read first.
    pub files: Vec<ReadFile>,
    /// Most recent first; a search made again (the same pattern, scope and filter) counts
    /// once, as its latest, with the files every time found.
    pub searches: Vec<ThreadSearch>,
    /// Files forgotten past [`FILES_KEPT`] (the least recently read).
    pub dropped_files: u64,
    /// Searches forgotten past [`SEARCHES_KEPT`] (the oldest).
    pub dropped_searches: u64,
}

impl ReadLog {
    pub(crate) fn apply(&mut self, reads: &[ThreadRead], searches: &[ThreadSearch]) {
        for read in reads {
            self.clock += 1;
            let file = self
                .files
                .entry(read.path.clone())
                .or_insert_with(|| ReadFile {
                    path: read.path.clone(),
                    whole: false,
                    lines: Vec::new(),
                    outside: read.outside,
                    recency: 0,
                });
            file.recency = self.clock;
            file.outside = read.outside;
            match read.lines {
                _ if file.whole => {}
                None => {
                    file.whole = true;
                    file.lines.clear();
                }
                Some(range) => merge(&mut file.lines, range),
            }
        }
        while self.files.len() > FILES_KEPT {
            let Some(oldest) = self
                .files
                .values()
                .min_by_key(|file| file.recency)
                .map(|file| file.path.clone())
            else {
                break;
            };
            self.files.remove(&oldest);
            self.dropped_files += 1;
        }
        for search in searches {
            let mut search = search.clone();
            // The same search again (or by another tool): the latest, with every file found.
            if let Some(at) = self
                .searches
                .iter()
                .position(|known| same_search(known, &search))
                && let Some(known) = self.searches.remove(at)
            {
                let mut more = known.more_hits;
                for hit in known.hits {
                    if search.hits.contains(&hit) {
                        continue;
                    }
                    if search.hits.len() < HITS_STORED {
                        search.hits.push(hit);
                    } else {
                        more += 1;
                    }
                }
                search.more_hits = search.more_hits.max(more);
            }
            self.searches.push_back(search);
        }
        while self.searches.len() > SEARCHES_KEPT {
            self.searches.pop_front();
            self.dropped_searches += 1;
        }
    }

    pub(crate) fn snapshot(&self) -> ThreadReads {
        let mut files: Vec<ReadFile> = self.files.values().cloned().collect();
        files.sort_by_key(|file| std::cmp::Reverse(file.recency));
        ThreadReads {
            files,
            searches: self.searches.iter().rev().cloned().collect(),
            dropped_files: self.dropped_files,
            dropped_searches: self.dropped_searches,
        }
    }
}

fn same_search(a: &ThreadSearch, b: &ThreadSearch) -> bool {
    a.kind == b.kind && a.pattern == b.pattern && a.scope == b.scope && a.glob == b.glob
}

/// Adds `range` to sorted, merged `lines`.
fn merge(lines: &mut Vec<LineRange>, range: LineRange) {
    lines.push(range);
    lines.sort_by_key(|range| range.start);
    let mut merged: Vec<LineRange> = Vec::with_capacity(lines.len());
    for range in lines.drain(..) {
        match merged.last_mut() {
            Some(last) if last.end.is_none_or(|end| range.start <= end + 1) => {
                last.end = match (last.end, range.end) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    _ => None,
                };
            }
            _ => merged.push(range),
        }
    }
    *lines = merged;
}

impl SessionManager {
    /// What the session's thread read and searched, for the context pack each worker starts
    /// with (THREAD-PLAN.md Q8 lever 1, phase 3): per file the union of the lines it read
    /// (or the whole file), most recently read first; then its searches, most recent first,
    /// with the files they found. Only the thread's own tool calls count, never a worker's.
    ///
    /// Bounded per conversation: the [`FILES_KEPT`] most recently read files, the
    /// [`SEARCHES_KEPT`] latest searches and [`HITS_STORED`] files per search. What goes past
    /// those is counted (`dropped_files`, `dropped_searches`, a search's `more_hits`), never
    /// dropped silently. Files outside the thread's workspace are kept, marked `outside`.
    #[cfg_attr(not(all(test, unix)), allow(dead_code))]
    pub(crate) async fn thread_reads(&self, id: &ConversationId) -> Result<ThreadReads> {
        Ok(self.core.board(id).await?.thread_reads.snapshot())
    }

    /// [`Self::thread_reads`] with what the thread's running turn read so far and isn't
    /// recorded yet, for a worker its turn starts (phase 3's context pack). The pump records
    /// what is held as soon as the thread starts a `delegate_task` call, before the call
    /// reaches Brigadier, so a read made just before it is in the board either way.
    pub(crate) async fn thread_reads_now(&self, id: &ConversationId) -> Result<ThreadReads> {
        let mut log = (*self.core.board(id).await?.thread_reads).clone();
        let held = match self.conv(id) {
            Ok(conv) => conv.peek_looked().await,
            Err(_) => Vec::new(),
        };
        if !held.is_empty() {
            let workspace = self.recorded_workspace(id).map(|workspace| workspace.path);
            let scratch = self.owned_dir("orch", &id.0);
            let (reads, searches) =
                super::blocking(move || Ok(resolve(held, workspace.as_deref(), &scratch))).await?;
            log.apply(&reads, &searches);
        }
        Ok(log.snapshot())
    }

    /// Holds what one of the thread's tool calls read or searched until its turn ends.
    pub(super) async fn hold_looked(&self, conv: &ConvLive, event: ProviderEvent) {
        if let Some(batch) = conv.hold_looked(event).await {
            self.record_looked(&conv.id, batch).await;
        }
    }

    /// Records what the thread read and searched since the last time: its turn ended, or its
    /// CLI went.
    pub(super) async fn record_held_looked(&self, conv: &ConvLive) {
        let batch = conv.take_looked().await;
        self.record_looked(&conv.id, batch).await;
    }

    /// A search of the thread's in the code index (`code_search`, `code_refs`): it searched
    /// the workspace, and found the files named (relative to it).
    pub(super) async fn looked_in_index(&self, id: &ConversationId, search: FileSearch) {
        let Ok(conv) = self.conv(id) else {
            return;
        };
        let cwd = self
            .recorded_workspace(id)
            .map(|workspace| workspace.path.display().to_string());
        let event = ProviderEvent::Looked {
            item_id: String::new(),
            cwd,
            reads: Vec::new(),
            searches: vec![search],
        };
        self.hold_looked(&conv, event).await;
    }

    /// Records `batch`, its paths resolved: relative ones against the CLI's working directory
    /// (the thread's own folder when it didn't say), checked against the thread's workspace.
    async fn record_looked(&self, id: &ConversationId, batch: Vec<ProviderEvent>) {
        if batch.is_empty() {
            return;
        }
        let workspace = self.recorded_workspace(id).map(|workspace| workspace.path);
        let scratch = self.owned_dir("orch", &id.0);
        let resolved =
            super::blocking(move || Ok(resolve(batch, workspace.as_deref(), &scratch))).await;
        let (reads, searches) = match resolved {
            Ok(resolved) => resolved,
            Err(err) => {
                tracing::warn!(conversation = %id, error = %err, "could not resolve what the thread read");
                return;
            }
        };
        if reads.is_empty() && searches.is_empty() {
            return;
        }
        if let Err(err) = self
            .core
            .record_conversation(
                id,
                vec![DomainEvent::ThreadLooked {
                    conversation_id: id.clone(),
                    reads,
                    searches,
                }],
            )
            .await
        {
            tracing::warn!(conversation = %id, error = %err, "could not record what the thread read");
        }
    }
}

/// The records of `batch`, paths resolved (blocking: it looks at the disk).
fn resolve(
    batch: Vec<ProviderEvent>,
    workspace: Option<&Path>,
    scratch: &Path,
) -> (Vec<ThreadRead>, Vec<ThreadSearch>) {
    let workspace = workspace.and_then(real_path);
    let outside = |path: &Path| {
        workspace
            .as_ref()
            .is_none_or(|workspace| !path.starts_with(workspace))
    };
    let mut reads = Vec::new();
    let mut searches = Vec::new();
    for event in batch {
        let ProviderEvent::Looked {
            cwd,
            reads: read,
            searches: searched,
            ..
        } = event
        else {
            continue;
        };
        let cwd = cwd.map_or_else(|| scratch.to_owned(), PathBuf::from);
        for read in read {
            let path = absolute(&cwd, &read.path);
            reads.push(ThreadRead {
                outside: outside(&path),
                path: path.display().to_string(),
                lines: read.lines,
            });
        }
        for search in searched {
            let scope = absolute(&cwd, search.scope.as_deref().unwrap_or("."));
            // A listing names files under its folder; a search, under the working directory
            // or (`rg x dir` with one folder) the folder itself.
            let bases = [cwd.clone(), scope.clone()];
            let mut hits: Vec<String> = Vec::new();
            let mut more = 0u32;
            for hit in &search.hits {
                let Some(found) = bases
                    .iter()
                    .map(|base| absolute(base, hit))
                    .find(|path| path.exists())
                else {
                    continue;
                };
                let found = found.display().to_string();
                if hits.contains(&found) {
                    continue;
                }
                if hits.len() < HITS_STORED {
                    hits.push(found);
                } else {
                    more += 1;
                }
            }
            searches.push(ThreadSearch {
                kind: search.kind,
                pattern: search.pattern,
                outside: outside(&scope),
                scope: scope.display().to_string(),
                glob: search.glob,
                hits,
                more_hits: more,
            });
        }
    }
    (reads, searches)
}

/// `path` made absolute against `base`, `.` and `..` resolved, symlinks resolved as far as
/// the file system has it: by the file system itself for a path that exists, as a `..` after
/// a symlink leaves the symlink's target, which resolving it by name gets wrong.
fn absolute(base: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    let joined = if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    };
    if let Ok(real) = joined.canonicalize() {
        return real;
    }
    let lexical = lexical(&joined);
    real_path(&lexical).unwrap_or(lexical)
}

/// `.` and `..` resolved without looking at the disk.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            part => out.push(part),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use brigadier_providers::{FileRead, FileSearch, SearchKind};

    use super::*;

    fn range(start: u64, end: Option<u64>) -> LineRange {
        LineRange { start, end }
    }

    fn read(path: &str, lines: Option<LineRange>) -> ThreadRead {
        ThreadRead {
            path: path.into(),
            lines,
            outside: false,
        }
    }

    #[test]
    fn a_file_s_reads_merge_into_their_union() {
        let mut log = ReadLog::default();
        log.apply(
            &[
                read("/r/a.rs", Some(range(35, Some(44)))),
                read("/r/a.rs", Some(range(45, Some(50)))),
                read("/r/a.rs", Some(range(1, Some(5)))),
                read("/r/a.rs", Some(range(100, None))),
                read("/r/a.rs", Some(range(120, Some(130)))),
                read("/r/b.rs", None),
                read("/r/b.rs", Some(range(3, Some(4)))),
            ],
            &[],
        );
        let reads = log.snapshot();
        assert_eq!(reads.files.len(), 2);
        let b = &reads.files[0];
        assert!(b.whole && b.lines.is_empty() && b.path == "/r/b.rs");
        let a = &reads.files[1];
        assert!(!a.whole);
        assert_eq!(
            a.lines,
            [range(1, Some(5)), range(35, Some(50)), range(100, None)]
        );
    }

    #[test]
    fn the_log_keeps_the_latest_and_counts_what_it_forgets() {
        let mut log = ReadLog::default();
        let reads: Vec<ThreadRead> = (0..FILES_KEPT + 3)
            .map(|n| read(&format!("/r/{n}.rs"), None))
            .collect();
        let search = |n: usize| ThreadSearch {
            kind: SearchKind::Content,
            pattern: Some(format!("p{n}")),
            scope: "/r".into(),
            glob: None,
            hits: Vec::new(),
            more_hits: 0,
            outside: false,
        };
        let mut searches: Vec<ThreadSearch> = (0..SEARCHES_KEPT + 2).map(search).collect();
        searches[5].hits = vec!["/r/a.rs".into()];
        log.apply(&reads, &searches);
        // Read again: the latest now, so it stays. A search made again keeps what both found.
        let mut again = search(5);
        again.hits = vec!["/r/b.rs".into(), "/r/a.rs".into()];
        log.apply(&[read("/r/3.rs", None)], &[again]);
        log.apply(&[read("/r/new.rs", None)], &[]);
        let snapshot = log.snapshot();
        assert_eq!(snapshot.files.len(), FILES_KEPT);
        assert_eq!(snapshot.dropped_files, 4);
        assert_eq!(snapshot.files[0].path, "/r/new.rs");
        assert_eq!(snapshot.files[1].path, "/r/3.rs");
        assert!(!snapshot.files.iter().any(|file| file.path == "/r/4.rs"));
        assert_eq!(snapshot.searches.len(), SEARCHES_KEPT);
        assert_eq!(snapshot.dropped_searches, 2);
        assert_eq!(snapshot.searches[0].pattern.as_deref(), Some("p5"));
        assert_eq!(snapshot.searches[0].hits, ["/r/b.rs", "/r/a.rs"]);
        assert_eq!(
            snapshot
                .searches
                .iter()
                .filter(|known| known.pattern.as_deref() == Some("p5"))
                .count(),
            1
        );
    }

    #[test]
    fn paths_are_resolved_and_checked_against_the_workspace() {
        let dir = std::env::temp_dir().join(format!("brigadier-reads-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let root = real_path(&dir).unwrap();
        let workspace = root.join("repo");
        std::fs::create_dir_all(workspace.join("src")).unwrap();
        std::fs::write(workspace.join("src/a.rs"), "a").unwrap();
        std::fs::write(workspace.join("b.rs"), "b").unwrap();
        std::fs::write(root.join("outside.txt"), "o").unwrap();
        let event = ProviderEvent::Looked {
            item_id: "1".into(),
            cwd: Some(workspace.display().to_string()),
            reads: vec![
                FileRead {
                    path: "src/../src/a.rs".into(),
                    lines: Some(range(1, Some(2))),
                },
                FileRead {
                    path: root.join("outside.txt").display().to_string(),
                    lines: None,
                },
            ],
            searches: vec![
                FileSearch {
                    kind: SearchKind::Content,
                    pattern: Some("x".into()),
                    scope: None,
                    glob: None,
                    hits: vec!["./b.rs".into(), "src/a.rs".into(), "gone.rs".into()],
                },
                // `ls src` names files under its folder.
                FileSearch {
                    kind: SearchKind::Files,
                    pattern: None,
                    scope: Some("src".into()),
                    glob: None,
                    hits: vec!["a.rs".into()],
                },
            ],
        };
        let (reads, searches) = resolve(vec![event], Some(&workspace), &root);
        let at = |path: &Path| path.display().to_string();
        assert_eq!(
            reads,
            [
                ThreadRead {
                    path: at(&workspace.join("src/a.rs")),
                    lines: Some(range(1, Some(2))),
                    outside: false
                },
                ThreadRead {
                    path: at(&root.join("outside.txt")),
                    lines: None,
                    outside: true
                },
            ]
        );
        assert_eq!(
            searches[0].hits,
            [at(&workspace.join("b.rs")), at(&workspace.join("src/a.rs"))]
        );
        assert_eq!(searches[0].scope, at(&workspace));
        assert_eq!(searches[1].hits, [at(&workspace.join("src/a.rs"))]);
        assert!(!searches[1].outside);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `link/..` is the parent of the link's target, not the folder the link is in.
    #[cfg(unix)]
    #[test]
    fn a_parent_after_a_symlink_is_the_targets_parent() {
        let dir = std::env::temp_dir().join(format!("brigadier-reads-{}", uuid::Uuid::new_v4()));
        let root = {
            std::fs::create_dir_all(&dir).unwrap();
            real_path(&dir).unwrap()
        };
        let workspace = root.join("repo");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("c.rs"), "inside").unwrap();
        std::fs::create_dir_all(root.join("elsewhere/dir")).unwrap();
        std::fs::write(root.join("elsewhere/c.rs"), "outside").unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere/dir"), workspace.join("link")).unwrap();
        let event = ProviderEvent::Looked {
            item_id: "1".into(),
            cwd: Some(workspace.display().to_string()),
            reads: vec![FileRead {
                path: "link/../c.rs".into(),
                lines: None,
            }],
            searches: Vec::new(),
        };
        let (reads, _) = resolve(vec![event], Some(&workspace), &root);
        assert_eq!(
            reads,
            [ThreadRead {
                path: root.join("elsewhere/c.rs").display().to_string(),
                lines: None,
                outside: true
            }]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
