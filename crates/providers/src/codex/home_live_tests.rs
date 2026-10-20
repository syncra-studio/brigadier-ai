//! Live (#121): a Codex thread with an output hook runs from a home of its own. It runs on a
//! scratch account home whose login links the machine's own (`~/.codex/auth.json`, which only a
//! token refresh writes, through the link), so nothing else of the user's home is touched:
//! `BRIGADIER_LIVE_CODEX=1 cargo test -p brigadier-providers --lib -- --ignored live_codex_thread_hook`

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::*;

struct Recorded(Mutex<Vec<Artifact>>);

impl Ledger for Recorded {
    fn record(&self, artifact: Artifact) -> crate::BoxFuture<'_, Result<()>> {
        self.0.lock().unwrap().push(artifact);
        Box::pin(async { Ok(()) })
    }
    fn holds(&self, _: &Artifact) -> bool {
        false
    }
}

/// The session's events until its turn ends (or its process does).
async fn turn(events: &mut mpsc::Receiver<ProviderEvent>) -> Vec<ProviderEvent> {
    let mut seen = Vec::new();
    let run = async {
        while let Some(event) = events.recv().await {
            let done = matches!(
                event,
                ProviderEvent::TurnCompleted { .. } | ProviderEvent::Exited { .. }
            );
            seen.push(event);
            if done {
                break;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(300), run)
        .await
        .expect("the turn ended");
    seen
}

/// `hooks/list` for `cwd` from a plain `codex app-server` on `home` (Brigadier's control
/// connections run with hooks off).
fn hooks_listed(codex: &Codex, home: &Path, cwd: &Path) -> Value {
    use std::io::{BufRead, Write};
    let mut child = std::process::Command::new(codex.binary().unwrap())
        .arg("app-server")
        .env("CODEX_HOME", home)
        .current_dir(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for line in [
        json!({ "id": 1, "method": "initialize", "params": { "clientInfo": { "name": "t", "version": "0" } } }),
        json!({ "method": "initialized" }),
        json!({ "id": 2, "method": "hooks/list", "params": { "cwds": [cwd] } }),
    ] {
        writeln!(stdin, "{line}").unwrap();
    }
    let reader = std::io::BufReader::new(child.stdout.take().unwrap());
    let listed = reader
        .lines()
        .map_while(std::result::Result::ok)
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .find(|message| message["id"] == 2)
        .expect("hooks/list answered");
    let _ = child.kill();
    let _ = child.wait();
    listed["result"].clone()
}

/// The thread homes under `data`.
fn homes(data: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(data.join(THREAD_HOMES))
        .map(|dir| dir.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default()
}

/// The rollout files under `dir` whose name holds `id`.
fn rollouts(dir: &Path, id: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(rollouts(&path, id));
        } else if path.to_string_lossy().contains(id) {
            found.push(path);
        }
    }
    found
}

/// Signed in through the linked login, the user's own trusted hook still runs, Brigadier's
/// runs after a shell call and after a patch with its grant from the environment, and its
/// context reaches the thread; the thread's rollout and databases stay in the account's home,
/// and the home stays for the next session, which resumes the thread with what it knew.
#[tokio::test]
#[ignore = "runs the real Codex CLI"]
async fn live_codex_thread_hook_runs_from_its_own_home() {
    if std::env::var_os("BRIGADIER_LIVE_CODEX").is_none() {
        return;
    }
    // A space in the path, as in the app's own data folder.
    let root = std::env::temp_dir().join(format!(
        "brigadier codex-hook-live-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let data = root.join("data");
    let main = root.join("main");
    let repo = root.join("repo");
    std::fs::create_dir_all(&main).unwrap();
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    std::fs::write(repo.join("sub/notes.txt"), "hello from sub\n").unwrap();
    let ok = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .unwrap();
    assert!(ok.success());
    let real_login = dirs::home_dir().unwrap().join(".codex/auth.json");
    std::os::unix::fs::symlink(&real_login, main.join("auth.json")).unwrap();
    let user_log = root.join("user-hook.log");
    std::fs::write(
        main.join("hooks.json"),
        json!({ "hooks": { "SessionStart": [{ "hooks": [{
            "type": "command",
            "command": format!("cat > /dev/null; echo ran >> '{}'", user_log.display()),
            "timeout": 10,
        }]}]}})
        .to_string(),
    )
    .unwrap();
    let events_log = root.join("events.jsonl");
    let grant_log = root.join("grant.log");
    let script = root.join("hook.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ncat >> '{events}'\necho >> '{events}'\necho \"grant=$PROBE_GRANT\" >> '{grant}'\nprintf '%s' '{{\"hookSpecificOutput\":{{\"hookEventName\":\"PostToolUse\",\"additionalContext\":\"The codeword is PELICAN-42.\"}}}}'\n",
            events = events_log.display(),
            grant = grant_log.display(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    std::fs::create_dir_all(&data).unwrap();
    let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
        data_dir: Some(data.clone()),
    })
    .unwrap();
    let captured = CliEnv::capture(&platform);
    let mut vars: Vec<_> = captured
        .vars()
        .into_iter()
        .filter(|(key, _)| key != "CODEX_HOME" && key != home::SQLITE_HOME)
        .collect();
    vars.push(("CODEX_HOME".into(), main.clone().into_os_string()));
    let codex = Codex::new(platform, Arc::new(captured.with_vars(vars)));

    // The user trusted their hook in their own home.
    let listed = hooks_listed(&codex, &main, &repo);
    let user_key = format!(
        "{}:session_start:0:0",
        main.canonicalize().unwrap().join("hooks.json").display()
    );
    let user_hash = listed["data"][0]["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|hook| hook["key"] == user_key.as_str())
        .and_then(|hook| hook["currentHash"].as_str())
        .unwrap_or_else(|| panic!("the user's hook is listed: {listed}"))
        .to_owned();
    std::fs::write(
        main.join("config.toml"),
        format!("[hooks.state.\"{user_key}\"]\ntrusted_hash = \"{user_hash}\"\n"),
    )
    .unwrap();

    let thread_spec = |origin: Origin| SessionSpec {
        cwd: repo.clone(),
        effort: Some("low".into()),
        origin,
        output_hook: Some(OutputHook {
            command: script.clone(),
            args: Vec::new(),
            env: vec![("PROBE_GRANT".into(), "g-live".into())],
            timeout_secs: 10,
        }),
        ..tests::spec(ToolSet::Thread, None)
    };
    let ledger = Arc::new(Recorded(Mutex::default()));
    let started = codex
        .start(thread_spec(Origin::New), ledger.clone())
        .await
        .expect("started");
    let mut events = started.events;
    let [home] = &homes(&data)[..] else {
        panic!("one thread home: {:?}", homes(&data));
    };
    let home = home.clone();
    started
        .session
        .send(TurnInput::text(
            "Run `cat sub/notes.txt` in the shell, then use apply_patch to add a line bye to \
             sub/notes.txt. Remember the word ZEBRA-7 for later. Then reply with only the \
             word DONE.",
        ))
        .await
        .unwrap();
    let seen = turn(&mut events).await;
    assert!(
        seen.iter()
            .any(|event| matches!(event, ProviderEvent::TurnCompleted { .. })),
        "the turn ran: {seen:?}"
    );
    let thread_id = started.session.native_id();
    let calls: Vec<Value> = std::fs::read_to_string(&events_log)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let tools: Vec<&str> = calls
        .iter()
        .filter_map(|call| call["tool_name"].as_str())
        .collect();
    assert!(tools.contains(&"Bash"), "{calls:?}");
    assert!(tools.contains(&"apply_patch"), "{calls:?}");
    assert!(
        std::fs::read_to_string(&grant_log)
            .unwrap()
            .lines()
            .all(|line| line == "grant=g-live"),
        "the hook has its grant"
    );
    assert!(user_log.exists(), "the user's trusted hook ran");
    for entry in std::fs::read_dir(&home).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            !name.contains(".sqlite"),
            "{name} stays in the account's home"
        );
    }
    assert!(main.join("state_5.sqlite").exists());
    // The rollout is in the account's home, and the hook's context is in it.
    let files = rollouts(&main.join("sessions"), &thread_id);
    assert_eq!(files.len(), 1, "{files:?}");
    let rollout = std::fs::read_to_string(&files[0]).unwrap();
    assert!(
        rollout.contains("PELICAN-42"),
        "the context reached the thread"
    );
    started.session.close().await;
    assert!(home.join("hooks.json").exists(), "the home stays");
    assert!(
        std::fs::symlink_metadata(main.join("auth.json"))
            .unwrap()
            .is_symlink()
    );

    // The next session resumes it (Brigadier archived it at close), from the same home, its
    // hook still trusted.
    let lines = |path: &Path| {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.is_empty())
            .count()
    };
    let hook_runs = lines(&events_log);
    let resumed = codex
        .start(
            thread_spec(Origin::Resume {
                native_id: thread_id.clone(),
            }),
            ledger.clone(),
        )
        .await
        .expect("resumed");
    assert_eq!(resumed.session.native_id(), thread_id);
    let mut events = resumed.events;
    resumed
        .session
        .send(TurnInput::text(
            "Run `cat sub/notes.txt` in the shell, then tell me which word I asked you to \
             remember. Reply with only that word.",
        ))
        .await
        .unwrap();
    let said: String = turn(&mut events)
        .await
        .into_iter()
        .filter_map(|event| match event {
            ProviderEvent::Message { text, .. } => Some(text),
            _ => None,
        })
        .collect();
    assert!(said.contains("ZEBRA-7"), "the resumed thread knows: {said}");
    assert!(
        lines(&events_log) > hook_runs,
        "Brigadier's hook still runs"
    );
    resumed.session.close().await;
    assert_eq!(homes(&data), [home]);
    // Brigadier's cleanup deletes it from the account's home.
    let made = ledger.0.lock().unwrap().clone();
    codex.remove(made).await.expect("removed");
    let left = [
        rollouts(&main.join("sessions"), &thread_id),
        rollouts(&main.join("archived_sessions"), &thread_id),
    ]
    .concat();
    assert!(left.is_empty(), "{left:?}");
    let _ = std::fs::remove_dir_all(&root);
}
