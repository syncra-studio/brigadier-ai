//! The helper's lifecycle as Brigadier relies on it: turning computer use off
//! holds for shims and daemons already running, and a daemon that stops
//! answering probes as hung.

#![cfg(target_os = "macos")]

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn rpc(
    stdin: &mut impl Write,
    stdout: &mut impl BufRead,
    id: u64,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let message =
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    writeln!(stdin, "{message}").unwrap();
    stdin.flush().unwrap();
    loop {
        let mut line = String::new();
        assert!(
            stdout.read_line(&mut line).unwrap() > 0,
            "shim closed stdout"
        );
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        if value.get("id") == Some(&serde_json::json!(id)) {
            return value;
        }
    }
}

#[test]
fn a_running_shim_is_refused_once_computer_use_is_turned_off_and_starts_nothing() {
    let home = std::env::temp_dir().join(format!("cu-lifecycle-off-shim-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let mut shim = Command::new(env!("CARGO_BIN_EXE_brigadier-cu"))
        .arg("mcp")
        .env("BRIGADIER_CU_HOME", &home)
        // Any start attempt would fail loudly with a different message.
        .env("BRIGADIER_CU_DAEMON_APP", home.join("Missing Helper.app"))
        .env("BRIGADIER_CU_SOCKET_AUTH_TOKEN", "t".repeat(64))
        .env_remove("BRIGADIER_CU_EMBEDDED")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = shim.stdin.take().unwrap();
    let mut stdout = BufReader::new(shim.stdout.take().unwrap());
    let init = rpc(
        &mut stdin,
        &mut stdout,
        1,
        "initialize",
        serde_json::json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}),
    );
    assert!(init.get("result").is_some(), "{init}");

    // Brigadier turns computer use off while the agent's shim is running.
    let host = Command::new(env!("CARGO_BIN_EXE_brigadier-cu"))
        .args(["host", "off"])
        .env("BRIGADIER_CU_HOME", &home)
        .output()
        .unwrap();
    assert!(host.status.success(), "{host:?}");

    let call = rpc(
        &mut stdin,
        &mut stdout,
        2,
        "tools/call",
        serde_json::json!({"name": "list_apps", "arguments": {}}),
    );
    let result = &call["result"];
    assert_eq!(result["isError"], true, "{call}");
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    assert!(text.contains("turned off"), "{text}");
    assert!(
        !home.join("cu.pid").exists(),
        "no helper may start while off"
    );
    assert!(
        !home.join("cu.sock").exists(),
        "no helper may start while off"
    );

    let _ = shim.kill();
    let _ = shim.wait();
    std::fs::remove_dir_all(&home).unwrap();
}

/// A daemon started from a copy of the binary (the branded name refuses
/// `serve` outside the helper app), on the default socket in `home`.
fn spawn_daemon(home: &std::path::Path) -> std::process::Child {
    let copy = home.join("cu-serve-under-test");
    std::fs::copy(env!("CARGO_BIN_EXE_brigadier-cu"), &copy).unwrap();
    Command::new(copy)
        .args(["serve", "--no-permissions-gate", "--no-overlay"])
        .env("BRIGADIER_CU_HOME", home)
        .env_remove("BRIGADIER_CU_SOCKET_AUTH_TOKEN")
        .env_remove("BRIGADIER_CU_SOCKET_AUTHORIZED_ROOT_PID")
        .env_remove("BRIGADIER_CU_SOCKET_AUTHORIZED_ROOT_START_SECONDS")
        .env_remove("BRIGADIER_CU_SOCKET_AUTHORIZED_ROOT_START_MICROSECONDS")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

/// Kills the daemon (both the launched process and the one serving) and
/// removes the home, even when an assertion fails.
struct Cleanup(std::process::Child, std::path::PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(pid) = std::fs::read_to_string(self.1.join("cu.pid"))
            .ok()
            .and_then(|pid| pid.trim().parse::<libc::pid_t>().ok())
        {
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_dir_all(&self.1);
    }
}

fn host(home: &std::path::Path, args: &[&str]) -> serde_json::Value {
    host_launching(home, None, args)
}

/// `host` as a shim set up to launch the daemon from `app` would run it.
fn host_launching(
    home: &std::path::Path,
    app: Option<&std::path::Path>,
    args: &[&str],
) -> serde_json::Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_brigadier-cu"));
    match app {
        Some(app) => command.env("BRIGADIER_CU_DAEMON_APP", app),
        None => command.env_remove("BRIGADIER_CU_DAEMON_APP"),
    };
    let out = command
        .arg("host")
        .args(args)
        .env("BRIGADIER_CU_HOME", home)
        .env_remove("BRIGADIER_CU_SOCKET_AUTH_TOKEN")
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("{out:?}"))
}

#[test]
fn the_daemon_reports_its_build_refuses_calls_while_off_and_probes_hung_when_stopped() {
    let home = std::env::temp_dir().join(format!("cu-lifecycle-daemon-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let _cleanup = Cleanup(spawn_daemon(&home), home.clone());
    let started = std::time::Instant::now();
    while host(&home, &["probe"])["state"] != "healthy" {
        assert!(
            started.elapsed().as_secs() < 15,
            "the daemon never answered"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // The running daemon is from no staged build. Against a staged bundle
    // on disk (an update arrived), it is from another build and must go.
    assert_eq!(host(&home, &["probe"])["current"], true);
    let staged = home.join("Staged.app");
    std::fs::create_dir_all(staged.join("Contents/Resources")).unwrap();
    std::fs::write(staged.join("Contents/Resources/build-id"), "9.9.9-new\n").unwrap();
    let probed = host_launching(&home, Some(&staged), &["probe"]);
    assert_eq!(probed["state"], "healthy");
    assert_eq!(probed["current"], false, "{probed}");

    // Off holds in the daemon too, for a call that reaches it directly.
    std::fs::write(home.join("off"), b"").unwrap();
    let refused = host(&home, &["request", "call", "{}", "--name", "list_apps"]);
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(refused["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .contains("turned off"));
    std::fs::remove_file(home.join("off")).unwrap();

    // A stopped daemon still accepts connections but never answers: hung.
    // The pid file names the process that serves (the launched one hands the
    // work to a child).
    let pid: libc::pid_t = std::fs::read_to_string(home.join("cu.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    unsafe { libc::kill(pid, libc::SIGSTOP) };
    let probed = std::time::Instant::now();
    assert_eq!(host(&home, &["probe"])["state"], "hung");
    assert!(probed.elapsed().as_secs() < 5);
    unsafe { libc::kill(pid, libc::SIGCONT) };
    assert_eq!(host(&home, &["probe"])["state"], "healthy");
}
