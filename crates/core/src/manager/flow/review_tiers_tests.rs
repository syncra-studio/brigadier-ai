//! Reviews the thread asks for (`request_review`): Brigadier starts none by itself. A review
//! the thread's answer waits for still brings it after a restart, one whose call went on
//! reaches the thread whatever it did meanwhile (#513), and one of a stopped request wakes
//! nothing; request_review reuses what reviews cover and re-checks only a fix.

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
    let workspace = &turn.cwd;
    let path = workspace.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let before = std::fs::read_to_string(&path).unwrap_or_default();
    std::fs::write(&path, format!("{before}{text}")).unwrap();
    git(workspace, &["add", file]);
    git(workspace, &["commit", "-q", "-m", message]);
    git(workspace, &["rev-parse", "HEAD"])
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

/// A thread that asks for a review of its change and waits, with [quiet], for its findings.
fn waiting_thread(answered: Arc<Mutex<Vec<String>>>) -> Script {
    script(move |turn| {
        let answered = answered.clone();
        async move {
            if turn.input.starts_with("Build the feature") {
                commit(&turn, "a.txt", &lines("a", 160), "Build the feature");
                let asked = turn
                    .call("request_review", json!({"wait_seconds": 0}))
                    .await;
                assert!(asked.text.contains("still reviews"), "{}", asked.text);
                return Reply::text("");
            }
            if turn.input.contains("found nothing") {
                answered.lock().unwrap().push(turn.input.clone());
                return Reply::text("Built the feature; the review found nothing.");
            }
            Reply::text("")
        }
    })
}

/// The review request_review told the thread to wait for is recorded so: it ended clean, but
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
        "request_review records that the answer waits"
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
                    let asked = turn
                        .call("request_review", json!({"wait_seconds": 0}))
                        .await;
                    assert!(asked.text.contains("still reviews"), "{}", asked.text);
                    return Reply::text("");
                }
                if turn.input.contains("found 1") {
                    heard.lock().unwrap().push(turn.input.clone());
                    return Reply::text("Built the feature; the one finding is not a bug.");
                }
                Reply::text("")
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
                    let asked = turn
                        .call("request_review", json!({"wait_seconds": 0}))
                        .await;
                    assert!(asked.text.contains("still reviews"), "{}", asked.text);
                    return Reply::text("");
                }
                if turn.input.contains("The build server is back.")
                    || turn.input.contains("The cache is warm.")
                {
                    seen.lock().unwrap().push(turn.input.clone());
                    return Reply::text("Built the feature.");
                }
                Reply::text("")
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

/// A thread that asks for a review of its change and answers instead of waiting with [quiet],
/// once `go` is set; each turn's input is logged.
fn answering_thread(
    inputs: Arc<Mutex<Vec<String>>>,
    go: Arc<std::sync::atomic::AtomicBool>,
) -> Script {
    script(move |turn| {
        let (inputs, go) = (inputs.clone(), go.clone());
        async move {
            inputs.lock().unwrap().push(turn.input.clone());
            if turn.input.starts_with("Build the feature") {
                commit(&turn, "a.txt", &lines("a", 160), "Build the feature");
                let asked = turn
                    .call("request_review", json!({"wait_seconds": 0}))
                    .await;
                assert!(asked.text.contains("still reviews"), "{}", asked.text);
                while !go.load(std::sync::atomic::Ordering::SeqCst) {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                return Reply::text("Built the feature; its review still runs.");
            }
            Reply::text("Noted.")
        }
    })
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

/// A review held until the returned notify, then clean.
fn held_clean_review() -> (Arc<tokio::sync::Notify>, Script) {
    let release = Arc::new(tokio::sync::Notify::new());
    let held = release.clone();
    let reviews = script(move |_| {
        let held = held.clone();
        async move {
            held.notified().await;
            Reply::text("No findings.")
        }
    });
    (release, reviews)
}

/// #513: request_review's call went on and the thread answered instead of waiting: the
/// review's clean end, after the answer, reaches the thread as a message once.
#[tokio::test]
async fn a_background_clean_review_reaches_a_thread_that_answered_once() {
    let (release, reviews) = held_clean_review();
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let flow = Flow::start(
        "review-tiers-answered-clean",
        Options {
            reviews: Some(reviews),
            ..Options::default()
        },
        answering_thread(inputs.clone(), Arc::new(true.into())),
    )
    .await;
    flow.say("Build the feature.").await;
    let board = flow
        .until("the answer, with the review still running", |board| {
            board.run == crate::work::RunState::Idle
                && !inputs.lock().unwrap().is_empty()
                && board
                    .reviews
                    .values()
                    .any(|review| review.state == ReviewState::Running)
        })
        .await;
    let request = board.latest_request().unwrap().id.clone();
    release.notify_one();
    flow.until("the clean review came as a message", |_| {
        heard(&inputs, "found nothing") > 0
    })
    .await;
    let board = flow.settled().await;
    assert_eq!(heard(&inputs, "found nothing"), 1);
    assert_eq!(board.requests[&request].state, RequestState::Done);
    flow.stop().await;
}

/// #513: a background review that ends clean while the turn that asked for it still runs,
/// when that turn then answers instead of replying [quiet], still comes as a message once.
#[tokio::test]
async fn a_background_clean_review_before_the_threads_answer_still_comes_once() {
    let (release, reviews) = held_clean_review();
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let go = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flow = Flow::start(
        "review-tiers-answered-clean-early",
        Options {
            reviews: Some(reviews),
            ..Options::default()
        },
        answering_thread(inputs.clone(), go.clone()),
    )
    .await;
    flow.say("Build the feature.").await;
    flow.until("the review to run", |board| {
        board
            .reviews
            .values()
            .any(|review| review.state == ReviewState::Running && review.answer_waits)
    })
    .await;
    release.notify_one();
    flow.until("the review ended clean while the turn ran", |board| {
        board.run != crate::work::RunState::Idle
            && board
                .reviews
                .values()
                .all(|review| review.state == ReviewState::Clean)
    })
    .await;
    go.store(true, std::sync::atomic::Ordering::SeqCst);
    flow.until("the clean review came as a message", |_| {
        heard(&inputs, "found nothing") > 0
    })
    .await;
    flow.settled().await;
    assert_eq!(heard(&inputs, "found nothing"), 1);
    flow.stop().await;
}

/// #513: a background review's clean end recorded after the thread answered and its request
/// read done, just before Brigadier stopped and before any turn took it, reaches the thread
/// after the restart, once: not again after a second restart.
#[tokio::test]
async fn a_background_clean_review_no_turn_took_before_a_restart_comes_after_it_once() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut flow = Flow::start(
        "review-tiers-answered-clean-restart",
        Options {
            reviews: Some(script(|_| std::future::pending())),
            ..Options::default()
        },
        answering_thread(inputs.clone(), Arc::new(true.into())),
    )
    .await;
    flow.say("Build the feature.").await;
    let board = flow
        .until("the answer, with the review still running", |board| {
            board.run == crate::work::RunState::Idle
                && !inputs.lock().unwrap().is_empty()
                && board
                    .reviews
                    .values()
                    .any(|review| review.state == ReviewState::Running)
        })
        .await;
    let request = board.latest_request().unwrap().id.clone();
    let mut review = board.reviews.values().next().unwrap().clone();
    assert!(review.answer_waits);
    // Its clean end is recorded and the answered request reads done; the daemon dies before
    // the thread hears it.
    review.state = ReviewState::Clean;
    review.ended_at_ms = Some(crate::now_ms());
    flow.core
        .record_conversation(
            &flow.conversation,
            vec![crate::model::DomainEvent::ReviewUpdated { review }],
        )
        .await
        .unwrap();
    flow.manager.settle_requests(&flow.conversation).await;
    flow.until("the request read done", |board| {
        board.requests[&request].state == RequestState::Done
    })
    .await;
    flow.crash().await;
    flow.until("the clean review came after the restart", |_| {
        heard(&inputs, "found nothing") > 0
    })
    .await;
    flow.settled().await;
    flow.crash().await;
    let board = flow.settled().await;
    assert_eq!(heard(&inputs, "found nothing"), 1);
    assert_eq!(board.reviews.len(), 1, "{:#?}", board.reviews);
    flow.stop().await;
}

/// What request_review answered, and what the thread heard besides.
#[derive(Default)]
struct Candidate {
    commits: Vec<String>,
    replies: Vec<String>,
    inputs: Vec<String>,
}

/// request_review on a 10-line change of the thread's own: the other vendor reviews it, and
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
                    return Reply::text("");
                }
                let one = commit(&turn, "a.txt", &lines("a", 10), "Ten lines");
                let first = turn.call("request_review", json!({})).await;
                assert!(!first.is_error, "{}", first.text);
                let again = turn.call("request_review", json!({"commit": one})).await;
                assert!(!again.is_error, "{}", again.text);
                let two = commit(&turn, "a.txt", "fixed\n", "Say it is fixed");
                let fix = turn.call("request_review", json!({})).await;
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

/// With no other-vendor reviewer, and with reviews off, request_review says so as a fact:
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
                    return Reply::text("");
                }
                commit(&turn, "a.txt", &lines(&turn.input, 3), "Three lines");
                let reply = turn.call("request_review", json!({})).await;
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

/// A request_review call cancelled just as its review ends (its CLI's bridge closes, a
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
                    return Reply::text("");
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
                    () = stood_aside => Reply::text(""),
                    reply = turn.call("request_review", json!({})) => {
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
