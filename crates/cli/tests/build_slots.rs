//! The multicall compiler launcher against real processes and independent lock handles.
#![cfg(unix)]

use std::fs::{self, File, TryLockError};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

struct Pool(PathBuf);

impl Pool {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("bslots-{name}-{}", std::process::id()));
        fs::create_dir_all(root.join("build-tools")).unwrap();
        fs::create_dir_all(root.join("build-slots")).unwrap();
        for i in 0..brigadier_sandbox::build_slots::count() {
            File::create(root.join(format!("build-slots/slot-{i}"))).unwrap();
        }
        std::os::unix::fs::symlink(
            env!("CARGO_BIN_EXE_brigadier"),
            root.join("build-tools/brigadier-build-slot"),
        )
        .unwrap();
        Self(root)
    }

    fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn command(&self, tool: &Path) -> Command {
        let mut cmd = Command::new(self.0.join("build-tools/brigadier-build-slot"));
        cmd.arg(tool)
            .env("TEST_POOL", &self.0)
            .env_remove("BRIGADIER_BUILD_RUSTC_WRAPPER")
            .env_remove("RUSTC")
            .env_remove("RUSTC_WORKSPACE_WRAPPER");
        cmd
    }

    fn lock(&self, i: usize) -> File {
        let file = File::open(self.0.join(format!("build-slots/slot-{i}"))).unwrap();
        file.try_lock().unwrap();
        file
    }

    fn wait_file(&self, name: &str) {
        let start = Instant::now();
        while !self.0.join(name).exists() {
            assert!(start.elapsed() < Duration::from_secs(5), "missing {name}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn wait(child: &mut Child) -> ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if start.elapsed() > Duration::from_secs(5) {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("wrapper did not finish");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn signal(pid: u32, sig: &str) {
    assert!(
        Command::new("/bin/kill")
            .args([sig, &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn concurrent_compilers_share_the_machine_pool() {
    let pool = Pool::new("concurrent");
    let tool = pool.script("compiler", "printf '+\\n' >> \"$TEST_POOL/events\"\nsleep 0.15\nprintf '%s\\n' - >> \"$TEST_POOL/events\"");
    let slots = brigadier_sandbox::build_slots::count();
    let mut children: Vec<_> = (0..slots * 4)
        .map(|_| pool.command(&tool).spawn().unwrap())
        .collect();
    for child in &mut children {
        assert!(wait(child).success());
    }
    let events = fs::read_to_string(pool.0.join("events")).unwrap();
    let (mut active, mut peak, mut finished) = (0, 0, 0);
    for event in events.lines() {
        if event == "+" {
            active += 1;
        } else {
            active -= 1;
            finished += 1;
        }
        peak = peak.max(active);
        assert!(active <= slots);
    }
    assert_eq!(active, 0);
    assert_eq!(finished, slots * 4);
    assert_eq!(peak, slots);
}

#[test]
fn killed_holder_frees_the_slot_without_waiting_for_its_child() {
    let pool = Pool::new("killed");
    let _others: Vec<_> = (1..brigadier_sandbox::build_slots::count())
        .map(|i| pool.lock(i))
        .collect();
    let tool = pool.script("compiler", "echo $$ > \"$TEST_POOL/child\"\nexec sleep 30");
    let mut holder = pool.command(&tool).spawn().unwrap();
    pool.wait_file("child");
    let file = File::open(pool.0.join("build-slots/slot-0")).unwrap();
    assert!(matches!(file.try_lock(), Err(TryLockError::WouldBlock)));
    holder.kill().unwrap();
    assert_eq!(holder.wait().unwrap().signal(), Some(9));
    file.try_lock().expect("the dead wrapper released its slot");
    let pid = fs::read_to_string(pool.0.join("child"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    signal(pid, "-KILL");
}

#[test]
fn compiler_descendant_inherits_no_slot() {
    let pool = Pool::new("descendant");
    let tool = pool.script(
        "compiler",
        "sleep 30 >/dev/null 2>&1 &\necho $! > \"$TEST_POOL/descendant\"",
    );
    assert!(wait(&mut pool.command(&tool).spawn().unwrap()).success());
    let _locks: Vec<_> = (0..brigadier_sandbox::build_slots::count())
        .map(|i| pool.lock(i))
        .collect();
    let pid = fs::read_to_string(pool.0.join("descendant"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    signal(pid, "-0");
    signal(pid, "-KILL");
}

#[test]
fn rust_probes_bypass_a_full_pool_and_preserve_exit_status() {
    let pool = Pool::new("probes");
    let _locks: Vec<_> = (0..brigadier_sandbox::build_slots::count())
        .map(|i| pool.lock(i))
        .collect();
    let tool = pool.script("rustc", "exit 23");
    for args in [
        vec!["-vV"],
        vec!["--version"],
        vec!["--print", "cfg"],
        vec!["--print=cfg"],
        vec!["--crate-name", "___"],
    ] {
        let mut child = pool.command(&tool).args(args).spawn().unwrap();
        assert_eq!(wait(&mut child).code(), Some(23));
    }
}

#[test]
fn existing_rust_wrapper_is_chained_only_for_rust() {
    let pool = Pool::new("chain");
    let chain = pool.script(
        "sccache",
        "printf '%s\\n' \"$@\" > \"$TEST_POOL/chain\"\nexec \"$@\"",
    );
    let tool = pool.script("rustc", "exit 17");
    let status = wait(
        &mut pool
            .command(&tool)
            .env("BRIGADIER_BUILD_RUSTC_WRAPPER", &chain)
            .arg("hello world")
            .spawn()
            .unwrap(),
    );
    assert_eq!(status.code(), Some(17));
    assert_eq!(
        fs::read_to_string(pool.0.join("chain")).unwrap(),
        format!("{}\nhello world\n", tool.display())
    );
    fs::remove_file(pool.0.join("chain")).unwrap();
    let tool = pool.script("cc", "exit 0");
    assert!(
        wait(
            &mut pool
                .command(&tool)
                .env("BRIGADIER_BUILD_RUSTC_WRAPPER", &chain)
                .spawn()
                .unwrap()
        )
        .success()
    );
    assert!(!pool.0.join("chain").exists());
}

#[test]
fn termination_and_interrupt_reach_the_compiler() {
    for sig in ["-TERM", "-INT"] {
        let pool = Pool::new(sig);
        let tool = pool.script("compiler", "echo $$ > \"$TEST_POOL/child\"\nexec sleep 30");
        let mut child = pool.command(&tool).spawn().unwrap();
        pool.wait_file("child");
        signal(child.id(), sig);
        assert_eq!(
            wait(&mut child).signal(),
            Some(if sig == "-TERM" { 15 } else { 2 })
        );
        let _locks: Vec<_> = (0..brigadier_sandbox::build_slots::count())
            .map(|i| pool.lock(i))
            .collect();
    }
}
