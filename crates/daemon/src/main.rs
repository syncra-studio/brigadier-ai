//! `brigadierd`: the Brigadier core process.
//!
//! Launched detached by the app, it keeps running when the window closes so long sessions
//! continue. Lifecycle:
//!
//! 1. Take the single-instance lock (released by the OS if we crash).
//! 2. Open the store, bind the IPC endpoint, publish the per-launch token.
//! 3. Serve until SIGTERM/SIGINT, a client's `shutdown`, or a fatal failure.
//! 4. Orderly quit: stop accepting, end every CLI session and store its last events, stop
//!    admitting writes, commit everything queued, acknowledge the client that asked, close
//!    connections, remove the token, exit 0.
//!
//! A critical task or the store writer dying is logged and exits with code 70 instead. A quit
//! that outlasts [`SHUTDOWN_DEADLINE`] or gets a second signal also exits with code 70, so a
//! stuck daemon can't keep the lock with its socket gone; the next launch sweeps what it left.
//!
//! The same binary has more jobs, chosen before any of the above runs:
//!
//! - `brigadierd hook post-tool-use`: a Claude thread's output hook, and `brigadierd hook
//!   stop` a Claude or Codex thread's Stop hook ([`hook`]);
//! - `brigadierd quit`: asks a data directory's daemon to quit ([`quit`]).

mod awake;
mod connect;
mod dictation;
mod hook;
mod idle;
mod logging;
mod metrics;
mod news;
mod quit;
mod registry;
mod server;
mod site_icons;
mod storage;
mod supervisor;
mod terminals;
mod text_round;
mod uninstall;
mod updates;
mod upgrade;
#[cfg(windows)]
mod user_path;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use brigadier_core::Core;
use brigadier_core::manager::{ManagerConfig, SessionManager};
use brigadier_core::pages::{PAGES_DIR, Pages};
use brigadier_core::runtime::{Runtime, Spawner};
use brigadier_ipc::protocol::{DaemonInfo, PROTOCOL_VERSION};
use brigadier_ipc::{Listener, Token};
use brigadier_sandbox::{InstanceLock, Platform, PlatformOptions};
use brigadier_store::{CHECKPOINT_INTERVAL, Store, StoreConfig};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::metrics::Metrics;
use crate::server::Daemon;
use crate::supervisor::Supervisor;

/// Exit code for an internal failure (EX_SOFTWARE).
const EXIT_FATAL: u8 = 70;
/// How long connections get to receive their goodbye after the store drained.
const CLOSE_GRACE: Duration = Duration::from_secs(2);
/// Longest a quit may take before the process exits regardless. Its bounded steps add up to about
/// 30 s at worst, and `brigadierd quit` waits 40 s before it reports a daemon as stuck.
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(40);
/// How long the log's background thread gets to write the forced exit's reason.
const LOG_FLUSH: Duration = Duration::from_millis(100);
const READERS: usize = 4;
const RUNTIME_WORKERS: usize = 2;
/// What all of the daemon's SQLite connections may hold together, most of it page cache. Each
/// connection would otherwise keep up to 2 MB of it, and a daemon with a project open has about
/// twenty (event store, Brains, code index), which filled keep it above the idle budget (PLAN.md
/// §4: < 60 MB) long after the work that read them.
const SQLITE_HEAP_BYTES: i64 = 16 * 1024 * 1024;

struct Args {
    data_dir: Option<PathBuf>,
    foreground: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        data_dir: None,
        foreground: false,
    };
    let mut iter = std::env::args_os().skip(1);
    while let Some(arg) = iter.next() {
        match arg.to_str() {
            Some("--data-dir") => {
                args.data_dir = Some(iter.next().ok_or("--data-dir needs a path")?.into());
            }
            Some("--foreground") => args.foreground = true,
            Some("--version") => {
                println!("brigadierd {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument {arg:?}")),
        }
    }
    Ok(args)
}

fn main() -> ExitCode {
    #[cfg(windows)]
    if let Some(exit) = connect::catch_browser_link() {
        return exit;
    }
    // `brigadierd hook post-tool-use [--data-dir PATH]`: a Claude thread's output hook; `hook
    // stop`: its Stop hook.
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "hook") {
        return hook::run(std::env::args_os().skip(2));
    }
    // `brigadierd transcribe <model>`: one dictation's speech engine (see `dictation`).
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "transcribe")
    {
        return dictation::transcribe_main(std::env::args_os().skip(2));
    }
    // `brigadierd quit [--data-dir PATH]`: asks that data directory's daemon to quit.
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "quit") {
        return quit::main(std::env::args_os().skip(2));
    }
    // `brigadierd uninstall-finish …`: what uninstalling removes once Brigadier quit (see
    // `uninstall`).
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "uninstall-finish")
    {
        return uninstall::finish_main(std::env::args_os().skip(2));
    }
    // `brigadierd index-scan <db> <root> <threads>`: one code index scan (see
    // `brigadier_index::ScanHelper`).
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "index-scan")
    {
        let code = brigadier_index::scan_helper_main(std::env::args_os().skip(2));
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    let args = match parse_args() {
        Ok(args) => args,
        Err(err) => {
            eprintln!(
                "brigadierd: {err}\nusage: brigadierd [--data-dir PATH] [--foreground]\n       brigadierd quit [--data-dir PATH]"
            );
            return ExitCode::from(2);
        }
    };
    let platform = match brigadier_sandbox::native(PlatformOptions {
        data_dir: args.data_dir,
    }) {
        Ok(platform) => platform,
        Err(err) => {
            eprintln!("brigadierd: {err}");
            return ExitCode::from(EXIT_FATAL);
        }
    };
    let _log_guard = match logging::init(&platform.paths().logs_dir, args.foreground) {
        Ok(guard) => guard,
        Err(err) => {
            eprintln!("brigadierd: cannot initialize logging: {err:#}");
            return ExitCode::from(EXIT_FATAL);
        }
    };
    // Every CLI, build and test inherits the daemon's open-files limit, and launchd's soft one
    // (256) is too few for a test suite.
    match brigadier_sandbox::limits::raise_open_files() {
        Ok(limits) => tracing::info!(soft = limits.soft, hard = limits.hard, "open-files limit"),
        Err(err) => tracing::warn!(error = %err, "could not raise the open-files limit"),
    }
    match start(platform) {
        Ok(code) => code,
        Err(err) => {
            tracing::error!(error = %format!("{err:#}"), "brigadierd failed");
            ExitCode::from(EXIT_FATAL)
        }
    }
}

fn start(platform: Arc<dyn Platform>) -> anyhow::Result<ExitCode> {
    let paths = platform.paths().clone();
    // Transcripts and the token live here: nobody but the current user may read them.
    for dir in [&paths.data_dir, &paths.run_dir] {
        platform
            .private_fs()
            .create_private_dir(dir)
            .with_context(|| format!("creating {}", dir.display()))?;
    }
    let lock = InstanceLock::try_acquire(&paths.lock_path).context("instance lock")?;
    let Some(_lock) = lock else {
        tracing::info!("another brigadierd owns this data directory; exiting");
        return Ok(ExitCode::SUCCESS);
    };
    // Folders of the command gate and git guard earlier versions kept (workers now run as the
    // permission level says, with nothing on their PATH or in their git config).
    for old in ["gate", "git-guard"] {
        let _ = std::fs::remove_dir_all(paths.data_dir.join(old));
    }
    let started_at_ms = brigadier_core::now_ms();
    tracing::info!(
        pid = std::process::id(),
        version = env!("CARGO_PKG_VERSION"),
        data_dir = %paths.data_dir.display(),
        "brigadierd starting"
    );

    // Process-wide: it covers every connection opened after it, whichever crate opens it.
    rusqlite::Connection::open_in_memory()
        .and_then(|conn| conn.pragma_update(None, "soft_heap_limit", SQLITE_HEAP_BYTES))
        .context("limiting SQLite's memory")?;
    // Opening and migrating the store is blocking work; do it before the runtime exists.
    let store = Store::open(StoreConfig {
        db_path: paths.db_path.clone(),
        blobs_dir: paths.blobs_dir.clone(),
        readers: READERS,
    })
    .context("opening the event store")?;
    let token = Token::generate().context("generating the IPC token")?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(RUNTIME_WORKERS)
        .thread_name("brigadierd-rt")
        .enable_all()
        .build()
        .context("starting the async runtime")?;
    let info = DaemonInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        protocol: PROTOCOL_VERSION,
        pid: std::process::id(),
        platform: platform.name().into(),
        started_at_ms,
        data_dir: paths.data_dir.display().to_string(),
    };
    let code = runtime.block_on(serve(platform.clone(), store, token, info));
    runtime.shutdown_timeout(Duration::from_secs(1));

    if let Err(err) = std::fs::remove_file(&paths.token_path) {
        tracing::warn!(error = %err, "could not remove the IPC token");
    }
    tracing::info!(code = ?code, "brigadierd stopped");
    Ok(code)
}

async fn serve(
    platform: Arc<dyn Platform>,
    store: Store,
    token: Token,
    info: DaemonInfo,
) -> ExitCode {
    match run(platform, store, token, info).await {
        Ok(code) => code,
        Err(err) => {
            tracing::error!(error = %format!("{err:#}"), "brigadierd failed");
            ExitCode::from(EXIT_FATAL)
        }
    }
}

async fn run(
    platform: Arc<dyn Platform>,
    store: Store,
    token: Token,
    info: DaemonInfo,
) -> anyhow::Result<ExitCode> {
    let stopping = CancellationToken::new();
    let closing = CancellationToken::new();
    let (supervisor, mut fatal) = Supervisor::new(stopping.clone());
    let (quit_tx, mut quit_rx) = mpsc::channel(1);
    let (drained_tx, drained_rx) = watch::channel(false);

    let core = Core::load(store.clone())
        .await
        .context("loading the catalog")?;
    let spawner: Spawner = {
        let supervisor = supervisor.clone();
        Arc::new(move |task| {
            supervisor.spawn(task);
        })
    };
    // Also sweeps what a crashed daemon left behind, before any client can start sessions.
    let providers = Runtime::start(core.clone(), platform.clone(), spawner.clone())
        .await
        .context("starting the provider runtime")?;
    let daemon_exe = std::env::current_exe().context("locating brigadierd")?;
    let cli_name = if cfg!(windows) {
        "brigadier.exe"
    } else {
        "brigadier"
    };
    let build_slots_ready = match brigadier_sandbox::build_slots::prepare(
        &platform.paths().data_dir,
        &daemon_exe.with_file_name(cli_name),
    ) {
        Ok(()) => true,
        Err(err) => {
            tracing::warn!(error = %err, "compiler slots unavailable; continuing without build caps");
            false
        }
    };
    let manager_config = ManagerConfig {
        daemon_exe,
        build_slots_ready,
    };
    let sessions = SessionManager::start(core.clone(), providers.clone(), spawner, manager_config)
        .await
        .context("starting the session manager")?;
    let awake = awake::Awake::new(
        core.clone(),
        sessions.clone(),
        platform.paths().data_dir.clone(),
    );
    awake.recover().await;
    let metrics = Metrics::start(
        supervisor.clone(),
        store.clone(),
        platform.clone(),
        &sessions,
    );
    let listener = Listener::bind(&*platform).context("binding the IPC endpoint")?;
    {
        let platform = platform.clone();
        let token = token.clone();
        tokio::task::spawn_blocking(move || {
            token.publish(&*platform, &platform.paths().token_path)
        })
        .await?
        .context("publishing the IPC token")?;
    }

    let daemon = Arc::new(Daemon::new(
        info,
        core,
        providers.clone(),
        sessions,
        store.clone(),
        metrics,
        supervisor.clone(),
        closing.clone(),
        quit_tx,
        drained_rx,
        Arc::new(dictation::Dictation::new(
            platform.paths().data_dir.join("models"),
        )),
        awake.clone(),
        Arc::new(Pages::open_in_background(
            platform.paths().data_dir.join(PAGES_DIR),
        )),
        Arc::new(site_icons::SiteIcons::new(
            platform.paths().data_dir.join("site-icons"),
        )),
    ));
    // "Open in terminal" runs a worker's session in the daemon's terminals.
    daemon.sessions.set_terminal_host(daemon.terminals.clone());
    // The thread reads and changes the page a message was sent from.
    daemon.sessions.set_pages(daemon.pages.clone());
    // `brigadier library login` opens each agent's sign-in page in the browser.
    daemon
        .sessions
        .set_browser(std::sync::Arc::new(crate::connect::open_in_browser));
    supervisor.spawn_critical(
        "ipc accept loop",
        server::accept_loop(daemon.clone(), listener, token),
    );
    supervisor.spawn_critical("wal checkpointer", checkpoint_loop(store.clone()));
    supervisor.spawn(awake.clone().run(stopping.clone()));
    supervisor.spawn(idle::exit_when_idle(daemon.clone(), stopping.clone()));
    supervisor.spawn(storage::compact_when_quiet(
        daemon.clone(),
        stopping.clone(),
    ));
    supervisor.spawn(registry::keep_current(daemon.clone(), stopping.clone()));
    supervisor.spawn(registry::keep_researched(daemon.clone(), stopping.clone()));
    supervisor.spawn(updates::keep_current(daemon.clone(), stopping.clone()));
    tracing::info!("brigadierd ready");

    let reason = tokio::select! {
        signal = termination_signal() => {
            limit_shutdown(SHUTDOWN_DEADLINE, 1);
            signal
        }
        Some(reason) = quit_rx.recv() => {
            // Two signals: at logout one reaches the daemon along with the app's quit, and that
            // one mustn't cut short the quit it is already doing.
            limit_shutdown(SHUTDOWN_DEADLINE, 2);
            reason
        }
        state = store.writer_stopped() => {
            tracing::error!(state = ?state, "store writer stopped; exiting");
            limit_shutdown(SHUTDOWN_DEADLINE, 2);
            stopping.cancel();
            store.stop_maintenance();
            awake.shutdown().await;
            daemon.terminals.close_all();
            daemon.sessions.shutdown().await;
            providers.shutdown().await;
            return Ok(ExitCode::from(EXIT_FATAL));
        }
        Some(reason) = fatal.recv() => {
            tracing::error!(reason = %reason, "critical task failed; exiting");
            limit_shutdown(SHUTDOWN_DEADLINE, 2);
            stopping.cancel();
            store.stop_maintenance();
            awake.shutdown().await;
            daemon.terminals.close_all();
            daemon.sessions.shutdown().await;
            providers.shutdown().await;
            return Ok(ExitCode::from(EXIT_FATAL));
        }
    };
    tracing::info!(reason, "shutting down");

    // 1. Stop accepting connections; critical tasks may now end without being fatal. A
    //    compaction running stops (rolled back whole) and none starts.
    stopping.cancel();
    store.stop_maintenance();
    // Sleep works normally again, even when the lid is closed.
    awake.shutdown().await;
    // 2. Stop admitting provider work, end every CLI session (bounded, whole process groups)
    //    and store their last events. Sessions, Chats and workers first: they are hosted by
    //    the provider runtime. The user's terminals end too.
    daemon.terminals.close_all();
    daemon.sessions.shutdown().await;
    providers.shutdown().await;
    // 3. Stop admitting writes and commit everything already queued.
    let drained = store.shutdown().await;
    // 4. Acknowledge the client that asked, then close every connection.
    drained_tx.send_replace(true);
    closing.cancel();
    daemon.connections.close();
    if tokio::time::timeout(CLOSE_GRACE, daemon.connections.wait())
        .await
        .is_err()
    {
        tracing::warn!("connections did not close in time");
    }
    match drained {
        Ok(()) => Ok(ExitCode::SUCCESS),
        Err(err) => {
            tracing::error!(error = %err, "store did not shut down cleanly");
            Ok(ExitCode::from(EXIT_FATAL))
        }
    }
}

/// From now on, exits the process once `deadline` passes or `signals` more signals arrive. On its
/// own thread and runtime, because what it guards against may have stopped the main runtime too.
fn limit_shutdown(deadline: Duration, signals: usize) {
    let limit = move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        let reason = match runtime {
            Ok(runtime) => runtime.block_on(async {
                tokio::select! {
                    () = tokio::time::sleep(deadline) => "the deadline passed",
                    () = async {
                        for _ in 0..signals {
                            termination_signal().await;
                        }
                    } => "another signal arrived",
                }
            }),
            Err(err) => {
                tracing::warn!(error = %err, "signals can't end a stuck shutdown");
                std::thread::sleep(deadline);
                "the deadline passed"
            }
        };
        tracing::error!(reason, "shutdown did not finish; exiting now");
        std::thread::sleep(LOG_FLUSH);
        std::process::exit(EXIT_FATAL.into());
    };
    if let Err(err) = std::thread::Builder::new()
        .name("brigadierd-shutdown-limit".into())
        .spawn(limit)
    {
        tracing::warn!(error = %err, "nothing limits this shutdown");
    }
}

/// Requests a PASSIVE WAL checkpoint periodically, but only after new commits.
async fn checkpoint_loop(store: Store) -> anyhow::Result<()> {
    let mut ticker = tokio::time::interval(CHECKPOINT_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_batches = store.stats().committed_batches;
    loop {
        ticker.tick().await;
        let batches = store.stats().committed_batches;
        if batches != last_batches {
            last_batches = batches;
            store.request_checkpoint();
        }
    }
}

/// Resolves on SIGTERM/SIGINT (Unix) or Ctrl-C / system shutdown (Windows).
async fn termination_signal() -> &'static str {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
        ) {
            (Ok(mut term), Ok(mut int)) => tokio::select! {
                _ = term.recv() => "SIGTERM",
                _ = int.recv() => "SIGINT",
            },
            _ => {
                tracing::warn!("signal handlers unavailable");
                std::future::pending().await
            }
        }
    }
    #[cfg(windows)]
    {
        use tokio::signal::windows::{ctrl_c, ctrl_close, ctrl_shutdown};
        // A detached daemon has no console, so these may be unavailable; IPC quit still works.
        let mut c = ctrl_c().ok();
        let mut close = ctrl_close().ok();
        let mut shutdown = ctrl_shutdown().ok();
        tokio::select! {
            Some(_) = async { match c.as_mut() { Some(s) => s.recv().await, None => std::future::pending().await } } => "Ctrl-C",
            Some(_) = async { match close.as_mut() { Some(s) => s.recv().await, None => std::future::pending().await } } => "console closed",
            Some(_) = async { match shutdown.as_mut() { Some(s) => s.recv().await, None => std::future::pending().await } } => "system shutdown",
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::{BufRead as _, BufReader};
    use std::process::{Child, Command, Stdio};
    use std::time::Instant;

    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    use super::*;

    /// Set in the copy of this test binary that plays a daemon stuck quitting:
    /// `<deadline in ms>,<signals>`.
    const STUCK: &str = "BRIGADIER_TEST_STUCK_SHUTDOWN";
    const LONG_MS: u64 = 600_000;

    /// Never returns. The handler stays installed on a runtime that never runs again, as in a
    /// daemon whose first signal was consumed and whose runtime then stopped.
    fn stuck(limit: &str) -> ! {
        let (deadline, signals) = limit.split_once(',').unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _handler = runtime
            .block_on(async {
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            })
            .unwrap();
        limit_shutdown(
            Duration::from_millis(deadline.parse().unwrap()),
            signals.parse().unwrap(),
        );
        // Time for the limit to start listening.
        std::thread::sleep(Duration::from_millis(300));
        // On a line of its own, whatever the test harness printed before it.
        println!("\nstuck");
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }

    fn start_stuck(deadline_ms: u64, signals: usize) -> Child {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "tests::a_stuck_shutdown_ends_at_its_deadline_or_on_a_second_signal",
                "--exact",
                "--nocapture",
            ])
            .env(STUCK, format!("{deadline_ms},{signals}"))
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let out = BufReader::new(child.stdout.take().unwrap());
        assert!(out.lines().any(|line| line.unwrap() == "stuck"));
        child
    }

    /// Its exit code, once it has exited; `None` when it still runs after `wait`.
    fn exit_within(child: &mut Child, wait: Duration) -> Option<Option<i32>> {
        let until = Instant::now() + wait;
        while Instant::now() < until {
            if let Some(status) = child.try_wait().unwrap() {
                return Some(status.code());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    fn term(child: &Child) {
        let pid = Pid::from_raw(i32::try_from(child.id()).unwrap());
        kill(pid, Signal::SIGTERM).unwrap();
    }

    #[test]
    fn a_stuck_shutdown_ends_at_its_deadline_or_on_a_second_signal() {
        if let Ok(limit) = std::env::var(STUCK) {
            stuck(&limit);
        }
        let fatal = Some(Some(i32::from(EXIT_FATAL)));
        let wait = Duration::from_secs(10);
        let mut outcomes = Vec::new();

        let mut past_deadline = start_stuck(500, 1);
        outcomes.push(("deadline", exit_within(&mut past_deadline, wait)));

        // A quit a signal started: the next signal ends it.
        let mut signalled = start_stuck(LONG_MS, 1);
        term(&signalled);
        outcomes.push(("second signal", exit_within(&mut signalled, wait)));

        // A quit a client asked for: one signal leaves it running, the next ends it.
        let mut asked = start_stuck(LONG_MS, 2);
        term(&asked);
        let after_one = exit_within(&mut asked, Duration::from_millis(500));
        term(&asked);
        outcomes.push(("two signals", exit_within(&mut asked, wait)));

        for mut child in [past_deadline, signalled, asked] {
            let _ = child.kill();
            let _ = child.wait();
        }
        assert_eq!(
            after_one, None,
            "one signal mustn't cut a client's quit short"
        );
        for (case, outcome) in outcomes {
            assert_eq!(outcome, fatal, "{case}");
        }
    }
}
