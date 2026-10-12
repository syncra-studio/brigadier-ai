//! Keeps exactly one healthy helper daemon per home directory.
//!
//! Every MCP shim calls [`ensure`] before forwarding a tool call and again
//! after a transport failure. The steps:
//!
//! 1. Probe the socket with a short deadline. A daemon that answers is used
//!    as is.
//! 2. Otherwise take an exclusive `flock` on `<home>/lifecycle.lock`, so that
//!    many shims recovering at once start one daemon between them, and probe
//!    again: another shim may have finished the recovery while we waited.
//! 3. A daemon that accepts but does not answer is hung. Stop it, but only
//!    after checking that the recorded PID still names our helper binary and
//!    that the process started before the PID file was written, so a reused
//!    PID is never signalled.
//! 4. Launch a fresh daemon through LaunchServices and wait for it to answer.
//!
//! The seams later stages build on: [`probe`] (a watchdog can call it on a
//! timer), [`stop_recorded_daemon`] (a version or permission change can force
//! a replacement), and [`ensure`] itself.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::serve::{serialize_request, DaemonRequest};

/// How long a probe may wait for the daemon to answer a `list` request.
const PROBE_DEADLINE: Duration = Duration::from_secs(3);
/// How long a launched daemon may take to answer its first probe.
const LAUNCH_DEADLINE: Duration = Duration::from_secs(15);
/// How long a recorded daemon without a socket gets before it counts as hung.
const BIND_GRACE: Duration = Duration::from_secs(3);

/// What a single probe of the daemon socket found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    /// The daemon answered a `list` request in time.
    Healthy,
    /// Nothing accepts connections on the socket.
    Absent,
    /// Something accepts connections but did not answer in time.
    Hung,
}

/// Probe the daemon socket once, never waiting longer than [`PROBE_DEADLINE`].
pub fn probe(socket_path: &str) -> Health {
    let Ok(mut stream) = UnixStream::connect(socket_path) else {
        return Health::Absent;
    };
    let request = DaemonRequest {
        method: "list".into(),
        name: None,
        args: None,
        session_id: None,
    };
    let Ok(line) = serialize_request(&request) else {
        return Health::Hung;
    };
    let deadline = Instant::now() + PROBE_DEADLINE;
    if stream.set_write_timeout(Some(PROBE_DEADLINE)).is_err()
        || stream.write_all(format!("{line}\n").as_bytes()).is_err()
    {
        return Health::Hung;
    }
    let mut response = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || stream.set_read_timeout(Some(remaining)).is_err() {
            return Health::Hung;
        }
        match stream.read(&mut chunk) {
            Ok(0) => return Health::Hung,
            Ok(n) => {
                response.extend_from_slice(&chunk[..n]);
                if let Some(end) = response.iter().position(|&b| b == b'\n') {
                    let answered_ok = serde_json::from_slice::<serde_json::Value>(&response[..end])
                        .ok()
                        .and_then(|v| v.get("ok").and_then(serde_json::Value::as_bool))
                        .unwrap_or(false);
                    return if answered_ok {
                        Health::Healthy
                    } else {
                        Health::Hung
                    };
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Health::Hung,
        }
    }
}

/// Make sure a healthy daemon answers on `socket_path`, starting or replacing
/// it if needed. Blocking; call it from a blocking context.
pub fn ensure(socket_path: &str, claude_code_compat: bool) -> anyhow::Result<()> {
    if probe(socket_path) == Health::Healthy {
        return Ok(());
    }
    let _lock = LifecycleLock::acquire(&lock_path(socket_path))?;
    let pid_file = crate::cli::daemon_pid_file_path(Some(socket_path));
    match probe(socket_path) {
        Health::Healthy => return Ok(()),
        Health::Hung => {
            crate::journal::record_event("helper_recovery", "daemon accepted but did not answer");
            stop_recorded_daemon(&pid_file);
        }
        Health::Absent => {
            // A recorded daemon that is alive but has no socket is either
            // just starting or stuck before bind. Give it a moment, then
            // treat it as hung.
            if recorded_daemon(&pid_file).is_some() {
                let until = Instant::now() + BIND_GRACE;
                while Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(100));
                    if probe(socket_path) == Health::Healthy {
                        return Ok(());
                    }
                }
                crate::journal::record_event("helper_recovery", "daemon running without a socket");
                stop_recorded_daemon(&pid_file);
            }
        }
    }
    crate::cli::launch_daemon(socket_path, claude_code_compat)?;
    let until = Instant::now() + LAUNCH_DEADLINE;
    while Instant::now() < until {
        if probe(socket_path) == Health::Healthy {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!(
        "Brigadier Computer Use did not start within {}s",
        LAUNCH_DEADLINE.as_secs()
    )
}

/// The lock file guarding daemon start and restart for this socket.
fn lock_path(socket_path: &str) -> PathBuf {
    Path::new(socket_path)
        .parent()
        .map(|dir| dir.join("lifecycle.lock"))
        .unwrap_or_else(|| PathBuf::from("lifecycle.lock"))
}

/// An exclusive advisory lock, released when dropped (or when the process
/// dies, since the kernel closes the descriptor).
struct LifecycleLock(std::fs::File);

impl LifecycleLock {
    fn acquire(path: &Path) -> anyhow::Result<Self> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;

        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| anyhow::anyhow!("open {}: {e}", path.display()))?;
        // Bounded wait: a holder is launching a daemon, which finishes within
        // LAUNCH_DEADLINE plus the stop time. Never block forever.
        let until = Instant::now() + LAUNCH_DEADLINE + Duration::from_secs(10);
        loop {
            let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if rc == 0 {
                return Ok(Self(file));
            }
            if Instant::now() >= until {
                anyhow::bail!("timed out waiting for another Computer Use start to finish");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for LifecycleLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// The PID in `pid_file` when it still names our helper daemon: its
/// executable is a `brigadier-cu` binary inside the helper app, and it
/// started no later than the PID file was written. A PID the kernel has
/// handed to an unrelated process fails one of those checks.
fn recorded_daemon(pid_file: &str) -> Option<u32> {
    let pid = crate::serve::read_pid_file(pid_file)?;
    if pid <= 1 || pid == std::process::id() {
        return None;
    }
    let written = std::fs::metadata(pid_file).ok()?.modified().ok()?;
    let path = process_executable(pid)?;
    if !path.ends_with("Brigadier Computer Use.app/Contents/MacOS/brigadier-cu") {
        return None;
    }
    let started = process_start_time(pid)?;
    // One second of slack covers the clock granularity of file times.
    if started > written + Duration::from_secs(1) {
        return None;
    }
    Some(pid)
}

/// Stop the daemon recorded in `pid_file`, if it is still ours. Sends
/// SIGTERM, then SIGKILL after a short grace period, and waits for exit.
pub fn stop_recorded_daemon(pid_file: &str) {
    let Some(pid) = recorded_daemon(pid_file) else {
        return;
    };
    let Ok(raw) = libc::pid_t::try_from(pid) else {
        return;
    };
    unsafe { libc::kill(raw, libc::SIGTERM) };
    let gone = |deadline: Duration| {
        let until = Instant::now() + deadline;
        while Instant::now() < until {
            if unsafe { libc::kill(raw, 0) } != 0 {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    };
    if !gone(Duration::from_secs(1)) {
        unsafe { libc::kill(raw, libc::SIGKILL) };
        let _ = gone(Duration::from_secs(2));
    }
}

fn process_executable(pid: u32) -> Option<PathBuf> {
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let written = unsafe {
        libc::proc_pidpath(
            i32::try_from(pid).ok()?,
            buffer.as_mut_ptr().cast(),
            u32::try_from(buffer.len()).ok()?,
        )
    };
    if written <= 0 {
        return None;
    }
    buffer.truncate(usize::try_from(written).ok()?);
    Some(PathBuf::from(String::from_utf8(buffer).ok()?))
}

fn process_start_time(pid: u32) -> Option<SystemTime> {
    let identity = crate::serve::macos_process_identity(pid)?;
    let seconds = u64::try_from(identity.start_seconds).ok()?;
    let micros = u64::try_from(identity.start_microseconds).ok()?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds) + Duration::from_micros(micros))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_socket_is_absent() {
        let dir = std::env::temp_dir().join(format!("cu-supervisor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("none.sock");
        assert_eq!(probe(socket.to_str().unwrap()), Health::Absent);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_silent_listener_is_hung_within_the_deadline() {
        let dir = std::env::temp_dir().join(format!("cu-supervisor-hung-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("hung.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let started = Instant::now();
        assert_eq!(probe(socket.to_str().unwrap()), Health::Hung);
        assert!(started.elapsed() < PROBE_DEADLINE + Duration::from_secs(1));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_pid_file_naming_another_program_is_never_ours() {
        let dir = std::env::temp_dir().join(format!("cu-supervisor-pid-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pid_file = dir.join("cu.pid");
        // The parent of the test runner is a live process that is not the helper.
        let parent = unsafe { libc::getppid() };
        std::fs::write(&pid_file, parent.to_string()).unwrap();
        assert_eq!(recorded_daemon(pid_file.to_str().unwrap()), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_lifecycle_lock_is_exclusive_across_descriptors() {
        let dir = std::env::temp_dir().join(format!("cu-supervisor-lock-{}", std::process::id()));
        let path = dir.join("lifecycle.lock");
        let held = LifecycleLock::acquire(&path).unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        use std::os::fd::AsRawFd;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert_ne!(rc, 0);
        drop(held);
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert_eq!(rc, 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
