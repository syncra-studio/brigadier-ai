//! The agents' own tools for the work that has a component of its own (thread-truth grill,
//! 10-21): the session's headline, the checks and the review. Brigadier starts none of that
//! work by itself; it stores what the tools say and sends it as events.

use std::sync::{Arc, Mutex};

use brigadier_providers::model::Origin;
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn, git};
use crate::manager::prompts::{REVISIONS, Revision};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// An open thread whose CLI was told the revision before `number` resumes its same session on
/// its next turn and hears that revision's note once. Returns the note's turn input.
async fn an_open_thread_hears_once(name: &str, number: u32) -> (&'static Revision, String) {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let mut flow = Flow::start(
        name,
        Options::default(),
        Arc::new(move |turn| {
            log.lock().unwrap().push(turn.input.clone());
            Box::pin(async { Reply::text("Done.") })
        }),
    )
    .await;
    flow.say("First.").await;
    flow.settled().await;
    let added = REVISIONS
        .iter()
        .find(|revision| revision.number == number)
        .unwrap();
    // What the build before logged: its instructions were one revision older.
    flow.core
        .record(vec![(
            crate::model::streams::orchestrator(&flow.conversation),
            crate::model::DomainEvent::OrchestratorLogged {
                conversation_id: flow.conversation.clone(),
                entry: crate::work::OrchestratorEntry::Injection {
                    injection: crate::work::ContextInjection {
                        kind: crate::work::InjectionKind::Instructions,
                        bytes: 100,
                        tokens_estimate: 25,
                        label: "instructions revision".into(),
                        task_id: None,
                        told: Some(crate::work::Told {
                            revision: Some(number - 1),
                            ..crate::work::Told::default()
                        }),
                    },
                },
            },
        )])
        .await
        .unwrap();
    flow.restart().await;
    flow.say("Second.").await;
    flow.until("the second turn", |_| inputs.lock().unwrap().len() >= 2)
        .await;
    flow.settled().await;
    let specs = flow.thread_specs();
    assert!(
        matches!(specs.last().unwrap().1.origin, Origin::Resume { .. }),
        "{specs:#?}"
    );
    let second = inputs.lock().unwrap()[1].clone();
    assert!(
        second.contains(&format!("[instructions] {}", added.thread)),
        "{second}"
    );
    flow.say("Third.").await;
    flow.until("the third turn", |_| inputs.lock().unwrap().len() >= 3)
        .await;
    flow.settled().await;
    let third = inputs.lock().unwrap()[2].clone();
    assert!(!third.contains("[instructions]"), "{third}");
    flow.stop().await;
    (added, second)
}

/// `set_headline` stores the session's headline, detail line and whether it tells of a failure,
/// and sends them as an event; an empty one is refused, and the user's next message clears it.
#[tokio::test]
async fn the_threads_headline_stays_until_the_users_next_message() {
    let replies: Arc<Mutex<Vec<(bool, String)>>> = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        "thread-headline",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.is_orchestrator() && turn.input.contains("Start the server") {
                    for arguments in [
                        json!({ "headline": "  " }),
                        json!({ "headline": "The build failed", "failed": true }),
                        json!({
                            "headline": "Starting   the dev server",
                            "detail": "Next: open the window",
                        }),
                    ] {
                        let reply = turn.call("set_headline", arguments).await;
                        log.lock().unwrap().push((reply.is_error, reply.text));
                    }
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;

    flow.say("Start the server").await;
    let board = flow.settled().await;
    let said = std::mem::take(&mut *replies.lock().unwrap());
    assert!(said[0].0, "an empty headline is refused: {said:?}");
    assert!(!said[1].0 && !said[2].0, "{said:?}");
    let headline = board.headline.clone().expect("the headline is stored");
    assert_eq!(headline.text, "Starting the dev server");
    assert_eq!(headline.detail.as_deref(), Some("Next: open the window"));
    assert!(
        !headline.failed,
        "a headline tells of no failure unless the thread says so"
    );
    let events = flow.events().await;
    assert!(
        events.iter().any(|event| matches!(
            event,
            crate::model::DomainEvent::HeadlineSet {
                headline: Some(said),
                ..
            } if said.text == "The build failed" && said.failed
        )),
        "sent as an event, with whether it tells of a failure"
    );

    // The turn ending leaves it; the user's next message clears it.
    flow.say("Thanks").await;
    let board = flow.settled().await;
    assert_eq!(board.headline, None);
    flow.stop().await;
}

/// The headline's rollout: an open thread hears once that it has set_headline.
#[tokio::test]
async fn an_open_thread_hears_once_that_it_sets_the_headline() {
    let (added, _) = an_open_thread_hears_once(
        "headline-revision",
        crate::manager::prompts::HEADLINE_REVISION,
    )
    .await;
    assert!(added.thread.contains("set_headline"));
    assert!(added.worker.is_empty());
}

/// The project's full checks: each run is logged in the repository's git folder, held while
/// `hold-checks` is there, and fails while the tree has a `broken.txt`.
const FULL_CHECKS: &str = r#"#!/bin/sh
common=$(cd "$(git rev-parse --git-common-dir)" && pwd -P)
echo "start $$" >> "$common/full-checks.runs"
while [ -f "$common/hold-checks" ]; do sleep 0.05; done
if [ -f broken.txt ]; then
  echo "broken.txt must not be in the tree"
  exit 1
fi
"#;

/// A package with a test of its own that fails while it holds a `broken.txt`.
const WEB_PACKAGE: &str =
    r#"{ "name": "web", "private": true, "scripts": { "test": "sh test.sh" } }"#;
const WEB_TEST: &str = "if [ -f broken.txt ]; then echo \"web is broken\"; exit 1; fi\n";

fn checks_options() -> Options {
    Options {
        files: vec![
            ("tools/full-checks.sh", FULL_CHECKS),
            ("web/package.json", WEB_PACKAGE),
            ("web/test.sh", WEB_TEST),
        ],
        ..Options::default()
    }
}

fn full_runs(repo: &std::path::Path) -> usize {
    std::fs::read_to_string(repo.join(".git/full-checks.runs"))
        .unwrap_or_default()
        .lines()
        .count()
}

/// Commits `file` in the thread's workspace, as the thread's own edit; its new tip.
fn thread_commit(turn: &Turn, file: &str, text: &str) -> String {
    let workspace = &turn.add_dirs[0];
    std::fs::write(workspace.join(file), text).unwrap();
    git(workspace, &["add", file]);
    git(workspace, &["commit", "-q", "-m", &format!("Add {file}")]);
    git(workspace, &["rev-parse", "HEAD"])
}

/// The `run_checks` runs on a board: who asked (none: the thread), scope, tip and state.
fn asked_runs(board: &crate::board::Board) -> Vec<crate::work::IntegratedCheck> {
    let mut runs: Vec<_> = board
        .integrated_checks
        .values()
        .filter(|check| check.asked.is_some())
        .cloned()
        .collect();
    runs.sort_by_key(|check| crate::manager::integrated_check_number(&check.id));
    runs
}

/// The thread's run_checks runs the full checks on the session branch's commit and answers
/// with the verdict: a pass at the same commit answers again at once, `rerun` runs them again,
/// a failure names its command. Each run is recorded with who asked, its scope and its commit,
/// and its progress is sent as events.
#[tokio::test]
async fn the_threads_run_checks_runs_the_full_checks_on_its_commit() {
    let replies: Arc<Mutex<Vec<(bool, String)>>> = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        "run-checks-thread",
        checks_options(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("Done.");
                }
                let call = |rerun: bool| turn.call("run_checks", json!({ "rerun": rerun }));
                if turn.input.contains("Check it") {
                    thread_commit(&turn, "notes.txt", "notes\n");
                    for rerun in [false, false, true] {
                        let reply = call(rerun).await;
                        log.lock().unwrap().push((reply.is_error, reply.text));
                    }
                } else if turn.input.contains("Break it") {
                    thread_commit(&turn, "broken.txt", "x\n");
                    let reply = call(false).await;
                    log.lock().unwrap().push((reply.is_error, reply.text));
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;

    flow.say("Check it").await;
    let board = flow.settled().await;
    let said = std::mem::take(&mut *replies.lock().unwrap());
    assert!(said.iter().all(|(error, _)| !error), "{said:?}");
    assert!(said[0].1.starts_with("[checks passed at "), "{said:?}");
    assert!(said[0].1.contains("tools/full-checks.sh"), "{said:?}");
    assert_eq!(said[1].1, said[0].1, "an earlier pass answers again");
    let runs = asked_runs(&board);
    assert_eq!(runs.len(), 2, "the rerun ran again: {runs:#?}");
    let branch = runs[0].branch.clone();
    let tip = git(&flow.repo, &["rev-parse", &branch]);
    for run in &runs {
        assert_eq!(run.tip, tip.trim());
        let asked = run.asked.clone().unwrap();
        assert_eq!(asked.task_id, None);
        assert_eq!(asked.scope, crate::work::ChecksScope::Full);
        assert!(matches!(
            run.state,
            crate::work::IntegratedCheckState::Passed { .. }
        ));
    }
    assert!(runs[1].rerun);
    let events = flow.events().await;
    assert!(
        events.iter().any(|event| matches!(
            event,
            crate::model::DomainEvent::IntegratedCheckUpdated { check }
                if check.id == runs[0].id
                    && matches!(
                        &check.state,
                        crate::work::IntegratedCheckState::Running { command: Some(_), done: 0, total: 1 }
                    )
        )),
        "its progress is sent"
    );

    flow.say("Break it").await;
    flow.settled().await;
    let said = std::mem::take(&mut *replies.lock().unwrap());
    assert!(said[0].1.starts_with("[checks failed at "), "{said:?}");
    assert!(said[0].1.contains("tools/full-checks.sh`"), "{said:?}");
    assert!(said[0].1.contains("run_checks again"), "{said:?}");

    flow.stop().await;
}

/// Full checks still going after the call's wait go on in the background: the call says so,
/// and the verdict comes as a message.
#[tokio::test]
async fn full_checks_past_the_calls_wait_tell_their_verdict_as_a_message() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let replies: Arc<Mutex<Vec<String>>> = Arc::default();
    let (seen, log) = (inputs.clone(), replies.clone());
    let flow = Flow::start(
        "run-checks-background",
        checks_options(),
        script(move |turn| {
            let (seen, log) = (seen.clone(), log.clone());
            async move {
                if turn.is_orchestrator() {
                    seen.lock().unwrap().push(turn.input.clone());
                    if turn.input.contains("Check it") {
                        let reply = turn.call("run_checks", json!({})).await;
                        log.lock().unwrap().push(reply.text);
                        return Reply::text("[quiet]");
                    }
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    let hold = flow.repo.join(".git/hold-checks");
    std::fs::write(&hold, "").unwrap();
    flow.say("Check it").await;
    flow.until("the call went to the background", |_| {
        !replies.lock().unwrap().is_empty()
    })
    .await;
    let said = replies.lock().unwrap()[0].clone();
    assert!(said.contains("still run"), "{said}");
    // The call's wait can end before the run's tree is set up on a loaded machine.
    let repo = flow.repo.clone();
    flow.until("the checks started", |_| full_runs(&repo) > 0)
        .await;
    assert_eq!(full_runs(&flow.repo), 1);
    std::fs::remove_file(&hold).unwrap();
    flow.until("the verdict came as a message", |_| {
        inputs
            .lock()
            .unwrap()
            .iter()
            .any(|input| input.contains("[checks passed at "))
    })
    .await;
    flow.settled().await;
    flow.stop().await;
}

/// #512: background full checks that pass before the thread's turn ends ([quiet]) still tell
/// their verdict as a message: the turn's answer wasn't deferred yet when they ended, and the
/// pass was dropped as an optional one, leaving the request Done with nothing told.
#[tokio::test]
async fn a_background_pass_before_the_threads_quiet_still_comes_as_a_message() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let replies: Arc<Mutex<Vec<String>>> = Arc::default();
    let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (seen, log, go) = (inputs.clone(), replies.clone(), release.clone());
    let flow = Flow::start(
        "run-checks-background-early",
        checks_options(),
        script(move |turn| {
            let (seen, log, go) = (seen.clone(), log.clone(), go.clone());
            async move {
                if turn.is_orchestrator() {
                    seen.lock().unwrap().push(turn.input.clone());
                    if turn.input.contains("Check it") {
                        let reply = turn.call("run_checks", json!({})).await;
                        log.lock().unwrap().push(reply.text);
                        // The turn ends only once the checks have passed.
                        while !go.load(std::sync::atomic::Ordering::SeqCst) {
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        }
                        return Reply::text("[quiet]");
                    }
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    let hold = flow.repo.join(".git/hold-checks");
    std::fs::write(&hold, "").unwrap();
    flow.say("Check it").await;
    flow.until("the call went to the background", |_| {
        !replies.lock().unwrap().is_empty()
    })
    .await;
    assert!(replies.lock().unwrap()[0].contains("still run"));
    std::fs::remove_file(&hold).unwrap();
    flow.until("the checks passed while the turn still ran", |board| {
        asked_runs(board)
            .iter()
            .any(|run| matches!(run.state, crate::work::IntegratedCheckState::Passed { .. }))
    })
    .await;
    release.store(true, std::sync::atomic::Ordering::SeqCst);
    flow.until("the verdict came as a message", |_| {
        inputs
            .lock()
            .unwrap()
            .iter()
            .any(|input| input.contains("[checks passed at "))
    })
    .await;
    flow.settled().await;
    flow.stop().await;
}

/// A later request's run_checks at a commit whose checks still run joins that run: the run
/// records it, so its verdict comes to that request too once the call has gone on.
#[tokio::test]
async fn a_later_run_checks_at_a_running_commit_joins_it() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let replies: Arc<Mutex<Vec<String>>> = Arc::default();
    let (seen, log) = (inputs.clone(), replies.clone());
    let flow = Flow::start(
        "run-checks-join",
        checks_options(),
        script(move |turn| {
            let (seen, log) = (seen.clone(), log.clone());
            async move {
                if turn.is_orchestrator() {
                    seen.lock().unwrap().push(turn.input.clone());
                    if turn.input.contains("Start the checks") {
                        let reply = turn.call("run_checks", json!({})).await;
                        log.lock().unwrap().push(reply.text);
                        return Reply::text("They run in the background.");
                    }
                    if turn.input.contains("Check it") {
                        let reply = turn.call("run_checks", json!({})).await;
                        log.lock().unwrap().push(reply.text);
                        return Reply::text("[quiet]");
                    }
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    let hold = flow.repo.join(".git/hold-checks");
    std::fs::write(&hold, "").unwrap();
    flow.say("Start the checks").await;
    // Answered, the first request is over while its checks still run.
    flow.until("the first request was answered", |board| {
        replies.lock().unwrap().len() == 1
            && board
                .requests
                .values()
                .all(|request| request.state == crate::work::RequestState::Done)
    })
    .await;
    flow.say("Check it").await;
    let board = flow
        .until("the second call went to the background", |_| {
            replies.lock().unwrap().len() == 2
        })
        .await;
    assert!(replies.lock().unwrap()[1].contains("still run"));
    let repo = flow.repo.clone();
    flow.until("the checks started", |_| full_runs(&repo) > 0)
        .await;
    assert_eq!(full_runs(&flow.repo), 1, "one run for both");
    let runs = asked_runs(&board);
    assert_eq!(runs.len(), 1, "{runs:#?}");
    assert_eq!(
        runs[0].requests.len(),
        2,
        "both requests are on it: {runs:#?}"
    );
    std::fs::remove_file(&hold).unwrap();
    flow.until("the verdict came to the later request as a message", |_| {
        inputs
            .lock()
            .unwrap()
            .iter()
            .any(|input| input.contains("[checks passed at "))
    })
    .await;
    flow.settled().await;
    flow.stop().await;
}

/// A thread that calls run_checks on "Check it" and answers `answer` at once; "Next" waits for
/// `go` before it answers (a follow-up quoting either is neither). What each turn heard and
/// what the call said are logged.
fn checking_thread(
    inputs: Arc<Mutex<Vec<String>>>,
    replies: Arc<Mutex<Vec<String>>>,
    go: Arc<std::sync::atomic::AtomicBool>,
    answer: &'static str,
) -> Script {
    script(move |turn| {
        let (seen, log, go) = (inputs.clone(), replies.clone(), go.clone());
        async move {
            if turn.is_orchestrator() {
                seen.lock().unwrap().push(turn.input.clone());
                if turn.input.starts_with("Check it") {
                    let reply = turn.call("run_checks", json!({})).await;
                    log.lock().unwrap().push(reply.text);
                    return Reply::text(answer);
                }
                if turn.input.starts_with("Next") {
                    while !go.load(std::sync::atomic::Ordering::SeqCst) {
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                    return Reply::text("Next is done.");
                }
            }
            Reply::text("Done.")
        }
    })
}

/// How often a turn of the thread took a message about check `id`.
async fn told(flow: &Flow, id: &str) -> usize {
    flow.core
        .list_orchestrator_log(&flow.conversation, None, 500)
        .await
        .unwrap()
        .entries
        .into_iter()
        .filter(|logged| {
            matches!(&logged.entry,
                crate::work::OrchestratorEntry::Injection { injection }
                    if injection.kind == crate::work::InjectionKind::Decision
                        && injection.label.ends_with(&format!(" {id}"))
            )
        })
        .count()
}

/// How many of `inputs` hold `what`.
fn heard(inputs: &Mutex<Vec<String>>, what: &str) -> usize {
    inputs
        .lock()
        .unwrap()
        .iter()
        .filter(|input| input.contains(what))
        .count()
}

/// Starts a held run_checks whose call goes on in the background, the thread answering
/// `answer`, and waits until the run has started and the call went on. Returns the hold.
async fn background_run(flow: &Flow, replies: &Mutex<Vec<String>>) -> std::path::PathBuf {
    let hold = flow.repo.join(".git/hold-checks");
    std::fs::write(&hold, "").unwrap();
    flow.say("Check it").await;
    flow.until("the call went to the background", |_| {
        !replies.lock().unwrap().is_empty()
    })
    .await;
    assert!(replies.lock().unwrap()[0].contains("still run"));
    let repo = flow.repo.clone();
    flow.until("the checks started", |_| full_runs(&repo) > 0)
        .await;
    hold
}

/// #513: background full checks whose call went on, and whose thread answered instead of
/// replying [quiet], still tell their pass as a message, once: the thread hears that the
/// checks it asked for passed without calling run_checks again. The board says so too.
#[tokio::test]
async fn a_background_pass_reaches_a_thread_that_answered_once() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let replies: Arc<Mutex<Vec<String>>> = Arc::default();
    let flow = Flow::start(
        "run-checks-background-answered",
        checks_options(),
        checking_thread(
            inputs.clone(),
            replies.clone(),
            Arc::default(),
            "They run in the background; I'll say when they pass.",
        ),
    )
    .await;
    let hold = background_run(&flow, &replies).await;
    // The thread answered: the request is over and the thread idle while they still run.
    flow.until("the request was answered", |board| {
        board.run == crate::work::RunState::Idle
            && board
                .requests
                .values()
                .all(|request| request.state == crate::work::RequestState::Done)
    })
    .await;
    std::fs::remove_file(&hold).unwrap();
    flow.until("the pass came as a message", |_| {
        heard(&inputs, "[checks passed at ") > 0
    })
    .await;
    let board = flow.settled().await;
    let runs = asked_runs(&board);
    assert!(matches!(
        runs[0].state,
        crate::work::IntegratedCheckState::Passed { .. }
    ));
    assert_eq!(heard(&inputs, "[checks passed at "), 1);
    assert_eq!(told(&flow, &runs[0].id).await, 1);
    assert_eq!(full_runs(&flow.repo), 1, "the thread didn't run them again");
    flow.stop().await;
}

/// #513: a background pass that ends while the turn that called run_checks still runs, when
/// that turn then answers instead of replying [quiet], still comes as a message once (#512
/// kept it only for a [quiet]).
#[tokio::test]
async fn a_background_pass_before_the_threads_answer_still_comes_once() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let replies: Arc<Mutex<Vec<String>>> = Arc::default();
    let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (seen, log, go) = (inputs.clone(), replies.clone(), release.clone());
    let flow = Flow::start(
        "run-checks-background-early-answer",
        checks_options(),
        script(move |turn| {
            let (seen, log, go) = (seen.clone(), log.clone(), go.clone());
            async move {
                if turn.is_orchestrator() {
                    seen.lock().unwrap().push(turn.input.clone());
                    if turn.input.contains("Check it") {
                        let reply = turn.call("run_checks", json!({})).await;
                        log.lock().unwrap().push(reply.text);
                        // The turn ends only once the checks have passed.
                        while !go.load(std::sync::atomic::Ordering::SeqCst) {
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        }
                        return Reply::text("They run in the background.");
                    }
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    let hold = background_run(&flow, &replies).await;
    std::fs::remove_file(&hold).unwrap();
    flow.until("the checks passed while the turn still ran", |board| {
        asked_runs(board)
            .iter()
            .any(|run| matches!(run.state, crate::work::IntegratedCheckState::Passed { .. }))
    })
    .await;
    release.store(true, std::sync::atomic::Ordering::SeqCst);
    flow.until("the pass came as a message", |_| {
        heard(&inputs, "[checks passed at ") > 0
    })
    .await;
    let board = flow.settled().await;
    assert_eq!(heard(&inputs, "[checks passed at "), 1);
    assert_eq!(told(&flow, &asked_runs(&board)[0].id).await, 1);
    flow.stop().await;
}

/// #513: a background pass that ends while the thread's next request runs reaches the
/// thread once, from that turn or the one after it.
#[tokio::test]
async fn a_background_pass_during_the_next_request_still_comes_once() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let replies: Arc<Mutex<Vec<String>>> = Arc::default();
    let go = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flow = Flow::start(
        "run-checks-background-next",
        checks_options(),
        checking_thread(inputs.clone(), replies.clone(), go.clone(), "They run."),
    )
    .await;
    let hold = background_run(&flow, &replies).await;
    flow.until("the request was answered", |board| {
        board.run == crate::work::RunState::Idle
    })
    .await;
    flow.say("Next").await;
    flow.until("the next message's turn runs", |_| {
        inputs
            .lock()
            .unwrap()
            .iter()
            .any(|input| input.starts_with("Next"))
    })
    .await;
    std::fs::remove_file(&hold).unwrap();
    let board = flow
        .until("the checks passed during it", |board| {
            asked_runs(board)
                .iter()
                .any(|run| matches!(run.state, crate::work::IntegratedCheckState::Passed { .. }))
        })
        .await;
    let check = asked_runs(&board)[0].id.clone();
    go.store(true, std::sync::atomic::Ordering::SeqCst);
    for _ in 0..400 {
        if told(&flow, &check).await > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    flow.settled().await;
    assert_eq!(
        told(&flow, &check).await,
        1,
        "{:#?}",
        inputs.lock().unwrap()
    );
    assert!(heard(&inputs, "[checks passed at ") <= 1);
    assert_eq!(
        replies.lock().unwrap().len(),
        1,
        "run_checks was called once"
    );
    flow.stop().await;
}

/// #513: background full checks a restart cut off tell the thread so once the session is
/// back, once: not again after a second restart. Their record reads interrupted.
#[tokio::test]
async fn background_checks_a_restart_cut_off_tell_the_thread_once() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let replies: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut flow = Flow::start(
        "run-checks-background-restart",
        checks_options(),
        checking_thread(inputs.clone(), replies.clone(), Arc::default(), "They run."),
    )
    .await;
    let hold = background_run(&flow, &replies).await;
    flow.until("the request was answered", |board| {
        board.run == crate::work::RunState::Idle
    })
    .await;
    flow.crash().await;
    // The cut-off run's script, if it outlived the daemon, ends.
    std::fs::remove_file(&hold).unwrap();
    flow.until("the thread heard they were cut off", |_| {
        heard(&inputs, "[checks couldn't run at ") > 0
    })
    .await;
    let board = flow.settled().await;
    let run = asked_runs(&board)[0].clone();
    assert!(
        matches!(&run.state, crate::work::IntegratedCheckState::Interrupted { reason } if reason.contains("Brigadier quit")),
        "{run:#?}"
    );
    assert_eq!(told(&flow, &run.id).await, 1);
    flow.crash().await;
    flow.settled().await;
    assert_eq!(heard(&inputs, "[checks couldn't run at "), 1);
    assert_eq!(told(&flow, &run.id).await, 1);
    flow.stop().await;
}

/// #513: a background pass recorded just before Brigadier stopped, before any turn of the
/// thread took it, reaches the thread after the restart, once.
#[tokio::test]
async fn a_background_pass_no_turn_took_before_a_restart_comes_after_it_once() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let replies: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut flow = Flow::start(
        "run-checks-background-restart-pass",
        checks_options(),
        checking_thread(inputs.clone(), replies.clone(), Arc::default(), "They run."),
    )
    .await;
    let hold = background_run(&flow, &replies).await;
    let board = flow
        .until("the request was answered", |board| {
            board.run == crate::work::RunState::Idle
        })
        .await;
    // Its pass is recorded; the daemon dies before the thread hears it.
    let mut run = asked_runs(&board)[0].clone();
    assert!(run.owed, "the call that went on owes its verdict");
    run.state = crate::work::IntegratedCheckState::Passed {
        commands: vec!["tools/full-checks.sh".into()],
    };
    run.ended_at_ms = Some(crate::now_ms());
    flow.core
        .record_conversation(
            &flow.conversation,
            vec![crate::model::DomainEvent::IntegratedCheckUpdated { check: run.clone() }],
        )
        .await
        .unwrap();
    flow.crash().await;
    // The cut-off run's script, if it outlived the daemon, ends.
    std::fs::remove_file(&hold).unwrap();
    flow.until("the pass came after the restart", |_| {
        heard(&inputs, "[checks passed at ") > 0
    })
    .await;
    flow.settled().await;
    assert_eq!(told(&flow, &run.id).await, 1);
    flow.crash().await;
    flow.settled().await;
    assert_eq!(heard(&inputs, "[checks passed at "), 1);
    assert_eq!(told(&flow, &run.id).await, 1);
    flow.stop().await;
}

/// A worker's run_checks runs the checks its change calls for in its own checkout and answers
/// in the call; the run is recorded as the worker's, scope change, at its commit.
#[tokio::test]
async fn a_workers_run_checks_runs_its_changes_checks() {
    let replies: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        "run-checks-worker",
        checks_options(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("Change web") {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({
                                    "title": "Change web",
                                    "kind": "implement",
                                    "spec": "Change web.",
                                    "effort": "low",
                                }),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                    }
                    return Reply::text("[quiet]");
                }
                turn.write("web/broken.txt", "x\n");
                turn.git(&["add", "web/broken.txt"]);
                turn.git(&["commit", "-q", "-m", "Break web"]);
                let failed = turn.call("run_checks", json!({})).await;
                turn.git(&["rm", "-q", "web/broken.txt"]);
                turn.write("web/page.txt", "page\n");
                turn.git(&["add", "web/page.txt"]);
                turn.git(&["commit", "-q", "-m", "Fix web"]);
                let passed = turn.call("run_checks", json!({})).await;
                log.lock().unwrap().extend([failed.text, passed.text]);
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Changed web.", "changes": ["web/broken.txt"]}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Change web").await;
    let board = flow
        .until("the worker checked twice", |_| {
            replies.lock().unwrap().len() == 2
        })
        .await;
    let said = replies.lock().unwrap().clone();
    assert!(said[0].starts_with("[checks failed at "), "{said:?}");
    assert!(said[0].contains("web is broken"), "{said:?}");
    assert!(said[1].starts_with("[checks passed at "), "{said:?}");
    let runs = asked_runs(&board);
    assert_eq!(runs.len(), 2, "{runs:#?}");
    let task = board.tasks.values().next().unwrap();
    for run in &runs {
        let asked = run.asked.clone().unwrap();
        assert_eq!(asked.task_id.as_ref(), Some(&task.id));
        assert_eq!(asked.scope, crate::work::ChecksScope::Change);
    }
    assert!(matches!(
        runs[0].state,
        crate::work::IntegratedCheckState::Failed { .. }
    ));
    assert!(matches!(
        runs[1].state,
        crate::work::IntegratedCheckState::Passed { .. }
    ));
    assert_ne!(runs[0].tip, runs[1].tip, "each at the worker's commit then");
    assert_eq!(full_runs(&flow.repo), 0, "never the full checks");
    flow.stop().await;
}

/// run_checks' rollout: an open thread hears once that rerun_checks is gone and when to call
/// run_checks; a resumed worker hears when it checks its own change.
#[tokio::test]
async fn an_open_thread_hears_once_that_run_checks_replaces_rerun_checks() {
    let (added, _) = an_open_thread_hears_once(
        "run-checks-revision",
        crate::manager::prompts::RUN_CHECKS_REVISION,
    )
    .await;
    assert!(added.thread.contains("rerun_checks is gone"));
    assert!(
        added
            .thread
            .contains(crate::manager::prompts::RUN_CHECKS_RULE)
    );
    assert!(
        added
            .worker
            .contains(crate::manager::prompts::WORKER_RUN_CHECKS_RULE)
    );
}

/// request_review replaces review_candidate and review_code: an open thread hears of it once,
/// and so do its workers (BUILD-PLAN rule R).
#[tokio::test]
async fn an_open_thread_hears_once_that_request_review_replaces_the_old_review_tools() {
    let (added, _) = an_open_thread_hears_once(
        "request-review-revision",
        crate::manager::prompts::REQUEST_REVIEW_REVISION,
    )
    .await;
    assert!(added.thread.contains("review_candidate is gone"));
    assert!(added.worker.contains("review_code is gone"));
    for text in [added.thread, added.worker] {
        assert!(text.contains("request_review"), "{text}");
    }
}

/// What the thread's finish_session calls answered: an error, and its text.
type Said = Arc<Mutex<Vec<(bool, String)>>>;

/// The merges on a board, oldest first: whether each skipped the checks.
fn merges(board: &crate::board::Board) -> Vec<bool> {
    board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            crate::work::OrchestratorStepKind::Merged { checks_skipped, .. } => {
                Some(*checks_skipped)
            }
            _ => None,
        })
        .collect()
}

/// The merge rule (thread-truth grill Q5): a merge into main goes through only once run_checks
/// passed on its exact commit. Never run, or failed, it is refused with the reason in plain
/// words, recorded for the user; the user's plain consent named as `anyway` doesn't skip it,
/// nor their words quoted out of a message that says not to, their "merge it anyway" does, and so does their "Merge anyway" click. A pass merges with no
/// waiver, and a merge that goes through ends the refusal.
#[tokio::test]
async fn a_merge_into_main_needs_run_checks_passed_on_its_commit_or_the_users_merge_anyway() {
    let said: Said = Arc::default();
    let log = said.clone();
    let flow = Flow::start(
        "merge-rule",
        checks_options(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("Done.");
                }
                let finish = |args: serde_json::Value| async {
                    let reply = turn.call("finish_session", args).await;
                    log.lock().unwrap().push((reply.is_error, reply.text));
                };
                let input = turn.input.clone();
                if input.contains("Add notes and merge it") {
                    thread_commit(&turn, "notes.txt", "notes\n");
                    finish(json!({ "consent": "merge it" })).await;
                } else if input.contains("Break it, check it and merge it") {
                    thread_commit(&turn, "broken.txt", "x\n");
                    turn.call("run_checks", json!({})).await;
                    finish(json!({ "consent": "merge it" })).await;
                    // The user's consent isn't their word to skip the checks.
                    finish(json!({ "consent": "merge it", "anyway": "merge it" })).await;
                } else if input.contains("Don't merge without the tests") {
                    // Words lifted out of a message that says not to.
                    finish(json!({
                        "consent": "Merge it once they pass",
                        "anyway": "merge without the tests",
                    }))
                    .await;
                } else if input.contains("Merge it anyway") {
                    finish(json!({ "consent": "Merge it anyway", "anyway": "Merge it anyway" }))
                        .await;
                } else if input.contains("Fix it, check it and merge it") {
                    let workspace = &turn.add_dirs[0];
                    git(workspace, &["rm", "-q", "broken.txt"]);
                    git(workspace, &["commit", "-q", "-m", "Fix it"]);
                    turn.call("run_checks", json!({})).await;
                    finish(json!({ "consent": "merge it" })).await;
                } else if input.contains("Add more") {
                    thread_commit(&turn, "more.txt", "more\n");
                    finish(json!({ "consent": "Add more" })).await;
                } else if input.contains("[merge]") {
                    finish(json!({})).await;
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;

    // Never checked: refused, and the reason is recorded for the user.
    flow.say("Add notes and merge it").await;
    let board = flow.settled().await;
    let (error, text) = said.lock().unwrap().remove(0);
    assert!(error, "{text}");
    assert!(
        text.starts_with("[not merged] The checks haven't run on "),
        "{text}"
    );
    assert!(text.contains("Call run_checks"), "{text}");
    let refused = board
        .merge_refused
        .clone()
        .expect("the refusal is recorded");
    let branch = refused.branch.clone();
    assert_eq!(refused.sha, git(&flow.repo, &["rev-parse", &branch]).trim());
    assert!(refused.waivable);
    assert!(refused.reason.starts_with("The checks haven't run on "));
    assert!(merges(&board).is_empty());

    // Failed: refused with the command that failed; plain consent as `anyway` changes nothing.
    flow.say("Break it, check it and merge it").await;
    let board = flow.settled().await;
    let replies = std::mem::take(&mut *said.lock().unwrap());
    assert!(replies.iter().all(|(error, _)| *error), "{replies:?}");
    assert!(
        replies[0].1.contains("The checks failed on ") && replies[0].1.contains("full-checks.sh"),
        "{replies:?}"
    );
    assert!(
        replies[1].1.contains("`anyway` doesn't skip them"),
        "{replies:?}"
    );
    assert!(
        board
            .merge_refused
            .as_ref()
            .unwrap()
            .reason
            .starts_with("The checks failed on "),
        "{:?}",
        board.merge_refused
    );
    assert!(merges(&board).is_empty());

    // Their words quoted out of a message that says not to skip them don't either.
    flow.say("Don't merge without the tests. Merge it once they pass")
        .await;
    let board = flow.settled().await;
    let (error, text) = said.lock().unwrap().remove(0);
    assert!(
        error && text.contains("`anyway` doesn't skip them"),
        "{text}"
    );
    assert!(merges(&board).is_empty());

    // The user's own "merge it anyway" merges, with the checks skipped, and ends the refusal.
    flow.say("Merge it anyway").await;
    let board = flow.settled().await;
    let (error, text) = said.lock().unwrap().remove(0);
    assert!(!error, "{text}");
    assert!(text.starts_with("[finished]"), "{text}");
    assert_eq!(merges(&board), [true]);
    assert_eq!(board.merge_refused, None);

    // A pass on the commit merges, with nothing skipped.
    flow.say("Fix it, check it and merge it").await;
    let board = flow.settled().await;
    let (error, text) = said.lock().unwrap().remove(0);
    assert!(!error, "{text}");
    assert_eq!(merges(&board), [true, false]);

    // The "Merge anyway" click skips them too, and ends the refusal at once.
    flow.say("Add more").await;
    let board = flow.settled().await;
    assert!(said.lock().unwrap().remove(0).0);
    assert!(board.merge_refused.is_some());
    flow.manager
        .merge_session(&flow.conversation, super::click(), true)
        .await
        .unwrap();
    let board = flow
        .until("the anyway click merged", |board| merges(board).len() == 3)
        .await;
    assert_eq!(merges(&board), [true, false, true]);
    assert_eq!(board.merge_refused, None);
    let events = flow.events().await;
    assert!(events.iter().any(|event| matches!(
        event,
        crate::model::DomainEvent::MergeRefusalSet {
            refusal: Some(_),
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        crate::model::DomainEvent::OrchestratorStepped { step, .. }
            if matches!(step.kind, crate::work::OrchestratorStepKind::MergeRequested { anyway: true, .. })
    )));
    flow.stop().await;
}

/// Only a run_checks pass counts: a pass Brigadier's own checks recorded on the same commit
/// (a session from before the rule) never lets a merge through.
#[tokio::test]
async fn an_old_automatic_pass_never_satisfies_the_merge_rule() {
    let said: Said = Arc::default();
    let log = said.clone();
    let flow = Flow::start(
        "merge-rule-old-pass",
        checks_options(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.is_orchestrator() && turn.input.contains("Merge it") {
                    let reply = turn
                        .call("finish_session", json!({ "consent": "Merge it" }))
                        .await;
                    log.lock().unwrap().push((reply.is_error, reply.text));
                } else if turn.is_orchestrator() && turn.input.contains("Add notes") {
                    thread_commit(&turn, "notes.txt", "notes\n");
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.say("Add notes").await;
    let board = flow.settled().await;
    let branch = flow
        .manager
        .recorded_workspace(&flow.conversation)
        .expect("the session's workspace")
        .branch;
    let tip = git(&flow.repo, &["rev-parse", &branch]);
    let old = crate::work::IntegratedCheck {
        id: crate::manager::integrated_checks::next_check_id(&board),
        conversation_id: flow.conversation.clone(),
        repo: flow.repo.to_string_lossy().into_owned(),
        branch: branch.clone(),
        tip: tip.trim().to_owned(),
        state: crate::work::IntegratedCheckState::Passed {
            commands: vec!["sh tools/full-checks.sh".into()],
        },
        due_at_ms: 0,
        started_at_ms: Some(1),
        ended_at_ms: Some(2),
        requests: Vec::new(),
        tasks: Vec::new(),
        rerun: false,
        asked: None,
        owed: false,
    };
    flow.core
        .record_conversation(
            &flow.conversation,
            vec![crate::model::DomainEvent::IntegratedCheckUpdated { check: old }],
        )
        .await
        .unwrap();
    flow.say("Merge it").await;
    let board = flow.settled().await;
    let (error, text) = said.lock().unwrap().remove(0);
    assert!(error, "{text}");
    assert!(text.contains("The checks haven't run on "), "{text}");
    assert!(merges(&board).is_empty());
    flow.stop().await;
}

/// The merge rule's rollout: an open thread hears once that a merge into main needs run_checks
/// passed and that only the user can skip them; its workers hear nothing.
#[tokio::test]
async fn an_open_thread_hears_once_of_the_merge_rule() {
    let (added, _) = an_open_thread_hears_once(
        "merge-rule-revision",
        crate::manager::prompts::MERGE_RULE_REVISION,
    )
    .await;
    assert!(
        added
            .thread
            .contains("finish_session refuses a merge into main")
    );
    assert!(added.thread.contains(crate::manager::prompts::MERGE_RULE));
    assert!(added.worker.is_empty());
}

/// The end of the automatic pipeline's rollout: an open thread hears once that Brigadier no
/// longer reviews or checks anything by itself, and what to call instead; its workers hear
/// nothing.
#[tokio::test]
async fn an_open_thread_hears_once_that_brigadier_reviews_and_checks_nothing_by_itself() {
    let (added, _) = an_open_thread_hears_once(
        "no-auto-pipeline-revision",
        crate::manager::prompts::NO_AUTO_PIPELINE_REVISION,
    )
    .await;
    assert!(
        added
            .thread
            .contains("Brigadier no longer reviews or checks anything by itself")
    );
    assert!(
        added
            .thread
            .contains("Call request_review for a large or risky change")
    );
    assert!(
        added
            .thread
            .contains("run_checks before every merge into main")
    );
    assert!(added.worker.is_empty());
}

/// Brigadier starts no review, no full checks and no fix by itself: not for a turn in which the
/// thread commits 240 lines of code over 6 files, nor for a worker's landing. Only the agents
/// start them (request_review, run_checks, delegate_task).
#[tokio::test]
async fn a_big_commit_and_a_landing_start_no_review_check_or_fix() {
    let flow = Flow::start(
        "no-auto-pipeline",
        Options::default(),
        script(|turn| async move {
            if !turn.is_orchestrator() {
                turn.write("feature.rs", "pub fn feature() {}\n");
                turn.git(&["add", "feature.rs"]);
                turn.git(&["commit", "-qm", "Add the feature"]);
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Added feature.rs.", "changes": ["feature.rs"]}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Reported.");
            }
            if turn.input.contains("[report task-1 ") {
                let reply = turn.call("land_phase", json!({"task": "task-1"})).await;
                assert!(reply.text.starts_with("Landed "), "{}", reply.text);
                return Reply::text("Built the modules and the feature.");
            }
            if turn.input.starts_with("Build the modules") {
                let workspace = &turn.add_dirs[0];
                for module in ["a", "b", "c", "d", "e", "f"] {
                    let text: String = (1..=40)
                        .map(|n| format!("pub fn {module}_{n}() {{}}\n"))
                        .collect();
                    std::fs::write(workspace.join(format!("{module}.rs")), text).unwrap();
                }
                git(workspace, &["add", "-A"]);
                git(
                    workspace,
                    &[
                        "commit",
                        "-qm",
                        "Add six modules\n\nBrigadier-Author: thread",
                    ],
                );
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "low", "title": "Add the feature", "kind": "implement",
                               "spec": "Add feature.rs with an empty `feature` function."}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("[quiet]");
            }
            Reply::text("Noted.")
        }),
    )
    .await;
    flow.say("Build the modules and the feature.").await;
    flow.until("the landing", |board| {
        board
            .tasks
            .values()
            .any(|task| task.state == crate::work::TaskState::Landed)
    })
    .await;
    flow.settled().await;
    // Time for anything automatic to start.
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let board = flow.board().await;
    assert!(board.reviews.is_empty(), "{:#?}", board.reviews);
    assert!(
        board.integrated_checks.is_empty(),
        "{:#?}",
        board.integrated_checks
    );
    assert_eq!(board.tasks.len(), 1, "{:#?}", board.tasks);
    flow.stop().await;
}

/// Workers ask for their own review only when it is worth one: a resumed worker hears it once,
/// the thread nothing new.
#[test]
fn a_resumed_worker_hears_once_when_to_ask_for_its_review() {
    use crate::manager::prompts::{WORKER_REVIEW_REVISION, revision_note};
    let note = revision_note(WORKER_REVIEW_REVISION - 1, true).unwrap();
    assert!(
        note.contains("request_review only when your task asks"),
        "{note}"
    );
    assert!(revision_note(WORKER_REVIEW_REVISION, true).is_none());
    assert_eq!(
        revision_note(WORKER_REVIEW_REVISION - 1, false),
        revision_note(WORKER_REVIEW_REVISION, false)
    );
}
