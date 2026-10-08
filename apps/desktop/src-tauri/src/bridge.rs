//! The app's single connection to `brigadierd`, bridged to the webview.
//!
//! One task owns the connection: it (re)connects, launching the daemon when nothing is
//! listening, resubscribes from the last event it forwarded, pairs requests with responses,
//! and forwards events and metrics to the webview channel. Requests made while disconnected
//! wait in a bounded queue until the connection is back. It also raises the desktop
//! notification for a card the user left unanswered for long.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_core::{ApprovalSubject, CardState, DomainEvent, WaitingItem, WaitingSource};
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
    navigation: Mutex<Option<String>>,
    /// Latest connection state, replayed to a webview that subscribes late or reloads.
    status: Mutex<Option<BridgeEvent>>,
    link: watch::Sender<Link>,
    metrics_wanted: AtomicBool,
    /// Highest event `seq` forwarded to the webview; the resubscribe cursor.
    last_seq: AtomicI64,
    stopping: AtomicBool,
    notify: Notify,
    /// "Waiting on you" items and approvals already notified, so an update doesn't notify again.
    notified: Mutex<HashSet<String>>,
}

impl Bridge {
    /// Starts the connection task on Tauri's async runtime.
    pub fn start(platform: Arc<dyn Platform>, launcher: Launcher, notify: Notify) -> Self {
        let (requests, queue) = mpsc::channel(QUEUED_REQUESTS);
        let bridge = Self {
            inner: Arc::new(Inner {
                platform,
                launcher,
                requests,
                ui: Mutex::new(None),
                navigation: Mutex::new(None),
                status: Mutex::new(None),
                link: watch::channel(Link::Down).0,
                metrics_wanted: AtomicBool::new(false),
                last_seq: AtomicI64::new(-1),
                stopping: AtomicBool::new(false),
                notify,
                notified: Mutex::new(HashSet::new()),
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
            Request::Archive { .. } | Request::Restore { .. } | Request::Delete { .. } => {
                LIFECYCLE_TIMEOUT
            }
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
        let mut first = Some(subscribe);

        let reason = loop {
            let outgoing = if let Some(subscribe) = first.take() {
                Some(subscribe)
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
                if let Some(Some(reply)) = pending.remove(&id) {
                    let _ = reply.send(match result {
                        Outcome::Ok { value } => Ok(value),
                        Outcome::Err { error } => Err(error),
                    });
                }
            }
            ServerFrame::Event { event } => {
                self.inner.last_seq.fetch_max(event.seq, Ordering::AcqRel);
                self.notify_waiting(&event);
                self.notify_approval(&event);
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
            ServerFrame::Closing => return Some("daemon is shutting down".into()),
            ServerFrame::Welcome { .. } => return Some("unexpected second welcome".into()),
        }
        None
    }

    /// Tells the user, once per item, that a card they left unanswered for long is now
    /// listed under "Waiting on you". The feed starts at the daemon's head and resumes after
    /// the last event forwarded, so older items don't notify again.
    fn notify_waiting(&self, event: &EventEnvelope) {
        let Some(item) = stuck_card(event) else {
            return;
        };
        if self
            .inner
            .notified
            .lock()
            .expect("notified lock")
            .insert(item.id)
        {
            (self.inner.notify)("Waiting on you", &item.what);
        }
    }

    /// Tells the user, once per approval, that a worker or the orchestrator waits for their
    /// go-ahead (the thread shows the same request in the composer's place).
    fn notify_approval(&self, event: &EventEnvelope) {
        let Some((id, what)) = pending_approval(event) else {
            return;
        };
        if self
            .inner
            .notified
            .lock()
            .expect("notified lock")
            .insert(format!("approval:{id}"))
        {
            (self.inner.notify)("Awaiting approval", &what);
        }
    }
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
        ApprovalSubject::Outline { title, .. } => format!("Start this plan? {title}"),
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
            Some(("a1".to_owned(), "Start this plan? Dark mode".to_owned()))
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
}
