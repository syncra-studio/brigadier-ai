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
use crate::work::{IntegratedCheckState, OrchestratorStepKind, TaskState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// The project's full checks: each run is logged in the repository's git folder (outside
/// every tree, so the check cache never sees it), held while `hold-checks` is there, and
/// fails while the tree has a `broken.txt`.
const FULL_CHECKS: &str = r#"#!/bin/sh
common=$(cd "$(git rev-parse --git-common-dir)" && pwd -P)
echo "start $$ $(pwd -P)" >> "$common/full-checks.runs"
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
    let (seen, heard) = (checked.clone(), inputs.clone());
    let mut flow = Flow::start(
        "integrated-once",
        options(),
        script(move |turn| {
            let (seen, heard) = (seen.clone(), heard.clone());
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
                if land_reports(&turn).await {
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
/// and the task whose change it was, and the merge the user asked for is held; the fix goes
/// back to that worker's own session, lands, the checks pass at the new tip and the merge
/// comes due.
#[tokio::test]
async fn a_failure_goes_back_to_the_worker_that_broke_it_and_holds_the_merge() {
    #[derive(Default)]
    struct Seen {
        inputs: Vec<String>,
        sessions: Vec<(u32, String)>,
        fix_session: Option<String>,
        merges: Vec<String>,
    }
    let seen: Arc<Mutex<Seen>> = Arc::default();
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
            let log = log.clone();
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
                if land_reports(&turn).await {
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[checks failed at ") {
                    // The landings' reviews end first: what holds the merge is the checks.
                    let deadline = std::time::Instant::now() + super::PATIENCE;
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
                    let merge = turn
                        .call("finish_session", json!({"user_words": "merge it"}))
                        .await;
                    assert!(merge.is_error, "{}", merge.text);
                    log.lock().unwrap().merges.push(merge.text);
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Fix the checks",
                                   "kind": "implement", "role": "fix", "subject": "task-2",
                                   "spec": "Remove broken.txt: the full checks fail on it."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
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
    flow.until("the merge", move |_| {
        log.lock()
            .unwrap()
            .merges
            .iter()
            .any(|text| text.contains("[finished]"))
    })
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
    // The merge was held by the failure, on the user's consent.
    assert!(
        seen.merges[0].contains("[not merged]") && seen.merges[0].contains("failed at"),
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
    // Two runs: the failure, then the pass at the fixed tip, which merged.
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
    flow.stop().await;
}

/// A merge asked while the checks run is held; a restart cuts the run off and runs it again;
/// its pass makes the merge due. Another session archived mid-run ends its run: its process
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
    // The merge waited for the run, then held.
    let log = merges.clone();
    flow.until("the held merge", move |_| !log.lock().unwrap().is_empty())
        .await;
    {
        let merges = merges.lock().unwrap();
        assert!(
            merges[0].contains("[not merged]") && merges[0].contains("still run at"),
            "{merges:#?}"
        );
    }
    assert_eq!(started_in(&flow.repo).len(), 1, "{:#?}", runs(&flow.repo));
    // A restart cuts the run off: it runs again, and its pass lets the merge through.
    flow.restart().await;
    let repo = flow.repo.clone();
    super::eventually("the run again", || started_in(&repo).len() == 2).await;
    std::fs::remove_file(hold(&flow.repo)).unwrap();
    let log = merges.clone();
    flow.until("the merge", move |_| {
        log.lock()
            .unwrap()
            .iter()
            .any(|text| text.contains("[finished]"))
    })
    .await;
    let board = flow.settled().await;
    let passed = board
        .integrated_checks
        .values()
        .filter(|check| matches!(check.state, IntegratedCheckState::Passed { .. }))
        .count();
    assert_eq!(passed, 1, "{:#?}", board.integrated_checks);

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
    let deadline = tokio::time::Instant::now() + super::PATIENCE;
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
