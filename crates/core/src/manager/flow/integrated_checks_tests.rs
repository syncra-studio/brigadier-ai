//! The session's full checks run on the integrated tree when an agent asks (`run_checks`,
//! SESSION-FIXES-PLAN.md §11), in the session's check checkout. Brigadier never starts them by
//! itself: a restart or an unarchive records a run it cut off, or one still waiting to start,
//! as interrupted, and runs nothing.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn, git};
use crate::model::DomainEvent;
use crate::work::{IntegratedCheck, IntegratedCheckState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// The project's full checks: each run is logged in the repository's git folder (outside
/// every tree, so the check cache never sees it), with its TMPDIR, held while `hold-checks`
/// is there, and fails while the tree has a `broken.txt`.
const FULL_CHECKS: &str = r#"#!/bin/sh
common=$(cd "$(git rev-parse --git-common-dir)" && pwd -P)
echo "start $$ $(pwd -P)" >> "$common/full-checks.runs"
echo "$TMPDIR" >> "$common/full-checks.tmp"
while [ -f "$common/hold-checks" ]; do sleep 0.05; done
if [ -f broken.txt ]; then
  echo "broken.txt must not be in the tree"
  exit 1
fi
echo "end $$" >> "$common/full-checks.runs"
"#;

fn options() -> Options {
    Options {
        files: vec![("tools/full-checks.sh", FULL_CHECKS)],
        ..Options::default()
    }
}

/// The runs' log: `start <pid> <dir>` and `end <pid>` lines.
fn runs(repo: &Path) -> Vec<String> {
    std::fs::read_to_string(repo.join(".git/full-checks.runs"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// A worker that writes `file`, runs its own check (run_check with no command: the checks of
/// its change) and reports; the run_check reply goes to `checked`.
async fn worker(turn: &Turn, file: &str, checked: &Mutex<Vec<String>>) -> Reply {
    turn.write(file, &format!("{file}\n"));
    turn.git(&["add", file]);
    turn.git(&["commit", "-q", "-m", &format!("Add {file}")]);
    let check = turn.call("run_check", json!({})).await;
    checked.lock().unwrap().push(check.text);
    let reply = turn
        .call(
            "submit_report",
            json!({"summary": format!("Added {file}."), "changes": [file]}),
        )
        .await;
    assert!(!reply.is_error, "{}", reply.text);
    Reply::text("Reported.")
}

/// The thread lands each report in its input, at once.
async fn land_reports(turn: &Turn) -> bool {
    let mut landed = false;
    for n in 1..=4 {
        if turn.input.contains(&format!("[report task-{n} ")) {
            let reply = turn
                .call("land_phase", json!({"task": format!("task-{n}")}))
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            assert!(reply.text.starts_with("Landed "), "{}", reply.text);
            landed = true;
        }
    }
    landed
}

/// Commits `file` in the thread's workspace, as the thread's own edit; its new tip.
fn thread_commit(turn: &Turn, file: &str, text: &str) -> String {
    let workspace = &turn.add_dirs[0];
    std::fs::write(workspace.join(file), text).unwrap();
    git(workspace, &["add", file]);
    git(workspace, &["commit", "-q", "-m", &format!("Add {file}")]);
    git(workspace, &["rev-parse", "HEAD"])
}

/// What a frontend change's run tells its agents with computer use `on` or off in Settings: the
/// thread's instructions, the UI worker's brief, and the pass of the run_checks the thread calls
/// after the landing.
async fn ui_change_with_computer_use(name: &str, on: bool) -> (String, String, String) {
    let checked: Arc<Mutex<Vec<String>>> = Arc::default();
    // The thread's instructions, the worker's brief, the pass.
    let told: Arc<Mutex<[String; 3]>> = Arc::default();
    let (seen, log) = (checked.clone(), told.clone());
    let flow = Flow::start(
        name,
        options(),
        script(move |turn| {
            let (seen, log) = (seen.clone(), log.clone());
            async move {
                if !turn.is_orchestrator() {
                    log.lock().unwrap()[1] = turn.prompt.clone();
                    return worker(&turn, "sidebar.css", &seen).await;
                }
                log.lock().unwrap()[0] = turn.prompt.clone();
                if turn.input.contains("[report task-1 ") {
                    let reply = turn.call("land_phase", json!({"task": "task-1"})).await;
                    assert!(reply.text.starts_with("Landed "), "{}", reply.text);
                    let checks = turn.call("run_checks", json!({})).await;
                    assert!(
                        checks.text.contains("[checks passed at "),
                        "{}",
                        checks.text
                    );
                    log.lock().unwrap()[2] = checks.text;
                    return Reply::text("The sidebar is restyled.");
                }
                if turn.input.starts_with("Restyle the sidebar") {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Restyle the sidebar",
                                   "kind": "implement", "areas": ["frontend"],
                                   "spec": "Restyle the sidebar in sidebar.css."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    let mut settings = flow.core.settings();
    settings.computer_use = on;
    flow.core.update_settings(settings).await.unwrap();
    flow.say("Restyle the sidebar.").await;
    flow.until("the pass to reach the thread", |_| {
        !told.lock().unwrap()[2].is_empty()
    })
    .await;
    flow.settled().await;
    flow.stop().await;
    let [thread, brief, passed] = told.lock().unwrap().clone();
    (thread, brief, passed)
}

/// Computer use off routes nothing to a live look (#24): the thread isn't told workers can use
/// apps, a pass asks for no live look, and the UI worker's scripted checks are the whole check.
/// With it on, the same run gets all three. The live look is this Mac's alone.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn computer_use_off_routes_nothing_to_a_live_look() {
    use crate::manager::prompts::{COMPUTER_ROUTING, scripted_ui_note};
    let (thread, brief, passed) = ui_change_with_computer_use("cu-off-no-look", false).await;
    assert!(!thread.contains(COMPUTER_ROUTING), "{thread}");
    assert!(!passed.contains("live_look"), "{passed}");
    assert!(brief.contains(&scripted_ui_note(false)), "{brief}");
    assert!(!brief.contains("one live look"), "{brief}");
    let (thread, brief, passed) = ui_change_with_computer_use("cu-on-look", true).await;
    assert!(thread.contains(COMPUTER_ROUTING), "{thread}");
    assert!(passed.contains("look at them live once"), "{passed}");
    assert!(passed.contains("task-1"), "{passed}");
    assert!(
        brief.contains("Don't build or launch the app itself"),
        "{brief}"
    );
}

/// Phase B: the check checkout runs the app as the session's worktree does: it gets the user's
/// checkout's gitignored `.env` and what `.worktreeinclude` names.
#[tokio::test]
async fn the_check_checkout_gets_the_checkouts_env_files() {
    const NEEDS_ENV: &str = r#"#!/bin/sh
grep -qx 'APP_KEY=k-1' .env || { echo "no .env"; exit 1; }
test -f local/config.json || { echo "no local/config.json"; exit 1; }
"#;
    let flow = Flow::start(
        "integrated-env",
        Options {
            files: vec![
                ("tools/full-checks.sh", NEEDS_ENV),
                (".gitignore", ".env\nlocal/\n"),
                (".worktreeinclude", "local/config.json\n"),
            ],
            ..Options::default()
        },
        script(move |turn| async move {
            if turn.is_orchestrator() && turn.input.starts_with("Build it") {
                thread_commit(&turn, "one.txt", "one\n");
                let checks = turn.call("run_checks", json!({})).await;
                assert!(!checks.is_error, "{}", checks.text);
                return Reply::text("Built one.");
            }
            Reply::text("[quiet]")
        }),
    )
    .await;
    std::fs::write(flow.repo.join(".env"), "APP_KEY=k-1\n").unwrap();
    std::fs::create_dir_all(flow.repo.join("local")).unwrap();
    std::fs::write(flow.repo.join("local/config.json"), "{}\n").unwrap();
    flow.say("Build it.").await;
    let board = flow
        .until("the full checks to end", |board| {
            board
                .integrated_checks
                .values()
                .any(|check| check.state.is_final())
        })
        .await;
    let checks: Vec<_> = board.integrated_checks.values().collect();
    assert!(
        matches!(checks[0].state, IntegratedCheckState::Passed { .. }),
        "{checks:#?}"
    );
    flow.stop().await;
}

/// Whether a worker ran it and why its cache was left out, for each check record of a command
/// containing `word`.
async fn check_records(flow: &Flow, word: &str) -> Vec<(bool, Option<String>)> {
    flow.events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            DomainEvent::CheckRan {
                task_id,
                command,
                bypassed,
                ..
            } if command.contains(word) => Some((task_id.is_some(), bypassed)),
            _ => None,
        })
        .collect()
}

/// #291: a one-off command the thread checks its change with (a `grep`) is no check of the
/// project's: the full checks of a later request never run it, so a revert that removes what
/// it looked for can't fail them for good.
#[tokio::test]
async fn the_full_checks_never_run_a_command_the_thread_checked_one_change_with() {
    let flow = Flow::start(
        "integrated-own-checks-only",
        Options::default(),
        script(move |turn| async move {
            if !turn.is_orchestrator() {
                return Reply::text("[quiet]");
            }
            if turn.input.starts_with("Add one") {
                thread_commit(&turn, "one.txt", "hello\n");
                let check = turn
                    .call("run_check", json!({"command": "grep -q hello one.txt"}))
                    .await;
                assert!(!check.is_error, "{}", check.text);
                let checks = turn.call("run_checks", json!({})).await;
                assert!(!checks.is_error, "{}", checks.text);
                return Reply::text("Added one.");
            }
            if turn.input.starts_with("Add two") {
                thread_commit(&turn, "two.txt", "two\n");
                let checks = turn.call("run_checks", json!({})).await;
                assert!(!checks.is_error, "{}", checks.text);
                return Reply::text("Added two.");
            }
            Reply::text("[quiet]")
        }),
    )
    .await;
    let ended = |n: usize| {
        move |board: &crate::board::Board| {
            board
                .integrated_checks
                .values()
                .filter(|check| check.state.is_final())
                .count()
                >= n
        }
    };
    flow.say("Add one.").await;
    flow.until("the first full checks to end", ended(1)).await;
    flow.say("Add two.").await;
    let board = flow.until("the second full checks to end", ended(2)).await;
    for check in board.integrated_checks.values() {
        if let IntegratedCheckState::Passed { commands } = &check.state {
            assert!(
                commands.iter().all(|command| !command.contains("grep")),
                "{commands:?}"
            );
        }
    }
    assert_eq!(check_records(&flow, "grep").await, [(false, None)]);
    flow.stop().await;
}

/// #291: the full checks' runs are kept like any other check's: their checkout's path is
/// taken as git names it, so its workdir is never "outside its repository".
#[tokio::test]
async fn the_full_checks_results_are_kept() {
    let flow = Flow::start(
        "integrated-kept",
        options(),
        script(move |turn| async move {
            if turn.is_orchestrator() && turn.input.starts_with("Build it") {
                thread_commit(&turn, "one.txt", "one\n");
                let checks = turn.call("run_checks", json!({})).await;
                assert!(!checks.is_error, "{}", checks.text);
                return Reply::text("Built one.");
            }
            Reply::text("[quiet]")
        }),
    )
    .await;
    flow.say("Build it.").await;
    flow.until("the full checks to end", |board| {
        board
            .integrated_checks
            .values()
            .any(|check| check.state.is_final())
    })
    .await;
    assert_eq!(check_records(&flow, "full-checks").await, [(false, None)]);
    flow.stop().await;
}

/// A full check of the session branch's tip, as `state`, Brigadier's own from before
/// (`asked`: none) or one `run_checks` asked for.
async fn record_check(flow: &Flow, number: u32, state: IntegratedCheckState, asked: bool) {
    let tip = git(&flow.repo, &["rev-parse", "main"]);
    let check = IntegratedCheck {
        id: format!("check-{number}"),
        conversation_id: flow.conversation.clone(),
        repo: flow.repo.to_string_lossy().into_owned(),
        branch: "main".into(),
        tip: tip.trim().to_owned(),
        state,
        due_at_ms: crate::now_ms(),
        started_at_ms: Some(crate::now_ms()),
        ended_at_ms: None,
        requests: Vec::new(),
        tasks: Vec::new(),
        rerun: false,
        asked: asked.then_some(crate::work::ChecksAsked {
            task_id: None,
            scope: crate::work::ChecksScope::Full,
        }),
        owed: false,
    };
    flow.core
        .record_conversation(
            &flow.conversation,
            vec![DomainEvent::IntegratedCheckUpdated { check }],
        )
        .await
        .unwrap();
}

fn running() -> IntegratedCheckState {
    IntegratedCheckState::Running {
        command: None,
        done: 0,
        total: 1,
    }
}

/// The reason each check of the board was interrupted with, by id; a check that wasn't
/// interrupted is missing.
fn interrupted(board: &crate::board::Board) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = board
        .integrated_checks
        .values()
        .filter_map(|check| match &check.state {
            IntegratedCheckState::Interrupted { reason } => {
                Some((check.id.clone(), reason.clone()))
            }
            _ => None,
        })
        .collect();
    found.sort();
    found
}

/// An upgrade from when Brigadier ran the full checks by itself: the old automatic run the
/// quit cut off, one still due, and a run_checks run cut off are all recorded interrupted at
/// the start, with a plain reason, and none of them runs again.
#[tokio::test]
async fn a_restart_runs_no_full_checks_again_and_records_them_interrupted() {
    let mut flow = Flow::start(
        "integrated-restart-interrupts",
        options(),
        script(|_turn| async { Reply::text("[quiet]") }),
    )
    .await;
    record_check(&flow, 1, running(), false).await;
    record_check(&flow, 2, IntegratedCheckState::Due, false).await;
    record_check(&flow, 3, running(), true).await;
    flow.restart().await;
    let board = flow
        .until("the checks to be recorded interrupted", |board| {
            interrupted(board).len() == 3
        })
        .await;
    assert_eq!(
        interrupted(&board),
        [
            (
                "check-1".to_owned(),
                "Brigadier quit while they ran: call run_checks again".to_owned()
            ),
            (
                "check-2".to_owned(),
                "Brigadier quit before they started: call run_checks again".to_owned()
            ),
            (
                "check-3".to_owned(),
                "Brigadier quit while they ran: call run_checks again".to_owned()
            ),
        ]
    );
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(runs(&flow.repo).is_empty(), "{:?}", runs(&flow.repo));
    assert_eq!(interrupted(&flow.board().await).len(), 3);
    flow.stop().await;
}

/// An unarchive starts nothing either: a check the archive cut off is recorded interrupted.
#[tokio::test]
async fn an_unarchive_runs_no_full_checks_again() {
    let flow = Flow::start(
        "integrated-unarchive-interrupts",
        options(),
        script(|_turn| async { Reply::text("[quiet]") }),
    )
    .await;
    flow.manager
        .archive(flow.conversation.clone())
        .await
        .unwrap();
    record_check(&flow, 1, running(), true).await;
    record_check(&flow, 2, IntegratedCheckState::Due, false).await;
    flow.manager
        .restore(flow.conversation.clone())
        .await
        .unwrap();
    let board = flow.board().await;
    assert_eq!(
        interrupted(&board),
        [
            (
                "check-1".to_owned(),
                "The session was archived while they ran: call run_checks again".to_owned()
            ),
            (
                "check-2".to_owned(),
                "The session was archived before they started: call run_checks again".to_owned()
            ),
        ]
    );
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(runs(&flow.repo).is_empty(), "{:?}", runs(&flow.repo));
    flow.stop().await;
}
