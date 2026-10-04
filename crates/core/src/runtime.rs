//! The provider runtime: live CLI sessions, their event streams, approvals, the cleanup ledger
//! and what Brigadier knows about each provider.
//!
//! In Phase 2 its sessions are "raw" sessions driven from the Inspector. Each one:
//!
//! - streams its normalized events into `raw:<id>`, with text deltas merged over
//!   [`DELTA_WINDOW`] so the store is not hit on every token;
//! - has its approvals routed by [`policy::route`]: Brigadier answers what stays inside the
//!   session's access, the user the rest;
//! - records every artifact its CLI creates in the cleanup ledger before relying on it, and
//!   removes exactly those when it is closed, when it fails to start, or in the crash sweep.
//!
//! A stopped session keeps its CLI session so it can be resumed; closing it removes everything.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use brigadier_providers::claude::Claude;
use brigadier_providers::cli::CliEnv;
use brigadier_providers::codex::Codex;
use brigadier_providers::history::PastFolder;
use brigadier_providers::policy::{self, ApprovalMode, Route};
use brigadier_providers::record::{self, Recording};
use brigadier_providers::{
    Access, ApprovalDecision, Decider, ModelCatalog, Origin, Provider, ProviderEvent, ProviderKind,
    ProviderSession, SessionSpec, Started, ToolSet, fixtures,
};
use brigadier_sandbox::Platform;
use brigadier_store::{NewEvent, Retention, StreamPage};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::ledger::CleanupLedger;
use crate::model::{
    DomainEvent, Fixture, ProviderOverview, ProvidersView, RawApprovals, RawEntry, RawPage,
    RawSession, RawSessionId, RawSource, RawState, streams,
};
use crate::routing::{QuotaMonitor, RegistryHolder, RoutingStore};
use crate::{Core, Error, Result, now_ms};

/// Text deltas arriving within this window are stored as one event.
const DELTA_WINDOW: Duration = Duration::from_millis(30);
/// How long a quit waits for sessions to end and their last events to be stored.
const SHUTDOWN_PUMPS: Duration = Duration::from_secs(3);
/// Live quota updates are stored as provider overviews at most this often.
const QUOTA_RECORD_INTERVAL_MS: i64 = 10_000;
/// Longest pause kept between lines when replaying a recording.
const REPLAY_MAX_GAP: Duration = Duration::from_millis(250);
const PROVIDER_CHECKS_KEPT: u32 = 100;
const STREAM_PAGE: u32 = 1_000;

/// Runs a task on the daemon's instrumented runtime.
pub type Spawner = Arc<dyn Fn(Pin<Box<dyn Future<Output = ()> + Send>>) + Send + Sync>;

/// What the Inspector asks for when it starts a raw session.
#[derive(Debug, Clone)]
pub struct StartRaw {
    pub provider: ProviderKind,
    pub cwd: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub access: Access,
    pub approvals: RawApprovals,
    pub record: bool,
}

struct Live {
    session: Arc<dyn ProviderSession>,
    /// Approvals waiting for the user.
    pending: HashSet<String>,
    /// Cancelled once the session's event pump has stored its last event.
    ended: CancellationToken,
}

#[derive(Default)]
struct State {
    sessions: HashMap<RawSessionId, RawSession>,
    live: HashMap<RawSessionId, Live>,
    overviews: HashMap<ProviderKind, ProviderOverview>,
    quota_recorded_ms: HashMap<ProviderKind, i64>,
    /// Providers being checked now.
    refreshing: HashSet<ProviderKind>,
}

pub struct Runtime {
    core: Arc<Core>,
    platform: Arc<dyn Platform>,
    claude: Arc<Claude>,
    codex: Arc<Codex>,
    spawner: Spawner,
    state: Mutex<State>,
    admitting: AtomicBool,
    pumps: TaskTracker,
    cache_dir: PathBuf,
    recordings_dir: PathBuf,
    ledger: Arc<CleanupLedger>,
    env: Arc<CliEnv>,
    monitor: Arc<QuotaMonitor>,
    routing: Option<Arc<RoutingStore>>,
    registry: Arc<RegistryHolder>,
    /// Cancelled when the daemon shuts down (ends the quota poller).
    quit: CancellationToken,
    /// Counts the provider overviews recorded: work waiting for quota looks again when it
    /// moves (a login, a limit, a fresh usage read).
    checked: tokio::sync::watch::Sender<u64>,
    /// Tests: scripted stand-ins for the Claude and Codex CLIs.
    #[cfg(test)]
    fakes: Option<[Arc<dyn Provider>; 2]>,
}

impl Runtime {
    /// Loads raw sessions and the ledger, sweeps what a crashed daemon left behind, and starts
    /// checking the providers.
    pub async fn start(
        core: Arc<Core>,
        platform: Arc<dyn Platform>,
        spawner: Spawner,
    ) -> Result<Arc<Self>> {
        Self::start_with(core, platform, spawner, None).await
    }

    /// [`Runtime::start`] with scripted CLIs (Claude's, then Codex's) in place of the real
    /// ones.
    #[cfg(test)]
    pub(crate) async fn start_faked(
        core: Arc<Core>,
        platform: Arc<dyn Platform>,
        spawner: Spawner,
        fakes: [Arc<dyn Provider>; 2],
    ) -> Result<Arc<Self>> {
        Self::start_with(core, platform, spawner, Some(fakes)).await
    }

    async fn start_with(
        core: Arc<Core>,
        platform: Arc<dyn Platform>,
        spawner: Spawner,
        #[cfg_attr(not(test), allow(unused_variables))] fakes: Option<[Arc<dyn Provider>; 2]>,
    ) -> Result<Arc<Self>> {
        let env = {
            let platform = platform.clone();
            tokio::task::spawn_blocking(move || CliEnv::capture(&platform))
                .await
                .map_err(|err| Error::Invalid(format!("capturing the login environment: {err}")))?
        };
        let env = Arc::new(env);
        let data_dir = platform.paths().data_dir.clone();
        let claude = Arc::new(Claude::new(platform.clone(), env.clone()));
        let codex = Arc::new(Codex::new(platform.clone(), env.clone()));
        let ledger = Arc::new(
            CleanupLedger::load(
                core.clone(),
                platform.clone(),
                claude.clone(),
                codex.clone(),
            )
            .await?,
        );
        let routing = {
            let path = data_dir.join("routing.sqlite");
            match tokio::task::spawn_blocking(move || RoutingStore::open(&path)).await {
                Ok(Ok(store)) => Some(store),
                Ok(Err(err)) => {
                    tracing::warn!(error = %err, "routing store unavailable");
                    None
                }
                Err(err) => {
                    tracing::warn!(error = %err, "routing store unavailable");
                    None
                }
            }
        };
        let monitor = QuotaMonitor::load(routing.clone(), now_ms()).await;
        let registry = {
            let cache = data_dir.join("cache");
            tokio::task::spawn_blocking(move || RegistryHolder::load(&cache))
                .await
                .map_err(|err| Error::Invalid(format!("loading the model registry: {err}")))?
        };
        let runtime = Arc::new(Self {
            monitor,
            routing,
            registry,
            quit: CancellationToken::new(),
            checked: tokio::sync::watch::Sender::new(0),
            claude,
            codex,
            ledger,
            env,
            core,
            platform,
            spawner,
            state: Mutex::new(State::default()),
            admitting: AtomicBool::new(true),
            pumps: TaskTracker::new(),
            cache_dir: data_dir.join("cache"),
            recordings_dir: record::recordings_dir(&data_dir),
            #[cfg(test)]
            fakes,
        });
        runtime.load().await?;
        runtime.sweep().await;
        let ledger = runtime.ledger.clone();
        runtime
            .pumps
            .spawn(async move { ledger.archive_codex_threads().await });
        runtime.refresh_providers(None);
        let poller = runtime.clone();
        runtime.spawn(async move { poller.poll_quota().await });
        Ok(runtime)
    }

    /// Moves each time a provider's overview is recorded (its login, limits or usage may have
    /// changed).
    pub fn provider_checks(&self) -> tokio::sync::watch::Receiver<u64> {
        self.checked.subscribe()
    }

    /// The quota monitor: every provider's windows as last reported, and their history.
    pub fn monitor(&self) -> &Arc<QuotaMonitor> {
        &self.monitor
    }

    /// A provider's quota as routing sees it: each window with its rolling estimate, from the
    /// monitor's samples over the window's span.
    pub fn provider_usage(
        &self,
        kind: ProviderKind,
        now: i64,
    ) -> Option<brigadier_router::ProviderQuota> {
        let snapshot = self.monitor.current(kind, now)?;
        let history: Vec<(String, Vec<brigadier_router::QuotaSample>)> = snapshot
            .windows
            .iter()
            .map(|window| {
                let span_ms = window.window_minutes.unwrap_or(7 * 24 * 60) * 60 * 1000;
                (
                    window.id.clone(),
                    self.monitor.history(kind, &window.id, now - span_ms),
                )
            })
            .collect();
        let history: Vec<(&str, &[brigadier_router::QuotaSample])> = history
            .iter()
            .map(|(id, samples)| (id.as_str(), samples.as_slice()))
            .collect();
        Some(brigadier_router::provider_quota(&snapshot, &history, now))
    }

    /// The model registry in use.
    pub fn registry(&self) -> &Arc<RegistryHolder> {
        &self.registry
    }

    /// `routing.sqlite`, when it could be opened.
    pub fn routing_store(&self) -> Option<&Arc<RoutingStore>> {
        self.routing.as_ref()
    }

    /// Reads every logged-in provider's quota on the monitor's schedule, until shutdown.
    async fn poll_quota(self: Arc<Self>) {
        let mut pruned_ms = now_ms();
        loop {
            let wait = self.monitor.next_poll(now_ms());
            tokio::select! {
                () = self.quit.cancelled() => return,
                () = tokio::time::sleep(wait) => {}
            }
            for kind in ProviderKind::ALL {
                let ready = {
                    let state = self.state();
                    !state.refreshing.contains(&kind)
                        && state
                            .overviews
                            .get(&kind)
                            .and_then(|overview| overview.status.as_ref())
                            .is_some_and(|status| status.logged_in)
                };
                if !ready {
                    continue;
                }
                match self.provider(kind).quota().await {
                    Ok(quota) => self.note_read(quota).await,
                    Err(err) => {
                        tracing::debug!(provider = %kind, error = %err, "quota read failed")
                    }
                }
            }
            let now = now_ms();
            if now - pruned_ms > 24 * 60 * 60 * 1000 {
                pruned_ms = now;
                if let Some(store) = &self.routing
                    && let Err(err) = store.prune(now).await
                {
                    tracing::warn!(error = %err, "could not prune the routing history");
                }
            }
        }
    }

    /// Development builds: holds `provider` at `limit` until its reset (see
    /// [`QuotaMonitor::inject`]) and records the provider's overview.
    #[cfg(debug_assertions)]
    pub fn debug_limit(
        self: &Arc<Self>,
        provider: ProviderKind,
        limit: brigadier_providers::LimitHit,
    ) {
        self.monitor.inject(provider, limit);
        let runtime = self.clone();
        self.spawn(async move {
            let overview = {
                let mut state = runtime.state();
                let Some(overview) = state.overviews.get_mut(&provider) else {
                    return;
                };
                overview.quota = runtime.monitor.current(provider, now_ms());
                overview.clone()
            };
            runtime.record_overview(overview).await;
        });
    }

    /// Takes in a limit a session's error reported (see [`QuotaMonitor::note_limit`]) and
    /// records the provider's overview.
    pub async fn note_limit(&self, provider: ProviderKind, limit: brigadier_providers::LimitHit) {
        let overview = {
            let mut state = self.state();
            let Some(overview) = state.overviews.get_mut(&provider) else {
                return;
            };
            overview.quota = Some(self.monitor.note_limit(provider, limit, now_ms()));
            overview.clone()
        };
        self.record_overview(overview).await;
    }

    /// Takes in a fresh quota read and records the provider's overview.
    async fn note_read(&self, quota: brigadier_providers::QuotaSnapshot) {
        let overview = {
            let mut state = self.state();
            let Some(overview) = state.overviews.get_mut(&quota.provider) else {
                return;
            };
            overview.quota = Some(self.monitor.note(&quota, now_ms()));
            overview.clone()
        };
        self.record_overview(overview).await;
    }

    /// The cleanup ledger shared by every CLI session and conversation.
    pub fn ledger(&self) -> &Arc<CleanupLedger> {
        &self.ledger
    }

    /// The environment CLIs (and git) run with: the user's login environment.
    pub fn cli_env(&self) -> &Arc<CliEnv> {
        &self.env
    }

    pub fn platform(&self) -> &Arc<dyn Platform> {
        &self.platform
    }

    /// The folders the user's own sessions of each CLI ran in.
    pub async fn past_folders(&self) -> Vec<(ProviderKind, PastFolder)> {
        let (claude, codex) = tokio::join!(self.claude.past_folders(), self.codex.past_folders());
        let claude = claude
            .into_iter()
            .map(|folder| (ProviderKind::Claude, folder));
        let codex = codex
            .into_iter()
            .map(|folder| (ProviderKind::Codex, folder));
        claude.chain(codex).collect()
    }

    /// What Brigadier last learned about a provider (login, models, quota).
    pub fn overview(&self, kind: ProviderKind) -> Option<ProviderOverview> {
        self.state().overviews.get(&kind).cloned()
    }

    /// Invalidates the routing views after completion, reset or registry supersession.
    pub async fn rankings_changed(&self) {
        let result = async {
            let event = new_event(streams::PROVIDERS, &DomainEvent::RankingsChanged)?;
            self.core.store().append(vec![event]).await?;
            Ok::<_, Error>(())
        }
        .await;
        if let Err(error) = result {
            tracing::warn!(%error, "could not publish rankings.changed");
        }
    }

    /// Starts a CLI session owned by `owner` (`orch:…`, `task:…`, `chat:…`); everything it
    /// creates is recorded under `owner` in the cleanup ledger.
    pub async fn start_hosted(
        &self,
        owner: &str,
        kind: ProviderKind,
        spec: SessionSpec,
    ) -> Result<Started> {
        self.admit()?;
        self.provider(kind)
            .start(spec, self.ledger.handle(owner.to_owned()))
            .await
            .map_err(provider_error)
    }

    fn provider(&self, kind: ProviderKind) -> Arc<dyn Provider> {
        #[cfg(test)]
        if let Some([claude, codex]) = &self.fakes {
            return match kind {
                ProviderKind::Claude => claude.clone(),
                ProviderKind::Codex => codex.clone(),
            };
        }
        match kind {
            ProviderKind::Claude => self.claude.clone(),
            ProviderKind::Codex => self.codex.clone(),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        (self.spawner)(Box::pin(task));
    }

    fn admit(&self) -> Result<()> {
        if self.admitting.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(Error::Invalid("Brigadier is shutting down".into()))
        }
    }

    async fn load(&self) -> Result<()> {
        let mut sessions: HashMap<RawSessionId, RawSession> = HashMap::new();
        for event in self.read_all(streams::RAW).await? {
            match event {
                DomainEvent::RawSessionCreated { session } => {
                    sessions.insert(session.id.clone(), session);
                }
                DomainEvent::RawSessionUpdated {
                    id,
                    state,
                    native_id,
                    error,
                } => {
                    if let Some(session) = sessions.get_mut(&id) {
                        apply_update(session, state, native_id, error);
                    }
                }
                _ => {}
            }
        }
        let cached = {
            let dir = self.cache_dir.clone();
            tokio::task::spawn_blocking(move || read_model_cache(&dir))
                .await
                .unwrap_or_default()
        };
        let mut state = self.state();
        state.sessions = sessions;
        for kind in ProviderKind::ALL {
            state.overviews.insert(
                kind,
                ProviderOverview {
                    provider: kind,
                    status: None,
                    models: cached
                        .iter()
                        .find(|catalog| catalog.provider == kind)
                        .cloned(),
                    quota: None,
                    usage: None,
                    error: None,
                    checked_at_ms: None,
                },
            );
        }
        Ok(())
    }

    async fn read_all(&self, stream: &str) -> Result<Vec<DomainEvent>> {
        let mut events = Vec::new();
        let mut after = 0;
        loop {
            let page = self
                .core
                .store()
                .read_stream_since(stream.into(), after, STREAM_PAGE)
                .await?;
            for event in &page {
                after = event.stream_seq;
                events.push(crate::sessions::decode(event)?);
            }
            if page.len() < STREAM_PAGE as usize {
                return Ok(events);
            }
        }
    }

    /// Crash sweep: whatever the previous daemon left running is ended; sessions that were
    /// running become stopped (resumable); sessions that never started, or were being closed,
    /// have their artifacts removed.
    async fn sweep(self: &Arc<Self>) {
        self.ledger.sweep().await;
        let sessions = self.state().sessions.values().cloned().collect::<Vec<_>>();
        for session in sessions {
            match session.state {
                RawState::Running => {
                    self.set_state(&session.id, RawState::Stopped, None, None)
                        .await;
                }
                RawState::Starting => {
                    let resumable = session.native_id.is_some();
                    if resumable {
                        self.set_state(&session.id, RawState::Stopped, None, None)
                            .await;
                    } else {
                        self.set_state(
                            &session.id,
                            RawState::Failed,
                            None,
                            Some("Brigadier quit while it was starting".into()),
                        )
                        .await;
                        let runtime = self.clone();
                        self.spawn(async move { runtime.remove_artifacts(&session.id).await });
                    }
                }
                RawState::Closing => {
                    let runtime = self.clone();
                    self.spawn(async move {
                        runtime.remove_artifacts(&session.id).await;
                        runtime
                            .set_state(&session.id, RawState::Closed, None, None)
                            .await;
                    });
                }
                RawState::Stopped | RawState::Failed | RawState::Closed => {}
            }
        }
    }

    /// Stops admitting work, ends every live session (bounded) and waits for their last
    /// events to be stored. Call before the store shuts down.
    pub async fn shutdown(&self) {
        self.admitting.store(false, Ordering::Release);
        self.quit.cancel();
        let sessions: Vec<(RawSessionId, Arc<dyn ProviderSession>)> = self
            .state()
            .live
            .iter()
            .map(|(id, live)| (id.clone(), live.session.clone()))
            .collect();
        if !sessions.is_empty() {
            tracing::info!(count = sessions.len(), "ending CLI sessions");
        }
        let closing = TaskTracker::new();
        for (id, session) in sessions {
            let ledger = self.ledger.clone();
            closing.spawn(async move {
                session.close().await;
                ledger.end_processes(&id.0).await;
            });
        }
        closing.close();
        closing.wait().await;
        self.pumps.close();
        if tokio::time::timeout(SHUTDOWN_PUMPS, self.pumps.wait())
            .await
            .is_err()
        {
            tracing::warn!("CLI session events were still being stored at shutdown");
        }
    }

    // ----- providers -------------------------------------------------------------------

    pub async fn view(&self) -> ProvidersView {
        let fixtures = {
            let dir = self.recordings_dir.clone();
            tokio::task::spawn_blocking(move || list_fixtures(&dir))
                .await
                .unwrap_or_default()
        };
        let state = self.state();
        let mut sessions: Vec<RawSession> = state.sessions.values().cloned().collect();
        sessions.sort_by_key(|session| std::cmp::Reverse(session.created_at_ms));
        ProvidersView {
            providers: ProviderKind::ALL
                .iter()
                .filter_map(|kind| state.overviews.get(kind).cloned())
                .collect(),
            sessions,
            fixtures,
        }
    }

    /// Checks every provider (or only `only`) in the background: login, live models (cached on
    /// disk) and quota. Results arrive as `providerChecked` events, each provider's as soon as
    /// it is known; a provider already being checked is not checked twice.
    pub fn refresh_providers(self: &Arc<Self>, only: Option<ProviderKind>) {
        for kind in ProviderKind::ALL {
            if only.is_some_and(|only| only != kind) || !self.state().refreshing.insert(kind) {
                continue;
            }
            let runtime = self.clone();
            self.spawn(async move {
                let overview = runtime.check(kind).await;
                runtime.record_overview(overview).await;
                runtime.state().refreshing.remove(&kind);
            });
        }
    }

    async fn check(&self, kind: ProviderKind) -> ProviderOverview {
        let provider = self.provider(kind);
        let previous = self.state().overviews.get(&kind).cloned();
        let previous_status = previous
            .as_ref()
            .and_then(|overview| overview.status.clone());
        let status = provider.status().await;
        let mut overview = ProviderOverview {
            provider: kind,
            models: previous
                .as_ref()
                .and_then(|overview| overview.models.clone()),
            quota: previous
                .as_ref()
                .and_then(|overview| overview.quota.clone()),
            usage: previous.and_then(|overview| overview.usage),
            status: Some(status.clone()),
            error: None,
            checked_at_ms: Some(now_ms()),
        };
        if !status.logged_in {
            return overview;
        }
        // Just signed in or installed: say so now; models and quota can take a while.
        let changed = previous_status.is_none_or(|previous| {
            previous.logged_in != status.logged_in || previous.path != status.path
        });
        if changed {
            self.record_overview(overview.clone()).await;
        }
        // The model list (or its failure) is published as soon as it is known: the startup
        // screen waits for it, and must not wait for a slow quota request too.
        let models = async {
            let models = provider.models().await;
            let mut with_models = overview.clone();
            match &models {
                Ok(catalog) => {
                    let dir = self.cache_dir.clone();
                    let cached = catalog.clone();
                    let _ =
                        tokio::task::spawn_blocking(move || write_model_cache(&dir, &cached)).await;
                    with_models.models = Some(catalog.clone());
                }
                Err(err) => with_models.error = Some(format!("models: {err}")),
            }
            self.record_overview(with_models).await;
            models
        };
        let (models, quota) = tokio::join!(models, provider.quota());
        let mut errors = Vec::new();
        match models {
            Ok(catalog) => overview.models = Some(catalog),
            Err(err) => errors.push(format!("models: {err}")),
        }
        match quota {
            Ok(quota) => overview.quota = Some(self.monitor.note(&quota, now_ms())),
            Err(err) => errors.push(format!("quota: {err}")),
        }
        overview.error = (!errors.is_empty()).then(|| errors.join("; "));
        overview
    }

    async fn record_overview(&self, mut overview: ProviderOverview) {
        // A model seen for the first time is known before routing can pick it.
        if let Some(catalog) = &overview.models {
            let ids: Vec<String> = catalog
                .models
                .iter()
                .map(|model| model.id.clone())
                .collect();
            if let Err(err) = self.core.note_models(overview.provider, &ids).await {
                tracing::warn!(error = %err, "could not record the models seen");
            }
        }
        // The monitor's view with its estimates, as of this check.
        overview.usage = self.provider_usage(overview.provider, now_ms());
        self.state()
            .overviews
            .insert(overview.provider, overview.clone());
        let event = DomainEvent::ProviderChecked { overview };
        let result = async {
            let new = new_event(streams::PROVIDERS, &event)?;
            self.core
                .store()
                .append_with(
                    vec![new],
                    Some(Retention {
                        keep_last: PROVIDER_CHECKS_KEPT,
                    }),
                )
                .await?;
            Ok::<_, Error>(())
        }
        .await;
        if let Err(err) = result {
            tracing::warn!(error = %err, "could not record a provider check");
        }
        self.checked.send_modify(|count| *count += 1);
    }

    // ----- raw sessions ----------------------------------------------------------------

    pub async fn start_session(self: &Arc<Self>, request: StartRaw) -> Result<RawSession> {
        self.admit()?;
        let cwd = PathBuf::from(request.cwd.trim());
        if !cwd.is_absolute() || !cwd.is_dir() {
            return Err(Error::Invalid(format!(
                "{} is not an existing absolute directory",
                cwd.display()
            )));
        }
        let id = RawSessionId::generate();
        let recording = request.record.then(|| {
            self.recordings_dir
                .join(format!("{}-{id}.jsonl", request.provider))
        });
        let now = now_ms();
        let session = RawSession {
            id: id.clone(),
            provider: request.provider,
            source: RawSource::Live,
            cwd: Some(cwd.display().to_string()),
            model: request.model.clone().filter(|model| !model.is_empty()),
            effort: request.effort.clone().filter(|effort| !effort.is_empty()),
            access: request.access.clone(),
            approvals: request.approvals,
            native_id: None,
            parent_id: None,
            state: RawState::Starting,
            error: None,
            recording: recording.as_ref().map(|path| path.display().to_string()),
            created_at_ms: now,
            updated_at_ms: now,
        };
        self.create(session.clone()).await?;
        let spec = spec_for(&session, Origin::New, recording);
        self.launch(id, spec, true);
        Ok(session)
    }

    /// Starts the stopped session's CLI session again, under the same raw session.
    pub async fn resume_session(self: &Arc<Self>, id: RawSessionId) -> Result<RawSession> {
        self.admit()?;
        let session = self.session(&id)?;
        let native_id = match (&session.source, session.state, &session.native_id) {
            (RawSource::Live, RawState::Stopped, Some(native_id)) => native_id.clone(),
            _ => {
                return Err(Error::Invalid(
                    "only a stopped live session can be resumed".into(),
                ));
            }
        };
        self.set_state(&id, RawState::Starting, None, None).await;
        let spec = spec_for(&session, Origin::Resume { native_id }, None);
        self.launch(id.clone(), spec, false);
        self.session(&id)
    }

    /// Branches a new raw session off this one's CLI session.
    pub async fn fork_session(self: &Arc<Self>, id: RawSessionId) -> Result<RawSession> {
        self.admit()?;
        let parent = self.session(&id)?;
        let native_id = match (&parent.source, &parent.native_id, parent.state) {
            (RawSource::Live, Some(native_id), RawState::Running | RawState::Stopped) => {
                native_id.clone()
            }
            _ => {
                return Err(Error::Invalid(
                    "only a started live session that is not closed can be forked".into(),
                ));
            }
        };
        let now = now_ms();
        let fork = RawSession {
            id: RawSessionId::generate(),
            parent_id: Some(parent.id.clone()),
            native_id: None,
            state: RawState::Starting,
            error: None,
            recording: None,
            created_at_ms: now,
            updated_at_ms: now,
            ..parent
        };
        self.create(fork.clone()).await?;
        let spec = spec_for(&fork, Origin::Fork { native_id }, None);
        self.launch(fork.id.clone(), spec, true);
        Ok(fork)
    }

    pub async fn send(&self, id: &RawSessionId, text: String, steer: bool) -> Result<()> {
        let session = self.live(id)?;
        let result = if steer {
            session.steer(text.into()).await
        } else {
            session.send(text.into()).await
        };
        result.map_err(provider_error)
    }

    pub async fn interrupt(&self, id: &RawSessionId) -> Result<()> {
        self.live(id)?.interrupt().await.map_err(provider_error)
    }

    /// The user's answer to an approval Brigadier could not answer on its own.
    pub async fn answer(
        &self,
        id: &RawSessionId,
        approval_id: String,
        decision: ApprovalDecision,
    ) -> Result<()> {
        let session = {
            let mut state = self.state();
            let live = state
                .live
                .get_mut(id)
                .ok_or_else(|| Error::Invalid("the session is not running".into()))?;
            if !live.pending.remove(&approval_id) {
                return Err(Error::NotFound(format!("approval {approval_id}")));
            }
            live.session.clone()
        };
        session
            .answer(approval_id.clone(), decision.clone())
            .await
            .map_err(provider_error)?;
        self.record_raw(
            id,
            vec![ProviderEvent::ApprovalResolved {
                id: approval_id,
                decision,
                decided_by: Decider::User,
            }],
        )
        .await;
        Ok(())
    }

    /// Ends the CLI process, keeping its CLI session for a later resume.
    pub async fn stop_session(&self, id: &RawSessionId) -> Result<()> {
        let session = self.live(id)?;
        session.close().await;
        self.ledger.end_processes(&id.0).await;
        Ok(())
    }

    /// Disposes of a session: ends its process and removes everything its CLI created. The
    /// transcript stays in Brigadier.
    pub async fn close_session(self: &Arc<Self>, id: RawSessionId) -> Result<RawSession> {
        let session = self.session(&id)?;
        if matches!(session.state, RawState::Closing | RawState::Closed) {
            return Ok(session);
        }
        self.set_state(&id, RawState::Closing, None, None).await;
        let runtime = self.clone();
        let closing = id.clone();
        self.spawn(async move {
            let live = runtime
                .state()
                .live
                .get(&closing)
                .map(|live| (live.session.clone(), live.ended.clone()));
            if let Some((session, ended)) = live {
                session.close().await;
                if tokio::time::timeout(SHUTDOWN_PUMPS, ended.cancelled())
                    .await
                    .is_err()
                {
                    tracing::warn!(session = %closing, "session events still pending at close");
                }
            }
            runtime.remove_artifacts(&closing).await;
            runtime
                .set_state(&closing, RawState::Closed, None, None)
                .await;
        });
        self.session(&id)
    }

    pub async fn transcript(
        &self,
        id: &RawSessionId,
        before: Option<i64>,
        limit: u32,
    ) -> Result<RawPage> {
        self.session(id)?;
        let limit = limit.clamp(1, brigadier_store::MAX_PAGE - 1);
        let mut events = self
            .core
            .store()
            .read_stream(
                streams::raw_session(id),
                StreamPage {
                    before,
                    kinds: Vec::new(),
                    limit: limit + 1,
                },
            )
            .await?;
        let has_more = events.len() > limit as usize;
        events.truncate(limit as usize);
        events.reverse();
        let entries = events
            .iter()
            .filter_map(|stored| match crate::sessions::decode(stored) {
                Ok(DomainEvent::RawEvent { event, .. }) => Some(Ok(RawEntry {
                    stream_seq: stored.stream_seq,
                    at_ms: stored.at_ms,
                    event,
                })),
                Ok(_) => None,
                Err(err) => Some(Err(err)),
            })
            .collect::<Result<_>>()?;
        Ok(RawPage { entries, has_more })
    }

    /// Replays a recording through a fresh parser into a new, isolated session.
    pub async fn replay(self: &Arc<Self>, fixture_id: &str) -> Result<RawSession> {
        self.admit()?;
        let text = load_fixture(&self.recordings_dir, fixture_id).await?;
        let recording = Recording::parse(&text).map_err(Error::Invalid)?;
        let lines = recording
            .lines
            .into_iter()
            .map(|line| (line.t, line.dir, line.line))
            .collect();
        let source = RawSource::Replay {
            title: recording.header.title,
        };
        self.feed_parser(recording.header.provider, source, lines, true)
            .await
    }

    /// Feeds a simulated usage-limit turn, in the CLI's real format, through a fresh parser
    /// in an isolated session, to show how it is detected and classified. Development builds
    /// only.
    #[cfg(debug_assertions)]
    pub async fn simulate_usage_limit(
        self: &Arc<Self>,
        provider: ProviderKind,
    ) -> Result<RawSession> {
        self.admit()?;
        let lines = brigadier_providers::simulate::usage_limit(provider)
            .into_iter()
            .map(|line| (0, record::Direction::Out, line))
            .collect();
        let source = RawSource::Simulation {
            title: format!("{} usage limit", provider.label()),
        };
        self.feed_parser(provider, source, lines, false).await
    }

    async fn feed_parser(
        self: &Arc<Self>,
        provider: ProviderKind,
        source: RawSource,
        lines: Vec<(u64, record::Direction, String)>,
        paced: bool,
    ) -> Result<RawSession> {
        let now = now_ms();
        let session = RawSession {
            id: RawSessionId::generate(),
            provider,
            source,
            cwd: None,
            model: None,
            effort: None,
            access: Access::ReadOnly,
            approvals: RawApprovals::DeclineAll,
            native_id: None,
            parent_id: None,
            state: RawState::Running,
            error: None,
            recording: None,
            created_at_ms: now,
            updated_at_ms: now,
        };
        self.create(session.clone()).await?;
        let (events_tx, events) = mpsc::channel(512);
        let mut replayer = self.provider(provider).replayer();
        self.spawn(async move {
            let mut last = 0;
            for (t, dir, line) in lines {
                if paced {
                    let gap = Duration::from_millis(t.saturating_sub(last)).min(REPLAY_MAX_GAP);
                    tokio::time::sleep(gap).await;
                    last = t;
                }
                for event in replayer.feed(dir, &line) {
                    if events_tx.send(event).await.is_err() {
                        return;
                    }
                }
            }
        });
        let ended = CancellationToken::new();
        let runtime = self.clone();
        let id = session.id.clone();
        self.pumps
            .spawn(async move { runtime.pump(id, events, ended).await });
        Ok(session)
    }

    // ----- internals -------------------------------------------------------------------

    fn session(&self, id: &RawSessionId) -> Result<RawSession> {
        self.state()
            .sessions
            .get(id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("raw session {id}")))
    }

    fn live(&self, id: &RawSessionId) -> Result<Arc<dyn ProviderSession>> {
        self.state()
            .live
            .get(id)
            .map(|live| live.session.clone())
            .ok_or_else(|| Error::Invalid("the session is not running".into()))
    }

    async fn create(&self, session: RawSession) -> Result<()> {
        let event = DomainEvent::RawSessionCreated {
            session: session.clone(),
        };
        self.core
            .store()
            .append(vec![new_event(streams::RAW, &event)?])
            .await?;
        self.state().sessions.insert(session.id.clone(), session);
        Ok(())
    }

    async fn set_state(
        &self,
        id: &RawSessionId,
        state: RawState,
        native_id: Option<String>,
        error: Option<String>,
    ) {
        let event = DomainEvent::RawSessionUpdated {
            id: id.clone(),
            state,
            native_id: native_id.clone(),
            error: error.clone(),
        };
        let stored = async {
            self.core
                .store()
                .append(vec![new_event(streams::RAW, &event)?])
                .await?;
            Ok::<_, Error>(())
        }
        .await;
        if let Err(err) = stored {
            tracing::warn!(session = %id, error = %err, "could not record a session state");
        }
        if let Some(session) = self.state().sessions.get_mut(id) {
            apply_update(session, state, native_id, error);
        }
    }

    /// Starts the CLI in the background. A failed fresh start removes whatever it created; a
    /// failed resume keeps the CLI session.
    fn launch(self: &Arc<Self>, id: RawSessionId, spec: SessionSpec, fresh: bool) {
        let runtime = self.clone();
        self.spawn(async move {
            let kind = match runtime.session(&id) {
                Ok(session) => session.provider,
                Err(_) => return,
            };
            let started = runtime
                .provider(kind)
                .start(spec, runtime.ledger.handle(id.0.clone()))
                .await;
            match started {
                Ok(Started { session, events }) => {
                    if !runtime.admitting.load(Ordering::Acquire) {
                        session.close().await;
                    }
                    let ended = CancellationToken::new();
                    runtime.state().live.insert(
                        id.clone(),
                        Live {
                            session,
                            pending: HashSet::new(),
                            ended: ended.clone(),
                        },
                    );
                    let pump = runtime.clone();
                    runtime
                        .pumps
                        .spawn(async move { pump.pump(id, events, ended).await });
                }
                Err(err) => {
                    let message = err.to_string();
                    tracing::warn!(session = %id, error = %message, "CLI session did not start");
                    if fresh {
                        runtime
                            .set_state(&id, RawState::Failed, None, Some(message))
                            .await;
                        runtime.remove_artifacts(&id).await;
                    } else {
                        runtime
                            .set_state(&id, RawState::Stopped, None, Some(message))
                            .await;
                    }
                }
            }
        });
    }

    /// Stores a session's events as they come, answering approvals on the way.
    async fn pump(
        self: Arc<Self>,
        id: RawSessionId,
        mut events: mpsc::Receiver<ProviderEvent>,
        ended: CancellationToken,
    ) {
        let mut deltas: Vec<ProviderEvent> = Vec::new();
        let mut deadline: Option<tokio::time::Instant> = None;
        loop {
            let flush_at = async {
                match deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                event = events.recv() => match event {
                    // Never stored.
                    Some(ProviderEvent::Progress { .. }) => {}
                    Some(event) if is_delta(&event) => {
                        merge_delta(&mut deltas, event);
                        deadline.get_or_insert_with(|| tokio::time::Instant::now() + DELTA_WINDOW);
                    }
                    Some(event) => {
                        deadline = None;
                        self.record_raw(&id, std::mem::take(&mut deltas)).await;
                        // The live session keeps its sender open; its exit ends the stream.
                        let exited = matches!(event, ProviderEvent::Exited { .. });
                        self.handle(&id, event).await;
                        if exited {
                            break;
                        }
                    }
                    None => break,
                },
                () = flush_at => {
                    deadline = None;
                    self.record_raw(&id, std::mem::take(&mut deltas)).await;
                }
            }
        }
        self.record_raw(&id, deltas).await;

        let closing = {
            let mut state = self.state();
            state.live.remove(&id);
            state
                .sessions
                .get(&id)
                .map(|session| matches!(session.state, RawState::Closing | RawState::Closed))
                .unwrap_or(true)
        };
        if !closing {
            self.set_state(&id, RawState::Stopped, None, None).await;
        }
        ended.cancel();
    }

    async fn handle(&self, id: &RawSessionId, event: ProviderEvent) {
        match &event {
            ProviderEvent::SessionStarted { native_id, .. } => {
                let native_id = native_id.clone();
                self.record_raw(id, vec![event]).await;
                self.set_state(id, RawState::Running, Some(native_id), None)
                    .await;
            }
            ProviderEvent::ApprovalRequested { request } => {
                let request = request.clone();
                self.record_raw(id, vec![event]).await;
                self.route_approval(id, request).await;
            }
            ProviderEvent::ApprovalResolved { id: approval, .. } => {
                if let Some(live) = self.state().live.get_mut(id) {
                    live.pending.remove(approval);
                }
                self.record_raw(id, vec![event]).await;
            }
            ProviderEvent::RateLimits { quota } => {
                let quota = quota.clone();
                self.record_raw(id, vec![event]).await;
                self.note_quota(id, quota).await;
            }
            _ => self.record_raw(id, vec![event]).await,
        }
    }

    async fn route_approval(
        &self,
        id: &RawSessionId,
        request: brigadier_providers::ApprovalRequest,
    ) {
        let (session, route) = {
            let mut state = self.state();
            let Some(raw) = state.sessions.get(id).cloned() else {
                return;
            };
            let Some(live) = state.live.get_mut(id) else {
                // A replay: nobody to answer.
                return;
            };
            let mode = match raw.approvals {
                RawApprovals::Delegated => ApprovalMode::Delegated,
                RawApprovals::DeclineAll => ApprovalMode::DeclineAll,
            };
            let route = policy::route(&request, &raw.access, mode);
            if route == Route::AskUser {
                live.pending.insert(request.id.clone());
            }
            (live.session.clone(), route)
        };
        let decision = match route {
            Route::AskUser => return,
            Route::Allow => ApprovalDecision::Allow,
            Route::Deny => ApprovalDecision::Deny {
                message: "Declined by Brigadier: this session may not do that.".into(),
            },
        };
        match session.answer(request.id.clone(), decision.clone()).await {
            Ok(()) => {
                self.record_raw(
                    id,
                    vec![ProviderEvent::ApprovalResolved {
                        id: request.id,
                        decision,
                        decided_by: Decider::Policy,
                    }],
                )
                .await;
            }
            Err(err) => tracing::warn!(session = %id, error = %err, "could not answer an approval"),
        }
    }

    /// Keeps the provider overview's quota current from live rate-limit events.
    async fn note_quota(&self, id: &RawSessionId, quota: brigadier_providers::QuotaSnapshot) {
        let overview = {
            let mut state = self.state();
            if !matches!(
                state.sessions.get(id).map(|session| &session.source),
                Some(RawSource::Live)
            ) {
                return;
            }
            let kind = quota.provider;
            let now = now_ms();
            let Some(overview) = state.overviews.get_mut(&kind) else {
                return;
            };
            overview.quota = Some(self.monitor.note(&quota, now));
            let overview = overview.clone();
            let last = state.quota_recorded_ms.entry(kind).or_default();
            if now - *last < QUOTA_RECORD_INTERVAL_MS {
                return;
            }
            *last = now;
            overview
        };
        self.record_overview(overview).await;
    }

    /// Keeps the provider overview's quota current from a hosted session's rate-limit events.
    pub async fn note_quota_snapshot(&self, quota: brigadier_providers::QuotaSnapshot) {
        let overview = {
            let mut state = self.state();
            let kind = quota.provider;
            let now = now_ms();
            let Some(overview) = state.overviews.get_mut(&kind) else {
                return;
            };
            overview.quota = Some(self.monitor.note(&quota, now));
            let overview = overview.clone();
            let last = state.quota_recorded_ms.entry(kind).or_default();
            if now - *last < QUOTA_RECORD_INTERVAL_MS {
                return;
            }
            *last = now;
            overview
        };
        self.record_overview(overview).await;
    }

    async fn record_raw(&self, id: &RawSessionId, events: Vec<ProviderEvent>) {
        if events.is_empty() {
            return;
        }
        let stream = streams::raw_session(id);
        let new = events
            .into_iter()
            .map(|event| {
                new_event(
                    &stream,
                    &DomainEvent::RawEvent {
                        session_id: id.clone(),
                        event,
                    },
                )
            })
            .collect::<Result<Vec<_>>>();
        let stored = match new {
            Ok(new) => self.core.store().append(new).await.map_err(Error::from),
            Err(err) => Err(err),
        };
        if let Err(err) = stored {
            tracing::warn!(session = %id, error = %err, "could not store session events");
        }
    }

    /// Removes everything recorded for `owner`, and nothing else.
    async fn remove_artifacts(&self, owner: &RawSessionId) {
        self.ledger.dispose(&owner.0).await;
    }
}

fn spec_for(session: &RawSession, origin: Origin, record_to: Option<PathBuf>) -> SessionSpec {
    SessionSpec {
        cwd: PathBuf::from(session.cwd.clone().unwrap_or_default()),
        model: session.model.clone(),
        effort: session.effort.clone(),
        fast: false,
        origin,
        access: session.access.clone(),
        append_system_prompt: None,
        mcp_servers: Vec::new(),
        tools: ToolSet::Default,
        env: Vec::new(),
        unset_env: Vec::new(),
        low_priority: false,
        record_to,
        redactor: None,
        owned_cwd: false,
        auto_compact: true,
        allowed_models: None,
        auto_review: false,
    }
}

fn apply_update(
    session: &mut RawSession,
    state: RawState,
    native_id: Option<String>,
    error: Option<String>,
) {
    session.state = state;
    if native_id.is_some() {
        session.native_id = native_id;
    }
    session.error = error;
    session.updated_at_ms = now_ms();
}

pub(crate) fn is_delta(event: &ProviderEvent) -> bool {
    matches!(
        event,
        ProviderEvent::MessageDelta { .. }
            | ProviderEvent::ReasoningDelta { .. }
            | ProviderEvent::CommandOutputDelta { .. }
    )
}

/// Appends a delta to the previous one when both continue the same item.
pub(crate) fn merge_delta(deltas: &mut Vec<ProviderEvent>, event: ProviderEvent) {
    use ProviderEvent::{CommandOutputDelta, MessageDelta, ReasoningDelta};
    match (deltas.last_mut(), event) {
        (
            Some(MessageDelta { item_id, text }),
            MessageDelta {
                item_id: next,
                text: more,
            },
        )
        | (
            Some(ReasoningDelta { item_id, text }),
            ReasoningDelta {
                item_id: next,
                text: more,
            },
        )
        | (
            Some(CommandOutputDelta { item_id, text }),
            CommandOutputDelta {
                item_id: next,
                text: more,
            },
        ) if *item_id == next => text.push_str(&more),
        (_, event) => deltas.push(event),
    }
}

fn new_event(stream: &str, event: &DomainEvent) -> Result<NewEvent> {
    Ok(NewEvent::new(stream, event.kind(), now_ms(), event)?)
}

fn provider_error(err: brigadier_providers::Error) -> Error {
    Error::Provider(err.to_string())
}

fn read_model_cache(dir: &Path) -> Vec<ModelCatalog> {
    ProviderKind::ALL
        .iter()
        .filter_map(|kind| {
            let text = std::fs::read_to_string(dir.join(format!("models-{kind}.json"))).ok()?;
            serde_json::from_str::<ModelCatalog>(&text).ok()
        })
        .collect()
}

fn write_model_cache(dir: &Path, catalog: &ModelCatalog) {
    let path = dir.join(format!("models-{}.json", catalog.provider));
    let written = std::fs::create_dir_all(dir).and_then(|()| {
        let text = serde_json::to_vec_pretty(catalog).map_err(std::io::Error::other)?;
        let partial = path.with_extension("json.partial");
        std::fs::write(&partial, text)?;
        std::fs::rename(&partial, &path)
    });
    if let Err(err) = written {
        tracing::warn!(path = %path.display(), error = %err, "could not cache the model list");
    }
}

fn list_fixtures(recordings: &Path) -> Vec<Fixture> {
    let describe = |id: String, text: &str| {
        let recording = Recording::parse(text).ok()?;
        Some(Fixture {
            id,
            title: recording.header.title,
            provider: recording.header.provider,
            cli_version: recording.header.cli_version,
            lines: recording.lines.len() as u32,
        })
    };
    let mut fixtures: Vec<Fixture> = fixtures::BUILTIN
        .iter()
        .filter_map(|(name, text)| describe(format!("builtin:{name}"), text))
        .collect();
    if let Ok(entries) = std::fs::read_dir(recordings) {
        let mut recorded: Vec<Fixture> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                name.ends_with(".jsonl").then_some(())?;
                let text = std::fs::read_to_string(entry.path()).ok()?;
                describe(format!("recording:{name}"), &text)
            })
            .collect();
        recorded.sort_by(|a, b| b.id.cmp(&a.id));
        fixtures.extend(recorded);
    }
    fixtures
}

async fn load_fixture(recordings: &Path, id: &str) -> Result<String> {
    if let Some(name) = id.strip_prefix("builtin:") {
        return fixtures::BUILTIN
            .iter()
            .find(|(builtin, _)| *builtin == name)
            .map(|(_, text)| (*text).to_owned())
            .ok_or_else(|| Error::NotFound(format!("fixture {id}")));
    }
    let name = id
        .strip_prefix("recording:")
        .filter(|name| !name.contains(['/', '\\']) && name.ends_with(".jsonl"))
        .ok_or_else(|| Error::Invalid(format!("{id} is not a fixture id")))?;
    let path = recordings.join(name);
    tokio::task::spawn_blocking(move || std::fs::read_to_string(path))
        .await
        .map_err(|err| Error::Invalid(err.to_string()))?
        .map_err(|_| Error::NotFound(format!("fixture {id}")))
}
