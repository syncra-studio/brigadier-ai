//! The CLIs' own locks around a sign-in's renewal and their stores' writes
//! (docs/research/phase7-probes.md (j)), taken the way each CLI takes them so that Brigadier and
//! the CLI never renew one sign-in at once.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use super::keychain::StoreError;

const RETRY: Duration = Duration::from_millis(50);
/// How long a lock is waited for, as both CLIs do.
const WAIT: Duration = Duration::from_secs(60);

/// A held lock, let go when dropped.
pub(crate) enum Held {
    /// A folder Claude Code's `proper-lockfile` makes, kept fresh while held.
    Folder {
        path: PathBuf,
        held: Arc<AtomicBool>,
    },
    /// An advisory lock on a file (Codex).
    File(#[allow(dead_code)] File),
}

impl Drop for Held {
    fn drop(&mut self) {
        if let Self::Folder { path, held } = self {
            held.store(false, Ordering::SeqCst);
            let _ = std::fs::remove_dir(path);
        }
    }
}

fn failed(what: &str, err: &std::io::Error) -> StoreError {
    StoreError::Failed(format!("{what}: {err}"))
}

/// Claude Code's lock on `file` (`proper-lockfile`, `realpath: false`): the folder
/// `<file>.lock`, taken over once its time is older than `stale`, its time renewed every 5 s
/// while held.
pub(crate) fn claude(file: &Path, stale: Duration) -> Result<Held, StoreError> {
    let mut path = file.as_os_str().to_owned();
    path.push(".lock");
    let path = PathBuf::from(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| failed("Claude's lock folder", &err))?;
    }
    let started = Instant::now();
    loop {
        match std::fs::create_dir(&path) {
            Ok(()) => break,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                let old = std::fs::metadata(&path)
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|time| SystemTime::now().duration_since(time).ok());
                if old.is_some_and(|old| old > stale) {
                    let _ = std::fs::remove_dir(&path);
                    continue;
                }
                if started.elapsed() > WAIT {
                    return Err(StoreError::Failed("Claude Code's lock stayed taken".into()));
                }
                std::thread::sleep(RETRY);
            }
            Err(err) => return Err(failed("Claude's lock", &err)),
        }
    }
    let held = Arc::new(AtomicBool::new(true));
    {
        let (path, held) = (path.clone(), held.clone());
        std::thread::spawn(move || {
            while held.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(250));
                let fresh = std::fs::metadata(&path)
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|time| SystemTime::now().duration_since(time).ok())
                    .is_some_and(|age| age < Duration::from_secs(5));
                if !fresh && held.load(Ordering::SeqCst) {
                    let _ = File::open(&path).and_then(|dir| dir.set_modified(SystemTime::now()));
                }
            }
        });
    }
    Ok(Held::Folder { path, held })
}

/// Codex's lock: an advisory lock on `path` (made if missing), `shared` for a read of its
/// file store as Codex reads it.
pub(crate) fn codex(path: &Path, shared: bool) -> Result<Held, StoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| failed("Codex's lock folder", &err))?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|err| failed("Codex's lock", &err))?;
    let started = Instant::now();
    loop {
        let tried = if shared {
            file.try_lock_shared()
        } else {
            file.try_lock()
        };
        match tried {
            Ok(()) => return Ok(Held::File(file)),
            Err(std::fs::TryLockError::WouldBlock) if started.elapsed() < WAIT => {
                std::thread::sleep(RETRY);
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(StoreError::Failed("Codex's lock stayed taken".into()));
            }
            Err(std::fs::TryLockError::Error(err)) => return Err(failed("Codex's lock", &err)),
        }
    }
}
