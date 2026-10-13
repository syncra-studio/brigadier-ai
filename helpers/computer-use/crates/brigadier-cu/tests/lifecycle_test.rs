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
    spawn_daemon_with(home, &[])
}

fn spawn_daemon_with(home: &std::path::Path, extra: &[&str]) -> std::process::Child {
    let copy = home.join("cu-serve-under-test");
    // A fresh file each time: macOS kills a process whose executable was
    // rewritten in place while an earlier one still had it mapped.
    let _ = std::fs::remove_file(&copy);
    std::fs::copy(env!("CARGO_BIN_EXE_brigadier-cu"), &copy).unwrap();
    Command::new(copy)
        .args(["serve", "--no-permissions-gate", "--no-overlay"])
        .args(extra)
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

impl Cleanup {
    fn kill_daemon(&mut self) {
        if let Some(pid) = std::fs::read_to_string(self.1.join("cu.pid"))
            .ok()
            .and_then(|pid| pid.trim().parse::<libc::pid_t>().ok())
        {
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }

    /// Kills the daemon and starts a fresh one on the same home, as a
    /// recovery would.
    fn replace(&mut self) {
        self.kill_daemon();
        self.0 = spawn_daemon(&self.1);
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        self.kill_daemon();
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

    // What it reads about its permissions is kept for Brigadier to show
    // without starting a daemon.
    let status = host(&home, &["request", "permissions_status"]);
    assert!(status["accessibility"].is_boolean(), "{status}");
    let kept: serde_json::Value =
        serde_json::from_slice(&std::fs::read(home.join("permissions.json")).unwrap()).unwrap();
    assert_eq!(kept["accessibility"], status["accessibility"]);
    assert_eq!(kept["screen_recording"], status["screen_recording"]);

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

/// One request on its own connection, as a shim's per-call connection.
fn daemon_request(home: &std::path::Path, request: serde_json::Value) -> serde_json::Value {
    let mut stream = std::os::unix::net::UnixStream::connect(home.join("cu.sock")).unwrap();
    writeln!(stream, "{request}").unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

/// A shim's control connection for `session`, kept open while it lives.
fn control_connection(home: &std::path::Path, session: &str) -> std::os::unix::net::UnixStream {
    let mut stream = std::os::unix::net::UnixStream::connect(home.join("cu.sock")).unwrap();
    writeln!(
        stream,
        "{}",
        serde_json::json!({"method": "session_begin", "session_id": session})
    )
    .unwrap();
    let mut line = String::new();
    BufReader::new(stream.try_clone().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert!(line.contains("session_begin"), "{line}");
    stream
}

fn wait_healthy(home: &std::path::Path) {
    let started = std::time::Instant::now();
    while host(home, &["probe"])["state"] != "healthy" {
        assert!(
            started.elapsed().as_secs() < 15,
            "the daemon never answered"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[test]
fn stop_using_an_app_holds_after_the_helper_is_replaced_and_only_for_that_agent() {
    let home = std::env::temp_dir().join(format!("cu-lifecycle-stop-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let finder: i64 = String::from_utf8(
        Command::new("pgrep")
            .args(["-x", "Finder"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .lines()
    .next()
    .expect("Finder runs")
    .parse()
    .unwrap();
    let mut cleanup = Cleanup(spawn_daemon(&home), home.clone());
    wait_healthy(&home);

    let minted = format!("brigadier-itest-mcp-{}-1", std::process::id());
    let control = control_connection(&home, &minted);
    let call = |host_session: &str| {
        daemon_request(
            &home,
            serde_json::json!({
                "method": "call",
                "name": "get_screen_size",
                "args": {"pid": finder, "_host_session": host_session},
                "session_id": minted,
            }),
        )
    };
    let first = call("brigadier-itest");
    assert_eq!(first["ok"], true, "{first}");
    assert_ne!(
        first["result"]["structuredContent"]["error"],
        "stopped_by_user"
    );
    let listed = host(&home, &["request", "agent_sessions"]);
    assert!(listed.to_string().contains("brigadier-itest"), "{listed}");
    assert!(listed.to_string().contains("com.apple.finder"), "{listed}");

    // The user chooses Stop using Finder.
    let stopped = host(
        &home,
        &[
            "request",
            "stop_app",
            r#"{"session":"brigadier-itest","bundle_id":"com.apple.finder","app":"Finder"}"#,
        ],
    );
    assert_eq!(stopped["stopped"], true, "{stopped}");
    let refused = call("brigadier-itest");
    assert_eq!(
        refused["result"]["structuredContent"]["error"], "stopped_by_user",
        "{refused}"
    );
    assert!(refused["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("stopped you from using Finder"));

    // The helper is replaced; the agent's shim reconnects and acts again.
    assert!(home.join("stopped.json").exists());
    // The helper dies first, as in a crash; the shim's control connection
    // goes with it (a shim that hangs up itself has exited, and its stops
    // lapse).
    cleanup.replace();
    drop(control);
    wait_healthy(&home);
    let _control = control_connection(&home, &minted);
    let still = call("brigadier-itest");
    assert_eq!(
        still["result"]["structuredContent"]["error"], "stopped_by_user",
        "{still}"
    );
    // Another agent may still use Finder.
    let other = call("brigadier-itest-other");
    assert_ne!(
        other["result"]["structuredContent"]["error"], "stopped_by_user",
        "{other}"
    );
    drop(cleanup);
}

#[test]
fn a_daemon_started_after_brigadier_finished_setup_is_ready_at_once() {
    let home = std::env::temp_dir().join(format!("cu-lifecycle-setup-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    // Brigadier's setup window confirmed screen capture earlier; the daemon
    // starts again (a restart or an update), told setup runs in Brigadier.
    std::fs::write(home.join("capture-confirmed"), b"").unwrap();
    let mut cleanup = Cleanup(
        spawn_daemon_with(&home, &["--external-permission-flow"]),
        home.clone(),
    );
    wait_healthy(&home);
    let status = host(&home, &["request", "permissions_status"]);
    assert_eq!(status["external_permission_ready"], true, "{status}");
    cleanup.kill_daemon();

    // Without the confirmation, setup isn't finished.
    std::fs::remove_file(home.join("capture-confirmed")).unwrap();
    cleanup.0 = spawn_daemon_with(&home, &["--external-permission-flow"]);
    wait_healthy(&home);
    let status = host(&home, &["request", "permissions_status"]);
    assert_eq!(status["external_permission_ready"], false, "{status}");
}
