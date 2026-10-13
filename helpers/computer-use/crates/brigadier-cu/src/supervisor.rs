//! Keeps exactly one healthy helper daemon per home directory.
//!
//! Every MCP shim calls [`ensure`] before forwarding a tool call and again
//! after a transport failure. The steps:
//!
//! 1. Probe the socket with a short deadline. A daemon that answers, from
//!    the build a fresh one would launch from, is used as is.
//! 2. Otherwise take an exclusive `flock` on `<home>/lifecycle.lock`, so that
//!    many shims recovering at once start one daemon between them, and probe
//!    again: another shim may have finished the recovery while we waited.
//! 3. A daemon that accepts but does not answer is hung. Stop it, but only
//!    after checking that the recorded PID still names our helper binary and
//!    that the process started before the PID file was written, so a reused
//!    PID is never signalled.
//! 4. Launch a fresh daemon through LaunchServices and wait for it to answer.
//!
//! A healthy daemon from another build (the app was updated while it ran)
//! is replaced the same way. Brigadier's watchdog uses [`probe`] and
//! [`restart`]; turning computer use off goes through [`set_off`], under the
//! same lock, so no shim starts a daemon while it is off.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::serve::{serialize_request, DaemonRequest};

/// What an agent is told when the running helper won't serve its process.
pub const REFUSED_MESSAGE: &str = "Brigadier Computer Use refused this agent's connection: the \
     program the agent reached it through isn't signed as part of Brigadier. Start the agent \
     again from Brigadier";

/// How long a probe may wait for the daemon to answer a `list` request.
const PROBE_DEADLINE: Duration = Duration::from_secs(3);
/// How long a launched daemon may take to answer its first probe.
const LAUNCH_DEADLINE: Duration = Duration::from_secs(15);
/// How long a recorded daemon without a socket gets before it counts as hung.
const BIND_GRACE: Duration = Duration::from_secs(3);

/// What a single probe of the daemon socket found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    /// The daemon answered a `health` request in time, reporting the build
    /// it started from.
    Healthy { build_id: Option<String> },
    /// Nothing accepts connections on the socket.
    Absent,
    /// Something accepts connections but did not answer in time, or its main
    /// loop has stopped running.
    Hung,
    /// The daemon answered that it won't serve this process. It is healthy
    /// for everyone else, so it is never stopped for this.
    Refused,
}

impl Health {
    pub fn is_healthy(&self) -> bool {
        matches!(self, Self::Healthy { .. })
    }
}

/// Probe the daemon socket once, never waiting longer than [`PROBE_DEADLINE`].
pub fn probe(socket_path: &str) -> Health {
    let Ok(mut stream) = UnixStream::connect(socket_path) else {
        return Health::Absent;
    };
    let request = DaemonRequest {
        method: "health".into(),
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
    let mut response = Vec::with_capacity(512);
    let mut chunk = [0u8; 512];
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
                    return health_from_answer(&response[..end]);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Health::Hung,
        }
    }
}

/// A `health` answer: healthy with its build id, or hung when it says no (a
/// silent main loop) or isn't an answer at all.
fn health_from_answer(line: &[u8]) -> Health {
    let Ok(answer) = serde_json::from_slice::<serde_json::Value>(line) else {
        return Health::Hung;
    };
    if answer.get("refused").and_then(serde_json::Value::as_bool) == Some(true) {
        return Health::Refused;
    }
    if answer.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Health::Hung;
    }
    Health::Healthy {
        build_id: answer
            .pointer("/result/build_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    }
}

/// Whether a healthy daemon reporting `build_id` is from another build than
/// the bundle a fresh one would launch from.
fn from_another_build(build_id: Option<&str>) -> bool {
    let on_disk = crate::lifecycle::launch_bundle()
        .as_deref()
        .and_then(crate::lifecycle::bundle_build_id);
    crate::lifecycle::needs_replacement(build_id, on_disk.as_deref())
}

/// Whether the daemon on `socket_path` is healthy and from the right build.
pub fn is_current(socket_path: &str) -> bool {
    match probe(socket_path) {
        Health::Healthy { build_id } => !from_another_build(build_id.as_deref()),
        _ => false,
    }
}

/// Make sure a healthy daemon from the right build answers on `socket_path`,
/// starting or replacing it if needed. Blocking; call it from a blocking
/// context. Refuses while computer use is turned off.
pub fn ensure(socket_path: &str, claude_code_compat: bool) -> anyhow::Result<()> {
    let off = || crate::lifecycle::is_off(&crate::lifecycle::home());
    if off() {
        anyhow::bail!(crate::lifecycle::OFF_MESSAGE);
    }
    if is_current(socket_path) {
        return Ok(());
    }
    let _lock = LifecycleLock::acquire(&lock_path(socket_path))?;
    // Turning off takes the same lock, so this holds until it is turned on.
    if off() {
        anyhow::bail!(crate::lifecycle::OFF_MESSAGE);
    }
    let pid_file = crate::cli::daemon_pid_file_path(Some(socket_path));
    match probe(socket_path) {
        Health::Healthy { build_id } => {
            if !from_another_build(build_id.as_deref()) {
                return Ok(());
            }
            crate::journal::record_event(
                "helper_recovery",
                "daemon is from another build; replacing it",
            );
            stop_recorded_daemon(&pid_file);
        }
        Health::Refused => {
            crate::journal::record_event("helper_recovery", "daemon refused this process");
            anyhow::bail!(REFUSED_MESSAGE);
        }
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
                    if is_current(socket_path) {
                        return Ok(());
                    }
                }
                crate::journal::record_event("helper_recovery", "daemon running without a socket");
                stop_recorded_daemon(&pid_file);
            }
        }
    }
    launch_and_wait(socket_path, claude_code_compat)
}

fn launch_and_wait(socket_path: &str, claude_code_compat: bool) -> anyhow::Result<()> {
    crate::cli::launch_daemon(socket_path, claude_code_compat)?;
    let until = Instant::now() + LAUNCH_DEADLINE;
    while Instant::now() < until {
        if probe(socket_path).is_healthy() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!(
        "Brigadier Computer Use did not start within {}s",
        LAUNCH_DEADLINE.as_secs()
    )
}

/// Stop the daemon on `socket_path` and start a fresh one, under the
/// lifecycle lock. For a watchdog that found it hung.
pub fn restart(socket_path: &str) -> anyhow::Result<()> {
    let _lock = LifecycleLock::acquire(&lock_path(socket_path))?;
    if crate::lifecycle::is_off(&crate::lifecycle::home()) {
        anyhow::bail!(crate::lifecycle::OFF_MESSAGE);
    }
    crate::journal::record_event("helper_recovery", "restarted by Brigadier's watchdog");
    stop_recorded_daemon(&crate::cli::daemon_pid_file_path(Some(socket_path)));
    launch_and_wait(socket_path, false)
}

/// Stop the daemon on `socket_path`, under the lifecycle lock.
pub fn stop(socket_path: &str) -> anyhow::Result<()> {
    let _lock = LifecycleLock::acquire(&lock_path(socket_path))?;
    stop_recorded_daemon(&crate::cli::daemon_pid_file_path(Some(socket_path)));
    Ok(())
}

/// Turn computer use off (stopping the daemon) or back on, under the
/// lifecycle lock, so no shim starts a daemon in between.
pub fn set_off(socket_path: &str, off: bool) -> anyhow::Result<()> {
    let _lock = LifecycleLock::acquire(&lock_path(socket_path))?;
    crate::lifecycle::set_off(&crate::lifecycle::home(), off)?;
    if off {
        stop_recorded_daemon(&crate::cli::daemon_pid_file_path(Some(socket_path)));
    }
    Ok(())
}

/// The recorded daemon's pid, when it is still ours and alive.
pub fn running_daemon(socket_path: &str) -> Option<u32> {
    recorded_daemon(&crate::cli::daemon_pid_file_path(Some(socket_path)))
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
    // The helper's own executable, wherever its bundle is now: an update moves the old
    // bundle aside (or deletes it) while its daemon still runs, and that daemon must still go.
    if !is_helper_executable(&process_executable(pid)?) {
        return None;
    }
    let started = process_start_time(pid)?;
    // One second of slack covers the clock granularity of file times.
    if started > written + Duration::from_secs(1) {
        return None;
    }
    Some(pid)
}

/// Whether `path` is the helper's executable inside a bundle
/// (`<bundle>/Contents/MacOS/brigadier-cu`), whatever the bundle is called now:
/// Brigadier swaps an update in by exchanging the two folders, so the old
/// daemon's bundle ends up under a temporary name.
fn is_helper_executable(path: &std::path::Path) -> bool {
    let mut parts = path.components().rev().map(|part| part.as_os_str());
    parts.next() == Some("brigadier-cu".as_ref())
        && parts.next() == Some("MacOS".as_ref())
        && parts.next() == Some("Contents".as_ref())
        && parts.next().is_some()
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
    fn the_old_daemon_is_known_by_its_executable_wherever_its_bundle_moved() {
        let helper = |path: &str| is_helper_executable(std::path::Path::new(path));
        assert!(helper(
            "/d/computer/Brigadier Computer Use.app/Contents/MacOS/brigadier-cu"
        ));
        // An update moved the old bundle aside while its daemon runs.
        assert!(helper(
            "/d/computer/.helper-new-42/Contents/MacOS/brigadier-cu"
        ));
        assert!(!helper("/usr/bin/brigadier-cu"));
        assert!(!helper("/d/Other.app/Contents/MacOS/other"));
        assert!(!helper("/d/cu-serve-under-test"));
    }

    #[test]
    fn a_missing_socket_is_absent() {
        let dir = std::env::temp_dir().join(format!("cu-supervisor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("none.sock");
        assert_eq!(probe(socket.to_str().unwrap()), Health::Absent);
        assert!(!is_current(socket.to_str().unwrap()));
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
    fn a_health_answer_carries_the_build_and_a_refusal_is_hung() {
        assert_eq!(
            health_from_answer(br#"{"ok":true,"result":{"build_id":"0.1-ab","pid":7}}"#),
            Health::Healthy {
                build_id: Some("0.1-ab".into())
            }
        );
        assert_eq!(
            health_from_answer(br#"{"ok":true,"result":{"pid":7}}"#),
            Health::Healthy { build_id: None }
        );
        // A silent main loop, or a daemon too old to know `health`.
        assert_eq!(
            health_from_answer(br#"{"ok":false,"error":"the main loop has been silent"}"#),
            Health::Hung
        );
        assert_eq!(health_from_answer(b"garbage"), Health::Hung);
        // A daemon that won't serve this process is healthy for the others.
        assert_eq!(
            health_from_answer(br#"{"ok":false,"refused":true,"error":"refused"}"#),
            Health::Refused
        );
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
