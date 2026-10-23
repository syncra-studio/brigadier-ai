//! The Project Brains, the Personal Brain and the code indexes the manager keeps open, and
//! what feeds them.
//!
//! - Each project gets a [`ProjectBrain`]: its Brain (`<data>/brains/<project>/brain.sqlite`)
//!   and, for a project with a repository, its code index (`index.sqlite` beside it), scanned
//!   on a thread of its own and then kept current by a watcher. Every changed file is handed
//!   to the Brain, which marks what it knew about the file stale.
//! - The index's modules and services become Brain nodes (origin `index`), so a question about
//!   the project's structure is answered without a scout.
//! - The user's answers and plan decisions become nodes with their provenance (session,
//!   commit); worker reports and the orchestrator's `remember` did in older versions.
//! - One local embedding model serves every Brain. It is downloaded once a project exists,
//!   loaded on first use, and unloaded after [`EMBEDDER_IDLE`].
//!
//! Every Brain and index call blocks (SQLite, the file system, parsing), so each runs on the
//! blocking pool or, for scans and downloads, a thread of its own; never on the async runtime.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use brigadier_brain::{
    Brain, BrainAnswer, BrainCaps, BrainGraph, BrainQuery, Claim, Edge, EdgeKind, Embedder,
    EmbedderState, FileRef, NewNode, Node, NodeFilter, NodeKind, NodeState, Origin, Provenance,
    Route, Scope, TranscriptEntry, TranscriptStream,
};
use brigadier_index::{
    CodeHit, CodeIndex, CodeQuery, FileChange, IndexConfig, ScanHelper, SearchKind,
};
use brigadier_providers::{ItemStatus, ProviderEvent};
use brigadier_sandbox::removal;

use super::{SessionManager, blocking};
use crate::knowledge::{BrainOverview, ConventionsExport};
use crate::model::{ConversationId, DomainEvent, MessageRole, Project, ProjectId, Setup, streams};
use crate::tools::{CodeRefs, CodeSearch, SearchTranscript};
use crate::work::{Report, Task};
use crate::{Error, Result, now_ms};

/// Removing a project waits at most this long for its code index scan to end.
/// The personal preference a user's old Short replies off becomes.
pub(crate) const LONGER_ANSWERS: &str = "Longer answers";

const REMOVE_SCAN_WAIT: Duration = Duration::from_secs(120);
/// The embedding model is freed after this long unused.
const EMBEDDER_IDLE: Duration = Duration::from_secs(10 * 60);
/// `query_brain` timings kept for the Inspector's p95.
const QUERY_SAMPLES: usize = 1_000;
/// Nodes embedded per maintenance round, once the model is loaded.
const EMBED_BATCH: u32 = 256;
/// The answer budget of one `query_brain` call, in tokens (each kind of result is capped too,
/// with the rest by page).
const QUERY_TOKENS: u32 = 1_000;
/// Definitions and references `query_brain` shows per name it finds in the code index.
const CODE_LOOKUP: u32 = 10;
/// Transcript events read per page while indexing a conversation's transcript.
const TRANSCRIPT_PAGE: u32 = 500;
/// Most of a thread step's command or call shown in its entry's first line, which leads its
/// search hits.
const STEP_LEAD_BYTES: usize = 200;
/// Most of a thread step's command or call input indexed (its output is already cut at 8 KiB).
const STEP_INPUT_BYTES: usize = 8 * 1024;
/// The markers around the conventions Brigadier writes into an AGENTS.md.
const EXPORT_START: &str = "<!-- brigadier:conventions:start -->";
const EXPORT_END: &str = "<!-- brigadier:conventions:end -->";

/// A project's Brain and code index.
pub(crate) struct ProjectBrain {
    pub brain: Brain,
    pub index: Option<CodeIndex>,
    pub root: Option<PathBuf>,
    watcher: Mutex<Option<brigadier_index::Watcher>>,
    indexing: AtomicBool,
    /// The project is being removed: a scan ending now starts nothing.
    removed: AtomicBool,
}

/// Session decisions a briefing's ledger reads at most. Their lines alone would fill a model's
/// whole context long before this.
pub(crate) const LEDGER_MAX: u32 = 10_000;

/// One Brain write of a conversation in flight; a briefing waits for it
/// ([`SessionManager::learned`]).
struct Learning {
    manager: Arc<SessionManager>,
    id: ConversationId,
}

impl Learning {
    fn start(manager: Arc<SessionManager>, id: &ConversationId) -> Self {
        *manager.brains.learning().entry(id.clone()).or_default() += 1;
        Self {
            manager,
            id: id.clone(),
        }
    }
}

impl Drop for Learning {
    fn drop(&mut self) {
        let brains = &self.manager.brains;
        {
            let mut learning = brains.learning();
            if let Some(count) = learning.get_mut(&self.id) {
                *count -= 1;
                if *count == 0 {
                    learning.remove(&self.id);
                }
            }
        }
        brains.learned.notify_waiters();
    }
}

/// A full scan's size and time, for the static index budget.
#[derive(Debug, Clone)]
pub struct IndexRunStats {
    pub root: String,
    pub files: u64,
    pub parsed: u64,
    pub duration_ms: u64,
    pub at_ms: i64,
}

/// What the daemon reports about the Brains in its metrics.
#[derive(Debug, Clone, Default)]
pub struct BrainCounters {
    /// Recent `query_brain` calls, end to end, in ms.
    pub query_ms: Vec<f64>,
    pub embedder_loaded: bool,
    pub largest_index_run: Option<IndexRunStats>,
}

pub(crate) struct Brains {
    root: PathBuf,
    embedder: Arc<Embedder>,
    personal: Mutex<Option<Brain>>,
    projects: Mutex<HashMap<ProjectId, Arc<ProjectBrain>>>,
    query_ms: Mutex<VecDeque<f64>>,
    largest_run: Mutex<Option<IndexRunStats>>,
    downloading: AtomicBool,
    cancel_download: Arc<AtomicBool>,
    /// Per conversation: Brain writes still running (reports, the user's decisions).
    learning: Mutex<HashMap<ConversationId, usize>>,
    learned: tokio::sync::Notify,
}

impl Brains {
    pub(crate) fn new(data_dir: &Path) -> Self {
        let root = data_dir.join("brains");
        Self {
            embedder: Embedder::new(data_dir.join("models").join("embeddings")),
            root,
            personal: Mutex::new(None),
            projects: Mutex::new(HashMap::new()),
            query_ms: Mutex::new(VecDeque::new()),
            largest_run: Mutex::new(None),
            downloading: AtomicBool::new(false),
            cancel_download: Arc::new(AtomicBool::new(false)),
            learning: Mutex::new(HashMap::new()),
            learned: tokio::sync::Notify::new(),
        }
    }

    fn learning(&self) -> MutexGuard<'_, HashMap<ConversationId, usize>> {
        self.learning.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn projects(&self) -> MutexGuard<'_, HashMap<ProjectId, Arc<ProjectBrain>>> {
        self.projects.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn note_query(&self, ms: f64) {
        let mut samples = self.query_ms.lock().unwrap_or_else(|p| p.into_inner());
        if samples.len() == QUERY_SAMPLES {
            samples.pop_front();
        }
        samples.push_back(ms);
    }

    fn note_run(&self, run: IndexRunStats) {
        let mut largest = self.largest_run.lock().unwrap_or_else(|p| p.into_inner());
        if largest
            .as_ref()
            .is_none_or(|known| run.files >= known.files)
        {
            *largest = Some(run);
        }
    }

    /// Stops what runs on the Brains' own threads: a download and the watchers,
    /// which the caller drops off the async runtime (stopping one joins its thread, which may
    /// be in the middle of a rescan).
    pub(crate) fn shutdown(&self) -> Vec<brigadier_index::Watcher> {
        self.cancel_download.store(true, Ordering::Release);
        self.projects()
            .values()
            .filter_map(|project| project.take_watcher())
            .collect()
    }
}

impl ProjectBrain {
    fn take_watcher(&self) -> Option<brigadier_index::Watcher> {
        self.watcher
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
    }
}

/// Stops watchers off the async runtime.
pub(super) async fn stop_watchers(watchers: Vec<brigadier_index::Watcher>) {
    if !watchers.is_empty() {
        let _ = blocking(move || {
            drop(watchers);
            Ok(())
        })
        .await;
    }
}

impl SessionManager {
    /// Opens every project's Brain and starts indexing its repository.
    pub(super) async fn open_brains(&self) {
        let projects = self.core.catalog().projects;
        for project in projects {
            // A freed-up project's Brain is built again when the project is used.
            if self.brain_dormant(&project.id) {
                continue;
            }
            if let Err(err) = self.project_brain(&project.id).await {
                tracing::warn!(project = %project.id, error = %err, "could not open the project's brain");
            }
        }
        if !self.brains.projects().is_empty() {
            self.ensure_embedder();
        }
    }

    /// A project was created or its repositories changed: (re)open its Brain and index.
    pub async fn project_changed(&self, project: &Project) {
        let stale = self.brains.projects().get(&project.id).is_some_and(|open| {
            open.root.as_deref().map(Path::to_path_buf)
                != project.repos.first().map(|repo| PathBuf::from(&repo.path))
        });
        let old = if stale {
            self.brains.projects().remove(&project.id)
        } else {
            None
        };
        if let Some(old) = old {
            stop_watchers(old.take_watcher().into_iter().collect()).await;
            // What the index (and the skeleton passes and enrichment of older Brigadiers)
            // learned describes the old repository: it goes before the new one is scanned.
            // Sessions' decisions and reports stay.
            if old.root.is_some() {
                let brain = old.brain.clone();
                let forgotten = blocking(move || {
                    brain
                        .forget_origins(&[Origin::Index, Origin::Skeleton, Origin::Enrichment])
                        .map_err(brain_error)
                })
                .await;
                if let Err(err) = forgotten {
                    tracing::warn!(project = %project.id, error = %err, "could not forget the old repository's structure");
                }
            }
        }
        match self.project_brain(&project.id).await {
            Ok(_) => self.ensure_embedder(),
            Err(err) => {
                tracing::warn!(project = %project.id, error = %err, "could not open the project's brain");
            }
        }
    }

    /// A project is being removed: its watcher stops, and a scan in progress (it
    /// can't be cancelled, and it holds the Brain and index open) is waited for, all within
    /// [`REMOVE_SCAN_WAIT`]; then its Brain and index close. Past that, removal goes on: the
    /// scan starts nothing when it ends, and its files are deleted by the cleanup ledger.
    pub(crate) async fn close_project_brain(&self, id: &ProjectId) {
        let Some(open) = self.brains.projects().get(id).cloned() else {
            return;
        };
        open.removed.store(true, Ordering::Release);
        let deadline = tokio::time::Instant::now() + REMOVE_SCAN_WAIT;
        let watchers = open.take_watcher().into_iter().collect();
        let closed = tokio::time::timeout_at(deadline, async {
            stop_watchers(watchers).await;
            while open.indexing.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;
        self.brains.projects().remove(id);
        if closed.is_err() {
            tracing::warn!(project = %id, "the project's code index was still scanning when it was removed");
            return;
        }
        // The scan thread lets go of the Brain just after it says it is done.
        while Arc::strong_count(&open) > 1 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// A project's Brain and index files.
    pub(crate) fn project_brain_dir(&self, id: &ProjectId) -> PathBuf {
        self.brains.root.join(&id.0)
    }

    /// Marks a freed-up project's Brain: no launch opens it again, its first use does.
    fn dormant_mark(&self, id: &ProjectId) -> PathBuf {
        self.brains.root.join(format!("{}.dormant", id.0))
    }

    pub(crate) fn brain_dormant(&self, id: &ProjectId) -> bool {
        self.dormant_mark(id).exists()
    }

    /// Leaves the project's Brain closed until the project is used again (see
    /// [`Self::project_brain`]).
    pub(crate) fn make_brain_dormant(&self, id: &ProjectId) -> Result<()> {
        let mark = self.dormant_mark(id);
        std::fs::create_dir_all(&self.brains.root)
            .and_then(|()| std::fs::write(&mark, b""))
            .map_err(|err| Error::Invalid(format!("writing {}: {err}", mark.display())))
    }

    /// The project is used, or gone: its Brain opens as usual again. Whether it was dormant.
    pub(crate) fn wake_brain(&self, id: &ProjectId) -> bool {
        let mark = self.dormant_mark(id);
        match std::fs::remove_file(&mark) {
            Ok(()) => true,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
            Err(err) => {
                tracing::warn!(project = %id, error = %err, "could not wake the project's brain");
                true
            }
        }
    }

    /// Moves a removed project's Brain and index files (`brains/<project>/`) to the Trash, once
    /// the project is gone so nothing opens them again: rebuilding them costs time, so they
    /// can be put back. What they took, or why they stay (Storage offers them later).
    pub(crate) async fn trash_project_brain(&self, id: &ProjectId) -> Result<u64> {
        let (root, dir) = (self.brains.root.clone(), self.project_brain_dir(id));
        let owner = self.runtime.platform().paths().instance.clone();
        blocking(move || {
            if !dir.exists() {
                return Ok(0);
            }
            let bytes = removal::allocated_size(&dir);
            removal::bind(&root, &dir)
                .and_then(|bound| removal::trash(&bound, &owner))
                .map_err(|err| Error::Invalid(err.to_string()))?;
            Ok(bytes)
        })
        .await
    }

    /// The project's Brain and index, opened (and its indexing started) on first use.
    pub(crate) async fn project_brain(&self, id: &ProjectId) -> Result<Arc<ProjectBrain>> {
        if let Some(open) = self.brains.projects().get(id) {
            return Ok(open.clone());
        }
        let project = self.core.project(id)?;
        let woke = self.wake_brain(id);
        let dir = self.brains.root.join(&id.0);
        let root = project.repos.first().map(|repo| PathBuf::from(&repo.path));
        let embedder = self.brains.embedder.clone();
        // Scans run in `brigadierd index-scan`, so their memory leaves with that process.
        let scan_helper = ScanHelper {
            program: self.config.daemon_exe.clone(),
            args: vec!["index-scan".into()],
        };
        let opened = {
            let root = root.clone();
            blocking(move || {
                std::fs::create_dir_all(&dir)
                    .map_err(|err| Error::Invalid(format!("creating {}: {err}", dir.display())))?;
                let brain = Brain::open(&dir.join("brain.sqlite"), Scope::Project, embedder)
                    .map_err(brain_error)?;
                let index = match &root {
                    Some(root) => Some(
                        CodeIndex::open(IndexConfig {
                            db_path: dir.join("index.sqlite"),
                            root: root.clone(),
                            threads: 0,
                            scan_helper: Some(scan_helper),
                        })
                        .map_err(index_error)?,
                    ),
                    None => None,
                };
                Ok(ProjectBrain {
                    brain,
                    index,
                    root,
                    watcher: Mutex::new(None),
                    indexing: AtomicBool::new(false),
                    removed: AtomicBool::new(false),
                })
            })
            .await?
        };
        let opened = {
            let mut projects = self.brains.projects();
            projects
                .entry(id.clone())
                .or_insert_with(|| Arc::new(opened))
                .clone()
        };
        self.start_indexing(id, &opened, false);
        // A launch opens no dormant Brain, so it may not have fetched the embedding model.
        if woke {
            self.ensure_embedder();
        }
        Ok(opened)
    }

    /// The Personal Brain, opened on first use.
    pub(crate) async fn personal_brain(&self) -> Result<Brain> {
        if let Some(brain) = self
            .brains
            .personal
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
        {
            return Ok(brain);
        }
        let (dir, embedder) = (self.brains.root.clone(), self.brains.embedder.clone());
        let brain = blocking(move || {
            std::fs::create_dir_all(&dir)
                .map_err(|err| Error::Invalid(format!("creating {}: {err}", dir.display())))?;
            Brain::open(&dir.join("personal.sqlite"), Scope::Personal, embedder)
                .map_err(brain_error)
        })
        .await?;
        let mut personal = self
            .brains
            .personal
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        Ok(personal.get_or_insert(brain).clone())
    }

    /// Watches the project's repository and scans it (from scratch with `rebuild`) on a thread
    /// of its own, then records its structure. Returns false when indexing already runs.
    ///
    /// The watcher starts first, so a file changed during the scan is picked up by a rescan
    /// once it ends (the index runs one scan at a time).
    fn start_indexing(&self, id: &ProjectId, project: &Arc<ProjectBrain>, rebuild: bool) -> bool {
        let Some(index) = project.index.clone() else {
            return false;
        };
        if project.indexing.swap(true, Ordering::AcqRel) {
            return false;
        }
        let flag = project.clone();
        let (manager, project, id) = (self.me.clone(), project.clone(), id.clone());
        let spawned = std::thread::Builder::new()
            .name(format!("brigadier-index-{}", id.0))
            .spawn(move || {
                let mut watcher = project.watcher.lock().unwrap_or_else(|p| p.into_inner());
                if watcher.is_none() && !project.removed.load(Ordering::Acquire) {
                    let brain = project.brain.clone();
                    let watched = index.clone();
                    let sink: brigadier_index::ChangeSink =
                        Arc::new(move |changes: Vec<FileChange>| {
                            files_changed(&brain, &changes);
                            if changes.iter().any(|change| is_manifest(&change.path)) {
                                learn_structure(&brain, &watched);
                            }
                        });
                    match index.watch(sink) {
                        Ok(started) => *watcher = Some(started),
                        Err(err) => {
                            tracing::warn!(project = %id, error = %err, "could not watch the repository");
                        }
                    }
                }
                drop(watcher);
                let brain = project.brain.clone();
                let scanned =
                    index.scan_into(rebuild, &mut |changed| files_changed(&brain, &changed));
                let Some(manager) = manager
                    .upgrade()
                    .filter(|_| !project.removed.load(Ordering::Acquire))
                else {
                    drop((brain, index));
                    project.indexing.store(false, Ordering::Release);
                    return;
                };
                match scanned {
                    Ok(stats) => {
                        manager.brains.note_run(IndexRunStats {
                            root: project
                                .root
                                .as_deref()
                                .map(|root| root.display().to_string())
                                .unwrap_or_default(),
                            files: stats.files,
                            parsed: stats.parsed,
                            duration_ms: stats.duration_ms,
                            at_ms: now_ms(),
                        });
                        tracing::info!(project = %id, files = stats.files, parsed = stats.parsed, ms = stats.duration_ms, "code index scanned");
                        learn_structure(&project.brain, &index);
                    }
                    Err(err) => {
                        tracing::warn!(project = %id, error = %err, "code index scan failed");
                    }
                }
                drop((brain, index));
                project.indexing.store(false, Ordering::Release);
            });
        match spawned {
            Ok(_) => true,
            Err(err) => {
                tracing::warn!(error = %err, "could not start indexing");
                flag.indexing.store(false, Ordering::Release);
                false
            }
        }
    }

    /// Downloads the embedding model if it is missing, on a thread of its own. It is loaded on
    /// first use (a query, or nodes waiting for embeddings at the next upkeep).
    fn ensure_embedder(&self) {
        let embedder = self.brains.embedder.clone();
        if !matches!(
            embedder.status().state,
            EmbedderState::NotInstalled | EmbedderState::Failed { .. }
        ) {
            return;
        }
        if self.brains.downloading.swap(true, Ordering::AcqRel) {
            return;
        }
        let (manager, cancel) = (self.me.clone(), self.brains.cancel_download.clone());
        let spawned = std::thread::Builder::new()
            .name("brigadier-embedder-download".into())
            .spawn(move || {
                if let Err(err) = embedder.download(&cancel) {
                    tracing::warn!(error = %err, "could not download the embedding model");
                }
                if let Some(manager) = manager.upgrade() {
                    manager.brains.downloading.store(false, Ordering::Release);
                }
            });
        if let Err(err) = spawned {
            self.brains.downloading.store(false, Ordering::Release);
            tracing::warn!(error = %err, "could not start the embedding model download");
        }
    }

    /// Periodic upkeep: embeds what waits for the model, ages research, frees an idle model.
    pub(super) async fn brain_upkeep(&self) {
        let brains: Vec<Brain> = self
            .brains
            .projects()
            .values()
            .map(|project| project.brain.clone())
            .chain(
                self.brains
                    .personal
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone(),
            )
            .collect();
        let embedder = self.brains.embedder.clone();
        let _ = blocking(move || {
            let loaded = matches!(embedder.status().state, EmbedderState::Loaded);
            for brain in &brains {
                if let Err(err) = brain.expire(now_ms()) {
                    tracing::debug!(error = %err, "could not age research nodes");
                }
                if loaded && let Err(err) = brain.embed_pending(EMBED_BATCH) {
                    tracing::debug!(error = %err, "could not embed pending nodes");
                }
            }
            if !loaded
                && brains.iter().any(|brain| {
                    brain
                        .stats()
                        .is_ok_and(|stats| stats.unembedded > 0 && stats.nodes > 0)
                })
                && matches!(embedder.status().state, EmbedderState::Installed)
            {
                embedder.request_load();
            }
            embedder.unload_if_idle(EMBEDDER_IDLE);
            Ok(())
        })
        .await;
    }

    /// The Brains' counters for the daemon's metrics.
    /// The Brains' work in progress, said plainly: an index scan, the embedding model's
    /// download. Empty when none runs.
    pub fn brain_work(&self) -> Vec<String> {
        let mut work = Vec::new();
        if self
            .brains
            .projects()
            .values()
            .any(|project| project.indexing.load(Ordering::Acquire))
        {
            work.push("indexing a project".to_owned());
        }
        if self.brains.downloading.load(Ordering::Acquire) {
            work.push("downloading the embedding model".to_owned());
        }
        work
    }

    pub fn brain_counters(&self) -> BrainCounters {
        BrainCounters {
            query_ms: self
                .brains
                .query_ms
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .iter()
                .copied()
                .collect(),
            embedder_loaded: matches!(self.brains.embedder.status().state, EmbedderState::Loaded),
            largest_index_run: self
                .brains
                .largest_run
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone(),
        }
    }

    /// The project a conversation belongs to, if it has one.
    pub(super) fn project_of(&self, id: &ConversationId) -> Option<ProjectId> {
        self.core.conversation(id).ok()?.project_id
    }

    // ----- the orchestrator's tools --------------------------------------------------------

    /// `query_brain`: the project's knowledge, then the user's preferences that match.
    pub(crate) async fn query_brain_tool(
        &self,
        id: &ConversationId,
        worktree: Option<&std::path::Path>,
        text: String,
        history: bool,
        page: Option<u32>,
    ) -> Result<String> {
        let started = Instant::now();
        // Each claim's sources are re-read for this answer (BUILD-PLAN E1).
        let reader = self.sources_for(id, worktree);
        let project = match self.project_of(id) {
            Some(project) => Some(self.project_brain(&project).await?),
            None => None,
        };
        let personal = self.personal_brain().await.ok();
        // A node id (from a briefing's decision lines): that node in full.
        if let Ok(node_id) = uuid::Uuid::parse_str(text.trim()) {
            let (node_id, brains) = (
                node_id.to_string(),
                project
                    .iter()
                    .map(|project| project.brain.clone())
                    .chain(personal.clone())
                    .collect::<Vec<_>>(),
            );
            let reader = reader.clone();
            let found = blocking(move || {
                for brain in brains {
                    if let Some(mut node) = brain.node(&node_id).map_err(brain_error)? {
                        brigadier_brain::check_sources(&mut [&mut node], &|source| {
                            reader.check(source)
                        });
                        return Ok(Some(node));
                    }
                }
                Ok(None)
            })
            .await?;
            self.brains
                .note_query(started.elapsed().as_secs_f64() * 1000.0);
            return Ok(match found {
                Some(node) => node_text(&node),
                None => format!("No Brain node has the id {}.", text.trim()),
            });
        }
        let query = text.clone();
        // Names only code has also go to the code index (once, on the first page); the Brain
        // still answers the whole question.
        let names = match brigadier_brain::route(&text) {
            Route::Code { names } if page.unwrap_or(1) <= 1 => names,
            _ => Vec::new(),
        };
        let index = project.as_ref().and_then(|project| project.index.clone());
        let (code, answer, preferences) = blocking(move || {
            let code: Vec<String> = match &index {
                Some(index) => names
                    .iter()
                    .filter_map(|name| {
                        let found = index.lookup(name, CODE_LOOKUP).ok()?;
                        (!found.is_empty()).then(|| format!("`{name}`\n{}", found.trim_end()))
                    })
                    .collect(),
                None => Vec::new(),
            };
            let answer = match &project {
                Some(project) => Some(
                    project
                        .brain
                        .query_with(
                            &BrainQuery {
                                text: query.clone(),
                                kinds: Vec::new(),
                                limit: None,
                                max_tokens: Some(QUERY_TOKENS),
                                files: true,
                                history,
                                caps: Some(BrainCaps::default()),
                                page,
                            },
                            &|source| reader.check(source),
                        )
                        .map_err(brain_error)?,
                ),
                None => None,
            };
            let preferences = match &personal {
                Some(personal) => personal
                    .query(&BrainQuery {
                        text: query,
                        kinds: vec![NodeKind::Preference],
                        limit: Some(4),
                        max_tokens: Some(300),
                        files: false,
                        history,
                        caps: None,
                        page: None,
                    })
                    .ok(),
                None => None,
            };
            Ok((code, answer, preferences))
        })
        .await?;
        let mut reply = String::new();
        if !code.is_empty() {
            reply.push_str("[Code index]\n");
            reply.push_str(&code.join("\n"));
            reply.push_str("\n\n");
        }
        match &answer {
            Some(answer) if !answer.hits.is_empty() => {
                reply.push_str("[Project Brain]\n");
                reply.push_str(&answer.text);
            }
            _ if !code.is_empty() => reply.push_str("[Project Brain]\nNothing more on this."),
            // The thread reads the code itself too (BUILD-PLAN B3): a scout is for a big look
            // around, not a missing Brain line.
            _ => reply.push_str(
                "The Project Brain has nothing on this yet: read the code yourself (`brigadier code search`, `code refs`, `code map`, then the lines you need).",
            ),
        }
        if let Some(preferences) = preferences.filter(|found| !found.hits.is_empty()) {
            reply.push_str("\n\n[The user's preferences]\n");
            reply.push_str(&preferences.text);
        }
        self.brains
            .note_query(started.elapsed().as_secs_f64() * 1000.0);
        Ok(reply)
    }

    /// `search_transcript`: passages of this conversation's full transcript.
    pub(crate) async fn search_transcript_tool(
        &self,
        id: &ConversationId,
        args: SearchTranscript,
    ) -> Result<String> {
        let project = self
            .project_of(id)
            .ok_or_else(|| Error::Invalid("this conversation has no transcript index".into()))?;
        let project = self.project_brain(&project).await?;
        self.sync_transcript(id, &project.brain).await?;
        let (brain, conversation, limit) = (
            project.brain.clone(),
            id.0.clone(),
            args.limit.unwrap_or(8).clamp(1, 30),
        );
        let query = args.query;
        let hits = blocking(move || {
            brain
                .search_transcript(&conversation, &query, limit)
                .map_err(brain_error)
        })
        .await?;
        if hits.is_empty() {
            return Ok("Nothing in this conversation's transcript matches.".into());
        }
        let mut text = String::new();
        for hit in hits {
            let date = crate::manager::prompts::date_of(hit.at_ms);
            let snippet = hit.snippet.trim();
            match hit.stream {
                // `#N` is how a user's message is cited (remember's `user_message`).
                TranscriptStream::Conversation => {
                    text.push_str(&format!("[{} · {date} · #{}]\n", hit.role, hit.seq));
                }
                // A step of the thread's own work is never shown as `#N`; its first line (the
                // command or call, and where a whole output is kept) leads, as the passage
                // may not reach it.
                TranscriptStream::Thread => {
                    text.push_str(&format!(
                        "[{} · {date} · thread step {}]\n",
                        hit.role, hit.seq
                    ));
                    if !hit.lead.is_empty() && !snippet.contains(hit.lead.trim()) {
                        text.push_str(hit.lead.trim());
                        text.push_str(if snippet.starts_with('…') {
                            "\n"
                        } else {
                            "\n…"
                        });
                    }
                }
            }
            text.push_str(snippet);
            text.push_str("\n\n");
        }
        Ok(text.trim_end().to_owned())
    }

    /// Adds what the conversation and its thread did since each stream's last indexed or read
    /// event to the Brain's transcript index. Each stream keeps its own watermark: a
    /// conversation indexed before the thread's stream was still has its thread's whole stream
    /// indexed from the start.
    pub(crate) async fn sync_transcript(&self, id: &ConversationId, brain: &Brain) -> Result<()> {
        for stream in [TranscriptStream::Conversation, TranscriptStream::Thread] {
            self.sync_transcript_stream(id, brain, stream).await?;
        }
        Ok(())
    }

    pub(crate) async fn sync_transcript_stream(
        &self,
        id: &ConversationId,
        brain: &Brain,
        stream: TranscriptStream,
    ) -> Result<()> {
        let conversation = id.0.clone();
        let mut after = {
            let (brain, conversation) = (brain.clone(), conversation.clone());
            blocking(move || {
                brain
                    .transcript_watermark(&conversation, stream)
                    .map_err(brain_error)
            })
            .await?
            .unwrap_or(0)
        };
        let name = match stream {
            TranscriptStream::Conversation => streams::conversation(id),
            TranscriptStream::Thread => streams::orchestrator(id),
        };
        loop {
            let page = self
                .core
                .store()
                .read_stream_since(name.clone(), after, TRANSCRIPT_PAGE)
                .await?;
            let Some(last) = page.last() else {
                return Ok(());
            };
            after = last.stream_seq;
            let full = page.len() as u32 == TRANSCRIPT_PAGE;
            let mut entries = Vec::new();
            for stored in page {
                let found = match stream {
                    TranscriptStream::Conversation => {
                        match serde_json::from_str::<DomainEvent>(stored.payload.get()) {
                            Ok(event) => self.transcript_text(event).await,
                            Err(_) => None,
                        }
                    }
                    TranscriptStream::Thread => match super::export::thread_event(&stored) {
                        super::export::Decoded::Event(event) => self
                            .thread_step_text(&event)
                            .map(|(role, text)| (role, None, text)),
                        _ => None,
                    },
                };
                if let Some((role, request_id, text)) = found {
                    entries.push(TranscriptEntry {
                        conversation_id: conversation.clone(),
                        stream,
                        seq: stored.stream_seq,
                        role: role.into(),
                        request_id,
                        at_ms: stored.at_ms,
                        text,
                    });
                }
            }
            let (brain, conversation) = (brain.clone(), conversation.clone());
            blocking(move || {
                brain
                    .index_transcript_through(&conversation, stream, after, entries)
                    .map_err(brain_error)
            })
            .await?;
            if !full {
                return Ok(());
            }
        }
    }

    /// What a finished command or tool call of the thread adds to the searchable transcript:
    /// its role (`command`, `read` for reads and searches, `tool`), and a first line with the
    /// call and where its whole output is kept, then the call in full when that line cuts it,
    /// then its output as recorded (cut at 8 KiB). A kept output is named, never copied in.
    /// The thread's own `search_transcript` calls are left out: their passages would only
    /// repeat what they found.
    fn thread_step_text(&self, event: &ProviderEvent) -> Option<(&'static str, String)> {
        let finished =
            |status: &ItemStatus| matches!(status, ItemStatus::Completed | ItemStatus::Failed);
        let (role, mut lead, call, output, kept) = match event {
            ProviderEvent::Command {
                command,
                status,
                exit_code,
                output,
                kept,
                ..
            } if finished(status) => {
                let mut lead = format!("Ran `{}`", lead_part(command));
                match exit_code {
                    Some(code) => lead.push_str(&format!(" (exit {code})")),
                    None if matches!(status, ItemStatus::Failed) => lead.push_str(" (failed)"),
                    None => {}
                }
                (command_role(command), lead, command.as_str(), output, kept)
            }
            ProviderEvent::ToolCall {
                name,
                input,
                status,
                output,
                kept,
                ..
            } if finished(status) => {
                if name.ends_with("search_transcript") {
                    return None;
                }
                let input = input.as_deref().unwrap_or_default();
                let args = serde_json::from_str::<serde_json::Value>(input).ok();
                let arg = |key: &str| {
                    args.as_ref()
                        .and_then(|args| args.get(key))
                        .and_then(serde_json::Value::as_str)
                        .map(lead_part)
                };
                let path = arg("file_path").or_else(|| arg("notebook_path"));
                let (role, mut lead) = match (name.as_str(), path, arg("pattern")) {
                    ("Read" | "NotebookRead", Some(path), _) => ("read", format!("Read {path}")),
                    ("Grep" | "Glob", _, Some(pattern)) => {
                        let within = arg("path").map(|path| format!(" in {path}"));
                        (
                            "read",
                            format!("{name} `{pattern}`{}", within.unwrap_or_default()),
                        )
                    }
                    ("Read" | "NotebookRead" | "Grep" | "Glob" | "LS", ..) => {
                        ("read", format!("{name} {}", lead_part(input)))
                    }
                    _ if matches!(input.trim(), "" | "{}") => ("tool", format!("Called {name}")),
                    _ => ("tool", format!("Called {name} with {}", lead_part(input))),
                };
                if matches!(status, ItemStatus::Failed) {
                    lead.push_str(" (failed)");
                }
                (role, lead, input, output, kept)
            }
            _ => return None,
        };
        let blobs = self.core.store().blobs();
        let kept_file = |hash: &str| {
            hash.parse::<brigadier_store::BlobHash>()
                .ok()
                .map(|hash| blobs.file_of(&hash).display().to_string())
        };
        if let Some(kept) = kept.as_deref() {
            if let Some(path) = kept.blob.as_deref().and_then(kept_file) {
                lead.push_str(&format!("; whole output ({} bytes): {path}", kept.bytes));
            }
            if let Some(file) = &kept.file
                && let Some(path) = file.blob.as_deref().and_then(kept_file)
            {
                lead.push_str(&format!(
                    "; the CLI's saved copy ({} bytes): {path}",
                    file.bytes
                ));
            }
        }
        let mut text = lead;
        if lead_part(call) != call.trim() {
            text.push('\n');
            text.push_str(&cut(call.trim(), STEP_INPUT_BYTES));
        }
        if let Some(output) = output.as_deref().filter(|output| !output.trim().is_empty()) {
            text.push('\n');
            text.push_str(output);
        }
        Some((role, text))
    }

    /// What a conversation event adds to the searchable transcript.
    async fn transcript_text(
        &self,
        event: DomainEvent,
    ) -> Option<(&'static str, Option<String>, String)> {
        match event {
            DomainEvent::MessageAppended { message } => {
                let role = match message.role {
                    MessageRole::User => "user",
                    MessageRole::Assistant => "assistant",
                    MessageRole::System => "brigadier",
                    MessageRole::Peer => "peer",
                };
                let text = crate::sessions::display_text(
                    &self.full_words(&message).await,
                    &message.attachments,
                );
                Some((role, message.request_id, text))
            }
            DomainEvent::TaskUpdated { task } => {
                let report = task.report.as_ref()?;
                if !matches!(task.state, crate::work::TaskState::Reported) {
                    return None;
                }
                Some((
                    "report",
                    task.request_id.clone(),
                    report_text(&task, report),
                ))
            }
            DomainEvent::QuestionUpdated { question } => {
                let answer = question.answer.as_ref()?;
                Some((
                    "decision",
                    question.request_id.clone(),
                    format!("Asked the user: {}\nThey answered: {answer}", question.text),
                ))
            }
            _ => None,
        }
    }

    // ----- the workers' code-index tools ---------------------------------------------------

    pub(super) async fn task_index(&self, id: &ConversationId) -> Result<CodeIndex> {
        let project = self
            .project_of(id)
            .ok_or_else(|| Error::Invalid("this task's session has no project".into()))?;
        self.project_brain(&project)
            .await?
            .index
            .clone()
            .ok_or_else(|| Error::Invalid("this project has no repository to index".into()))
    }

    /// `code_search`: its answer, and the files it names (repository-relative).
    pub(crate) async fn code_search_tool(
        &self,
        id: &ConversationId,
        args: CodeSearch,
    ) -> Result<(String, Vec<String>)> {
        code_search_found(self.task_index(id).await?, args).await
    }

    /// `code_refs`: its answer, and the files it names (repository-relative).
    pub(crate) async fn code_refs_tool(
        &self,
        id: &ConversationId,
        args: CodeRefs,
    ) -> Result<(String, Vec<String>)> {
        code_refs_found(self.task_index(id).await?, args).await
    }

    /// `project_map`.
    pub(crate) async fn project_map_tool(&self, id: &ConversationId) -> Result<String> {
        project_map(self.task_index(id).await?).await
    }

    // ----- what the Brain learns ------------------------------------------------------------

    /// Something the user settled on a card (an answer, a plan or outline go-ahead): a user
    /// decision node.
    pub(crate) fn learn_user_decision(
        &self,
        id: &ConversationId,
        key: String,
        title: String,
        body: String,
    ) {
        let manager = self.arc();
        let learning = Learning::start(self.arc(), id);
        let id = id.clone();
        self.spawn(async move {
            let _learning = learning;
            let Some(project) = manager.project_of(&id) else {
                return;
            };
            let commit = manager.head_commit(&id).await;
            let recorded = async {
                let project = manager.project_brain(&project).await?;
                let brain = project.brain.clone();
                let node = NewNode {
                    kind: NodeKind::Decision,
                    key: Some(key),
                    title: one_line(&title, 240),
                    body,
                    provenance: Provenance {
                        origin: Origin::User,
                        session_id: Some(id.0.clone()),
                        task_id: None,
                        job_id: None,
                        worker: None,
                        commit,
                        recorded_at_ms: now_ms(),
                        claim: None,
                    },
                    files: Vec::new(),
                    expires_at_ms: None,
                };
                blocking(move || brain.record(node).map_err(brain_error)).await
            };
            if let Err(err) = recorded.await {
                tracing::warn!(conversation = %id, error = %err, "the Brain could not keep a decision");
            }
        });
    }

    /// Waits, at most `limit`, until the conversation's Brain writes in flight are done.
    pub(crate) async fn learned(&self, id: &ConversationId, limit: Duration) {
        let settled = async {
            loop {
                let notified = self.brains.learned.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if !self.brains.learning().contains_key(id) {
                    return;
                }
                notified.await;
            }
        };
        if tokio::time::timeout(limit, settled).await.is_err() {
            tracing::warn!(conversation = %id, "Brain writes still running for a briefing");
        }
    }

    /// Decisions a handoff note lists that were never kept with `remember`: each becomes a
    /// node of the conversation, so this briefing's ledger and every later one carry it with
    /// an id. They are the thread's own claims, sourced by the session and the note's
    /// generation (BUILD-PLAN A11), unless a line cites a user decision: the user's words in
    /// double quotes, found in one of their messages here, or the id of a user decision
    /// node. Returns how many it kept.
    pub(crate) async fn keep_note_decisions(
        &self,
        id: &ConversationId,
        lines: Vec<String>,
        generation: u32,
    ) -> Result<u32> {
        let Some(project) = self.project_of(id) else {
            return Ok(0);
        };
        if lines.is_empty() {
            return Ok(0);
        }
        let project = self.project_brain(&project).await?;
        let commit = self.head_commit(id).await;
        let said = self.user_texts(id).await;
        let mut nodes = Vec::new();
        for line in lines {
            let cited = cites_user_words(&line, &said)
                || self.cites_user_decision(&project.brain, &line).await;
            let (origin, claim, from) = if cited {
                (Origin::User, None, "citing the user")
            } else {
                (
                    Origin::Orchestrator,
                    Some(Claim {
                        generation: Some(generation),
                        ..Claim::unverified()
                    }),
                    "your claim, unverified",
                )
            };
            nodes.push(NewNode {
                kind: NodeKind::Decision,
                key: Some(format!("handoff:{}:{}", id.0, blake_key(&line))),
                title: one_line(&line, 240),
                body: format!(
                    "{line}\n(from the handoff note before rebirth {generation}, {from})"
                ),
                provenance: Provenance {
                    origin,
                    session_id: Some(id.0.clone()),
                    task_id: None,
                    job_id: None,
                    worker: None,
                    commit: commit.clone(),
                    recorded_at_ms: now_ms(),
                    claim,
                },
                files: Vec::new(),
                expires_at_ms: None,
            });
        }
        let brain = project.brain.clone();
        blocking(move || {
            let mut kept = 0;
            for node in nodes {
                brain.record(node).map_err(brain_error)?;
                kept += 1;
            }
            Ok(kept)
        })
        .await
    }

    /// The full words of the user's messages in this conversation, newest first (at most the
    /// latest [`USER_TEXTS_MAX`]).
    async fn user_texts(&self, id: &ConversationId) -> Vec<String> {
        let mut texts = Vec::new();
        let mut before = None;
        while texts.len() < USER_TEXTS_MAX {
            let Ok(page) = self
                .core
                .list_messages(id.clone(), before, brigadier_store::MAX_PAGE - 1)
                .await
            else {
                break;
            };
            for message in page.messages.iter().rev() {
                if message.role == MessageRole::User {
                    texts.push(self.full_words(message).await);
                }
            }
            match page.messages.first() {
                Some(oldest) if page.has_more => before = Some(oldest.seq),
                _ => break,
            }
        }
        texts
    }

    /// Whether `line` names the id of a node the user decided.
    async fn cites_user_decision(&self, brain: &Brain, line: &str) -> bool {
        let ids: Vec<String> = line
            .split(|c: char| !(c.is_ascii_hexdigit() || c == '-'))
            .filter(|word| uuid::Uuid::parse_str(word).is_ok())
            .map(str::to_owned)
            .collect();
        if ids.is_empty() {
            return false;
        }
        let brain = brain.clone();
        blocking(move || {
            for id in ids {
                if brain
                    .node(&id)
                    .map_err(brain_error)?
                    .is_some_and(|node| node.provenance.origin == Origin::User)
                {
                    return Ok(true);
                }
            }
            Ok(false)
        })
        .await
        .unwrap_or(false)
    }

    /// The session checkout's HEAD commit, for provenance.
    async fn head_commit(&self, id: &ConversationId) -> Option<String> {
        let Some(Setup::Session { repo, .. }) = self.core.conversation(id).ok()?.setup else {
            return None;
        };
        let git = self.git.clone();
        blocking(move || {
            let repo = git.open(Path::new(&repo)).map_err(super::git_error)?;
            Ok(repo.resolve("HEAD").map_err(super::git_error)?.0)
        })
        .await
        .ok()
    }

    /// The decisions settled in a conversation, oldest first (the rebirth ledger).
    pub(crate) async fn session_decisions(&self, id: &ConversationId) -> Result<Vec<Node>> {
        let Some(project) = self.project_of(id) else {
            return Ok(Vec::new());
        };
        let project = self.project_brain(&project).await?;
        let (brain, session) = (project.brain.clone(), id.0.clone());
        let mut nodes = blocking(move || {
            brain
                .nodes(&NodeFilter {
                    kinds: vec![NodeKind::Decision, NodeKind::Convention, NodeKind::Contract],
                    session_id: Some(session),
                    current_only: true,
                    text: None,
                    limit: Some(LEDGER_MAX),
                })
                .map_err(brain_error)
        })
        .await?;
        nodes.sort_by_key(|node| node.created_at_ms);
        Ok(nodes)
    }

    /// Delete forgets the session’s entries and transcript index (G15).
    pub(crate) async fn forget_brain_conversation(
        &self,
        id: &ConversationId,
        project: Option<ProjectId>,
    ) {
        let Some(project) = project else {
            return;
        };
        // A freed-up project's Brain goes to the Trash whole: there is nothing to open for it.
        if self.brain_dormant(&project) && !self.brains.projects().contains_key(&project) {
            return;
        }
        let Ok(project) = self.project_brain(&project).await else {
            return;
        };
        let (brain, session) = (project.brain.clone(), id.0.clone());
        let forgotten = blocking(move || {
            brain
                .forget_session(&session)
                .map(|_| ())
                .map_err(brain_error)
        })
        .await;
        match forgotten {
            Ok(()) => {
                crate::logging::private_trace!(self.tab_chat_logs(id), info, "the Brain let go of a deleted conversation's transcript"; conversation = %id)
            }
            Err(err) => {
                crate::logging::private_trace!(self.tab_chat_logs(id), warn, "could not clear a deleted conversation from the Brain"; conversation = %id, error = %err)
            }
        }
    }

    // ----- the Inspector and Settings --------------------------------------------------------

    async fn brain_for(&self, project: Option<&ProjectId>) -> Result<(Brain, Option<CodeIndex>)> {
        match project {
            Some(id) => {
                let project = self.project_brain(id).await?;
                Ok((project.brain.clone(), project.index.clone()))
            }
            None => Ok((self.personal_brain().await?, None)),
        }
    }

    pub async fn brain_overview(&self, project: Option<ProjectId>) -> Result<BrainOverview> {
        let (brain, index) = self.brain_for(project.as_ref()).await?;
        let stats = blocking(move || brain.stats().map_err(brain_error)).await?;
        Ok(BrainOverview {
            project_id: project,
            stats,
            index: index.map(|index| index.status()),
            embedder: self.brains.embedder.status(),
        })
    }

    pub async fn query_brain(
        &self,
        project: Option<ProjectId>,
        query: BrainQuery,
    ) -> Result<BrainAnswer> {
        let (brain, _) = self.brain_for(project.as_ref()).await?;
        blocking(move || brain.query(&query).map_err(brain_error)).await
    }

    pub async fn brain_graph(
        &self,
        project: Option<ProjectId>,
        filter: NodeFilter,
    ) -> Result<BrainGraph> {
        let (brain, _) = self.brain_for(project.as_ref()).await?;
        blocking(move || brain.graph(&filter).map_err(brain_error)).await
    }

    /// Brigadier no longer has a Short replies setting (BUILD-PLAN §1.4): a user who had it off
    /// keeps their choice as a personal preference for longer answers, copied once, and the
    /// setting is then saved as on, so it isn't copied again.
    pub(super) async fn carry_short_replies(&self) {
        let settings = self.core.settings();
        if settings.short_replies {
            return;
        }
        let kept = async {
            let brain = self.personal_brain().await?;
            let node = NewNode {
                kind: NodeKind::Preference,
                key: None,
                title: LONGER_ANSWERS.into(),
                body: "The user had turned Short replies off: write fuller answers than the \
                       short default, with full detail where it helps."
                    .into(),
                provenance: Provenance {
                    origin: Origin::User,
                    session_id: None,
                    task_id: None,
                    job_id: None,
                    worker: None,
                    commit: None,
                    recorded_at_ms: now_ms(),
                    claim: None,
                },
                files: Vec::new(),
                expires_at_ms: None,
            };
            blocking(move || brain.record(node).map_err(brain_error)).await?;
            self.core
                .update_settings(crate::model::Settings {
                    short_replies: true,
                    ..settings
                })
                .await
        };
        match kept.await {
            Ok(_) => tracing::info!("kept Short replies off as the preference for longer answers"),
            Err(err) => {
                tracing::warn!(error = %err, "could not keep Short replies off as a preference")
            }
        }
    }

    /// The Personal Brain's memories, newest first.
    pub async fn list_memories(&self) -> Result<Vec<Node>> {
        let brain = self.personal_brain().await?;
        blocking(move || {
            brain
                .nodes(&NodeFilter {
                    kinds: vec![NodeKind::Preference],
                    session_id: None,
                    current_only: true,
                    text: None,
                    limit: Some(1_000),
                })
                .map_err(brain_error)
        })
        .await
    }

    /// The user's correction of a memory, in place: the same memory (where it came from too),
    /// with no earlier text kept; a Chat that saved it shows the new text.
    pub async fn edit_memory(&self, node_id: String, text: String) -> Result<Node> {
        let text = one_line(&text, 240);
        if text.is_empty() {
            return Err(Error::Invalid(
                "a memory can't be empty; forget it instead".into(),
            ));
        }
        if let Some(reason) = super::memory_guard::not_for_memory(&text) {
            return Err(Error::Invalid(format!(
                "Brigadier doesn't keep that: {reason}"
            )));
        }
        let brain = self.personal_brain().await?;
        let id = node_id.clone();
        let title = text.clone();
        let node = blocking(move || {
            let current = brain.node(&id).map_err(brain_error)?;
            if current
                .as_ref()
                .is_none_or(|node| node.kind != NodeKind::Preference)
            {
                return Ok(None);
            }
            brain.edit(&id, title, String::new()).map_err(brain_error)?;
            brain.node(&id).map_err(brain_error)
        })
        .await?
        .ok_or_else(|| Error::NotFound(format!("memory {node_id}")))?;
        if let Some(session) = node.provenance.session_id.clone() {
            let conversation = ConversationId(session);
            if self.core.conversation(&conversation).is_ok() {
                self.core
                    .record_conversation(
                        &conversation,
                        vec![DomainEvent::MemoryUpdated {
                            conversation_id: conversation.clone(),
                            memory: crate::knowledge::MemoryChange {
                                node_id,
                                text,
                                forgotten: false,
                                request_id: None,
                                at_ms: now_ms(),
                            },
                        }],
                    )
                    .await?;
            }
        }
        Ok(node)
    }

    /// Removes a memory; a Chat that saved it shows it as removed.
    pub async fn forget_memory(&self, node_id: String) -> Result<()> {
        let brain = self.personal_brain().await?;
        let id = node_id.clone();
        let node = blocking(move || {
            let node = brain.node(&id).map_err(brain_error)?;
            brain.delete(&id).map_err(brain_error)?;
            Ok(node)
        })
        .await?
        .ok_or_else(|| Error::NotFound(format!("memory {node_id}")))?;
        if let Some(session) = node.provenance.session_id {
            let conversation = ConversationId(session);
            if self.core.conversation(&conversation).is_ok() {
                self.core
                    .record_conversation(
                        &conversation,
                        vec![DomainEvent::MemoryUpdated {
                            conversation_id: conversation.clone(),
                            memory: crate::knowledge::MemoryChange {
                                node_id,
                                text: node.title,
                                forgotten: true,
                                request_id: None,
                                at_ms: now_ms(),
                            },
                        }],
                    )
                    .await?;
            }
        }
        Ok(())
    }

    /// Writes the project's conventions into the Brigadier section of an AGENTS.md at `path`.
    pub async fn export_conventions(
        &self,
        project: ProjectId,
        path: String,
    ) -> Result<ConventionsExport> {
        let target = PathBuf::from(&path);
        if !target.is_absolute() {
            return Err(Error::Invalid("give the file's absolute path".into()));
        }
        let project = self.project_brain(&project).await?;
        let brain = project.brain.clone();
        blocking(move || {
            let mut conventions = brain
                .nodes(&NodeFilter {
                    kinds: vec![NodeKind::Convention],
                    session_id: None,
                    current_only: true,
                    text: None,
                    limit: Some(500),
                })
                .map_err(brain_error)?;
            conventions.sort_by(|a, b| a.title.cmp(&b.title));
            let mut section = format!(
                "{EXPORT_START}\n## Conventions\n\nKept by Brigadier from this project's Brain; edit them there, since this section is rewritten on export.\n\n"
            );
            for node in &conventions {
                let body = node.body.trim();
                if body.is_empty() {
                    section.push_str(&format!("- {}\n", node.title));
                } else {
                    section.push_str(&format!("- {}: {}\n", node.title, one_line(body, 400)));
                }
            }
            section.push_str(EXPORT_END);
            let existing = match std::fs::read_to_string(&target) {
                Ok(text) => Some(text),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
                Err(err) => {
                    return Err(Error::Invalid(format!("reading {}: {err}", target.display())));
                }
            };
            let created = existing.is_none();
            let text = match existing {
                Some(text) => match (text.find(EXPORT_START), text.find(EXPORT_END)) {
                    (Some(start), Some(end)) if end > start => format!(
                        "{}{section}{}",
                        &text[..start],
                        &text[end + EXPORT_END.len()..]
                    ),
                    _ => format!("{}\n\n{section}\n", text.trim_end()),
                },
                None => format!("# AGENTS.md\n\n{section}\n"),
            };
            std::fs::write(&target, text)
                .map_err(|err| Error::Invalid(format!("writing {}: {err}", target.display())))?;
            Ok(ConventionsExport {
                path: target.display().to_string(),
                conventions: conventions.len() as u32,
                created,
            })
        })
        .await
    }

    /// Rebuilds a project's code index from its files, in the background. The index is
    /// emptied in place, so tool calls and jobs holding it keep a valid handle, and its
    /// watcher keeps running.
    pub async fn rebuild_index(&self, id: ProjectId) -> Result<()> {
        let project = self.project_brain(&id).await?;
        if project.index.is_none() {
            return Err(Error::Invalid("this project has no repository".into()));
        }
        if !self.start_indexing(&id, &project, true) {
            return Err(Error::Invalid(
                "the index is being built; rebuild it once that ends".into(),
            ));
        }
        Ok(())
    }
}

/// A node in full, as `query_brain` returns one asked for by id.
fn node_text(node: &Node) -> String {
    let state = match &node.state {
        NodeState::Fresh => String::new(),
        NodeState::Stale { reason, .. } => format!(" [may be outdated: {reason}]"),
        NodeState::Superseded { by, reason, .. } => match reason {
            Some(reason) => format!(" [replaced by {by}: {reason}]"),
            None => format!(" [replaced by {by}]"),
        },
    };
    let files: Vec<&str> = node.files.iter().map(|file| file.path.as_str()).collect();
    let mut text = format!(
        "[{}] {:?}: {}{state}\n{}",
        node.id,
        node.kind,
        node.title,
        node.body.trim()
    );
    if !files.is_empty() {
        text.push_str(&format!("\nFiles: {}", files.join(", ")));
    }
    text.push_str(&format!(
        "\n({}, {})",
        brigadier_brain::source_words(node.kind, &node.provenance),
        crate::manager::prompts::date_of(node.provenance.recorded_at_ms)
    ));
    text
}

/// Hands changed files to the Brain (on the index's threads).
fn files_changed(brain: &Brain, changes: &[FileChange]) {
    if changes.is_empty() {
        return;
    }
    let refs: Vec<FileRef> = changes
        .iter()
        .map(|change| FileRef {
            path: change.path.clone(),
            hash: change.hash.clone(),
        })
        .collect();
    match brain.files_changed(&refs) {
        Ok(stale) if !stale.is_empty() => {
            tracing::info!(
                files = changes.len(),
                stale = stale.len(),
                "brain nodes went stale"
            );
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(error = %err, "could not mark brain nodes stale"),
    }
}

fn is_manifest(path: &str) -> bool {
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
            | "Dockerfile"
            | "Procfile"
    ) || name.starts_with("docker-compose")
        || name.starts_with("compose.")
}

/// The index's modules and services as Brain nodes (origin `index`). A node a model has
/// described since (skeleton pass, enrichment) keeps its description.
fn learn_structure(brain: &Brain, index: &CodeIndex) {
    let map = match index.project_map() {
        Ok(map) => map,
        Err(err) => {
            tracing::warn!(error = %err, "could not read the project map");
            return;
        }
    };
    let provenance = || Provenance {
        origin: Origin::Index,
        session_id: None,
        task_id: None,
        job_id: None,
        worker: None,
        commit: None,
        recorded_at_ms: now_ms(),
        claim: None,
    };
    let described = |kind: NodeKind, key: &str| {
        brain
            .node_by_key(kind, key)
            .ok()
            .flatten()
            .is_some_and(|node| node.provenance.origin != Origin::Index)
    };
    let (mut sources, mut edges) = (Vec::new(), Vec::new());
    for module in &map.modules {
        let key = format!("module:{}", module.path);
        if described(NodeKind::Module, &key) {
            continue;
        }
        let languages: Vec<String> = module
            .languages
            .iter()
            .map(|lang| format!("{} {}", lang.files, lang.language))
            .collect();
        let mut body = format!(
            "Module `{}` at {} ({}), {} files: {}.",
            module.name,
            module.path,
            module.manifest,
            module.files,
            languages.join(", ")
        );
        if !module.depends_on.is_empty() {
            body.push_str(&format!(" Depends on: {}.", module.depends_on.join(", ")));
        }
        let node = NewNode {
            kind: NodeKind::Module,
            key: Some(key.clone()),
            title: format!("{} ({})", module.name, module.path),
            body,
            provenance: provenance(),
            files: vec![FileRef {
                path: module.manifest.clone(),
                hash: None,
            }],
            expires_at_ms: None,
        };
        if let Err(err) = brain.record(node) {
            tracing::warn!(error = %err, "could not record a module");
            continue;
        }
        sources.push(format!("key:{key}"));
        for dependency in &module.depends_on {
            if let Some(other) = map.modules.iter().find(|m| &m.name == dependency) {
                edges.push(Edge {
                    from: format!("key:{key}"),
                    to: format!("key:module:{}", other.path),
                    kind: EdgeKind::DependsOn,
                });
            }
        }
    }
    for service in &map.services {
        let key = format!("service:{}", service.name);
        if described(NodeKind::Service, &key) {
            continue;
        }
        let mut body = format!("Service `{}`, defined in {}.", service.name, service.source);
        if let Some(image) = &service.image {
            body.push_str(&format!(" Image: {image}."));
        }
        if let Some(path) = &service.path {
            body.push_str(&format!(" Built from {path}."));
        }
        if !service.ports.is_empty() {
            body.push_str(&format!(" Ports: {}.", service.ports.join(", ")));
        }
        if !service.depends_on.is_empty() {
            body.push_str(&format!(" Depends on: {}.", service.depends_on.join(", ")));
        }
        let node = NewNode {
            kind: NodeKind::Service,
            key: Some(key.clone()),
            title: format!("service {}", service.name),
            body,
            provenance: provenance(),
            files: vec![FileRef {
                path: service.source.clone(),
                hash: None,
            }],
            expires_at_ms: None,
        };
        if let Err(err) = brain.record(node) {
            tracing::warn!(error = %err, "could not record a service");
            continue;
        }
        sources.push(format!("key:{key}"));
        for dependency in &service.depends_on {
            if map.services.iter().any(|s| &s.name == dependency) {
                edges.push(Edge {
                    from: format!("key:{key}"),
                    to: format!("key:service:{dependency}"),
                    kind: EdgeKind::DependsOn,
                });
            }
        }
        if let Some(path) = &service.path
            && let Some(module) = map.modules.iter().find(|m| &m.path == path)
        {
            edges.push(Edge {
                from: format!("key:{key}"),
                to: format!("key:module:{}", module.path),
                kind: EdgeKind::Contains,
            });
        }
    }
    // Replaced, not added to: a dependency a manifest dropped goes too.
    if !sources.is_empty()
        && let Err(err) = brain.relink_structure(sources, edges)
    {
        tracing::warn!(error = %err, "could not link the project's modules");
    }
}

/// A report as the Brain keeps it: the question it answered, then the answer.
pub(crate) fn report_text(task: &Task, report: &Report) -> String {
    let mut text = format!(
        "Task: {}\n{}\n\nAnswer: {}",
        task.title,
        cut(&task.spec, 800),
        report.summary
    );
    for (title, items) in [
        ("Decisions", &report.decisions),
        ("Changes", &report.changes),
        ("Verification", &report.verification),
        ("Done when", &report.done_when),
        ("Open questions", &report.open_questions),
        ("Risks", &report.risks),
    ] {
        if !items.is_empty() {
            text.push_str(&format!("\n{title}:"));
            for item in items {
                text.push_str(&format!("\n- {item}"));
            }
        }
    }
    text
}

/// The user's messages a handoff note's decisions are checked against, at most.
const USER_TEXTS_MAX: usize = 2_000;
/// A quoted span shorter than this ("yes") proves nothing.
const QUOTE_MIN: usize = 8;

/// Whether `line` quotes, in double quotes, words one of `said` holds.
fn cites_user_words(line: &str, said: &[String]) -> bool {
    quoted(line)
        .into_iter()
        .filter(|span| span.trim().len() >= QUOTE_MIN)
        .any(|span| said.iter().any(|text| says(text, span)))
}

/// The spans of `text` in double quotes, straight or curly.
fn quoted(text: &str) -> Vec<&str> {
    let mut spans = Vec::new();
    let mut open: Option<usize> = None;
    for (at, c) in text.char_indices() {
        match (c, open) {
            ('"' | '\u{201c}', None) => open = Some(at + c.len_utf8()),
            ('"' | '\u{201d}', Some(start)) => {
                spans.push(&text[start..at]);
                open = None;
            }
            _ => {}
        }
    }
    spans
}

/// Whether `text` says `quote`, ignoring case, spacing and the quote marks around it.
pub(crate) fn says(text: &str, quote: &str) -> bool {
    let plain = |text: &str| {
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let quote = plain(quote.trim_matches(['"', '\'', '\u{201c}', '\u{201d}', ' ']));
    !quote.is_empty() && plain(text).contains(&quote)
}

pub(super) fn blake_key(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.trim().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The first line of `text`, at most `max` bytes (on a character boundary).
pub(crate) fn one_line(text: &str, max: usize) -> String {
    cut(text.trim().lines().next().unwrap_or_default().trim(), max)
}

/// `text` on one line with its whitespace collapsed, cut at [`STEP_LEAD_BYTES`]: a thread
/// step's call in its entry's first line.
/// A Codex command's role in the thread's transcript index: `read` for one that only reads,
/// lists or searches files (`sed -n`, `rg`, `ls`…), as Claude's `Read` and `Grep` are, else
/// `command`.
fn command_role(command: &str) -> &'static str {
    if brigadier_providers::only_looks(&brigadier_providers::policy::unwrapped_command(command)) {
        "read"
    } else {
        "command"
    }
}

fn lead_part(text: &str) -> String {
    cut(
        &text.split_whitespace().collect::<Vec<_>>().join(" "),
        STEP_LEAD_BYTES,
    )
}

/// `text` cut to at most `max` bytes on a character boundary, with an ellipsis if cut.
pub(crate) fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

pub(crate) fn brain_error(err: brigadier_brain::Error) -> Error {
    match err {
        brigadier_brain::Error::NotFound(what) => Error::NotFound(what),
        other => Error::Invalid(other.to_string()),
    }
}

pub(crate) fn index_error(err: brigadier_index::Error) -> Error {
    Error::Invalid(err.to_string())
}

/// [`code_search`], with the files its answer names, each once.
async fn code_search_found(index: CodeIndex, args: CodeSearch) -> Result<(String, Vec<String>)> {
    let kind = match args.kind.as_deref() {
        Some("symbol") => SearchKind::Symbol,
        Some("file") => SearchKind::File,
        Some("any") | None => SearchKind::Any,
        Some(other) => {
            return Err(Error::Invalid(format!(
                "`kind` is \"symbol\", \"file\" or \"any\", not \"{other}\""
            )));
        }
    };
    let query = CodeQuery {
        query: args.query,
        kind,
        language: args.language,
        path: args.path,
        limit: args.limit,
    };
    let hits = blocking(move || index.search(&query).map_err(index_error)).await?;
    if hits.is_empty() {
        return Ok(("No symbol or file matches.".into(), Vec::new()));
    }
    let mut text = String::new();
    let mut files: Vec<String> = Vec::new();
    for hit in hits {
        let path = match hit {
            CodeHit::Symbol { symbol } => {
                text.push_str(&format!(
                    "{} {} — {}:{}\n  {}\n",
                    symbol.kind, symbol.name, symbol.path, symbol.line, symbol.signature
                ));
                symbol.path
            }
            CodeHit::File {
                path,
                language,
                bytes,
            } => {
                text.push_str(&format!("file {path} ({language}, {bytes} bytes)\n"));
                path
            }
        };
        if !files.contains(&path) {
            files.push(path);
        }
    }
    Ok((text.trim_end().to_owned(), files))
}

/// [`code_refs`], with the files its answer names, each once.
async fn code_refs_found(index: CodeIndex, args: CodeRefs) -> Result<(String, Vec<String>)> {
    let limit = args.limit.unwrap_or(50).clamp(1, 500);
    let refs = blocking(move || index.refs(&args.symbol, limit).map_err(index_error)).await?;
    if refs.definitions.is_empty() && refs.references.is_empty() {
        return Ok((
            format!("Nothing named `{}` is indexed.", refs.name),
            Vec::new(),
        ));
    }
    let mut files: Vec<String> = Vec::new();
    for path in refs
        .definitions
        .iter()
        .map(|def| &def.path)
        .chain(refs.references.iter().map(|reference| &reference.path))
    {
        if !files.contains(path) {
            files.push(path.clone());
        }
    }
    let mut text = format!(
        "`{}` (references are matched by name, without type information)\nDefinitions:\n",
        refs.name
    );
    for def in &refs.definitions {
        text.push_str(&format!(
            "- {} {}:{} {}\n",
            def.kind, def.path, def.line, def.signature
        ));
    }
    text.push_str("References:\n");
    for reference in &refs.references {
        text.push_str(&format!(
            "- {}:{} ({}) {}\n",
            reference.path, reference.line, reference.kind, reference.context
        ));
    }
    if refs.truncated {
        text.push_str("[more references exist; raise `limit` to see them]\n");
    }
    Ok((text.trim_end().to_owned(), files))
}

/// `project_map`, answered from `index`.
pub(crate) async fn project_map(index: CodeIndex) -> Result<String> {
    blocking(move || index.digest(24_000).map_err(index_error)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Codex read (a command that only reads or searches, wrapped in its shell) is indexed
    /// as a read; any other command as a command.
    #[test]
    fn a_codex_read_is_indexed_as_a_read() {
        assert_eq!(
            command_role("/bin/zsh -lc \"sed -n '1,80p' src/a.rs\""),
            "read"
        );
        assert_eq!(command_role("rg -n thread_items crates"), "read");
        assert_eq!(command_role("/bin/zsh -lc 'cargo test -p core'"), "command");
    }
}
