//! Computer use for workers (COMPUTER-USE-PLAN.md §4–§5): the broker between the workers'
//! computer grants and the helper process that drives the desktop.
//!
//! - **The helper** is started on the first call: the bundled `Brigadier Computer Use.app`
//!   through LaunchServices (`open -n`, so each daemon gets its own), or in development a
//!   `brigadier-computer` binary spawned directly (`BRIGADIER_COMPUTER_HELPER`, or one next to
//!   brigadierd). One that died is started again on the next call; the call that was running
//!   fails and is never replayed.
//! - **Permission levels.** Full access runs everything. Lower levels ask once on a card
//!   before `launch`, and before the first `act` on an app instance's window; the approval is
//!   bound to that instance (pid and start time) and those windows.
//! - **Leases.** A batch holds its window, and its app when it types, presses keys, picks
//!   menus or selects text, while it runs and for [`LEASE_TAIL`] after; another worker gets
//!   `busy`. A worker's end, and the user's Stop, release them.
//! - **Cancellation.** A call whose future is dropped (the CLI cancelled it, its connection
//!   closed) cancels its request in the helper.
//! - **The action log.** Each action is a `ComputerActed` event of the conversation; the
//!   batch's annotated screenshot goes to the blob store, referenced by the event, so it lives
//!   and dies with the conversation like any stored output.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use brigadier_computer::action::{ActRequest, Action};
use brigadier_computer::client::{Answer, Gone};
use brigadier_computer::error::{CuError, ErrorCode};
use brigadier_computer::geom::Provider;
use brigadier_computer::wire::{Described, Event, Instance, Launched, Op, Policy};
use brigadier_providers::{Artifact, BoxFuture, ProviderKind};
use tokio::sync::{oneshot, watch};

use super::SessionManager;
use super::cards::CardAnswer;
use crate::model::{ConversationId, DomainEvent, PermissionLevel};
use crate::tools::{ComputerCall, ToolReply};
use crate::work::{ApprovalSubject, CardState, ComputerAction, TaskId};
use brigadier_providers::{ApprovalDecision, Decider};

/// The environment variable holding a worker's computer grant, for `brigadierd computer` in
/// its shell (no KEY/SECRET/TOKEN in the name, which Codex's default environment filter drops).
pub(crate) const GRANT_ENV: &str = "BRIGADIER_COMPUTER_GRANT";
/// How long a window or app stays leased after its owner's last batch ends.
pub(crate) const LEASE_TAIL: Duration = Duration::from_secs(30);
/// What all of a batch's `wait` actions may take together; with the engine's 30 s for the
/// rest, no batch runs past 330 s (§4.7).
pub(crate) const MAX_BATCH_WAITS: Duration = Duration::from_secs(300);
/// How long a started helper has to answer.
const START_TIMEOUT: Duration = Duration::from_secs(10);
/// What the model is told when a permission is missing.
const PERMISSION_FIX: &str =
    "Ask the user to open Brigadier's Settings, Computer use, and allow what's missing.";

/// The broker's view of a helper connection; a fake in tests.
pub(crate) trait HelperLink: Send + Sync {
    fn next_id(&self) -> u64;
    fn send(
        &self,
        id: u64,
        worker: &str,
        provider: Provider,
        policy: Policy,
        op: Op,
        done: Box<dyn FnOnce(Result<Answer, Gone>) + Send>,
    );
    fn is_alive(&self) -> bool;
}

#[cfg(unix)]
impl HelperLink for brigadier_computer::client::Client {
    fn next_id(&self) -> u64 {
        brigadier_computer::client::Client::next_id(self)
    }
    fn send(
        &self,
        id: u64,
        worker: &str,
        provider: Provider,
        policy: Policy,
        op: Op,
        done: Box<dyn FnOnce(Result<Answer, Gone>) + Send>,
    ) {
        brigadier_computer::client::Client::send(self, id, worker, provider, policy, op, done);
    }
    fn is_alive(&self) -> bool {
        brigadier_computer::client::Client::is_alive(self)
    }
}

/// Starts a helper and connects to it; `on_event` gets what the helper says on its own.
pub(crate) type HelperStarter = Arc<
    dyn Fn(
            Arc<dyn Fn(Event) + Send + Sync>,
        ) -> BoxFuture<'static, Result<Arc<dyn HelperLink>, String>>
        + Send
        + Sync,
>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum LeaseKey {
    Window(u32),
    App(Instance),
}

struct Lease {
    owner: TaskId,
    label: String,
    running: bool,
    until: Instant,
}

/// What a worker may drive at the lower levels: approved instances, and their windows.
#[derive(Default)]
struct Approved {
    instances: HashSet<Instance>,
    windows: HashSet<(Instance, u32)>,
}

#[derive(Default)]
struct State {
    leases: HashMap<LeaseKey, Lease>,
    approved: HashMap<TaskId, Approved>,
    /// Processes and windows each worker's `launch` created.
    launched: HashMap<TaskId, Vec<Launched>>,
    host_pid: Option<i32>,
}

pub(crate) struct Computer {
    starter: Mutex<HelperStarter>,
    link: tokio::sync::Mutex<Option<Arc<dyn HelperLink>>>,
    state: Arc<Mutex<State>>,
    /// The bundle of the Brigadier app that hosts this daemon, blocked for its workers.
    host_bundle: Option<String>,
    /// Bumped by the user's Stop and by any worker's end, so a call waiting on a card
    /// looks again.
    changes: watch::Sender<u64>,
    /// How long the last helper took from its start to its first answer (S7).
    cold_start_ms: Mutex<Option<f64>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Computer {
    pub(crate) fn new(daemon_exe: &Path, data_dir: &Path) -> Self {
        Self::with_starter(daemon_exe, real_starter(daemon_exe, data_dir))
    }

    pub(crate) fn with_starter(daemon_exe: &Path, starter: HelperStarter) -> Self {
        Self {
            starter: Mutex::new(starter),
            link: tokio::sync::Mutex::new(None),
            state: Arc::default(),
            host_bundle: host_bundle(daemon_exe),
            changes: watch::channel(0).0,
            cold_start_ms: Mutex::new(None),
        }
    }

    /// Starts later helpers with `starter` (a fake, in tests).
    #[cfg(all(test, target_os = "macos"))]
    pub(crate) fn set_starter(&self, starter: HelperStarter) {
        *lock(&self.starter) = starter;
    }

    /// The Brigadier app that connected to this daemon: its windows hold the cards a worker
    /// must never answer itself.
    pub(crate) fn set_host_pid(&self, pid: i32) {
        lock(&self.state).host_pid = Some(pid);
    }

    pub(crate) fn cold_start_ms(&self) -> Option<f64> {
        *lock(&self.cold_start_ms)
    }

    async fn link(&self) -> Result<Arc<dyn HelperLink>, CuError> {
        let mut link = self.link.lock().await;
        if let Some(l) = link.as_ref().filter(|l| l.is_alive()) {
            return Ok(l.clone());
        }
        let state = Arc::downgrade(&self.state);
        let changes = self.changes.clone();
        let on_event: Arc<dyn Fn(Event) + Send + Sync> = Arc::new(move |event| match event {
            Event::Stopped { by } => {
                tracing::info!(by, "the user stopped computer use");
                if let Some(state) = state.upgrade() {
                    lock(&state).leases.clear();
                }
                changes.send_modify(|n| *n += 1);
            }
        });
        let started = Instant::now();
        let starter = lock(&self.starter).clone();
        let fresh = starter(on_event).await.map_err(|why| {
            CuError::new(
                ErrorCode::AppNotResponding,
                format!("Brigadier Computer Use couldn't start: {why}"),
            )
        })?;
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        *lock(&self.cold_start_ms) = Some(ms);
        tracing::info!(ms, "computer-use helper started");
        *link = Some(fresh.clone());
        Ok(fresh)
    }

    /// Sends one request and waits for its answer. Dropping the future cancels the request
    /// in the helper.
    async fn request(
        &self,
        worker: &TaskId,
        provider: Provider,
        policy: Policy,
        op: Op,
    ) -> Result<Answer, CuError> {
        let link = self.link().await?;
        let id = link.next_id();
        let (tx, rx) = oneshot::channel();
        link.send(
            id,
            &worker.0,
            provider,
            policy,
            op,
            Box::new(move |answer| {
                let _ = tx.send(answer);
            }),
        );
        let mut guard = CancelOnDrop {
            link: Some(link),
            id,
            worker: worker.0.clone(),
        };
        let answer = rx.await;
        guard.link = None;
        match answer {
            Ok(Ok(answer)) => Ok(answer),
            _ => Err(CuError::new(
                ErrorCode::AppNotResponding,
                "Brigadier Computer Use stopped while working on this; it starts again on the next call, but this request isn't repeated",
            )),
        }
    }

    fn tell(&self, link: &Arc<dyn HelperLink>, worker: &str, op: Op) {
        let id = link.next_id();
        link.send(
            id,
            worker,
            Provider::Claude,
            Policy::default(),
            op,
            Box::new(|_| {}),
        );
    }

    /// A worker ended: its requests stop, its leases and approvals go.
    pub(crate) async fn end_worker(&self, task_id: &TaskId) {
        {
            let mut state = lock(&self.state);
            state.leases.retain(|_, l| &l.owner != task_id);
            state.approved.remove(task_id);
            state.launched.remove(task_id);
        }
        self.changes.send_modify(|n| *n += 1);
        let link = self.link.lock().await.clone();
        if let Some(link) = link.filter(|l| l.is_alive()) {
            self.tell(&link, &task_id.0, Op::EndSession);
        }
    }

    fn policy(&self, task_id: &TaskId) -> Policy {
        let state = lock(&self.state);
        let launched = state.launched.get(task_id);
        Policy {
            host_pid: state.host_pid,
            host_bundle_path: self.host_bundle.clone(),
            launched_pids: launched
                .map(|l| {
                    l.iter()
                        .filter(|x| x.new_process)
                        .map(|x| x.instance.pid)
                        .collect()
                })
                .unwrap_or_default(),
            launched_windows: launched
                .map(|l| {
                    l.iter()
                        .flat_map(|x| x.new_windows.iter().copied())
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// Takes the leases a batch needs, or says who holds one.
    fn take_leases(&self, task_id: &TaskId, label: &str, keys: &[LeaseKey]) -> Result<(), CuError> {
        let mut state = lock(&self.state);
        let now = Instant::now();
        for key in keys {
            if let Some(l) = state.leases.get(key)
                && &l.owner != task_id
                && (l.running || l.until > now)
            {
                return Err(CuError::new(
                    ErrorCode::Busy,
                    format!("{} is using it", l.label),
                ));
            }
        }
        for key in keys {
            state.leases.insert(
                key.clone(),
                Lease {
                    owner: task_id.clone(),
                    label: label.to_owned(),
                    running: true,
                    until: now,
                },
            );
        }
        Ok(())
    }

    fn release_leases(&self, task_id: &TaskId, keys: &[LeaseKey]) {
        let mut state = lock(&self.state);
        for key in keys {
            if let Some(l) = state.leases.get_mut(key)
                && &l.owner == task_id
            {
                l.running = false;
                l.until = Instant::now() + LEASE_TAIL;
            }
        }
    }

    fn approved(&self, task_id: &TaskId, d: &Described) -> bool {
        lock(&self.state).approved.get(task_id).is_some_and(|a| {
            a.instances.contains(&d.instance)
                && a.windows.contains(&(d.instance.clone(), d.window.id))
        })
    }

    fn approve(&self, task_id: &TaskId, instance: &Instance, windows: &[u32]) {
        let mut state = lock(&self.state);
        let a = state.approved.entry(task_id.clone()).or_default();
        a.instances.insert(instance.clone());
        for w in windows {
            a.windows.insert((instance.clone(), *w));
        }
    }
}

impl SessionManager {
    /// The helper's system permissions (starting it if needed): `Err` says why they couldn't
    /// be read.
    pub async fn computer_permissions(
        &self,
    ) -> std::result::Result<brigadier_computer::wire::Permissions, String> {
        let worker = TaskId("settings".into());
        let a = self
            .computer
            .request(
                &worker,
                Provider::Claude,
                Policy::default(),
                Op::Permissions,
            )
            .await
            .map_err(|e| e.detail)?;
        a.reply
            .permissions
            .ok_or_else(|| "Brigadier Computer Use didn't say".into())
    }

    /// Registers the helper with the system for `grant` (the system's own prompt), so it is
    /// listed in System Settings; the caller opens the pane.
    pub async fn request_computer_permission(
        &self,
        grant: brigadier_computer::wire::Grant,
    ) -> std::result::Result<brigadier_computer::wire::Permissions, String> {
        let worker = TaskId("settings".into());
        let a = self
            .computer
            .request(
                &worker,
                Provider::Claude,
                Policy::default(),
                Op::RequestPermission { grant },
            )
            .await
            .map_err(|e| e.detail)?;
        a.reply
            .permissions
            .ok_or_else(|| "Brigadier Computer Use didn't say".into())
    }

    /// How long the helper took from its start to its first answer, last time (S7).
    pub fn computer_cold_start_ms(&self) -> Option<f64> {
        self.computer.cold_start_ms()
    }

    /// Starts the computer-use helper with `starter` from now on (a fake, in tests).
    #[cfg(all(test, target_os = "macos"))]
    pub(crate) fn set_computer_starter(&self, starter: HelperStarter) {
        self.computer.set_starter(starter);
    }
}

/// Cancels a request in the helper unless its answer came.
struct CancelOnDrop {
    link: Option<Arc<dyn HelperLink>>,
    id: u64,
    worker: String,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(link) = self.link.take() {
            let id = link.next_id();
            link.send(
                id,
                &self.worker,
                Provider::Claude,
                Policy::default(),
                Op::Cancel { request: self.id },
                Box::new(|_| {}),
            );
        }
    }
}

/// The `.app` folder that holds `daemon_exe`, when it is bundled.
fn host_bundle(daemon_exe: &Path) -> Option<String> {
    daemon_exe
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(|p| p.to_string_lossy().into_owned())
}

/// The keys a batch leases: its window, and its app for keyboard, menu and focus work.
fn lease_keys(d: &Described, act: &ActRequest) -> Vec<LeaseKey> {
    let mut keys = vec![LeaseKey::Window(d.window.id)];
    if act.actions.iter().any(Action::uses_app_focus) {
        keys.push(LeaseKey::App(d.instance.clone()));
    }
    keys
}

/// The time a batch's `wait` actions ask for together.
fn batch_waits(act: &ActRequest) -> Duration {
    act.actions
        .iter()
        .map(|a| match a {
            Action::Wait { timeout_ms, .. } => Duration::from_millis(*timeout_ms),
            _ => Duration::ZERO,
        })
        .sum()
}

fn provider_of(kind: ProviderKind) -> Provider {
    match kind {
        ProviderKind::Codex => Provider::Codex,
        _ => Provider::Claude,
    }
}

/// What the model reads for an engine error.
fn error_text(e: &CuError) -> String {
    match e.code {
        ErrorCode::PermissionMissing => format!("{e} {PERMISSION_FIX}"),
        _ => e.to_string(),
    }
}

impl SessionManager {
    /// A worker's computer call (§4.6). `grant` is checked again after anything that waits.
    pub(crate) async fn computer_call(
        &self,
        grant: &str,
        conversation_id: ConversationId,
        task_id: TaskId,
        call: ComputerCall,
    ) -> ToolReply {
        match self
            .computer_call_inner(grant, &conversation_id, &task_id, call)
            .await
        {
            Ok(reply) => reply,
            Err(e) => ToolReply::error(error_text(&e)),
        }
    }

    async fn computer_call_inner(
        &self,
        grant: &str,
        conversation_id: &ConversationId,
        task_id: &TaskId,
        call: ComputerCall,
    ) -> Result<ToolReply, CuError> {
        let task = self
            .task_by_id(conversation_id, task_id)
            .await
            .map_err(|e| CuError::new(ErrorCode::NoSuchTarget, e.to_string()))?;
        let provider = provider_of(task.route.choice.provider);
        let label = format!("task-{} ({})", task.number, task.title);
        let full = self.permission(conversation_id) == PermissionLevel::FullAccess;
        let computer = &self.computer;
        let policy = computer.policy(task_id);
        match call {
            ComputerCall::Apps => {
                let a = computer
                    .request(task_id, provider, policy, Op::Apps)
                    .await?;
                Ok(reply_of(a))
            }
            ComputerCall::Observe(req) => {
                let a = computer
                    .request(task_id, provider, policy, Op::Observe(req))
                    .await?;
                Ok(reply_of(a))
            }
            ComputerCall::Zoom(req) => {
                let a = computer
                    .request(task_id, provider, policy, Op::Zoom(req))
                    .await?;
                Ok(reply_of(a))
            }
            ComputerCall::Launch(req) => {
                let what = match (&req.app, &req.open) {
                    (Some(app), Some(open)) => format!("{open} in {app}"),
                    (Some(app), None) => app.clone(),
                    (None, Some(open)) => open.clone(),
                    (None, None) => {
                        return Err(CuError::new(
                            ErrorCode::BadRequest,
                            "name an app or something to open",
                        ));
                    }
                };
                if !full {
                    self.ask_computer(
                        grant,
                        conversation_id,
                        task_id,
                        format!("Let {label} open {what} and use it"),
                        "It runs in the background; your cursor and keyboard stay yours. This approval covers what the launch opens, nothing else.".into(),
                    )
                    .await?;
                }
                let a = computer
                    .request(task_id, provider, policy, Op::Launch(req))
                    .await?;
                if let Some(l) = &a.reply.launched {
                    self.own_launch(task_id, l).await;
                }
                Ok(reply_of(a))
            }
            ComputerCall::Act(act) => {
                if batch_waits(&act) > MAX_BATCH_WAITS {
                    return Err(CuError::new(
                        ErrorCode::BadRequest,
                        "a batch's waits may take 300 s together; split it",
                    ));
                }
                let describe = |policy: Policy| {
                    computer.request(
                        task_id,
                        provider,
                        policy,
                        Op::Describe { window: act.window },
                    )
                };
                let d = describe(policy.clone())
                    .await?
                    .reply
                    .described
                    .ok_or_else(|| {
                        CuError::new(ErrorCode::Failed, "the helper didn't describe the window")
                    })?;
                if let Some(why) = &d.blocked {
                    return Err(CuError::new(ErrorCode::Blocked, why.clone()));
                }
                if !full && !computer.approved(task_id, &d) {
                    self.ask_computer(
                        grant,
                        conversation_id,
                        task_id,
                        format!("Let {label} use {} (window \"{}\")", d.app_name, d.window.title),
                        format!(
                            "It clicks and types in that window in the background; your cursor and keyboard stay yours. Only this window of this {} process (pid {}).",
                            d.app_name, d.instance.pid
                        ),
                    )
                    .await?;
                    // The window may have changed hands while the card waited.
                    let now = describe(computer.policy(task_id))
                        .await?
                        .reply
                        .described
                        .ok_or_else(|| {
                            CuError::new(ErrorCode::Failed, "the helper didn't describe the window")
                        })?;
                    if now.instance != d.instance {
                        return Err(CuError::new(
                            ErrorCode::StaleRef,
                            "that window belongs to another process now",
                        ));
                    }
                    computer.approve(task_id, &d.instance, &[d.window.id]);
                }
                let keys = lease_keys(&d, &act);
                computer.take_leases(task_id, &label, &keys)?;
                let answer = computer
                    .request(task_id, provider, computer.policy(task_id), Op::Act(act))
                    .await;
                computer.release_leases(task_id, &keys);
                let mut a = answer?;
                self.log_actions(conversation_id, task_id, &mut a).await;
                Ok(reply_of(a))
            }
        }
    }

    /// Asks the user once, on a card, and waits. A Stop, or the worker's end, ends the wait.
    async fn ask_computer(
        &self,
        grant: &str,
        conversation_id: &ConversationId,
        task_id: &TaskId,
        action: String,
        details: String,
    ) -> Result<(), CuError> {
        let mut changes = self.computer.changes.subscribe();
        let seen = *changes.borrow_and_update();
        let (card, mut rx) = self
            .open_approval(
                conversation_id,
                Some(task_id.clone()),
                ApprovalSubject::Action { action, details },
            )
            .await
            .map_err(|e| CuError::new(ErrorCode::Failed, e.to_string()))?;
        loop {
            tokio::select! {
                answer = &mut rx => {
                    return match answer {
                        Ok(CardAnswer::Decision(ApprovalDecision::Allow | ApprovalDecision::AllowSimilar))
                            if self.grants.resolve(grant).is_some() => Ok(()),
                        Ok(CardAnswer::Decision(ApprovalDecision::Deny { message })) => Err(CuError::new(
                            ErrorCode::Blocked,
                            if message.trim().is_empty() {
                                "the user declined".to_owned()
                            } else {
                                format!("the user declined: {message}")
                            },
                        )),
                        _ => Err(CuError::new(ErrorCode::Cancelled, "the worker ended")),
                    };
                }
                changed = changes.changed() => {
                    let gone = changed.is_err() || self.grants.resolve(grant).is_none();
                    if gone || *changes.borrow_and_update() != seen {
                        self.settle_approval(&card, CardState::Denied {
                            by: Decider::Policy,
                            message: Some("computer use was stopped".into()),
                        }).await;
                        return Err(CuError::new(ErrorCode::StoppedByUser, "computer use was stopped while the card waited"));
                    }
                }
            }
        }
    }

    /// Records what a launch created as the worker's: a new process in the cleanup ledger
    /// (quit when the worker ends), its windows as approved. A process the system handed
    /// back already running is never the worker's to quit.
    async fn own_launch(&self, task_id: &TaskId, l: &Launched) {
        if l.new_process {
            let artifact = Artifact::Process {
                pid: l.instance.pid as u32,
                started_at_ms: Some(l.instance.started_us as f64 / 1000.0),
            };
            if let Err(err) = self
                .runtime
                .ledger()
                .record(&format!("task:{task_id}"), artifact)
                .await
            {
                tracing::warn!(task = %task_id, error = %err, "could not record a launched app");
            }
        }
        self.computer.approve(task_id, &l.instance, &l.new_windows);
        lock(&self.computer.state)
            .launched
            .entry(task_id.clone())
            .or_default()
            .push(l.clone());
    }

    /// The batch's actions as session events, its annotated screenshot in the blob store. A
    /// conversation being deleted gets nothing new.
    async fn log_actions(
        &self,
        conversation_id: &ConversationId,
        task_id: &TaskId,
        a: &mut Answer,
    ) {
        if a.reply.records.is_empty() || self.is_closing(conversation_id) {
            return;
        }
        // A blob no event mentions is collected after the store's grace, so a crash between
        // the two writes leaves nothing behind.
        let image = match a.trajectory.take() {
            Some(png) => match self.core.store().blobs().put(png).await {
                Ok(hash) => Some(hash.to_string()),
                Err(err) => {
                    tracing::warn!(error = %err, "could not keep an action-log screenshot");
                    None
                }
            },
            None => None,
        };
        let events = a
            .reply
            .records
            .iter()
            .enumerate()
            .map(|(i, r)| DomainEvent::ComputerActed {
                conversation_id: conversation_id.clone(),
                task_id: task_id.clone(),
                action: ComputerAction {
                    at_ms: r.at_ms as i64,
                    kind: r.action.kind().into(),
                    app_window: r.window_title.clone(),
                    pid: r.pid,
                    window: r.window,
                    status: wire_name(&r.status).unwrap_or_default(),
                    rung: r.rung.as_ref().and_then(wire_name),
                    effect: r.effect.as_ref().and_then(wire_name),
                    error: r.error.map(|c| c.as_str().to_owned()),
                    dispatch_ms: r.timings.dispatch_ms,
                    record: serde_json::to_string(r).unwrap_or_default(),
                    image: if i == 0 { image.clone() } else { None },
                },
            })
            .collect();
        if let Err(err) = self.core.record_conversation(conversation_id, events).await {
            tracing::warn!(error = %err, "could not record computer actions");
        }
    }
}

/// An enum's name as it reads on the wire (`background_activated`).
fn wire_name(v: &impl serde::Serialize) -> Option<String> {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
}

/// The model's reply: the image first, then the text.
fn reply_of(a: Answer) -> ToolReply {
    let reply = if a.reply.ok {
        ToolReply::ok(a.reply.text)
    } else {
        ToolReply::error(match &a.reply.error {
            Some(e) => error_text(e),
            None => a.reply.text,
        })
    };
    let mut reply = match (a.image, a.reply.image) {
        (Some(png), Some(meta)) => reply.with_image(meta.mime, png),
        _ => reply,
    };
    reply.engine_us = Some((a.reply.engine_ms * 1000.0).round() as u64);
    reply
}

/// Where the helper lives and how it starts, from the daemon's own place.
fn real_starter(daemon_exe: &Path, data_dir: &Path) -> HelperStarter {
    let daemon_exe = daemon_exe.to_path_buf();
    let dir = data_dir.join("computer");
    Arc::new(move |on_event| {
        let (daemon_exe, dir) = (daemon_exe.clone(), dir.clone());
        Box::pin(async move {
            tokio::task::spawn_blocking(move || start_helper(&daemon_exe, &dir, on_event))
                .await
                .map_err(|e| e.to_string())?
        })
    })
}

#[cfg(unix)]
fn start_helper(
    daemon_exe: &Path,
    dir: &Path,
    on_event: Arc<dyn Fn(Event) + Send + Sync>,
) -> Result<Arc<dyn HelperLink>, String> {
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};

    if !cfg!(target_os = "macos") {
        return Err("computer use isn't available on this system yet".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    let socket = dir.join("helper.sock");
    let token_file = dir.join("helper.token");
    let _ = std::fs::remove_file(&token_file);
    let parent = std::process::id().to_string();
    let serve: Vec<String> = vec![
        "serve".into(),
        "--socket".into(),
        socket.to_string_lossy().into_owned(),
        "--token-file".into(),
        token_file.to_string_lossy().into_owned(),
        "--parent".into(),
        parent,
    ];
    let bundle = daemon_exe
        .parent()
        .and_then(Path::parent)
        .map(|contents| contents.join("Helpers/Brigadier Computer Use.app"))
        .filter(|b| b.exists());
    let direct: Option<PathBuf> = std::env::var_os("BRIGADIER_COMPUTER_HELPER")
        .map(PathBuf::from)
        .or_else(|| {
            bundle
                .is_none()
                .then(|| daemon_exe.with_file_name("brigadier-computer"))
        });
    let spawned = match (direct, bundle) {
        // Development: started as our child, it runs with the grants of whatever launched
        // the daemon (a terminal), not its own.
        (Some(bin), _) => Command::new(bin)
            .args(&serve)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ()),
        // The bundled helper, through LaunchServices: its own responsible process, so the
        // grants are "Brigadier Computer Use"'s. `-n`: never another daemon's helper.
        (None, Some(app)) => Command::new("/usr/bin/open")
            .args(["-n", "-g", "-a"])
            .arg(&app)
            .arg("--args")
            .args(&serve)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .and_then(|s| {
                if s.success() {
                    Ok(())
                } else {
                    Err(std::io::Error::other(format!("open exited with {s}")))
                }
            }),
        (None, None) => return Err("the helper app is missing from this install".into()),
    };
    spawned.map_err(|e| format!("couldn't start it: {e}"))?;
    let started = Instant::now();
    loop {
        if let Ok(token) = std::fs::read_to_string(&token_file)
            && !token.is_empty()
        {
            let on_event = on_event.clone();
            match brigadier_computer::client::Client::connect(&socket, token.trim(), move |e| {
                on_event(e)
            }) {
                Ok(client) => {
                    // The first answer is the cold start's end.
                    let (tx, rx) = std::sync::mpsc::channel();
                    let id = client.next_id();
                    client.send(
                        id,
                        "broker",
                        Provider::Claude,
                        Policy::default(),
                        Op::Ping,
                        move |a| {
                            let _ = tx.send(a.is_ok());
                        },
                    );
                    if rx.recv_timeout(START_TIMEOUT).unwrap_or(false) {
                        return Ok(Arc::new(client));
                    }
                }
                Err(_) if started.elapsed() < START_TIMEOUT => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        if started.elapsed() > START_TIMEOUT {
            return Err("it didn't answer in time".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(not(unix))]
fn start_helper(
    _: &Path,
    _: &Path,
    _: Arc<dyn Fn(Event) + Send + Sync>,
) -> Result<Arc<dyn HelperLink>, String> {
    Err("computer use isn't available on this system yet".into())
}

/// A fake helper for the broker's tests and the flow tests: a desktop of windows the test
/// lays out, and every op the broker sent.
#[cfg(test)]
pub(crate) mod fake {
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    use brigadier_computer::desktop::WindowInfo;
    use brigadier_computer::geom::Rect;
    use brigadier_computer::wire::Reply;

    use super::*;

    pub(crate) type Waiter = Box<dyn FnOnce(Result<Answer, Gone>) + Send>;
    pub(crate) type OnEvent = Arc<dyn Fn(Event) + Send + Sync>;

    /// What the fake desktop shows: each window's process, and what the next `launch` opens.
    #[derive(Default)]
    pub(crate) struct Desktop {
        pub windows: HashMap<u32, Instance>,
        pub launch: Option<(Instance, u32)>,
    }

    /// A helper connection that answers pings, cancels, session ends, describes of the
    /// desktop's windows, acts, and launches the desktop names at once, and holds everything
    /// else until the test says.
    #[derive(Default)]
    pub(crate) struct FakeLink {
        next: AtomicU64,
        pub(crate) sent: Mutex<Vec<(u64, String, Op, Policy)>>,
        pub(crate) held: Mutex<HashMap<u64, Waiter>>,
        dead: AtomicBool,
        desktop: Arc<Mutex<Desktop>>,
    }

    fn answer(reply: Reply) -> Result<Answer, Gone> {
        Ok(Answer {
            reply,
            image: None,
            trajectory: None,
        })
    }

    impl FakeLink {
        pub(crate) fn ops(&self) -> Vec<Op> {
            lock(&self.sent).iter().map(|s| s.2.clone()).collect()
        }

        /// The helper died: every waiting request learns it.
        pub(crate) fn die(&self) {
            self.dead.store(true, Ordering::SeqCst);
            let held: Vec<Waiter> = lock(&self.held).drain().map(|(_, w)| w).collect();
            for w in held {
                w(Err(Gone));
            }
        }
    }

    impl HelperLink for FakeLink {
        fn next_id(&self) -> u64 {
            self.next.fetch_add(1, Ordering::SeqCst) + 1
        }
        fn send(
            &self,
            id: u64,
            worker: &str,
            _: Provider,
            policy: Policy,
            op: Op,
            done: Box<dyn FnOnce(Result<Answer, Gone>) + Send>,
        ) {
            lock(&self.sent).push((id, worker.to_owned(), op.clone(), policy));
            if self.dead.load(Ordering::SeqCst) {
                return done(Err(Gone));
            }
            let ok = Reply {
                id,
                ok: true,
                ..Default::default()
            };
            match op {
                Op::Ping | Op::Cancel { .. } | Op::EndSession | Op::Act(_) => done(answer(ok)),
                Op::Describe { window } => {
                    let instance = lock(&self.desktop).windows.get(&window).cloned();
                    done(answer(match instance {
                        Some(instance) => Reply {
                            described: Some(Described {
                                window: WindowInfo {
                                    id: window,
                                    pid: instance.pid,
                                    title: format!("Window {window}"),
                                    frame: Rect::default(),
                                    on_screen: true,
                                    minimized: false,
                                },
                                instance,
                                app_name: "TextEdit".into(),
                                bundle_id: Some("com.apple.TextEdit".into()),
                                bundle_path: None,
                                blocked: None,
                            }),
                            ..ok
                        },
                        None => Reply {
                            ok: false,
                            error: Some(CuError::new(ErrorCode::NoSuchTarget, "no such window")),
                            ..ok
                        },
                    }))
                }
                Op::Launch(_) if lock(&self.desktop).launch.is_some() => {
                    let (instance, window) = {
                        let mut desktop = lock(&self.desktop);
                        let (instance, window) = desktop.launch.take().unwrap();
                        desktop.windows.insert(window, instance.clone());
                        (instance, window)
                    };
                    done(answer(Reply {
                        launched: Some(Launched {
                            instance,
                            app_name: "TextEdit".into(),
                            bundle_id: Some("com.apple.TextEdit".into()),
                            new_process: true,
                            new_windows: vec![window],
                            front_restored: false,
                        }),
                        ..ok
                    }))
                }
                _ => {
                    lock(&self.held).insert(id, done);
                }
            }
        }
        fn is_alive(&self) -> bool {
            !self.dead.load(Ordering::SeqCst)
        }
    }

    /// Hands out a fresh [`FakeLink`] on each start, keeping them and the broker's event hook.
    #[derive(Clone, Default)]
    pub(crate) struct FakeHelper {
        pub(crate) links: Arc<Mutex<Vec<Arc<FakeLink>>>>,
        pub(crate) events: Arc<Mutex<Option<OnEvent>>>,
        pub(crate) starts: Arc<AtomicUsize>,
        pub(crate) desktop: Arc<Mutex<Desktop>>,
    }

    impl FakeHelper {
        pub(crate) fn starter(&self) -> HelperStarter {
            let helper = self.clone();
            Arc::new(move |on_event| {
                helper.starts.fetch_add(1, Ordering::SeqCst);
                *lock(&helper.events) = Some(on_event);
                let link = Arc::new(FakeLink {
                    desktop: helper.desktop.clone(),
                    ..FakeLink::default()
                });
                lock(&helper.links).push(link.clone());
                Box::pin(async move { Ok(link as Arc<dyn HelperLink>) })
            })
        }
    }

    // What the flow tests (macOS only) drive the fake with.
    #[cfg(target_os = "macos")]
    impl FakeHelper {
        /// Every op the helpers were sent, oldest first.
        pub(crate) fn ops(&self) -> Vec<Op> {
            lock(&self.links).iter().flat_map(|l| l.ops()).collect()
        }

        /// Puts `window` of `instance` on the desktop (again: the window changed hands).
        pub(crate) fn show(&self, window: u32, instance: Instance) {
            lock(&self.desktop).windows.insert(window, instance);
        }

        /// The next `launch` starts `instance` with `window`.
        pub(crate) fn launches(&self, instance: Instance, window: u32) {
            lock(&self.desktop).launch = Some((instance, window));
        }

        /// The user's Stop, from the helper's menu.
        pub(crate) fn stop(&self) {
            let on_event = lock(&self.events).clone().expect("a helper started");
            on_event(Event::Stopped { by: "menu".into() });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::fake::{FakeHelper, FakeLink, OnEvent};
    use super::*;

    /// A broker on a fake helper, with the helper's links and event hook.
    struct Rig {
        computer: Computer,
        links: Arc<Mutex<Vec<Arc<FakeLink>>>>,
        events: Arc<Mutex<Option<OnEvent>>>,
        starts: Arc<AtomicUsize>,
    }

    fn rig() -> Rig {
        let helper = FakeHelper::default();
        Rig {
            computer: Computer::with_starter(
                Path::new("/x/Brigadier.app/Contents/MacOS/brigadierd"),
                helper.starter(),
            ),
            links: helper.links.clone(),
            events: helper.events.clone(),
            starts: helper.starts.clone(),
        }
    }

    fn task(n: &str) -> TaskId {
        TaskId(n.into())
    }

    fn instance(pid: i32) -> Instance {
        Instance { pid, started_us: 1 }
    }

    #[test]
    fn a_held_lease_names_its_holder_and_the_owner_ends_it() {
        let r = rig();
        let (a, b) = (task("a"), task("b"));
        let keys = [LeaseKey::Window(3), LeaseKey::App(instance(9))];
        r.computer.take_leases(&a, "task-1 (Check)", &keys).unwrap();
        let e = r
            .computer
            .take_leases(&b, "task-2", &[LeaseKey::Window(3)])
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Busy);
        assert!(e.detail.contains("task-1 (Check)"), "{e}");
        // Another window of another app is free.
        r.computer
            .take_leases(&b, "task-2", &[LeaseKey::Window(4)])
            .unwrap();
        // Released, it stays for the tail; its owner may take it again.
        r.computer.release_leases(&a, &keys);
        assert!(r.computer.take_leases(&b, "task-2", &keys[..1]).is_err());
        r.computer.take_leases(&a, "task-1 (Check)", &keys).unwrap();
        r.computer.release_leases(&a, &keys);
        // The worker's end frees it at once.
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(r.computer.end_worker(&a));
        r.computer.take_leases(&b, "task-2", &keys).unwrap();
    }

    #[tokio::test]
    async fn the_users_stop_revokes_every_lease_and_wakes_waiting_cards() {
        let r = rig();
        r.computer.link().await.unwrap();
        r.computer
            .take_leases(&task("a"), "task-1", &[LeaseKey::Window(3)])
            .unwrap();
        let mut changes = r.computer.changes.subscribe();
        let seen = *changes.borrow_and_update();
        let on_event = lock(&r.events).clone().unwrap();
        on_event(Event::Stopped { by: "menu".into() });
        assert!(lock(&r.computer.state).leases.is_empty());
        assert_ne!(*changes.borrow_and_update(), seen);
        r.computer
            .take_leases(&task("b"), "task-2", &[LeaseKey::Window(3)])
            .unwrap();
    }

    #[test]
    fn each_sessions_block_list_holds_only_its_own_launches() {
        let r = rig();
        r.computer.set_host_pid(77);
        let launched = |pid, new_process, window| Launched {
            instance: instance(pid),
            app_name: "Terminal".into(),
            bundle_id: Some("com.apple.Terminal".into()),
            new_process,
            new_windows: vec![window],
            front_restored: false,
        };
        {
            let mut s = lock(&r.computer.state);
            s.launched.insert(
                task("a"),
                vec![launched(50, true, 7), launched(60, false, 8)],
            );
        }
        let a = r.computer.policy(&task("a"));
        assert_eq!(
            a.launched_pids,
            vec![50],
            "a reused process isn't the session's"
        );
        assert_eq!(a.launched_windows, vec![7, 8]);
        assert_eq!(a.host_pid, Some(77));
        assert_eq!(a.host_bundle_path.as_deref(), Some("/x/Brigadier.app"));
        let b = r.computer.policy(&task("b"));
        assert!(b.launched_pids.is_empty() && b.launched_windows.is_empty());
        assert_eq!(b.host_pid, Some(77));
    }

    #[tokio::test]
    async fn a_dropped_call_cancels_its_request_in_the_helper() {
        let r = rig();
        let a = task("a");
        let call = r
            .computer
            .request(&a, Provider::Claude, Policy::default(), Op::Apps);
        // The caller gives up (an MCP cancel, a closed connection) before the answer.
        let gave_up = tokio::time::timeout(Duration::from_millis(50), call).await;
        assert!(gave_up.is_err());
        let link = lock(&r.links)[0].clone();
        let ops = link.ops();
        let id = lock(&link.sent)[0].0;
        assert_eq!(ops.last(), Some(&Op::Cancel { request: id }), "{ops:?}");
    }

    #[tokio::test]
    async fn a_crash_fails_the_running_call_and_the_next_one_starts_a_new_helper() {
        let r = Arc::new(rig());
        let r2 = r.clone();
        let running = tokio::spawn(async move {
            r2.computer
                .request(&task("a"), Provider::Claude, Policy::default(), Op::Apps)
                .await
        });
        while lock(&r.links)
            .first()
            .is_none_or(|l| lock(&l.held).is_empty())
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        lock(&r.links)[0].die();
        let e = running.await.unwrap().unwrap_err();
        assert_eq!(e.code, ErrorCode::AppNotResponding);
        assert!(e.detail.contains("isn't repeated"), "{e}");
        // The next call starts a second helper, which never sees the first call again.
        let a = task("a");
        let next = r
            .computer
            .request(&a, Provider::Claude, Policy::default(), Op::Ping);
        next.await.unwrap();
        assert_eq!(r.starts.load(Ordering::SeqCst), 2);
        assert_eq!(lock(&r.links)[1].ops(), vec![Op::Ping]);
    }

    #[test]
    fn a_batch_may_wait_300_seconds_in_all() {
        let wait = |ms| {
            serde_json::from_value::<Action>(serde_json::json!({
                "do": "wait", "expect": {"is": "appears", "find": "Done"}, "timeout_ms": ms
            }))
            .unwrap()
        };
        let act = |actions| ActRequest {
            window: 1,
            actions,
            screenshot: Default::default(),
        };
        assert_eq!(
            batch_waits(&act(vec![wait(200_000), wait(100_000)])),
            MAX_BATCH_WAITS
        );
        assert!(batch_waits(&act(vec![wait(200_000), wait(100_001)])) > MAX_BATCH_WAITS);
        // The worker's tool timeout outlasts the longest batch the engine allows.
        assert!(
            Duration::from_secs(crate::manager::workers::WORKER_TOOL_TIMEOUT_SECS)
                > MAX_BATCH_WAITS + brigadier_computer::engine::REQUEST_DEADLINE
        );
    }
}
