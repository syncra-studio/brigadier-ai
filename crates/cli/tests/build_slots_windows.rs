//! Killing the Windows launcher must end the compiler before its slot can be reused.
#![cfg(windows)]

use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

struct Pool(PathBuf);

impl Drop for Pool {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn killed_launcher_leaves_no_surviving_unlocked_compiler() {
    let root = std::env::temp_dir().join(format!("brig-slot-job-{}", std::process::id()));
    fs::create_dir_all(root.join("build-tools")).unwrap();
    fs::create_dir_all(root.join("slots")).unwrap();
    let pool = Pool(root);
    for i in 0..brigadier_sandbox::build_slots::count() {
        fs::write(pool.0.join(format!("slots/slot-{i}")), "").unwrap();
    }
    let launcher = pool.0.join("build-tools/brigadier-build-slot.exe");
    fs::hard_link(env!("CARGO_BIN_EXE_brigadier"), &launcher).unwrap();
    let pid_file = pool.0.join("compiler-pid");
    let shell = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
        .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    let mut child = Command::new(launcher)
        .arg(shell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$PID | Set-Content -Encoding Ascii -LiteralPath $env:COMPILER_PID_FILE; Start-Sleep -Seconds 30",
        ])
        .env("COMPILER_PID_FILE", &pid_file)
        .env(
            brigadier_sandbox::build_slots::POOL_ENV,
            pool.0.join("slots"),
        )
        .spawn()
        .unwrap();
    let started = Instant::now();
    let compiler: u32 = loop {
        if let Some(pid) = fs::read_to_string(&pid_file)
            .ok()
            .and_then(|text| text.trim().parse().ok())
        {
            break pid;
        }
        if started.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("compiler did not start");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
        data_dir: Some(pool.0.join("data")),
    })
    .unwrap();
    assert!(platform.processes().is_alive(compiler));
    child.kill().unwrap();
    child.wait().unwrap();
    let started = Instant::now();
    while platform.processes().is_alive(compiler) && started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let survived = platform.processes().is_alive(compiler);
    if survived {
        let _ = platform.processes().terminate(compiler);
    }
    assert!(!survived, "a compiler survived the launcher's job closing");
    let slot = OpenOptions::new()
        .read(true)
        .write(true)
        .open(pool.0.join("slots/slot-0"))
        .unwrap();
    slot.try_lock().unwrap();
}
