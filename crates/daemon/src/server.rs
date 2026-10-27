//! Accepts IPC connections and serves requests and the live event feed.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use brigadier_core::heap;
use brigadier_core::manager::search::SearchRun;
use brigadier_core::manager::workspace::WorkspaceWatch;
use brigadier_core::manager::{ComputerNews, SessionManager};
use brigadier_core::pages::{PageChange, PageError, Pages};
use brigadier_core::runtime::{Runtime, StartRaw};
use brigadier_core::storage::{CleanProgress, FreeUpProjectsReport};
use brigadier_core::{
    AppCall, Conversation, ConversationId, Core, Lifecycle, MAX_ATTACHMENT_BYTES, ProjectId,
    SearchUpdate, Setup, WorkspaceTarget, streams,
};
use brigadier_ipc::frame::ClientRead;
use brigadier_ipc::metrics::{DaemonMetrics, Diagnostics, budgets};
use brigadier_ipc::protocol::{
    ArtifactText, ClientFrame, ClientInfo, DaemonActivity, DaemonInfo, DictationStatus,
    DictationUpdate, ErrorCode, EventEnvelope, IpcError, LifecycleOutcome, Outcome, RawJson,
    Request, Response, SendOutcome, ServerFrame, TerminalInfo, TerminalOutput,
};
use brigadier_ipc::{Accepted, Connection, Listener, Reader, Token};
use brigadier_providers::ProviderKind;
use brigadier_providers::login_mark::LoginMark;
use brigadier_store::{Store, StoredEvent};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, mpsc, watch};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::awake::Awake;
use crate::dictation::{Dictation, Started};
use crate::idle;
use crate::metrics::Metrics;
use crate::news::{NewsWriter, news_due};
use crate::site_icons::SiteIcons;
use crate::storage::Storage;
use crate::supervisor::Supervisor;
use crate::terminals::Terminals;
use crate::uninstall::Uninstall;
use crate::updates::Updates;
use crate::upgrade;

/// The owner of Home's terminals (they belong to no conversation).
const HOME_TERMINALS: &str = "home";
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
    pub terminals: Arc<Terminals>,
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
    /// Connect's install and sign-in of each agent.
    pub connects: crate::connect::Connects,
    /// The user's pages: local Markdown documents in the data folder.
    pub pages: Arc<Pages>,
    /// Web sites' icons for the links in answers.
    pub site_icons: Arc<SiteIcons>,
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
        pages: Arc<Pages>,
        site_icons: Arc<SiteIcons>,
    ) -> Self {
        let agent_folders = ProviderKind::ALL
            .into_iter()
            .filter_map(|provider| runtime.cli_env().installer_folder(provider))
            .collect();
        let terminals = Arc::new(
            Terminals::with_platform(runtime.platform().clone()).with_agent_folders(agent_folders),
        );
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
            terminals,
            dictation,
            awake,
            storage: Storage::default(),
            uninstall: Uninstall::default(),
            updates: Updates::default(),
            connects: crate::connect::Connects::default(),
            pages,
            site_icons,
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
                Ok(Accepted::Hook {
                    grant,
                    output,
                    stream,
                }) => upgrade::serve_hook(daemon_for_task, grant, output, stream).await,
                Ok(Accepted::Cli {
                    session,
                    grant,
                    connection,
                }) => upgrade::serve_cli(daemon_for_task, session, grant, connection).await,
                Err(err) => tracing::warn!(error = %err, "rejected IPC connection"),
            }
        });
        daemon.connections.spawn(task);
    }
}

async fn serve(daemon: Arc<Daemon>, connection: Connection, client: ClientInfo) {
    let id = daemon.next_connection.fetch_add(1, Ordering::Relaxed);
    tracing::info!(connection = id, client = %client.name, pid = client.pid, "client connected");
    daemon.metrics.connection_opened(id, client.clone());
    let (late_tx, late) = mpsc::channel(LATE_ANSWERS);
    let (reads, mut read_queue) = mpsc::unbounded_channel::<(u32, Request)>();
    let reader = daemon.supervisor.spawn({
        let (daemon, late) = (daemon.clone(), late_tx.clone());
        async move {
            while let Some((id, request)) = read_queue.recv().await {
                let outcome = handle_request(&daemon, request).await;
                if late.send((id, outcome)).await.is_err() {
                    return;
                }
            }
        }
    });
    let mut session = Session {
        daemon: daemon.clone(),
        client,
        writer: NewsWriter::new(connection.writer),
        feed: None,
        last_sent: 0,
        metrics: None,
        terminal_feed: None,
        terminals: HashSet::new(),
        dictation_feed: None,
        dictations: Started::new(daemon.dictation.clone()),
        page_feed: None,
        thread_feed: None,
        storage_feed: None,
        computer_feed: None,
        app_feed: None,
        checkout_saves: None,
        workspace_watch: None,
        search: None,
        searches: 0,
        late_tx,
        late,
        reads,
    };
    if let Err(err) = session.run(connection.reader).await {
        tracing::debug!(connection = id, error = %err, "connection ended with an error");
    }
    reader.abort();
    if session.metrics.is_some() {
        daemon.metrics.set_streaming(false);
    }
    // App calls waiting on this window fail now, unless another window runs them.
    if session.app_feed.take().is_some() {
        daemon.sessions.app_window_gone();
    }
    daemon.metrics.connection_closed(id);
    tracing::info!(connection = id, "client disconnected");
}

struct Session {
    daemon: Arc<Daemon>,
    /// Who connected: kept with what this connection answers for the user.
    client: ClientInfo,
    /// Its frames, the live threads' news held for a frame ([`NewsWriter`]).
    writer: NewsWriter<interprocess::local_socket::tokio::SendHalf>,
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
    /// The dictations this connection started: only their text is forwarded, and the ones
    /// still taking audio are dropped when it ends.
    dictations: Started,
    /// Page changes, once subscribed to the live feed.
    page_feed: Option<broadcast::Receiver<PageChange>>,
    thread_feed: Option<broadcast::Receiver<brigadier_core::thread_view::ThreadNews>>,
    /// How a clean under way goes (Settings › Storage), once subscribed to the live feed.
    storage_feed: Option<broadcast::Receiver<CleanProgress>>,
    /// Computer use's news, once subscribed to the live feed.
    computer_feed: Option<broadcast::Receiver<ComputerNews>>,
    /// Agents' app calls (a Browser tab's among them), once subscribed to the live feed: the
    /// app runs them.
    app_feed: Option<broadcast::Receiver<AppCall>>,
    /// The checkouts a tab chat saved a file in, once subscribed to the live feed.
    checkout_saves: Option<broadcast::Receiver<WorkspaceTarget>>,
    /// The checkout this connection's right sidebar shows, watched while it shows.
    workspace_watch: Option<(WorkspaceTarget, WorkspaceWatch)>,
    /// The right sidebar's search running for this connection, by its id.
    search: Option<(u32, SearchRun)>,
    /// The last search id given out.
    searches: u32,
    /// Answers to slow requests (clones), which run beside the others, by request id.
    late_tx: mpsc::Sender<(u32, Result<Response, IpcError>)>,
    late: mpsc::Receiver<(u32, Result<Response, IpcError>)>,
    /// Reads of checkouts and diffs ([`read_beside`]), answered in turn on a task of their own.
    reads: mpsc::UnboundedSender<(u32, Request)>,
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
                match reader.read_client().await {
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
                    Some(Ok(ClientRead::Frame(ClientFrame::Request { id, request }))) => {
                        match self.handle(id, request).await? {
                            Flow::Continue => {}
                            Flow::Close => break Ok(()),
                        }
                    }
                    Some(Ok(ClientRead::Rejected(response))) => {
                        self.writer.write(&response).await?;
                    }
                    Some(Ok(ClientRead::Frame(_))) => {
                        break Err(anyhow::anyhow!("handshake frame after the handshake"));
                    }
                    Some(Err(err)) => break Err(err.into()),
                    None => break Ok(()),
                },
                event = next_event(&mut self.feed) => match event {
                    Ok(event) => {
                        if event.seq > self.last_sent {
                            self.last_sent = event.seq;
                            if streams::on_live_feed(&event.stream) {
                                self.writer.write(&ServerFrame::Event { event: envelope(&event) }).await?;
                            }
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
                Some((id, outcome)) = self.late.recv() => {
                    self.respond(id, outcome).await?;
                }
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
                progress = next_clean_progress(&mut self.storage_feed) => {
                    if let Some(progress) = progress {
                        self.writer.write(&ServerFrame::StorageClean { progress }).await?;
                    }
                }
                news = next_thread_news(&mut self.thread_feed) => {
                    if let Some(news) = news {
                        self.writer.news(news).await?;
                    }
                }
                () = news_due(self.writer.due()) => {
                    self.writer.send_news().await?;
                }
                change = next_page_change(&mut self.page_feed) => {
                    if let Some(PageChange { id, revision }) = change {
                        self.writer.write(&ServerFrame::PageChanged { id, revision }).await?;
                    }
                }
                news = next_computer_news(&mut self.computer_feed) => {
                    match news {
                        Some(ComputerNews::Status(status)) => {
                            self.writer.write(&ServerFrame::ComputerUse { status }).await?;
                        }
                        Some(ComputerNews::SetupNeeded) => {
                            self.writer.write(&ServerFrame::ComputerUseSetupNeeded).await?;
                        }
                        None => {}
                    }
                }
                call = next_app_call(&mut self.app_feed) => {
                    if let Some(call) = call {
                        self.writer.write(&ServerFrame::AppCall { call }).await?;
                    }
                }
                target = next_checkout_save(&mut self.checkout_saves) => {
                    if let Some(target) = target {
                        self.writer.write(&ServerFrame::WorkspaceChanged { target }).await?;
                    }
                }
                target = next_workspace_change(&mut self.workspace_watch) => {
                    if let Some(target) = target {
                        self.writer.write(&ServerFrame::WorkspaceChanged { target }).await?;
                    }
                }
                found = next_search_update(&mut self.search) => {
                    if let Some((id, update)) = found {
                        self.writer.write(&ServerFrame::Search { id, update }).await?;
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
                if self.page_feed.is_none() {
                    self.page_feed = Some(self.daemon.pages.subscribe());
                }
                if self.thread_feed.is_none() {
                    self.thread_feed = Some(self.daemon.sessions.subscribe_thread_news());
                }
                if self.storage_feed.is_none() {
                    self.storage_feed = Some(self.daemon.storage.subscribe_progress());
                }
                if self.checkout_saves.is_none() {
                    self.checkout_saves = Some(self.daemon.sessions.subscribe_checkout_saves());
                }
                if self.app_feed.is_none() {
                    self.app_feed = Some(self.daemon.sessions.subscribe_app());
                }
                if self.computer_feed.is_none() {
                    self.computer_feed = Some(self.daemon.sessions.subscribe_computer_use());
                    // A window that subscribes late (the app reopened) hears where things
                    // stand now, not only what changes next.
                    if let Some(status) = self.daemon.sessions.last_computer_status() {
                        self.writer
                            .write(&ServerFrame::ComputerUse { status })
                            .await?;
                    }
                }
                self.subscribe(after_seq).await.map_err(internal)
            }
            Request::SetMetricsStreaming { enabled } => {
                self.set_metrics(enabled);
                Ok(Response::SetMetricsStreaming { enabled })
            }
            Request::OpenTerminal {
                conversation_id,
                project_id,
                session_id,
                cwd,
                fresh,
                cols,
                rows,
            } => self.open_terminal(
                conversation_id.as_ref(),
                project_id.as_ref(),
                session_id.as_deref(),
                cwd,
                fresh.unwrap_or(false),
                (cols, rows),
            ),
            Request::AddAccount {
                provider,
                cols,
                rows,
            } => match self.daemon.sessions.add_account(provider).await {
                Ok(account) => match self.open_account_terminal(&account.id, cols, rows).await? {
                    Ok(terminal) => Ok(Response::AddAccount { account, terminal }),
                    Err(err) => Err(err),
                },
                Err(err) => Err(IpcError::from(err)),
            },
            Request::SignInAccount { id, cols, rows } => self
                .open_account_terminal(&id, cols, rows)
                .await?
                .map(|terminal| Response::SignInAccount { terminal }),
            Request::WatchWorkspace { target } => {
                // The old watch stops first, and a failed new one leaves none.
                self.workspace_watch = None;
                match target {
                    None => Ok(Response::WatchWorkspace { watching: false }),
                    Some(target) => match self.daemon.sessions.watch_workspace(&target).await {
                        Ok(watch) => {
                            self.workspace_watch = Some((target, watch));
                            Ok(Response::WatchWorkspace { watching: true })
                        }
                        Err(err) => Err(IpcError::from(err)),
                    },
                }
            }
            Request::StartSearch { target, query } => {
                // The old search stops first, and a failed new one leaves none.
                self.search = None;
                match self.daemon.sessions.start_search(&target, query).await {
                    Ok(run) => {
                        self.searches = self.searches.wrapping_add(1);
                        self.search = Some((self.searches, run));
                        Ok(Response::StartSearch { id: self.searches })
                    }
                    Err(err) => Err(IpcError::from(err)),
                }
            }
            Request::CancelSearch { id } => {
                if self
                    .search
                    .as_ref()
                    .is_some_and(|(running, _)| *running == id)
                {
                    self.search = None;
                }
                Ok(Response::CancelSearch)
            }
            // A window that reconnects mid-download asks this first: it hears the rest.
            Request::GetDictation => Ok(Response::GetDictation {
                dictation: follow_status(&self.daemon.dictation, &mut self.dictation_feed),
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
                let settings = self.daemon.core.settings();
                self.daemon
                    .dictation
                    .start(&settings.dictation_language, &settings.dictation_words)
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
            // A checkout's or a diff's read: answered in turn with the others of its kind, beside
            // everything else.
            request if read_beside(&request) => {
                if self.reads.send((id, request)).is_err() {
                    anyhow::bail!("the connection's reads stopped");
                }
                return Ok(Flow::Continue);
            }
            // Long: answered beside the connection's other requests.
            request if answered_beside(&request) => {
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
            Request::Shutdown => {
                // Stop admission and drain first; acknowledge only once writes are committed.
                let _ = self.daemon.quit.try_send("quit requested by a client");
                let mut drained = self.daemon.drained.clone();
                let _ = drained.wait_for(|drained| *drained).await;
                self.respond(id, Ok(Response::Shutdown)).await?;
                return Ok(Flow::Close);
            }
            Request::AnswerQuestion {
                conversation_id,
                card_id,
                answers,
                input,
            } => {
                let origin = brigadier_core::work::AnswerOrigin {
                    input,
                    client: self.client.name.clone(),
                    pid: self.client.pid,
                };
                let sessions = self.daemon.sessions.clone();
                heap(|| sessions.answer_question(conversation_id, card_id, answers, Some(origin)))
                    .await
                    .map(|()| Response::AnswerQuestion)
                    .map_err(IpcError::from)
            }
            Request::MergeSession { conversation_id } => {
                let origin = brigadier_core::work::AnswerOrigin {
                    input: Some(brigadier_core::work::AnswerInput::Click),
                    client: self.client.name.clone(),
                    pid: self.client.pid,
                };
                let sessions = self.daemon.sessions.clone();
                heap(|| sessions.merge_session(&conversation_id, origin))
                    .await
                    .map(|text| Response::MergeSession { text })
                    .map_err(IpcError::from)
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
            .write_response(id, Outcome::from(outcome))
            .await?;
        Ok(())
    }

    /// Opens (or re-attaches to) a conversation's terminal, or Home's; its output then comes
    /// to this connection.
    fn open_terminal(
        &mut self,
        conversation_id: Option<&ConversationId>,
        project_id: Option<&ProjectId>,
        session_id: Option<&str>,
        restored_cwd: Option<String>,
        fresh: bool,
        size: (u16, u16),
    ) -> Result<Response, IpcError> {
        let (cols, rows) = size;
        let (owner, cwd) = match conversation_id {
            Some(id) => (id.0.clone(), self.conversation_terminal_dir(id)?),
            None => (
                HOME_TERMINALS.to_owned(),
                self.project_terminal_dir(project_id)?,
            ),
        };
        // A remembered shell folder may have been removed since the tab was last open.
        let cwd = restored_cwd
            .filter(|path| {
                let path = std::path::Path::new(path);
                path.is_absolute() && path.is_dir()
            })
            .unwrap_or(cwd);
        // Subscribed first, so no output between the scrollback and the feed is lost.
        if self.terminal_feed.is_none() {
            self.terminal_feed = Some(self.daemon.terminals.subscribe());
        }
        if fresh
            && let Some(session) = session_id
            && let Some(id) = self.daemon.terminals.running(&owner, session)
        {
            self.daemon.terminals.close(&id);
        }
        let terminal = self
            .daemon
            .terminals
            .open_session(&owner, session_id, cwd, cols, rows)?;
        self.terminals.insert(terminal.id.clone());
        Ok(Response::OpenTerminal { terminal })
    }

    /// Where a conversation's terminal starts: a session's checkout, else its project's first
    /// repository, else the home folder. An archived or deleted one has none.
    fn conversation_terminal_dir(&self, id: &ConversationId) -> Result<String, IpcError> {
        let conversation = self.daemon.core.conversation(id)?;
        if conversation.lifecycle == Lifecycle::Archived || conversation.deleting {
            return Err(brigadier_core::Error::Invalid(
                "an archived or deleted thread has no terminal".into(),
            )
            .into());
        }
        if matches!(conversation.setup, Some(Setup::Session { .. })) {
            return Ok(self.daemon.sessions.checkout_dir(id)?);
        }
        self.project_terminal_dir(conversation.project_id.as_ref())
    }

    /// The project's first repository, else the home folder.
    fn project_terminal_dir(&self, project: Option<&ProjectId>) -> Result<String, IpcError> {
        let repo = project
            .and_then(|id| self.daemon.core.project(id).ok())
            .and_then(|project| project.repos.into_iter().next());
        if let Some(repo) = repo {
            return Ok(repo.path);
        }
        let env = self.daemon.runtime.cli_env();
        Ok(env
            .home()
            .ok_or_else(|| IpcError::from(brigadier_core::Error::Invalid("no home folder".into())))?
            .display()
            .to_string())
    }

    /// Opens (or shows again) the terminal that signs extra account `id` in: its CLI's own
    /// sign-in, run directly with the account's home and the user's environment less any
    /// login variable. The account is checked again the moment its login is stored, and when
    /// the sign-in ends.
    async fn open_account_terminal(
        &mut self,
        id: &str,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<Result<TerminalInfo, IpcError>> {
        let invalid = |text: String| IpcError::from(brigadier_core::Error::Invalid(text));
        let Some(entry) = self
            .daemon
            .core
            .settings()
            .accounts
            .into_iter()
            .find(|entry| entry.id == id)
        else {
            return Ok(Err(invalid("that account was removed".into())));
        };
        let runtime = self.daemon.runtime.clone();
        let home = runtime.account_home(id);
        let env = runtime.cli_env();
        let Some(program) = env.resolve(entry.provider) else {
            return Ok(Err(invalid(format!(
                "{} is not installed",
                entry.provider.label()
            ))));
        };
        if self.terminal_feed.is_none() {
            self.terminal_feed = Some(self.daemon.terminals.subscribe());
        }
        let key = format!("account:{id}");
        let terminal_id = match self.daemon.terminals.running(&key, &key) {
            Some(open) => open,
            None => {
                let account_env = env.for_account(entry.provider, &home);
                // Where its login is kept, as it stands before the sign-in starts.
                let mark = LoginMark::of(entry.provider, &account_env);
                let before = match &mark {
                    Some(mark) => mark.read().await,
                    None => None,
                };
                let command = brigadier_providers::TerminalCommand {
                    program,
                    args: brigadier_core::runtime::accounts::sign_in_args(entry.provider)
                        .iter()
                        .map(|arg| (*arg).to_owned())
                        .collect(),
                    cwd: home.clone(),
                    env: account_env.vars(),
                };
                let started_at_ms = brigadier_core::now_ms();
                let hosted = match self
                    .daemon
                    .terminals
                    .start_command(&key, &key, command, cols, rows)
                {
                    Ok(hosted) => hosted,
                    Err(err) => return Ok(Err(IpcError::from(err))),
                };
                self.daemon.terminals.watch_account_sign_in(
                    &hosted.id,
                    mark.clone(),
                    before.clone(),
                    started_at_ms,
                );
                let (exited, account) = (hosted.exited, id.to_owned());
                self.daemon
                    .supervisor
                    .spawn(crate::connect::follow_account_sign_in(
                        runtime,
                        entry.provider,
                        account,
                        mark,
                        before,
                        async move {
                            let _ = exited.await;
                        },
                    ));
                hosted.id
            }
        };
        let Some(feed) = self.terminal_feed.as_mut() else {
            return Ok(Err(invalid("the terminal feed is closed".into())));
        };
        let (terminal, others) = match self.daemon.terminals.attach(&terminal_id, feed, cols, rows)
        {
            Ok(attached) => attached,
            Err(err) => return Ok(Err(IpcError::from(err))),
        };
        for output in others {
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
        self.terminals.insert(terminal.id.clone());
        Ok(Ok(terminal))
    }

    /// Forwards dictation updates to this connection from now on.
    fn follow_dictation(&mut self) {
        follow(&self.daemon.dictation, &mut self.dictation_feed);
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
                if !streams::on_live_feed(&event.stream) {
                    continue;
                }
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

/// The running search's next update, by its id; pending forever while none runs. It is
/// dropped once it ended.
async fn next_search_update(search: &mut Option<(u32, SearchRun)>) -> Option<(u32, SearchUpdate)> {
    match search {
        Some((id, run)) => {
            let id = *id;
            let update = run.next().await;
            if update
                .as_ref()
                .is_none_or(|update| matches!(update, SearchUpdate::Done { .. }))
            {
                *search = None;
            }
            update.map(|update| (id, update))
        }
        None => std::future::pending().await,
    }
}

/// The watched checkout's next change; pending forever while none is watched.
async fn next_workspace_change(
    watch: &mut Option<(WorkspaceTarget, WorkspaceWatch)>,
) -> Option<WorkspaceTarget> {
    match watch {
        Some((target, changes)) => match changes.changed().await {
            Some(()) => Some(target.clone()),
            None => {
                *watch = None;
                None
            }
        },
        None => std::future::pending().await,
    }
}

/// The next checkout a tab chat saved a file in; pending forever until subscribed.
async fn next_checkout_save(
    feed: &mut Option<broadcast::Receiver<WorkspaceTarget>>,
) -> Option<WorkspaceTarget> {
    match feed {
        Some(rx) => match rx.recv().await {
            Ok(target) => Some(target),
            // A file tab still reads again when it comes to the front or the window gains focus.
            Err(RecvError::Lagged(_)) => None,
            Err(RecvError::Closed) => {
                *feed = None;
                None
            }
        },
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

fn follow(dictation: &Dictation, feed: &mut Option<broadcast::Receiver<DictationUpdate>>) {
    if feed.is_none() {
        *feed = Some(dictation.subscribe());
    }
}

/// The speech model's status, with its updates forwarded from then on. Subscribed first, so
/// a download that ends in between is still heard.
fn follow_status(
    dictation: &Dictation,
    feed: &mut Option<broadcast::Receiver<DictationUpdate>>,
) -> DictationStatus {
    follow(dictation, feed);
    dictation.status()
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

async fn next_app_call(feed: &mut Option<broadcast::Receiver<AppCall>>) -> Option<AppCall> {
    match feed {
        Some(rx) => match rx.recv().await {
            Ok(call) => Some(call),
            // Calls skipped fail on their own: nobody answers them in time.
            Err(RecvError::Lagged(_)) => None,
            Err(RecvError::Closed) => {
                *feed = None;
                None
            }
        },
        None => std::future::pending().await,
    }
}

async fn next_computer_news(
    feed: &mut Option<broadcast::Receiver<ComputerNews>>,
) -> Option<ComputerNews> {
    match feed {
        Some(rx) => match rx.recv().await {
            Ok(news) => Some(news),
            // Only the newest status matters, and the next one follows.
            Err(RecvError::Lagged(_)) => None,
            Err(RecvError::Closed) => {
                *feed = None;
                None
            }
        },
        None => std::future::pending().await,
    }
}

async fn next_clean_progress(
    feed: &mut Option<broadcast::Receiver<CleanProgress>>,
) -> Option<CleanProgress> {
    match feed {
        Some(rx) => match rx.recv().await {
            Ok(progress) => Some(progress),
            // Behind: the next one says where it stands.
            Err(RecvError::Lagged(_)) => None,
            Err(RecvError::Closed) => {
                *feed = None;
                None
            }
        },
        None => std::future::pending().await,
    }
}

async fn next_thread_news(
    feed: &mut Option<broadcast::Receiver<brigadier_core::thread_view::ThreadNews>>,
) -> Option<brigadier_core::thread_view::ThreadNews> {
    match feed {
        Some(rx) => loop {
            match rx.recv().await {
                Ok(news) => break Some(news),
                // Behind: what it missed is read again with the next `threadView`.
                Err(RecvError::Lagged(missed)) => {
                    tracing::warn!(missed, "a connection missed thread news");
                }
                Err(RecvError::Closed) => {
                    *feed = None;
                    break None;
                }
            }
        },
        None => std::future::pending().await,
    }
}

async fn next_page_change(
    feed: &mut Option<broadcast::Receiver<PageChange>>,
) -> Option<PageChange> {
    match feed {
        Some(rx) => match rx.recv().await {
            Ok(change) => Some(change),
            // Hundreds behind: some are missed. An editor still finds out at its next save,
            // which the newer revision refuses as a conflict.
            Err(RecvError::Lagged(missed)) => {
                tracing::warn!(missed, "a connection missed page changes");
                None
            }
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

/// A new session started on a message or on the thread's offer, the notice of a message sent on
/// to another session, and "Run here" on one offered as a new session (in its own boxed future: the dispatch stays
/// under its size).
async fn open_session(sessions: &SessionManager, request: Request) -> Result<Response, IpcError> {
    Ok(match request {
        Request::StartSession {
            from,
            setup,
            text,
            attachments,
            mentions,
        } => {
            let (conversation, outcome) = sessions
                .start_session(
                    &from,
                    setup.map(|setup| *setup),
                    text,
                    attachments,
                    mentions,
                )
                .await?;
            Response::StartSession {
                conversation: Box::new(conversation),
                outcome: send_outcome(outcome),
            }
        }
        Request::StartOfferedSession {
            conversation_id,
            offer_id,
        } => {
            let (conversation, outcome) = sessions
                .start_offered_session(&conversation_id, &offer_id)
                .await?;
            Response::StartOfferedSession {
                conversation: Box::new(conversation),
                outcome: send_outcome(outcome),
            }
        }
        Request::StartOfferedFusion {
            conversation_id,
            offer_id,
        } => Response::StartOfferedFusion {
            outcome: send_outcome(
                sessions
                    .start_offered_fusion(&conversation_id, &offer_id)
                    .await?,
            ),
        },
        Request::GetSettledTabs => {
            let (tabs, landed) = sessions.tab_marks().await;
            Response::GetSettledTabs { tabs, landed }
        }
        Request::GetMergeUndo {
            conversation_id,
            merge_commit,
        } => Response::GetMergeUndo {
            undo: sessions.merge_undo(&conversation_id, &merge_commit).await?,
            pushed: sessions
                .merge_pushed(&conversation_id, &merge_commit)
                .await?,
        },
        _ => unreachable!("only tab requests come here"),
    })
}

/// A sent message's outcome, as the app reads it.
fn send_outcome(outcome: brigadier_core::manager::SendOutcome) -> SendOutcome {
    match outcome {
        brigadier_core::manager::SendOutcome::Sent(message) => SendOutcome::Sent {
            message: Box::new(message),
        },
        brigadier_core::manager::SendOutcome::Queued(item) => SendOutcome::Queued { item },
    }
}

fn internal(err: brigadier_store::Error) -> IpcError {
    IpcError::from(brigadier_core::Error::from(err))
}

/// Every await here goes through [`heap`]: one frame holds all of them in an unoptimized build,
/// and before it the dispatch alone took 1.26 MB of a runtime thread's 2 MiB stack, so a
/// message to a session overflowed it.
/// Whether `request` may take long, so it is answered beside the connection's other requests
/// rather than before them. A site's icon waits on the site itself, which may take its whole
/// timeout; the tabs' settled states read git in every tab; the requests that read the user's
/// folders (the first run's project scan, Add project's) run git there and may wait on a
/// macOS privacy prompt nobody has answered yet. A conversation's view waits for nothing slow
/// before it: the app shows a conversation once its view is in (#484).
fn answered_beside(request: &Request) -> bool {
    matches!(
        request,
        Request::SiteIcon { .. }
            | Request::GetConversation { .. }
            | Request::FindProjects
            | Request::FindRepos
            | Request::GetRepoInfo { .. }
            | Request::ListCliCommands { .. }
            | Request::BrowseFolders { .. }
            | Request::CheckFolder { .. }
            | Request::AddProject { .. }
            | Request::ScanStorage { .. }
            | Request::PreviewDelete { .. }
            | Request::CleanStorage { .. }
            | Request::PreviewRemoveProject { .. }
            | Request::RemoveProject { .. }
            | Request::FreeUpProjects { .. }
            | Request::PreviewUninstall { .. }
            | Request::Uninstall { .. }
            | Request::GetSettledTabs
            | Request::GetMergeUndo { .. }
            | Request::RestartPreview { .. }
            | Request::WriteUpFeedback { .. }
            | Request::FileFeedback { .. }
            | Request::FeedbackAccount
            | Request::StartGitHubSignIn
            | Request::WaitGitHubSignIn
    )
}

/// Reads of a checkout or a diff: git and the disk, a few hundred milliseconds each on a big
/// repository, and the side panels and the Changes card ask for several on every change while
/// a session works. In the connection's turn they held up a message sent meanwhile, a Stop and
/// every event for seconds (#491). They are answered in turn with each other, so an older
/// read's answer never comes after a newer one's, beside the connection's other requests;
/// their git takes no locks, so a write beside them still goes.
fn read_beside(request: &Request) -> bool {
    matches!(
        request,
        Request::GetSourceState { .. }
            | Request::GetProjectSourceState { .. }
            | Request::GetSessionDiff { .. }
            | Request::GetWorkerDiffs { .. }
            | Request::GetWorkerSource { .. }
            | Request::PrepareTurnRevert { .. }
            | Request::RevertTurn { .. }
            | Request::GetPendingChanges { .. }
            | Request::GetReviewDiff { .. }
            | Request::GetWorkspaceTree { .. }
            | Request::ListWorkspaceFolder { .. }
            | Request::ListFiles { .. }
            | Request::ListProjectFiles { .. }
            | Request::ListSourceBranches { .. }
            | Request::GetThreadMetrics { .. }
            | Request::ThreadView { .. }
            | Request::WorkerThread { .. }
    )
}

async fn handle_request(daemon: &Arc<Daemon>, request: Request) -> Result<Response, IpcError> {
    let core = &daemon.core;
    let sessions = &daemon.sessions;
    Ok(match request {
        Request::GetCatalog => Response::GetCatalog {
            catalog: Box::new(core.visible_catalog()),
        },
        Request::GetActivity => Response::GetActivity {
            activity: heap(|| sessions.activity()).await,
        },
        Request::CreateProject { name, repo } => {
            let project = heap(|| core.create_project(name, repo)).await?;
            heap(|| sessions.project_changed(&project)).await;
            Response::CreateProject {
                project: Box::new(project),
            }
        }
        Request::UpdateProject { id, patch } => {
            let project = heap(|| core.update_project(id, patch)).await?;
            heap(|| sessions.project_changed(&project)).await;
            Response::UpdateProject {
                project: Box::new(project),
            }
        }
        Request::SetFolderTrust { id, path, trusted } => Response::SetFolderTrust {
            report: Box::new(heap(|| sessions.set_folder_trust(id, path, trusted)).await?),
        },
        Request::CreateConversation {
            kind,
            project_id,
            title,
            setup,
        } => Response::CreateConversation {
            conversation: Box::new(
                heap(|| sessions.create_conversation(kind, project_id, title, setup)).await?,
            ),
        },
        request @ (Request::StartSession { .. }
        | Request::StartOfferedFusion { .. }
        | Request::StartOfferedSession { .. }
        | Request::GetSettledTabs
        | Request::GetMergeUndo { .. }) => heap(|| open_session(sessions, request)).await?,
        Request::ForkConversation {
            conversation_id,
            message_id,
            place,
        } => Response::ForkConversation {
            conversation: Box::new(
                heap(|| sessions.fork(conversation_id, message_id, place)).await?,
            ),
        },
        Request::UpdateSetup { id, setup } => Response::UpdateSetup {
            conversation: Box::new(heap(|| sessions.set_setup(id, setup)).await?),
        },
        Request::ContinueOn {
            id,
            provider,
            model,
        } => Response::ContinueOn {
            when: heap(|| sessions.continue_on(&id, provider, model, true))
                .await
                .map_err(brigadier_core::Error::Invalid)?
                .0,
        },
        Request::GetConversation { id, limit } => Response::GetConversation {
            view: Box::new(heap(|| sessions.conversation_view(id, limit)).await?),
        },
        Request::SendMessage {
            conversation_id,
            text,
            attachments,
            mentions,
            steer,
            fusion,
            queue_index,
        } => {
            let text = if fusion {
                sessions.fusion_message(&conversation_id, text)?
            } else {
                text
            };
            Response::SendMessage {
                outcome: send_outcome(
                    heap(|| {
                        sessions.send_message(
                            conversation_id,
                            text,
                            attachments,
                            mentions,
                            steer,
                            queue_index,
                        )
                    })
                    .await?,
                ),
            }
        }
        Request::EditQueued {
            conversation_id,
            item_id,
            text,
            attachments,
            mentions,
        } => Response::EditQueued {
            // Through the sessions: a merge being prepared must see the edit.
            queue: heap(|| {
                sessions.edit_queued(&conversation_id, &item_id, text, attachments, mentions)
            })
            .await?,
        },
        Request::DeleteQueued {
            conversation_id,
            item_id,
        } => {
            // Through the sessions: one the session already has is withdrawn (run2-D46).
            heap(|| sessions.delete_queued(&conversation_id, &item_id)).await?;
            Response::DeleteQueued {
                queue: heap(|| core.conversation_view(conversation_id, 1))
                    .await?
                    .queue,
            }
        }
        Request::MoveQueued {
            conversation_id,
            item_id,
            index,
        } => Response::MoveQueued {
            queue: heap(|| core.move_queued(&conversation_id, &item_id, index)).await?,
        },
        Request::ResumeQueue { conversation_id } => Response::ResumeQueue {
            queue: heap(|| sessions.resume_queue(conversation_id)).await?,
        },
        Request::RunQueuedHere {
            conversation_id,
            item_id,
        } => Response::RunQueuedHere {
            queue: heap(|| sessions.run_queued_here(&conversation_id, &item_id)).await?,
        },
        Request::DismissSent {
            conversation_id,
            item_id,
        } => Response::DismissSent {
            queue: heap(|| core.dismiss_sent(&conversation_id, &item_id)).await?,
        },
        Request::DismissNotice {
            conversation_id,
            at_ms,
        } => {
            heap(|| core.dismiss_notice(&conversation_id, at_ms)).await?;
            Response::DismissNotice
        }
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
                attachment: heap(|| core.add_attachment(name, mime, bytes, pasted)).await?,
            }
        }
        Request::PinDraftAttachments { scope, attachments } => {
            heap(|| core.pin_draft_attachments(scope, attachments)).await?;
            Response::PinDraftAttachments
        }
        Request::ReadAttachment { id } => Response::ReadAttachment {
            data: BASE64.encode(heap(|| core.read_attachment(&id)).await?),
        },
        Request::ListWorkerEvents {
            task_id,
            before,
            limit,
        } => Response::ListWorkerEvents {
            page: heap(|| core.list_worker_events(&task_id, before, limit)).await?,
        },
        Request::GetThreadMetrics { conversation_id } => Response::GetThreadMetrics {
            metrics: heap(|| daemon.sessions.thread_metrics(&conversation_id)).await?,
        },
        Request::ReadArtifact { id, offset, limit } => {
            let (bytes, total_bytes) = heap(|| core.read_blob_range(id, offset, limit)).await?;
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
            heap(|| sessions.save_artifact(id, path)).await?;
            Response::SaveArtifact
        }
        Request::OpenArtifact { id, file_name } => Response::OpenArtifact {
            path: heap(|| sessions.artifact_copy(id, file_name))
                .await?
                .display()
                .to_string(),
        },
        Request::ResolveFileLink {
            conversation_id,
            link,
            path,
            at_ms,
        } => Response::ResolveFileLink {
            exists: std::path::Path::new(&path).is_absolute()
                && tokio::fs::try_exists(&path).await.unwrap_or(false),
            stored: heap(|| sessions.stored_output(&conversation_id, &link, at_ms)).await?,
        },
        Request::SiteIcon { origin } => Response::SiteIcon {
            icon: daemon.site_icons.icon(&origin).await,
        },
        Request::GetBrain { project_id } => Response::GetBrain {
            overview: Box::new(heap(|| sessions.brain_overview(project_id)).await?),
        },
        Request::QueryBrain { project_id, query } => Response::QueryBrain {
            answer: Box::new(heap(|| sessions.query_brain(project_id, query)).await?),
        },
        Request::GetBrainGraph { project_id, filter } => Response::GetBrainGraph {
            graph: Box::new(heap(|| sessions.brain_graph(project_id, filter)).await?),
        },
        Request::ExportConventions { project_id, path } => Response::ExportConventions {
            export: heap(|| sessions.export_conventions(project_id, path)).await?,
        },
        Request::RebuildIndex { project_id } => {
            heap(|| sessions.rebuild_index(project_id)).await?;
            Response::RebuildIndex
        }
        Request::GetRepoInfo { path } => Response::GetRepoInfo {
            repo: heap(|| sessions.repo_info(path)).await?,
        },
        Request::ListCliCommands {
            conversation_id,
            provider,
            project_id,
            repo,
        } => Response::ListCliCommands {
            commands: heap(|| sessions.cli_commands(conversation_id, provider, project_id, repo))
                .await?,
        },
        Request::FindProjects => Response::FindProjects {
            candidates: heap(|| sessions.find_projects()).await?,
        },
        Request::FindRepos => Response::FindRepos {
            repos: heap(|| sessions.find_repos()).await?,
        },
        Request::BrowseFolders { path } => Response::BrowseFolders {
            listing: heap(|| sessions.browse_folders(path)).await?,
        },
        Request::CheckFolder { path } => Response::CheckFolder {
            check: heap(|| sessions.check_folder(path)).await?,
        },
        Request::AddProject { path, name, init } => Response::AddProject {
            project: Box::new(heap(|| sessions.add_project(path, name, init)).await?),
        },
        Request::ListFiles {
            conversation_id,
            query,
        } => {
            let (files, truncated) = heap(|| sessions.list_files(&conversation_id, query)).await?;
            Response::ListFiles { files, truncated }
        }
        Request::ReadFile {
            conversation_id,
            path,
        } => Response::ReadFile {
            file: heap(|| sessions.read_file(&conversation_id, path)).await?,
        },
        Request::OpenTerminal { .. }
        | Request::AddAccount { .. }
        | Request::SignInAccount { .. } => {
            return Err(IpcError::from(brigadier_core::Error::Invalid(
                "a terminal opens on the connection that shows it".into(),
            )));
        }
        Request::CloneProject { .. } => {
            return Err(IpcError::from(brigadier_core::Error::Invalid(
                "a clone is answered on the connection that asked for it".into(),
            )));
        }
        Request::OpenSideChat {
            conversation_id,
            side_chat_id,
        } => Response::OpenSideChat {
            conversation: Box::new(
                heap(|| sessions.open_side_chat(&conversation_id, side_chat_id)).await?,
            ),
        },
        Request::OpenTabChat {
            conversation_id,
            tab_id,
            target,
            chat_id,
        } => Response::OpenTabChat {
            conversation: Box::new(
                heap(|| sessions.open_tab_chat(&conversation_id, tab_id, target, chat_id)).await?,
            ),
        },
        Request::KeepTabChats { chat_ids } => {
            heap(|| sessions.keep_tab_chats(chat_ids)).await?;
            Response::KeepTabChats
        }
        Request::SetTabPage {
            chat_id,
            url,
            title,
            text,
            selection,
        } => {
            sessions.set_tab_page(
                &chat_id,
                brigadier_core::manager::BrowserPage {
                    url,
                    title,
                    text,
                    selection,
                },
            )?;
            Response::SetTabPage
        }
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
        Request::ClearTerminal { terminal_id } => {
            daemon.terminals.clear(&terminal_id)?;
            Response::ClearTerminal
        }
        Request::CloseTerminal { terminal_id } => {
            let account = daemon
                .terminals
                .session_of(&terminal_id)
                .and_then(|key| key.strip_prefix("account:").map(str::to_owned))
                .and_then(|id| {
                    daemon
                        .runtime
                        .accounts_view()
                        .accounts
                        .into_iter()
                        .find(|view| view.account.account.as_deref() == Some(id.as_str()))
                });
            if let Some(account) = account {
                daemon
                    .terminals
                    .close_account_sign_in(
                        &terminal_id,
                        account.checked_at_ms,
                        account.status.is_some_and(|status| status.logged_in),
                        crate::connect::LINGER,
                    )
                    .await;
            } else {
                daemon.terminals.close(&terminal_id);
            }
            Response::CloseTerminal
        }
        Request::GetComputerUse => Response::GetComputerUse {
            computer_use: heap(|| sessions.computer_use_status()).await,
        },
        Request::GrantComputerPermission { permission } => {
            heap(|| sessions.grant_computer_permission(permission))
                .await
                .map_err(computer_error)?;
            Response::GrantComputerPermission
        }
        Request::CheckComputerUse => Response::CheckComputerUse {
            computer_use: heap(|| sessions.check_computer_use())
                .await
                .map_err(computer_error)?,
        },
        Request::ConfirmComputerCapture => {
            let (computer_use, problem) = heap(|| sessions.confirm_computer_capture())
                .await
                .map_err(computer_error)?;
            Response::ConfirmComputerCapture {
                computer_use,
                problem,
            }
        }
        Request::StopComputerSession {
            session,
            bundle_id,
            app,
        } => {
            heap(|| sessions.stop_computer_session(&session, bundle_id.as_deref(), &app))
                .await
                .map_err(computer_error)?;
            Response::StopComputerSession
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
            heap(|| sessions.rate(&conversation_id, subject, rating)).await?;
            Response::RateMessage
        }
        Request::GetSessionDiff { id } => {
            let diff = heap(|| sessions.session_diff_stat(&id)).await?;
            Response::GetSessionDiff {
                commits: diff.as_ref().map_or(0, |(_, commits)| *commits),
                stat: diff.map(|(stat, _)| stat),
            }
        }
        Request::GetWorkerDiffs { conversation_id } => Response::GetWorkerDiffs {
            diffs: heap(|| sessions.worker_diffs(&conversation_id)).await?,
        },
        Request::GetWorkerSource {
            conversation_id,
            task_id,
        } => Response::GetWorkerSource {
            source: heap(|| sessions.worker_source(&conversation_id, &task_id)).await?,
        },
        Request::SteerQueued {
            conversation_id,
            item_id,
        } => {
            heap(|| sessions.steer_queued(conversation_id, item_id)).await?;
            Response::SteerQueued
        }
        Request::Interrupt { conversation_id } => {
            heap(|| sessions.interrupt(conversation_id)).await?;
            Response::Interrupt
        }
        Request::Resume { conversation_id } => {
            heap(|| sessions.resume(conversation_id)).await?;
            Response::Resume
        }
        Request::StopPreview {
            conversation_id,
            preview_id,
        } => {
            heap(|| sessions.stop_preview(conversation_id, preview_id)).await?;
            Response::StopPreview
        }
        Request::PausePreview {
            conversation_id,
            preview_id,
        } => {
            heap(|| sessions.pause_preview(conversation_id, preview_id)).await?;
            Response::PausePreview
        }
        Request::ResumePreview {
            conversation_id,
            preview_id,
        } => {
            heap(|| sessions.resume_preview(conversation_id, preview_id)).await?;
            Response::ResumePreview
        }
        Request::RestartPreview {
            conversation_id,
            preview_id,
        } => {
            heap(|| sessions.restart_preview(conversation_id, preview_id)).await?;
            Response::RestartPreview
        }
        Request::DismissBackground {
            conversation_id,
            id,
        } => {
            heap(|| sessions.dismiss_background(conversation_id, id)).await?;
            Response::DismissBackground
        }
        Request::ClearPreviews { conversation_id } => {
            heap(|| sessions.clear_previews(conversation_id)).await?;
            Response::ClearPreviews
        }
        Request::PreviewLog {
            conversation_id,
            preview_id,
            lines,
        } => {
            let (tail, url) =
                heap(|| sessions.preview_log_tail(conversation_id, preview_id, lines)).await?;
            Response::PreviewLog { tail, url }
        }
        Request::Compact { conversation_id } => {
            heap(|| sessions.compact(conversation_id)).await?;
            Response::Compact
        }
        Request::GetConversationStatus { conversation_id } => Response::GetConversationStatus {
            status: heap(|| sessions.conversation_status(conversation_id)).await?,
        },
        Request::EditMessage {
            conversation_id,
            message_id,
            text,
            attachments,
        } => {
            heap(|| sessions.edit_message(conversation_id, message_id, text, attachments)).await?;
            Response::EditMessage
        }
        Request::Regenerate {
            conversation_id,
            request_id,
        } => {
            heap(|| sessions.regenerate(conversation_id, request_id)).await?;
            Response::Regenerate
        }
        Request::SwitchBranch {
            conversation_id,
            head,
        } => {
            heap(|| sessions.switch_branch(conversation_id, head)).await?;
            Response::SwitchBranch
        }
        Request::PrepareTurnRevert {
            conversation_id,
            request_id,
        } => Response::PrepareTurnRevert {
            preview: heap(|| sessions.prepare_turn_revert(conversation_id, request_id)).await?,
        },
        Request::RevertTurn {
            conversation_id,
            request_id,
            expected_tree,
        } => Response::RevertTurn {
            outcome: heap(|| sessions.revert_turn(conversation_id, request_id, expected_tree))
                .await?,
        },
        Request::GetPendingChanges {
            conversation_id,
            path,
        } => Response::GetPendingChanges {
            pending: heap(|| sessions.pending_changes(conversation_id, path)).await?,
        },
        Request::KeepChanges {
            conversation_id,
            path,
            hunks,
        } => {
            heap(|| sessions.keep_changes(conversation_id, path, hunks)).await?;
            Response::KeepChanges
        }
        Request::UndoPendingChanges {
            conversation_id,
            path,
            hunks,
        } => {
            let outcome =
                heap(|| sessions.undo_pending_changes(conversation_id, path, hunks)).await;
            daemon.metrics.undo_ended(outcome.is_ok());
            Response::UndoPendingChanges { outcome: outcome? }
        }
        Request::ReadThreadImage {
            conversation_id,
            item_id,
        } => {
            let (data, mime) =
                heap(|| sessions.read_thread_image(&conversation_id, &item_id)).await?;
            Response::ReadThreadImage { data, mime }
        }
        Request::StopBackgroundTask {
            conversation_id,
            worker_id,
            task_id,
            started_at_ms,
        } => {
            sessions
                .stop_background_task(&conversation_id, worker_id.as_ref(), task_id, started_at_ms)
                .await?;
            Response::StopBackgroundTask
        }
        Request::ReadBackgroundTaskOutput {
            conversation_id,
            worker_id,
            task_id,
            started_at_ms,
        } => Response::ReadBackgroundTaskOutput {
            output: sessions.background_task_output(
                &conversation_id,
                worker_id.as_ref(),
                &task_id,
                started_at_ms,
            )?,
        },
        Request::ThreadView { conversation_id } => Response::ThreadView {
            view: heap(|| sessions.thread_view_with_workers(&conversation_id)).await?,
        },
        Request::ContinueWaiting { conversation_id } => {
            heap(|| sessions.continue_waiting(&conversation_id)).await?;
            Response::ContinueWaiting
        }
        Request::GetBrigadeSkill => {
            let skill = sessions.brigade_skill();
            Response::GetBrigadeSkill {
                custom: skill.custom,
                path: skill.path.display().to_string(),
            }
        }
        Request::ResetBrigadeSkill => {
            let skill = sessions.reset_brigade_skill()?;
            Response::ResetBrigadeSkill {
                custom: skill.custom,
                path: skill.path.display().to_string(),
            }
        }
        Request::CreateBrigadeSkill => {
            let skill = sessions.create_brigade_skill()?;
            Response::CreateBrigadeSkill {
                custom: skill.custom,
                path: skill.path.display().to_string(),
            }
        }
        Request::GetModelRules => Response::GetModelRules {
            rules: sessions.model_rules()?,
        },
        Request::SetModelRules { rows, rules } => Response::SetModelRules {
            rules: sessions.set_model_rules(&rows, &rules)?,
        },
        Request::DontResume { conversation_id } => {
            heap(|| sessions.dont_resume(&conversation_id)).await?;
            Response::DontResume
        }
        Request::WorkerThread {
            conversation_id,
            task_id,
        } => Response::WorkerThread {
            view: heap(|| sessions.worker_thread(&conversation_id, &task_id)).await?,
        },
        Request::ReadSessionFileImage {
            conversation_id,
            image,
        } => {
            let (data, mime) = sessions.thread_image(&conversation_id, &image)?;
            Response::ReadThreadImage { data, mime }
        }
        Request::GetGitState { conversation_id } => Response::GetGitState {
            state: heap(|| sessions.git_state(&conversation_id)).await?,
        },
        Request::GetBackgroundHistory {
            conversation_id,
            before,
            limit,
        } => Response::GetBackgroundHistory {
            history: heap(|| sessions.background_history(&conversation_id, before, limit)).await?,
        },
        Request::ReadBackgroundHistoryOutput {
            conversation_id,
            record_id,
        } => Response::ReadBackgroundHistoryOutput {
            output: heap(|| sessions.background_history_output(&conversation_id, &record_id))
                .await?,
        },
        Request::GetTrackedWork { conversation_id } => Response::GetTrackedWork {
            work: heap(|| sessions.tracked_work_status(&conversation_id)).await?,
        },
        Request::WatchPullRequest {
            conversation_id,
            url,
            account,
        } => Response::WatchPullRequest {
            pull_request: heap(|| {
                sessions.watch_pull_request(&conversation_id, &url, account, "you")
            })
            .await?,
        },
        Request::ForgetPullRequest {
            conversation_id,
            key,
        } => {
            heap(|| sessions.forget_pull_request(&conversation_id, &key)).await?;
            Response::ForgetPullRequest
        }
        Request::AddWaitingOn {
            conversation_id,
            wait,
        } => Response::AddWaitingOn {
            wait: heap(|| sessions.add_waiting_on(&conversation_id, "you", wait)).await?,
        },
        Request::CompleteWaitingOn {
            conversation_id,
            id,
        } => Response::CompleteWaitingOn {
            wait: heap(|| sessions.complete_waiting_on(&conversation_id, &id)).await?,
        },
        Request::GetPullRequest { conversation_id } => {
            let status = heap(|| sessions.pull_request(&conversation_id)).await?;
            Response::GetPullRequest {
                pull_request: status.pull_request,
                auth_required: status.auth_required,
            }
        }
        Request::CommitChanges {
            conversation_id,
            message,
            include_unstaged,
            push,
            amend,
        } => Response::CommitChanges {
            outcome: heap(|| {
                sessions.commit_changes(
                    &conversation_id,
                    message,
                    include_unstaged,
                    push,
                    amend.unwrap_or(false),
                )
            })
            .await?,
        },
        Request::PushChanges { conversation_id } => Response::PushChanges {
            branch: heap(|| sessions.push_changes(&conversation_id)).await?,
        },
        Request::GetSourceState { conversation_id } => Response::GetSourceState {
            state: heap(|| sessions.source_state(&conversation_id)).await?,
        },
        Request::StageFiles {
            conversation_id,
            paths,
        } => Response::StageFiles {
            state: heap(|| sessions.stage_files(&conversation_id, paths)).await?,
        },
        Request::UnstageFiles {
            conversation_id,
            paths,
        } => Response::UnstageFiles {
            state: heap(|| sessions.unstage_files(&conversation_id, paths)).await?,
        },
        Request::DiscardFiles {
            conversation_id,
            scope,
            paths,
        } => Response::DiscardFiles {
            state: heap(|| sessions.discard_files(&conversation_id, scope, paths)).await?,
        },
        Request::GetWorkspaceTree { target } => Response::GetWorkspaceTree {
            tree: heap(|| sessions.workspace_tree(&target)).await?,
        },
        Request::ListWorkspaceFolder { target, path } => Response::ListWorkspaceFolder {
            folder: heap(|| sessions.workspace_folder(&target, path)).await?,
        },
        Request::ReplaceInFiles {
            target,
            query,
            replacement,
            files,
        } => Response::ReplaceInFiles {
            outcome: heap(|| sessions.replace_in_files(&target, query, replacement, files)).await?,
        },
        Request::ListProjectFiles { project_id, query } => {
            let (files, truncated) =
                heap(|| sessions.list_project_files(&project_id, query)).await?;
            Response::ListProjectFiles { files, truncated }
        }
        Request::GetProjectSourceState { project_id } => Response::GetProjectSourceState {
            state: heap(|| sessions.project_source_state(&project_id)).await?,
        },
        Request::ChangeProjectSource { project_id, change } => Response::ChangeProjectSource {
            state: heap(|| sessions.change_project_source(&project_id, change)).await?,
        },
        Request::CommitProjectChanges {
            project_id,
            message,
            include_unstaged,
            push,
            amend,
        } => Response::CommitProjectChanges {
            outcome: heap(|| {
                sessions.commit_project_changes(
                    &project_id,
                    message,
                    include_unstaged,
                    push,
                    amend.unwrap_or(false),
                )
            })
            .await?,
        },
        Request::ListSourceBranches { target } => Response::ListSourceBranches {
            branches: heap(|| sessions.source_branches(&target)).await?,
        },
        Request::SourceAction { target, action } => Response::SourceAction {
            state: heap(|| sessions.source_action(&target, action)).await?,
        },
        Request::GetSourceGitOutput { target } => Response::GetSourceGitOutput {
            runs: heap(|| sessions.source_git_output(&target)).await?,
        },
        Request::GetConflictFile { target, path } => Response::GetConflictFile {
            file: heap(|| sessions.conflict_file(&target, path)).await?,
        },
        Request::WriteCommitMessage { target } => {
            let (message, note) = heap(|| sessions.write_source_message(&target)).await?;
            Response::WriteCommitMessage { message, note }
        }
        Request::GetReviewDiff {
            conversation_id,
            scope,
            whole_files,
            ignore_whitespace,
        } => Response::GetReviewDiff {
            review: heap(|| {
                sessions.review_diff(&conversation_id, scope, whole_files, ignore_whitespace)
            })
            .await?,
        },
        Request::AnswerCard {
            conversation_id,
            card_id,
            decision,
        } => {
            heap(|| sessions.answer_card(conversation_id, card_id, decision)).await?;
            Response::AnswerCard
        }
        // Answered by the connection, which keeps who sent it with the answer.
        Request::AnswerQuestion { .. } | Request::MergeSession { .. } => {
            return Err(IpcError::from(brigadier_core::Error::Invalid(
                "an answer or a merge click comes through a client's connection".into(),
            )));
        }
        Request::WriteUpFeedback { text } => Response::WriteUpFeedback {
            write_up: heap(|| sessions.write_up_feedback(&text)).await?,
        },
        Request::FileFeedback { items } => {
            let filed = heap(|| sessions.file_feedback(&items)).await;
            Response::FileFeedback {
                signed_in: filed.is_some(),
                results: filed.unwrap_or_default(),
            }
        }
        Request::FeedbackAccount => {
            let account = heap(|| sessions.feedback_account()).await;
            Response::FeedbackAccount {
                signed_in: account.is_some(),
                login: account.flatten(),
            }
        }
        Request::StartGitHubSignIn => Response::StartGitHubSignIn {
            sign_in: heap(|| sessions.start_github_sign_in()).await?,
        },
        Request::WaitGitHubSignIn => {
            heap(|| sessions.wait_github_sign_in()).await?;
            Response::WaitGitHubSignIn
        }
        Request::CancelGitHubSignIn => {
            sessions.cancel_github_sign_in();
            Response::CancelGitHubSignIn
        }
        Request::AnswerFeedbackOffer {
            conversation_id,
            offer_id,
            sent,
            results,
        } => {
            heap(|| sessions.answer_feedback_offer(&conversation_id, &offer_id, sent, results))
                .await?;
            Response::AnswerFeedbackOffer
        }
        Request::CloseWorkspace { conversation_id } => Response::CloseWorkspace {
            text: heap(|| sessions.close_workspace(&conversation_id)).await?,
        },
        Request::AppAnswer { id, result } => {
            sessions.app_answer(&id, result);
            Response::AppAnswer
        }
        Request::BrowserPaused {
            conversation_id,
            tab,
            paused,
        } => {
            sessions.browser_paused(&conversation_id, &tab, paused);
            Response::BrowserPaused
        }
        Request::StopTask { task_id } => {
            heap(|| sessions.stop_task_by_user(task_id)).await?;
            Response::StopTask
        }
        Request::StopWorkers { conversation_id } => Response::StopWorkers {
            task_ids: heap(|| sessions.stop_workers(conversation_id)).await?,
        },
        Request::SendMockupNotes {
            task_id,
            text,
            attachments,
        } => {
            heap(|| sessions.send_mockup_notes(task_id, text, attachments)).await?;
            Response::SendMockupNotes
        }
        Request::SendToWorker {
            conversation_id,
            task_id,
            text,
            attachments,
            mentions,
        } => Response::SendToWorker {
            queued: heap(|| {
                sessions.send_to_worker(conversation_id, task_id, text, attachments, mentions)
            })
            .await?,
        },
        Request::PauseTask { task_id } => {
            heap(|| sessions.pause_task(task_id)).await?;
            Response::PauseTask
        }
        Request::ResumeTask { task_id } => {
            heap(|| sessions.resume_task(task_id)).await?;
            Response::ResumeTask
        }
        Request::RestoreKeptWork { task_id } => Response::RestoreKeptWork {
            outcome: heap(|| sessions.restore_kept_work(task_id)).await?,
        },
        Request::Hibernate { id } => Response::Hibernate {
            conversation: Box::new(heap(|| sessions.hibernate(id)).await?),
        },
        Request::Archive { ids } => {
            let outcomes = heap(|| sessions.archive_all(&ids)).await;
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
                outcomes.push(heap(|| sessions.restore(id.clone())).await.map(Some));
            }
            Response::Restore {
                outcomes: lifecycle_outcomes(ids, outcomes),
            }
        }
        Request::PreviewRemoveProject { id } => Response::PreviewRemoveProject {
            removal: Box::new(heap(|| sessions.preview_remove_project(id)).await?),
        },
        Request::RemoveProject {
            id,
            delete_branches,
            keep_brain,
        } => {
            // Refused while one of its conversations works: before its terminals close.
            if let Some(why) = heap(|| sessions.preview_remove_project(id.clone()))
                .await?
                .blocked
            {
                return Err(invalid(why));
            }
            for conversation in core.catalog().conversations {
                if conversation.project_id.as_ref() == Some(&id) {
                    daemon.terminals.close_conversation(&conversation.id.0);
                }
            }
            Response::RemoveProject {
                report: heap(|| sessions.remove_project(id, delete_branches, keep_brain)).await?,
            }
        }
        Request::FreeUpProjects { ids } => Response::FreeUpProjects {
            report: heap(|| free_up_projects(daemon, ids)).await,
        },
        Request::Delete { ids } => {
            let outcomes = heap(|| sessions.delete_all(&ids)).await;
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
            branches: heap(|| sessions.preview_delete(&ids)).await,
            files: heap(|| sessions.preview_delete_files(&ids)).await,
        },
        Request::RenameConversation { id, title } => Response::RenameConversation {
            conversation: Box::new(heap(|| core.rename_conversation(id, title)).await?),
        },
        Request::SetPinned { id, pinned } => Response::SetPinned {
            conversation: Box::new(heap(|| core.set_pinned(id, pinned)).await?),
        },
        Request::AppendMessage {
            conversation_id,
            text,
        } => Response::AppendMessage {
            message: Box::new(heap(|| core.append_message(conversation_id, text)).await?),
        },
        Request::ListMessages {
            conversation_id,
            before,
            limit,
        } => Response::ListMessages {
            page: heap(|| core.list_messages(conversation_id, before, limit)).await?,
        },
        Request::SearchMessages {
            conversation_id,
            leaf,
            query,
            limit,
        } => Response::SearchMessages {
            found: heap(|| core.search_messages(conversation_id, leaf, query, limit)).await?,
        },
        Request::ReadBlobText { hash } => Response::ReadBlobText {
            text: heap(|| core.read_blob_text(hash)).await?,
        },
        Request::UpdateSettings { settings } => {
            let before = core.settings();
            let settings =
                heap(|| core.update_settings(brigadier_core::accounts::edited(&before, *settings)))
                    .await?;
            heap(|| daemon.awake.apply()).await;
            if before.computer_use != settings.computer_use {
                // The helper stops, and running agents' calls are refused, until it is on again.
                let sessions = daemon.sessions.clone();
                daemon
                    .supervisor
                    .spawn(async move { sessions.computer_use_turned().await });
            }
            if before.agent_paths != settings.agent_paths {
                // The chosen binaries run from now on: the agents, their accounts and their
                // updates are checked again with them.
                daemon.runtime.choose_agent_paths(&settings);
                let generation = daemon.runtime.refresh_all();
                let checking = daemon.clone();
                daemon.supervisor.spawn(async move {
                    // Updates are read from the checks of the chosen binaries.
                    checking
                        .runtime
                        .providers_checked(generation, std::time::Duration::from_secs(60))
                        .await;
                    crate::updates::check(&checking).await
                });
            }
            // An account change, or an agent or model turned back on, may let work waiting for
            // quota run now.
            if before.accounts != settings.accounts
                || before.manual_accounts != settings.manual_accounts
            {
                // A new account is set up and checked, a removed one dropped, and the account
                // new work starts on chosen again: work waiting for quota may run on it.
                let (runtime, sessions) = (daemon.runtime.clone(), daemon.sessions.clone());
                daemon.supervisor.spawn(async move {
                    runtime.sync_accounts().await;
                    sessions.retry_waiting_work().await;
                });
            } else if brigadier_core::routing::availability::wakes_waiting_work(&before, &settings)
            {
                let sessions = daemon.sessions.clone();
                daemon
                    .supervisor
                    .spawn(async move { heap(|| sessions.retry_waiting_work()).await });
            }
            if before.own_account_names != settings.own_account_names {
                // An own login renamed shows by its new name at once.
                let runtime = daemon.runtime.clone();
                daemon
                    .supervisor
                    .spawn(async move { runtime.publish_accounts().await });
            }
            Response::UpdateSettings {
                settings: Box::new(settings),
            }
        }
        Request::GetKeepAwake => Response::GetKeepAwake {
            status: heap(|| daemon.awake.apply()).await,
        },
        Request::SetUpLidClosed => Response::SetUpLidClosed {
            status: heap(|| daemon.awake.set_up_lid_closed()).await,
        },
        Request::EventsSince { after_seq, limit } => {
            let events = heap(|| daemon.store.read_since(after_seq, limit))
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
                metrics: heap(|| daemon.metrics.sample()).await,
                processes: heap(|| daemon.metrics.processes()).await,
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
        Request::GetConnect => Response::GetConnect {
            connect: daemon.connects.view(),
        },
        Request::ConnectAgent { provider } => {
            crate::connect::start(daemon, provider).map_err(brigadier_core::Error::Invalid)?;
            Response::ConnectAgent
        }
        Request::CancelConnect { provider } => {
            heap(|| crate::connect::cancel(daemon, provider)).await;
            Response::CancelConnect
        }
        Request::SendConnectCode { provider, code } => {
            crate::connect::send_code(daemon, provider, code)
                .map_err(brigadier_core::Error::Invalid)?;
            Response::SendConnectCode
        }
        Request::GetAccounts => Response::GetAccounts {
            accounts: daemon.runtime.accounts_view(),
        },
        Request::GetPluginCatalog => Response::GetPluginCatalog {
            catalog: daemon.sessions.cached_plugin_catalog().await?,
        },
        Request::SetPluginChoice { row_id, on } => {
            daemon.sessions.set_plugin_choice(&row_id, on)?;
            Response::SetPluginChoice
        }
        Request::SetPluginChoiceScope { row_id, scope } => {
            daemon.sessions.set_plugin_choice_scope(&row_id, scope)?;
            Response::SetPluginChoiceScope
        }
        Request::StartPluginSignIn { row_id } => Response::StartPluginSignIn {
            state: heap(|| daemon.sessions.start_plugin_sign_in(&row_id)).await?,
        },
        Request::GetPluginSignInStatus { row_id } => Response::GetPluginSignInStatus {
            state: heap(|| daemon.sessions.get_plugin_sign_in_status(&row_id)).await?,
        },
        Request::CancelPluginSignIn { row_id } => Response::CancelPluginSignIn {
            state: heap(|| daemon.sessions.cancel_plugin_sign_in(&row_id)).await?,
        },
        Request::CheckPluginAdd { text, name } => Response::CheckPluginAdd {
            check: daemon.sessions.check_plugin_add(text, name).await?,
        },
        Request::AddPlugin {
            text,
            name,
            project_ids,
            values,
            source,
        } => {
            daemon
                .sessions
                .add_library_draft(text, name, project_ids, values, source)
                .await?;
            daemon.sessions.rebuild_plugin_catalog().await?;
            Response::AddPlugin { job: String::new() }
        }
        Request::ImportPlugin {
            key,
            name,
            project_ids,
            contents,
        } => {
            daemon
                .sessions
                .import_library_draft(key, name, project_ids, contents)
                .await?;
            daemon.sessions.rebuild_plugin_catalog().await?;
            Response::ImportPlugin { job: String::new() }
        }
        Request::RemovePlugin { key } => {
            daemon.sessions.remove_library_item(key).await?;
            daemon.sessions.rebuild_plugin_catalog().await?;
            Response::RemovePlugin { job: String::new() }
        }
        Request::GetPluginItem { key } => Response::GetPluginItem {
            item: daemon.sessions.catalog_item_details(key)?,
        },
        Request::SavePluginSkill {
            key,
            path,
            contents,
            expected,
        } => Response::SavePluginSkill {
            outcome: daemon
                .sessions
                .save_catalog_skill(key, path, contents, expected)?,
        },
        Request::RemoveAccount { id } => Response::RemoveAccount {
            settings: Box::new(heap(|| daemon.sessions.remove_account(&id)).await?),
        },
        Request::RefreshAccounts => Response::RefreshAccounts {
            generation: daemon.runtime.refresh_all(),
        },
        Request::GetProviders => Response::GetProviders {
            view: heap(|| daemon.runtime.view()).await,
        },
        Request::RefreshProviders { provider } => {
            daemon.runtime.refresh_providers(provider);
            Response::RefreshProviders
        }
        Request::CheckAgentPath { provider, path } => {
            let runtime = &daemon.runtime;
            let checked = brigadier_providers::cli::check_binary(
                runtime.platform(),
                runtime.cli_env(),
                provider,
                std::path::Path::new(&path),
            )
            .await;
            Response::CheckAgentPath {
                version: checked.map_err(invalid)?,
            }
        }
        Request::GetUsage {} => Response::GetUsage {
            usage: Box::new(heap(|| daemon.sessions.usage_view()).await?),
        },
        Request::GetUsageActivity { days } => Response::GetUsageActivity {
            activity: daemon.sessions.usage_activity(days).await,
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
            heap(|| daemon.runtime.rankings_changed()).await;
            Response::ResetRankings {
                refresh: daemon.runtime.registry().rankings_refresh(),
            }
        }
        Request::CheckRegistry => {
            heap(|| crate::registry::check(daemon)).await;
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
            heap(|| {
                sessions.debug_inject_limit(
                    key,
                    provider,
                    window,
                    reset_in_minutes,
                    after_tool_calls,
                )
            })
            .await?;
            Response::DebugInjectLimit
        }
        #[cfg(debug_assertions)]
        Request::DebugInjectError {
            target,
            provider,
            error,
            after_tool_calls,
        } => {
            use brigadier_core::manager::fault::{FaultKey, InjectedError as Error};
            use brigadier_ipc::protocol::{FaultTarget, InjectedError};
            let key = match target {
                FaultTarget::Task { task_id } => FaultKey::Task(task_id),
                FaultTarget::Conversation { conversation_id } => {
                    FaultKey::Conversation(conversation_id)
                }
            };
            let error = match error {
                InjectedError::ContextWindow => Error::ContextWindow,
                InjectedError::Auth => Error::Auth,
                InjectedError::Billing => Error::Billing,
            };
            heap(|| sessions.debug_inject_error(key, provider, error, after_tool_calls)).await?;
            Response::DebugInjectError
        }
        #[cfg(debug_assertions)]
        Request::DebugPlantLearned {
            conversation_id,
            text,
        } => {
            sessions.debug_plant_learned(conversation_id, text);
            Response::DebugPlantLearned
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
                heap(|| {
                    daemon.runtime.start_session(StartRaw {
                        provider,
                        cwd,
                        model,
                        effort,
                        access,
                        approvals,
                        record,
                    })
                })
                .await?,
            ),
        },
        Request::ResumeRawSession { id } => Response::ResumeRawSession {
            session: Box::new(heap(|| daemon.runtime.resume_session(id)).await?),
        },
        Request::ForkRawSession { id } => Response::ForkRawSession {
            session: Box::new(heap(|| daemon.runtime.fork_session(id)).await?),
        },
        Request::SendRawSession { id, text, steer } => {
            heap(|| daemon.runtime.send(&id, text, steer)).await?;
            Response::SendRawSession
        }
        Request::InterruptRawSession { id } => {
            heap(|| daemon.runtime.interrupt(&id)).await?;
            Response::InterruptRawSession
        }
        Request::AnswerApproval {
            id,
            approval_id,
            decision,
        } => {
            heap(|| daemon.runtime.answer(&id, approval_id, decision)).await?;
            Response::AnswerApproval
        }
        Request::StopRawSession { id } => {
            heap(|| daemon.runtime.stop_session(&id)).await?;
            Response::StopRawSession
        }
        Request::CloseRawSession { id } => Response::CloseRawSession {
            session: Box::new(heap(|| daemon.runtime.close_session(id)).await?),
        },
        Request::ListRawEvents { id, before, limit } => Response::ListRawEvents {
            page: heap(|| daemon.runtime.transcript(&id, before, limit)).await?,
        },
        Request::ReplayFixture { fixture_id } => Response::ReplayFixture {
            session: Box::new(heap(|| daemon.runtime.replay(&fixture_id)).await?),
        },
        #[cfg(debug_assertions)]
        Request::SimulateUsageLimit { provider } => Response::SimulateUsageLimit {
            session: Box::new(heap(|| daemon.runtime.simulate_usage_limit(provider)).await?),
        },
        Request::GetDaemonActivity => Response::GetDaemonActivity {
            activity: DaemonActivity {
                // The asking connection is one of them.
                clients: u32::try_from(daemon.metrics.clients().saturating_sub(1))
                    .unwrap_or(u32::MAX),
                running: heap(|| idle::running(daemon)).await,
            },
        },
        Request::ScanStorage { app } => Response::ScanStorage {
            report: Box::new(
                heap(|| daemon.storage.scan(daemon, app))
                    .await
                    .map_err(invalid)?,
            ),
        },
        Request::CleanStorage { scan_id, items } => Response::CleanStorage {
            report: heap(|| daemon.storage.clean(daemon, &scan_id, items))
                .await
                .map_err(invalid)?,
        },
        Request::PreviewUninstall { app } => Response::PreviewUninstall {
            plan: Box::new(
                heap(|| daemon.uninstall.preview(daemon, app))
                    .await
                    .map_err(invalid)?,
            ),
        },
        Request::Uninstall {
            plan_id,
            keep_data,
            delete_branches,
        } => Response::Uninstall {
            report: heap(|| {
                daemon
                    .uninstall
                    .run(daemon, &plan_id, keep_data, delete_branches)
            })
            .await
            .map_err(invalid)?,
        },
        Request::PageCreate => Response::PageCreate {
            page: heap(|| on_pages(daemon, |pages| pages.create())).await?,
        },
        Request::PageRead { id } => Response::PageRead {
            page: heap(|| on_pages(daemon, move |pages| pages.read(&id))).await?,
        },
        Request::PageWrite {
            id,
            content,
            base_revision,
        } => Response::PageWrite {
            outcome: heap(|| {
                on_pages(daemon, move |pages| {
                    pages.write(&id, &content, &base_revision)
                })
            })
            .await?,
        },
        Request::PageList => Response::PageList {
            pages: heap(|| on_pages(daemon, |pages| pages.list())).await?,
        },
        Request::PageDelete { id } => {
            let page = id.clone();
            heap(|| on_pages(daemon, move |pages| pages.delete(&id))).await?;
            // The chats of the tabs that showed it go with it.
            heap(|| daemon.sessions.page_deleted(&page)).await;
            Response::PageDelete
        }
        Request::Subscribe { .. }
        | Request::SetMetricsStreaming { .. }
        | Request::WatchWorkspace { .. }
        | Request::StartSearch { .. }
        | Request::CancelSearch { .. }
        | Request::Shutdown => {
            return Err(IpcError {
                code: ErrorCode::Invalid,
                message: "handled by the session".into(),
            });
        }
    })
}

/// Runs a page request's file work (reads, flushed saves) off the async runtime. The
/// connection waits for it, so one window's saves land in the order it sent them.
/// Frees up projects not in use (Settings › Storage); their conversations' terminals close
/// with them, as a project removal's do.
async fn free_up_projects(daemon: &Daemon, ids: Vec<ProjectId>) -> FreeUpProjectsReport {
    let working = daemon.sessions.free_up_blocked(&ids).await;
    for conversation in daemon.core.catalog().conversations {
        if conversation
            .project_id
            .as_ref()
            .is_some_and(|project| ids.contains(project) && !working.contains(project))
        {
            daemon.terminals.close_conversation(&conversation.id.0);
        }
    }
    daemon.sessions.free_up_projects(ids).await
}

async fn on_pages<T: Send + 'static>(
    daemon: &Arc<Daemon>,
    work: impl FnOnce(&Pages) -> Result<T, PageError> + Send + 'static,
) -> Result<T, IpcError> {
    let pages = daemon.pages.clone();
    tokio::task::spawn_blocking(move || work(&pages))
        .await
        .map_err(|error| IpcError {
            code: ErrorCode::Internal,
            message: format!("the page request stopped: {error}"),
        })?
        .map_err(IpcError::from)
}

/// A computer-use request that didn't work, in the helper's own words.
fn computer_error(message: String) -> IpcError {
    IpcError::from(brigadier_core::Error::Invalid(message))
}

/// Each remote server's site icon on the page, as far as it is known within a moment: a lookup
/// still running goes on and is cached, so the next read has it.
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_window_that_asks_for_the_speech_model_hears_its_download_end() {
        let models =
            std::env::temp_dir().join(format!("brigadier-models-{}", uuid::Uuid::new_v4()));
        let dictation = Arc::new(Dictation::new(models.clone()));
        let mut feed = None;
        assert!(!follow_status(&dictation, &mut feed).installed);
        // The download ends (here: the model is already there), after the window asked.
        let model = models.join("whisper").join("ggml-base-q5_1.bin");
        std::fs::create_dir_all(model.parent().unwrap()).unwrap();
        std::fs::File::create(&model)
            .unwrap()
            .set_len(dictation.status().model_bytes)
            .unwrap();
        dictation.download();
        let update = tokio::time::timeout(Duration::from_secs(5), next_dictation_update(&mut feed))
            .await
            .unwrap();
        let _ = std::fs::remove_dir_all(&models);
        assert!(matches!(update, Some(DictationUpdate::Downloaded)));
    }

    /// The first run's project scan ran git in a folder macOS then asked the user about, and
    /// while the prompt waited every request after it on the connection timed out: Continue
    /// without projects (its settings write rolled back) and Add project. The requests that
    /// read the user's folders are answered beside the others; a settings write still is in
    /// turn.
    #[test]
    fn a_stuck_folder_scan_holds_up_no_other_request() {
        let path = || "/Users/someone/Documents/app".to_owned();
        for request in [
            Request::FindProjects,
            Request::FindRepos,
            Request::GetRepoInfo { path: path() },
            Request::BrowseFolders { path: path() },
            Request::CheckFolder { path: path() },
            Request::AddProject {
                path: path(),
                name: String::new(),
                init: false,
            },
        ] {
            assert!(answered_beside(&request), "{request:?} is answered beside");
        }
        let settings = Box::new(brigadier_core::model::Settings::default());
        assert!(!answered_beside(&Request::UpdateSettings { settings }));
        assert!(!answered_beside(&Request::GetCatalog));
    }

    /// #491: a message sent while a session works showed nowhere for seconds. Its send waited
    /// in the connection's turn behind the side panels' and the Changes card's reads of the
    /// checkout (12 of them held a request 2.1 s on an idle Mac). Those reads go beside; the
    /// send, a Stop and the queue's changes stay in turn.
    #[test]
    fn a_checkouts_reads_hold_up_no_send() {
        let id = || ConversationId("c1".into());
        let target = || WorkspaceTarget::Conversation {
            conversation_id: id(),
        };
        for request in [
            Request::GetSourceState {
                conversation_id: id(),
            },
            Request::GetSessionDiff { id: id() },
            Request::GetWorkerDiffs {
                conversation_id: id(),
            },
            Request::GetWorkerSource {
                conversation_id: id(),
                task_id: brigadier_core::TaskId("t1".into()),
            },
            Request::GetPendingChanges {
                conversation_id: id(),
                path: None,
            },
            Request::GetWorkspaceTree { target: target() },
            Request::ListSourceBranches { target: target() },
        ] {
            assert!(read_beside(&request), "{request:?} is read beside");
        }
        for request in [
            Request::SendMessage {
                conversation_id: id(),
                text: "Also add CSV.".into(),
                fusion: false,
                attachments: Vec::new(),
                mentions: Vec::new(),
                steer: false,
                queue_index: None,
            },
            Request::Interrupt {
                conversation_id: id(),
            },
            Request::DeleteQueued {
                conversation_id: id(),
                item_id: "q1".into(),
            },
            Request::StageFiles {
                conversation_id: id(),
                paths: None,
            },
        ] {
            assert!(!read_beside(&request), "{request:?} stays in turn");
            assert!(!answered_beside(&request), "{request:?} stays in turn");
        }
    }

    /// Switching to a session showed it finished for as long as its view waited behind the
    /// connection's slower requests, minutes on a slow network (#484): the view waits for none.
    #[test]
    fn a_conversation_view_waits_for_no_slower_request() {
        let id = ConversationId("a".into());
        assert!(answered_beside(&Request::GetConversation {
            id,
            limit: 200
        }));
    }

    /// Send feedback's writer turns and GitHub calls take a minute or more: the connection's
    /// other requests and its events don't wait for them.
    #[test]
    fn send_feedback_holds_up_no_other_request() {
        for request in [
            Request::WriteUpFeedback {
                text: "Stop leaves a worker running".into(),
            },
            Request::FileFeedback { items: Vec::new() },
            Request::FeedbackAccount,
            Request::StartGitHubSignIn,
            Request::WaitGitHubSignIn,
        ] {
            assert!(answered_beside(&request), "{request:?} is answered beside");
        }
    }

    /// A Codex rollout can hold a single output larger than the wire limit (#527). The
    /// daemon sends it as bounded parts, and the desktop's reader recovers every byte and
    /// reads another response on the same connection instead of reconnecting.
    #[tokio::test]
    async fn a_rollout_over_the_frame_limit_opens_and_keeps_the_connection() {
        use brigadier_ipc::frame::{FrameReader, FrameWriter};
        use brigadier_providers::{ProviderEvent, ProviderKind, session_file};
        let path = std::env::temp_dir().join(format!(
            "20261022-0805-take-over-5-sessions-w181-rollout-{}.jsonl",
            std::process::id()
        ));
        let output = "雪\n\"\\".repeat(brigadier_ipc::MAX_FRAME_BYTES / 6 + 1);
        assert!(output.len() > brigadier_ipc::MAX_FRAME_BYTES);
        let rollout = serde_json::json!({
            "type": "event_msg",
            "payload": {"type": "item_completed", "item": {
                "type": "CommandExecution", "id": "large-command",
                "command": ["cat", "large.log"], "status": "completed",
                "exit_code": 0, "aggregated_output": output
            }}
        });
        std::fs::write(&path, rollout.to_string()).unwrap();
        let entries = session_file::read(ProviderKind::Codex, &path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(entries.len(), 1);
        let view = brigadier_core::thread_view::ThreadView {
            entries,
            turns: vec![],
            background: vec![],
            mcp: vec![],
            workers: vec![],
            seams: vec![],
            jobs: Default::default(),
        };
        let frame = ServerFrame::Response {
            id: 7,
            result: Outcome::Ok {
                value: Box::new(Response::ThreadView { view }),
            },
        };
        assert!(serde_json::to_vec(&frame).unwrap().len() > brigadier_ipc::MAX_FRAME_BYTES);
        assert!(
            matches!(
                FrameWriter::new(tokio::io::sink()).write(&frame).await,
                Err(brigadier_ipc::Error::FrameTooLarge(_))
            ),
            "the old single-frame path cannot send this view"
        );
        let ServerFrame::Response { id, result } = frame else {
            unreachable!()
        };
        let (send, recv) = tokio::io::duplex(64 * 1024);
        let sending = async {
            let mut writer = FrameWriter::new(send);
            writer.write_response(id, result).await.unwrap();
            writer
                .write_response(
                    8,
                    Outcome::Ok {
                        value: Box::new(Response::Shutdown),
                    },
                )
                .await
                .unwrap();
        };
        let receiving = async {
            let mut reader = FrameReader::new(recv);
            let Some(ServerFrame::Response {
                id: 7,
                result: Outcome::Ok { value },
            }) = reader.read_server().await.unwrap()
            else {
                panic!("the full thread must open");
            };
            let Response::ThreadView { view } = *value else {
                panic!("expected thread view")
            };
            let ProviderEvent::Command {
                output: Some(actual),
                ..
            } = &view.entries[0].event
            else {
                panic!("expected command output")
            };
            assert_eq!(actual, &output);
            assert!(
                matches!(
                    reader.read_server().await.unwrap(),
                    Some(ServerFrame::Response { id: 8, .. })
                ),
                "the same connection remains usable"
            );
        };
        tokio::join!(sending, receiving);
    }

    /// Every await of `handle_request` goes through [`heap`]: an unoptimized build keeps all
    /// of a function's awaited futures in its stack frame at once, and before that the
    /// dispatch's frame took 1.26 MB of a runtime thread's 2 MiB stack, so a message to a
    /// session overflowed it. Its future was 35 KB then and is under 2 KB now; one past 4 KB
    /// means an engine future is awaited around it again.
    #[test]
    fn the_request_dispatch_keeps_its_futures_on_the_heap() {
        fn future_size<R>(_: fn(&'static Arc<Daemon>, Request) -> R) -> usize {
            std::mem::size_of::<R>()
        }
        let size = future_size(handle_request);
        assert!(size <= 4 << 10, "handle_request's future is {size} bytes");
    }
}
