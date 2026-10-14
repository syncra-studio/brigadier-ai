//! Process-level contract for `brigadier-cu serve --owner-pid <pid>`: the daemon
//! exits with status 0 and removes its socket when the owner process exits,
//! and exits at once when the owner is already dead at start.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Linux refuses to exec a file that any process still holds open for
/// writing (ETXTBSY). A child forked by a parallel test inherits the copy's
/// write descriptor until its own exec, so every copy and every spawn in
/// this file runs under one lock.
static SPAWN_LOCK: Mutex<()> = Mutex::new(());

/// Every copy is a new unsigned identity, which must never ask macOS about
/// accessibility or screen capture: each one that does is added to the
/// privacy lists in System Settings.
const NO_TCC: &str = "BRIGADIER_CU_NO_TCC_FOR_TESTS";

fn spawn_lock() -> MutexGuard<'static, ()> {
    SPAWN_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn private_root() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("temp root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("make temp root private");
    root
}

fn spawn_owner() -> Child {
    let _guard = spawn_lock();
    Command::new("sleep")
        .arg("60")
        .spawn()
        .expect("spawn sleep owner")
}

/// The binary named `brigadier-cu` refuses `serve` outside the branded helper
/// app (exit 78). A copy under another name in the test's temp root runs it.
/// `fs::copy` writes a new file, so it never rewrites a signed image in place.
fn unbranded_binary(root: &Path) -> std::path::PathBuf {
    let copy = root.join("cu-serve-under-test");
    std::fs::copy(env!("CARGO_BIN_EXE_brigadier-cu"), &copy).expect("copy test binary");
    copy
}

fn spawn_serve(root: &Path, socket: &Path, owner_pid: u32) -> Child {
    let _guard = spawn_lock();
    Command::new(unbranded_binary(root))
        .arg("serve")
        .arg("--socket")
        .arg(socket)
        .arg("--no-permissions-gate")
        .arg("--no-overlay")
        .arg("--owner-pid")
        .arg(owner_pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env(NO_TCC, "1")
        .env_remove("BRIGADIER_CU_SOCKET_AUTHORIZED_ROOT_PID")
        .env_remove("BRIGADIER_CU_SOCKET_AUTHORIZED_ROOT_START_SECONDS")
        .env_remove("BRIGADIER_CU_SOCKET_AUTHORIZED_ROOT_START_MICROSECONDS")
        .spawn()
        .expect("spawn brigadier-cu serve")
}

fn wait_with_deadline(child: &mut Child, limit: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().expect("poll daemon") {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn serve_exits_zero_and_removes_its_socket_when_the_owner_exits() {
    let root = private_root();
    let socket = root.path().join("owned.sock");
    let mut owner = spawn_owner();
    let mut daemon = KillOnDrop(spawn_serve(root.path(), &socket, owner.id()));

    let bind_deadline = Instant::now() + Duration::from_secs(10);
    while UnixStream::connect(&socket).is_err() {
        assert!(
            daemon.0.try_wait().expect("poll daemon").is_none(),
            "daemon exited before the owner exited"
        );
        assert!(Instant::now() < bind_deadline, "daemon did not bind");
        std::thread::sleep(Duration::from_millis(20));
    }

    owner.kill().expect("kill owner");
    owner.wait().expect("reap owner");

    let status = wait_with_deadline(&mut daemon.0, Duration::from_secs(2))
        .expect("daemon must exit within 2 s after the owner exits");
    assert!(status.success(), "daemon must exit 0, got {status}");
    assert!(!socket.exists(), "daemon must remove its socket");
}

#[test]
fn serve_exits_zero_at_once_when_the_owner_is_already_dead() {
    let root = private_root();
    let socket = root.path().join("orphan.sock");
    let mut owner = spawn_owner();
    let owner_pid = owner.id();
    owner.kill().expect("kill owner");
    owner.wait().expect("reap owner");

    let mut daemon = KillOnDrop(spawn_serve(root.path(), &socket, owner_pid));
    let status = wait_with_deadline(&mut daemon.0, Duration::from_secs(2))
        .expect("daemon must exit at once when the owner is already dead");
    assert!(status.success(), "daemon must exit 0, got {status}");
    assert!(!socket.exists(), "daemon must not leave a socket behind");
}

#[test]
fn serve_refuses_an_invalid_owner_pid() {
    let root = private_root();
    let socket = root.path().join("invalid.sock");
    let guard = spawn_lock();
    let mut daemon = KillOnDrop(
        Command::new(unbranded_binary(root.path()))
            .arg("serve")
            .arg("--socket")
            .arg(&socket)
            .arg("--no-permissions-gate")
            .arg("--owner-pid")
            .arg("0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .env(NO_TCC, "1")
            .spawn()
            .expect("spawn brigadier-cu serve"),
    );
    drop(guard);
    let status = wait_with_deadline(&mut daemon.0, Duration::from_secs(5))
        .expect("daemon must refuse an invalid owner pid at once");
    assert_eq!(
        status.code(),
        Some(2),
        "invalid --owner-pid is a usage error"
    );
    assert!(!socket.exists());
}
