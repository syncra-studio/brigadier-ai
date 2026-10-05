//! Settings → Storage, the daemon's side: it runs the session manager's scan, adds what only
//! the daemon can see (other data directories' daemons, stale connection sockets), keeps each
//! scan's items (bound to what was found) for a while, and removes the items the app picks by
//! id. The app never names a path.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(unix)]
use brigadier_core::manager::disk::counted;
use brigadier_core::manager::disk::{Action, ScanContext, ScanItem};
use brigadier_core::storage::{CleanCategory, CleanFailure, CleanItem, CleanReport, StorageReport};
use brigadier_ipc::protocol::{ClientFrame, ClientInfo, Outcome, Request, Response, ServerFrame};
use brigadier_sandbox::AppPaths;

use tokio_util::sync::CancellationToken;

use crate::server::Daemon;

/// How long a scan's items can be cleaned.
const SCAN_LIFETIME: Duration = Duration::from_secs(15 * 60);
/// How long another daemon gets to answer.
const ASK_TIMEOUT: Duration = Duration::from_secs(3);
/// How long another daemon gets to quit.
const QUIT_TIMEOUT: Duration = Duration::from_secs(30);
/// Housekeeping's first run after the daemon starts, once launch-time work has settled.
const HOUSEKEEPING_DELAY: Duration = Duration::from_secs(2 * 60);
/// And then once a day.
const HOUSEKEEPING_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// The database compacts on its own once nothing has run or been written for this long.
const COMPACT_QUIET: Duration = Duration::from_secs(2 * 60);
/// Its first look after the daemon starts, once launch-time work has settled.
const COMPACT_DELAY: Duration = Duration::from_secs(2 * 60);
/// How often it looks (and right after a delete has finished).
const COMPACT_LOOK_EVERY: Duration = Duration::from_secs(30);
/// A connection folder younger than this may belong to a daemon about to listen.
#[cfg(unix)]
const SOCKET_MIN_AGE: Duration = Duration::from_secs(10 * 60);

/// What the daemon removes itself.
#[derive(Debug, Clone)]
enum DaemonAction {
    /// Another data directory's daemon, found unused: asked to quit, SIGTERM only if asking
    /// fails.
    QuitDaemon {
        data_dir: PathBuf,
        pid: u32,
        started_at: u64,
    },
    /// Shown only.
    None,
}

struct Entry {
    item: CleanItem,
    action: Action,
    own: Option<DaemonAction>,
}

struct Scan {
    at: Instant,
    entries: HashMap<String, Entry>,
}

#[derive(Default)]
pub struct Storage {
    scans: Mutex<HashMap<String, Scan>>,
    /// One clean at a time.
    cleaning: tokio::sync::Mutex<()>,
}

impl Storage {
    fn scans(&self) -> std::sync::MutexGuard<'_, HashMap<String, Scan>> {
        self.scans.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub async fn scan(&self, daemon: &Daemon) -> Result<StorageReport, String> {
        let (mut items, usage) = daemon
            .sessions
            .scan_storage(context(daemon))
            .await
            .map_err(|err| err.to_string())?;
        let mut own = HashMap::new();
        let paths = daemon.runtime.platform().paths().clone();
        let found =
            tokio::task::spawn_blocking(move || (stale_sockets(&paths), other_daemons(&paths)))
                .await
                .map_err(|err| err.to_string())?;
        let (sockets, daemons) = found;
        items.extend(sockets);
        for (item, action) in ask_daemons(daemons).await {
            let key = format!("daemon-{}", own.len());
            own.insert(key.clone(), action);
            items.push(ScanItem {
                item,
                action: Action::External(key),
                routine: false,
            });
        }
        let scan_id = uuid::Uuid::now_v7().to_string();
        let mut entries = HashMap::new();
        let mut listed = Vec::new();
        for (
            index,
            ScanItem {
                mut item, action, ..
            },
        ) in items.into_iter().enumerate()
        {
            item.id = format!("item-{index}");
            let own = match &action {
                Action::External(key) => Some(own.get(key).cloned().unwrap_or(DaemonAction::None)),
                _ => None,
            };
            listed.push(item.clone());
            entries.insert(item.id.clone(), Entry { item, action, own });
        }
        let cleanable_bytes = listed
            .iter()
            .filter(|item| item.checked && item.selectable)
            .map(|item| item.bytes)
            .sum();
        {
            let mut scans = self.scans();
            scans.retain(|_, scan| scan.at.elapsed() < SCAN_LIFETIME);
            scans.insert(
                scan_id.clone(),
                Scan {
                    at: Instant::now(),
                    entries,
                },
            );
        }
        Ok(StorageReport {
            scan_id,
            data_dir: daemon.info.data_dir.clone(),
            total_bytes: usage.total_bytes,
            cleanable_bytes,
            projects: usage.projects,
            shared: usage.shared,
            items: listed,
        })
    }

    /// Removes the picked items of a scan. Each is checked again first; what fails is
    /// reported and left in place.
    pub async fn clean(
        &self,
        daemon: &Daemon,
        scan_id: &str,
        picked: Vec<String>,
    ) -> Result<CleanReport, String> {
        let _one = self.cleaning.lock().await;
        let mut entries: Vec<Entry> = {
            let mut scans = self.scans();
            let scan = scans
                .get_mut(scan_id)
                .filter(|scan| scan.at.elapsed() < SCAN_LIFETIME)
                .ok_or("This scan is too old; scan again.")?;
            picked
                .iter()
                .filter_map(|id| scan.entries.remove(id))
                .filter(|entry| entry.item.selectable)
                .collect()
        };
        entries.sort_by_key(|entry| rank(&entry.action));
        let mut report = CleanReport::default();
        for entry in entries {
            let outcome = match (&entry.action, entry.own) {
                (
                    Action::External(_),
                    Some(DaemonAction::QuitDaemon {
                        data_dir,
                        pid,
                        started_at,
                    }),
                ) => quit_daemon(&data_dir, pid, started_at)
                    .await
                    .map(|()| Default::default()),
                (Action::External(_), _) => Err("This item can't be removed from here.".into()),
                (action, _) => {
                    daemon
                        .sessions
                        .clean_storage(action.clone(), context(daemon))
                        .await
                }
            };
            match outcome {
                Ok(cleaned) => {
                    report.reclaimed_bytes += cleaned.reclaimed;
                    report.trashed_bytes += cleaned.trashed;
                    if cleaned.failures.is_empty() {
                        report.removed += 1;
                        tracing::info!(item = %entry.item.label, "cleaned");
                    } else {
                        let error = cleaned.failures.join("; ");
                        tracing::warn!(item = %entry.item.label, %error, "cleaned only in part");
                        report.failures.push(CleanFailure {
                            label: entry.item.label,
                            path: entry.item.path,
                            error,
                        });
                    }
                }
                Err(error) => {
                    tracing::warn!(item = %entry.item.label, %error, "could not clean");
                    report.failures.push(CleanFailure {
                        label: entry.item.label,
                        path: entry.item.path,
                        error,
                    });
                }
            }
        }
        Ok(report)
    }
}

/// Removes on its own, shortly after the daemon starts and then daily, what is clearly left
/// over, purely Brigadier's and rebuildable: empty worktree folders, this data directory's
/// session temp folders untouched for a day with nothing working in them, connection folders
/// nobody listens on. Never user data, never git branches, never anything with changes.
pub async fn housekeeping(daemon: Arc<Daemon>, stop: CancellationToken) -> anyhow::Result<()> {
    let mut wait = HOUSEKEEPING_DELAY;
    loop {
        tokio::select! {
            () = stop.cancelled() => return Ok(()),
            () = tokio::time::sleep(wait) => {}
        }
        wait = HOUSEKEEPING_EVERY;
        if daemon.uninstall.started() {
            return Ok(());
        }
        daemon.storage.housekeep(&daemon).await;
    }
}

/// Compacts the database on its own (PLAN.md §2, Delete): once Brigadier has been quiet for
/// [`COMPACT_QUIET`] (nothing works, nothing was written) and that gives back enough space.
/// Never in the daemon's first minutes, and never once it is stopping.
pub async fn compact_when_quiet(
    daemon: Arc<Daemon>,
    stop: CancellationToken,
) -> anyhow::Result<()> {
    let (delay, quiet) = compact_timing();
    tokio::select! {
        () = stop.cancelled() => return Ok(()),
        () = tokio::time::sleep(delay) => {}
    }
    let mut tick = tokio::time::interval(COMPACT_LOOK_EVERY.min(quiet));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The generation the quiet period started at, and when.
    let mut quiet_since: Option<(u64, Instant)> = None;
    loop {
        tokio::select! {
            () = stop.cancelled() => return Ok(()),
            _ = tick.tick() => {}
            () = daemon.sessions.space_freed() => {}
        }
        if daemon.uninstall.started() {
            return Ok(());
        }
        if daemon.sessions.busy_for_maintenance().await.is_some() {
            quiet_since = None;
            continue;
        }
        let generation = daemon.sessions.maintenance_generation();
        let since = match quiet_since {
            Some((at, since)) if at == generation => since,
            _ => {
                quiet_since = Some((generation, Instant::now()));
                continue;
            }
        };
        if since.elapsed() < quiet {
            continue;
        }
        let _one = daemon.storage.cleaning.lock().await;
        if stop.is_cancelled() {
            return Ok(());
        }
        daemon.sessions.compact_when_quiet(generation).await;
        quiet_since = None;
    }
}

/// [`COMPACT_DELAY`] and [`COMPACT_QUIET`], or in a debug build both `BRIGADIER_COMPACT_QUIET_SECS`
/// (to see it without waiting minutes).
fn compact_timing() -> (Duration, Duration) {
    #[cfg(debug_assertions)]
    if let Some(secs) = std::env::var("BRIGADIER_COMPACT_QUIET_SECS")
        .ok()
        .and_then(|secs| secs.parse::<u64>().ok())
    {
        let secs = Duration::from_secs(secs.max(1));
        return (secs, secs);
    }
    (COMPACT_DELAY, COMPACT_QUIET)
}

impl Storage {
    async fn housekeep(&self, daemon: &Daemon) {
        let _one = self.cleaning.lock().await;
        let items = match daemon.sessions.scan_storage(context(daemon)).await {
            Ok((items, _)) => items,
            Err(err) => {
                tracing::warn!(error = %err, "housekeeping could not look around");
                return;
            }
        };
        let paths = daemon.runtime.platform().paths().clone();
        let sockets = tokio::task::spawn_blocking(move || stale_sockets(&paths))
            .await
            .unwrap_or_default();
        let mut removed = 0;
        for ScanItem { item, action, .. } in
            items.into_iter().chain(sockets).filter(|item| item.routine)
        {
            match daemon.sessions.clean_storage(action, context(daemon)).await {
                Ok(cleaned) if !cleaned.failures.is_empty() => {
                    let error = cleaned.failures.join("; ");
                    tracing::debug!(item = %item.label, %error, "housekeeping left part of an item");
                }
                Ok(_) => {
                    removed += 1;
                    tracing::info!(item = %item.label, path = item.path.as_deref().unwrap_or(""), "housekeeping removed a leftover");
                }
                Err(error) => {
                    tracing::debug!(item = %item.label, %error, "housekeeping left an item");
                }
            }
        }
        tracing::info!(removed, "housekeeping done");
    }
}

/// The order removals run in: processes first (nothing then writes what goes next), then what
/// the ledger holds, worktrees before their records and branches, blobs and the database last.
/// What the daemon knows now that the session manager doesn't.
fn context(daemon: &Daemon) -> ScanContext {
    ScanContext {
        speech_busy: !daemon.dictation.work().is_empty(),
    }
}

fn rank(action: &Action) -> u8 {
    match action {
        Action::External(_) => 0,
        Action::Dispose { .. } => 1,
        Action::RemoveWorktree { .. } => 2,
        Action::PruneWorktrees { .. } => 3,
        Action::DeleteBranch { .. } => 4,
        Action::Delete(_)
        | Action::Trash(_)
        | Action::RemoveEmptyDir(_)
        | Action::DeleteModel { .. } => 5,
        Action::CollectBlobs => 6,
        Action::Compact => 7,
    }
}

fn plain_item(
    category: CleanCategory,
    label: String,
    path: Option<&Path>,
    reason: String,
    checked: bool,
    selectable: bool,
) -> CleanItem {
    CleanItem {
        id: String::new(),
        category,
        label,
        path: path.map(|path| path.display().to_string()),
        bytes: 0,
        reason,
        checked,
        selectable,
        to_trash: false,
        badges: Vec::new(),
    }
}

/// Connection folders other data directories' daemons left (`<temp>/brigadier-<uid>-<id>`,
/// used when a data directory's path is too long for a socket) with nobody listening.
#[cfg(unix)]
fn stale_sockets(paths: &AppPaths) -> Vec<ScanItem> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let uid = nix::unistd::getuid().as_raw();
    let temp = std::env::temp_dir();
    let prefix = format!("brigadier-{uid}-");
    let own = paths.socket_dir().map(Path::to_owned);
    let Ok(entries) = std::fs::read_dir(&temp) else {
        return Vec::new();
    };
    let mut stale = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_ours = name
            .strip_prefix(&prefix)
            .is_some_and(|id| id.len() == 12 && id.chars().all(|c| c.is_ascii_hexdigit()));
        let path = temp.join(&name);
        if !is_ours || own.as_deref() == Some(path.as_path()) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.is_dir() || meta.uid() != uid || meta.permissions().mode() & 0o077 != 0 {
            continue;
        }
        // A daemon makes its folder a moment before it listens; only a folder nobody touched
        // for a while is left over.
        let settled = meta
            .modified()
            .ok()
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age >= SOCKET_MIN_AGE);
        if !settled {
            continue;
        }
        let socket = path.join("d.sock");
        let listening = std::os::unix::net::UnixStream::connect(&socket).is_ok();
        if listening {
            continue;
        }
        if let Ok(bound) = brigadier_sandbox::removal::bind(&temp, &path) {
            stale.push((bound, brigadier_sandbox::removal::allocated_size(&path)));
        }
    }
    if stale.is_empty() {
        return Vec::new();
    }
    let item = plain_item(
        CleanCategory::Processes,
        counted(
            stale.len(),
            "stale connection folder",
            "stale connection folders",
        ),
        Some(&temp),
        "Left by Brigadier daemons that are no longer running; nothing listens on them.".into(),
        true,
        true,
    );
    vec![ScanItem {
        item,
        action: Action::Delete(stale),
        routine: true,
    }]
}

#[cfg(not(unix))]
fn stale_sockets(_paths: &AppPaths) -> Vec<ScanItem> {
    Vec::new()
}

/// Another data directory's daemon.
struct OtherDaemon {
    pid: u32,
    started_at: u64,
    data_dir: Option<PathBuf>,
}

/// This user's other `brigadierd` daemons (not their helpers), with their data directories.
fn other_daemons(paths: &AppPaths) -> Vec<OtherDaemon> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_user(UpdateKind::Always),
    );
    let me = std::process::id();
    let my_user = system
        .process(sysinfo::Pid::from_u32(me))
        .and_then(|process| process.user_id().cloned());
    let mut found = Vec::new();
    for (pid, process) in system.processes() {
        let pid = pid.as_u32();
        if pid == me || process.user_id().cloned() != my_user {
            continue;
        }
        let cmd = process.cmd();
        let is_daemon =
            cmd.first().and_then(|exe| Path::new(exe).file_name()) == Some("brigadierd".as_ref());
        // Its helpers (`mcp`, `transcribe`, `index-scan`, `quit`) take a subcommand first.
        let helper = cmd
            .get(1)
            .is_some_and(|arg| !arg.to_string_lossy().starts_with("--"));
        if !is_daemon || helper {
            continue;
        }
        let data_dir = cmd
            .iter()
            .position(|arg| arg == "--data-dir")
            .and_then(|at| cmd.get(at + 1))
            .map(PathBuf::from);
        if data_dir.as_deref() == Some(paths.data_dir.as_path()) {
            continue;
        }
        found.push(OtherDaemon {
            pid,
            started_at: process.start_time(),
            data_dir,
        });
    }
    found
}

/// Asks each other daemon whether it is in use: only an unused one is offered.
async fn ask_daemons(daemons: Vec<OtherDaemon>) -> Vec<(CleanItem, DaemonAction)> {
    let mut items = Vec::new();
    for daemon in daemons {
        let label = match &daemon.data_dir {
            Some(dir) => format!(
                "Brigadier daemon for {} (pid {})",
                dir.display(),
                daemon.pid
            ),
            None => format!("Brigadier daemon (pid {})", daemon.pid),
        };
        let path = daemon.data_dir.clone();
        let Some(data_dir) = daemon.data_dir else {
            items.push((
                plain_item(
                    CleanCategory::Processes,
                    label,
                    None,
                    "It doesn't say which data directory it serves, so it can't be asked \
                     whether it is in use."
                        .into(),
                    false,
                    false,
                ),
                DaemonAction::None,
            ));
            continue;
        };
        let (reason, unused) = match activity(&data_dir).await {
            Ok(activity) if activity.clients > 0 => {
                ("An app is connected to it.".to_owned(), false)
            }
            Ok(activity) if !activity.running.is_empty() => (
                format!("It is working: {}.", activity.running.join(", ")),
                false,
            ),
            Ok(_) => (
                "No app is connected and nothing runs in it. It is asked to quit the way the \
                 app's Quit does."
                    .to_owned(),
                true,
            ),
            Err(err) => {
                tracing::info!(
                    data_dir = %data_dir.display(),
                    error = %err,
                    "another daemon couldn't be asked whether it is in use"
                );
                (
                    "It didn't answer whether it is in use (it may belong to another copy of \
                     Brigadier), so it is left alone."
                        .to_owned(),
                    false,
                )
            }
        };
        items.push((
            plain_item(
                CleanCategory::Processes,
                label,
                path.as_deref(),
                reason,
                false,
                unused,
            ),
            if unused {
                DaemonAction::QuitDaemon {
                    data_dir,
                    pid: daemon.pid,
                    started_at: daemon.started_at,
                }
            } else {
                DaemonAction::None
            },
        ));
    }
    items
}

/// Connects to `data_dir`'s daemon with its own token.
async fn connect(data_dir: &Path) -> Result<brigadier_ipc::Connection, String> {
    let paths = AppPaths::resolve(data_dir.to_owned()).map_err(|err| err.to_string())?;
    let client = ClientInfo {
        name: "Brigadier storage".into(),
        pid: std::process::id(),
    };
    let (connection, _, _) =
        tokio::time::timeout(ASK_TIMEOUT, brigadier_ipc::connect_to(&paths, client))
            .await
            .map_err(|_| "it did not answer".to_owned())?
            .map_err(|err| err.to_string())?;
    Ok(connection)
}

async fn ask(
    connection: &mut brigadier_ipc::Connection,
    request: Request,
    timeout: Duration,
) -> Result<Option<Response>, String> {
    connection
        .writer
        .write(&ClientFrame::Request { id: 1, request })
        .await
        .map_err(|err| err.to_string())?;
    tokio::time::timeout(timeout, async {
        loop {
            match connection.reader.read::<ServerFrame>().await {
                Ok(Some(ServerFrame::Response { id: 1, result })) => {
                    return match result {
                        Outcome::Ok { value } => Ok(Some(value)),
                        Outcome::Err { error } => Err(error.message),
                    };
                }
                Ok(Some(ServerFrame::Closing) | None) => return Ok(None),
                Ok(Some(_)) => {}
                Err(err) => return Err(err.to_string()),
            }
        }
    })
    .await
    .map_err(|_| "it did not answer".to_owned())?
}

async fn activity(data_dir: &Path) -> Result<brigadier_ipc::protocol::DaemonActivity, String> {
    let mut connection = connect(data_dir).await?;
    match ask(&mut connection, Request::GetDaemonActivity, ASK_TIMEOUT).await? {
        Some(Response::GetDaemonActivity { activity }) => Ok(activity),
        _ => Err("it gave no answer".into()),
    }
}

/// Asks an unused daemon to quit, checking again that it is unused; SIGTERM (which quits it the
/// same orderly way) only when it can't be asked, and only while its pid is still that process.
async fn quit_daemon(data_dir: &Path, pid: u32, started_at: u64) -> Result<(), String> {
    let same_process = move || {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
        let mut system = System::new();
        let sys_pid = sysinfo::Pid::from_u32(pid);
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[sys_pid]),
            true,
            ProcessRefreshKind::nothing(),
        );
        system
            .process(sys_pid)
            .is_some_and(|process| process.start_time() == started_at)
    };
    let asked = async {
        let activity = activity(data_dir).await?;
        if activity.clients > 0 || !activity.running.is_empty() {
            return Err::<bool, String>("it is in use again, so it stays".into());
        }
        let mut connection = connect(data_dir).await?;
        ask(&mut connection, Request::Shutdown, QUIT_TIMEOUT).await?;
        Ok(true)
    }
    .await;
    match asked {
        Ok(_) => {}
        Err(err) if err.contains("in use") => return Err(err),
        Err(err) => {
            tracing::info!(pid, error = %err, "asking a daemon to quit failed; sending SIGTERM");
            if !same_process() {
                return Ok(());
            }
            #[cfg(unix)]
            nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(i32::try_from(pid).map_err(|err| err.to_string())?),
                nix::sys::signal::Signal::SIGTERM,
            )
            .map_err(|err| err.to_string())?;
            #[cfg(not(unix))]
            return Err(format!("it could not be asked to quit: {err}"));
        }
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while same_process() {
        if Instant::now() > deadline {
            return Err("it is still running".into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Ok(())
}
