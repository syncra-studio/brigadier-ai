//! The Project Brains and the code indexes the manager keeps open, and what feeds them.
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
use std::time::Duration;

use brigadier_brain::{
    Brain, BrainAnswer, BrainGraph, BrainQuery, Edge, EdgeKind, Embedder, EmbedderState, FileRef,
    NewNode, NodeFilter, NodeKind, Origin, Provenance, Scope,
};
use brigadier_index::{
    CodeHit, CodeIndex, CodeQuery, FileChange, IndexConfig, ScanHelper, SearchKind,
};
use brigadier_sandbox::removal;

use super::{SessionManager, blocking};
use crate::knowledge::{BrainOverview, ConventionsExport};
use crate::model::{ConversationId, Project, ProjectId, Setup};
use crate::tools::{CodeRefs, CodeSearch};
use crate::{Error, Result, now_ms};

/// Removing a project waits at most this long for its code index scan to end.
const REMOVE_SCAN_WAIT: Duration = Duration::from_secs(120);
/// The embedding model is freed after this long unused.
const EMBEDDER_IDLE: Duration = Duration::from_secs(10 * 60);
/// `query_brain` timings kept for the Inspector's p95.
const QUERY_SAMPLES: usize = 1_000;
/// Nodes embedded per maintenance round, once the model is loaded.
const EMBED_BATCH: u32 = 256;
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
    projects: Mutex<HashMap<ProjectId, Arc<ProjectBrain>>>,
    query_ms: Mutex<VecDeque<f64>>,
    largest_run: Mutex<Option<IndexRunStats>>,
    downloading: AtomicBool,
    cancel_download: Arc<AtomicBool>,
}

impl Brains {
    pub(crate) fn new(data_dir: &Path) -> Self {
        let root = data_dir.join("brains");
        Self {
            embedder: Embedder::new(data_dir.join("models").join("embeddings")),
            root,
            projects: Mutex::new(HashMap::new()),
            query_ms: Mutex::new(VecDeque::new()),
            largest_run: Mutex::new(None),
            downloading: AtomicBool::new(false),
            cancel_download: Arc::new(AtomicBool::new(false)),
        }
    }

    fn projects(&self) -> MutexGuard<'_, HashMap<ProjectId, Arc<ProjectBrain>>> {
        self.projects.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(super) fn note_query(&self, ms: f64) {
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
        // Its repositories are what sharing reads and watches.
        self.plugin_projects_changed();
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
            // What the index and the skeleton passes (older Brigadiers' idle passes too)
            // learned describes the old repository: it goes before the new one is scanned.
            // Sessions' decisions and reports stay.
            if old.root.is_some() {
                let brain = old.brain.clone();
                let forgotten = blocking(move || {
                    brain
                        .forget_origins(&[Origin::Index, Origin::Skeleton])
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
        let id = id.clone();
        self.spawn(async move {
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

    pub async fn brain_overview(&self, project: ProjectId) -> Result<BrainOverview> {
        let opened = self.project_brain(&project).await?;
        let (brain, index) = (opened.brain.clone(), opened.index.clone());
        let stats = blocking(move || brain.stats().map_err(brain_error)).await?;
        Ok(BrainOverview {
            project_id: project,
            stats,
            index: index.map(|index| index.status()),
            embedder: self.brains.embedder.status(),
        })
    }

    pub async fn query_brain(&self, project: ProjectId, query: BrainQuery) -> Result<BrainAnswer> {
        let brain = self.project_brain(&project).await?.brain.clone();
        blocking(move || brain.query(&query).map_err(brain_error)).await
    }

    pub async fn brain_graph(&self, project: ProjectId, filter: NodeFilter) -> Result<BrainGraph> {
        let brain = self.project_brain(&project).await?.brain.clone();
        blocking(move || brain.graph(&filter).map_err(brain_error)).await
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
/// described since (a skeleton pass) keeps its description.
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
