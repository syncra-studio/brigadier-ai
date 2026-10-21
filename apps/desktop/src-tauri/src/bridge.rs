//! The app's single connection to `brigadierd`, bridged to the webview.
//!
//! One task owns the connection: it (re)connects, launching the daemon when nothing is
//! listening, resubscribes from the last event it forwarded, pairs requests with responses,
//! and forwards events and metrics to the webview channel. Requests made while disconnected
//! wait in a bounded queue until the connection is back. It also raises the desktop
//! notifications: a request finished, and what needs the user (an approval, a question, a card
//! left unanswered for long), as Settings › General › Notifications allows.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_core::{
    AccountsView, ApprovalSubject, CardState, ConversationId, DomainEvent, NotifyFinished,
    Question, QuotaWarning, RequestState, Settings, UserRequest, WaitingItem, WaitingSource,
};
use brigadier_ipc::app::BridgeEvent;
use brigadier_ipc::protocol::{
    ClientFrame, ClientInfo, DaemonInfo, ErrorCode, EventEnvelope, IpcError, Outcome, Request,
    Response, ServerFrame,
};
use brigadier_ipc::{Connection, Reader};
use brigadier_sandbox::Platform;
use tauri::ipc::Channel;
use tokio::sync::{mpsc, oneshot, watch};

use crate::launcher::Launcher;

/// Requests waiting for a connection, at most.
const QUEUED_REQUESTS: usize = 256;
/// How long a request may wait (including reconnecting) before failing.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// A clone is answered when it ends: big repositories on slow links take a while.
const CLONE_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// Archive, restore and delete of a conversation wait for its cleanup under way (an Undo right
/// after an archive), and that waits for work already started, two minutes at most.
const LIFECYCLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Freeing up projects deletes each one's conversations in turn and waits for its Brain's scan
/// under way (two minutes at most per project).
const FREE_UP_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// Send feedback's write-up (two writer turns and GitHub reads), its filing (one `gh` call or
/// two per report) and its GitHub sign-in (up to 15 minutes) take minutes at worst; a timeout
/// would only lose the answer, as the daemon carries on.
const FEEDBACK_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// How long to wait for a freshly launched daemon to accept connections.
const LAUNCH_WAIT: Duration = Duration::from_secs(10);
const CONNECT_POLL: Duration = Duration::from_millis(20);
const MIN_BACKOFF: Duration = Duration::from_millis(100);
const MAX_BACKOFF: Duration = Duration::from_secs(2);
/// A connection that lasted this long resets the reconnect backoff.
const HEALTHY_CONNECTION: Duration = Duration::from_secs(5);

type Reply = oneshot::Sender<Result<Response, IpcError>>;

/// Shows a desktop notification as the app, given its title and body.
pub type Notify = Box<dyn Fn(&str, &str) + Send + Sync>;

/// Whether a Brigadier window is in front (shown and focused).
pub type InFront = Box<dyn Fn() -> bool + Send + Sync>;

/// Opens computer use's setup window for an agent waiting on it.
pub type OpenSetup = Box<dyn Fn() + Send + Sync>;

/// Where the connection task is. Quitting waits out an attempt in flight, since that attempt
/// may just have launched a daemon.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Link {
    Down,
    Connecting,
    Up,
}

struct Outgoing {
    request: Request,
    reply: Option<Reply>,
}

#[derive(Clone)]
pub struct Bridge {
    inner: Arc<Inner>,
}

struct Inner {
    platform: Arc<dyn Platform>,
    launcher: Launcher,
    requests: mpsc::Sender<Outgoing>,
    ui: Mutex<Option<Channel<BridgeEvent>>>,
    /// The computer-use setup window's feed: computer use's status only.
    setup_ui: Mutex<Option<Channel<BridgeEvent>>>,
    navigation: Mutex<Option<String>>,
    /// Latest connection state, replayed to a webview that subscribes late or reloads.
    status: Mutex<Option<BridgeEvent>>,
    link: watch::Sender<Link>,
    metrics_wanted: AtomicBool,
    /// Highest event `seq` forwarded to the webview; the resubscribe cursor.
    last_seq: AtomicI64,
    stopping: AtomicBool,
    notify: Notify,
    in_front: InFront,
    open_setup: OpenSetup,
    /// What was already notified (a "Waiting on you" item, an approval, a question, a request's
    /// finish), so an update doesn't notify again. Quota keys persist across restarts.
    notified: Mutex<NotificationKeys>,
}

impl Bridge {
    /// Starts the connection task on Tauri's async runtime.
    pub fn start(
        platform: Arc<dyn Platform>,
        launcher: Launcher,
        notify: Notify,
        in_front: InFront,
        open_setup: OpenSetup,
    ) -> Self {
        let (requests, queue) = mpsc::channel(QUEUED_REQUESTS);
        let notified = NotificationKeys::load(
            platform
                .paths()
                .data_dir
                .join("notified-quota-warnings.json"),
        );
        let bridge = Self {
            inner: Arc::new(Inner {
                platform,
                launcher,
                requests,
                ui: Mutex::new(None),
                setup_ui: Mutex::new(None),
                navigation: Mutex::new(None),
                status: Mutex::new(None),
                link: watch::channel(Link::Down).0,
                metrics_wanted: AtomicBool::new(false),
                last_seq: AtomicI64::new(-1),
                stopping: AtomicBool::new(false),
                notify,
                in_front,
                open_setup,
                notified: Mutex::new(notified),
            }),
        };
        let task = bridge.clone();
        tauri::async_runtime::spawn(async move { task.run(queue).await });
        bridge
    }

    #[cfg(target_os = "macos")]
    pub fn data_dir(&self) -> std::path::PathBuf {
        self.inner.platform.paths().data_dir.clone()
    }

    /// Sends a request and waits for its response.
    pub async fn request(&self, request: Request) -> Result<Response, IpcError> {
        if let Request::SetMetricsStreaming { enabled } = &request {
            self.inner.metrics_wanted.store(*enabled, Ordering::Release);
        }
        let timeout = match &request {
            Request::CloneProject { .. } => CLONE_TIMEOUT,
            Request::FreeUpProjects { .. } => FREE_UP_TIMEOUT,
            Request::Archive { .. } | Request::Restore { .. } | Request::Delete { .. } => {
                LIFECYCLE_TIMEOUT
            }
            Request::WriteUpFeedback { .. }
            | Request::FileFeedback { .. }
            | Request::WaitGitHubSignIn => FEEDBACK_TIMEOUT,
            _ => REQUEST_TIMEOUT,
        };
        let (reply, response) = oneshot::channel();
        let outgoing = Outgoing {
            request,
            reply: Some(reply),
        };
        let result = tokio::time::timeout(timeout, async {
            self.inner
                .requests
                .send(outgoing)
                .await
                .map_err(|_| unavailable("the bridge has stopped"))?;
            response
                .await
                .map_err(|_| unavailable("the daemon disconnected before answering"))?
        })
        .await;
        result.unwrap_or_else(|_| Err(unavailable("the daemon did not answer in time")))
    }

    /// Routes events to the webview. Sends the current connection state right away.
    pub fn attach_ui(&self, channel: Channel<BridgeEvent>) {
        if let Some(status) = self.inner.status.lock().expect("status lock").clone() {
            let _ = channel.send(status);
        }
        *self.inner.ui.lock().expect("ui lock") = Some(channel);
        let navigation = self
            .inner
            .navigation
            .lock()
            .expect("navigation lock")
            .take();
        if let Some(id) = navigation {
            self.emit(BridgeEvent::OpenConversation {
                conversation_id: id,
            });
        }
    }

    /// Routes computer use's status to the setup window (replacing a closed one's feed).
    pub fn attach_setup_ui(&self, channel: Channel<BridgeEvent>) {
        *self.inner.setup_ui.lock().expect("setup ui lock") = Some(channel);
    }

    /// Retains native activation until the webview is ready to receive it.
    #[cfg(target_os = "macos")]
    pub fn open_conversation(&self, conversation_id: String) {
        let ui = self.inner.ui.lock().expect("ui lock");
        if let Some(channel) = ui.as_ref() {
            let _ = channel.send(BridgeEvent::OpenConversation { conversation_id });
        } else {
            *self.inner.navigation.lock().expect("navigation lock") = Some(conversation_id);
        }
    }

    pub fn emit(&self, event: BridgeEvent) {
        if matches!(
            event,
            BridgeEvent::Connected { .. } | BridgeEvent::Disconnected { .. }
        ) {
            *self.inner.status.lock().expect("status lock") = Some(event.clone());
        }
        if matches!(event, BridgeEvent::ComputerUse { .. })
            && let Some(channel) = self.inner.setup_ui.lock().expect("setup ui lock").as_ref()
        {
            let _ = channel.send(event.clone());
        }
        if let Some(channel) = self.inner.ui.lock().expect("ui lock").as_ref() {
            let _ = channel.send(event);
        }
    }

    /// Asks the daemon to drain and exit, waiting for its acknowledgement, and stops
    /// reconnecting. A connection attempt in flight is waited for within `timeout`, so a daemon
    /// it launched is stopped too. Returns false if the daemon could not be reached.
    pub async fn shutdown_daemon(&self, timeout: Duration) -> bool {
        self.inner.stopping.store(true, Ordering::Release);
        let deadline = tokio::time::Instant::now() + timeout;
        let mut link = self.inner.link.subscribe();
        let up = matches!(
            tokio::time::timeout_at(deadline, link.wait_for(|link| *link != Link::Connecting)).await,
            Ok(Ok(link)) if *link == Link::Up
        );
        if !up {
            return false;
        }
        matches!(
            tokio::time::timeout_at(deadline, self.request(Request::Shutdown)).await,
            Ok(Ok(Response::Shutdown))
        )
    }

    async fn run(&self, mut queue: mpsc::Receiver<Outgoing>) {
        let mut backoff = MIN_BACKOFF;
        loop {
            // Announced before checking `stopping`: a quit either sees this attempt and waits
            // for it, or this check sees the quit.
            self.inner.link.send_replace(Link::Connecting);
            if self.inner.stopping.load(Ordering::Acquire) {
                self.inner.link.send_replace(Link::Down);
                return;
            }
            match self.connect().await {
                Ok((connection, daemon, last_seq)) => {
                    tracing::info!(pid = daemon.pid, "connected to brigadierd");
                    // Resume right after the last event we forwarded; first connection starts
                    // at the daemon's head.
                    let cursor = match self.inner.last_seq.load(Ordering::Acquire) {
                        -1 => last_seq,
                        seen => seen,
                    };
                    self.inner.last_seq.store(cursor, Ordering::Release);
                    self.emit(BridgeEvent::Connected {
                        daemon,
                        last_seq: cursor,
                    });
                    self.inner.link.send_replace(Link::Up);
                    let connected_at = tokio::time::Instant::now();
                    let reason = self.serve(connection, cursor, &mut queue).await;
                    self.inner.link.send_replace(Link::Down);
                    tracing::warn!(reason = %reason, "disconnected from brigadierd");
                    if self.inner.stopping.load(Ordering::Acquire) {
                        return;
                    }
                    self.emit(BridgeEvent::Disconnected { reason });
                    // A connection that dies right away (e.g. on a frame we cannot read) must
                    // not turn into a reconnect storm: back off unless it was healthy a while.
                    if connected_at.elapsed() < HEALTHY_CONNECTION {
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                    } else {
                        backoff = MIN_BACKOFF;
                    }
                }
                Err(reason) => {
                    self.inner.link.send_replace(Link::Down);
                    tracing::warn!(reason = %reason, "cannot reach brigadierd");
                    self.emit(BridgeEvent::Disconnected { reason });
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            }
        }
    }

    /// Connects, launching the daemon if nothing answers.
    async fn connect(&self) -> Result<(Connection, DaemonInfo, i64), String> {
        let client = ClientInfo {
            name: "Brigadier".into(),
            pid: std::process::id(),
        };
        if let Ok(connected) = brigadier_ipc::connect(&*self.inner.platform, client.clone()).await {
            return Ok(connected);
        }
        self.inner.launcher.ensure_launched().await?;
        let deadline = tokio::time::Instant::now() + LAUNCH_WAIT;
        loop {
            match brigadier_ipc::connect(&*self.inner.platform, client.clone()).await {
                Ok(connected) => return Ok(connected),
                Err(err) if tokio::time::Instant::now() >= deadline => {
                    return Err(format!("brigadierd did not start: {err}"));
                }
                Err(_) => tokio::time::sleep(CONNECT_POLL).await,
            }
        }
    }

    /// Serves one connection until it drops. Returns why it ended.
    async fn serve(
        &self,
        connection: Connection,
        cursor: i64,
        queue: &mut mpsc::Receiver<Outgoing>,
    ) -> String {
        let Connection { reader, mut writer } = connection;
        // Frames are read on their own task: reading is not cancel-safe inside select!.
        let (frames_tx, mut frames) = mpsc::channel(64);
        let reader_task = tauri::async_runtime::spawn(read_frames(reader, frames_tx));

        let mut pending: HashMap<u32, Option<Reply>> = HashMap::new();
        let mut next_id: u32 = 1;
        let subscribe = Outgoing {
            request: Request::Subscribe {
                after_seq: cursor,
                metrics: self.inner.metrics_wanted.load(Ordering::Acquire),
            },
            reply: None,
        };
        // The first subscription skips history. Recover active quota warnings from the
        // snapshot; repeat on reconnect in case an earlier snapshot request failed.
        let mut first = [
            subscribe,
            Outgoing {
                request: Request::GetAccounts,
                reply: None,
            },
        ]
        .into_iter();

        let reason = loop {
            let outgoing = if let Some(outgoing) = first.next() {
                Some(outgoing)
            } else {
                tokio::select! {
                    frame = frames.recv() => {
                        match frame {
                            Some(Ok(frame)) => {
                                if let Some(reason) = self.dispatch(frame, &mut pending) {
                                    break reason;
                                }
                            }
                            Some(Err(err)) => break err,
                            None => break "connection closed".to_owned(),
                        }
                        None
                    }
                    outgoing = queue.recv() => match outgoing {
                        Some(outgoing) => Some(outgoing),
                        None => break "bridge stopped".to_owned(),
                    },
                }
            };
            if let Some(Outgoing { request, reply }) = outgoing {
                // The caller gave up (it timed out while we reconnected): running its request
                // now would apply a change it already reported as failed.
                if reply.as_ref().is_some_and(Reply::is_closed) {
                    continue;
                }
                let id = next_id;
                next_id = next_id.wrapping_add(1).max(1);
                let frame = ClientFrame::Request { id, request };
                if let Err(err) = writer.write(&frame).await {
                    if let Some(reply) = reply {
                        let _ = reply.send(Err(unavailable("the daemon connection broke")));
                    }
                    break format!("write failed: {err}");
                }
                pending.insert(id, reply);
            }
        };
        reader_task.abort();
        // Dropping the pending replies fails those requests; the UI retries after reconnect.
        drop(pending);
        reason
    }

    /// Handles one frame from the daemon. Returns a reason when the connection should end.
    fn dispatch(
        &self,
        frame: ServerFrame,
        pending: &mut HashMap<u32, Option<Reply>>,
    ) -> Option<String> {
        match frame {
            ServerFrame::Response { id, result } => {
                if pending.get(&id).is_some_and(Option::is_none)
                    && let Outcome::Ok { value } = &result
                    && let Response::GetAccounts { accounts } = value.as_ref()
                {
                    self.notify_warnings(accounts);
                }
                if let Some(Some(reply)) = pending.remove(&id) {
                    let _ = reply.send(match result {
                        Outcome::Ok { value } => Ok(*value),
                        Outcome::Err { error } => Err(error),
                    });
                }
            }
            ServerFrame::Event { event } => {
                self.inner.last_seq.fetch_max(event.seq, Ordering::AcqRel);
                self.notify(&event);
                self.emit(BridgeEvent::Event { event });
            }
            ServerFrame::Lagged { resume_after } => {
                // The webview reloads its state; resume the live feed from the daemon's head.
                self.inner.last_seq.store(-1, Ordering::Release);
                self.emit(BridgeEvent::Lagged { resume_after });
                return Some("lagged behind the live feed".into());
            }
            ServerFrame::Metrics { metrics } => self.emit(BridgeEvent::Metrics { metrics }),
            ServerFrame::Terminal { output } => self.emit(BridgeEvent::Terminal { output }),
            ServerFrame::Dictation { update } => self.emit(BridgeEvent::Dictation { update }),
            ServerFrame::StorageClean { progress } => {
                self.emit(BridgeEvent::StorageClean { progress });
            }
            ServerFrame::PageChanged { id, revision } => {
                self.emit(BridgeEvent::PageChanged { id, revision });
            }
            ServerFrame::WorkspaceChanged { target } => {
                self.emit(BridgeEvent::WorkspaceChanged { target });
            }
            ServerFrame::Search { id, update } => {
                self.emit(BridgeEvent::Search { id, update });
            }
            ServerFrame::ComputerUse { status } => {
                crate::computer_menu::status_changed(&status);
                self.emit(BridgeEvent::ComputerUse { status });
            }
            ServerFrame::ComputerUseSetupNeeded => {
                (self.inner.open_setup)();
                (self.inner.notify)(
                    "Computer use needs setting up",
                    "An agent wants to use apps on this Mac. Finish setup in the window Brigadier opened.",
                );
            }
            ServerFrame::AppCall { call } => self.emit(BridgeEvent::AppCall { call }),
            ServerFrame::Closing => return Some("daemon is shutting down".into()),
            ServerFrame::Welcome { .. } => return Some("unexpected second welcome".into()),
        }
        None
    }

    /// Tells the user, once each, what an event brings that they would want to know while
    /// away: a request finished, or something needs them (a pending approval or question, a
    /// card left unanswered for long and now listed under "Waiting on you"). The feed starts at
    /// the daemon's head and resumes after the last event forwarded, so older ones don't notify
    /// again. Whether it shows is up to the settings, read when it happens.
    fn notify(&self, event: &EventEnvelope) {
        // Restored warned markers may precede the provider's first quota check. Its later
        // snapshot recovers warnings that were absent from the first GetAccounts response.
        let raw = event.event.0.get();
        if raw.contains("\"accountsChecked\"")
            && let Ok(DomainEvent::AccountsChecked { accounts }) = serde_json::from_str(raw)
        {
            self.notify_warnings(&accounts);
        }
        if let Some(notice) = notice_for(event) {
            self.notify_notice(notice);
        }
    }

    fn notify_warnings(&self, accounts: &AccountsView) {
        let now = brigadier_core::now_ms();
        for warning in &accounts.warnings {
            if warning.resets_at_ms > now {
                self.notify_notice(quota_notice(warning, now));
            }
        }
    }

    fn notify_notice(&self, notice: Notice) {
        // Not while the app quits: reading a window waits on the main thread, which
        // is waiting on the daemon.
        if crate::shell::is_quitting() {
            return;
        }
        let bridge = self.clone();
        tauri::async_runtime::spawn(async move {
            let Ok(Response::GetCatalog { catalog }) = bridge.request(Request::GetCatalog).await
            else {
                return;
            };
            if !allowed(notice.kind, &catalog.settings, (bridge.inner.in_front)()) {
                return;
            }
            if !bridge
                .inner
                .notified
                .lock()
                .expect("notified lock")
                .insert(notice.key.clone(), brigadier_core::now_ms())
            {
                return;
            }
            // A finished request is told under its conversation's name.
            let name = |id: &ConversationId| {
                catalog
                    .conversations
                    .iter()
                    .find(|conversation| &conversation.id == id)
                    .map(|conversation| conversation.title.clone())
                    .filter(|title| !title.trim().is_empty())
            };
            let title = match &notice.title {
                Title::Fixed(title) => title.clone(),
                Title::Conversation(id) => name(id).unwrap_or_else(|| "Finished".to_owned()),
                Title::AfterPeer { id, from } => {
                    after_peer(name(id).as_deref(), name(from).as_deref())
                }
            };
            (bridge.inner.notify)(&title, &notice.body);
        });
    }
}

/// What a desktop notification is about, as the settings tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A request is done, or ended with an error.
    Finished,
    /// An approval, a question or a card waits for the user.
    NeedsYou,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Title {
    Fixed(String),
    /// The conversation's name, read when the notification shows.
    Conversation(ConversationId),
    /// "‹title› finished · after a message from ‹sender›": a request another session's message
    /// started (SESSION-MESSAGING-PLAN.md §6.4), the names read when it shows.
    AfterPeer {
        id: ConversationId,
        from: ConversationId,
    },
}

/// The title of a finished request another session's message started, by the two sessions'
/// names (either may be gone).
fn after_peer(title: Option<&str>, from: Option<&str>) -> String {
    let title = title.unwrap_or("A session");
    match from {
        Some(from) => format!("{title} finished · after a message from {from}"),
        None => format!("{title} finished · after a message from another session"),
    }
}

/// A desktop notification an event asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Notice {
    /// Tells it apart from every other, so it shows once.
    key: String,
    kind: Kind,
    title: Title,
    body: String,
}

/// Whether the settings let a notification of `kind` show, with a Brigadier window in front
/// or not. "Needs you" ones show wherever the user is: what waits may be in another
/// conversation.
fn allowed(kind: Kind, settings: &Settings, in_front: bool) -> bool {
    match kind {
        Kind::NeedsYou => settings.notify_needs_you,
        Kind::Finished => match settings.notify_finished {
            NotifyFinished::Background => !in_front,
            NotifyFinished::Always => true,
            NotifyFinished::Never => false,
        },
    }
}

/// The notification an event asks for, if any.
fn notice_for(event: &EventEnvelope) -> Option<Notice> {
    if let Some(item) = stuck_card(event) {
        return Some(Notice {
            key: item.id,
            kind: Kind::NeedsYou,
            title: Title::Fixed("Waiting on you".to_owned()),
            body: item.what,
        });
    }
    if let Some((id, what)) = pending_approval(event) {
        return Some(Notice {
            key: format!("approval:{id}"),
            kind: Kind::NeedsYou,
            title: Title::Fixed("Awaiting approval".to_owned()),
            body: what,
        });
    }
    if let Some(question) = pending_question(event) {
        return Some(Notice {
            key: format!("question:{}", question.id.0),
            kind: Kind::NeedsYou,
            title: Title::Fixed("A question for you".to_owned()),
            body: question
                .round()
                .first()
                .map(|item| item.text.clone())
                .unwrap_or(question.text),
        });
    }
    if let Some(warning) = lost_quota(event) {
        return Some(quota_notice(&warning, event.at_ms));
    }
    let request = finished_request(event)?;
    let body = match &request.state {
        RequestState::Failed { error } => format!("Stopped with an error: {error}"),
        _ => format!("Done: {}", request.preview),
    };
    Some(Notice {
        key: format!(
            "finished:{}:{}",
            request.id,
            request.ended_at_ms.unwrap_or_default()
        ),
        kind: Kind::Finished,
        title: match request.from_peer {
            Some(from) => Title::AfterPeer {
                id: request.conversation_id,
                from,
            },
            None => Title::Conversation(request.conversation_id),
        },
        body,
    })
}

/// Snapshot recovery and live events share the same key and notification text.
fn quota_notice(warning: &QuotaWarning, at_ms: i64) -> Notice {
    let hours = ((warning.resets_at_ms - at_ms).max(0) as u64).div_ceil(3_600_000);
    Notice {
        key: format!(
            "quota:{}:{}:{}:{}",
            warning.account.provider,
            warning.account.key(),
            warning.window,
            warning.resets_at_ms
        ),
        kind: Kind::NeedsYou,
        title: Title::Fixed("Weekly quota resets soon".to_owned()),
        body: format!(
            "{} has {}% of its weekly quota left; it resets in {hours} h.",
            warning.who,
            warning.left_percent.round()
        ),
    }
}

/// Ordinary notices resume from the live cursor. Quota warnings also come from a snapshot,
/// so their keys survive app restarts. The bridge's mutex serializes writes and live events.
struct NotificationKeys {
    path: PathBuf,
    keys: HashSet<String>,
}

impl NotificationKeys {
    fn load(path: PathBuf) -> Self {
        let keys = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                tracing::warn!(%error, "could not read quota notification keys");
                HashSet::new()
            }),
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(%error, "could not read quota notification keys");
                }
                HashSet::new()
            }
        };
        Self { path, keys }
    }

    fn insert(&mut self, key: String, now_ms: i64) -> bool {
        let durable = key.starts_with("quota:");
        if durable && let Some((window, _)) = key.rsplit_once(':') {
            // Match the monitor: a provider may nudge a reset's timestamp. Until the
            // original notified reset passes, this account/window is already told.
            if self.keys.iter().any(|told| {
                told.rsplit_once(':').is_some_and(|(previous, reset)| {
                    previous == window && reset.parse::<i64>().is_ok_and(|reset| reset > now_ms)
                })
            }) {
                return false;
            }
        }
        if !self.keys.insert(key) {
            return false;
        }
        if durable {
            let keys: Vec<_> = self
                .keys
                .iter()
                .filter(|key| key.starts_with("quota:"))
                .collect();
            let temporary = self.path.with_extension("json.tmp");
            let result = (|| -> std::io::Result<()> {
                use std::io::Write;
                let mut file = std::fs::File::create(&temporary)?;
                file.write_all(&serde_json::to_vec(&keys)?)?;
                file.sync_all()?;
                std::fs::rename(&temporary, &self.path)
            })();
            if let Err(error) = result {
                let _ = std::fs::remove_file(&temporary);
                tracing::warn!(%error, "could not save quota notification keys");
            }
        }
        true
    }
}

/// A request an event says is over, done or failed, as the daemon settles requests from what
/// runs and waits. Not one the user stopped, one steered into another's answer (that one
/// tells), or an overnight run's (its report's notification tells).
fn finished_request(event: &EventEnvelope) -> Option<UserRequest> {
    let raw = event.event.0.get();
    // Most events aren't about requests: skip decoding them.
    if !raw.contains("\"requestUpdated\"") {
        return None;
    }
    let DomainEvent::RequestUpdated { request } = serde_json::from_str(raw).ok()? else {
        return None;
    };
    let over = matches!(
        request.state,
        RequestState::Done | RequestState::Failed { .. }
    );
    (over && request.steered_into.is_none() && !brigadier_core::is_run_request(&request.id))
        .then_some(request)
}

/// Weekly quota an event says is about to be lost unused.
fn lost_quota(event: &EventEnvelope) -> Option<QuotaWarning> {
    let raw = event.event.0.get();
    if !raw.contains("\"quotaWillBeLost\"") {
        return None;
    }
    let DomainEvent::QuotaWillBeLost { warning } = serde_json::from_str(raw).ok()? else {
        return None;
    };
    Some(warning)
}

/// A question an event asks the user, while it is unanswered.
fn pending_question(event: &EventEnvelope) -> Option<Question> {
    let raw = event.event.0.get();
    if !raw.contains("\"questionUpdated\"") {
        return None;
    }
    let DomainEvent::QuestionUpdated { question } = serde_json::from_str(raw).ok()? else {
        return None;
    };
    (question.answer.is_none() && question.answered_at_ms.is_none()).then_some(question)
}

/// An approval an event opens, as its id and what it asks in a line, if it opens one.
fn pending_approval(event: &EventEnvelope) -> Option<(String, String)> {
    let raw = event.event.0.get();
    // Most events aren't about approvals: skip decoding them.
    if !raw.contains("\"approvalUpdated\"") || !raw.contains("\"pending\"") {
        return None;
    }
    let DomainEvent::ApprovalUpdated { approval } = serde_json::from_str(raw).ok()? else {
        return None;
    };
    if approval.state != CardState::Pending {
        return None;
    }
    let what = match approval.subject {
        ApprovalSubject::Cli { request } => request
            .reason
            .filter(|reason| !reason.trim().is_empty())
            .or(request.command)
            .unwrap_or_else(|| format!("A worker asks to use {}.", request.tool)),
        // Only in recorded conversations: nothing asks this way any more (a merge is asked
        // for in words).
        ApprovalSubject::OutwardCommand { .. } | ApprovalSubject::FinishSession { .. } => {
            return None;
        }
        ApprovalSubject::Landing { branch, .. } => format!("Land a commit on {branch}?"),
        ApprovalSubject::Action { action, .. } => action,
        ApprovalSubject::Outline { title, .. } => format!("Implement this plan? {title}"),
        ApprovalSubject::PeerMessage { title, project, .. } => {
            format!("Send a message to {title} in {project}?")
        }
        ApprovalSubject::Browser { question, .. } => question,
    };
    Some((approval.id.0, what))
}

/// The "Waiting on you" item an event lists for an unanswered card, if it lists one.
fn stuck_card(event: &EventEnvelope) -> Option<WaitingItem> {
    let raw = event.event.0.get();
    // Most events aren't about waiting items: skip decoding them.
    if !raw.contains("\"waitingOnYou\"") {
        return None;
    }
    match serde_json::from_str(raw).ok()? {
        DomainEvent::WaitingOnYou { item } if matches!(item.source, WaitingSource::Card { .. }) => {
            Some(item)
        }
        _ => None,
    }
}

async fn read_frames(mut reader: Reader, frames: mpsc::Sender<Result<ServerFrame, String>>) {
    loop {
        let frame = match reader.read::<ServerFrame>().await {
            Ok(Some(frame)) => Ok(frame),
            Ok(None) => return,
            Err(err) => Err(format!("read failed: {err}")),
        };
        let failed = frame.is_err();
        if frames.send(frame).await.is_err() || failed {
            return;
        }
    }
}

fn unavailable(message: &str) -> IpcError {
    IpcError {
        code: ErrorCode::Internal,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use brigadier_core::{CardId, TaskId};
    use brigadier_ipc::protocol::RawJson;

    use super::*;

    fn envelope(event: &DomainEvent) -> EventEnvelope {
        EventEnvelope {
            seq: 1,
            stream: "conversation".into(),
            stream_seq: 1,
            at_ms: 0,
            event: RawJson(serde_json::value::to_raw_value(event).unwrap()),
        }
    }

    fn waiting(source: WaitingSource) -> DomainEvent {
        DomainEvent::WaitingOnYou {
            item: WaitingItem {
                id: "w1".into(),
                request_id: None,
                source,
                key: "k".into(),
                what: "Answer the card".into(),
                created_at_ms: 0,
            },
        }
    }

    #[test]
    fn quota_about_to_be_lost_notifies_with_who_and_when() {
        let warning: QuotaWarning = serde_json::from_str(
            r#"{"account": {"provider": "claude", "account": "work"}, "who": "Work (Claude Code)",
                "window": "seven_day", "leftPercent": 35.4, "resetsAtMs": 32340000}"#,
        )
        .unwrap();
        let event = DomainEvent::QuotaWillBeLost { warning };
        let notice = notice_for(&envelope(&event)).unwrap();
        assert_eq!(notice.kind, Kind::NeedsYou);
        assert_eq!(
            notice.body,
            "Work (Claude Code) has 35% of its weekly quota left; it resets in 9 h."
        );
    }

    #[test]
    fn only_stuck_cards_notify() {
        let card = waiting(WaitingSource::Card {
            card_id: CardId("c1".into()),
        });
        assert_eq!(
            stuck_card(&envelope(&card)).map(|item| item.what),
            Some("Answer the card".to_owned())
        );
        let task = waiting(WaitingSource::Task {
            task_id: TaskId("task-1".into()),
        });
        assert!(stuck_card(&envelope(&task)).is_none());
        assert!(stuck_card(&envelope(&waiting(WaitingSource::Orchestrator))).is_none());
    }

    fn approval(subject: ApprovalSubject, state: CardState) -> DomainEvent {
        DomainEvent::ApprovalUpdated {
            approval: brigadier_core::Approval {
                id: CardId("a1".into()),
                conversation_id: brigadier_core::ConversationId("c1".into()),
                task_id: None,
                request_id: None,
                position: 0,
                subject,
                state,
                created_at_ms: 0,
                resolved_at_ms: None,
            },
        }
    }

    #[test]
    fn pending_approvals_notify_with_what_they_ask() {
        let outline = ApprovalSubject::Outline {
            task_id: TaskId("task-1".into()),
            title: "Dark mode".into(),
            outline: "1. Add the switch".into(),
        };
        assert_eq!(
            pending_approval(&envelope(&approval(outline.clone(), CardState::Pending))),
            Some(("a1".to_owned(), "Implement this plan? Dark mode".to_owned()))
        );
        // A recorded merge card notifies no more: merging is asked for in words.
        let merge = ApprovalSubject::FinishSession {
            branch: "brigadier/s1".into(),
            base: "main".into(),
            commits: 2,
            diff_stat: Default::default(),
        };
        assert!(pending_approval(&envelope(&approval(merge, CardState::Pending))).is_none());
        // A settled approval doesn't notify.
        let answered = CardState::Expired {
            reason: "The worker stopped.".into(),
        };
        assert!(pending_approval(&envelope(&approval(outline, answered))).is_none());
        assert!(pending_approval(&envelope(&waiting(WaitingSource::Orchestrator))).is_none());
    }

    fn request(id: &str, state: RequestState) -> DomainEvent {
        DomainEvent::RequestUpdated {
            request: UserRequest {
                id: id.into(),
                conversation_id: ConversationId("c1".into()),
                preview: "Add dark mode".into(),
                state,
                started_at_ms: 0,
                ended_at_ms: Some(5),
                steered_into: None,
                steered_after: None,
                undo: None,
                worked: Vec::new(),
                waited: Vec::new(),
                closed: Vec::new(),
                quota_wait: false,
                waiting_for: Vec::new(),
                from_peer: None,
                held_by: None,
                background_jobs: None,
                changes: None,
                review: Default::default(),
                suggestion: None,
                deferred: false,
            },
        }
    }

    #[test]
    fn a_request_done_or_failed_notifies_once_under_its_conversation() {
        let done = notice_for(&envelope(&request("m1", RequestState::Done))).unwrap();
        assert_eq!(done.kind, Kind::Finished);
        assert_eq!(done.title, Title::Conversation(ConversationId("c1".into())));
        assert_eq!(done.body, "Done: Add dark mode");
        assert_eq!(done.key, "finished:m1:5");
        let failed = RequestState::Failed {
            error: "The model is overloaded.".into(),
        };
        assert_eq!(
            notice_for(&envelope(&request("m1", failed))).unwrap().body,
            "Stopped with an error: The model is overloaded."
        );
        // Still working or waiting, stopped by the user, or an overnight run's: no notice.
        for state in [
            RequestState::Working,
            RequestState::Waiting,
            RequestState::Stopped,
        ] {
            assert!(notice_for(&envelope(&request("m1", state))).is_none());
        }
        assert!(notice_for(&envelope(&request("run-ab12-g1", RequestState::Done))).is_none());
        let DomainEvent::RequestUpdated {
            request: mut steered,
        } = request("m2", RequestState::Done)
        else {
            unreachable!()
        };
        steered.steered_into = Some("m1".into());
        let steered = DomainEvent::RequestUpdated { request: steered };
        assert!(notice_for(&envelope(&steered)).is_none());
    }

    #[test]
    fn a_request_another_session_started_says_so_when_it_finishes() {
        let DomainEvent::RequestUpdated { mut request } = request("m3", RequestState::Done) else {
            unreachable!()
        };
        request.from_peer = Some(ConversationId("c2".into()));
        let done = notice_for(&envelope(&DomainEvent::RequestUpdated { request })).unwrap();
        assert_eq!(
            done.title,
            Title::AfterPeer {
                id: ConversationId("c1".into()),
                from: ConversationId("c2".into()),
            }
        );
        assert_eq!(
            after_peer(Some("Docs"), Some("API work")),
            "Docs finished · after a message from API work"
        );
        assert_eq!(
            after_peer(Some("Docs"), None),
            "Docs finished · after a message from another session"
        );
    }

    #[test]
    fn unanswered_questions_need_the_user() {
        let mut question = brigadier_core::Question {
            id: CardId("q1".into()),
            conversation_id: ConversationId("c1".into()),
            task_id: None,
            request_id: None,
            position: 0,
            kind: brigadier_core::QuestionKind::Orchestrator,
            text: "Which colour?".into(),
            options: vec!["Blue".into()],
            recommended: None,
            items: Vec::new(),
            answer: None,
            answers: Vec::new(),
            title: None,
            created_at_ms: 0,
            answered_at_ms: None,
            answered_by: None,
            words_id: None,
        };
        let asked = DomainEvent::QuestionUpdated {
            question: question.clone(),
        };
        let notice = notice_for(&envelope(&asked)).unwrap();
        assert_eq!(notice.kind, Kind::NeedsYou);
        assert_eq!(notice.body, "Which colour?");
        question.answer = Some("Blue".into());
        question.answered_at_ms = Some(1);
        assert!(notice_for(&envelope(&DomainEvent::QuestionUpdated { question })).is_none());
    }

    #[test]
    fn the_settings_decide_which_notifications_show() {
        let mut settings = Settings::default();
        // By default: finished only in the background, what needs the user always.
        assert!(allowed(Kind::Finished, &settings, false));
        assert!(!allowed(Kind::Finished, &settings, true));
        assert!(allowed(Kind::NeedsYou, &settings, true));
        settings.notify_finished = NotifyFinished::Always;
        assert!(allowed(Kind::Finished, &settings, true));
        settings.notify_finished = NotifyFinished::Never;
        assert!(!allowed(Kind::Finished, &settings, false));
        settings.notify_needs_you = false;
        assert!(!allowed(Kind::NeedsYou, &settings, false));
        assert!(!allowed(Kind::NeedsYou, &settings, true));
    }
}

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod connection_tests;
