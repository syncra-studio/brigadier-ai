//! Brain jobs: a cheap model deepening a project's Brain in the background.
//!
//! - The **skeleton pass** runs once a project's repository is first indexed: each module's
//!   purpose, the stack, conventions, the run/build/verify recipe and the central files.
//! - **Idle-quota enrichment** (Settings, on by default) spends quota that would otherwise go
//!   unused: when a provider's usage window resets within the hour with plenty left and the
//!   provider has been idle for a while, a job refreshes stale nodes first, then fills gaps. It
//!   yields as soon as the user's own work starts on that provider, the window runs hot, or it
//!   resets.
//!
//! A job is a sandboxed, read-only CLI session on the provider's cheapest model, with the code
//! index's digest up front and the Brigadier MCP tools of a job (`record_nodes` and the code
//! index); its cleanup-ledger owner is `brain:<job>`. One job runs at a time.
//!
//! Chats' `save_memory` lands here too: a preference in the Personal Brain, shown in the Chat
//! as a Memory chip.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use brigadier_brain::{
    Edge, EdgeKind, NewNode, NodeFilter, NodeKind, NodeState, Origin, Provenance, WorkerRef,
};
use brigadier_providers::codex;
use brigadier_providers::policy::{self, ApprovalMode, Route as PolicyRoute};
use brigadier_providers::{
    Access, ApprovalDecision, Origin as SessionOrigin, ProviderEvent, ProviderKind, QuotaSnapshot,
    SessionSpec, Started, ToolSet, TurnInput, TurnStatus,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::brains::{
    ProjectBrain, blake_key, brain_error, code_refs, code_search, file_refs, one_line, project_map,
};
use super::usage::TokenOwner;
use super::{SessionManager, blocking, git_error, secrets};
use crate::knowledge::{BrainJob, BrainJobKind, BrainJobState, MemoryChange};
use crate::model::{ConversationId, DomainEvent, ProjectId, streams};
use crate::routing::TokenMeter;
use crate::tools::{ChatCall, JobCall, NodeInput, Role, SaveMemory, ToolReply};
use crate::{Error, Result, now_ms};

/// How long a skeleton pass may run.
const SKELETON_TIME: Duration = Duration::from_secs(5 * 60);
/// How long an enrichment job may run.
const ENRICH_TIME: Duration = Duration::from_secs(10 * 60);
/// The code index digest a job starts from, in bytes.
const DIGEST_BYTES: usize = 20_000;
/// A job's Brigadier tools answer within this.
const JOB_TOOL_TIMEOUT_SECS: u64 = 120;
/// How long a skeleton pass that could not start waits before it is asked for again.
const SKELETON_RETRY: Duration = Duration::from_secs(5 * 60);
/// Enrichment waits until a provider has run none of the user's work for this long.
pub(super) const ENRICH_IDLE_MS: i64 = 10 * 60 * 1000;
/// … and a usage window resets within this …
const ENRICH_RESET_WITHIN_MS: i64 = 60 * 60 * 1000;
/// … with at most this much of it used …
const ENRICH_MAX_USED: f64 = 60.0;
/// … and no window of the provider past this. A running job yields past it too.
const ENRICH_HOT: f64 = 85.0;
/// Stale nodes and gaps listed in an enrichment job's prompt.
const ENRICH_TARGETS: usize = 40;
/// The user's memories a Chat's or orchestrator's instructions carry, in bytes.
pub(super) const MEMORY_BYTES: usize = 6_000;

/// The running job and the skeleton queue.
pub(crate) struct BrainJobs {
    live: Mutex<Option<JobLive>>,
    skeletons: mpsc::UnboundedSender<ProjectId>,
    queued: Mutex<Option<mpsc::UnboundedReceiver<ProjectId>>>,
    /// Projects whose skeleton pass this daemon run already tried.
    tried: Mutex<HashMap<ProjectId, u32>>,
    /// Since when each provider has run none of the user's work.
    idle_since: Mutex<HashMap<ProviderKind, i64>>,
    /// The usage window reset each project was last enriched before, so one window enriches a
    /// project once.
    enriched: Mutex<HashMap<ProjectId, i64>>,
}

struct JobLive {
    job: BrainJob,
    cancel: CancellationToken,
    /// Why it is being stopped.
    stop: Option<String>,
    /// For enrichment the scheduler started: the usage window it spends, which it yields at.
    window_resets_at_ms: Option<i64>,
    commit: Option<String>,
    /// The project's Brain and index as the job started: its findings go there, and only while
    /// the project still has them open (a changed repository replaces them).
    project: Arc<ProjectBrain>,
}

impl BrainJobs {
    pub(crate) fn new() -> Self {
        let (skeletons, queued) = mpsc::unbounded_channel();
        Self {
            live: Mutex::new(None),
            skeletons,
            queued: Mutex::new(Some(queued)),
            tried: Mutex::new(HashMap::new()),
            idle_since: Mutex::new(HashMap::new()),
            enriched: Mutex::new(HashMap::new()),
        }
    }

    fn live(&self) -> MutexGuard<'_, Option<JobLive>> {
        self.live.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Whether a job runs now.
    pub(crate) fn running(&self) -> bool {
        self.live().is_some()
    }

    /// Asks for a project's skeleton pass (from the indexing thread, after its first scan).
    pub(crate) fn want_skeleton(&self, project: ProjectId) {
        let _ = self.skeletons.send(project);
    }

    /// The project moved to another repository: its skeleton pass gets its tries again.
    pub(crate) fn forget_tries(&self, project: &ProjectId) {
        self.tried
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(project);
    }

    /// Stops the running job, if any.
    pub(crate) fn stop(&self, reason: &str) {
        if let Some(live) = self.live().as_mut() {
            live.stop.get_or_insert_with(|| reason.to_owned());
            live.cancel.cancel();
        }
    }

    /// The user's work started on `provider`: it is no longer idle, and an enrichment job on it
    /// yields now rather than at the next upkeep tick.
    pub(crate) fn user_work(&self, provider: ProviderKind) {
        self.idle_since
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(provider, now_ms());
        if let Some(live) = self.live().as_mut().filter(|live| {
            live.job.kind == BrainJobKind::Enrichment && live.job.provider == provider
        }) {
            live.stop
                .get_or_insert_with(|| format!("your work started on {provider}"));
            live.cancel.cancel();
        }
    }

    /// Stops the running job if it works on `project`.
    pub(crate) fn stop_for(&self, project: &ProjectId, reason: &str) {
        if let Some(live) = self
            .live()
            .as_mut()
            .filter(|live| live.job.project_id == *project)
        {
            live.stop.get_or_insert_with(|| reason.to_owned());
            live.cancel.cancel();
        }
    }
}

impl SessionManager {
    /// Starts the consumer that runs the skeleton passes asked for, one at a time.
    pub(super) fn start_skeleton_queue(&self) {
        let Some(mut queued) = self
            .brains
            .jobs
            .queued
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        else {
            return;
        };
        let manager = self.me.clone();
        self.spawn(async move {
            while let Some(project) = queued.recv().await {
                let Some(manager) = manager.upgrade() else {
                    return;
                };
                manager.skeleton_if_needed(project).await;
            }
        });
    }

    /// Whether the project's Brain still holds nodes a skeleton pass wrote.
    async fn skeleton_kept(&self, project: &ProjectId) -> bool {
        let Ok(open) = self.project_brain(project).await else {
            return true;
        };
        let brain = open.brain.clone();
        blocking(move || brain.holds_origin(Origin::Skeleton).map_err(brain_error))
            .await
            .unwrap_or(true)
    }

    /// Runs the project's skeleton pass unless one succeeded before (or this daemon run
    /// already tried it twice).
    async fn skeleton_if_needed(&self, project: ProjectId) {
        // A project removed since it asked has nothing to map.
        if self.core.project(&project).is_err() {
            return;
        }
        let jobs = self.brain_jobs(&project).await;
        let live = self
            .brains
            .jobs
            .live()
            .as_ref()
            .map(|live| live.job.id.clone());
        // A job the log shows running that is not live was cut short by a restart.
        for job in jobs.iter().filter(|job| {
            job.state == BrainJobState::Running && live.as_deref() != Some(job.id.as_str())
        }) {
            let mut ended = job.clone();
            ended.state = BrainJobState::Failed {
                error: "Brigadier stopped while it ran".into(),
            };
            ended.ended_at_ms = Some(now_ms());
            self.record_job(&ended).await;
        }
        // Done, unless what it wrote is gone (the project moved to another repository).
        if jobs
            .iter()
            .any(|job| job.kind == BrainJobKind::Skeleton && job.state == BrainJobState::Done)
            && self.skeleton_kept(&project).await
        {
            return;
        }
        if self.skeleton_tries(&project) >= 2 {
            return;
        }
        // One job at a time: wait for a running one to end.
        while self.brains.jobs.live().is_some() {
            if self.admit().is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        let Some(provider) = self.job_provider(false) else {
            tracing::info!(project = %project, "no provider can run the skeleton pass now; asking again later");
            self.retry_skeleton(project);
            return;
        };
        match self
            .start_brain_job(&project, BrainJobKind::Skeleton, provider, None)
            .await
        {
            Ok(id) => {
                // Only a pass that started counts as an attempt.
                *self
                    .brains
                    .jobs
                    .tried
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .entry(project.clone())
                    .or_default() += 1;
                // A failed pass is retried once in this daemon run.
                let ended = loop {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    if self
                        .brains
                        .jobs
                        .live()
                        .as_ref()
                        .is_none_or(|live| live.job.id != id)
                    {
                        break self
                            .brain_jobs(&project)
                            .await
                            .into_iter()
                            .find(|job| job.id == id);
                    }
                };
                if ended.is_some_and(|job| job.state != BrainJobState::Done) {
                    let _ = self.brains.jobs.skeletons.send(project);
                }
            }
            Err(err) => {
                tracing::warn!(project = %project, error = %err, "could not start the skeleton pass; asking again later");
                self.retry_skeleton(project);
            }
        }
    }

    /// How many skeleton passes this daemon run started for the project.
    fn skeleton_tries(&self, project: &ProjectId) -> u32 {
        self.brains
            .jobs
            .tried
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(project)
            .copied()
            .unwrap_or_default()
    }

    /// Asks for the project's skeleton pass again after [`SKELETON_RETRY`].
    fn retry_skeleton(&self, project: ProjectId) {
        let manager = self.me.clone();
        self.spawn(async move {
            tokio::time::sleep(SKELETON_RETRY).await;
            if let Some(manager) = manager.upgrade() {
                manager.brains.jobs.want_skeleton(project);
            }
        });
    }

    /// Starts a Brain job now (the Inspector's "Enrich now", a skeleton pass redone).
    pub async fn run_brain_job(&self, project: ProjectId, kind: BrainJobKind) -> Result<String> {
        self.admit()?;
        if self.brains.jobs.live().is_some() {
            return Err(Error::Invalid("a Brain job is already running".into()));
        }
        let provider = self
            .job_provider(false)
            .ok_or_else(|| Error::Invalid("no provider is signed in with quota to spare".into()))?;
        self.start_brain_job(&project, kind, provider, None).await
    }

    /// The signed-in provider with the most quota left; with `known_only`, only one whose
    /// usage windows are known. Codex qualifies only where its sandbox can keep the job out of
    /// the run folder (the IPC token): a job's scratch folder outside any repository.
    fn job_provider(&self, known_only: bool) -> Option<ProviderKind> {
        let scratch = self.owned_dir("scratch", "");
        ProviderKind::ALL
            .into_iter()
            .filter(|kind| self.provider_usable(*kind) && self.cheapest(*kind).is_ok())
            .filter(|kind| *kind != ProviderKind::Codex || codex::can_deny_reads(true, &scratch))
            .filter_map(|kind| {
                let quota = self
                    .runtime
                    .overview(kind)
                    .and_then(|overview| overview.quota);
                // A window that has reset since it was reported counts as unused.
                let now = now_ms();
                let left = quota
                    .as_ref()
                    .filter(|quota| !quota.windows.is_empty())
                    .map(|quota| {
                        100.0
                            - quota
                                .windows
                                .iter()
                                .filter(|window| window.resets_at_ms.is_none_or(|at| at > now))
                                .map(|window| window.used_percent)
                                .fold(0.0, f64::max)
                    });
                match (left, known_only) {
                    (None, true) => None,
                    (None, false) => Some((kind, 50.0)),
                    // Used up: a job would only fail on the limit (the skeleton pass asks again
                    // later).
                    (Some(left), _) if left <= 0.0 => None,
                    (Some(left), _) => Some((kind, left)),
                }
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(kind, _)| kind)
    }

    async fn record_job(&self, job: &BrainJob) {
        let event = DomainEvent::BrainJobUpdated { job: job.clone() };
        if let Err(err) = self
            .core
            .record(vec![(streams::brain(&job.project_id), event)])
            .await
        {
            tracing::warn!(job = %job.id, error = %err, "could not record a Brain job");
        }
    }

    async fn start_brain_job(
        &self,
        project: &ProjectId,
        kind: BrainJobKind,
        provider: ProviderKind,
        window_resets_at_ms: Option<i64>,
    ) -> Result<String> {
        let brain = self.project_brain(project).await?;
        let (Some(index), Some(root)) = (brain.index.clone(), brain.root.clone()) else {
            return Err(Error::Invalid("this project has no repository".into()));
        };
        let (model, effort) = self.cheapest(provider)?;
        let commit = {
            let (git, root) = (self.git.clone(), root.clone());
            blocking(move || {
                let repo = git.open(&root).map_err(git_error)?;
                Ok(repo.resolve("HEAD").map_err(git_error)?.0)
            })
            .await
            .ok()
        };
        let digest = {
            let index = index.clone();
            blocking(move || {
                index
                    .digest(DIGEST_BYTES)
                    .map_err(super::brains::index_error)
            })
            .await?
        };
        let targets = match kind {
            BrainJobKind::Skeleton => String::new(),
            BrainJobKind::Enrichment => {
                let brain = brain.brain.clone();
                blocking(move || enrichment_targets(&brain)).await?
            }
        };
        let job = BrainJob {
            id: uuid::Uuid::now_v7().to_string(),
            project_id: project.clone(),
            kind,
            state: BrainJobState::Running,
            provider,
            model: Some(model.clone()),
            started_at_ms: now_ms(),
            ended_at_ms: None,
            nodes: 0,
            note: match kind {
                BrainJobKind::Skeleton => {
                    "Mapping modules, stack, conventions and the recipe".into()
                }
                BrainJobKind::Enrichment => "Refreshing stale nodes, then filling gaps".into(),
            },
        };
        let cancel = CancellationToken::new();
        // Enrichment the scheduler chose is checked once more as it starts: the user's work
        // may have begun, or the window run hot, since it looked.
        if window_resets_at_ms.is_some() {
            let now = now_ms();
            let idle_since = self
                .brains
                .jobs
                .idle_since
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(&provider)
                .copied()
                .unwrap_or(now);
            let hot = self
                .runtime
                .overview(provider)
                .and_then(|overview| overview.quota)
                .is_some_and(|quota| hottest(&quota) >= ENRICH_HOT);
            if now - idle_since < ENRICH_IDLE_MS || hot {
                return Err(Error::Invalid(format!(
                    "{provider} is no longer idle with quota to spare"
                )));
            }
        }
        {
            let mut live = self.brains.jobs.live();
            if live.is_some() {
                return Err(Error::Invalid("a Brain job is already running".into()));
            }
            *live = Some(JobLive {
                job: job.clone(),
                cancel: cancel.clone(),
                stop: None,
                window_resets_at_ms,
                commit,
                project: brain.clone(),
            });
        }
        self.record_job(&job).await;
        let prompt = job_prompt(kind, provider, &root, &digest, &targets);
        let manager = self.arc();
        let id = job.id.clone();
        self.spawn(async move {
            let outcome = manager
                .run_job(&job, provider, model, effort, root, prompt, cancel)
                .await;
            let mut ended = manager
                .brains
                .jobs
                .live()
                .take()
                .map_or(job.clone(), |live| {
                    let mut ended = live.job;
                    if let Some(reason) = live.stop {
                        ended.state = BrainJobState::Stopped { reason };
                    }
                    ended
                });
            if ended.state == BrainJobState::Running {
                ended.state = match outcome {
                    Ok(()) if ended.nodes == 0 => BrainJobState::Failed {
                        error: "it recorded nothing".into(),
                    },
                    Ok(()) => BrainJobState::Done,
                    Err(err) => BrainJobState::Failed {
                        error: err.to_string(),
                    },
                };
            }
            ended.ended_at_ms = Some(now_ms());
            tracing::info!(job = %ended.id, kind = ?ended.kind, nodes = ended.nodes, state = ?ended.state, "brain job ended");
            manager.record_job(&ended).await;
        });
        Ok(id)
    }

    /// The job's CLI session, from start to cleanup.
    #[allow(clippy::too_many_arguments)]
    async fn run_job(
        &self,
        job: &BrainJob,
        provider: ProviderKind,
        model: String,
        effort: Option<String>,
        root: PathBuf,
        prompt: String,
        cancel: CancellationToken,
    ) -> Result<()> {
        let owner = format!("brain:{}", job.id);
        let scratch = self.owned_dir("scratch", &format!("brain-{}", job.id));
        self.prepare_owned_dir(&owner, &scratch).await?;
        let grant = self.grants.issue(
            &owner,
            Role::BrainJob {
                project_id: job.project_id.clone(),
                job_id: job.id.clone(),
            },
        );
        // Codex cannot run with a read-only cwd: it works from the scratch folder and reads
        // the repository by path, as read-only workers do. There its sandbox is a permission
        // profile that denies reading the run folder, or the session does not start.
        let codex = provider == ProviderKind::Codex;
        let run_dir = self.runtime.platform().paths().run_dir.clone();
        let spec = SessionSpec {
            cwd: if codex { scratch.clone() } else { root },
            model: Some(model),
            effort,
            fast: false,
            origin: SessionOrigin::New,
            access: Access::Scoped {
                write_cwd: codex,
                writable_roots: if codex {
                    Vec::new()
                } else {
                    vec![scratch.clone()]
                },
                network: false,
                deny_read: vec![run_dir],
                unix_sockets: self.socket_path().into_iter().collect(),
            },
            append_system_prompt: Some(JOB_ROLE.to_owned()),
            mcp_servers: vec![self.brigadier_server(grant.clone(), JOB_TOOL_TIMEOUT_SECS, true)],
            tools: ToolSet::Default,
            add_dirs: Vec::new(),
            env: vec![("TMPDIR".into(), scratch.to_string_lossy().into_owned())],
            unset_env: Vec::new(),
            low_priority: false,
            record_to: None,
            redactor: secrets::redactor(vec![grant]),
            owned_cwd: codex,
            auto_compact: true,
            // A Brain job hands nothing on: no sub-agents at all (Codex runs without them,
            // Claude without its Agent tool).
            allowed_models: Some(brigadier_providers::AllowedModels::default()),
            auto_review: false,
            omit_ai_coauthors: false,
            output_hook: None,
        };
        let access = spec.access.clone();
        let time = match job.kind {
            BrainJobKind::Skeleton => SKELETON_TIME,
            BrainJobKind::Enrichment => ENRICH_TIME,
        };
        let ran = async {
            let Started {
                session,
                mut events,
            } = self.runtime.start_hosted(&owner, provider, spec).await?;
            let meter = TokenMeter::default();
            let turn = async {
                session
                    .send(TurnInput::text(prompt))
                    .await
                    .map_err(|err| Error::Provider(format!("the job didn't start: {err}")))?;
                while let Some(event) = events.recv().await {
                    match event {
                        ProviderEvent::RateLimits { quota } => {
                            self.runtime
                            .note_quota_snapshot(&crate::accounts::AccountRef::own(quota.provider), quota.clone())
                            .await;
                        }
                        ProviderEvent::Usage { total, last } => self.note_tokens(
                            &meter,
                            provider,
                            job.model.as_deref(),
                            TokenOwner::Project(&job.project_id),
                            &total,
                            last.as_ref(),
                        ).await,
                        ProviderEvent::ApprovalRequested { request } => {
                            // It stays inside its sandbox; nobody is asked on its behalf.
                            let decision = match policy::route(&request, &access, ApprovalMode::Delegated) {
                                PolicyRoute::Allow => ApprovalDecision::Allow,
                                _ => ApprovalDecision::Deny {
                                    message: "Declined: a Brain job only reads the repository and records nodes.".into(),
                                },
                            };
                            let _ = session.answer(request.id, decision).await;
                        }
                        ProviderEvent::TurnCompleted { status, .. } => {
                            // A turn cut short leaves the job incomplete, whatever it recorded.
                            return match status {
                                TurnStatus::Completed => Ok(()),
                                TurnStatus::Failed => {
                                    Err(Error::Provider("the job's turn failed".into()))
                                }
                                TurnStatus::Interrupted => {
                                    Err(Error::Provider("the job's turn was interrupted".into()))
                                }
                            };
                        }
                        ProviderEvent::Exited { .. } => {
                            return Err(Error::Provider("the job's CLI exited".into()));
                        }
                        _ => {}
                    }
                }
                Err(Error::Provider("the job's CLI went away".into()))
            };
            let outcome = tokio::select! {
                outcome = tokio::time::timeout(time, turn) => outcome.unwrap_or_else(|_| {
                    self.brains.jobs.stop(&format!("its {}-minute time box ran out", time.as_secs() / 60));
                    Ok(())
                }),
                () = cancel.cancelled() => Ok(()),
            };
            session.close().await;
            outcome
        }
        .await;
        self.grants.revoke_owner(&owner);
        let _ = self.runtime.ledger().dispose(&owner).await;
        let _ = tokio::fs::remove_dir_all(&scratch).await;
        ran
    }

    /// A Brain job's tool call.
    pub(super) async fn job_call(
        &self,
        project: ProjectId,
        job_id: String,
        call: JobCall,
    ) -> ToolReply {
        let live = self
            .brains
            .jobs
            .live()
            .as_ref()
            .filter(|live| live.job.id == job_id && live.job.project_id == project)
            .filter(|live| {
                self.brains
                    .open_project(&project)
                    .is_some_and(|open| Arc::ptr_eq(&open, &live.project))
            })
            .map(|live| (live.job.clone(), live.commit.clone(), live.project.clone()));
        let Some((job, commit, brain)) = live else {
            return ToolReply::error("This Brain job has ended.");
        };
        let index = brain
            .index
            .clone()
            .ok_or_else(|| Error::Invalid("this project has no repository".into()));
        let result = match call {
            JobCall::RecordNodes(args) => {
                self.record_job_nodes(&job, &brain, commit, args.nodes)
                    .await
            }
            JobCall::CodeSearch(args) => match index {
                Ok(index) => code_search(index, args).await,
                Err(err) => Err(err),
            },
            JobCall::CodeRefs(args) => match index {
                Ok(index) => code_refs(index, args).await,
                Err(err) => Err(err),
            },
            JobCall::ProjectMap => match index {
                Ok(index) => project_map(index).await,
                Err(err) => Err(err),
            },
        };
        match result {
            Ok(text) => ToolReply::ok(text),
            Err(err) => ToolReply::error(err.to_string()),
        }
    }

    /// `record_nodes`: the job's findings, keyed so a later job updates them in place.
    async fn record_job_nodes(
        &self,
        job: &BrainJob,
        project: &ProjectBrain,
        commit: Option<String>,
        inputs: Vec<NodeInput>,
    ) -> Result<String> {
        if inputs.is_empty() {
            return Err(Error::Invalid("pass at least one node in `nodes`".into()));
        }
        let modules: Vec<String> = match project.index.clone() {
            Some(index) => blocking(move || {
                Ok(index
                    .project_map()
                    .map(|map| map.modules.into_iter().map(|module| module.path).collect())
                    .unwrap_or_default())
            })
            .await
            .unwrap_or_default(),
            None => Vec::new(),
        };
        let provenance = Provenance {
            origin: match job.kind {
                BrainJobKind::Skeleton => Origin::Skeleton,
                BrainJobKind::Enrichment => Origin::Enrichment,
            },
            session_id: None,
            task_id: None,
            job_id: Some(job.id.clone()),
            worker: Some(WorkerRef {
                provider: job.provider.binary().into(),
                model: job.model.clone(),
            }),
            commit,
            recorded_at_ms: now_ms(),
        };
        let mut nodes = Vec::new();
        let mut summaries = Vec::new();
        let mut refused = Vec::new();
        for input in inputs {
            let title = one_line(&input.title, 240);
            let body = input.body.trim().to_owned();
            if title.is_empty() || body.is_empty() {
                refused.push(format!("a node without a title or body ({})", input.kind));
                continue;
            }
            let path = input
                .path
                .as_deref()
                .map(clean_path)
                .filter(|path| !path.is_empty() || input.kind == "module");
            let (kind, key) = match (input.kind.as_str(), &path) {
                ("module", Some(path)) => {
                    let known = modules
                        .iter()
                        .find(|module| clean_path(module) == *path)
                        .cloned()
                        .unwrap_or_else(|| path.clone());
                    (NodeKind::Module, format!("module:{known}"))
                }
                ("fileSummary", Some(path)) => (NodeKind::FileSummary, format!("file:{path}")),
                ("service", _) => (
                    NodeKind::Service,
                    format!("service:{}", path.clone().unwrap_or_else(|| title.clone())),
                ),
                ("convention", _) => (
                    NodeKind::Convention,
                    format!("convention:{}", blake_key(&title.to_lowercase())),
                ),
                ("contract", _) => (
                    NodeKind::Contract,
                    format!("contract:{}", blake_key(&title.to_lowercase())),
                ),
                ("decision", _) => (
                    NodeKind::Decision,
                    format!("decision:{}", blake_key(&title.to_lowercase())),
                ),
                ("module" | "fileSummary", None) => {
                    refused.push(format!("\"{title}\": a {} needs its `path`", input.kind));
                    continue;
                }
                (other, _) => {
                    refused.push(format!("\"{title}\": unknown kind \"{other}\""));
                    continue;
                }
            };
            let mut files = input.files.clone();
            if kind == NodeKind::FileSummary
                && let Some(path) = &path
            {
                files.push(path.clone());
            }
            files.sort();
            files.dedup();
            let files = file_refs(project.index.as_ref(), &files).await;
            if kind == NodeKind::FileSummary {
                summaries.push((key.clone(), path.clone().unwrap_or_default()));
            }
            let replaces = input
                .replaces
                .as_deref()
                .map(str::trim)
                .filter(|old| !old.is_empty())
                .map(|old| {
                    // A key as listed in the prompt (`convention:…`), or a node id.
                    if old.contains(':') && !old.starts_with("key:") {
                        format!("key:{old}")
                    } else {
                        old.to_owned()
                    }
                });
            nodes.push((
                NewNode {
                    kind,
                    key: Some(key),
                    title,
                    body,
                    provenance: provenance.clone(),
                    files,
                    expires_at_ms: None,
                },
                replaces,
            ));
        }
        let recorded = nodes.len() as u32;
        let brain = project.brain.clone();
        let reason = format!(
            "replaced by the {} job {}",
            match job.kind {
                BrainJobKind::Skeleton => "skeleton",
                BrainJobKind::Enrichment => "enrichment",
            },
            job.id
        );
        let unreplaced = blocking(move || {
            let mut unreplaced = Vec::new();
            for (node, replaces) in nodes {
                match replaces {
                    Some(old) => {
                        if let Err(err) = brain.record_replacing(node.clone(), &old, &reason) {
                            // A wrong `replaces` doesn't lose the finding.
                            unreplaced.push(format!("\"{}\" replaces nothing ({err})", node.title));
                            brain.record(node).map_err(brain_error)?;
                        }
                    }
                    None => {
                        brain.record(node).map_err(brain_error)?;
                    }
                }
            }
            // Each file summary belongs to the deepest module above it that the Brain knows.
            let mut edges = Vec::new();
            for (key, path) in summaries {
                let module = modules
                    .iter()
                    .filter(|module| {
                        let module = clean_path(module);
                        module.is_empty() || path.starts_with(&format!("{module}/"))
                    })
                    .max_by_key(|module| clean_path(module).len());
                if let Some(module) = module
                    && brain
                        .node_by_key(NodeKind::Module, &format!("module:{module}"))
                        .map_err(brain_error)?
                        .is_some()
                {
                    edges.push(Edge {
                        from: format!("key:module:{module}"),
                        to: format!("key:{key}"),
                        kind: EdgeKind::Contains,
                    });
                }
            }
            if !edges.is_empty() {
                brain.link(edges).map_err(brain_error)?;
            }
            Ok(unreplaced)
        })
        .await?;
        let snapshot = {
            let mut live = self.brains.jobs.live();
            live.as_mut()
                .filter(|live| live.job.id == job.id)
                .map(|live| {
                    live.job.nodes += recorded;
                    live.job.clone()
                })
        };
        if let Some(snapshot) = snapshot {
            self.record_job(&snapshot).await;
        }
        let mut reply = format!("Recorded {recorded} node(s).");
        if !refused.is_empty() {
            reply.push_str(&format!(" Not recorded: {}.", refused.join("; ")));
        }
        if !unreplaced.is_empty() {
            reply.push_str(&format!(" Recorded as new: {}.", unreplaced.join("; ")));
        }
        Ok(reply)
    }

    /// How long no conversation or worker has been at work on `kind` (as of the last tick).
    pub(super) fn provider_idle_ms(&self, kind: ProviderKind, now: i64) -> i64 {
        now - self
            .brains
            .jobs
            .idle_since
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&kind)
            .copied()
            .unwrap_or(now)
    }

    /// Each minute: starts enrichment when a provider has quota to spare and is idle, and
    /// makes a running enrichment yield when that stops being true.
    pub(super) async fn brain_job_tick(&self) {
        let now = now_ms();
        let mut busy = Vec::new();
        let convs: Vec<_> = self.convs_lock().values().cloned().collect();
        for conv in convs {
            busy.extend(conv.busy_provider().await);
        }
        let tasks: Vec<_> = self.tasks_lock().values().cloned().collect();
        for task in tasks {
            busy.extend(task.busy_provider().await);
        }
        {
            let mut idle = self
                .brains
                .jobs
                .idle_since
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            for kind in ProviderKind::ALL {
                if busy.contains(&kind) {
                    idle.insert(kind, now);
                } else {
                    idle.entry(kind).or_insert(now);
                }
            }
        }
        let enrich = self.core.settings().enrich_brain;
        let running = self
            .brains
            .jobs
            .live()
            .as_ref()
            .map(|live| (live.job.kind, live.job.provider, live.window_resets_at_ms));
        if let Some((kind, provider, resets_at)) = running {
            if kind != BrainJobKind::Enrichment {
                return;
            }
            let quota = self
                .runtime
                .overview(provider)
                .and_then(|overview| overview.quota);
            let reason = if busy.contains(&provider) {
                Some(format!("your work started on {provider}"))
            } else if resets_at.is_some() && !enrich {
                Some("enrichment was turned off in Settings".into())
            } else if resets_at.is_some()
                && quota.as_ref().is_some_and(|q| hottest(q) >= ENRICH_HOT)
            {
                Some("the usage window is nearly used up".into())
            } else if resets_at.is_some_and(|at| now >= at) {
                Some("the usage window reset".into())
            } else {
                None
            };
            if let Some(reason) = reason {
                self.brains.jobs.stop(&reason);
            }
            return;
        }
        if !enrich || self.admit().is_err() {
            return;
        }
        let Some((provider, resets_at)) = ProviderKind::ALL.into_iter().find_map(|kind| {
            let idle_for = now
                - self
                    .brains
                    .jobs
                    .idle_since
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .get(&kind)
                    .copied()
                    .unwrap_or(now);
            if idle_for < ENRICH_IDLE_MS
                || !self.provider_usable(kind)
                || self.cheapest(kind).is_err()
            {
                return None;
            }
            let quota = self.runtime.overview(kind)?.quota?;
            spare_window(&quota, now).map(|resets_at| (kind, resets_at))
        }) else {
            return;
        };
        let Some(project) = self.enrichment_project(resets_at).await else {
            return;
        };
        match self
            .start_brain_job(
                &project,
                BrainJobKind::Enrichment,
                provider,
                Some(resets_at),
            )
            .await
        {
            Ok(id) => {
                // Once per usage window, counted from a job that started: one that could not
                // (user work began, another job took the slot) leaves the window to try again.
                self.brains
                    .jobs
                    .enriched
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(project.clone(), resets_at);
                tracing::info!(project = %project, job = %id, provider = %provider, "enrichment started on spare quota")
            }
            Err(err) => {
                tracing::warn!(project = %project, error = %err, "could not start enrichment")
            }
        }
    }

    /// The project most in need of enrichment that this usage window has not enriched yet: its
    /// skeleton pass is done, and it has stale nodes or modules only the index describes.
    async fn enrichment_project(&self, resets_at: i64) -> Option<ProjectId> {
        let open: Vec<(ProjectId, brigadier_brain::Brain)> = self
            .brains
            .open_projects()
            .into_iter()
            .filter(|(_, project)| project.index.is_some())
            .map(|(id, project)| (id, project.brain.clone()))
            .collect();
        let mut best: Option<(ProjectId, usize)> = None;
        for (id, brain) in open {
            let done_for = self
                .brains
                .jobs
                .enriched
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(&id)
                .copied();
            if done_for == Some(resets_at) {
                continue;
            }
            let skeleton =
                self.brain_jobs(&id).await.iter().any(|job| {
                    job.kind == BrainJobKind::Skeleton && job.state == BrainJobState::Done
                });
            if !skeleton {
                continue;
            }
            let need = blocking(move || {
                let nodes = brain
                    .nodes(&NodeFilter {
                        kinds: vec![
                            NodeKind::Module,
                            NodeKind::Service,
                            NodeKind::FileSummary,
                            NodeKind::Convention,
                            NodeKind::Contract,
                        ],
                        session_id: None,
                        current_only: true,
                        text: None,
                        limit: Some(10_000),
                    })
                    .map_err(brain_error)?;
                Ok(nodes
                    .iter()
                    .filter(|node| {
                        matches!(node.state, NodeState::Stale { .. })
                            || node.provenance.origin == Origin::Index
                    })
                    .count())
            })
            .await
            .unwrap_or(0);
            if need > 0 && best.as_ref().is_none_or(|(_, most)| need > *most) {
                best = Some((id, need));
            }
        }
        best.map(|(id, _)| id)
    }

    // ----- Chats ------------------------------------------------------------------------

    /// A Chat's tool call.
    pub(super) async fn chat_call(&self, id: ConversationId, call: ChatCall) -> ToolReply {
        let result = match call {
            ChatCall::SaveMemory(args) => self.save_memory(&id, args).await,
        };
        match result {
            Ok(text) => ToolReply::ok(text),
            Err(err) => ToolReply::error(err.to_string()),
        }
    }

    /// `save_memory`: a preference in the Personal Brain, and a Memory chip in the Chat.
    ///
    /// Keyed per Chat: saving it again in the same Chat updates it in place, while the same
    /// memory saved in another Chat is a node of its own, so forgetting one Chat's never
    /// removes what another Chat saved.
    async fn save_memory(&self, id: &ConversationId, args: SaveMemory) -> Result<String> {
        let text = one_line(&args.memory, 240);
        if text.is_empty() {
            return Err(Error::Invalid("say what to remember in `memory`".into()));
        }
        let brain = self.personal_brain().await?;
        let before = self.memory_lines(MEMORY_BYTES).await;
        let node = NewNode {
            kind: NodeKind::Preference,
            key: Some(format!(
                "preference:{}:{}",
                id.0,
                blake_key(&text.to_lowercase())
            )),
            title: text.clone(),
            body: String::new(),
            provenance: Provenance {
                origin: Origin::User,
                session_id: Some(id.0.clone()),
                task_id: None,
                job_id: None,
                worker: None,
                commit: None,
                recorded_at_ms: now_ms(),
            },
            files: Vec::new(),
            expires_at_ms: None,
        };
        let node_id = blocking(move || brain.record(node).map_err(brain_error)).await?;
        self.told_own_preference(id, &before).await;
        let request_id = match self.conv(id) {
            Ok(conv) => conv.running_request().await,
            Err(_) => None,
        };
        self.core
            .record_conversation(
                id,
                vec![DomainEvent::MemoryUpdated {
                    conversation_id: id.clone(),
                    memory: MemoryChange {
                        node_id,
                        text,
                        forgotten: false,
                        request_id,
                        at_ms: now_ms(),
                    },
                }],
            )
            .await?;
        Ok("Saved. The user sees it and can remove it.".into())
    }

    /// The user's memories, newest first, within `max_bytes`, as lines for instructions.
    /// The same memory saved in several Chats is one line.
    pub(super) async fn memory_lines(&self, max_bytes: usize) -> Vec<String> {
        let Ok(memories) = self.list_memories().await else {
            return Vec::new();
        };
        let mut seen = std::collections::HashSet::new();
        let mut used = 0;
        memories
            .into_iter()
            .map(|node| match node.body.trim() {
                "" => node.title,
                body => format!("{}: {}", node.title, one_line(body, 300)),
            })
            .filter(|line| seen.insert(line.to_lowercase()))
            .take_while(|line| {
                used += line.len() + 3;
                used <= max_bytes
            })
            .collect()
    }
}

/// A window of `quota` that resets within the hour with plenty left, while no window runs hot:
/// when it resets.
pub(super) fn spare_window(quota: &QuotaSnapshot, now: i64) -> Option<i64> {
    if quota.limit.is_some() || quota.windows.is_empty() || hottest(quota) >= ENRICH_HOT {
        return None;
    }
    quota
        .windows
        .iter()
        .filter(|window| window.used_percent <= ENRICH_MAX_USED)
        .filter_map(|window| window.resets_at_ms)
        .filter(|at| *at > now && *at - now <= ENRICH_RESET_WITHIN_MS)
        .min()
}

fn hottest(quota: &QuotaSnapshot) -> f64 {
    quota
        .windows
        .iter()
        .map(|window| window.used_percent)
        .fold(0.0, f64::max)
}

/// A repository-relative path as nodes key it: `/`-separated, no leading `./`, no trailing
/// `/`; the repository root is empty.
fn clean_path(path: &str) -> String {
    let path = path.trim().replace('\\', "/");
    let path = path.trim_start_matches("./").trim_end_matches('/');
    if path == "." {
        String::new()
    } else {
        path.to_owned()
    }
}

/// What an enrichment job should look at first, as prompt lines.
fn enrichment_targets(brain: &brigadier_brain::Brain) -> Result<String> {
    let nodes = brain
        .nodes(&NodeFilter {
            kinds: vec![
                NodeKind::Module,
                NodeKind::Service,
                NodeKind::FileSummary,
                NodeKind::Convention,
                NodeKind::Contract,
            ],
            session_id: None,
            current_only: true,
            text: None,
            limit: Some(10_000),
        })
        .map_err(brain_error)?;
    let mut text = String::new();
    let stale: Vec<_> = nodes
        .iter()
        .filter_map(|node| match &node.state {
            NodeState::Stale { reason, .. } => Some((node, reason)),
            _ => None,
        })
        .take(ENRICH_TARGETS)
        .collect();
    if !stale.is_empty() {
        text.push_str("Stale nodes (a file they describe changed); re-read and record them again with the same kind and path. A convention, contract or decision recorded under another title or kind passes its key below as `replaces`, so the stale one is kept as history and stops being current:\n");
        for (node, reason) in stale {
            let files: Vec<&str> = node.files.iter().map(|file| file.path.as_str()).collect();
            text.push_str(&format!(
                "- {:?} \"{}\" (key {}; {reason}; files: {})\n",
                node.kind,
                node.title,
                node.key.as_deref().unwrap_or(&node.id),
                files.join(", ")
            ));
        }
    }
    let bare: Vec<_> = nodes
        .iter()
        .filter(|node| node.provenance.origin == Origin::Index)
        .take(ENRICH_TARGETS)
        .collect();
    if !bare.is_empty() {
        text.push_str("\nOnly the index describes these (no purpose yet); describe them:\n");
        for node in bare {
            text.push_str(&format!(
                "- {:?} {}\n",
                node.kind,
                node.key.as_deref().unwrap_or(&node.title)
            ));
        }
    }
    let summarized: Vec<&str> = nodes
        .iter()
        .filter(|node| node.kind == NodeKind::FileSummary)
        .filter_map(|node| node.key.as_deref()?.strip_prefix("file:"))
        .collect();
    if !summarized.is_empty() {
        text.push_str(&format!(
            "\nFiles already summarized (skip unless stale): {}\n",
            summarized.join(", ")
        ));
    }
    let conventions: Vec<&str> = nodes
        .iter()
        .filter(|node| node.kind == NodeKind::Convention)
        .map(|node| node.title.as_str())
        .collect();
    if !conventions.is_empty() {
        text.push_str(&format!(
            "\nConventions already known (don't repeat them): {}\n",
            conventions.join("; ")
        ));
    }
    Ok(text)
}

const JOB_ROLE: &str = "You are a background job of Brigadier, a desktop app that runs coding agents. \
You build the Project Brain: short, factual notes about a repository that later agents read instead \
of exploring it again. You only read; never change a file. Record what you learn with the \
`record_nodes` tool as you go (several calls are fine); anything not recorded is lost. Use \
`project_map`, `code_search` and `code_refs` to find your way before reading files. Keep every body \
to a few plain sentences of what a newcomer needs: what it is for, how it fits, what to watch out \
for. Record only what you read in the code, never guesses. When you are done, reply `done`.";

fn job_prompt(
    kind: BrainJobKind,
    provider: ProviderKind,
    root: &std::path::Path,
    digest: &str,
    targets: &str,
) -> String {
    let place = match provider {
        ProviderKind::Codex => format!(
            "The repository is at {} (read it by path; your working folder is a scratch folder).",
            root.display()
        ),
        ProviderKind::Claude => {
            format!("The repository is your working folder, {}.", root.display())
        }
    };
    let task = match kind {
        BrainJobKind::Skeleton => "This is the skeleton pass: the first map of this repository. Record:\n\
- one `module` node per module in the digest (its `path` as listed): its purpose, main entry points and how it relates to the others;\n\
- a `convention` titled \"Stack: …\" naming the languages, frameworks, key libraries and tools;\n\
- a `convention` titled \"Recipe: build, run and verify\" with the exact commands (install, build, run, test, lint, format) from the manifests, scripts and CI;\n\
- other conventions you see (code style, error handling, naming, tests, commit messages), one node each;\n\
- a `service` node per service (its name as `path`), and `contract` nodes for interfaces parts must keep to (an IPC protocol, an API, a schema);\n\
- `fileSummary` nodes (with `path`) for the handful of most central files.\n\
Read only what you need (READMEs, manifests, entry points). Finish within a few minutes.".to_owned(),
        BrainJobKind::Enrichment => format!(
            "This is an enrichment pass: deepen what the Brain already knows, most useful first.\n{targets}\n\
Work in this order: refresh the stale nodes; describe the modules and services only the index knows; \
summarize the most used files not yet summarized (`fileSummary` with `path`); then add conventions and \
contracts not yet known. Record as you go; you may be stopped at any time."
        ),
    };
    format!("{place}\n\n{task}\n\nThe code index's digest of the repository:\n\n{digest}")
}
