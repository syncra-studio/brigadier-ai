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
        self.command_at(&self.0.join("build-tools/brigadier-build-slot"), tool)
    }

    fn command_at(&self, launcher: &Path, tool: &Path) -> Command {
        let mut cmd = Command::new(launcher);
        cmd.arg(tool)
            .env("TEST_POOL", &self.0)
            .env(
                brigadier_sandbox::build_slots::POOL_ENV,
                self.0.join("build-slots"),
            )
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
fn concurrent_daemon_launchers_share_the_machine_pool() {
    let pool = Pool::new("concurrent");
    let other = pool.0.join("other-daemon/build-tools/brigadier-build-slot");
    fs::create_dir_all(other.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_brigadier"), &other).unwrap();
    let tool = pool.script("compiler", "printf '+\\n' >> \"$TEST_POOL/events\"\nsleep 0.15\nprintf '%s\\n' - >> \"$TEST_POOL/events\"");
    let slots = brigadier_sandbox::build_slots::count();
    let mut children: Vec<_> = (0..slots * 4)
        .map(|i| {
            if i % 2 == 0 {
                pool.command(&tool)
            } else {
                pool.command_at(&other, &tool)
            }
            .spawn()
            .unwrap()
        })
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
fn killing_the_slot_holder_leaves_no_surviving_unlocked_compiler() {
    let pool = Pool::new("killed");
    let _others: Vec<_> = (1..brigadier_sandbox::build_slots::count())
        .map(|i| pool.lock(i))
        .collect();
    let tool = pool.script("compiler", "echo $$ > \"$TEST_POOL/child\"\nexec sleep 30");
    let mut holder = pool.command(&tool).spawn().unwrap();
    pool.wait_file("child");
    let file = File::open(pool.0.join("build-slots/slot-0")).unwrap();
    assert!(matches!(file.try_lock(), Err(TryLockError::WouldBlock)));
    let compiler: u32 = fs::read_to_string(pool.0.join("child"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(compiler, holder.id(), "the launcher must exec the compiler");
    holder.kill().unwrap();
    assert_eq!(holder.wait().unwrap().signal(), Some(9));
    file.try_lock()
        .expect("the dead compiler released its slot");
    assert!(
        !Command::new("/bin/kill")
            .args(["-0", &compiler.to_string()])
            .output()
            .unwrap()
            .status
            .success(),
        "an unlocked compiler survived"
    );
}

#[test]
fn compiler_descendants_keep_the_slot_until_they_exit() {
    let pool = Pool::new("descendant");
    let tool = pool.script(
        "compiler",
        "sleep 30 >/dev/null 2>&1 &\necho $! > \"$TEST_POOL/descendant\"",
    );
    assert!(wait(&mut pool.command(&tool).spawn().unwrap()).success());
    let file = File::open(pool.0.join("build-slots/slot-0")).unwrap();
    assert!(matches!(file.try_lock(), Err(TryLockError::WouldBlock)));
    let pid = fs::read_to_string(pool.0.join("descendant"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    signal(pid, "-0");
    signal(pid, "-KILL");
    let start = Instant::now();
    loop {
        if file.try_lock().is_ok() {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "descendant did not release the slot"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
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

#[test]
fn cargo_config_wrapper_and_custom_workspace_compiler_stay_chained() {
    let pool = Pool::new("cargo-config");
    let project = pool.0.join("project");
    fs::create_dir_all(project.join(".cargo")).unwrap();
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(
        project.join("Cargo.toml"),
        "[package]\nname='wrapper_config_proof'\nversion='0.1.0'\nedition='2024'\n",
    )
    .unwrap();
    fs::write(project.join("src/main.rs"), "fn main() {}\n").unwrap();
    pool.script(
        "configured-cache",
        "echo configured-cache >> \"$TEST_POOL/config-chain\"\nexec \"$@\"",
    );
    pool.script(
        "workspace-cache",
        "echo workspace-cache >> \"$TEST_POOL/config-chain\"\nexec \"$@\"",
    );
    pool.script(
        "custom-compiler",
        "echo custom-compiler >> \"$TEST_POOL/config-chain\"\nexec rustc \"$@\"",
    );
    fs::write(project.join(".cargo/config.toml"), "[build]\nrustc-wrapper='../configured-cache'\nrustc-workspace-wrapper='../workspace-cache'\nrustc='../custom-compiler'\n").unwrap();
    let launcher = pool.0.join("build-tools/brigadier-build-slot");
    let cargo_home = pool.0.join("cargo-home");
    let vars = brigadier_sandbox::build_slots::environment_in(&launcher, Some(&project), |name| {
        (name == "CARGO_HOME").then(|| cargo_home.to_string_lossy().into_owned())
    });
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["build", "--offline"])
        .current_dir(&project)
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("RUSTC")
        .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC")
        .env_remove("CARGO_TARGET_DIR")
        .envs(vars)
        .env("CARGO_HOME", cargo_home)
        .env("TEST_POOL", &pool.0)
        .env(
            brigadier_sandbox::build_slots::POOL_ENV,
            pool.0.join("build-slots"),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(pool.0.join("config-chain")).unwrap();
    for program in ["configured-cache", "workspace-cache", "custom-compiler"] {
        assert!(
            log.lines().any(|line| line == program),
            "{program} was masked: {log}"
        );
    }
}
