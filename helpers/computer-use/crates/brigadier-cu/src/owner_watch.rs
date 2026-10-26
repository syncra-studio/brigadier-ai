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
//! - Windows: a thread waits on the owner's process handle.
//!
//! The kqueue and pidfd paths bind to the process at registration time, so a
//! later pid reuse cannot hide the exit. The fallback poll can miss an exit
//! only if the pid is reused within one poll period.

/// Result of starting a watch on the owner pid.
#[derive(Debug)]
pub enum OwnerWatch {
    /// The owner is alive. The receiver completes when it exits.
    Watching(tokio::sync::oneshot::Receiver<()>),
    /// The owner was already dead when the watch started.
    AlreadyExited,
    /// This platform cannot watch a pid. The reason is safe to log.
    #[allow(dead_code)]
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

    /// Linux: a pidfd for the owner, polled until it is readable (the
    /// process exited). Kernels without `pidfd_open` fall back to the poll.
    #[cfg(target_os = "linux")]
    pub fn start(pid: i32, tx: tokio::sync::oneshot::Sender<()>) -> Started {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

        // SAFETY: pidfd_open takes a pid and flags and returns a descriptor
        // or -1; the descriptor is owned from here on.
        let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if raw < 0 {
            if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                return Started::AlreadyExited;
            }
            super::spawn_waiter("cu-owner-poll", move || super::poll_until_exit(pid, tx));
            return Started::Watching;
        }
        let Ok(raw) = i32::try_from(raw) else {
            super::spawn_waiter("cu-owner-poll", move || super::poll_until_exit(pid, tx));
            return Started::Watching;
        };
        // SAFETY: `raw` is a fresh descriptor nobody else owns.
        let pidfd = unsafe { OwnedFd::from_raw_fd(raw) };
        super::spawn_waiter("cu-owner-pidfd", move || loop {
            let mut poll = libc::pollfd {
                fd: pidfd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd; no timeout.
            let n = unsafe { libc::poll(&mut poll, 1, -1) };
            if n > 0 {
                let _ = tx.send(());
                return;
            }
            if n < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            super::poll_until_exit(pid, tx);
            return;
        });
        Started::Watching
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    pub fn start(pid: i32, tx: tokio::sync::oneshot::Sender<()>) -> Started {
        super::spawn_waiter("cu-owner-poll", move || super::poll_until_exit(pid, tx));
        Started::Watching
    }
}

/// Windows: open the owner's process and wait on its handle on a thread.
#[cfg(windows)]
pub fn watch(pid: i32) -> OwnerWatch {
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, INFINITE, PROCESS_SYNCHRONIZE,
    };
    let Ok(pid) = u32::try_from(pid) else {
        return OwnerWatch::AlreadyExited;
    };
    // SAFETY: the handle is checked, moved to the waiting thread and closed there.
    let Ok(process) = (unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }) else {
        return OwnerWatch::AlreadyExited;
    };
    // SAFETY: a zero timeout only polls the valid handle.
    if unsafe { WaitForSingleObject(process, 0) } != WAIT_TIMEOUT {
        let _ = unsafe { CloseHandle(process) };
        return OwnerWatch::AlreadyExited;
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    // A HANDLE is a pointer; carry it to the thread as an integer.
    let raw = process.0 as usize;
    std::thread::Builder::new()
        .name("cu-owner-wait".to_owned())
        .spawn(move || {
            let process = HANDLE(raw as *mut _);
            // SAFETY: the handle stays open until this thread closes it.
            unsafe {
                WaitForSingleObject(process, INFINITE);
                let _ = CloseHandle(process);
            }
            let _ = tx.send(());
        })
        .expect("spawn owner watch thread");
    OwnerWatch::Watching(rx)
}
