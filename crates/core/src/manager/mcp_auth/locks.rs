//! Codex's own lock around its file store (docs/research/phase7-probes.md (j)), taken the way
//! Codex takes it, so a read never sees a write half done.

use std::fs::{File, OpenOptions};
use std::path::Path;
use std::time::{Duration, Instant};

use super::keychain::StoreError;

const RETRY: Duration = Duration::from_millis(50);
/// How long a lock is waited for, as both CLIs do.
const WAIT: Duration = Duration::from_secs(60);

/// A held advisory lock on a file, let go when dropped.
pub(crate) struct Held(#[allow(dead_code)] File);

fn failed(what: &str, err: &std::io::Error) -> StoreError {
    StoreError::Failed(format!("{what}: {err}"))
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
            Ok(()) => return Ok(Held(file)),
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
