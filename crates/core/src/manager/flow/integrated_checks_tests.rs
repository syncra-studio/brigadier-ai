//! The session's full checks run once on the integrated tree (SESSION-FIXES-PLAN.md §11):
//! workers check only their own change and land at once; the project's full checks run once
//! per tip of the session branch, in the session's check checkout, after the landings; a
//! failure goes back to the worker whose change broke it; a merge waits for a pass at the tip
//! it merges; a restart runs a cut-off check again; an archive ends it. Nobody is asked how
//! much to check.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn, git};
use crate::model::DomainEvent;
use crate::work::{IntegratedCheckState, OrchestratorStepKind, ReviewState, TaskState};

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

/// The folders the full checks started in, in order.
fn started_in(repo: &Path) -> Vec<String> {
    runs(repo)
        .iter()
        .filter_map(|line| line.strip_prefix("start "))
        .map(|rest| rest.split_once(' ').unwrap().1.to_owned())
        .collect()
}

/// 60 lines naming `file`: three such files cross the review line (Q48).
fn notes(file: &str) -> String {
    (1..=60).map(|n| format!("{file} {n}\n")).collect()
}

fn hold(repo: &Path) -> PathBuf {
    repo.join(".git/hold-checks")
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

/// The thread lands the first two workers' reports together, in the turn that hears the second,
/// so both landings fall in one settle window however loaded the machine is (lands spread over
/// two turns were seconds apart under load, and each got a check of its own); a later report
/// lands at once.
async fn land_pair(turn: &Turn, reported: &Mutex<Vec<u32>>) -> bool {
    let pair: Vec<u32> = (1..=2)
        .filter(|n| turn.input.contains(&format!("[report task-{n} ")))
        .collect();
    if pair.is_empty() {
        return land_reports(turn).await;
    }
    let both = {
        let mut reported = reported.lock().unwrap();
        reported.extend(pair);
        reported.sort_unstable();
        reported.dedup();
        reported.len() == 2
    };
    if both {
        for n in 1..=2 {
            let reply = turn
                .call("land_phase", json!({"task": format!("task-{n}")}))
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            assert!(reply.text.starts_with("Landed "), "{}", reply.text);
        }
    }
    true
}

async fn delegate(turn: &Turn, title: &str, spec: &str) {
    let reply = turn
        .call(
            "delegate_task",
            json!({"effort": "high", "title": title, "kind": "implement", "spec": spec}),
        )
        .await;
    assert!(!reply.is_error, "{}", reply.text);
}

/// The session branch and its tip.
async fn session_tip(flow: &Flow) -> (String, String) {
    let board = flow.board().await;
    let branch = Flow::task(&board, 1)
        .workspace
        .as_ref()
        .and_then(|w| w.target.clone())
        .unwrap();
    let tip = git(&flow.repo, &["rev-parse", &branch]);
    (branch, tip)
}

/// Two workers in parallel: each checks only its own change in its worktree and lands at
/// once; the full checks run once, after both landings, in the check checkout, at the
/// integrated tip; that tip is never checked again, not even after a restart. No card asks
/// how much to check.
#[tokio::test]
async fn parallel_workers_land_at_once_and_the_integrated_tree_is_checked_once() {
    let checked: Arc<Mutex<Vec<String>>> = Arc::default();
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let reported: Arc<Mutex<Vec<u32>>> = Arc::default();
    let (seen, heard) = (checked.clone(), inputs.clone());
    let mut flow = Flow::start(
        "integrated-once",
        options(),
        script(move |turn| {
            let (seen, heard, reported) = (seen.clone(), heard.clone(), reported.clone());
            async move {
                if !turn.is_orchestrator() {
                    let file = if turn.task_number() == Some(1) {
                        "a.txt"
                    } else {
                        "b.txt"
                    };
                    return worker(&turn, file, &seen).await;
                }
                heard.lock().unwrap().push(turn.input.clone());
                if land_pair(&turn, &reported).await {
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[checks passed at ") {
                    return Reply::text("Both files are in, and the full checks pass.");
                }
                if turn.input.starts_with("Add a.txt") {
                    delegate(&turn, "Add a", "Create a.txt.").await;
                    delegate(&turn, "Add b", "Create b.txt.").await;
                    return Reply::text("[quiet]");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    // Landings within a few seconds of each other make one due check.
    *flow.manager.integrated.settle.lock().unwrap() = Some(Duration::from_secs(3));
    flow.say("Add a.txt and b.txt.").await;
    flow.until("the pass to reach the thread", |_| {
        inputs
            .lock()
            .unwrap()
            .iter()
            .any(|input| input.contains("[checks passed at "))
    })
    .await;
    let board = flow.settled().await;
    // Workers ran only their own checks: the full script never ran in a worker's worktree.
    let checked = checked.lock().unwrap().clone();
    assert_eq!(checked.len(), 2, "{checked:#?}");
    assert!(
        checked
            .iter()
            .all(|reply| !reply.contains("full-checks.sh")),
        "{checked:#?}"
    );
    let dirs = started_in(&flow.repo);
    assert_eq!(dirs.len(), 1, "one run for both landings: {dirs:#?}");
    assert!(
        dirs[0].ends_with("/tree") && dirs[0].contains("/checks/"),
        "the session's check checkout: {dirs:#?}"
    );
    // Its temporary files go to the full checks' own short folder, never the data folder's.
    let tmp = crate::manager::workers::session_temp_dir(
        &flow.conversation,
        crate::manager::workers::SessionTemp::Checks,
    );
    assert_eq!(
        std::fs::read_to_string(flow.repo.join(".git/full-checks.tmp")).unwrap(),
        format!("{}\n", tmp.display())
    );
    for task in board.tasks.values() {
        let worktree = task.workspace.as_ref().unwrap().worktree.clone().unwrap();
        assert!(!dirs[0].starts_with(&worktree), "{dirs:#?} {worktree}");
    }
    // Both landed before the full checks started.
    let events = flow.events().await;
    let landed = events
        .iter()
        .enumerate()
        .filter(|(_, event)| {
            matches!(event, DomainEvent::OrchestratorStepped { step }
                if matches!(step.kind, OrchestratorStepKind::Landed { .. }))
        })
        .map(|(at, _)| at)
        .collect::<Vec<_>>();
    let running = events
        .iter()
        .position(|event| {
            matches!(event, DomainEvent::IntegratedCheckUpdated { check }
                if matches!(check.state, IntegratedCheckState::Running { .. }))
        })
        .unwrap();
    assert_eq!(landed.len(), 2);
    assert!(
        landed.iter().all(|at| *at < running),
        "{landed:?} {running}"
    );
    // One check, passed at the integrated tip, covering both tasks.
    let (branch, tip) = session_tip(&flow).await;
    let checks: Vec<_> = board.integrated_checks.values().collect();
    assert_eq!(checks.len(), 1, "{checks:#?}");
    assert_eq!(
        (checks[0].branch.as_str(), checks[0].tip.as_str()),
        (branch.as_str(), tip.as_str())
    );
    assert!(matches!(
        checks[0].state,
        IntegratedCheckState::Passed { .. }
    ));
    let numbers: Vec<u32> = checks[0]
        .tasks
        .iter()
        .map(|task| board.tasks[task].number)
        .collect();
    assert_eq!(numbers, vec![1, 2]);
    assert!(
        board
            .tasks
            .values()
            .all(|task| task.state == TaskState::Landed)
    );
    // No card, and no question about how much to check.
    assert!(board.approvals.is_empty(), "{:#?}", board.approvals);
    // The same tip made due again runs nothing; nor does a restart.
    flow.manager
        .checks_due(
            &flow.conversation,
            &flow.repo,
            &branch,
            &brigadier_git::Oid(tip.clone()),
            None,
        )
        .await;
    flow.restart().await;
    flow.say("Anything else?").await;
    let board = flow.settled().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(board.integrated_checks.len(), 1);
    assert_eq!(started_in(&flow.repo).len(), 1, "{:#?}", runs(&flow.repo));
    flow.stop().await;
}

/// A worker's change breaks the full checks: the thread hears the failing command, its output
/// and the task whose change it was; the fix goes back to that worker's own session. The
/// merge the user asked for waits only for that writer, never for the checks (#241), and says
/// where they stand; a failed tip reads as failed.
#[tokio::test]
async fn a_failure_goes_back_to_the_worker_that_broke_it_and_the_merge_states_it() {
    #[derive(Default)]
    struct Seen {
        inputs: Vec<String>,
        sessions: Vec<(u32, String)>,
        fix_session: Option<String>,
        merges: Vec<String>,
    }
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let reported: Arc<Mutex<Vec<u32>>> = Arc::default();
    let log = seen.clone();
    let flow = Flow::start(
        "integrated-fail",
        Options {
            // The CLIs keep session files, so a landed worker's session can be resumed.
            behavior: Arc::new(super::FakeBehavior {
                cleanup: true,
                ..Default::default()
            }),
            ..options()
        },
        script(move |turn| {
            let (log, reported) = (log.clone(), reported.clone());
            async move {
                if turn.is_review() {
                    return Reply::text("No findings.");
                }
                if !turn.is_orchestrator() {
                    let checked = Mutex::default();
                    if turn.input.contains("Remove broken.txt") {
                        log.lock().unwrap().fix_session = Some(turn.native_id.clone());
                        turn.git(&["rm", "-q", "broken.txt"]);
                        turn.git(&["commit", "-q", "-m", "Remove broken.txt"]);
                        let reply = turn
                            .call(
                                "submit_report",
                                json!({"summary": "Removed broken.txt.", "changes": ["broken.txt"]}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("Reported.");
                    }
                    let n = turn.task_number().unwrap();
                    log.lock()
                        .unwrap()
                        .sessions
                        .push((n, turn.native_id.clone()));
                    let file = if n == 1 { "a.txt" } else { "broken.txt" };
                    return worker(&turn, file, &checked).await;
                }
                log.lock().unwrap().inputs.push(turn.input.clone());
                if land_pair(&turn, &reported).await {
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[checks failed at ") {
                    // The landings' reviews end first: what the merge says is the checks.
                    let deadline = std::time::Instant::now() + *super::PATIENCE;
                    loop {
                        let board = turn.host.core.board(&turn.conversation().unwrap()).await;
                        if board.unwrap().reviews.values().all(|review| {
                            review.state != crate::work::ReviewState::Running
                        }) {
                            break;
                        }
                        assert!(std::time::Instant::now() < deadline);
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Fix the checks",
                                   "kind": "implement", "role": "fix", "subject": "task-2",
                                   "spec": "Remove broken.txt: the full checks fail on it."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    // The merge waits for the fix's writer, as the thread asks, not for the
                    // checks.
                    let merge = turn
                        .call(
                            "finish_session",
                            json!({"user_words": "merge it", "when": "workers_finish"}),
                        )
                        .await;
                    assert!(!merge.is_error, "{}", merge.text);
                    log.lock().unwrap().merges.push(merge.text);
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[merge due]") {
                    let merge = turn
                        .call("finish_session", json!({"user_words": "merge it"}))
                        .await;
                    log.lock().unwrap().merges.push(merge.text.clone());
                    assert!(merge.text.contains("[finished]"), "{}", merge.text);
                    return Reply::text("Merged.");
                }
                if turn.input.starts_with("Add a.txt") {
                    delegate(&turn, "Add a", "Create a.txt.").await;
                    delegate(&turn, "Add broken", "Create broken.txt.").await;
                    return Reply::text("[quiet]");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    *flow.manager.integrated.settle.lock().unwrap() = Some(Duration::from_secs(3));
    flow.say("Add a.txt and broken.txt, and merge it.").await;
    let log = seen.clone();
    flow.until(
        "the merge and the checks passing after the fix",
        move |board| {
            log.lock()
                .unwrap()
                .merges
                .iter()
                .any(|text| text.contains("[finished]"))
                && board
                    .integrated_checks
                    .values()
                    .any(|check| matches!(check.state, IntegratedCheckState::Passed { .. }))
        },
    )
    .await;
    let board = flow.settled().await;
    let seen = std::mem::take(&mut *seen.lock().unwrap());
    // The failure names its command, its output and the task that landed it.
    let failed = seen
        .inputs
        .iter()
        .find(|input| input.contains("[checks failed at "))
        .unwrap();
    assert!(failed.contains("full-checks.sh"), "{failed}");
    assert!(
        failed.contains("broken.txt must not be in the tree"),
        "{failed}"
    );
    assert!(
        failed.contains("task-2 \"Add broken\" (changed broken.txt)"),
        "{failed}"
    );
    assert!(failed.contains("role fix"), "{failed}");
    // The merge was held for the fix's writer only, then went ahead saying where the checks
    // stood at the tip it merged.
    assert!(
        seen.merges[0].contains("[merge held]"),
        "{:#?}",
        seen.merges
    );
    assert!(
        seen.merges[1].contains("[finished]")
            && seen.merges[1].contains("The session's full checks: "),
        "{:#?}",
        seen.merges
    );
    // The fix resumed task-2's own session.
    let broke = seen
        .sessions
        .iter()
        .find(|(n, _)| *n == 2)
        .map(|(_, id)| id.clone());
    assert_eq!(seen.fix_session, broke);
    // Two runs: the failure, then the pass at the fixed tip.
    let dirs = started_in(&flow.repo);
    assert_eq!(dirs.len(), 2, "{:#?}", runs(&flow.repo));
    let mut checks: Vec<_> = board.integrated_checks.values().collect();
    checks.sort_by_key(|check| crate::manager::integrated_check_number(&check.id));
    assert!(matches!(
        checks[0].state,
        IntegratedCheckState::Failed { .. }
    ));
    assert!(matches!(
        checks.last().unwrap().state,
        IntegratedCheckState::Passed { .. }
    ));
    assert!(board.approvals.is_empty(), "{:#?}", board.approvals);
    let main = git(&flow.repo, &["ls-tree", "-r", "--name-only", "main"]);
    assert!(
        main.contains("a.txt") && !main.contains("broken.txt"),
        "{main}"
    );
    // A merge of the failed tip would have said so.
    let failed = flow
        .manager
        .checks_line(
            &flow.conversation,
            &checks[0].branch,
            &brigadier_git::Oid(checks[0].tip.clone()),
        )
        .await;
    assert!(
        failed.contains("The session's full checks: failed at")
            && failed.contains("full-checks.sh"),
        "{failed}"
    );
    flow.stop().await;
}

/// A merge asked while the checks run goes ahead and says they still run (#241); a restart
/// cuts the run off and runs it again, and it passes. Another session archived mid-run ends its run: its process
/// stops, its checkout goes, and the thread hears nothing of it; restored, it runs again.
#[tokio::test]
async fn a_run_cut_off_by_a_restart_runs_again_and_an_archive_ends_it() {
    let merges: Arc<Mutex<Vec<String>>> = Arc::default();
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let (log, heard) = (merges.clone(), inputs.clone());
    let mut flow = Flow::start(
        "integrated-restart",
        options(),
        script(move |turn| {
            let (log, heard) = (log.clone(), heard.clone());
            async move {
                if turn.is_review() {
                    return Reply::text("No findings.");
                }
                if !turn.is_orchestrator() {
                    let file = if turn.input.contains("Create c.txt") {
                        "c.txt"
                    } else {
                        "a.txt"
                    };
                    return worker(&turn, file, &Mutex::default()).await;
                }
                heard.lock().unwrap().push(turn.input.clone());
                if land_reports(&turn).await {
                    // Only the first session's user asked for a merge.
                    if turn
                        .input
                        .contains("[report task-1 · Implement · \"Add a\"")
                    {
                        let merge = turn
                            .call("finish_session", json!({"user_words": "merge it"}))
                            .await;
                        log.lock().unwrap().push(merge.text);
                    }
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[merge due]") {
                    let merge = turn
                        .call("finish_session", json!({"user_words": "merge it"}))
                        .await;
                    log.lock().unwrap().push(merge.text);
                    return Reply::text("Merged.");
                }
                if turn.input.starts_with("Add a.txt") {
                    delegate(&turn, "Add a", "Create a.txt.").await;
                    return Reply::text("[quiet]");
                }
                if turn.input.starts_with("Add c.txt") {
                    delegate(&turn, "Add c", "Create c.txt.").await;
                    return Reply::text("[quiet]");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    std::fs::write(hold(&flow.repo), "").unwrap();
    flow.say("Add a.txt, and merge it.").await;
    // The merge went ahead while the run went on.
    let log = merges.clone();
    flow.until("the merge", move |_| !log.lock().unwrap().is_empty())
        .await;
    {
        let merges = merges.lock().unwrap();
        assert!(
            merges[0].contains("[finished]") && merges[0].contains("The session's full checks: "),
            "{merges:#?}"
        );
    }
    let repo = flow.repo.clone();
    super::eventually("the run", || started_in(&repo).len() == 1).await;
    // A restart cuts the run off: it runs again, and passes.
    flow.restart().await;
    let repo = flow.repo.clone();
    super::eventually("the run again", || started_in(&repo).len() == 2).await;
    std::fs::remove_file(hold(&flow.repo)).unwrap();
    flow.until("the pass", |board| {
        board
            .integrated_checks
            .values()
            .any(|check| matches!(check.state, IntegratedCheckState::Passed { .. }))
    })
    .await;
    let board = flow.settled().await;
    let passed = board
        .integrated_checks
        .values()
        .filter(|check| matches!(check.state, IntegratedCheckState::Passed { .. }))
        .count();
    assert_eq!(passed, 1, "{:#?}", board.integrated_checks);
    // The merge went ahead while they ran: their checkout goes once they end.
    let line = runs(&flow.repo)
        .into_iter()
        .rfind(|line| line.starts_with("start "))
        .unwrap();
    let tree = PathBuf::from(line.splitn(3, ' ').nth(2).unwrap());
    super::eventually("the merged session's check checkout to go", || {
        !tree.exists()
    })
    .await;

    // Another session, archived while its checks run.
    let other = flow
        .add_session("Other", None, brigadier_providers::ProviderKind::Claude)
        .await;
    std::fs::write(hold(&flow.repo), "").unwrap();
    let before = started_in(&flow.repo).len();
    flow.say_to(&other, "Add c.txt.").await;
    let repo = flow.repo.clone();
    super::eventually("its run", || started_in(&repo).len() == before + 1).await;
    let line = runs(&flow.repo)
        .into_iter()
        .rfind(|line| line.starts_with("start "))
        .unwrap();
    let mut parts = line.splitn(3, ' ').skip(1);
    let pid: i32 = parts.next().unwrap().parse().unwrap();
    let tree = PathBuf::from(parts.next().unwrap());
    let heard_before = inputs.lock().unwrap().len();
    flow.manager.archive(other.clone()).await.unwrap();
    super::eventually("its process to stop", || {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err()
    })
    .await;
    super::eventually("its checkout to go", || !tree.exists()).await;
    std::fs::remove_file(hold(&flow.repo)).unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let heard = inputs.lock().unwrap()[heard_before..].to_vec();
    assert!(
        heard.iter().all(|input| !input.contains("[checks ")),
        "{heard:#?}"
    );
    assert!(
        !runs(&flow.repo).contains(&format!("end {pid}")),
        "{:#?}",
        runs(&flow.repo)
    );
    // Restored, the check its archive cut off runs again, to its end.
    let before = started_in(&flow.repo).len();
    flow.manager.restore(other.clone()).await.unwrap();
    super::eventually("its run again", || started_in(&repo).len() == before + 1).await;
    flow.until_in(&other, "its pass", |board| {
        board
            .integrated_checks
            .values()
            .any(|check| matches!(check.state, IntegratedCheckState::Passed { .. }))
    })
    .await;
    flow.stop().await;
}

/// A pass queued while a later turn runs is needed only if that turn still defers its answer.
async fn pass_during_a_later_turn(quiet: bool) {
    let first = Arc::new(tokio::sync::Notify::new());
    let later = Arc::new(tokio::sync::Notify::new());
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let (first_gate, later_gate, heard) = (first.clone(), later.clone(), inputs.clone());
    let flow = Flow::start(
        "integrated-mid-turn",
        options(),
        script(move |turn| {
            let (first, later, heard) = (first_gate.clone(), later_gate.clone(), heard.clone());
            async move {
                heard.lock().unwrap().push(turn.input.clone());
                if turn.input.starts_with("Wait for checks") {
                    first.notified().await;
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("Continue the answer") {
                    later.notified().await;
                    return Reply::text(if quiet {
                        "[quiet]"
                    } else {
                        "The work is complete."
                    });
                }
                Reply::text("The checks pass.")
            }
        }),
    )
    .await;
    std::fs::write(hold(&flow.repo), "").unwrap();
    flow.say("Wait for checks.").await;
    let board = flow
        .until("the first turn", |_| !inputs.lock().unwrap().is_empty())
        .await;
    let request = board.latest_request().unwrap().id.clone();
    let tip = brigadier_git::Oid(git(&flow.repo, &["rev-parse", "main"]));
    flow.manager
        .checks_due(
            &flow.conversation,
            &flow.repo,
            "main",
            &tip,
            Some(request.clone()),
        )
        .await;
    first.notify_one();
    flow.until("the first turn deferring to checks", |board| {
        board.run == crate::work::RunState::Idle
            && board.requests[&request].state == crate::work::RequestState::Working
    })
    .await;
    // A check of a commit starts once its request settles (BUILD-PLAN B1): it runs before
    // the later turn of the same request starts.
    flow.until("the check to run", |board| {
        board
            .integrated_checks
            .values()
            .any(|check| matches!(check.state, IntegratedCheckState::Running { .. }))
    })
    .await;
    flow.manager
        .deliver_for(
            &flow.conversation,
            crate::manager::conversation::Envelope {
                kind: crate::work::InjectionKind::Decision,
                label: "continue".into(),
                task_id: None,
                text: "Continue the answer".into(),
            },
            Some(request.clone()),
        )
        .await;
    flow.until("the later turn", |_| {
        inputs
            .lock()
            .unwrap()
            .iter()
            .any(|input| input.contains("Continue the answer"))
    })
    .await;
    std::fs::remove_file(hold(&flow.repo)).unwrap();
    // Wait for delivery, not only the stored verdict: it is in the inbox before this turn ends.
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    let deadline = tokio::time::Instant::now() + *super::PATIENCE;
    while !conv.request_activity().await.carried.contains(&request) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "pass was not queued"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    later.notify_one();
    flow.settled().await;
    let heard = inputs.lock().unwrap().clone();
    assert_eq!(
        heard
            .iter()
            .filter(|input| input.contains("[checks passed at "))
            .count(),
        usize::from(quiet),
        "{heard:#?}"
    );
    assert_eq!(heard.len(), if quiet { 3 } else { 2 }, "{heard:#?}");
    flow.stop().await;
}

#[tokio::test]
async fn a_pass_during_a_turn_that_answers_does_not_start_an_extra_turn() {
    pass_during_a_later_turn(false).await;
}

#[tokio::test]
async fn a_pass_during_a_turn_that_still_defers_delivers_once() {
    pass_during_a_later_turn(true).await;
}

/// Independent requests can wait on the same run; its pass answers each request once.
#[tokio::test]
async fn one_check_delivers_its_pass_to_each_waiting_request_once() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let inputs: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let (turn_gate, heard) = (gate.clone(), inputs.clone());
    let flow = Flow::start(
        "integrated-two-requests",
        options(),
        script(move |turn| {
            let (gate, heard) = (turn_gate.clone(), heard.clone());
            async move {
                let request = turn
                    .host
                    .request_for(&turn.conversation().unwrap(), None)
                    .await
                    .unwrap();
                heard.lock().unwrap().push((request, turn.input.clone()));
                if turn.input.starts_with("Wait") {
                    gate.notified().await;
                    Reply::text("[quiet]")
                } else {
                    Reply::text("The checks pass.")
                }
            }
        }),
    )
    .await;
    std::fs::write(hold(&flow.repo), "").unwrap();
    let tip = brigadier_git::Oid(git(&flow.repo, &["rev-parse", "main"]));
    let mut requests = Vec::new();
    for number in 1..=2 {
        // These are separate user requests, already routed as independent by the thread.
        let message = flow
            .core
            .append_user_message(
                flow.conversation.clone(),
                format!("Wait {number}"),
                Vec::new(),
                Vec::new(),
            )
            .await
            .unwrap();
        let request = message.request_id.clone().unwrap();
        let conv = flow.manager.conv(&flow.conversation).unwrap();
        flow.manager.prepare_proposal_turn(&conv, message).await;
        flow.until("the request's turn", |_| {
            inputs.lock().unwrap().iter().any(|(id, _)| id == &request)
        })
        .await;
        flow.manager
            .checks_due(
                &flow.conversation,
                &flow.repo,
                "main",
                &tip,
                Some(request.clone()),
            )
            .await;
        gate.notify_one();
        flow.until("the request deferring", |board| {
            board.run == crate::work::RunState::Idle
                && board.requests[&request].state == crate::work::RequestState::Working
        })
        .await;
        requests.push(request);
    }
    std::fs::remove_file(hold(&flow.repo)).unwrap();
    let board = flow.settled().await;
    assert_eq!(board.integrated_checks.len(), 1);
    let check = board.integrated_checks.values().next().unwrap();
    assert_eq!(check.requests, requests);
    let heard = inputs.lock().unwrap().clone();
    for request in &requests {
        assert_eq!(
            heard
                .iter()
                .filter(|(id, input)| id == request && input.contains("[checks passed at "))
                .count(),
            1,
            "{heard:#?}"
        );
        assert_eq!(
            board.requests[request].state,
            crate::work::RequestState::Done
        );
    }
    assert_eq!(started_in(&flow.repo).len(), 1);
    flow.stop().await;
}

/// Commits `file` in the thread's workspace, as the thread's own edit; its new tip.
fn thread_commit(turn: &Turn, file: &str, text: &str) -> String {
    let workspace = &turn.add_dirs[0];
    std::fs::write(workspace.join(file), text).unwrap();
    git(workspace, &["add", file]);
    git(workspace, &["commit", "-q", "-m", &format!("Add {file}")]);
    git(workspace, &["rev-parse", "HEAD"])
}

/// BUILD-PLAN B1: one request makes three commits of the thread's own and lands a worker's
/// change; the session's full checks run exactly once, at the tip after the last of them, and
/// only once nothing of the request runs any more (no turn, worker or review).
#[tokio::test]
async fn full_checks_run_once_per_request_after_its_last_commit_and_landing() {
    let tips: Arc<Mutex<Vec<String>>> = Arc::default();
    let made = tips.clone();
    let flow = Flow::start(
        "integrated-once-per-request",
        options(),
        script(move |turn| {
            let made = made.clone();
            async move {
                if !turn.is_orchestrator() {
                    return worker(&turn, "w.txt", &Mutex::default()).await;
                }
                if turn.input.starts_with("Build it") {
                    made.lock()
                        .unwrap()
                        .push(thread_commit(&turn, "one.txt", "one\n"));
                    delegate(&turn, "Add w", "Create w.txt.").await;
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[report task-1 ") {
                    made.lock()
                        .unwrap()
                        .push(thread_commit(&turn, "two.txt", "two\n"));
                    let landed = turn.call("land_phase", json!({"task": "task-1"})).await;
                    assert!(!landed.is_error, "{}", landed.text);
                    made.lock()
                        .unwrap()
                        .push(thread_commit(&turn, "three.txt", "three\n"));
                    return Reply::text("Built: one, two, three and w.");
                }
                Reply::text("[quiet]")
            }
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
    let board = flow.settled().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (_, tip) = session_tip(&flow).await;
    assert_eq!(tips.lock().unwrap().last(), Some(&tip));
    assert_eq!(started_in(&flow.repo).len(), 1, "{:#?}", runs(&flow.repo));
    let checks: Vec<_> = board.integrated_checks.values().collect();
    assert_eq!(checks.len(), 1, "{checks:#?}");
    assert_eq!(checks[0].tip, tip);
    assert!(matches!(
        checks[0].state,
        IntegratedCheckState::Passed { .. }
    ));
    // It started after the request's last turn, its landing and every review of its work.
    let events = flow.events().await;
    let running = events
        .iter()
        .position(|event| {
            matches!(event, DomainEvent::IntegratedCheckUpdated { check }
                if matches!(check.state, IntegratedCheckState::Running { .. }))
        })
        .unwrap();
    let last_of = |matches: &dyn Fn(&DomainEvent) -> bool| events.iter().rposition(matches);
    let landed = last_of(&|event| {
        matches!(event, DomainEvent::OrchestratorStepped { step }
            if matches!(step.kind, OrchestratorStepKind::Landed { .. }))
    })
    .unwrap();
    let reviewed = last_of(&|event| {
        matches!(event, DomainEvent::ReviewUpdated { review }
            if review.state != crate::work::ReviewState::Running)
    })
    .unwrap();
    let idle = events[..running]
        .iter()
        .rposition(|event| {
            matches!(
                event,
                DomainEvent::RunStateChanged {
                    state: crate::work::RunState::Idle,
                    ..
                }
            )
        })
        .unwrap();
    let busy = events[..running].iter().rposition(|event| {
        matches!(event, DomainEvent::RunStateChanged { state, .. }
            if *state != crate::work::RunState::Idle)
    });
    assert!(
        landed < running && reviewed < running,
        "{landed} {reviewed} {running}"
    );
    assert!(
        busy.is_none_or(|busy| busy < idle),
        "the thread was idle when it started"
    );
    flow.stop().await;
}

/// A thread whose 40-line change gets a review held until `release`, which then finds one
/// thing; on the findings it fixes them (`fix`) or ignores them. The tips it made go to `tips`.
fn reviewed_thread(
    fix: bool,
    tips: Arc<Mutex<Vec<String>>>,
) -> (Options, Script, Arc<tokio::sync::Notify>) {
    let release = Arc::new(tokio::sync::Notify::new());
    let held = release.clone();
    let options = Options {
        reviews: Some(script(move |_| {
            let held = held.clone();
            async move {
                held.notified().await;
                Reply::text("- [P2] a.txt has no title — a.txt:1\n  Add one.")
            }
        })),
        ..options()
    };
    let thread = script(move |turn| {
        let tips = tips.clone();
        async move {
            if turn.input.starts_with("Build it") {
                let text: String = (1..=160).map(|n| format!("a {n}\n")).collect();
                tips.lock()
                    .unwrap()
                    .push(thread_commit(&turn, "a.txt", &text));
                // It answers at once: the end of its turn starts the review (the backstop).
                return Reply::text("Built it.");
            }
            if turn
                .input
                .contains("[review of your commits · Codex found 1")
            {
                if !fix {
                    return Reply::text("Leaving the finding: a.txt needs no title.");
                }
                let workspace = &turn.add_dirs[0];
                std::fs::write(workspace.join("a.txt"), "# Title\n").unwrap();
                git(workspace, &["add", "a.txt"]);
                git(
                    workspace,
                    &[
                        "commit",
                        "-q",
                        "-m",
                        "Give a.txt a title\n\nBrigadier-Fixes: R1.1",
                    ],
                );
                tips.lock()
                    .unwrap()
                    .push(git(workspace, &["rev-parse", "HEAD"]));
                let recorded = turn
                    .call(
                        "record_findings",
                        json!({"items": [{"id": "R1.1", "status": "fixed", "checked": "re-read a.txt:1"}]}),
                    )
                    .await;
                assert!(!recorded.is_error, "{}", recorded.text);
                return Reply::text("Fixed R1.1.");
            }
            Reply::text("[quiet]")
        }
    });
    (options, thread, release)
}

/// BUILD-PLAN C3: the review of the thread's change finds one thing late; the thread fixes it,
/// and only then do the session's full checks run, once, at the fix's tip: none ran while the
/// review ran, nor between its findings and the fix.
#[tokio::test]
async fn the_full_checks_run_once_after_a_late_findings_fix() {
    let tips: Arc<Mutex<Vec<String>>> = Arc::default();
    let (options, thread, release) = reviewed_thread(true, tips.clone());
    let flow = Flow::start("integrated-after-fix", options, thread).await;
    flow.say("Build it.").await;
    flow.until("the review to run, with the thread idle", |board| {
        board.run == crate::work::RunState::Idle
            && board
                .reviews
                .values()
                .any(|review| review.state == crate::work::ReviewState::Running)
    })
    .await;
    release.notify_one();
    flow.until("the full checks to end", |board| {
        board
            .integrated_checks
            .values()
            .any(|check| check.state.is_final())
    })
    .await;
    let board = flow.settled().await;
    let tips = tips.lock().unwrap().clone();
    assert_eq!(tips.len(), 2, "the change, then its fix");
    assert_eq!(started_in(&flow.repo).len(), 1, "{:#?}", runs(&flow.repo));
    let checks: Vec<_> = board.integrated_checks.values().collect();
    assert_eq!(checks.len(), 1, "{checks:#?}");
    assert_eq!(checks[0].tip, tips[1], "at the fix's tip");
    assert!(matches!(
        checks[0].state,
        IntegratedCheckState::Passed { .. }
    ));
    flow.stop().await;
}

/// The Delegator's rule: findings hold the full checks back only until the end of the turn
/// that heard them. A thread that ignores a finding (no record, no fix) still gets its checks,
/// exactly once, after that turn.
#[tokio::test]
async fn ignored_findings_hold_the_full_checks_only_until_the_turn_that_heard_them_ends() {
    let tips: Arc<Mutex<Vec<String>>> = Arc::default();
    let (options, thread, release) = reviewed_thread(false, tips.clone());
    let flow = Flow::start("integrated-ignored-finding", options, thread).await;
    flow.say("Build it.").await;
    flow.until("the review to run, with the thread idle", |board| {
        board.run == crate::work::RunState::Idle
            && board
                .reviews
                .values()
                .any(|review| review.state == crate::work::ReviewState::Running)
    })
    .await;
    release.notify_one();
    flow.until("the full checks to end", |board| {
        board
            .integrated_checks
            .values()
            .any(|check| check.state.is_final())
    })
    .await;
    let board = flow.settled().await;
    assert_eq!(started_in(&flow.repo).len(), 1, "{:#?}", runs(&flow.repo));
    let checks: Vec<_> = board.integrated_checks.values().collect();
    assert_eq!(checks.len(), 1, "{checks:#?}");
    assert_eq!(checks[0].tip, tips.lock().unwrap()[0]);
    // It started after the turn that heard the finding had answered.
    let events = flow.events().await;
    let running = events
        .iter()
        .position(|event| {
            matches!(event, DomainEvent::IntegratedCheckUpdated { check }
                if matches!(check.state, IntegratedCheckState::Running { .. }))
        })
        .unwrap();
    let heard = events
        .iter()
        .position(|event| {
            matches!(event, DomainEvent::MessageAppended { message }
                if message.text.starts_with("Leaving the finding"))
        })
        .unwrap();
    assert!(heard < running, "{heard} {running}");
    flow.stop().await;
}

/// What the user's checkout holds that Brigadier must never touch: HEAD, the refs, the index's
/// entries and the working files' changes.
fn checkout_state(repo: &Path) -> Vec<String> {
    [
        &["rev-parse", "HEAD"][..],
        &["for-each-ref"],
        &["ls-files", "-s"],
        &["status", "--porcelain", "--untracked-files=all"],
        &["diff"],
    ]
    .iter()
    .map(|args| git(repo, args))
    // Brigadier's own refs under refs/brigadier/ (a request's file records) are not the user's.
    .map(|out| {
        out.lines()
            .filter(|line| !line.contains("\trefs/brigadier/"))
            .collect::<Vec<_>>()
            .join("\n")
    })
    .collect()
}

/// BUILD-PLAN B2 on a local checkout, where the thread leaves its changes uncommitted: a
/// request's 3-file change gets its review from a snapshot of the checkout's files against the
/// request's start, and the full checks run once on that snapshot in their own checkout; the
/// user's HEAD, refs, index and working files are just as the thread left them.
#[tokio::test]
async fn a_local_checkouts_uncommitted_change_is_reviewed_and_checked_from_a_snapshot() {
    let left: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = left.clone();
    let mut flow = Flow::start(
        "integrated-local-snapshot",
        options(),
        script(move |turn| {
            let seen = seen.clone();
            async move {
                if turn.input.contains("Change three files.") {
                    let workspace = turn.add_dirs[0].clone();
                    for file in ["a.txt", "b.txt", "c.txt"] {
                        std::fs::write(workspace.join(file), notes(file)).unwrap();
                    }
                    // One of them staged by hand, as a user might have.
                    git(&workspace, &["add", "b.txt"]);
                    *seen.lock().unwrap() = checkout_state(&workspace);
                    return Reply::text("Changed three files; not committed.");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id;
    flow.conversation = flow
        .manager
        .create_conversation(
            crate::model::ConversationKind::Session,
            project,
            Some("Local".into()),
            Some(crate::model::SetupRequest::Session {
                repo: flow.repo.display().to_string(),
                environment: crate::model::EnvironmentRequest::LocalCheckout {
                    branch: "main".into(),
                    create_from: None,
                },
                permission: crate::model::PermissionLevel::FullAccess,
                orchestrator: crate::model::ModelChoice {
                    provider: brigadier_providers::ProviderKind::Claude,
                    model: Some("claude-opus-5-5".into()),
                    effort: None,
                    fast: None,
                    account: None,
                },
                plan_mode: false,
            }),
        )
        .await
        .unwrap()
        .id;
    let start = git(&flow.repo, &["rev-parse", "HEAD"]);
    flow.say("Change three files.").await;
    flow.until("the full checks to end", |board| {
        board
            .integrated_checks
            .values()
            .any(|check| check.state.is_final())
    })
    .await;
    let board = flow.settled().await;
    // One review, of the request's change: from its start to a snapshot of the files.
    let reviews: Vec<_> = board.reviews.values().collect();
    assert_eq!(reviews.len(), 1, "{reviews:#?}");
    let review = reviews[0];
    assert!(review.task_id.is_none());
    assert_eq!(
        review.base, start,
        "nothing had changed when the request started"
    );
    for file in ["a.txt", "b.txt", "c.txt"] {
        assert_eq!(
            git(&flow.repo, &["show", &format!("{}:{file}", review.tip)]),
            notes(file).trim_end()
        );
    }
    // One full-check run, on that same snapshot, in the check checkout.
    let dirs = started_in(&flow.repo);
    assert_eq!(dirs.len(), 1, "{:#?}", runs(&flow.repo));
    assert!(dirs[0].contains("/checks/"), "{dirs:#?}");
    let checks: Vec<_> = board.integrated_checks.values().collect();
    assert_eq!(checks.len(), 1, "{checks:#?}");
    assert_eq!(checks[0].tip, review.tip);
    assert!(matches!(
        checks[0].state,
        IntegratedCheckState::Passed { .. }
    ));
    // The user's checkout is as the thread left it.
    assert_eq!(checkout_state(&flow.repo), left.lock().unwrap().clone());
    assert_eq!(git(&flow.repo, &["rev-parse", "HEAD"]), start);
    flow.stop().await;
}

/// Issue #33: on the user's own checkout a review's fix is a change like any other (UI audit
/// M6). The findings say to leave the fix uncommitted, the thread does, nothing is committed on
/// the user's branch, and the fix, however big, gets no review of its own. run_check's `fixes`
/// records it fixed on a pass, but not when a file changed while the check ran.
#[tokio::test]
async fn a_review_fix_on_the_local_checkout_stays_uncommitted() {
    let told: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = told.clone();
    // Every input and run_check reply, so a check's result is there whether it came in the
    // call or, past the foreground limit, from the background.
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let heard = log.clone();
    let mut flow = Flow::start(
        "integrated-local-fix",
        Options {
            reviews: Some(script(|_| async {
                Reply::text("- [P2] a.txt has no title — a.txt:1\n  Add one.")
            })),
            ..options()
        },
        script(move |turn| {
            let (seen, heard) = (seen.clone(), heard.clone());
            async move {
                heard.lock().unwrap().push(turn.input.clone());
                let workspace = turn.add_dirs.first().cloned();
                if turn.input.contains("Check it again.") {
                    // On the user's own checkout a pass records them, with nothing committed.
                    let check = turn
                        .call("run_check", json!({"command": "true", "fixes": ["R1.1"]}))
                        .await;
                    let quiet = check.text.contains("goes on in the background");
                    heard.lock().unwrap().push(check.text);
                    return Reply::text(if quiet { "[quiet]" } else { "Checked." });
                }
                if turn.input.contains("Change three files.") {
                    let workspace = workspace.unwrap();
                    for file in ["a.txt", "b.txt", "c.txt"] {
                        std::fs::write(workspace.join(file), notes(file)).unwrap();
                    }
                    let check = turn.call("run_check", json!({"command": "true"})).await;
                    // Its review starts with its pass, in the call or, under load, once the
                    // check finishes in the background.
                    assert!(
                        check.text.contains("[review running]")
                            || check.text.contains("goes on in the background"),
                        "{}",
                        check.text
                    );
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[review of your changes") || turn.input.contains("found 1")
                {
                    seen.lock().unwrap().push(turn.input.clone());
                    let workspace = workspace.unwrap();
                    let title: String = (1..=40).map(|n| format!("# Title {n}\n")).collect();
                    std::fs::write(workspace.join("a.txt"), format!("{title}a.txt\n")).unwrap();
                    // A check that changes a file while it runs didn't check what is there
                    // now: its `fixes` aren't recorded (Phase O Codex review 3).
                    let check = turn
                        .call(
                            "run_check",
                            json!({"command": "echo more >> b.txt", "fixes": ["R1.1"]}),
                        )
                        .await;
                    assert!(!check.text.contains("[review running]"), "{}", check.text);
                    let quiet = check.text.contains("goes on in the background");
                    heard.lock().unwrap().push(check.text);
                    if quiet {
                        return Reply::text("[quiet]");
                    }
                    return Reply::text("Fixed R1.1. Changed 1 file, not committed.");
                }
                Reply::text("[quiet]")
            }
        }),
    )
    .await;
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id;
    flow.conversation = flow
        .manager
        .create_conversation(
            crate::model::ConversationKind::Session,
            project,
            Some("Local".into()),
            Some(crate::model::SetupRequest::Session {
                repo: flow.repo.display().to_string(),
                environment: crate::model::EnvironmentRequest::LocalCheckout {
                    branch: "main".into(),
                    create_from: None,
                },
                permission: crate::model::PermissionLevel::FullAccess,
                orchestrator: crate::model::ModelChoice {
                    provider: brigadier_providers::ProviderKind::Claude,
                    model: Some("claude-opus-5-5".into()),
                    effort: None,
                    fast: None,
                    account: None,
                },
                plan_mode: false,
            }),
        )
        .await
        .unwrap()
        .id;
    let start = git(&flow.repo, &["rev-parse", "HEAD"]);
    flow.say("Change three files.").await;
    flow.until("the answer", |board| {
        board
            .requests
            .values()
            .any(|request| request.state == crate::work::RequestState::Done)
    })
    .await;
    let board = flow.settled().await;
    let told = told.lock().unwrap().clone();
    assert_eq!(told.len(), 1, "{told:#?}");
    assert!(
        told[0].contains(
            "This is the user's own checkout: make the fix yourself and leave it uncommitted"
        ),
        "{}",
        told[0]
    );
    assert!(!told[0].contains("Brigadier-Fixes"), "{}", told[0]);
    assert!(!told[0].contains("delegate"), "{}", told[0]);
    // Nothing was committed on the user's branch; the fix is in their files.
    assert_eq!(git(&flow.repo, &["rev-parse", "HEAD"]), start);
    assert!(
        std::fs::read_to_string(flow.repo.join("a.txt"))
            .unwrap()
            .starts_with("# Title 1\n")
    );
    // One review: the fix got none of its own.
    assert_eq!(board.reviews.len(), 1, "{:#?}", board.reviews);
    let said = |log: &Arc<Mutex<Vec<String>>>, text: &str| {
        log.lock().unwrap().iter().any(|said| said.contains(text))
    };
    let changed = log.clone();
    flow.until("the fix's first check", move |_| {
        said(
            &changed,
            "Not recorded fixed: R1.1, as b.txt changed while the check ran, so it didn't check them.",
        )
    })
    .await;
    // A clean pass records its finding fixed, checked by that check.
    flow.say("Check it again.").await;
    let recorded = log.clone();
    let board = flow
        .until("the fix's second check", move |board| {
            said(&recorded, "Recorded: R1.1 fixed")
                && board.reviews.values().any(|review| {
                    matches!(
                        review.items[0].status,
                        crate::work::FindingStatus::Fixed { .. }
                    )
                })
        })
        .await;
    flow.settled().await;
    let review = board.reviews.values().next().unwrap();
    assert!(
        matches!(
            &review.items[0].status,
            crate::work::FindingStatus::Fixed { checked: Some(checked), .. }
                if checked.starts_with("`true` passed after the fix")
        ),
        "{:?}",
        review.items[0].status
    );
    flow.stop().await;
}

/// The user's checkout already held a change of theirs when the request started, and the
/// thread's work removes it: the review reads exactly the request's change, from the start's
/// snapshot to the end's, whose commit goes on the start's (two snapshots are siblings on
/// HEAD, and a reviewer's diff from the merge base would show the old change as the thread's
/// and miss its removal).
#[tokio::test]
async fn a_dirty_local_checkouts_review_reads_only_the_requests_change() {
    let mut flow = Flow::start(
        "integrated-local-dirty",
        options(),
        script(|turn| async move {
            if turn.input.contains("Change three files.") {
                let workspace = turn.add_dirs[0].clone();
                for file in ["b.txt", "c.txt", "d.txt"] {
                    std::fs::write(workspace.join(file), notes(file)).unwrap();
                }
                std::fs::remove_file(workspace.join("user.txt")).unwrap();
                return Reply::text("Changed three files; not committed.");
            }
            Reply::text("Noted.")
        }),
    )
    .await;
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id;
    flow.conversation = flow
        .manager
        .create_conversation(
            crate::model::ConversationKind::Session,
            project,
            Some("Local".into()),
            Some(crate::model::SetupRequest::Session {
                repo: flow.repo.display().to_string(),
                environment: crate::model::EnvironmentRequest::LocalCheckout {
                    branch: "main".into(),
                    create_from: None,
                },
                permission: crate::model::PermissionLevel::FullAccess,
                orchestrator: crate::model::ModelChoice {
                    provider: brigadier_providers::ProviderKind::Claude,
                    model: Some("claude-opus-5-5".into()),
                    effort: None,
                    fast: None,
                    account: None,
                },
                plan_mode: false,
            }),
        )
        .await
        .unwrap()
        .id;
    std::fs::write(flow.repo.join("user.txt"), "the user's own\n").unwrap();
    flow.say("Change three files.").await;
    flow.until("the review's end", |board| {
        !board.reviews.is_empty()
            && board
                .reviews
                .values()
                .all(|review| review.state != ReviewState::Running)
    })
    .await;
    let board = flow.settled().await;
    let reviews: Vec<_> = board.reviews.values().collect();
    assert_eq!(reviews.len(), 1, "{reviews:#?}");
    let review = reviews[0];
    let ancestor = std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", &review.base, &review.tip])
        .current_dir(&flow.repo)
        .status()
        .unwrap();
    assert!(ancestor.success(), "the reviewed tip goes on its base");
    assert_eq!(
        git(
            &flow.repo,
            &[
                "diff",
                "--name-status",
                &format!("{}...{}", review.base, review.tip)
            ]
        ),
        "A\tb.txt\nA\tc.txt\nA\td.txt\nD\tuser.txt"
    );
    flow.stop().await;
}

/// A restart while a request waits quietly on its checks: it still waits after the start, the
/// cut-off check runs again, and its pass reaches the thread for that request (#13).
#[tokio::test]
async fn a_restart_keeps_a_request_waiting_on_its_checks() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let (turn_gate, heard) = (gate.clone(), inputs.clone());
    let mut flow = Flow::start(
        "integrated-restart-deferred",
        options(),
        script(move |turn| {
            let (gate, heard) = (turn_gate.clone(), heard.clone());
            async move {
                heard.lock().unwrap().push(turn.input.clone());
                if turn.input.starts_with("Wait for checks") {
                    gate.notified().await;
                    return Reply::text("[quiet]");
                }
                Reply::text("The checks pass.")
            }
        }),
    )
    .await;
    std::fs::write(hold(&flow.repo), "").unwrap();
    flow.say("Wait for checks.").await;
    let board = flow
        .until("the first turn", |_| !inputs.lock().unwrap().is_empty())
        .await;
    let request = board.latest_request().unwrap().id.clone();
    let tip = brigadier_git::Oid(git(&flow.repo, &["rev-parse", "main"]));
    flow.manager
        .checks_due(
            &flow.conversation,
            &flow.repo,
            "main",
            &tip,
            Some(request.clone()),
        )
        .await;
    gate.notify_one();
    flow.until("the check to run", |board| {
        board.run == crate::work::RunState::Idle
            && board.requests[&request].state == crate::work::RequestState::Working
            && board
                .integrated_checks
                .values()
                .any(|check| matches!(check.state, IntegratedCheckState::Running { .. }))
    })
    .await;
    flow.restart().await;
    flow.until("the check to run again", |board| {
        board
            .integrated_checks
            .values()
            .any(|check| matches!(check.state, IntegratedCheckState::Running { .. }))
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        flow.board().await.requests[&request].state,
        crate::work::RequestState::Working,
        "it still waits for its checks"
    );
    std::fs::remove_file(hold(&flow.repo)).unwrap();
    let board = flow
        .until("the request answered", |board| {
            board.requests[&request].state == crate::work::RequestState::Done
        })
        .await;
    assert!(!board.requests[&request].deferred);
    let heard = inputs.lock().unwrap().clone();
    assert!(
        heard
            .iter()
            .any(|input| input.contains("[checks passed at ")),
        "{heard:#?}"
    );
    flow.stop().await;
}

/// What a frontend change's run tells its agents with computer use `on` or off in Settings: the
/// thread's instructions, the UI worker's brief, and the full checks' pass as the thread hears it.
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
                if land_reports(&turn).await {
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[checks passed at ") {
                    log.lock().unwrap()[2] = turn.input.clone();
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
