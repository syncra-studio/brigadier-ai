//! Reviews scaled to risk (BUILD-PLAN B2, C2): the thread's own changes for a request are
//! measured together; tiny work gets no other-vendor review, work over the size line or
//! marked risky gets one review of the whole range, and the thread's answer waits for its
//! findings (`run_check` says so before it answers). With "Review the work" off nothing is
//! reviewed and nothing waits.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn, git};
use crate::manager::conversation::Envelope;
use crate::work::{InjectionKind, RequestState, ReviewState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// `lines` lines of text, each naming `tag`.
fn lines(tag: &str, lines: usize) -> String {
    (1..=lines).map(|n| format!("{tag} {n}\n")).collect()
}

/// Commits `file` (appending `text`) in the thread's workspace; its new tip.
fn commit(turn: &Turn, file: &str, text: &str, message: &str) -> String {
    let workspace = &turn.add_dirs[0];
    let path = workspace.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let before = std::fs::read_to_string(&path).unwrap_or_default();
    std::fs::write(&path, format!("{before}{text}")).unwrap();
    git(workspace, &["add", file]);
    git(workspace, &["commit", "-q", "-m", message]);
    git(workspace, &["rev-parse", "HEAD"])
}

/// A message for the running request, starting its next turn.
async fn continue_request(turn: &Turn, text: &str) {
    let id = turn.conversation().unwrap();
    let request = turn.host.request_for(&id, None).await;
    turn.host
        .deliver_for(
            &id,
            Envelope {
                kind: InjectionKind::Decision,
                label: "continue".into(),
                task_id: None,
                text: text.into(),
            },
            request,
        )
        .await;
}

async fn set_reviews(flow: &Flow, on: bool) {
    let mut settings = flow.core.settings();
    settings.review_work = on;
    flow.core.update_settings(settings).await.unwrap();
}

/// What a scripted thread saw and said.
#[derive(Default)]
struct Log {
    /// The workspace's tip before the work, then each commit's.
    commits: Vec<String>,
    /// `run_check`'s replies.
    checks: Vec<String>,
    /// Each turn's input, and its reply.
    turns: Vec<(String, String)>,
}

/// Four commits of 10 changed lines each, in 3 files, over two turns of one request: each is
/// under the line, and the turn between finds the first two still under it; together they
/// cross it, so `run_check` before the answer starts one review of their whole range and says
/// to wait. The answer comes once the review's word is in.
#[tokio::test]
async fn small_commits_of_one_request_get_one_review_of_their_combined_range() {
    let log: Arc<Mutex<Log>> = Arc::default();
    let seen = log.clone();
    let flow = Flow::start(
        "review-tiers-combined",
        Options::default(),
        script(move |turn| {
            let seen = seen.clone();
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("");
                }
                let reply = if turn.input.starts_with("Build the feature") {
                    let start = git(&turn.add_dirs[0], &["rev-parse", "HEAD"]);
                    let one = commit(&turn, "a.txt", &lines("a", 40), "One");
                    let two = commit(&turn, "b.txt", &lines("b", 40), "Two");
                    seen.lock().unwrap().commits.extend([start, one, two]);
                    continue_request(&turn, "Continue the feature.").await;
                    "[quiet]".to_owned()
                } else if turn.input.contains("Continue the feature.") {
                    // The turn before found the first two under the line: nothing reviewed.
                    let board = turn.host.core.board(&turn.conversation().unwrap()).await;
                    assert!(board.unwrap().reviews.is_empty());
                    let three = commit(&turn, "a.txt", &lines("a2", 40), "Three");
                    let four = commit(&turn, "c.txt", &lines("c", 40), "Four");
                    seen.lock().unwrap().commits.extend([three, four]);
                    let check = turn.call("run_check", json!({"command": "true"})).await;
                    assert!(!check.is_error, "{}", check.text);
                    seen.lock().unwrap().checks.push(check.text.clone());
                    if check.text.contains("[review running]") {
                        "[quiet]".to_owned()
                    } else {
                        "Built it.".to_owned()
                    }
                } else if turn
                    .input
                    .contains("[review of your commits · Codex found nothing")
                {
                    "Built it: four commits, and the review found nothing.".to_owned()
                } else {
                    "[quiet]".to_owned()
                };
                seen.lock()
                    .unwrap()
                    .turns
                    .push((turn.input.clone(), reply.clone()));
                Reply::text(reply)
            }
        }),
    )
    .await;
    flow.say("Build the feature.").await;
    let board = flow
        .until("the answer", |board| {
            board
                .requests
                .values()
                .any(|request| request.state == RequestState::Done)
        })
        .await;
    let log = std::mem::take(&mut *log.lock().unwrap());
    let reviews: Vec<_> = board.reviews.values().collect();
    assert_eq!(reviews.len(), 1, "{reviews:#?}");
    let review = reviews[0];
    assert!(review.task_id.is_none());
    assert_eq!(review.state, ReviewState::Clean);
    assert_eq!(
        (review.base.as_str(), review.tip.as_str()),
        (log.commits[0].as_str(), log.commits[4].as_str()),
        "the whole range of the four commits"
    );
    assert_eq!(log.checks.len(), 1);
    assert!(
        log.checks[0].contains("[review running] Codex reviews your change"),
        "{}",
        log.checks[0]
    );
    assert!(
        log.checks[0].contains("end this turn with exactly [quiet]"),
        "{}",
        log.checks[0]
    );
    // Every reply before the review's word was [quiet]; the answer came after it.
    let answered: Vec<usize> = log
        .turns
        .iter()
        .enumerate()
        .filter(|(_, (_, reply))| reply != "[quiet]")
        .map(|(at, _)| at)
        .collect();
    assert_eq!(answered.len(), 1, "{:#?}", log.turns);
    assert!(
        log.turns[answered[0]].0.contains("found nothing"),
        "{:#?}",
        log.turns
    );
    flow.stop().await;
}

/// A tiny edit with its test and a line in the docs gets no review: tests and docs don't count
/// toward the size line's files, and its 6 lines are under 30.
#[tokio::test]
async fn a_tiny_edit_with_its_test_and_docs_gets_no_review() {
    let checks: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = checks.clone();
    let flow = Flow::start(
        "review-tiers-tests-docs",
        Options {
            reviews: Some(script(|turn| async move {
                panic!(
                    "no review of a tiny edit with its test and docs: {}",
                    turn.input
                )
            })),
            ..Options::default()
        },
        script(move |turn| {
            let seen = seen.clone();
            async move {
                if turn.input.starts_with("Add the region") {
                    commit(&turn, "tax.ts", &lines("rate", 2), "Add the region");
                    commit(&turn, "tax.test.ts", &lines("test", 2), "Test it");
                    commit(&turn, "docs/rates.md", &lines("row", 2), "List it");
                    let check = turn.call("run_check", json!({"command": "true"})).await;
                    seen.lock().unwrap().push(check.text);
                    return Reply::text("Added the region.");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("Add the region.").await;
    let board = flow.settled().await;
    assert!(board.reviews.is_empty(), "{:#?}", board.reviews);
    let checks = checks.lock().unwrap().clone();
    assert!(!checks[0].contains("[review running]"), "{}", checks[0]);
    flow.stop().await;
}

/// One tiny commit gets no review: `run_check` says nothing of one, and the answer stands.
#[tokio::test]
async fn a_tiny_commit_gets_no_review() {
    let checks: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = checks.clone();
    let flow = Flow::start(
        "review-tiers-tiny",
        Options {
            reviews: Some(script(|turn| async move {
                panic!("no review of a tiny commit: {}", turn.input)
            })),
            ..Options::default()
        },
        script(move |turn| {
            let seen = seen.clone();
            async move {
                if turn.input.starts_with("Fix the typo") {
                    commit(&turn, "README.md", "typo fixed\n", "Fix the typo");
                    let check = turn.call("run_check", json!({"command": "true"})).await;
                    seen.lock().unwrap().push(check.text);
                    return Reply::text("Fixed the typo.");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("Fix the typo.").await;
    let board = flow.settled().await;
    assert!(board.reviews.is_empty(), "{:#?}", board.reviews);
    let checks = checks.lock().unwrap().clone();
    assert_eq!(checks.len(), 1);
    assert!(!checks[0].contains("[review running]"), "{}", checks[0]);
    assert!(
        board
            .requests
            .values()
            .all(|request| request.state == RequestState::Done)
    );
    flow.stop().await;
}

/// A tiny change the thread marks risky gets its review all the same, and the answer waits
/// for it.
#[tokio::test]
async fn a_tiny_change_marked_risky_gets_its_review() {
    let checks: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = checks.clone();
    let flow = Flow::start(
        "review-tiers-risky",
        Options::default(),
        script(move |turn| {
            let seen = seen.clone();
            async move {
                if turn.input.starts_with("Drop the old table") {
                    commit(
                        &turn,
                        "schema.sql",
                        "DROP TABLE old;\n",
                        "Drop the old table",
                    );
                    let check = turn
                        .call(
                            "run_check",
                            json!({"command": "true", "risky": "a migration that deletes data"}),
                        )
                        .await;
                    assert!(!check.is_error, "{}", check.text);
                    let waits = check.text.contains("[review running]");
                    seen.lock().unwrap().push(check.text);
                    return Reply::text(if waits { "[quiet]" } else { "Dropped it." });
                }
                if turn.input.contains("found nothing") {
                    return Reply::text("Dropped the old table; the review found nothing.");
                }
                Reply::text("[quiet]")
            }
        }),
    )
    .await;
    flow.say("Drop the old table.").await;
    let board = flow
        .until("the answer", |board| {
            board
                .requests
                .values()
                .any(|request| request.state == RequestState::Done)
        })
        .await;
    assert_eq!(board.reviews.len(), 1, "{:#?}", board.reviews);
    assert!(
        board
            .unreviewed
            .as_ref()
            .is_some_and(|range| range.risky.is_none())
    );
    let checks = checks.lock().unwrap().clone();
    assert!(checks[0].contains("[review running]"), "{}", checks[0]);
    flow.stop().await;
}

/// With "Review the work" off, nothing is reviewed, however big or risky, and nothing waits.
#[tokio::test]
async fn reviews_off_review_nothing_and_nothing_waits() {
    let checks: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = checks.clone();
    let flow = Flow::start(
        "review-tiers-off",
        Options {
            reviews: Some(script(|turn| async move {
                panic!("no review runs while reviews are off: {}", turn.input)
            })),
            ..Options::default()
        },
        script(move |turn| {
            let seen = seen.clone();
            async move {
                if turn.input.starts_with("Build it") {
                    for (n, file) in ["a.txt", "b.txt", "c.txt", "d.txt"].iter().enumerate() {
                        commit(&turn, file, &lines(file, 20), &format!("Part {n}"));
                    }
                    let check = turn
                        .call(
                            "run_check",
                            json!({"command": "true", "risky": "persistence"}),
                        )
                        .await;
                    seen.lock().unwrap().push(check.text);
                    return Reply::text("Built it.");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    set_reviews(&flow, false).await;
    flow.say("Build it.").await;
    let board = flow.settled().await;
    assert!(board.reviews.is_empty(), "{:#?}", board.reviews);
    let checks = checks.lock().unwrap().clone();
    assert!(!checks[0].contains("[review running]"), "{}", checks[0]);
    assert!(
        board
            .requests
            .values()
            .all(|request| request.state == RequestState::Done)
    );
    flow.stop().await;
}

/// The assistant messages the user sees, oldest first.
async fn answers(flow: &Flow) -> Vec<crate::model::Message> {
    let head = flow.core.head(&flow.conversation).await.unwrap().unwrap();
    flow.core
        .branch(&flow.conversation, &head)
        .await
        .unwrap()
        .into_iter()
        .filter(|message| message.role == crate::model::MessageRole::Assistant)
        .collect()
}

/// BUILD-PLAN C2: the thread calls `run_check` before answering, with its change over the line:
/// the reply says a review runs and to wait, so the thread ends on [quiet]. No answer is
/// published while the review runs; its finding comes as a message, the thread fixes it in a
/// commit that names it, and only then answers.
#[tokio::test]
async fn the_answer_waits_for_the_reviews_findings_and_their_fix() {
    let release = Arc::new(tokio::sync::Notify::new());
    let held = release.clone();
    let order: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = order.clone();
    let flow = Flow::start(
        "review-tiers-answer-waits",
        Options {
            reviews: Some(script(move |_| {
                let held = held.clone();
                async move {
                    held.notified().await;
                    Reply::text("- [P2] a.txt has no title — a.txt:1\n  Add one.")
                }
            })),
            ..Options::default()
        },
        script(move |turn| {
            let seen = seen.clone();
            async move {
                if turn.input.starts_with("Build the feature") {
                    commit(&turn, "a.txt", &lines("a", 160), "Build the feature");
                    let check = turn.call("run_check", json!({"command": "true"})).await;
                    assert!(check.text.contains("[review running]"), "{}", check.text);
                    seen.lock().unwrap().push("waited".into());
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[review of your commits · Codex found 1") {
                    commit(
                        &turn,
                        "a.txt",
                        "# Title\n",
                        "Give a.txt a title\n\nBrigadier-Fixes: R1.1",
                    );
                    seen.lock().unwrap().push("fixed".into());
                    let recorded = turn
                        .call(
                            "record_findings",
                            json!({"items": [{"id": "R1.1", "status": "fixed", "checked": "re-read a.txt:1"}]}),
                        )
                        .await;
                    assert!(!recorded.is_error, "{}", recorded.text);
                    seen.lock().unwrap().push("answered".into());
                    return Reply::text("Built the feature, and fixed the review's finding (R1.1).");
                }
                Reply::text("[quiet]")
            }
        }),
    )
    .await;
    flow.say("Build the feature.").await;
    let board = flow
        .until("the review to run, with the thread idle", |board| {
            board.run == crate::work::RunState::Idle
                && board
                    .reviews
                    .values()
                    .any(|review| review.state == ReviewState::Running)
        })
        .await;
    let request = board.latest_request().unwrap().id.clone();
    assert_eq!(board.requests[&request].state, RequestState::Working);
    assert!(
        answers(&flow).await.is_empty(),
        "no answer while the review runs"
    );
    release.notify_one();
    flow.until("the answer", |board| {
        board.requests[&request].state == RequestState::Done
    })
    .await;
    let board = flow.settled().await;
    assert_eq!(
        order.lock().unwrap().clone(),
        ["waited", "fixed", "answered"]
    );
    let answers = answers(&flow).await;
    assert_eq!(answers.len(), 1, "{answers:#?}");
    assert!(answers[0].text.contains("fixed the review's finding"));
    let review = board.reviews.values().next().unwrap();
    assert!(answers[0].created_at_ms >= review.ended_at_ms.unwrap());
    // The fix named its finding: it got no review of its own.
    assert_eq!(board.reviews.len(), 1, "{:#?}", board.reviews);
    flow.stop().await;
}

/// C2 in the order a thread works: it edits three files, runs its checks with `run_check`
/// before it commits, then commits and answers. The uncommitted change is over the line, so
/// `run_check` starts the review of a snapshot of it and says to wait; the commit with the
/// same files that follows needs no review of its own, and the answer comes after the
/// review's word.
#[tokio::test]
async fn a_change_checked_before_its_commit_is_reviewed_before_the_answer() {
    let log: Arc<Mutex<Log>> = Arc::default();
    let seen = log.clone();
    let flow = Flow::start(
        "review-tiers-uncommitted",
        Options::default(),
        script(move |turn| {
            let seen = seen.clone();
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("");
                }
                let reply = if turn.input.starts_with("Add the region") {
                    let workspace = turn.add_dirs[0].clone();
                    for file in ["tax.ts", "rates.ts", "region.ts"] {
                        std::fs::write(workspace.join(file), lines(file, 60)).unwrap();
                    }
                    let check = turn.call("run_check", json!({"command": "true"})).await;
                    assert!(!check.is_error, "{}", check.text);
                    seen.lock().unwrap().checks.push(check.text.clone());
                    git(&workspace, &["add", "-A"]);
                    git(&workspace, &["commit", "-q", "-m", "Add the region"]);
                    let tip = git(&workspace, &["rev-parse", "HEAD"]);
                    seen.lock().unwrap().commits.push(tip);
                    if check.text.contains("[review running]") {
                        "[quiet]".to_owned()
                    } else {
                        "Added the region.".to_owned()
                    }
                } else if turn
                    .input
                    .contains("[review of your commits · Codex found nothing")
                {
                    "Added the region; the review found nothing.".to_owned()
                } else {
                    "[quiet]".to_owned()
                };
                seen.lock()
                    .unwrap()
                    .turns
                    .push((turn.input.clone(), reply.clone()));
                Reply::text(reply)
            }
        }),
    )
    .await;
    flow.say("Add the region.").await;
    flow.until("the answer", |board| {
        board
            .requests
            .values()
            .any(|request| request.state == RequestState::Done)
    })
    .await;
    let board = flow.settled().await;
    let log = std::mem::take(&mut *log.lock().unwrap());
    assert!(
        log.checks[0].contains("[review running] Codex reviews your change"),
        "{}",
        log.checks[0]
    );
    // One review, of the files the commit then held.
    let reviews: Vec<_> = board.reviews.values().collect();
    assert_eq!(reviews.len(), 1, "{reviews:#?}");
    assert_eq!(reviews[0].state, ReviewState::Clean);
    let tree = |rev: &str| git(&flow.repo, &["rev-parse", &format!("{rev}^{{tree}}")]);
    assert_eq!(tree(&reviews[0].tip), tree(&log.commits[0]));
    let answered: Vec<&(String, String)> = log
        .turns
        .iter()
        .filter(|(_, reply)| reply != "[quiet]")
        .collect();
    assert_eq!(answered.len(), 1, "{:#?}", log.turns);
    assert!(answered[0].0.contains("found nothing"), "{:#?}", log.turns);
    let shown = answers(&flow).await;
    assert_eq!(
        shown.last().map(|message| message.text.as_str()),
        Some("Added the region; the review found nothing.")
    );
    flow.stop().await;
}

/// A thread that ran `run_check` on a change over the line and waits, with [quiet], for the
/// review it started.
fn waiting_thread(answered: Arc<Mutex<Vec<String>>>) -> Script {
    script(move |turn| {
        let answered = answered.clone();
        async move {
            if turn.input.starts_with("Build the feature") {
                commit(&turn, "a.txt", &lines("a", 160), "Build the feature");
                let check = turn.call("run_check", json!({"command": "true"})).await;
                assert!(check.text.contains("[review running]"), "{}", check.text);
                return Reply::text("[quiet]");
            }
            if turn.input.contains("found nothing") {
                answered.lock().unwrap().push(turn.input.clone());
                return Reply::text("Built the feature; the review found nothing.");
            }
            Reply::text("[quiet]")
        }
    })
}

/// The review `run_check` told the thread to wait for is recorded so: it ended clean, but
/// Brigadier stopped before its word reached a turn. After the restart the word goes again
/// and the request gets its answer, instead of ending with none.
#[tokio::test]
async fn an_awaited_review_that_ended_before_a_restart_still_brings_the_answer() {
    let answered: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut flow = Flow::start(
        "review-tiers-awaited-restart",
        Options {
            reviews: Some(script(|_| std::future::pending())),
            ..Options::default()
        },
        waiting_thread(answered.clone()),
    )
    .await;
    flow.say("Build the feature.").await;
    let board = flow
        .until("the review to run, with the thread idle", |board| {
            board.run == crate::work::RunState::Idle
                && board
                    .reviews
                    .values()
                    .any(|review| review.state == ReviewState::Running)
        })
        .await;
    let request = board.latest_request().unwrap().id.clone();
    let mut review = board.reviews.values().next().unwrap().clone();
    assert!(
        review.answer_waits,
        "run_check records that the answer waits"
    );
    // Its end is recorded; the daemon dies before the thread hears it.
    review.state = ReviewState::Clean;
    review.ended_at_ms = Some(crate::now_ms());
    flow.core
        .record_conversation(
            &flow.conversation,
            vec![crate::model::DomainEvent::ReviewUpdated { review }],
        )
        .await
        .unwrap();
    flow.crash().await;
    let board = flow
        .until("the answer", |board| {
            board.requests[&request].state == RequestState::Done
        })
        .await;
    assert_eq!(answered.lock().unwrap().len(), 1);
    let shown = answers(&flow).await;
    assert_eq!(
        shown.last().map(|message| message.text.as_str()),
        Some("Built the feature; the review found nothing.")
    );
    assert_eq!(board.reviews.len(), 1, "{:#?}", board.reviews);
    flow.stop().await;
}

/// The review's findings were recorded, but Brigadier stopped before a turn of the thread
/// took them from its inbox (#69): after the restart they go again, once, and the request
/// gets its answer.
#[tokio::test]
async fn findings_the_thread_had_not_taken_before_a_restart_still_reach_it() {
    let answered: Arc<Mutex<Vec<String>>> = Arc::default();
    let heard = answered.clone();
    let mut flow = Flow::start(
        "review-tiers-findings-restart",
        Options {
            reviews: Some(script(|_| std::future::pending())),
            ..Options::default()
        },
        script(move |turn| {
            let heard = heard.clone();
            async move {
                if turn.input.starts_with("Build the feature") {
                    commit(&turn, "a.txt", &lines("a", 160), "Build the feature");
                    let check = turn.call("run_check", json!({"command": "true"})).await;
                    assert!(check.text.contains("[review running]"), "{}", check.text);
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("found 1") {
                    heard.lock().unwrap().push(turn.input.clone());
                    return Reply::text("Built the feature; the one finding is not a bug.");
                }
                Reply::text("[quiet]")
            }
        }),
    )
    .await;
    flow.say("Build the feature.").await;
    let board = flow
        .until("the review to run, with the thread idle", |board| {
            board.run == crate::work::RunState::Idle
                && board
                    .reviews
                    .values()
                    .any(|review| review.state == ReviewState::Running)
        })
        .await;
    let request = board.latest_request().unwrap().id.clone();
    let mut review = board.reviews.values().next().unwrap().clone();
    // Its findings are recorded; the daemon dies before the thread hears them.
    let text = "- [P2] The loop never ends — a.txt:3\n  It counts up from 0 forever.";
    let hash = flow
        .core
        .store()
        .blobs()
        .put(text.as_bytes().to_vec())
        .await
        .unwrap();
    review.findings = Some(hash.to_string());
    review.state = ReviewState::Findings { count: 1 };
    review.ended_at_ms = Some(crate::now_ms());
    flow.core
        .record_conversation(
            &flow.conversation,
            vec![crate::model::DomainEvent::ReviewUpdated { review }],
        )
        .await
        .unwrap();
    flow.crash().await;
    flow.until("the answer", |board| {
        board.requests[&request].state == RequestState::Done
    })
    .await;
    let heard = answered.lock().unwrap().clone();
    assert_eq!(heard.len(), 1, "{heard:#?}");
    assert!(heard[0].contains("The loop never ends"), "{}", heard[0]);
    flow.stop().await;
}

/// Any other message for the thread (a worker's report, a decision) that Brigadier stopped
/// before a turn took it from the inbox goes again after the restart, once (#210).
#[tokio::test]
async fn a_message_the_thread_had_not_taken_before_a_restart_still_reaches_it() {
    let heard: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = heard.clone();
    let mut flow = Flow::start(
        "inbox-restart",
        Options {
            reviews: Some(script(|_| std::future::pending())),
            ..Options::default()
        },
        script(move |turn| {
            let seen = seen.clone();
            async move {
                if turn.input.starts_with("Build the feature") {
                    commit(&turn, "a.txt", &lines("a", 160), "Build the feature");
                    let check = turn.call("run_check", json!({"command": "true"})).await;
                    assert!(check.text.contains("[review running]"), "{}", check.text);
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("The build server is back.")
                    || turn.input.contains("The cache is warm.")
                {
                    seen.lock().unwrap().push(turn.input.clone());
                    return Reply::text("Built the feature.");
                }
                Reply::text("[quiet]")
            }
        }),
    )
    .await;
    flow.say("Build the feature.").await;
    let board = flow
        .until("the review to run, with the thread idle", |board| {
            board.run == crate::work::RunState::Idle
                && board
                    .reviews
                    .values()
                    .any(|review| review.state == ReviewState::Running)
        })
        .await;
    let request = board.latest_request().unwrap().id.clone();
    // Queued for the thread's next turn, two of the same label; the daemon dies before a turn
    // takes them.
    for text in ["The build server is back.", "The cache is warm."] {
        let note = Envelope {
            kind: InjectionKind::Decision,
            label: "build server".into(),
            task_id: None,
            text: format!("[build server] {text}"),
        };
        flow.manager
            .queue_envelope(&flow.conversation, note, Some(request.clone()))
            .await
            .unwrap();
    }
    flow.crash().await;
    flow.until("the answer", |board| {
        board.requests[&request].state == RequestState::Done
    })
    .await;
    let both = || {
        let heard = heard.lock().unwrap().join("\n");
        heard.contains("The build server is back.") && heard.contains("The cache is warm.")
    };
    tokio::time::timeout(*super::PATIENCE, async {
        while !both() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("both messages heard: {:#?}", heard.lock().unwrap()));
    let heard = heard.lock().unwrap().join("\n");
    for text in ["The build server is back.", "The cache is warm."] {
        assert_eq!(heard.matches(text).count(), 1, "{heard}");
    }
    flow.stop().await;
}

/// The user stops a request whose answer waits for its review: when the review then ends
/// clean, nothing wakes the thread to answer, and the request stays stopped.
#[tokio::test]
async fn a_stopped_request_is_not_woken_by_its_clean_review() {
    let release = Arc::new(tokio::sync::Notify::new());
    let held = release.clone();
    let answered: Arc<Mutex<Vec<String>>> = Arc::default();
    let flow = Flow::start(
        "review-tiers-awaited-stop",
        Options {
            reviews: Some(script(move |_| {
                let held = held.clone();
                async move {
                    held.notified().await;
                    Reply::text("No findings.")
                }
            })),
            ..Options::default()
        },
        waiting_thread(answered.clone()),
    )
    .await;
    flow.say("Build the feature.").await;
    let board = flow
        .until("the review to run, with the thread idle", |board| {
            board.run == crate::work::RunState::Idle
                && board
                    .reviews
                    .values()
                    .any(|review| review.state == ReviewState::Running)
        })
        .await;
    let request = board.latest_request().unwrap().id.clone();
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    flow.until("the request stopped", |board| {
        board.requests[&request].state == RequestState::Stopped
    })
    .await;
    release.notify_one();
    flow.until("the review's end", |board| {
        board
            .reviews
            .values()
            .all(|review| review.state == ReviewState::Clean)
    })
    .await;
    let board = flow.settled().await;
    assert_eq!(board.requests[&request].state, RequestState::Stopped);
    assert!(answered.lock().unwrap().is_empty());
    assert!(answers(&flow).await.is_empty());
    flow.stop().await;
}

/// The thread checks its uncommitted change before committing, as threads do: the review of
/// that snapshot finds something, and the thread fixes it and commits everything in one commit
/// without the fix trailer. That commit holds the fix, not new work: no second review of the
/// whole range (no review rounds).
#[tokio::test]
async fn a_commit_that_fixes_a_snapshots_findings_gets_no_second_review() {
    let flow = Flow::start(
        "review-tiers-snapshot-fix",
        Options {
            reviews: Some(script(|_| async {
                Reply::text("- [P2] tax.ts has no title — tax.ts:1\n  Add one.")
            })),
            ..Options::default()
        },
        script(|turn| async move {
            if !turn.is_orchestrator() {
                return Reply::text("");
            }
            let workspace = turn.add_dirs[0].clone();
            if turn.input.starts_with("Add the region") {
                for file in ["tax.ts", "rates.ts", "region.ts"] {
                    std::fs::write(workspace.join(file), lines(file, 60)).unwrap();
                }
                let check = turn.call("run_check", json!({"command": "true"})).await;
                assert!(check.text.contains("[review running]"), "{}", check.text);
                return Reply::text("[quiet]");
            }
            if turn
                .input
                .contains("[review of your commits · Codex found 1")
            {
                std::fs::write(
                    workspace.join("tax.ts"),
                    format!("// Tax\n{}", lines("tax.ts", 60)),
                )
                .unwrap();
                git(&workspace, &["add", "-A"]);
                git(&workspace, &["commit", "-q", "-m", "Add the region"]);
                return Reply::text("Added the region and fixed the review's finding.");
            }
            Reply::text("[quiet]")
        }),
    )
    .await;
    flow.say("Add the region.").await;
    flow.until("the answer", |board| {
        board
            .requests
            .values()
            .any(|request| request.state == RequestState::Done)
    })
    .await;
    let board = flow.settled().await;
    assert_eq!(board.reviews.len(), 1, "{:#?}", board.reviews);
    flow.stop().await;
}

/// The review of the uncommitted snapshot finds nothing, then the thread writes more before it
/// commits: the commit gets a review of only what it holds beyond the snapshot, never the whole
/// range again.
#[tokio::test]
async fn a_commit_beyond_a_reviewed_snapshot_is_reviewed_only_from_the_snapshot() {
    let flow = Flow::start(
        "review-tiers-snapshot-more",
        Options::default(),
        script(|turn| async move {
            if !turn.is_orchestrator() {
                return Reply::text("");
            }
            let workspace = turn.add_dirs[0].clone();
            if turn.input.starts_with("Add the region") {
                for file in ["tax.ts", "rates.ts", "region.ts"] {
                    std::fs::write(workspace.join(file), lines(file, 60)).unwrap();
                }
                let check = turn.call("run_check", json!({"command": "true"})).await;
                assert!(check.text.contains("[review running]"), "{}", check.text);
                return Reply::text("[quiet]");
            }
            if turn.input.contains("found nothing") && !workspace.join("report.ts").exists() {
                std::fs::write(workspace.join("report.ts"), lines("report.ts", 160)).unwrap();
                git(&workspace, &["add", "-A"]);
                git(
                    &workspace,
                    &["commit", "-q", "-m", "Add the region and its report"],
                );
                let check = turn.call("run_check", json!({"command": "true"})).await;
                if check.text.contains("[review running]") {
                    return Reply::text("[quiet]");
                }
                return Reply::text("Added the region and its report.");
            }
            if turn.input.contains("found nothing") {
                return Reply::text("Added the region and its report; the reviews found nothing.");
            }
            Reply::text("[quiet]")
        }),
    )
    .await;
    flow.say("Add the region.").await;
    flow.until("the answer", |board| {
        board
            .requests
            .values()
            .any(|request| request.state == RequestState::Done)
    })
    .await;
    let board = flow.settled().await;
    let mut reviews: Vec<_> = board.reviews.values().collect();
    reviews.sort_by_key(|review| review.number);
    assert_eq!(reviews.len(), 2, "{reviews:#?}");
    let later = reviews[1];
    let names = git(
        &flow.repo,
        &[
            "diff",
            "--name-only",
            &format!("{}...{}", later.base, later.tip),
        ],
    );
    assert_eq!(
        names, "report.ts",
        "only what the commit added beyond the snapshot"
    );
    flow.stop().await;
}

/// The review line (the user, Q48): 150 changed lines of code or more than 5 files of code,
/// tests and docs aside. 145 lines over 5 code files, with a long test and a doc beside them,
/// get no review; a sixth code file of a few lines in the same request crosses it.
#[tokio::test]
async fn the_review_line_is_150_lines_or_more_than_five_files_of_code() {
    let checks: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = checks.clone();
    let flow = Flow::start(
        "review-tiers-q48",
        Options::default(),
        script(move |turn| {
            let seen = seen.clone();
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("");
                }
                if turn.input.starts_with("Build the feature") {
                    for file in ["a.ts", "b.ts", "c.ts", "d.ts", "e.ts"] {
                        commit(&turn, file, &lines(file, 29), &format!("Add {file}"));
                    }
                    commit(&turn, "a.test.ts", &lines("test", 120), "Test it");
                    commit(&turn, "docs/feature.md", &lines("doc", 40), "Document it");
                    let check = turn.call("run_check", json!({"command": "true"})).await;
                    seen.lock().unwrap().push(check.text);
                    continue_request(&turn, "Add the sixth part.").await;
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("Add the sixth part.") {
                    commit(&turn, "f.ts", &lines("f.ts", 3), "Add f.ts");
                    let check = turn.call("run_check", json!({"command": "true"})).await;
                    let waits = check.text.contains("[review running]");
                    seen.lock().unwrap().push(check.text);
                    return Reply::text(if waits { "[quiet]" } else { "Built it." });
                }
                if turn.input.contains("found nothing") {
                    return Reply::text("Built it; the review found nothing.");
                }
                Reply::text("[quiet]")
            }
        }),
    )
    .await;
    flow.say("Build the feature.").await;
    flow.until("the answer", |board| {
        board
            .requests
            .values()
            .any(|request| request.state == RequestState::Done)
    })
    .await;
    let board = flow.settled().await;
    let checks = checks.lock().unwrap().clone();
    assert!(!checks[0].contains("[review running]"), "{}", checks[0]);
    assert!(checks[1].contains("[review running]"), "{}", checks[1]);
    assert_eq!(board.reviews.len(), 1, "{:#?}", board.reviews);
    flow.stop().await;
}

/// What review_candidate answered, and what the thread heard besides.
#[derive(Default)]
struct Candidate {
    commits: Vec<String>,
    replies: Vec<String>,
    inputs: Vec<String>,
}

/// review_candidate on a 10-line change of the thread's own: the other vendor reviews it, and
/// its finding comes back in the call's reply, never as a message. Asked again on the same
/// commit, the review that covers it answers and nothing new starts. After the fix, only the
/// fix's range is reviewed, as a re-check, and it comes back clean.
#[tokio::test]
async fn review_candidate_reviews_a_small_change_reuses_coverage_and_rechecks_only_the_fix() {
    let log: Arc<Mutex<Candidate>> = Arc::default();
    let seen = log.clone();
    let flow = Flow::start(
        "review-candidate",
        Options {
            reviews: Some(script(|turn| async move {
                let text = std::fs::read_to_string(turn.cwd.join("a.txt"))
                    .expect("the reviewer's checkout holds the candidate");
                if text.contains("fixed") {
                    Reply::text("No findings.")
                } else {
                    Reply::text("- [P2] a.txt never says it is fixed — a.txt:1\n  Say so.")
                }
            })),
            ..Options::default()
        },
        script(move |turn| {
            let seen = seen.clone();
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("");
                }
                seen.lock().unwrap().inputs.push(turn.input.clone());
                if !turn.input.starts_with("Build the feature") {
                    return Reply::text("[quiet]");
                }
                let one = commit(&turn, "a.txt", &lines("a", 10), "Ten lines");
                let first = turn.call("review_candidate", json!({})).await;
                assert!(!first.is_error, "{}", first.text);
                let again = turn.call("review_candidate", json!({"commit": one})).await;
                assert!(!again.is_error, "{}", again.text);
                let two = commit(&turn, "a.txt", "fixed\n", "Say it is fixed");
                let fix = turn.call("review_candidate", json!({})).await;
                assert!(!fix.is_error, "{}", fix.text);
                let mut seen = seen.lock().unwrap();
                seen.commits.extend([one, two]);
                seen.replies.extend([first.text, again.text, fix.text]);
                Reply::text("Built it, reviewed, fixed.")
            }
        }),
    )
    .await;
    flow.say("Build the feature.").await;
    let board = flow.settled().await;
    let log = std::mem::take(&mut *log.lock().unwrap());
    // The finding in the first reply, the same review's outcome in the second.
    assert!(log.replies[0].contains("found 1"), "{}", log.replies[0]);
    assert!(
        log.replies[0].contains("a.txt never says it is fixed"),
        "{}",
        log.replies[0]
    );
    assert!(log.replies[1].contains("found 1"), "{}", log.replies[1]);
    assert!(
        log.replies[2].contains("found nothing"),
        "{}",
        log.replies[2]
    );
    let mut reviews: Vec<_> = board.reviews.values().collect();
    reviews.sort_by_key(|review| review.number);
    assert_eq!(reviews.len(), 2, "{reviews:#?}");
    assert!(matches!(
        reviews[0].state,
        ReviewState::Findings { count: 1 }
    ));
    assert_eq!(reviews[0].tip, log.commits[0]);
    // The re-check reads the fix alone.
    assert_eq!(
        (reviews[1].base.as_str(), reviews[1].tip.as_str()),
        (log.commits[0].as_str(), log.commits[1].as_str())
    );
    assert_eq!(reviews[1].state, ReviewState::Clean);
    // Nothing came as a message besides.
    assert!(
        !log.inputs.iter().any(|input| input.contains("[review of")),
        "{:#?}",
        log.inputs
    );
    flow.stop().await;
}

/// With no other-vendor reviewer, and with reviews off, review_candidate says so as a fact:
/// the thread decides what to do.
#[tokio::test]
async fn review_candidate_says_when_no_reviewer_can_take_it_or_reviews_are_off() {
    let log: Arc<Mutex<Candidate>> = Arc::default();
    let seen = log.clone();
    let flow = Flow::start(
        "review-candidate-none",
        Options::default(),
        script(move |turn| {
            let seen = seen.clone();
            async move {
                if !turn.is_orchestrator()
                    || !(turn.input.contains("Change one.") || turn.input.contains("Change two."))
                {
                    return Reply::text("[quiet]");
                }
                commit(&turn, "a.txt", &lines(&turn.input, 3), "Three lines");
                let reply = turn.call("review_candidate", json!({})).await;
                assert!(!reply.is_error, "{}", reply.text);
                seen.lock().unwrap().replies.push(reply.text);
                Reply::text("Changed it.")
            }
        }),
    )
    .await;
    let mut settings = flow.core.settings();
    settings
        .disabled_providers
        .push(brigadier_providers::ProviderKind::Codex);
    flow.core.update_settings(settings).await.unwrap();
    flow.say("Change one.").await;
    flow.settled().await;
    set_reviews(&flow, false).await;
    flow.say("Change two.").await;
    flow.settled().await;
    let replies = std::mem::take(&mut log.lock().unwrap().replies);
    assert!(
        replies[0].contains("No other-vendor review could run"),
        "{}",
        replies[0]
    );
    assert!(replies[0].contains("yours"), "{}", replies[0]);
    assert!(replies[1].contains("Reviews are off"), "{}", replies[1]);
    flow.stop().await;
}

/// A review_candidate call cancelled just as its review ends (its CLI's bridge closes, a
/// handoff), after the review's message stood aside for the call: the thread still hears the
/// findings, as a message (#332).
#[tokio::test]
async fn a_review_candidate_call_cancelled_as_its_review_ends_still_tells_the_findings() {
    let manager: Arc<std::sync::OnceLock<Arc<crate::manager::SessionManager>>> = Arc::default();
    let log: Arc<Mutex<Candidate>> = Arc::default();
    let (seen, waits) = (log.clone(), manager.clone());
    let flow = Flow::start(
        "review-candidate-cancelled",
        Options {
            reviews: Some(script(|_| async {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                Reply::text("- [P2] a.txt never says it is fixed — a.txt:1\n  Say so.")
            })),
            ..Options::default()
        },
        script(move |turn| {
            let (seen, waits) = (seen.clone(), waits.clone());
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("");
                }
                seen.lock().unwrap().inputs.push(turn.input.clone());
                if !turn.input.starts_with("Build the feature") {
                    return Reply::text("[quiet]");
                }
                commit(&turn, "a.txt", &lines("a", 10), "Ten lines");
                let claims = || {
                    waits
                        .get()
                        .unwrap()
                        .review_replies
                        .lock()
                        .unwrap()
                        .waiting()
                };
                // The call claims the review's outcome; its end then takes the claim, its
                // message skipped for the call's reply. The call is cancelled at once.
                let stood_aside = async {
                    while claims() == 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                    }
                    while claims() > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                    }
                };
                tokio::select! {
                    biased;
                    () = stood_aside => Reply::text("[quiet]"),
                    reply = turn.call("review_candidate", json!({})) => {
                        seen.lock().unwrap().replies.push(reply.text);
                        Reply::text("Reviewed.")
                    }
                }
            }
        }),
    )
    .await;
    assert!(manager.set(flow.manager.clone()).is_ok());
    flow.say("Build the feature.").await;
    let heard = |log: &Candidate| {
        log.inputs
            .iter()
            .chain(&log.replies)
            .any(|text| text.contains("a.txt never says it is fixed"))
    };
    flow.until("the thread to hear the findings", |_| {
        heard(&log.lock().unwrap())
    })
    .await;
    flow.settled().await;
    flow.stop().await;
}
