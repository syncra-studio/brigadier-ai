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

/// `set_headline` stores the session's headline and detail line and sends them as an event;
/// an empty one is refused, and the user's next message clears it.
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
    assert!(!said[1].0, "{said:?}");
    let headline = board.headline.clone().expect("the headline is stored");
    assert_eq!(headline.text, "Starting the dev server");
    assert_eq!(headline.detail.as_deref(), Some("Next: open the window"));
    let events = flow.events().await;
    assert!(
        events.iter().any(|event| matches!(
            event,
            crate::model::DomainEvent::HeadlineSet {
                headline: Some(_),
                ..
            }
        )),
        "sent as an event"
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
