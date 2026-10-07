//! Static code index: the file tree, tree-sitter symbols and cross-references, package
//! manifests, scripts and the service map of one repository (PLAN.md §6 Phase 4). No model is
//! involved.
//!
//! One [`CodeIndex`] per project repository, stored in its own SQLite file under Brigadier's
//! data directory (never in the repository). It is a derived cache: a schema change rebuilds it
//! from the files.
//!
//! Threads: every method here blocks (SQLite, the file system, parsing). The index runs its own
//! writer thread, a bounded parse pool during [`CodeIndex::scan`] (or a [`ScanHelper`] process
//! that does the scan) and the watcher's threads;
//! callers on an async runtime use a dedicated thread for `scan` and `spawn_blocking` for the
//! reads, which answer in milliseconds.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension};

mod db;
mod helper;
mod manifests;
mod parse;
mod reads;
mod scan;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use helper::{ScanHelper, scan_helper_main};

/// The content hash the index records, and [`CodeIndex::file_hashes`] reports, for a file at
/// the repository-relative `path` holding `content`: `None` for a file the index does not read
/// (too large, binary, minified or a lockfile). For content not on disk yet, such as a file as
/// a commit left it.
pub fn content_hash(path: &str, content: &[u8]) -> Option<String> {
    scan::indexed_hash(path, content)
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("code index database: {0}")]
    Db(String),
    #[error("reading {path}: {message}")]
    Io { path: String, message: String },
    #[error("file watcher: {0}")]
    Watch(String),
    #[error("{0}")]
    Invalid(String),
    /// The index was closed.
    #[error("the code index is closed")]
    Closed,
}

/// Where an index lives and what it covers.
#[derive(Debug, Clone)]
pub struct IndexConfig {
    /// The index's SQLite file, e.g. `<data>/brains/<project>/index.sqlite`.
    pub db_path: PathBuf,
    /// The repository's top-level directory.
    pub root: PathBuf,
    /// Parse threads for a scan; 0 means one less than the machine's cores (at least 1).
    pub threads: usize,
    /// Runs scans in a process of their own (see [`ScanHelper`]); `None` scans in this one.
    pub scan_helper: Option<ScanHelper>,
}

/// A file whose content changed since the index last saw it, as the watcher and scans report
/// it to the Brain (which marks the nodes that depend on it stale).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    /// Repository-relative path, `/`-separated.
    pub path: String,
    /// The new content's BLAKE3 hash (hex); `None` when the file was deleted.
    pub hash: Option<String>,
}

/// Called with each batch of changed files, from the index's own threads.
pub type ChangeSink = Arc<dyn Fn(Vec<FileChange>) + Send + Sync>;

/// What the index is doing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum IndexState {
    /// Opened, never scanned.
    New,
    Scanning {
        done: u64,
        /// Files found so far (the walk and the parse overlap, so it can grow).
        total: u64,
    },
    Ready,
    Failed {
        error: String,
    },
}

/// Files of one language.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LanguageCount {
    /// `rust`, `typescript`, `tsx`, `javascript`, `python`, `go`, `java`, `c`, `cpp`,
    /// `csharp`, `ruby`, `php`, `swift`, `kotlin`, or `other`.
    pub language: String,
    pub files: u64,
}

/// The index at a glance (Inspector, budgets).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatus {
    pub root: String,
    pub state: IndexState,
    pub files: u64,
    /// Definitions.
    pub symbols: u64,
    pub references: u64,
    pub languages: Vec<LanguageCount>,
    /// The watcher is running.
    pub watching: bool,
    pub last_scan_at_ms: Option<i64>,
    /// How long the last full scan took (walk, hash, parse, store).
    pub last_scan_ms: Option<u64>,
    /// Files that scan re-parsed (the rest were unchanged).
    pub last_scan_parsed: Option<u64>,
    /// Last time the watcher applied a change.
    pub updated_at_ms: Option<i64>,
}

/// What a scan did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ScanStats {
    pub files: u64,
    /// Re-parsed because new or changed.
    pub parsed: u64,
    pub removed: u64,
    /// Skipped: too large, binary, minified or generated.
    pub skipped: u64,
    pub symbols: u64,
    pub references: u64,
    pub duration_ms: u64,
    /// The files whose content changed (for the Brain's staleness).
    #[ts(skip)]
    #[serde(skip)]
    pub changed: Vec<FileChange>,
}

/// What a search looks for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SearchKind {
    /// Symbol definitions by name.
    Symbol,
    /// Files by path.
    File,
    /// Both.
    #[default]
    Any,
}

/// A `code_search` query.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CodeQuery {
    /// Name or path fragment; matched as a substring, best matches first (exact, prefix,
    /// substring, then fuzzy).
    pub query: String,
    #[serde(default)]
    pub kind: SearchKind,
    /// Only this language (see [`LanguageCount::language`]).
    #[serde(default)]
    pub language: Option<String>,
    /// Only under this repository-relative folder.
    #[serde(default)]
    pub path: Option<String>,
    /// At most this many hits (default 30, at most 200).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// A symbol definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SymbolHit {
    pub name: String,
    /// The tags query's kind: `function`, `method`, `class`, `interface`, `module`, `macro`,
    /// `constant`, `type`, …
    pub kind: String,
    pub path: String,
    /// 1-based.
    pub line: u32,
    pub end_line: u32,
    /// The definition's first line, trimmed (at most 200 characters).
    pub signature: String,
    /// Its doc comment, when the grammar's tags query captures one (at most 300 characters).
    pub doc: Option<String>,
}

/// One `code_search` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CodeHit {
    Symbol {
        symbol: SymbolHit,
    },
    File {
        path: String,
        language: String,
        bytes: u64,
    },
}

/// A place a symbol is referenced (a call, a type use).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceHit {
    pub path: String,
    pub line: u32,
    /// `call`, `type`, `implementation`, …, from the tags query.
    pub kind: String,
    /// The referencing line, trimmed (at most 200 characters).
    pub context: String,
}

/// `code_refs`: where a name is defined and referenced. References are matched to definitions
/// by name only (tags carry no type information), so an overloaded name lists them all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SymbolRefs {
    pub name: String,
    pub definitions: Vec<SymbolHit>,
    pub references: Vec<ReferenceHit>,
    /// More references exist than were returned.
    pub truncated: bool,
}

/// A package manifest the index understood.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub path: String,
    /// `cargo`, `npm`, `pnpmWorkspace`, `python`, `go`, `ruby`, `composer`, `maven`,
    /// `gradle`.
    pub kind: String,
    /// The package or module name, when it has one.
    pub name: Option<String>,
    pub version: Option<String>,
    /// Workspace members / packages it declares.
    pub members: Vec<String>,
    /// Direct dependency names with their version requirements.
    pub dependencies: Vec<Dependency>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Dependency {
    pub name: String,
    pub requirement: Option<String>,
    /// A development-only dependency.
    pub dev: bool,
}

/// A runnable command the repository defines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Script {
    pub name: String,
    pub command: String,
    /// Where it is defined: `package.json`, `Makefile`, `justfile`, a workflow file, …
    pub source: String,
}

/// A service of the service map (compose files, Dockerfiles, Procfiles).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    pub name: String,
    /// The file that defines it.
    pub source: String,
    /// Its build context or working folder, repository-relative, when known.
    pub path: Option<String>,
    pub image: Option<String>,
    /// Published ports as written (`"8080:80"`).
    pub ports: Vec<String>,
    pub depends_on: Vec<String>,
}

/// A module: a workspace member or package folder, with what it depends on inside the repo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Module {
    pub name: String,
    pub path: String,
    /// The manifest that makes it a module.
    pub manifest: String,
    /// Other modules of this repository it depends on, by name.
    pub depends_on: Vec<String>,
    pub files: u64,
    pub languages: Vec<LanguageCount>,
}

/// `project_map`: the repository's structure.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMap {
    pub modules: Vec<Module>,
    pub manifests: Vec<Manifest>,
    pub scripts: Vec<Script>,
    pub services: Vec<Service>,
    /// Top-level folders with their file counts.
    pub folders: Vec<FolderCount>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FolderCount {
    pub path: String,
    pub files: u64,
}

/// A running watcher; dropping it stops watching.
pub struct Watcher {
    debouncer: Option<
        notify_debouncer_full::Debouncer<
            notify::RecommendedWatcher,
            notify_debouncer_full::NoCache,
        >,
    >,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    status: Arc<Mutex<IndexStatus>>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.debouncer.take();
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let Ok(mut status) = self.status.lock() {
            status.watching = false;
        }
    }
}

struct Inner {
    root: PathBuf,
    db_path: PathBuf,
    threads: usize,
    scan_helper: Option<ScanHelper>,
    writer: mpsc::Sender<db::Command>,
    readers: Vec<Mutex<Connection>>,
    next_reader: AtomicUsize,
    status: Arc<Mutex<IndexStatus>>,
    scan_lock: Mutex<()>,
}

/// The code index of one repository. Cheap to clone (a handle).
#[derive(Clone)]
pub struct CodeIndex {
    inner: Arc<Inner>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis().min(i64::MAX as u128) as i64)
}

fn is_metadata_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    matches!(
        name,
        "Cargo.toml"
            | "package.json"
            | "pnpm-workspace.yaml"
            | "pyproject.toml"
            | "go.mod"
            | "Gemfile"
            | "composer.json"
            | "pom.xml"
            | "build.gradle"
            | "build.gradle.kts"
            | "Makefile"
            | "justfile"
            | "Dockerfile"
            | "Procfile"
    ) || name.starts_with("requirements") && name.ends_with(".txt")
        || name.starts_with("Dockerfile.")
        || (name.starts_with("docker-compose") || name.starts_with("compose"))
            && (name.ends_with(".yml") || name.ends_with(".yaml"))
        || path.starts_with(".github/workflows/")
            && (name.ends_with(".yml") || name.ends_with(".yaml"))
}

impl CodeIndex {
    /// Opens (creating or rebuilding) the index database and starts its writer thread. Does not scan.
    pub fn open(config: IndexConfig) -> Result<Self> {
        let root = config.root.canonicalize().map_err(|e| Error::Io {
            path: config.root.display().to_string(),
            message: e.to_string(),
        })?;
        if !root.is_dir() {
            return Err(Error::Invalid("index root must be a directory".into()));
        }
        let (writer, readers) = db::open(&config.db_path)?;
        let threads = if config.threads == 0 {
            thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1).max(1))
        } else {
            config.threads
        };
        let status = IndexStatus {
            root: root.display().to_string(),
            state: IndexState::New,
            files: 0,
            symbols: 0,
            references: 0,
            languages: Vec::new(),
            watching: false,
            last_scan_at_ms: None,
            last_scan_ms: None,
            last_scan_parsed: None,
            updated_at_ms: None,
        };
        let index = Self {
            inner: Arc::new(Inner {
                root,
                db_path: config.db_path,
                threads,
                scan_helper: config.scan_helper,
                writer,
                readers,
                next_reader: AtomicUsize::new(0),
                status: Arc::new(Mutex::new(status)),
                scan_lock: Mutex::new(()),
            }),
        };
        index.refresh_status()?;
        Ok(index)
    }

    fn with_read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let i = self.inner.next_reader.fetch_add(1, Ordering::Relaxed) % self.inner.readers.len();
        let conn = self.inner.readers[i].lock().map_err(|_| Error::Closed)?;
        f(&conn)
    }

    fn refresh_status(&self) -> Result<()> {
        let (files, symbols, references, languages) = self.with_read(reads::counts)?;
        let mut status = self.inner.status.lock().map_err(|_| Error::Closed)?;
        status.files = files;
        status.symbols = symbols;
        status.references = references;
        status.languages = languages;
        Ok(())
    }

    /// Brings the index up to date with the files. Blocks until done.
    pub fn scan(&self) -> Result<ScanStats> {
        self.scan_collecting(false)
    }

    /// Forgets everything indexed and scans the files again. Blocks until done. The database
    /// is emptied in place, so every handle to this index stays valid.
    pub fn rebuild(&self) -> Result<ScanStats> {
        self.scan_collecting(true)
    }

    /// [`Self::scan`] (or with `rebuild`, [`Self::rebuild`]) that hands the changed files to
    /// `changed` in slices as they are found instead of collecting them in the stats: a first
    /// scan of a large repository changes every file.
    pub fn scan_into(
        &self,
        rebuild: bool,
        changed: &mut dyn FnMut(Vec<FileChange>),
    ) -> Result<ScanStats> {
        self.scan_impl(rebuild, changed)
    }

    fn scan_collecting(&self, rebuild: bool) -> Result<ScanStats> {
        let mut all = Vec::new();
        let mut stats = self.scan_impl(rebuild, &mut |mut files| all.append(&mut files))?;
        stats.changed = all;
        Ok(stats)
    }

    /// Watches the repository and keeps the index current. An event only says that something
    /// changed (the scan finds what), so no file-id cache is kept: on macOS the default one
    /// walks and remembers every path under the root, ignored ones included.
    pub fn watch(&self, sink: ChangeSink) -> Result<Watcher> {
        let (tx, rx) = mpsc::channel::<notify_debouncer_full::DebounceEventResult>();
        let mut debouncer =
            notify_debouncer_full::new_debouncer_opt::<_, notify::RecommendedWatcher, _>(
                Duration::from_millis(500),
                None,
                move |result| {
                    let _ = tx.send(result);
                },
                notify_debouncer_full::NoCache,
                notify::Config::default(),
            )
            .map_err(|e| Error::Watch(e.to_string()))?;
        debouncer
            .watch(&self.inner.root, notify::RecursiveMode::Recursive)
            .map_err(|e| Error::Watch(e.to_string()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let index = self.clone();
        let handle = thread::Builder::new()
            .name("index-watcher".into())
            .spawn(move || {
                while !thread_stop.load(Ordering::SeqCst) {
                    let Ok(result) = rx.recv_timeout(Duration::from_millis(200)) else {
                        continue;
                    };
                    let relevant = match &result {
                        Ok(events) => events.iter().any(|event| {
                            event.event.need_rescan()
                                || event.paths.iter().any(|path| {
                                    let Ok(relative) = path.strip_prefix(&index.inner.root) else {
                                        return false;
                                    };
                                    let rel = relative.to_string_lossy().replace('\\', "/");
                                    if rel == ".git/HEAD" {
                                        return true;
                                    }
                                    if rel.starts_with(".git/") {
                                        return false;
                                    }
                                    let mut builder = ignore::WalkBuilder::new(&index.inner.root);
                                    builder.hidden(false);
                                    builder.build_matchers().into_iter().next().is_none_or(
                                        |mut matcher| {
                                            !matcher.matched(relative, path.is_dir()).is_ignore()
                                        },
                                    )
                                })
                        }),
                        Err(_) => true,
                    };
                    if relevant
                        && let Err(error) = index.scan_into(false, &mut |changed| sink(changed))
                    {
                        tracing::warn!(%error, "watcher rescan failed");
                    }
                }
            })
            .map_err(|e| Error::Watch(e.to_string()))?;
        if let Ok(mut status) = self.inner.status.lock() {
            status.watching = true;
        }
        Ok(Watcher {
            debouncer: Some(debouncer),
            stop,
            thread: Some(handle),
            status: self.inner.status.clone(),
        })
    }

    /// The index at a glance; cheap (no I/O).
    pub fn status(&self) -> IndexStatus {
        self.inner.status.lock().map_or_else(
            |_| IndexStatus {
                root: self.inner.root.display().to_string(),
                state: IndexState::Failed {
                    error: "status unavailable".into(),
                },
                files: 0,
                symbols: 0,
                references: 0,
                languages: Vec::new(),
                watching: false,
                last_scan_at_ms: None,
                last_scan_ms: None,
                last_scan_parsed: None,
                updated_at_ms: None,
            },
            |s| s.clone(),
        )
    }

    /// Symbols and files matching `query`.
    pub fn search(&self, query: &CodeQuery) -> Result<Vec<CodeHit>> {
        self.with_read(|c| reads::search(c, query))
    }

    /// Where `name` is defined and referenced (at most `limit` references).
    pub fn refs(&self, name: &str, limit: u32) -> Result<SymbolRefs> {
        self.with_read(|c| reads::refs(c, name, limit))
    }

    /// What the index has under exactly `name`, as text for a model: the files at a path
    /// (`src/lib.rs`, `_layout.tsx`), or a symbol's definitions (the last part of
    /// `Type::method`) and at most `limit` of its references. Empty when nothing has that
    /// name.
    pub fn lookup(&self, name: &str, limit: u32) -> Result<String> {
        let limit = limit.clamp(1, 50);
        let file = name.contains('/')
            || name.rsplit_once('.').is_some_and(|(stem, ext)| {
                !stem.is_empty() && ext.chars().all(char::is_alphanumeric)
            });
        let mut text = String::new();
        if file {
            let hits = self.with_read(|conn| reads::lookup_files(conn, name, limit))?;
            for hit in hits {
                if let CodeHit::File { path, language, .. } = hit {
                    text.push_str(&format!("- file {path} ({language})\n"));
                }
            }
            return Ok(text);
        }
        let symbol = name.rsplit("::").next().unwrap_or(name);
        let refs = self.refs(symbol, limit)?;
        if refs.definitions.is_empty() && refs.references.is_empty() {
            return Ok(text);
        }
        for def in refs.definitions.iter().take(limit as usize) {
            text.push_str(&format!(
                "- {} {}:{} {}\n",
                def.kind, def.path, def.line, def.signature
            ));
        }
        if refs.definitions.len() > limit as usize {
            text.push_str(&format!(
                "  (+{} more definitions)\n",
                refs.definitions.len() - limit as usize
            ));
        }
        if !refs.references.is_empty() {
            text.push_str("  used at:\n");
            for reference in &refs.references {
                text.push_str(&format!(
                    "  - {}:{} {}\n",
                    reference.path, reference.line, reference.context
                ));
            }
            if refs.truncated {
                text.push_str("  (more references: code_refs lists them)\n");
            }
        }
        Ok(text)
    }

    /// Modules, manifests, scripts, services and top-level folders.
    pub fn project_map(&self) -> Result<ProjectMap> {
        self.with_read(reads::project_map)
    }

    /// The indexed content hash of each path (`None`: not indexed), for node provenance.
    pub fn file_hashes(&self, paths: &[String]) -> Result<Vec<(String, Option<String>)>> {
        self.with_read(|conn| {
            let mut stmt = conn
                .prepare("SELECT hash FROM files WHERE path=?1")
                .map_err(|e| Error::Db(e.to_string()))?;
            paths
                .iter()
                .map(|path| {
                    let hash: Option<String> = stmt
                        .query_row([path], |r| r.get(0))
                        .optional()
                        .map_err(|e| Error::Db(e.to_string()))?;
                    Ok((path.clone(), hash.filter(|h| !h.is_empty())))
                })
                .collect()
        })
    }

    /// The definitions in one file (repository-relative), in line order, at most `limit`.
    pub fn outline(&self, path: &str, limit: u32) -> Result<Vec<SymbolHit>> {
        self.with_read(|c| reads::outline(c, path, limit))
    }

    /// A compact text overview for a model, at most `max_bytes`.
    pub fn digest(&self, max_bytes: usize) -> Result<String> {
        self.with_read(|c| reads::digest(c, max_bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn exact_file_lookup_filters_before_the_limit() {
        let dir =
            std::env::temp_dir().join(format!("index-lookup-{}-{}", std::process::id(), now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let cleanup = TempDir(dir.clone());
        let index = CodeIndex::open(IndexConfig {
            db_path: dir.join("index.sqlite"),
            root: dir.clone(),
            threads: 1,
            scan_helper: None,
        })
        .unwrap();
        let paths = (0..10).map(|n| format!("main.rs.{n}")).chain([
            "src/main.rs".into(),
            "src/_main.rs".into(),
            "src/xmain.rs".into(),
        ]);
        let files = paths
            .map(|path| db::FileRow {
                path,
                lang: "rust".into(),
                size: 1,
                mtime_ns: 1,
                hash: "h1".into(),
                symbols: Vec::new(),
                is_new: true,
            })
            .collect();
        db::send_apply(&index.inner.writer, files, Vec::new()).unwrap();
        assert_eq!(
            index.lookup("main.rs", 10).unwrap(),
            "- file src/main.rs (rust)\n"
        );
        assert_eq!(
            index.lookup("src/main.rs", 10).unwrap(),
            "- file src/main.rs (rust)\n"
        );
        assert_eq!(
            index.lookup("_main.rs", 1).unwrap(),
            "- file src/_main.rs (rust)\n"
        );
        drop(index);
        drop(cleanup);
    }
}
