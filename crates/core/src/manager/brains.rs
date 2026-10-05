//! The Project Brains, the Personal Brain and the code indexes the manager keeps open, and
//! what feeds them.
//!
//! - Each project gets a [`ProjectBrain`]: its Brain (`<data>/brains/<project>/brain.sqlite`)
//!   and, for a project with a repository, its code index (`index.sqlite` beside it), scanned
//!   on a thread of its own and then kept current by a watcher. Every changed file is handed
//!   to the Brain, which marks what it knew about the file stale.
//! - The index's modules and services become Brain nodes (origin `index`), so a question about
//!   the project's structure is answered without a scout.
//! - Worker reports, the user's answers and plan decisions, and the orchestrator's `remember`
//!   become nodes with their provenance (worker, session, commit).
//! - One local embedding model serves every Brain. It is downloaded once a project exists,
//!   loaded on first use, and unloaded after [`EMBEDDER_IDLE`].
//!
//! Every Brain and index call blocks (SQLite, the file system, parsing), so each runs on the
//! blocking pool or, for scans and downloads, a thread of its own; never on the async runtime.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use brigadier_brain::{
    Brain, BrainAnswer, BrainCaps, BrainGraph, BrainQuery, Edge, EdgeKind, Embedder, EmbedderState,
    FileRef, NewNode, Node, NodeFilter, NodeKind, NodeState, Origin, Provenance, Route, Scope,
    TranscriptEntry, WorkerRef,
};
use brigadier_index::{
    CodeHit, CodeIndex, CodeQuery, FileChange, IndexConfig, ScanHelper, SearchKind,
};
use brigadier_sandbox::removal;
use brigadier_store::StreamPage;

use super::brain_jobs::BrainJobs;
use super::{SessionManager, blocking};
use crate::knowledge::{BrainJob, BrainOverview, ConventionsExport};
use crate::model::{ConversationId, DomainEvent, MessageRole, Project, ProjectId, Setup, streams};
use crate::tools::{CodeRefs, CodeSearch, MemoryKind, Remember, SearchTranscript};
use crate::work::{ArtifactKind, ArtifactRef, Report, Task, TaskKind};
use crate::{Error, Result, now_ms};

/// Removing a project waits at most this long for its code index scan to end.
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
/// Research nodes go stale after this (PLAN.md §6 Phase 6: a TTL of about 7 days).
pub(crate) const RESEARCH_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// A report's findings files come into the Brain in parts of at most this many bytes, each a
/// node of its own, so a part fits whole in an answer (see the Brain's per-body cap).
const FINDINGS_PART_BYTES: usize = 1_400;
/// At most this much of one report's findings files goes into the Brain...
const FINDINGS_MAX_BYTES: usize = 24_000;
/// ...in at most this many parts.
const FINDINGS_MAX_PARTS: usize = 32;
/// Transcript events read per page while indexing a conversation's transcript.
const TRANSCRIPT_PAGE: u32 = 500;
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

/// Versions are assigned before spawning a report's Brain write. A later version may run
/// first, but an older one must never replace it or delete its findings parts.
#[derive(Default)]
pub(crate) struct ReportLearning {
    next: AtomicU64,
    stored: tokio::sync::Mutex<u64>,
}

impl ReportLearning {
    fn next(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed) + 1
    }

    async fn keep(&self, version: u64, write: impl Future<Output = Result<()>>) -> Result<()> {
        let mut stored = self.stored.lock().await;
        if version <= *stored {
            return Ok(());
        }
        write.await?;
        *stored = version;
        Ok(())
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
    pub(super) jobs: BrainJobs,
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
            jobs: BrainJobs::new(),
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

    /// The project's Brain, if it is open.
    pub(super) fn open_project(&self, id: &ProjectId) -> Option<Arc<ProjectBrain>> {
        self.projects().get(id).cloned()
    }

    /// The projects whose Brains are open.
    pub(super) fn open_projects(&self) -> Vec<(ProjectId, Arc<ProjectBrain>)> {
        self.projects()
            .iter()
            .map(|(id, project)| (id.clone(), project.clone()))
            .collect()
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

    /// Stops what runs on the Brains' own threads: a download, a Brain job, and the watchers,
    /// which the caller drops off the async runtime (stopping one joins its thread, which may
    /// be in the middle of a rescan).
    pub(crate) fn shutdown(&self) -> Vec<brigadier_index::Watcher> {
        self.cancel_download.store(true, Ordering::Release);
        self.jobs.stop("Brigadier is shutting down");
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
        self.start_skeleton_queue();
        let projects = self.core.catalog().projects;
        for project in projects {
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
            // A running job keeps the old repository's root and commit: it ends here, and
            // anything it still sends is refused (see `brain_job_call`).
            self.brains
                .jobs
                .stop_for(&project.id, "the project's repository changed");
            self.brains.jobs.forget_tries(&project.id);
            stop_watchers(old.take_watcher().into_iter().collect()).await;
            // What the index, the skeleton pass and enrichment learned describes the old
            // repository: it goes before the new one is scanned, and the skeleton pass maps
            // the new one. Sessions' decisions and reports stay.
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

    /// A project is being removed: its Brain job and watcher stop, and a scan in progress (it
    /// can't be cancelled, and it holds the Brain and index open) is waited for, all within
    /// [`REMOVE_SCAN_WAIT`]; then its Brain and index close. Past that, removal goes on: the
    /// scan starts nothing when it ends, and its files are deleted by the cleanup ledger.
    pub(crate) async fn close_project_brain(&self, id: &ProjectId) {
        self.brains.jobs.stop_for(id, "the project was removed");
        self.brains.jobs.forget_tries(id);
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
                        manager.brains.jobs.want_skeleton(id.clone());
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
        self.brain_job_tick().await;
        self.research_tick().await;
    }

    /// The Brains' counters for the daemon's metrics.
    /// The Brains' work in progress, said plainly: a Brain job, an index scan, the embedding
    /// model's download. Empty when none runs.
    pub fn brain_work(&self) -> Vec<String> {
        let mut work = Vec::new();
        if self.brains.jobs.running() {
            work.push("a Brain job".to_owned());
        }
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
    fn project_of(&self, id: &ConversationId) -> Option<ProjectId> {
        self.core.conversation(id).ok()?.project_id
    }

    // ----- the orchestrator's tools --------------------------------------------------------

    /// `query_brain`: the project's knowledge, then the user's preferences that match.
    pub(crate) async fn query_brain_tool(
        &self,
        id: &ConversationId,
        text: String,
        history: bool,
        page: Option<u32>,
        worker: bool,
    ) -> Result<String> {
        let started = Instant::now();
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
            let found = blocking(move || {
                for brain in brains {
                    if let Some(node) = brain.node(&node_id).map_err(brain_error)? {
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
                        .query(&BrainQuery {
                            text: query.clone(),
                            kinds: Vec::new(),
                            limit: None,
                            max_tokens: Some(QUERY_TOKENS),
                            files: true,
                            history,
                            caps: Some(BrainCaps::default()),
                            page,
                        })
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
            _ if worker => reply.push_str(
                "The Project Brain has nothing on this yet: look in the code (code_search, code_refs, project_map).",
            ),
            _ => reply.push_str(
                "The Project Brain has nothing on this yet. Delegate a scout (or research) task; its report is kept in the Brain for next time.",
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

    /// `remember`: a decision, convention, preference or contract.
    pub(crate) async fn remember_tool(
        &self,
        id: &ConversationId,
        args: Remember,
    ) -> Result<String> {
        let title = one_line(&args.title, 240);
        if title.is_empty() {
            return Err(Error::Invalid("say what to remember in `title`".into()));
        }
        let personal = args.personal || args.kind == MemoryKind::Preference;
        let kind = match args.kind {
            MemoryKind::Decision => NodeKind::Decision,
            MemoryKind::Convention => NodeKind::Convention,
            MemoryKind::Preference => NodeKind::Preference,
            MemoryKind::Contract => NodeKind::Contract,
        };
        let before = if personal {
            self.memory_lines(super::brain_jobs::MEMORY_BYTES).await
        } else {
            Vec::new()
        };
        let (brain, index) = if personal {
            (self.personal_brain().await?, None)
        } else {
            let project = self.project_of(id).ok_or_else(|| {
                Error::Invalid(
                    "this conversation has no project; set `personal` to keep it for the user"
                        .into(),
                )
            })?;
            let project = self.project_brain(&project).await?;
            (project.brain.clone(), project.index.clone())
        };
        let files = if personal {
            Vec::new()
        } else {
            file_refs(index.as_ref(), &args.files).await
        };
        let commit = if personal {
            None
        } else {
            self.head_commit(id).await
        };
        let node = NewNode {
            kind: if personal { NodeKind::Preference } else { kind },
            key: None,
            title,
            body: args.detail.unwrap_or_default(),
            provenance: Provenance {
                origin: Origin::Orchestrator,
                session_id: Some(id.0.clone()),
                task_id: None,
                job_id: None,
                worker: None,
                commit,
                recorded_at_ms: now_ms(),
            },
            files,
            expires_at_ms: None,
        };
        let replaces = args.replaces;
        // One write: nothing is kept from a call that failed.
        let node_id = blocking(move || {
            match replaces {
                Some(old) => brain.record_replacing(node, &old, "replaced by the orchestrator"),
                None => brain.record(node),
            }
            .map_err(brain_error)
        })
        .await?;
        if personal {
            self.told_own_preference(id, &before).await;
        }
        Ok(format!(
            "Remembered as {node_id}{}.",
            if personal {
                " (the user's preferences)"
            } else {
                ""
            }
        ))
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
            text.push_str(&format!(
                "[{} · {} · #{}]\n{}\n\n",
                hit.role,
                crate::manager::prompts::date_of(hit.at_ms),
                hit.seq,
                hit.snippet.trim()
            ));
        }
        Ok(text.trim_end().to_owned())
    }

    /// Adds the conversation's transcript since the last indexed event to the Brain's
    /// transcript index.
    pub(crate) async fn sync_transcript(&self, id: &ConversationId, brain: &Brain) -> Result<()> {
        let conversation = id.0.clone();
        let mut after = {
            let (brain, conversation) = (brain.clone(), conversation.clone());
            blocking(move || {
                brain
                    .transcript_watermark(&conversation)
                    .map_err(brain_error)
            })
            .await?
            .unwrap_or(0)
        };
        loop {
            let page = self
                .core
                .store()
                .read_stream_since(streams::conversation(id), after, TRANSCRIPT_PAGE)
                .await?;
            let Some(last) = page.last() else {
                return Ok(());
            };
            after = last.stream_seq;
            let full = page.len() as u32 == TRANSCRIPT_PAGE;
            let mut entries = Vec::new();
            for stored in page {
                let Ok(event) = serde_json::from_str::<DomainEvent>(stored.payload.get()) else {
                    continue;
                };
                if let Some((role, request_id, text)) = self.transcript_text(event).await {
                    entries.push(TranscriptEntry {
                        conversation_id: conversation.clone(),
                        seq: stored.stream_seq,
                        role: role.into(),
                        request_id,
                        at_ms: stored.at_ms,
                        text,
                    });
                }
            }
            if !entries.is_empty() {
                let brain = brain.clone();
                blocking(move || brain.index_transcript(entries).map_err(brain_error)).await?;
            }
            if !full {
                return Ok(());
            }
        }
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
            DomainEvent::PlanUpdated { plan } => {
                let decided = match &plan.state {
                    crate::work::PlanState::Approved { .. } => "approved",
                    crate::work::PlanState::Rejected { .. } => "rejected",
                    _ => return None,
                };
                Some((
                    "decision",
                    plan.request_id.clone(),
                    format!("The plan \"{}\" was {decided}.", plan.title),
                ))
            }
            _ => None,
        }
    }

    // ----- the workers' code-index tools ---------------------------------------------------

    async fn task_index(&self, id: &ConversationId) -> Result<CodeIndex> {
        let project = self
            .project_of(id)
            .ok_or_else(|| Error::Invalid("this task's session has no project".into()))?;
        self.project_brain(&project)
            .await?
            .index
            .clone()
            .ok_or_else(|| Error::Invalid("this project has no repository to index".into()))
    }

    /// `code_search`.
    pub(crate) async fn code_search_tool(
        &self,
        id: &ConversationId,
        args: CodeSearch,
    ) -> Result<String> {
        code_search(self.task_index(id).await?, args).await
    }

    /// `code_refs`.
    pub(crate) async fn code_refs_tool(
        &self,
        id: &ConversationId,
        args: CodeRefs,
    ) -> Result<String> {
        code_refs(self.task_index(id).await?, args).await
    }

    /// `project_map`.
    pub(crate) async fn project_map_tool(&self, id: &ConversationId) -> Result<String> {
        project_map(self.task_index(id).await?).await
    }

    // ----- what the Brain learns ------------------------------------------------------------

    /// A read-only task reported, or a write task landed: its report becomes a node (the
    /// task's question with its answer), each of its decisions another, linked to the report
    /// and to the modules it touched. `order`, when given, assigns a version before spawning
    /// the write, so an older report cannot replace a newer one even if it runs later.
    pub(crate) fn learn_report(
        &self,
        task: &Task,
        report: &Report,
        order: Option<Arc<ReportLearning>>,
    ) {
        let manager = self.arc();
        let learning = Learning::start(self.arc(), &task.conversation_id);
        let (task, report) = (task.clone(), report.clone());
        let version = order.as_ref().map(|order| order.next());
        self.spawn(async move {
            let _learning = learning;
            let write = manager.record_report(&task, &report);
            let result = match (order, version) {
                (Some(order), Some(version)) => order.keep(version, write).await,
                _ => write.await,
            };
            if let Err(err) = result {
                tracing::warn!(task = %task.id, error = %err, "the Brain could not keep a report");
            }
        });
    }

    /// A landed task's files with the hashes of their content in the commit that landed. The
    /// index may not have seen that commit yet: with the hashes from before it, the report
    /// would go stale as soon as the watcher caught up with the task's own change. Files the
    /// index doesn't track stay untracked; one the commit lacks or git can't read is left out
    /// of staleness rather than guessed, and so is every file when the repository can't be
    /// read.
    async fn as_landed(&self, task: &Task, landed: &str, files: Vec<FileRef>) -> Vec<FileRef> {
        let unhashed: Vec<FileRef> = files
            .iter()
            .map(|file| FileRef {
                path: file.path.clone(),
                hash: None,
            })
            .collect();
        let Ok(repo) = self.task_repo(task) else {
            return unhashed;
        };
        let (git, commit) = (self.git.clone(), brigadier_git::Oid(landed.to_owned()));
        blocking(move || {
            let repo = git.open(&repo).map_err(super::git_error)?;
            Ok(files
                .into_iter()
                .map(|file| {
                    let hash = file.hash.as_ref().and_then(|_| {
                        repo.file_at(&commit, &file.path)
                            .ok()
                            .flatten()
                            .and_then(|content| brigadier_index::content_hash(&file.path, &content))
                    });
                    FileRef { hash, ..file }
                })
                .collect())
        })
        .await
        .unwrap_or(unhashed)
    }

    async fn record_report(&self, task: &Task, report: &Report) -> Result<()> {
        if task.kind == TaskKind::Review {
            return Ok(());
        }
        let Some(project) = self.project_of(&task.conversation_id) else {
            return Ok(());
        };
        let project = self.project_brain(&project).await?;
        let mut paths: Vec<String> = report.changes.clone();
        paths.extend(mentioned_paths(&report.summary));
        paths.sort();
        paths.dedup();
        let mut files = file_refs(project.index.as_ref(), &paths).await;
        if let Some(landed) = &task.landed {
            files = self.as_landed(task, landed, files).await;
        }
        let provenance = Provenance {
            origin: Origin::Report,
            session_id: Some(task.conversation_id.0.clone()),
            task_id: Some(task.id.0.clone()),
            job_id: None,
            worker: Some(WorkerRef {
                provider: task.route.choice.provider.binary().into(),
                model: task.route.choice.model.clone(),
            }),
            // A landed write task's commit; what a read-only task looked at.
            commit: task
                .landed
                .clone()
                .or_else(|| task.workspace.as_ref().and_then(|w| w.base.clone())),
            recorded_at_ms: now_ms(),
        };
        let kind = match task.kind {
            TaskKind::Research => NodeKind::Research,
            TaskKind::Implement | TaskKind::Merge => NodeKind::Task,
            _ => NodeKind::Report,
        };
        let expires_at_ms = (kind == NodeKind::Research).then(|| now_ms() + RESEARCH_TTL_MS);
        let node = NewNode {
            kind,
            key: Some(format!("task:{}", task.id.0)),
            title: format!("task-{} {}", task.number, task.title),
            body: report_body(task, report),
            provenance: provenance.clone(),
            files: files.clone(),
            expires_at_ms,
        };
        // What it left in files rather than in its summary, with the report's provenance.
        let parts: Vec<NewNode> = self
            .findings_parts(task, report)
            .await
            .into_iter()
            .enumerate()
            .map(|(index, part)| NewNode {
                kind,
                key: Some(part_key(&task.id.0, index)),
                title: part.title,
                body: part.body,
                provenance: provenance.clone(),
                files: files.clone(),
                expires_at_ms,
            })
            .collect();
        let task_id = task.id.0.clone();
        let decisions: Vec<NewNode> = report
            .decisions
            .iter()
            .filter(|decision| !decision.trim().is_empty())
            .map(|decision| NewNode {
                kind: NodeKind::Decision,
                key: Some(format!("task:{}:{}", task.id.0, blake_key(decision))),
                title: one_line(decision, 240),
                body: format!("{decision}\n(made in task-{}: {})", task.number, task.title),
                provenance: provenance.clone(),
                files: files.clone(),
                expires_at_ms: None,
            })
            .collect();
        let modules = modules_of(&project, &paths).await;
        let brain = project.brain.clone();
        blocking(move || record_report_nodes(&brain, &task_id, node, parts, decisions, modules))
            .await
    }

    /// A report's findings files as Brain parts: the text a worker left in artifacts rather
    /// than in its summary (research notes, markdown or plain-text findings; never a diff, a
    /// transcript, command output or a binary), redacted with the task's secrets like
    /// everything it recorded, at most [`FINDINGS_MAX_BYTES`] in all.
    async fn findings_parts(&self, task: &Task, report: &Report) -> Vec<FindingsPart> {
        let mut findings: Vec<&ArtifactRef> = Vec::new();
        for artifact in report.artifacts.iter().chain(&task.outputs) {
            if is_findings(artifact) && !findings.iter().any(|seen| seen.id == artifact.id) {
                findings.push(artifact);
            }
        }
        if findings.is_empty() {
            return Vec::new();
        }
        let redactor = self.task_redactor(task).await;
        let mut room = FINDINGS_MAX_BYTES;
        let mut parts = Vec::new();
        for artifact in findings {
            if room < FINDINGS_PART_BYTES / 4 {
                break;
            }
            let Ok(hash) = artifact.id.parse() else {
                continue;
            };
            let Some(text) = self
                .core
                .store()
                .blobs()
                .get(hash)
                .await
                .ok()
                .flatten()
                .and_then(|bytes| String::from_utf8(bytes).ok())
            else {
                continue;
            };
            let text = match &redactor {
                Some(redactor) => redactor.redact(&text).into_owned(),
                None => text,
            };
            let kept = &text[..floor_boundary(&text, room)];
            room -= kept.len();
            let pieces = split_findings(kept);
            let count = pieces.len();
            for (index, piece) in pieces.into_iter().enumerate() {
                let mut title = format!(
                    "task-{} {} · {}",
                    task.number,
                    task.title,
                    one_line(&artifact.title, 120)
                );
                if let Some(heading) = &piece.heading {
                    title.push_str(&format!(" › {heading}"));
                }
                if count > 1 {
                    title.push_str(&format!(" (part {} of {count})", index + 1));
                }
                let body = format!(
                    "{}\n(from artifact {}, bytes {}–{} of {})",
                    piece.text.trim_end(),
                    artifact.id,
                    piece.start,
                    piece.end,
                    text.len()
                );
                parts.push(FindingsPart {
                    title: one_line(&title, 240),
                    body,
                });
            }
        }
        parts.truncate(FINDINGS_MAX_PARTS);
        parts
    }

    /// Something the user settled on a card (an answer, a plan decision): a decision node.
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
    /// decision node of the conversation, so this briefing's ledger and every later one carry
    /// it with an id. Returns how many it kept.
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
        let nodes: Vec<NewNode> = lines
            .into_iter()
            .map(|line| NewNode {
                kind: NodeKind::Decision,
                key: Some(format!("handoff:{}:{}", id.0, blake_key(&line))),
                title: one_line(&line, 240),
                body: format!("{line}\n(from the handoff note before rebirth {generation})"),
                provenance: Provenance {
                    origin: Origin::Orchestrator,
                    session_id: Some(id.0.clone()),
                    task_id: None,
                    job_id: None,
                    worker: None,
                    commit: commit.clone(),
                    recorded_at_ms: now_ms(),
                },
                files: Vec::new(),
                expires_at_ms: None,
            })
            .collect();
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

    /// A conversation was deleted: its transcript index goes, and with `forget` so does what
    /// the project's Brain and the Personal Brain learned in it.
    pub(crate) async fn forget_brain_conversation(
        &self,
        id: &ConversationId,
        project: Option<ProjectId>,
        forget: bool,
    ) {
        let mut brains = Vec::new();
        if let Some(project) = project
            && let Ok(project) = self.project_brain(&project).await
        {
            brains.push(project.brain.clone());
        }
        if forget && let Ok(personal) = self.personal_brain().await {
            brains.push(personal);
        }
        let session = id.0.clone();
        let forgotten = blocking(move || {
            let mut nodes = 0;
            for brain in brains {
                if forget {
                    nodes += brain.forget_session(&session).map_err(brain_error)?;
                } else {
                    brain.forget_transcript(&session).map_err(brain_error)?;
                }
            }
            Ok(nodes)
        })
        .await;
        match forgotten {
            Ok(nodes) => {
                tracing::info!(conversation = %id, nodes, forget, "the Brain let go of a deleted conversation")
            }
            Err(err) => {
                tracing::warn!(conversation = %id, error = %err, "could not clear a deleted conversation from the Brain")
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
        let jobs = match &project {
            Some(id) => self.brain_jobs(id).await,
            None => Vec::new(),
        };
        Ok(BrainOverview {
            project_id: project,
            stats,
            index: index.map(|index| index.status()),
            embedder: self.brains.embedder.status(),
            jobs,
        })
    }

    /// A project's Brain jobs, newest first.
    pub(crate) async fn brain_jobs(&self, id: &ProjectId) -> Vec<BrainJob> {
        let Ok(page) = self
            .core
            .store()
            .read_stream(
                streams::brain(id),
                StreamPage {
                    before: None,
                    kinds: vec!["brain.job".into()],
                    limit: 200,
                },
            )
            .await
        else {
            return Vec::new();
        };
        let mut jobs: Vec<BrainJob> = Vec::new();
        for stored in page {
            if let Ok(DomainEvent::BrainJobUpdated { job }) =
                serde_json::from_str::<DomainEvent>(stored.payload.get())
                && !jobs.iter().any(|known| known.id == job.id)
            {
                jobs.push(job);
            }
        }
        jobs
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
        "\n(from {:?}, {})",
        node.provenance.origin,
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

/// The node keys of the modules that hold `paths` (the deepest module per path).
async fn modules_of(project: &ProjectBrain, paths: &[String]) -> Vec<String> {
    let Some(index) = project.index.clone() else {
        return Vec::new();
    };
    let paths = paths.to_vec();
    blocking(move || {
        let map = index.project_map().map_err(index_error)?;
        let mut keys: Vec<String> = paths
            .iter()
            .filter_map(|path| {
                map.modules
                    .iter()
                    .filter(|module| {
                        module.path.is_empty()
                            || module.path == "."
                            || path.starts_with(&format!("{}/", module.path.trim_end_matches('/')))
                    })
                    .max_by_key(|module| module.path.len())
                    .map(|module| format!("module:{}", module.path))
            })
            .collect();
        keys.sort();
        keys.dedup();
        Ok(keys)
    })
    .await
    .unwrap_or_default()
}

/// Repository-relative `paths` with their indexed content hashes.
pub(super) async fn file_refs(index: Option<&CodeIndex>, paths: &[String]) -> Vec<FileRef> {
    let paths: Vec<String> = paths
        .iter()
        .map(|path| path.trim().trim_start_matches("./").to_owned())
        .filter(|path| !path.is_empty() && !path.starts_with('/'))
        .collect();
    if paths.is_empty() {
        return Vec::new();
    }
    let Some(index) = index.cloned() else {
        return paths
            .into_iter()
            .map(|path| FileRef { path, hash: None })
            .collect();
    };
    blocking(move || index.file_hashes(&paths).map_err(index_error))
        .await
        .map(|hashes| {
            hashes
                .into_iter()
                .map(|(path, hash)| FileRef { path, hash })
                .collect()
        })
        .unwrap_or_default()
}

/// Repository-relative paths a text mentions (`crates/core/src/lib.rs`, `src/app.tsx`).
fn mentioned_paths(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || matches!(c, '`' | '(' | ')' | ',' | ';' | '"' | '\''))
        .map(|word| word.trim_end_matches(['.', ':']))
        .filter(|word| {
            word.contains('/')
                && !word.starts_with('/')
                && !word.contains("://")
                && word
                    .rsplit('/')
                    .next()
                    .is_some_and(|name| name.contains('.') && !name.ends_with('.'))
        })
        .map(|word| word.split(':').next().unwrap_or(word).to_owned())
        .collect()
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

/// A report as the Brain keeps it: [`report_text`], then its artifacts, for `read_artifact`.
fn report_body(task: &Task, report: &Report) -> String {
    let mut text = report_text(task, report);
    let artifacts: Vec<&ArtifactRef> = report.artifacts.iter().chain(&task.outputs).collect();
    if !artifacts.is_empty() {
        text.push_str("\nArtifacts:");
        for artifact in artifacts {
            text.push_str(&format!(
                "\n- {} (artifact {})",
                one_line(&artifact.title, 120),
                artifact.id
            ));
        }
    }
    text
}

/// The cleanup-ledger owner of a project's Brain files.
/// A part of a report's findings files, as a Brain node's title and body.
struct FindingsPart {
    title: String,
    body: String,
}

/// The key of the `index`th findings part of a task's report.
fn part_key(task_id: &str, index: usize) -> String {
    format!("task:{task_id}:part:{}", index + 1)
}

/// Whether an artifact holds findings worth keeping in the Brain: a note or a markdown or
/// plain-text file, not a diff, transcript, command output, log or binary.
fn is_findings(artifact: &ArtifactRef) -> bool {
    let log = artifact
        .file_name
        .as_deref()
        .is_some_and(|name| name.ends_with(".log") || name.ends_with(".out"));
    matches!(artifact.kind, ArtifactKind::Note | ArtifactKind::File)
        && (artifact.mime == "text/markdown" || (artifact.mime == "text/plain" && !log))
}

/// Records a report, its findings parts and its decisions in one write. What an earlier
/// version of the report recorded and this one doesn't (surplus parts, dropped decisions)
/// loses the task's support, and goes unless something else supports it.
fn record_report_nodes(
    brain: &Brain,
    task_id: &str,
    node: NewNode,
    parts: Vec<NewNode>,
    decisions: Vec<NewNode>,
    modules: Vec<String>,
) -> Result<()> {
    let count = parts.len();
    let nodes: Vec<NewNode> = std::iter::once(node)
        .chain(parts)
        .chain(decisions)
        .collect();
    brain
        .refresh_task(task_id, nodes, move |ids| {
            let Some((report_id, rest)) = ids.split_first() else {
                return Vec::new();
            };
            let (parts, decisions) = rest.split_at(count.min(rest.len()));
            let mut edges = Vec::new();
            for module in &modules {
                edges.push(Edge {
                    from: report_id.clone(),
                    to: format!("key:{module}"),
                    kind: EdgeKind::About,
                });
            }
            for part_id in parts {
                edges.push(Edge {
                    from: report_id.clone(),
                    to: part_id.clone(),
                    kind: EdgeKind::Contains,
                });
            }
            for decision_id in decisions {
                edges.push(Edge {
                    from: decision_id.clone(),
                    to: report_id.clone(),
                    kind: EdgeKind::DecidedIn,
                });
                for module in &modules {
                    edges.push(Edge {
                        from: decision_id.clone(),
                        to: format!("key:{module}"),
                        kind: EdgeKind::About,
                    });
                }
            }
            edges
        })
        .map_err(brain_error)?;
    Ok(())
}

/// One part of a findings file: its text, where it sits in the file, and the heading it is
/// under.
#[derive(Debug, PartialEq)]
struct FindingsPiece {
    text: String,
    start: usize,
    end: usize,
    heading: Option<String>,
}

/// A findings file in parts of at most [`FINDINGS_PART_BYTES`], whole lines where it can: a
/// new part starts at a heading once the one before is half full, and a line longer than a
/// part is split between words. Each part knows the last heading before it.
fn split_findings(text: &str) -> Vec<FindingsPiece> {
    let mut pieces = Vec::new();
    let mut heading: Option<String> = None;
    let mut part_heading: Option<String> = None;
    let mut start = 0;
    let mut end = 0;
    let mut at = 0;
    let flush =
        |pieces: &mut Vec<FindingsPiece>, start: usize, end: usize, heading: &Option<String>| {
            if !text[start..end].trim().is_empty() {
                pieces.push(FindingsPiece {
                    text: text[start..end].to_owned(),
                    start,
                    end,
                    heading: heading.clone(),
                });
            }
        };
    for line in text.split_inclusive('\n') {
        let line_start = at;
        at += line.len();
        let title = line
            .trim_start()
            .strip_prefix('#')
            .map(|rest| one_line(rest.trim_start_matches('#'), 120));
        let full = end - start + line.len() > FINDINGS_PART_BYTES;
        let at_heading = title.is_some() && end - start >= FINDINGS_PART_BYTES / 2;
        if end > start && (full || at_heading) {
            flush(&mut pieces, start, end, &part_heading);
            start = line_start;
            end = line_start;
            part_heading.clone_from(&heading);
        }
        if let Some(title) = title.filter(|title| !title.is_empty()) {
            heading = Some(title);
            if end == start {
                part_heading.clone_from(&heading);
            }
        }
        if line.len() <= FINDINGS_PART_BYTES {
            end = at;
            continue;
        }
        // A line longer than a part: whole parts of it, split between words.
        let mut from = line_start;
        while at - from > FINDINGS_PART_BYTES {
            let limit = floor_boundary(&text[from..], FINDINGS_PART_BYTES);
            let cut = text[from..from + limit]
                .rfind([' ', '\t'])
                .filter(|cut| *cut >= limit / 2)
                .map_or(limit, |cut| cut + 1);
            flush(&mut pieces, from, from + cut, &part_heading);
            from += cut;
        }
        start = from;
        end = at;
    }
    flush(&mut pieces, start, end, &part_heading);
    pieces
}

/// The largest length of at most `max` bytes that ends on a character boundary of `text`.
fn floor_boundary(text: &str, max: usize) -> usize {
    if text.len() <= max {
        return text.len();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// A stable key part for a piece of text.
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

/// `code_search`, answered from `index`.
pub(crate) async fn code_search(index: CodeIndex, args: CodeSearch) -> Result<String> {
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
        return Ok("No symbol or file matches.".into());
    }
    let mut text = String::new();
    for hit in hits {
        match hit {
            CodeHit::Symbol { symbol } => {
                text.push_str(&format!(
                    "{} {} — {}:{}\n  {}\n",
                    symbol.kind, symbol.name, symbol.path, symbol.line, symbol.signature
                ));
            }
            CodeHit::File {
                path,
                language,
                bytes,
            } => text.push_str(&format!("file {path} ({language}, {bytes} bytes)\n")),
        }
    }
    Ok(text.trim_end().to_owned())
}

/// `code_refs`, answered from `index`.
pub(crate) async fn code_refs(index: CodeIndex, args: CodeRefs) -> Result<String> {
    let limit = args.limit.unwrap_or(50).clamp(1, 500);
    let refs = blocking(move || index.refs(&args.symbol, limit).map_err(index_error)).await?;
    if refs.definitions.is_empty() && refs.references.is_empty() {
        return Ok(format!("Nothing named `{}` is indexed.", refs.name));
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
    Ok(text.trim_end().to_owned())
}

/// `project_map`, answered from `index`.
pub(crate) async fn project_map(index: CodeIndex) -> Result<String> {
    blocking(move || index.digest(24_000).map_err(index_error)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDir(PathBuf);

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn report_node(key: String, body: &str) -> NewNode {
        NewNode {
            kind: NodeKind::Report,
            key: Some(key),
            title: "Worker findings".into(),
            body: body.into(),
            provenance: Provenance {
                origin: Origin::Report,
                session_id: None,
                task_id: Some("test".into()),
                job_id: None,
                worker: None,
                commit: None,
                recorded_at_ms: 1,
            },
            files: Vec::new(),
            expires_at_ms: None,
        }
    }

    #[tokio::test]
    async fn findings_survive_when_the_addendum_runs_before_the_original() {
        let dir = TestDir(
            std::env::temp_dir().join(format!("brigadier-report-order-{}", uuid::Uuid::new_v4())),
        );
        let brain = Brain::open(
            &dir.0.join("brain.sqlite"),
            Scope::Project,
            Embedder::new(dir.0.join("embeddings")),
        )
        .unwrap();
        let order = ReportLearning::default();
        // Like learn_report, reserve both versions before either future is polled.
        let original_version = order.next();
        let addendum_version = order.next();
        let original = async {
            record_report_nodes(
                &brain,
                "test",
                report_node("task:test".into(), "The findings are below."),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )
        };
        let addendum = || async {
            record_report_nodes(
                &brain,
                "test",
                report_node("task:test".into(), "The findings are in the addendum."),
                vec![report_node(part_key("test", 0), "The late findings.")],
                Vec::new(),
                Vec::new(),
            )
        };
        order.keep(addendum_version, addendum()).await.unwrap();
        let part = brain
            .node_by_key(NodeKind::Report, &part_key("test", 0))
            .unwrap()
            .unwrap();
        order.keep(original_version, original).await.unwrap();
        assert_eq!(
            brain
                .node_by_key(NodeKind::Report, "task:test")
                .unwrap()
                .unwrap()
                .body,
            "The findings are in the addendum."
        );
        assert_eq!(
            brain
                .node_by_key(NodeKind::Report, &part_key("test", 0))
                .unwrap()
                .unwrap(),
            part
        );
        // Learning the same augmented report again updates its existing nodes.
        order.keep(order.next(), addendum()).await.unwrap();
        let kept = brain
            .node_by_key(NodeKind::Report, &part_key("test", 0))
            .unwrap()
            .unwrap();
        assert_eq!(kept.id, part.id);
        assert_eq!(kept.body, "The late findings.");
        assert_eq!(brain.nodes(&NodeFilter::default()).unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_failed_report_write_can_be_retried() {
        let order = ReportLearning::default();
        let version = order.next();
        assert!(
            order
                .keep(version, async {
                    Err(Error::Invalid("write failed".into()))
                })
                .await
                .is_err()
        );
        let mut retried = false;
        order
            .keep(version, async {
                retried = true;
                Ok(())
            })
            .await
            .unwrap();
        assert!(retried);
    }
}
