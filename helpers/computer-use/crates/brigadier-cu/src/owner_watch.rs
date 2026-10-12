//! `serve --owner-pid <pid>`: notice when the owning process exits.
//!
//! A host app (for example Brigadier) starts `brigadier-cu serve` through
//! LaunchServices, so the daemon's parent is launchd and it never sees the
//! app die. The app passes its own pid; the daemon watches that pid and shuts
//! down (removing its socket) when the pid exits.
//!
//! Mechanism per platform:
//! - macOS: kqueue `EVFILT_PROC` with `NOTE_EXIT`.
//! - Linux: `pidfd_open(2)` plus `poll(2)` (kernel 5.3+). If `pidfd_open` is
//!   not available, fall back to a 250 ms `kill(pid, 0)` liveness poll.
//! - Other Unix targets: the same 250 ms liveness poll.
//! - Windows: not supported; serve logs that it ignores the flag.
//!
//! The kqueue and pidfd paths bind to the process at registration time, so a
//! later pid reuse cannot hide the exit. The fallback poll can miss an exit
//! only if the pid is reused within one poll period.

/// Result of starting a watch on the owner pid.
#[derive(Debug)]
pub enum OwnerWatch {
    /// The owner is alive. The receiver completes when it exits.
    #[cfg_attr(not(unix), allow(dead_code))]
    Watching(tokio::sync::oneshot::Receiver<()>),
    /// The owner was already dead when the watch started.
    #[cfg_attr(not(unix), allow(dead_code))]
    AlreadyExited,
    /// This platform cannot watch a pid. The reason is safe to log.
    #[cfg_attr(unix, allow(dead_code))]
    Unsupported(&'static str),
}

/// Parse an `--owner-pid` value. Only a positive 32-bit process id is valid.
pub fn parse_owner_pid(raw: &str) -> Result<i32, String> {
    match raw.trim().parse::<i32>() {
        Ok(pid) if pid > 0 => Ok(pid),
        _ => Err(format!(
            "--owner-pid needs a positive process id, got {raw:?}"
        )),
    }
}

/// Start watching `pid`. Never blocks; the wait runs on a dedicated thread.
#[cfg(unix)]
pub fn watch(pid: i32) -> OwnerWatch {
    if !process_exists(pid) {
        return OwnerWatch::AlreadyExited;
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    match platform::start(pid, tx) {
        platform::Started::Watching => OwnerWatch::Watching(rx),
        platform::Started::AlreadyExited => OwnerWatch::AlreadyExited,
    }
}

/// `kill(pid, 0)`: `ESRCH` means no such process. `EPERM` means the process
/// exists but belongs to another user, so it counts as alive.
#[cfg(unix)]
fn process_exists(pid: i32) -> bool {
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(unix)]
fn spawn_waiter(name: &str, wait: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(wait)
        .expect("spawn owner watch thread");
}

/// Portable fallback: poll `kill(pid, 0)` until the pid is gone.
#[cfg(unix)]
fn poll_until_exit(pid: i32, tx: tokio::sync::oneshot::Sender<()>) {
    const POLL_PERIOD: std::time::Duration = std::time::Duration::from_millis(250);
    while process_exists(pid) {
        if tx.is_closed() {
            return;
        }
        std::thread::sleep(POLL_PERIOD);
    }
    let _ = tx.send(());
}

#[cfg(unix)]
mod platform {
    pub enum Started {
        Watching,
        AlreadyExited,
    }

    #[cfg(target_os = "macos")]
    pub fn start(pid: i32, tx: tokio::sync::oneshot::Sender<()>) -> Started {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

        let raw = unsafe { libc::kqueue() };
        if raw < 0 {
            super::spawn_waiter("cu-owner-poll", move || super::poll_until_exit(pid, tx));
            return Started::Watching;
        }
        let kq = unsafe { OwnedFd::from_raw_fd(raw) };
        let mut change: libc::kevent = unsafe { std::mem::zeroed() };
        change.ident = pid as libc::uintptr_t;
        change.filter = libc::EVFILT_PROC;
        change.flags = libc::EV_ADD | libc::EV_ONESHOT;
        change.fflags = libc::NOTE_EXIT;
        let rc = unsafe {
            libc::kevent(
                kq.as_raw_fd(),
                &change,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        if rc < 0 {
            if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                return Started::AlreadyExited;
            }
            super::spawn_waiter("cu-owner-poll", move || super::poll_until_exit(pid, tx));
            return Started::Watching;
        }
        super::spawn_waiter("cu-owner-kqueue", move || loop {
            let mut event: libc::kevent = unsafe { std::mem::zeroed() };
            let n = unsafe {
                libc::kevent(
                    kq.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    &mut event,
                    1,
                    std::ptr::null(),
                )
            };
            if n > 0 {
                let _ = tx.send(());
                return;
            }
            if n < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            // Unexpected kqueue failure: keep watching with the portable poll.
            super::poll_until_exit(pid, tx);
            return;
        });
        Started::Watching
    }
}
