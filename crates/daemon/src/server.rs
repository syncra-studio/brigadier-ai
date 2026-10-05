//! Accepts IPC connections and serves requests and the live event feed.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use brigadier_core::manager::SessionManager;
use brigadier_core::runtime::{Runtime, StartRaw};
use brigadier_core::{Conversation, ConversationId, Core, MAX_ATTACHMENT_BYTES};
use brigadier_ipc::metrics::{DaemonMetrics, Diagnostics, budgets};
use brigadier_ipc::protocol::{
    ArtifactText, ClientFrame, ClientInfo, DaemonActivity, DaemonInfo, DictationUpdate, ErrorCode,
    EventEnvelope, IpcError, LifecycleOutcome, Outcome, RawJson, Request, Response, SendOutcome,
    ServerFrame, TerminalOutput,
};
use brigadier_ipc::{Accepted, Connection, Listener, Reader, Token, Writer};
use brigadier_providers::ProviderKind;
use brigadier_store::{Store, StoredEvent};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, mpsc, watch};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::awake::Awake;
use crate::dictation::Dictation;
use crate::idle;
use crate::metrics::Metrics;
use crate::storage::Storage;
use crate::supervisor::Supervisor;
use crate::terminals::Terminals;
use crate::uninstall::Uninstall;
use crate::updates::Updates;
use crate::upgrade;

/// Frames buffered between a connection's reader task and its handler.
const INBOUND_FRAMES: usize = 32;
/// Slow requests' answers waiting to be written.
const LATE_ANSWERS: usize = 8;
/// Page size when replaying committed events to a new subscriber.
const REPLAY_PAGE: u32 = 500;
/// A subscriber further behind than this resyncs through `eventsSince` instead.
const MAX_REPLAY: usize = 10_000;
/// Consecutive accept failures (e.g. out of file descriptors) before the daemon gives up.
const MAX_ACCEPT_FAILURES: u32 = 50;

/// State shared by every connection.
pub struct Daemon {
    pub info: DaemonInfo,
    pub core: Arc<Core>,
    /// Provider sessions and what Brigadier knows about each provider.
    pub runtime: Arc<Runtime>,
    /// Live sessions, Chats and workers; answers the Brigadier MCP tools.
    pub sessions: Arc<SessionManager>,
    pub store: Store,
    pub metrics: Arc<Metrics>,
    pub supervisor: Supervisor,
    /// Cancelled once the store has drained; connections then say goodbye and close.
    pub closing: CancellationToken,
    /// Asks the daemon to quit, saying why (a client asked, nobody used it for a while).
    pub quit: mpsc::Sender<&'static str>,
    /// Becomes true when all admitted writes are committed during shutdown.
    pub drained: watch::Receiver<bool>,
    pub connections: TaskTracker,
    /// The sessions' terminals (the side panel's Terminal tab).
    pub terminals: Terminals,
    /// The composer's dictation: its speech model and running dictations.
    pub dictation: Arc<Dictation>,
    /// Keeps the computer awake per the settings.
    pub awake: Arc<Awake>,
    /// Settings → Storage's scans and cleaning.
    pub storage: Storage,
    /// Uninstall Brigadier…'s plans and teardown.
    pub uninstall: Uninstall,
    /// Newer versions of Brigadier and the agent CLIs, and the update running.
    pub updates: Updates,
    next_connection: AtomicU64,
}

impl Daemon {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        info: DaemonInfo,
        core: Arc<Core>,
        runtime: Arc<Runtime>,
        sessions: Arc<SessionManager>,
        store: Store,
        metrics: Arc<Metrics>,
        supervisor: Supervisor,
        closing: CancellationToken,
        quit: mpsc::Sender<&'static str>,
        drained: watch::Receiver<bool>,
        dictation: Arc<Dictation>,
        awake: Arc<Awake>,
    ) -> Self {
        Self {
            info,
            core,
            runtime,
            sessions,
            store,
            metrics,
            supervisor,
            closing,
            quit,
            drained,
            connections: TaskTracker::new(),
            terminals: Terminals::new(),
            dictation,
            awake,
            storage: Storage::default(),
            uninstall: Uninstall::default(),
            updates: Updates::default(),
            next_connection: AtomicU64::new(1),
        }
    }
}

/// Accepts connections until shutdown begins. Each connection authenticates on its own task:
/// the app with the token, CLI sessions' MCP bridges with their grant.
pub async fn accept_loop(
    daemon: Arc<Daemon>,
    listener: Listener,
    token: Token,
) -> anyhow::Result<()> {
    let stopping = daemon.supervisor.shutdown_token().clone();
    let mut failures = 0;
    loop {
        let pending = tokio::select! {
            _ = stopping.cancelled() => return Ok(()),
            pending = listener.accept() => pending,
        };
        let pending = match pending {
            Ok(pending) => {
                failures = 0;
                pending
            }
            Err(err) => {
                failures += 1;
                if failures >= MAX_ACCEPT_FAILURES {
                    anyhow::bail!("accept keeps failing: {err}");
                }
                tracing::warn!(error = %err, "accept failed");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        let daemon_for_task = daemon.clone();
        let token = token.clone();
        let task = daemon.supervisor.monitor().instrument(async move {
            match pending.handshake(&token).await {
                Ok(Accepted::Client { connection, client }) => {
                    serve(daemon_for_task, connection, client).await
                }
                Ok(Accepted::Mcp { grant, stream }) => {
                    upgrade::serve_mcp(daemon_for_task, grant, stream).await
                }
                Err(err) => tracing::warn!(error = %err, "rejected IPC connection"),
            }
        });
        daemon.connections.spawn(task);
    }
}

async fn serve(daemon: Arc<Daemon>, connection: Connection, client: ClientInfo) {
    let id = daemon.next_connection.fetch_add(1, Ordering::Relaxed);
    tracing::info!(connection = id, client = %client.name, pid = client.pid, "client connected");
    daemon.metrics.connection_opened(id, client);
    let (late_tx, late) = mpsc::channel(LATE_ANSWERS);
    let mut session = Session {
        daemon: daemon.clone(),
        writer: connection.writer,
        feed: None,
        last_sent: 0,
        metrics: None,
        terminal_feed: None,
        terminals: HashSet::new(),
        dictation_feed: None,
        dictations: HashSet::new(),
        late_tx,
        late,
    };
    if let Err(err) = session.run(connection.reader).await {
        tracing::debug!(connection = id, error = %err, "connection ended with an error");
    }
    if session.metrics.is_some() {
        daemon.metrics.set_streaming(false);
    }
    daemon.metrics.connection_closed(id);
    tracing::info!(connection = id, "client disconnected");
}

struct Session {
    daemon: Arc<Daemon>,
    writer: Writer,
    /// Live event feed, once subscribed.
    feed: Option<broadcast::Receiver<Arc<StoredEvent>>>,
    /// Highest `seq` delivered to this client.
    last_sent: i64,
    metrics: Option<watch::Receiver<DaemonMetrics>>,
    /// Output of every terminal, once this connection opened one.
    terminal_feed: Option<broadcast::Receiver<TerminalOutput>>,
    /// The terminals this connection opened: only their output is forwarded.
    terminals: HashSet<String>,
    /// Dictation updates, once this connection dictated or downloaded the speech model.
    dictation_feed: Option<broadcast::Receiver<DictationUpdate>>,
    /// The dictations this connection started: only their text is forwarded.
    dictations: HashSet<String>,
    /// Answers to slow requests (clones), which run beside the others, by request id.
    late_tx: mpsc::Sender<(u32, Result<Response, IpcError>)>,
    late: mpsc::Receiver<(u32, Result<Response, IpcError>)>,
}

enum Flow {
    Continue,
    Close,
}

impl Session {
    async fn run(&mut self, reader: Reader) -> anyhow::Result<()> {
        self.writer
            .write(&ServerFrame::Welcome {
                daemon: self.daemon.info.clone(),
                last_seq: self.daemon.store.last_seq(),
            })
            .await?;

        // Reading a frame is not cancel-safe, so it happens on its own task and whole frames
        // arrive over a channel the select below can poll safely.
        let (frames_tx, mut frames) = mpsc::channel(INBOUND_FRAMES);
        let reader_task = self.daemon.supervisor.spawn(async move {
            let mut reader = reader;
            loop {
                match reader.read::<ClientFrame>().await {
                    Ok(Some(frame)) => {
                        if frames_tx.send(Ok(frame)).await.is_err() {
                            return;
                        }
                    }
                    Ok(None) => return,
                    Err(err) => {
                        let _ = frames_tx.send(Err(err)).await;
                        return;
                    }
                }
            }
        });

        let closing = self.daemon.closing.clone();
        let result = loop {
            tokio::select! {
                biased;
                _ = closing.cancelled() => {
                    let _ = self.writer.write(&ServerFrame::Closing).await;
                    break Ok(());
                }
                frame = frames.recv() => match frame {
                    Some(Ok(ClientFrame::Request { id, request })) => {
                        match self.handle(id, request).await? {
                            Flow::Continue => {}
                            Flow::Close => break Ok(()),
                        }
                    }
                    Some(Ok(_)) => {
                        break Err(anyhow::anyhow!("handshake frame after the handshake"));
                    }
                    Some(Err(err)) => break Err(err.into()),
                    None => break Ok(()),
                },
                event = next_event(&mut self.feed) => match event {
                    Ok(event) => {
                        if event.seq > self.last_sent {
                            self.last_sent = event.seq;
                            self.writer.write(&ServerFrame::Event { event: envelope(&event) }).await?;
                        }
                    }
                    Err(RecvError::Lagged(_)) => {
                        // Bounded feed: a slow client is cut off and resyncs from its cursor.
                        self.feed = None;
                        self.writer
                            .write(&ServerFrame::Lagged { resume_after: self.last_sent })
                            .await?;
                    }
                    Err(RecvError::Closed) => self.feed = None,
                },
                Some((id, outcome)) = self.late.recv() => self.respond(id, outcome).await?,
                sample = next_metrics(&mut self.metrics) => {
                    if let Some(metrics) = sample {
                        self.writer.write(&ServerFrame::Metrics { metrics }).await?;
                    }
                }
                output = next_terminal_output(&mut self.terminal_feed) => {
                    if let Some(output) = output {
                        let id = match &output {
                            TerminalOutput::Data { terminal_id, .. }
                            | TerminalOutput::Exited { terminal_id, .. } => terminal_id,
                        };
                        if self.terminals.contains(id) {
                            if matches!(output, TerminalOutput::Exited { .. }) {
                                self.terminals.remove(id);
                            }
                            self.writer.write(&ServerFrame::Terminal { output }).await?;
                        }
                    }
                }
                update = next_dictation_update(&mut self.dictation_feed) => {
                    if let Some(update) = update {
                        let ours = match &update {
                            DictationUpdate::Transcribed { dictation_id, .. }
                            | DictationUpdate::Failed { dictation_id, .. } => {
                                self.dictations.remove(dictation_id)
                            }
                            // The model is one for everyone.
                            _ => true,
                        };
                        if ours {
                            self.writer.write(&ServerFrame::Dictation { update }).await?;
                        }
                    }
                }
            }
        };
        reader_task.abort();
        result
    }

    async fn handle(&mut self, id: u32, request: Request) -> anyhow::Result<Flow> {
        let outcome = match request {
            Request::Subscribe { after_seq, metrics } => {
                self.set_metrics(metrics);
                self.subscribe(after_seq).await.map_err(internal)
            }
            Request::SetMetricsStreaming { enabled } => {
                self.set_metrics(enabled);
                Ok(Response::SetMetricsStreaming { enabled })
            }
            Request::OpenTerminal {
                conversation_id,
                session_id,
                cols,
                rows,
            } => self.open_terminal(&conversation_id, session_id.as_deref(), cols, rows),
            Request::OpenSetupTerminal {
                provider,
                install,
                cols,
                rows,
            } => self.open_setup_terminal(provider, install, cols, rows),
            Request::GetDictation => Ok(Response::GetDictation {
                dictation: self.daemon.dictation.status(),
            }),
            Request::DownloadDictationModel => {
                self.follow_dictation();
                self.daemon.dictation.download();
                Ok(Response::DownloadDictationModel)
            }
            Request::CancelDictationDownload => {
                self.daemon.dictation.cancel_download();
                Ok(Response::CancelDictationDownload)
            }
            Request::StartDictation => {
                self.follow_dictation();
                self.daemon
                    .dictation
                    .start()
                    .map(|dictation_id| {
                        self.dictations.insert(dictation_id.clone());
                        Response::StartDictation { dictation_id }
                    })
                    .map_err(IpcError::from)
            }
            Request::AppendDictation {
                dictation_id,
                audio,
            } => self
                .daemon
                .dictation
                .append(&dictation_id, &audio)
                .await
                .map(|()| Response::AppendDictation)
                .map_err(IpcError::from),
            Request::FinishDictation { dictation_id } => self
                .daemon
                .dictation
                .finish(&dictation_id)
                .map(|()| Response::FinishDictation)
                .map_err(IpcError::from),
            Request::CancelDictation { dictation_id } => {
                self.daemon.dictation.cancel(&dictation_id);
                self.dictations.remove(&dictation_id);
                Ok(Response::CancelDictation)
            }
            Request::CloneProject {
                url,
                parent,
                folder,
                name,
            } => {
                let sessions = self.daemon.sessions.clone();
                let late = self.late_tx.clone();
                self.daemon.supervisor.spawn(async move {
                    let outcome = sessions
                        .clone_project(url, parent, folder, name)
                        .await
                        .map(|project| Response::CloneProject {
                            project: Box::new(project),
                        })
                        .map_err(IpcError::from);
                    let _ = late.send((id, outcome)).await;
                });
                return Ok(Flow::Continue);
            }
            // Long: answered beside the connection's other requests.
            request @ (Request::ScanStorage
            | Request::PreviewDelete { .. }
            | Request::DeletesFinished { .. }
            | Request::CleanStorage { .. }
            | Request::PreviewRemoveProject { .. }
            | Request::RemoveProject { .. }
            | Request::PreviewUninstall { .. }
            | Request::Uninstall { .. }) => {
                let daemon = self.daemon.clone();
                let late = self.late_tx.clone();
                self.daemon.supervisor.spawn(async move {
                    let outcome = handle_request(&daemon, request).await;
                    let _ = late.send((id, outcome)).await;
                });
                return Ok(Flow::Continue);
            }
            // Archive, restore and delete of a conversation run in the order they were asked
            // for, beside the connection's other requests: one waiting for a cleanup (a restore
            // right after an archive) holds up nothing else. An id named twice counts once.
            mut request @ (Request::Archive { .. }
            | Request::Restore { .. }
            | Request::Delete { .. }) => {
                let (Request::Archive { ids } | Request::Restore { ids } | Request::Delete { ids }) =
                    &mut request
                else {
                    unreachable!("matched above")
                };
                let mut seen = HashSet::new();
                ids.retain(|id| seen.insert(id.clone()));
                let mut turn = self.daemon.sessions.in_order(ids);
                let daemon = self.daemon.clone();
                let late = self.late_tx.clone();
                self.daemon.supervisor.spawn(async move {
                    turn.ready().await;
                    let outcome = handle_request(&daemon, request).await;
                    drop(turn);
                    let _ = late.send((id, outcome)).await;
                });
                return Ok(Flow::Continue);
            }
            Request::RefreshRankings => {
                let daemon = self.daemon.clone();
                let admitted = self.daemon.sessions.refresh_rankings(async move {
                    crate::registry::check(&daemon).await;
                });
                let (response, start) = match admitted {
                    Ok((job_id, start)) => (Ok(Response::RefreshRankings { job_id }), start),
                    Err(error) => (Err(IpcError::from(error)), None),
                };
                self.respond(id, response).await?;
                if let Some(start) = start {
                    let _ = start.send(());
                }
                return Ok(Flow::Continue);
            }
            Request::Shutdown => {
                // Stop admission and drain first; acknowledge only once writes are committed.
                let _ = self.daemon.quit.try_send("quit requested by a client");
                let mut drained = self.daemon.drained.clone();
                let _ = drained.wait_for(|drained| *drained).await;
                self.respond(id, Ok(Response::Shutdown)).await?;
                return Ok(Flow::Close);
            }
            other => handle_request(&self.daemon, other).await,
        };
        self.respond(id, outcome).await?;
        Ok(Flow::Continue)
    }

    async fn respond(
        &mut self,
        id: u32,
        outcome: Result<Response, IpcError>,
    ) -> anyhow::Result<()> {
        self.writer
            .write(&ServerFrame::Response {
                id,
                result: Outcome::from(outcome),
            })
            .await?;
        Ok(())
    }

    /// Opens (or re-attaches to) a session's terminal; its output then comes to this
    /// connection.
    fn open_terminal(
        &mut self,
        conversation_id: &ConversationId,
        session_id: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> Result<Response, IpcError> {
        let cwd = self.daemon.sessions.checkout_dir(conversation_id)?;
        // Subscribed first, so no output between the scrollback and the feed is lost.
        if self.terminal_feed.is_none() {
            self.terminal_feed = Some(self.daemon.terminals.subscribe());
        }
        let terminal =
            self.daemon
                .terminals
                .open_session(&conversation_id.0, session_id, cwd, cols, rows)?;
        self.terminals.insert(terminal.id.clone());
        Ok(Response::OpenTerminal { terminal })
    }

    /// Opens (or re-attaches to) the terminal that sets up a CLI: its install or sign-in
    /// command runs in the home folder, and the terminal ends with it.
    fn open_setup_terminal(
        &mut self,
        provider: ProviderKind,
        install: bool,
        cols: u16,
        rows: u16,
    ) -> Result<Response, IpcError> {
        let env = self.daemon.runtime.cli_env();
        let cwd = env
            .home()
            .ok_or_else(|| IpcError::from(brigadier_core::Error::Invalid("no home folder".into())))?
            .display()
            .to_string();
        let command = if install {
            provider.install_command(cfg!(windows)).to_owned()
        } else {
            // The binary Brigadier found, so the sign-in is the one Brigadier will use.
            let (binary, arguments) = provider
                .login_command()
                .split_once(' ')
                .unwrap_or((provider.login_command(), ""));
            let binary = env.resolve(provider).map_or_else(
                || binary.to_owned(),
                |path| shell_quote(&path.display().to_string()),
            );
            format!("{binary} {arguments}")
        };
        if self.terminal_feed.is_none() {
            self.terminal_feed = Some(self.daemon.terminals.subscribe());
        }
        let terminal = self.daemon.terminals.open(
            &format!("setup:{provider}"),
            cwd,
            cols,
            rows,
            Some(&command),
        )?;
        self.terminals.insert(terminal.id.clone());
        Ok(Response::OpenSetupTerminal { terminal })
    }

    /// Forwards dictation updates to this connection from now on.
    fn follow_dictation(&mut self) {
        if self.dictation_feed.is_none() {
            self.dictation_feed = Some(self.daemon.dictation.subscribe());
        }
    }

    fn set_metrics(&mut self, enabled: bool) {
        match (enabled, self.metrics.is_some()) {
            (true, false) => {
                self.metrics = Some(self.daemon.metrics.subscribe());
                self.daemon.metrics.set_streaming(true);
            }
            (false, true) => {
                self.metrics = None;
                self.daemon.metrics.set_streaming(false);
            }
            _ => {}
        }
    }

    /// Starts the live feed, first replaying committed events after `after_seq`.
    async fn subscribe(&mut self, after_seq: i64) -> Result<Response, brigadier_store::Error> {
        // Subscribe before reading so nothing committed in between is missed; duplicates are
        // skipped by `last_sent`.
        let feed = self.daemon.store.subscribe();
        let mut cursor = after_seq.max(0);
        let mut replayed = 0;
        loop {
            let page = self.daemon.store.read_since(cursor, REPLAY_PAGE).await?;
            for event in &page {
                cursor = event.seq;
                let frame = ServerFrame::Event {
                    event: envelope(event),
                };
                if self.writer.write_buffered(&frame).await.is_err() {
                    return Err(brigadier_store::Error::Io(
                        std::io::ErrorKind::BrokenPipe.into(),
                    ));
                }
            }
            replayed += page.len();
            if page.len() < REPLAY_PAGE as usize {
                break;
            }
            if replayed >= MAX_REPLAY {
                self.last_sent = cursor;
                self.feed = None;
                let _ = self
                    .writer
                    .write_buffered(&ServerFrame::Lagged {
                        resume_after: cursor,
                    })
                    .await;
                return Ok(Response::Subscribe { last_seq: cursor });
            }
        }
        self.last_sent = cursor;
        self.feed = Some(feed);
        Ok(Response::Subscribe { last_seq: cursor })
    }
}

async fn next_event(
    feed: &mut Option<broadcast::Receiver<Arc<StoredEvent>>>,
) -> Result<Arc<StoredEvent>, RecvError> {
    match feed {
        Some(feed) => feed.recv().await,
        None => std::future::pending().await,
    }
}

async fn next_terminal_output(
    feed: &mut Option<broadcast::Receiver<TerminalOutput>>,
) -> Option<TerminalOutput> {
    match feed {
        Some(rx) => match rx.recv().await {
            Ok(output) => Some(output),
            // A connection that fell behind misses some output; the shell goes on.
            Err(RecvError::Lagged(_)) => None,
            Err(RecvError::Closed) => {
                *feed = None;
                None
            }
        },
        None => std::future::pending().await,
    }
}

async fn next_dictation_update(
    feed: &mut Option<broadcast::Receiver<DictationUpdate>>,
) -> Option<DictationUpdate> {
    match feed {
        Some(rx) => match rx.recv().await {
            Ok(update) => Some(update),
            // Progress is superseded by the next update; the ends are rare enough not to lag.
            Err(RecvError::Lagged(_)) => None,
            Err(RecvError::Closed) => {
                *feed = None;
                None
            }
        },
        None => std::future::pending().await,
    }
}

async fn next_metrics(
    metrics: &mut Option<watch::Receiver<DaemonMetrics>>,
) -> Option<DaemonMetrics> {
    match metrics {
        Some(rx) => match rx.changed().await {
            Ok(()) => Some(rx.borrow_and_update().clone()),
            Err(_) => {
                *metrics = None;
                None
            }
        },
        None => std::future::pending().await,
    }
}

fn envelope(event: &StoredEvent) -> EventEnvelope {
    EventEnvelope {
        seq: event.seq,
        stream: event.stream.clone(),
        stream_seq: event.stream_seq,
        at_ms: event.at_ms,
        event: RawJson(event.payload.clone()),
    }
}

fn invalid(message: String) -> IpcError {
    IpcError {
        code: ErrorCode::Invalid,
        message,
    }
}

/// Each conversation's outcome, with the error in plain words.
fn lifecycle_outcomes(
    ids: Vec<ConversationId>,
    outcomes: impl IntoIterator<Item = brigadier_core::Result<Option<Conversation>>>,
) -> Vec<LifecycleOutcome> {
    ids.into_iter()
        .zip(outcomes)
        .map(|(id, outcome)| match outcome {
            Ok(conversation) => LifecycleOutcome {
                id,
                conversation: conversation.map(Box::new),
                error: None,
            },
            Err(err) => LifecycleOutcome {
                id,
                conversation: None,
                error: Some(match err {
                    // The row names it already; its id would only be noise.
                    brigadier_core::Error::NotFound(_) => "it's already gone".to_owned(),
                    err => IpcError::from(err).message,
                }),
            },
        })
        .collect()
}

fn internal(err: brigadier_store::Error) -> IpcError {
    IpcError::from(brigadier_core::Error::from(err))
}

async fn handle_request(daemon: &Arc<Daemon>, request: Request) -> Result<Response, IpcError> {
    let core = &daemon.core;
    let sessions = &daemon.sessions;
    Ok(match request {
        Request::GetCatalog => Response::GetCatalog {
            catalog: Box::new(core.visible_catalog()),
        },
        Request::GetActivity => Response::GetActivity {
            activity: core.activity().await,
        },
        Request::CreateProject { name, repo } => {
            let project = core.create_project(name, repo).await?;
            sessions.project_changed(&project).await;
            Response::CreateProject {
                project: Box::new(project),
            }
        }
        Request::UpdateProject { id, patch } => {
            let project = core.update_project(id, patch).await?;
            sessions.project_changed(&project).await;
            Response::UpdateProject {
                project: Box::new(project),
            }
        }
        Request::CreateConversation {
            kind,
            project_id,
            title,
            setup,
        } => Response::CreateConversation {
            conversation: Box::new(
                sessions
                    .create_conversation(kind, project_id, title, setup)
                    .await?,
            ),
        },
        Request::ForkConversation {
            conversation_id,
            message_id,
            place,
        } => Response::ForkConversation {
            conversation: Box::new(sessions.fork(conversation_id, message_id, place).await?),
        },
        Request::UpdateSetup { id, setup } => Response::UpdateSetup {
            conversation: Box::new(sessions.set_setup(id, setup).await?),
        },
        Request::GetConversation { id, limit } => Response::GetConversation {
            view: Box::new(core.conversation_view(id, limit).await?),
        },
        Request::SendMessage {
            conversation_id,
            text,
            attachments,
            mentions,
            steer,
            queue_index,
        } => Response::SendMessage {
            outcome: match sessions
                .send_message(
                    conversation_id,
                    text,
                    attachments,
                    mentions,
                    steer,
                    queue_index,
                )
                .await?
            {
                brigadier_core::manager::SendOutcome::Sent(message) => SendOutcome::Sent {
                    message: Box::new(message),
                },
                brigadier_core::manager::SendOutcome::Queued(item) => SendOutcome::Queued { item },
            },
        },
        Request::EditQueued {
            conversation_id,
            item_id,
            text,
            attachments,
            mentions,
        } => Response::EditQueued {
            queue: core
                .edit_queued(&conversation_id, &item_id, text, attachments, mentions)
                .await?,
        },
        Request::DeleteQueued {
            conversation_id,
            item_id,
        } => {
            core.take_queued(&conversation_id, &item_id).await?;
            Response::DeleteQueued {
                queue: core.conversation_view(conversation_id, 1).await?.queue,
            }
        }
        Request::MoveQueued {
            conversation_id,
            item_id,
            index,
        } => Response::MoveQueued {
            queue: core.move_queued(&conversation_id, &item_id, index).await?,
        },
        Request::ResumeQueue { conversation_id } => Response::ResumeQueue {
            queue: sessions.resume_queue(conversation_id).await?,
        },
        Request::AddAttachment {
            name,
            mime,
            data,
            pasted,
        } => {
            if data.len() > MAX_ATTACHMENT_BYTES.div_ceil(3) * 4 {
                return Err(IpcError {
                    code: ErrorCode::Invalid,
                    message: "the attachment is too large".into(),
                });
            }
            let bytes = BASE64.decode(data.as_bytes()).map_err(|err| IpcError {
                code: ErrorCode::Invalid,
                message: format!("the attachment is not valid base64: {err}"),
            })?;
            Response::AddAttachment {
                attachment: core.add_attachment(name, mime, bytes, pasted).await?,
            }
        }
        Request::PinDraftAttachments { scope, attachments } => {
            core.pin_draft_attachments(scope, attachments).await?;
            Response::PinDraftAttachments
        }
        Request::ReadAttachment { id } => Response::ReadAttachment {
            data: BASE64.encode(core.read_attachment(&id).await?),
        },
        Request::ListWorkerEvents {
            task_id,
            before,
            limit,
        } => Response::ListWorkerEvents {
            page: core.list_worker_events(&task_id, before, limit).await?,
        },
        Request::ListOrchestratorLog {
            conversation_id,
            before,
            limit,
        } => Response::ListOrchestratorLog {
            page: core
                .list_orchestrator_log(&conversation_id, before, limit)
                .await?,
        },
        Request::ReadArtifact { id, offset, limit } => {
            let (bytes, total_bytes) = core.read_blob_range(id, offset, limit).await?;
            let text = match String::from_utf8(bytes) {
                Ok(text) => Some(text),
                // A slice can end inside a character; keep what decodes.
                Err(err) if err.utf8_error().error_len().is_none() => {
                    let valid = err.utf8_error().valid_up_to();
                    let mut bytes = err.into_bytes();
                    bytes.truncate(valid);
                    String::from_utf8(bytes).ok()
                }
                Err(_) => None,
            };
            Response::ReadArtifact {
                text: ArtifactText {
                    binary: text.is_none(),
                    text: text.unwrap_or_default(),
                    offset,
                    total_bytes,
                },
            }
        }
        Request::SaveArtifact { id, path } => {
            sessions.save_artifact(id, path).await?;
            Response::SaveArtifact
        }
        Request::OpenArtifact { id, file_name } => Response::OpenArtifact {
            path: sessions
                .artifact_copy(id, file_name)
                .await?
                .display()
                .to_string(),
        },
        Request::GetBrain { project_id } => Response::GetBrain {
            overview: Box::new(sessions.brain_overview(project_id).await?),
        },
        Request::QueryBrain { project_id, query } => Response::QueryBrain {
            answer: Box::new(sessions.query_brain(project_id, query).await?),
        },
        Request::GetBrainGraph { project_id, filter } => Response::GetBrainGraph {
            graph: Box::new(sessions.brain_graph(project_id, filter).await?),
        },
        Request::ListMemories => Response::ListMemories {
            memories: sessions.list_memories().await?,
        },
        Request::ForgetMemory { node_id } => {
            sessions.forget_memory(node_id).await?;
            Response::ForgetMemory
        }
        Request::ExportConventions { project_id, path } => Response::ExportConventions {
            export: sessions.export_conventions(project_id, path).await?,
        },
        Request::RunBrainJob { project_id, kind } => Response::RunBrainJob {
            job_id: sessions.run_brain_job(project_id, kind).await?,
        },
        Request::RebuildIndex { project_id } => {
            sessions.rebuild_index(project_id).await?;
            Response::RebuildIndex
        }
        Request::GetRepoInfo { path } => Response::GetRepoInfo {
            repo: sessions.repo_info(path).await?,
        },
        Request::FindProjects => Response::FindProjects {
            candidates: sessions.find_projects().await?,
        },
        Request::BrowseFolders { path } => Response::BrowseFolders {
            listing: sessions.browse_folders(path).await?,
        },
        Request::CheckFolder { path } => Response::CheckFolder {
            check: sessions.check_folder(path).await?,
        },
        Request::AddProject { path, name, init } => Response::AddProject {
            project: Box::new(sessions.add_project(path, name, init).await?),
        },
        Request::ListFiles {
            conversation_id,
            query,
        } => {
            let (files, truncated) = sessions.list_files(&conversation_id, query).await?;
            Response::ListFiles { files, truncated }
        }
        Request::ReadFile {
            conversation_id,
            path,
        } => Response::ReadFile {
            file: sessions.read_file(&conversation_id, path).await?,
        },
        Request::OpenTerminal { .. } | Request::OpenSetupTerminal { .. } => {
            return Err(IpcError::from(brigadier_core::Error::Invalid(
                "a terminal opens on the connection that shows it".into(),
            )));
        }
        Request::CloneProject { .. } => {
            return Err(IpcError::from(brigadier_core::Error::Invalid(
                "a clone is answered on the connection that asked for it".into(),
            )));
        }
        Request::OpenSideChat { conversation_id } => Response::OpenSideChat {
            conversation: Box::new(sessions.open_side_chat(&conversation_id).await?),
        },
        Request::WriteTerminal { terminal_id, data } => {
            daemon.terminals.write(&terminal_id, &data)?;
            Response::WriteTerminal
        }
        Request::ResizeTerminal {
            terminal_id,
            cols,
            rows,
        } => {
            daemon.terminals.resize(&terminal_id, cols, rows)?;
            Response::ResizeTerminal
        }
        Request::CloseTerminal { terminal_id } => {
            daemon.terminals.close(&terminal_id);
            Response::CloseTerminal
        }
        Request::GetDictation
        | Request::DownloadDictationModel
        | Request::CancelDictationDownload
        | Request::StartDictation
        | Request::AppendDictation { .. }
        | Request::FinishDictation { .. }
        | Request::CancelDictation { .. } => {
            return Err(IpcError::from(brigadier_core::Error::Invalid(
                "dictation runs on the connection that shows it".into(),
            )));
        }
        Request::RateMessage {
            conversation_id,
            subject,
            rating,
        } => {
            sessions.rate(&conversation_id, subject, rating).await?;
            Response::RateMessage
        }
        Request::GetSessionDiff { id } => Response::GetSessionDiff {
            stat: sessions.session_diff_stat(&id).await?,
        },
        Request::GetWorkerDiffs { conversation_id } => Response::GetWorkerDiffs {
            diffs: sessions.worker_diffs(&conversation_id).await?,
        },
        Request::SteerQueued {
            conversation_id,
            item_id,
        } => {
            sessions.steer_queued(conversation_id, item_id).await?;
            Response::SteerQueued
        }
        Request::Interrupt { conversation_id } => {
            sessions.interrupt(conversation_id).await?;
            Response::Interrupt
        }
        Request::Resume { conversation_id } => {
            sessions.resume(conversation_id).await?;
            Response::Resume
        }
        Request::Compact { conversation_id } => {
            sessions.compact(conversation_id).await?;
            Response::Compact
        }
        Request::GetConversationStatus { conversation_id } => Response::GetConversationStatus {
            status: sessions.conversation_status(conversation_id).await?,
        },
        Request::EditMessage {
            conversation_id,
            message_id,
            text,
            attachments,
        } => {
            sessions
                .edit_message(conversation_id, message_id, text, attachments)
                .await?;
            Response::EditMessage
        }
        Request::Regenerate {
            conversation_id,
            request_id,
        } => {
            sessions.regenerate(conversation_id, request_id).await?;
            Response::Regenerate
        }
        Request::SwitchBranch {
            conversation_id,
            head,
        } => {
            sessions.switch_branch(conversation_id, head).await?;
            Response::SwitchBranch
        }
        Request::UndoChanges {
            conversation_id,
            request_id,
            reapply,
        } => {
            sessions
                .undo_request(conversation_id, request_id, reapply)
                .await?;
            Response::UndoChanges
        }
        Request::GetGitState { conversation_id } => Response::GetGitState {
            state: sessions.git_state(&conversation_id).await?,
        },
        Request::GetPullRequest { conversation_id } => Response::GetPullRequest {
            pull_request: sessions.pull_request(&conversation_id).await?,
        },
        Request::CommitChanges {
            conversation_id,
            message,
            include_unstaged,
            push,
        } => Response::CommitChanges {
            outcome: sessions
                .commit_changes(&conversation_id, message, include_unstaged, push)
                .await?,
        },
        Request::PushChanges { conversation_id } => Response::PushChanges {
            branch: sessions.push_changes(&conversation_id).await?,
        },
        Request::GetReviewDiff {
            conversation_id,
            scope,
            whole_files,
            ignore_whitespace,
        } => Response::GetReviewDiff {
            review: sessions
                .review_diff(&conversation_id, scope, whole_files, ignore_whitespace)
                .await?,
        },
        Request::AnswerCard {
            conversation_id,
            card_id,
            decision,
        } => {
            sessions
                .answer_card(conversation_id, card_id, decision)
                .await?;
            Response::AnswerCard
        }
        Request::AnswerQuestion {
            conversation_id,
            card_id,
            answer,
        } => {
            sessions
                .answer_question(conversation_id, card_id, answer)
                .await?;
            Response::AnswerQuestion
        }
        Request::DecidePlan {
            conversation_id,
            card_id,
            approve,
            message,
        } => {
            sessions
                .decide_plan(conversation_id, card_id, approve, message)
                .await?;
            Response::DecidePlan
        }
        Request::ProposeOvernight {
            conversation_id,
            command_id,
            words,
            plan,
        } => Response::ProposeOvernight {
            run: Box::new(
                sessions
                    .propose_overnight(conversation_id, command_id, words, plan)
                    .await?,
            ),
        },
        Request::StartOvernight {
            conversation_id,
            run_id,
            command_id,
            revision,
        } => Response::StartOvernight {
            run: Box::new(
                sessions
                    .start_overnight(conversation_id, run_id, command_id, revision)
                    .await?,
            ),
        },
        Request::StopOvernight {
            conversation_id,
            run_id,
            command_id,
        } => Response::StopOvernight {
            run: Box::new(
                sessions
                    .stop_overnight(conversation_id, run_id, command_id)
                    .await?,
            ),
        },
        Request::SteerOvernight {
            conversation_id,
            run_id,
            command_id,
            words,
        } => Response::SteerOvernight {
            run: Box::new(
                sessions
                    .steer_overnight(conversation_id, run_id, command_id, words)
                    .await?,
            ),
        },
        Request::ContinueOvernight {
            conversation_id,
            run_id,
            command_id,
            words,
        } => Response::ContinueOvernight {
            run: Box::new(
                sessions
                    .continue_overnight(conversation_id, run_id, command_id, words)
                    .await?,
            ),
        },
        Request::MergeOvernight {
            conversation_id,
            run_id,
            command_id,
            verified_commit,
        } => Response::MergeOvernight {
            run: Box::new(
                sessions
                    .merge_overnight(conversation_id, run_id, command_id, verified_commit)
                    .await?,
            ),
        },
        Request::GetRunDiff {
            conversation_id,
            run_id,
        } => Response::GetRunDiff {
            diff: sessions.run_diff_stat(&conversation_id, &run_id).await?,
        },
        Request::PendingOvernightNotifications => Response::PendingOvernightNotifications {
            notifications: sessions
                .pending_overnight_notifications()
                .await
                .into_iter()
                .map(|(conversation_id, run_id, notification)| {
                    brigadier_ipc::protocol::PendingRunNotification {
                        conversation_id,
                        run_id,
                        notification,
                    }
                })
                .collect(),
        },
        Request::FailOvernightNotification {
            conversation_id,
            run_id,
            notification_id,
            error,
        } => {
            sessions
                .fail_overnight_notification(conversation_id, run_id, notification_id, error)
                .await?;
            Response::FailOvernightNotification
        }
        Request::AckOvernightNotification {
            conversation_id,
            run_id,
            notification_id,
        } => {
            sessions
                .ack_overnight_notification(conversation_id, run_id, notification_id)
                .await?;
            Response::AckOvernightNotification
        }
        Request::StopTask { task_id } => {
            sessions.stop_task(task_id).await?;
            Response::StopTask
        }
        Request::PauseTask { task_id } => {
            sessions.pause_task(task_id).await?;
            Response::PauseTask
        }
        Request::ResumeTask { task_id } => {
            sessions.resume_task(task_id).await?;
            Response::ResumeTask
        }
        Request::RestoreKeptWork { task_id } => Response::RestoreKeptWork {
            outcome: sessions.restore_kept_work(task_id).await?,
        },
        Request::ResolveWaiting {
            conversation_id,
            id,
        } => {
            sessions.resolve_waiting(conversation_id, id).await?;
            Response::ResolveWaiting
        }
        Request::Hibernate { id } => Response::Hibernate {
            conversation: Box::new(sessions.hibernate(id).await?),
        },
        Request::Archive { ids } => {
            let outcomes = sessions.archive_all(&ids).await;
            for (id, outcome) in ids.iter().zip(&outcomes) {
                if outcome.is_ok() {
                    daemon.terminals.close_conversation(&id.0);
                }
            }
            Response::Archive {
                outcomes: lifecycle_outcomes(ids, outcomes.into_iter().map(|o| o.map(Some))),
            }
        }
        Request::Restore { ids } => {
            let mut outcomes = Vec::with_capacity(ids.len());
            for id in &ids {
                outcomes.push(sessions.restore(id.clone()).await.map(Some));
            }
            Response::Restore {
                outcomes: lifecycle_outcomes(ids, outcomes),
            }
        }
        Request::PreviewRemoveProject { id } => Response::PreviewRemoveProject {
            removal: Box::new(sessions.preview_remove_project(id).await?),
        },
        Request::RemoveProject {
            id,
            delete_branches,
            keep_brain,
        } => {
            // Refused while one of its conversations works: before its terminals close.
            if let Some(why) = sessions.preview_remove_project(id.clone()).await?.blocked {
                return Err(invalid(why));
            }
            for conversation in core.catalog().conversations {
                if conversation.project_id.as_ref() == Some(&id) {
                    daemon.terminals.close_conversation(&conversation.id.0);
                }
            }
            Response::RemoveProject {
                report: sessions
                    .remove_project(id, delete_branches, keep_brain)
                    .await?,
            }
        }
        Request::Delete { ids } => {
            let outcomes = sessions.delete_all(&ids).await;
            for (id, outcome) in ids.iter().zip(&outcomes) {
                if outcome.is_ok() {
                    daemon.terminals.close_conversation(&id.0);
                }
            }
            Response::Delete {
                outcomes: lifecycle_outcomes(ids, outcomes.into_iter().map(|o| o.map(|()| None))),
            }
        }
        Request::PreviewDelete { ids } => Response::PreviewDelete {
            branches: sessions.preview_delete(&ids).await,
        },
        Request::DeletesFinished { ids } => Response::DeletesFinished {
            compactable_bytes: sessions.deletes_finished(&ids).await,
        },
        Request::RenameConversation { id, title } => Response::RenameConversation {
            conversation: Box::new(core.rename_conversation(id, title).await?),
        },
        Request::SetPinned { id, pinned } => Response::SetPinned {
            conversation: Box::new(core.set_pinned(id, pinned).await?),
        },
        Request::AppendMessage {
            conversation_id,
            text,
        } => Response::AppendMessage {
            message: Box::new(core.append_message(conversation_id, text).await?),
        },
        Request::ListMessages {
            conversation_id,
            before,
            limit,
        } => Response::ListMessages {
            page: core.list_messages(conversation_id, before, limit).await?,
        },
        Request::ReadBlobText { hash } => Response::ReadBlobText {
            text: core.read_blob_text(hash).await?,
        },
        Request::UpdateSettings { settings } => {
            let before = core.settings();
            let settings = core.update_settings(settings).await?;
            daemon.awake.apply().await;
            // New rules or rankings, or an agent or model turned back on, may let work waiting
            // for quota run now.
            if brigadier_core::routing::availability::wakes_waiting_work(&before, &settings) {
                let sessions = daemon.sessions.clone();
                daemon
                    .supervisor
                    .spawn(async move { sessions.retry_waiting_work().await });
            }
            Response::UpdateSettings {
                settings: Box::new(settings),
            }
        }
        Request::GetKeepAwake => Response::GetKeepAwake {
            status: daemon.awake.apply().await,
        },
        Request::SetUpLidClosed => Response::SetUpLidClosed {
            status: daemon.awake.set_up_lid_closed().await,
        },
        Request::EventsSince { after_seq, limit } => {
            let events = daemon
                .store
                .read_since(after_seq, limit)
                .await
                .map_err(internal)?;
            Response::EventsSince {
                events: events.iter().map(envelope).collect(),
                last_seq: daemon.store.last_seq(),
            }
        }
        Request::GetDiagnostics => Response::GetDiagnostics {
            diagnostics: Box::new(Diagnostics {
                daemon: daemon.info.clone(),
                metrics: daemon.metrics.sample().await,
                processes: daemon.metrics.processes().await,
                budgets: budgets(),
            }),
        },
        Request::ProbeBurst { count, interval_ms } => {
            let (burst, run) = core.probe_burst(count, interval_ms)?;
            daemon.supervisor.spawn(run);
            Response::ProbeBurst { burst }
        }
        Request::GetUpdates => Response::GetUpdates {
            updates: daemon.updates.view(),
        },
        Request::RunUpdate { target } => {
            crate::updates::start(daemon, target).map_err(brigadier_core::Error::Invalid)?;
            Response::RunUpdate
        }
        Request::GetProviders => Response::GetProviders {
            view: daemon.runtime.view().await,
        },
        Request::RefreshProviders { provider } => {
            daemon.runtime.refresh_providers(provider);
            Response::RefreshProviders
        }
        Request::GetUsage { project_id } => Response::GetUsage {
            usage: Box::new(daemon.sessions.usage_view(project_id).await?),
        },
        Request::PreviewRoutes { project_id, areas } => Response::PreviewRoutes {
            routes: daemon.sessions.preview_routes(project_id, areas).await,
        },
        Request::GetRankingsRefresh => Response::GetRankingsRefresh {
            refresh: daemon.runtime.registry().rankings_refresh(),
        },
        Request::ResetRankings => {
            daemon
                .runtime
                .registry()
                .reset_rankings()
                .map_err(|error| brigadier_core::Error::Invalid(error.to_string()))?;
            daemon.runtime.rankings_changed().await;
            Response::ResetRankings {
                refresh: daemon.runtime.registry().rankings_refresh(),
            }
        }
        Request::CheckRegistry => {
            crate::registry::check(daemon).await;
            Response::CheckRegistry {
                registry: daemon.runtime.registry().info(),
            }
        }
        #[cfg(debug_assertions)]
        Request::DebugInjectLimit {
            target,
            provider,
            window,
            reset_in_minutes,
            after_tool_calls,
        } => {
            use brigadier_core::manager::fault::FaultKey;
            use brigadier_ipc::protocol::FaultTarget;
            let key = match target {
                FaultTarget::Task { task_id } => FaultKey::Task(task_id),
                FaultTarget::Conversation { conversation_id } => {
                    FaultKey::Conversation(conversation_id)
                }
            };
            sessions
                .debug_inject_limit(key, provider, window, reset_in_minutes, after_tool_calls)
                .await?;
            Response::DebugInjectLimit
        }
        Request::StartRawSession {
            provider,
            cwd,
            model,
            effort,
            access,
            approvals,
            record,
        } => Response::StartRawSession {
            session: Box::new(
                daemon
                    .runtime
                    .start_session(StartRaw {
                        provider,
                        cwd,
                        model,
                        effort,
                        access,
                        approvals,
                        record,
                    })
                    .await?,
            ),
        },
        Request::ResumeRawSession { id } => Response::ResumeRawSession {
            session: Box::new(daemon.runtime.resume_session(id).await?),
        },
        Request::ForkRawSession { id } => Response::ForkRawSession {
            session: Box::new(daemon.runtime.fork_session(id).await?),
        },
        Request::SendRawSession { id, text, steer } => {
            daemon.runtime.send(&id, text, steer).await?;
            Response::SendRawSession
        }
        Request::InterruptRawSession { id } => {
            daemon.runtime.interrupt(&id).await?;
            Response::InterruptRawSession
        }
        Request::AnswerApproval {
            id,
            approval_id,
            decision,
        } => {
            daemon.runtime.answer(&id, approval_id, decision).await?;
            Response::AnswerApproval
        }
        Request::StopRawSession { id } => {
            daemon.runtime.stop_session(&id).await?;
            Response::StopRawSession
        }
        Request::CloseRawSession { id } => Response::CloseRawSession {
            session: Box::new(daemon.runtime.close_session(id).await?),
        },
        Request::ListRawEvents { id, before, limit } => Response::ListRawEvents {
            page: daemon.runtime.transcript(&id, before, limit).await?,
        },
        Request::ReplayFixture { fixture_id } => Response::ReplayFixture {
            session: Box::new(daemon.runtime.replay(&fixture_id).await?),
        },
        #[cfg(debug_assertions)]
        Request::SimulateUsageLimit { provider } => Response::SimulateUsageLimit {
            session: Box::new(daemon.runtime.simulate_usage_limit(provider).await?),
        },
        Request::GetDaemonActivity => Response::GetDaemonActivity {
            activity: DaemonActivity {
                // The asking connection is one of them.
                clients: u32::try_from(daemon.metrics.clients().saturating_sub(1))
                    .unwrap_or(u32::MAX),
                running: idle::running(daemon).await,
                overnight: daemon.sessions.overnight_active(),
            },
        },
        Request::ScanStorage => Response::ScanStorage {
            report: Box::new(daemon.storage.scan(daemon).await.map_err(invalid)?),
        },
        Request::CleanStorage { scan_id, items } => Response::CleanStorage {
            report: daemon
                .storage
                .clean(daemon, &scan_id, items)
                .await
                .map_err(invalid)?,
        },
        Request::PreviewUninstall { app } => Response::PreviewUninstall {
            plan: Box::new(
                daemon
                    .uninstall
                    .preview(daemon, app)
                    .await
                    .map_err(invalid)?,
            ),
        },
        Request::Uninstall {
            plan_id,
            keep_data,
            delete_branches,
        } => Response::Uninstall {
            report: daemon
                .uninstall
                .run(daemon, &plan_id, keep_data, delete_branches)
                .await
                .map_err(invalid)?,
        },
        Request::Subscribe { .. }
        | Request::SetMetricsStreaming { .. }
        | Request::RefreshRankings
        | Request::Shutdown => {
            return Err(IpcError {
                code: ErrorCode::Invalid,
                message: "handled by the session".into(),
            });
        }
    })
}

/// A path as one word for the shell a setup terminal runs in (PowerShell on Windows, where
/// a quoted program needs the call operator).
fn shell_quote(path: &str) -> String {
    if cfg!(windows) {
        format!("& '{}'", path.replace('\'', "''"))
    } else {
        format!("'{}'", path.replace('\'', "'\"'\"'"))
    }
}
