//! `run_check` end to end (THREAD-PLAN.md Q8 lever 3): a worker's check runs once per tree,
//! the thread re-checking the same files gets the worker's result from the cache, `rerun` and
//! an edit run it again, a folder outside git leaves the cache out, and a pass is learned into
//! the project's Brain.

use std::sync::{Arc, Mutex};

use brigadier_brain::NodeKind;
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::model::DomainEvent;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

#[tokio::test]
async fn a_check_runs_once_per_tree_for_workers_and_the_thread() {
    let replies: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        "checks",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                let check = |args: serde_json::Value| {
                    let turn = &turn;
                    let log = log.clone();
                    async move {
                        let reply = turn.call("run_check", args).await;
                        assert!(!reply.is_error, "{}", reply.text);
                        log.lock().unwrap().push(reply.text);
                    }
                };
                if !turn.is_orchestrator() {
                    check(json!({})).await;
                    check(json!({ "command": "echo checked" })).await;
                    check(json!({ "command": "echo checked" })).await;
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "The check passes.", "verification": "echo checked"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                if turn.earlier == 0 {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Run the check", "kind": "scout",
                                   "spec": "Run the check."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                // The worker's report: the same files, so the same result.
                check(json!({ "command": "echo checked" })).await;
                check(json!({ "command": "echo checked", "rerun": true })).await;
                std::fs::write(turn.add_dirs[0].join("greeting.txt"), "hello\n").unwrap();
                check(json!({ "command": "echo checked" })).await;
                check(json!({ "command": "echo checked", "workdir": turn.cwd })).await;
                Reply::text("Checked.")
            }
        }),
    )
    .await;
    flow.say("Check the greeting.").await;
    flow.until("the thread's checks", |_| {
        replies.lock().unwrap().len() == 7
    })
    .await;
    flow.settled().await;

    let replies = replies.lock().unwrap().clone();
    assert!(replies[0].starts_with("Nothing changed"), "{}", replies[0]);
    assert_eq!(replies[1], "[exit 0]\nchecked\n");
    assert!(replies[2].starts_with("[cached: "), "{}", replies[2]);
    assert!(
        replies[2].ends_with("[exit 0]\nchecked\n"),
        "{}",
        replies[2]
    );
    assert!(replies[3].starts_with("[cached: "), "{}", replies[3]);
    for fresh in &replies[4..] {
        assert_eq!(fresh, "[exit 0]\nchecked\n");
    }

    let checks: Vec<_> = flow
        .events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            DomainEvent::CheckRan {
                task_id,
                tree,
                cached,
                bypassed,
                status,
                artifact,
                workdir,
                ..
            } => {
                assert_eq!(status, "exit 0");
                assert!(artifact.is_some());
                assert_eq!(workdir, "");
                Some((task_id.is_some(), cached, bypassed, tree))
            }
            _ => None,
        })
        .collect();
    let shape: Vec<_> = checks
        .iter()
        .map(|(worker, cached, bypassed, _)| (*worker, *cached, bypassed.clone()))
        .collect();
    assert_eq!(
        shape,
        [
            (true, false, None),
            (true, true, None),
            (false, true, None),
            (false, false, None),
            (false, false, None),
            (false, false, Some("not in a git repository".to_owned())),
        ]
    );
    // The thread's files were the worker's; after the edit they are not.
    assert_eq!(checks[0].3, checks[2].3);
    assert_eq!(checks[2].3, checks[3].3);
    assert_ne!(checks[3].3, checks[4].3);
    assert_eq!(checks[5].3, None);

    // The pass is learned for the root's folder, once.
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id
        .unwrap();
    let brain = flow.manager.project_brain(&project).await.unwrap();
    let mut learned = None;
    super::eventually("the pass to be learned", || {
        learned = brain
            .brain
            .node_by_key(NodeKind::Convention, "checks:.")
            .unwrap();
        learned.is_some()
    })
    .await;
    let learned = learned.unwrap();
    assert_eq!(
        learned.body.matches("`echo checked`").count(),
        1,
        "{}",
        learned.body
    );
    flow.stop().await;
}

/// Bug 8: a check runs with a raised open-files limit and the task's test folder as TMPDIR,
/// and a prefix that changes neither is dropped, so the check answers from the cache; one
/// that changes how it runs keeps its own entry and runs as written.
#[tokio::test]
async fn a_prefix_that_changes_nothing_still_answers_from_the_cache() {
    // As the daemon does at start; the checks inherit it.
    let limits = brigadier_sandbox::limits::raise_open_files().unwrap();
    let replies: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        "check-prefixes",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.earlier == 0 {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "high", "title": "Run the check", "kind": "scout",
                                       "spec": "Run the check."}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                    }
                    return Reply::text("[quiet]");
                }
                let check = |command: String| {
                    let turn = &turn;
                    let log = log.clone();
                    async move {
                        let reply = turn.call("run_check", json!({ "command": command })).await;
                        assert!(!reply.is_error, "{}", reply.text);
                        log.lock().unwrap().push(reply.text.clone());
                        reply.text
                    }
                };
                let env = check("echo \"$TMPDIR\"; ulimit -Sn".into()).await;
                let tmpdir = env.lines().nth(1).unwrap().to_owned();
                check("echo checked".into()).await;
                check(format!("ulimit -Sn {}; echo checked", limits.soft)).await;
                check(format!("TMPDIR={tmpdir} echo checked")).await;
                // Lower than the limits it inherits (8192 at least), whatever they are: a shell
                // started with both at 8192 would make `ulimit -n 8192` change nothing.
                check("ulimit -n 4096; echo checked".into()).await;
                check("TMPDIR=/tmp/elsewhere echo checked".into()).await;
                let reply = turn
                    .call("submit_report", json!({"summary": "Checked."}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Check it.").await;
    flow.until("the worker's checks", |_| {
        replies.lock().unwrap().len() == 6
    })
    .await;
    flow.settled().await;
    let replies = replies.lock().unwrap().clone();
    let env: Vec<&str> = replies[0].lines().collect();
    assert!(
        env[1].contains("brigadier-test-"),
        "TMPDIR is the task's test folder: {}",
        replies[0]
    );
    let soft: u64 = env[2].parse().unwrap_or(u64::MAX);
    assert!(soft >= 8192, "{}", replies[0]);
    assert_eq!(replies[1], "[exit 0]\nchecked\n");
    for cached in &replies[2..4] {
        assert!(cached.starts_with("[cached: "), "{cached}");
    }
    for fresh in &replies[4..] {
        assert_eq!(fresh, "[exit 0]\nchecked\n");
    }
    // The cached answers are recorded under the command that ran, without the prefix.
    let commands: Vec<String> = flow
        .events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            DomainEvent::CheckRan { command, .. } => Some(command),
            _ => None,
        })
        .collect();
    assert_eq!(commands[1..4], ["echo checked"; 3]);
    flow.stop().await;
}

/// A Tauri app's build wants its sidecar programs beside its config, and they are git-ignored:
/// a task's fresh worktree gets an empty stand-in for each, so `cargo clippy` runs there, and
/// git still sees nothing to commit.
#[tokio::test]
async fn a_task_worktree_gets_a_stand_in_for_each_ignored_sidecar() {
    let seen: Arc<Mutex<Option<(u64, String)>>> = Arc::default();
    let log = seen.clone();
    let flow = Flow::start(
        "sidecars",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("[report task-1") {
                        let reply = turn.call("land_phase", json!({"task": "task-1"})).await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("Noted.");
                    }
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Note the app", "kind": "implement",
                                   "spec": "Add notes.txt.", "provider": "claude"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                let sidecar = turn.cwd.join(format!(
                    "app/src-tauri/binaries/helper-{}{}",
                    super::super::sidecars::host_triple(),
                    std::env::consts::EXE_SUFFIX
                ));
                let size = std::fs::metadata(&sidecar).map_or(u64::MAX, |meta| meta.len());
                *log.lock().unwrap() =
                    Some((size, turn.git(&["status", "--porcelain"]).trim().to_owned()));
                turn.write("notes.txt", "notes\n");
                turn.git(&["add", "notes.txt"]);
                turn.git(&["commit", "-qm", "Add notes"]);
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Added notes.txt.", "changes": ["notes.txt"]}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    let app = flow.repo.join("app/src-tauri");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(
        app.join("tauri.conf.json"),
        r#"{"bundle": {"externalBin": ["binaries/helper"]}}"#,
    )
    .unwrap();
    std::fs::write(
        flow.repo.join(".gitignore"),
        "*.log\n/app/src-tauri/binaries/\n",
    )
    .unwrap();
    super::git(&flow.repo, &["add", "-A"]);
    super::git(&flow.repo, &["commit", "-q", "-m", "Add the app"]);
    flow.say("Add notes.txt.").await;
    flow.until("the worker's look", |_| seen.lock().unwrap().is_some())
        .await;
    flow.settled().await;
    let (size, status) = seen.lock().unwrap().clone().unwrap();
    assert_eq!(size, 0, "an empty stand-in");
    assert_eq!(status, "", "nothing to commit");
    flow.stop().await;
}

/// A thread's checks and commands get its short temporary folder under /tmp as TMPDIR, not
/// its folder in the data folder (a long path with spaces, where a test that binds a Unix
/// socket fails): at each level, in the sandbox that level gives, a socket binds there. macOS
/// only: the worker sandbox the lower levels need isn't there on Linux yet.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn a_threads_checks_and_commands_bind_sockets_in_their_short_temporary_folder() {
    use crate::manager::workers::{SessionTemp, session_temp_dir};
    use crate::model::PermissionLevel;

    const BIND: &str = "printf '%s\\n' \"$TMPDIR\"; python3 -c 'import os, socket; \
        s = socket.socket(socket.AF_UNIX); s.bind(os.path.join(os.environ[\"TMPDIR\"], \"probe.sock\")); \
        print(\"bound\")'; rm -f \"$TMPDIR/probe.sock\"";
    for permission in [
        PermissionLevel::AskForApproval,
        PermissionLevel::ApproveForMe,
        PermissionLevel::FullAccess,
    ] {
        let replies: Arc<Mutex<Vec<String>>> = Arc::default();
        let log = replies.clone();
        let flow = Flow::start(
            "short-tmp",
            Options {
                permission,
                ..Options::default()
            },
            script(move |turn| {
                let log = log.clone();
                async move {
                    for (tool, args) in [
                        ("run_check", json!({ "command": BIND })),
                        ("run", json!({ "command": BIND })),
                    ] {
                        let reply = turn.call(tool, args).await;
                        assert!(!reply.is_error, "{tool}: {}", reply.text);
                        log.lock().unwrap().push(reply.text);
                    }
                    Reply::text("Bound.")
                }
            }),
        )
        .await;
        flow.say("Bind a socket.").await;
        flow.settled().await;
        let tmp = session_temp_dir(&flow.conversation, SessionTemp::Thread);
        let tmp = tmp.to_str().unwrap();
        assert!(
            tmp.starts_with("/tmp/brigadier-test-t") && !tmp.contains(' '),
            "{tmp}"
        );
        let replies = replies.lock().unwrap().clone();
        assert_eq!(replies.len(), 2, "{permission:?}");
        for reply in replies {
            assert_eq!(reply, format!("[exit 0]\n{tmp}\nbound\n"), "{permission:?}");
        }
        // The folder goes with the session.
        flow.manager
            .delete(flow.conversation.clone())
            .await
            .unwrap();
        super::eventually("the temporary folder to go", || {
            !std::path::Path::new(tmp).exists()
        })
        .await;
        flow.stop().await;
    }
}

/// A pass's long output reaches the thread as its warnings and summary, its whole output one
/// read_artifact away; a failure's still comes as the full digest (BUILD-PLAN C1).
#[tokio::test]
async fn a_passing_checks_long_output_comes_short_and_a_failure_whole() {
    let replies: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        "checks-pass-short",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                for command in [
                    "i=0; while [ $i -lt 200 ]; do echo \"test case_$i ... ok\"; i=$((i+1)); done; \
                     echo 'warning: slow test'; echo 'Tests  200 passed (200)'",
                    "i=0; while [ $i -lt 200 ]; do echo \"test case_$i ... ok\"; i=$((i+1)); done; \
                     echo 'FAIL case_7'; exit 1",
                ] {
                    let reply = turn.call("run_check", json!({ "command": command })).await;
                    log.lock().unwrap().push(reply.text);
                }
                Reply::text("Checked.")
            }
        }),
    )
    .await;
    flow.say("Check it.").await;
    flow.settled().await;
    let replies = replies.lock().unwrap().clone();
    let pass = &replies[0];
    assert!(pass.len() < 1_500, "{} {pass}", pass.len());
    assert!(
        pass.starts_with("exit 0 [full output: read_artifact out-"),
        "{pass}"
    );
    assert!(
        pass.contains("\n[lines with a warning]\nwarning: slow test\n"),
        "{pass}"
    );
    assert!(pass.contains("Tests  200 passed (200)"), "{pass}");
    assert!(!pass.contains("case_0 ... ok"), "{pass}");
    let fail = &replies[1];
    assert!(
        fail.starts_with("[exit 1]\n") || fail.starts_with("exit 1"),
        "{fail}"
    );
    assert!(fail.contains("test case_0 ... ok"), "{fail}");
    assert!(fail.contains("FAIL case_7"), "{fail}");
    flow.stop().await;
}
