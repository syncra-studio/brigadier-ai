use std::sync::Arc;

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::workers::test_data_dir;
use crate::work::{CardState, RequestState, TaskState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

#[tokio::test]
async fn a_scout_reports_and_the_answer_ends_the_request() {
    let flow = Flow::start(
        "scout",
        Options::default(),
        script(|turn| async move {
            if turn.is_orchestrator() {
                if turn.input.contains("[report task-1") {
                    return Reply::text("The repository holds a README only.");
                }
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "high", "title": "Look around", "kind": "scout", "spec": "List the files."}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("[quiet]");
            }
            // Its test data folder is there while it works.
            let brief = format!("{}\n{}", turn.prompt, turn.input);
            let (_, rest) = brief
                .split_once("Your test data folder, ")
                .expect("the test data folder is named");
            let (_, rest) = rest.split_once("): ").unwrap();
            let (folder, _) = rest.split_once(". Never").unwrap();
            assert!(std::path::Path::new(folder).is_dir(), "{folder}");
            let reply = turn
                .call(
                    "submit_report",
                    json!({"summary": "Only README.md and .gitignore."}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        }),
    )
    .await;
    flow.say("What is in the repository?").await;
    let board = flow.settled().await;
    let task = Flow::task(&board, 1);
    assert_eq!(task.state, TaskState::Done);
    assert!(
        board
            .requests
            .values()
            .all(|request| request.state == RequestState::Done)
    );
    // The task is over: its test data folder went with it.
    let folder = test_data_dir(&task.id);
    flow.until("the test data folder to go", |_| !folder.exists())
        .await;
    // One a crash left behind goes at the next launch.
    std::fs::create_dir_all(folder.join("data")).unwrap();
    let mut flow = flow;
    flow.restart().await;
    flow.until("the launch to sweep it", |_| !folder.exists())
        .await;
    flow.stop().await;
}

/// A copy of a real store (`BRIGADIER_FLOW_STORE`, made read-only with `sqlite3 "file:…?mode=ro"
/// ".backup …"`): the thread engine's first start deletes every conversation in it, and they
/// all load long enough to be deleted. Run by hand: `BRIGADIER_FLOW_STORE=/tmp/brig-store-copy.db
/// cargo test -p brigadier-core --lib real_store -- --ignored`.
#[tokio::test]
#[ignore = "needs a copy of a real store"]
async fn a_real_store_is_cleared_on_the_first_start() {
    let store = std::env::var_os("BRIGADIER_FLOW_STORE").expect("BRIGADIER_FLOW_STORE");
    let flow = Flow::start(
        "real-store",
        Options {
            store: Some(store.into()),
            ..Options::default()
        },
        script(|_| async { Reply::text("[quiet]") }),
    )
    .await;
    let old = flow.core.catalog().conversations.len() - 1;
    let visible = flow.core.visible_catalog().conversations;
    assert_eq!(
        visible.len(),
        1,
        "only the session made after the first start"
    );
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
    while flow.core.catalog().conversations.len() > 1 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{} conversations are still being deleted",
            flow.core.catalog().conversations.len() - 1
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    eprintln!("deleted {old} conversations");
    flow.stop().await;
}

/// What every scripted CLI was asked, in order.
type Heard = Arc<std::sync::Mutex<Vec<String>>>;

/// Asks for a review of the worker's own work and waits for the findings, which steer into
/// its running turn. What the review found, or why there is none.
async fn own_review(turn: &Turn) -> String {
    let started = turn.call("review_code", json!({})).await;
    assert!(!started.is_error, "{}", started.text);
    if !started.text.starts_with("Started a review") {
        return started.text;
    }
    turn.steered().await.expect("the review's findings")
}

/// Waits until the session has at least `count` one-shot reviews and all have ended.
async fn reviews_ended(flow: &Flow, count: usize) -> crate::board::Board {
    flow.until("the reviews to end", |board| {
        board.reviews.len() >= count
            && board
                .reviews
                .values()
                .all(|review| review.state != crate::work::ReviewState::Running)
    })
    .await
}

/// A lead outlines big work and waits; the orchestrator gets the outline at once and sends
/// the go-ahead with its corrections, and the lead builds. A plan review by the other vendor
/// runs in the background meanwhile; its findings reach the orchestrator when they come.
#[tokio::test]
async fn an_outline_gets_its_go_ahead_at_once_and_a_plan_review_in_the_background() {
    let heard: Heard = Arc::default();
    let log = heard.clone();
    let flow = Flow::start(
        "outline",
        Options {
            reviews: Some(script(|turn| async move {
                assert!(
                    turn.input.contains("1. Read a.rs"),
                    "the reviewer reads the outline"
                );
                assert!(turn.input.contains("Rework the parser."), "and the brief");
                Reply::text("- [P1] Step 2 misses the caller in b.rs — b.rs:1\n  Update it too.")
            })),
            ..Options::default()
        },
        script(move |turn| {
            log.lock().unwrap().push(turn.input.clone());
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("[outline task-1") {
                        let reply = turn
                            .call(
                                "approve_outline",
                                json!({"task": "task-1", "corrections": "Also update b.rs."}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("[quiet]");
                    }
                    if turn.input.contains("[plan review task-1") {
                        assert!(turn.input.contains("Step 2 misses the caller in b.rs"));
                        return Reply::text("The plan review agrees with the change.");
                    }
                    if turn.input.contains("[report task-1") {
                        return Reply::text("Done: nothing needed changing.");
                    }
                    if turn.input.contains("[report") {
                        return Reply::text("[quiet]");
                    }
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Rework the parser", "kind": "implement",
                                   "spec": "Rework the parser.", "provider": "claude"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                if turn.earlier == 0 {
                    let reply = turn
                        .call(
                            "submit_outline",
                            json!({"outline": "1. Read a.rs\n2. Change parse()"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Waiting for the go-ahead.");
                }
                assert!(turn.input.contains("Go ahead"), "{}", turn.input);
                assert!(turn.input.contains("Also update b.rs."));
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Nothing needed changing."}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Rework the parser.").await;
    reviews_ended(&flow, 1).await;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    while !heard
        .lock()
        .unwrap()
        .iter()
        .any(|input| input.contains("[plan review task-1"))
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the orchestrator hears the plan review"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    flow.settled().await;
    // The lead, which changed nothing, ends once its turn is over: that may come after the
    // orchestrator's final answer.
    let board = flow
        .until("every task to end", |board| {
            board.tasks.values().all(|task| task.state.is_final())
        })
        .await;
    let lead = Flow::task(&board, 1);
    assert_eq!(board.tasks.len(), 1, "one lead and no reviewer task");
    assert_eq!(lead.role, Some(crate::work::WorkerRole::Lead));
    assert_eq!(lead.state, TaskState::Done);
    // Its one plan review came from the other vendor.
    assert_eq!(board.reviews.len(), 1);
    let review = board.reviews.values().next().unwrap();
    assert_eq!(review.kind, crate::work::ReviewKind::Plan);
    assert_eq!(review.task_id.as_ref(), Some(&lead.id));
    assert_eq!(review.author, lead.route.choice.provider);
    assert_ne!(review.reviewer, lead.route.choice.provider);
    assert_eq!(
        review.state,
        crate::work::ReviewState::Findings { count: 1 }
    );
    // The request has its one phase: the outline and its stage live there.
    assert_eq!(board.plans.len(), 1);
    let plan = board.plans.values().next().unwrap();
    assert_eq!(plan.steps.len(), 1);
    assert_eq!(plan.steps[0].task_id.as_ref(), Some(&lead.id));
    assert!(
        plan.steps[0]
            .outline
            .as_deref()
            .unwrap()
            .contains("Change parse()")
    );
    assert!(board.approvals.is_empty(), "no cards under Full access");
    let outlines = heard
        .lock()
        .unwrap()
        .iter()
        .filter(|input| input.contains("[outline task-1"))
        .count();
    assert_eq!(outlines, 1, "the orchestrator gets the outline once");
    let events = flow.events().await;
    assert!(events.iter().any(|event| matches!(
        event,
        crate::model::DomainEvent::OrchestratorStepped { step }
            if matches!(&step.kind, crate::work::OrchestratorStepKind::Created { task_id } if task_id == &lead.id)
    )));
    flow.stop().await;
}

/// Two workers on disjoint files commit their own steps (one also commits a stray log). The
/// orchestrator lands each report with land_phase: the first fast-forwards; the second finds
/// the branch moved, is rebased, runs a quick self-check and lands on its own. No cards, no
/// reviews, and the log never lands.
#[tokio::test]
async fn reported_work_lands_with_its_own_commits_and_a_self_check_after_a_rebase() {
    let heard: Heard = Arc::default();
    let log = heard.clone();
    let flow = Flow::start(
        "land",
        Options::default(),
        script(move |turn| {
            log.lock().unwrap().push(turn.input.clone());
            let log = log.clone();
            async move {
                if turn.is_orchestrator() {
                    let mut landed = Vec::new();
                    for n in [1, 2] {
                        if turn.input.contains(&format!("[report task-{n} ")) {
                            let reply = turn
                                .call("land_phase", json!({"task": format!("task-{n}")}))
                                .await;
                            assert!(!reply.is_error, "{}", reply.text);
                            log.lock().unwrap().push(reply.text.clone());
                            landed.push(reply.text);
                        }
                    }
                    if !landed.is_empty() {
                        return Reply::text(format!("[quiet] {}", landed.join(" | ")));
                    }
                    if turn.input.contains("[landed task-") {
                        return Reply::text("Both files are in.");
                    }
                    for (title, file) in [("Add a", "a.txt"), ("Add b", "b.txt")] {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "high", "title": title, "kind": "implement",
                                       "spec": format!("Create {file}.")}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                    }
                    return Reply::text("[quiet]");
                }
                let n = turn.task_number().unwrap();
                let file = if n == 1 { "a.txt" } else { "b.txt" };
                if turn.input.contains("Run a quick self-check") {
                    assert!(turn.git(&["status", "--porcelain"]).trim().is_empty());
                    assert!(turn.cwd.join("a.txt").exists() && turn.cwd.join("b.txt").exists());
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "Builds and passes after the rebase.",
                                   "changes": [file]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Checked.");
                }
                turn.write(file, &format!("{file}\n"));
                turn.git(&["add", file]);
                turn.git(&["commit", "-q", "-m", &format!("Add {file}")]);
                if n == 1 {
                    turn.write("debug.log", "scratch\n");
                    turn.git(&["add", "-f", "debug.log"]);
                    turn.git(&["commit", "-q", "-m", "Keep a log"]);
                }
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": format!("Added {file}."), "changes": [file]}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Add a.txt and b.txt.").await;
    let board = flow
        .until("both to land", |board| {
            board.tasks.len() == 2
                && board
                    .tasks
                    .values()
                    .all(|task| task.state == TaskState::Landed)
        })
        .await;
    let first = Flow::task(&board, 1);
    let target = first
        .workspace
        .as_ref()
        .and_then(|w| w.target.clone())
        .unwrap();
    let files = super::git(&flow.repo, &["ls-tree", "-r", "--name-only", &target]);
    assert!(
        files.contains("a.txt") && files.contains("b.txt"),
        "{files}"
    );
    assert!(!files.contains("debug.log"), "the log stays out: {files}");
    let subjects = super::git(&flow.repo, &["log", "--format=%s", &target]);
    assert!(subjects.contains("Add a.txt") && subjects.contains("Add b.txt"));
    assert!(board.approvals.is_empty(), "no cards");
    assert!(
        board
            .tasks
            .values()
            .all(|task| task.kind == crate::work::TaskKind::Implement),
        "no reviewers or verifiers"
    );
    let said = heard.lock().unwrap().join("\n");
    assert!(said.contains("Run a quick self-check"), "one was rebased");
    assert!(
        said.contains("Left out as litter") && said.contains("debug.log"),
        "the orchestrator hears what stayed out"
    );
    let landings = flow
        .events()
        .await
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                crate::model::DomainEvent::OrchestratorStepped { step }
                    if matches!(step.kind, crate::work::OrchestratorStepKind::Landed { .. })
            )
        })
        .count();
    assert_eq!(landings, 2);
    flow.settled().await;
    flow.stop().await;
}

/// The task numbers of the reports in an orchestrator's input, in order.
fn reports_in(input: &str) -> Vec<u32> {
    input
        .split("[report task-")
        .skip(1)
        .filter_map(|rest| {
            rest.split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse()
                .ok()
        })
        .collect()
}

/// Done when (1): a small request goes straight to a lead, with no outline. The lead (Codex,
/// which can't commit from its sandbox, so Brigadier commits for it) asks for its own review
/// (review_code), which returns at once; the review comes from the other vendor, reads its
/// work in a checkout of its own, and its findings reach the lead as a message. Then the
/// orchestrator lands it, and the landing gets its own background review. No verifier, no
/// reviewer task, no cards.
#[tokio::test]
async fn a_small_request_is_reviewed_by_its_lead_and_lands_without_a_verifier() {
    let flow = Flow::start(
        "small",
        Options {
            reviews: Some(script(|turn| async move {
                assert!(turn.input.contains("git diff"), "{}", turn.input);
                let greeting = std::fs::read_to_string(turn.cwd.join("hello.txt"))
                    .expect("the reviewer's checkout holds the work");
                if greeting.ends_with('\n') {
                    Reply::text("No findings.")
                } else {
                    Reply::text(
                        "- [P2] hello.txt lacks a trailing newline — hello.txt:1\n  Add one.",
                    )
                }
            })),
            ..Options::default()
        },
        script(|turn| async move {
            if turn.is_orchestrator() {
                if let Some(n) = reports_in(&turn.input).first() {
                    let reply = turn
                        .call("land_phase", json!({"task": format!("task-{n}")}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    assert!(reply.text.contains("Landed"), "{}", reply.text);
                    return Reply::text("Added the greeting.");
                }
                if turn.input.contains("[review ") {
                    return Reply::text("The review of the landing found nothing to fix.");
                }
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "high", "title": "Add a greeting", "kind": "implement",
                               "spec": "Create hello.txt.", "provider": "codex"}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("[quiet]");
            }
            // The lead never commits: its sandbox can't.
            turn.write("hello.txt", "hello");
            let review = own_review(&turn).await;
            assert!(review.contains("trailing newline"), "{review}");
            turn.write("hello.txt", "hello\n");
            let reply = turn
                .call(
                    "submit_report",
                    json!({"summary": "Added hello.txt; fixed the review's finding.",
                           "changes": ["hello.txt"]}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        }),
    )
    .await;
    flow.say("Add a greeting file and merge it.").await;
    flow.settled().await;
    // The lead's own review and the landing's.
    let board = reviews_ended(&flow, 2).await;
    let lead = Flow::task(&board, 1);
    assert_eq!(
        board.tasks.len(),
        1,
        "a lead only: no verifier, no reviewer task"
    );
    assert_eq!(lead.state, TaskState::Landed);
    let mut reviews: Vec<_> = board.reviews.values().collect();
    reviews.sort_by_key(|review| review.started_at_ms);
    assert_eq!(reviews.len(), 2, "{reviews:#?}");
    for review in &reviews {
        assert_eq!(review.kind, crate::work::ReviewKind::Code);
        assert_eq!(review.author, lead.route.choice.provider);
        assert_ne!(review.reviewer, lead.route.choice.provider);
        assert_eq!(review.task_id.as_ref(), Some(&lead.id));
    }
    assert_eq!(
        reviews[0].state,
        crate::work::ReviewState::Findings { count: 1 }
    );
    assert_eq!(reviews[1].state, crate::work::ReviewState::Clean);
    assert_eq!(
        lead.landed.as_deref(),
        Some(reviews[1].tip.as_str()),
        "the landing's review reads what landed"
    );
    assert!(board.plans.is_empty(), "a small request has no phases");
    assert!(board.approvals.is_empty(), "no cards");
    let target = lead.workspace.as_ref().unwrap().target.clone().unwrap();
    assert_eq!(
        super::git(
            &flow.repo,
            &["cat-file", "-s", &format!("{target}:hello.txt")]
        )
        .trim(),
        "6",
        "the lead's fix after its review landed (\"hello\\n\")"
    );
    flow.stop().await;
}

/// The landing's review is still running when the user merges the session: the merge doesn't
/// wait for it, and its findings still reach the orchestrator after the merge. The review read
/// a checkout of its own, which the merge left alone and the review's end removed. One review
/// for the one landing.
#[tokio::test]
async fn a_review_still_running_at_the_merge_reports_its_findings_after_it() {
    let heard: Heard = Arc::default();
    let log = heard.clone();
    let release = Arc::new(tokio::sync::Notify::new());
    let held = release.clone();
    let checkout: Arc<std::sync::Mutex<Option<std::path::PathBuf>>> = Arc::default();
    let seen = checkout.clone();
    let flow = Flow::start(
        "merged-before-review",
        Options {
            reviews: Some(script(move |turn| {
                let (held, seen) = (held.clone(), seen.clone());
                async move {
                    *seen.lock().unwrap() = Some(turn.cwd.clone());
                    held.notified().await;
                    std::fs::read_to_string(turn.cwd.join("hello.txt"))
                        .expect("the review's checkout outlives the merge");
                    Reply::text(
                        "- [P2] hello.txt lacks a trailing newline — hello.txt:1\n  Add one.",
                    )
                }
            })),
            ..Options::default()
        },
        script(move |turn| {
            log.lock().unwrap().push(turn.input.clone());
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("[review task-1") {
                        return Reply::text(
                            "The review found a missing newline; tell me if you want it fixed.",
                        );
                    }
                    if let Some(n) = reports_in(&turn.input).first() {
                        let reply = turn
                            .call("land_phase", json!({"task": format!("task-{n}")}))
                            .await;
                        assert!(reply.text.contains("Landed"), "{}", reply.text);
                        // The user asked for the merge with the work: no card.
                        let reply = turn
                            .call("finish_session", json!({"user_words": "merge it"}))
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        assert!(reply.text.contains("[finished]"), "{}", reply.text);
                        assert!(reply.text.contains("review still runs"), "{}", reply.text);
                        return Reply::text("Merged the greeting into main.");
                    }
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Add a greeting", "kind": "implement",
                                   "spec": "Create hello.txt.", "provider": "codex"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                turn.write("hello.txt", "hello");
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Added hello.txt.", "changes": ["hello.txt"]}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Add a greeting file and merge it.").await;
    let repo = flow.repo.clone();
    let board = flow
        .until("the merge", |_| {
            std::process::Command::new("git")
                .args(["cat-file", "-e", "main:hello.txt"])
                .current_dir(&repo)
                .status()
                .is_ok_and(|status| status.success())
        })
        .await;
    assert_eq!(board.reviews.len(), 1);
    assert_eq!(
        board.reviews.values().next().unwrap().state,
        crate::work::ReviewState::Running,
        "the merge didn't wait for the review"
    );
    assert!(board.approvals.is_empty(), "no merge card");
    release.notify_one();
    let board = reviews_ended(&flow, 1).await;
    flow.until("the orchestrator to hear the review", |_| {
        heard
            .lock()
            .unwrap()
            .iter()
            .any(|input| input.contains("[review task-1") && input.contains("trailing newline"))
    })
    .await;
    let lead = Flow::task(&board, 1);
    assert_eq!(
        board.tasks.len(),
        1,
        "a lead only: no verifier, no reviewer task"
    );
    assert_eq!(board.reviews.len(), 1, "one review for the one landing");
    let review = board.reviews.values().next().unwrap();
    assert_eq!(review.kind, crate::work::ReviewKind::Code);
    assert_eq!(review.task_id.as_ref(), Some(&lead.id));
    assert_eq!(lead.landed.as_deref(), Some(review.tip.as_str()));
    assert_eq!(
        review.state,
        crate::work::ReviewState::Findings { count: 1 }
    );
    // The thread shows it as a "Reviewed" row.
    let reviewed: Vec<_> = flow
        .events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            crate::model::DomainEvent::OrchestratorStepped { step } => matches!(
                step.kind,
                crate::work::OrchestratorStepKind::Reviewed { .. }
            )
            .then_some(step.kind),
            _ => None,
        })
        .collect();
    assert_eq!(
        reviewed,
        vec![crate::work::OrchestratorStepKind::Reviewed {
            task_ids: vec![lead.id.clone()],
            findings: 1,
        }]
    );
    let checkout = checkout.lock().unwrap().clone().expect("the review ran");
    assert!(!checkout.exists(), "the review's checkout is removed");
    assert!(
        !super::git(&flow.repo, &["worktree", "list"]).contains(&checkout.display().to_string()),
        "git forgot the review's checkout"
    );
    flow.stop().await;
}

/// A verifier only when the orchestrator wants one: it starts it on the lead's report; the
/// verifier asks for the review (review_code), which returns at once, gets the findings as a
/// message, fixes the one it agrees with and reports; the orchestrator lands the verifier.
#[tokio::test]
async fn a_verifier_the_orchestrator_started_triages_its_review_and_reports() {
    let flow = Flow::start(
        "verifier-review",
        Options {
            reviews: Some(script(|turn| async move {
                let greeting = std::fs::read_to_string(turn.cwd.join("hello.txt"))
                    .expect("the reviewer's checkout holds the work");
                if greeting.ends_with('\n') {
                    Reply::text("No findings.")
                } else {
                    Reply::text(
                        "- [P2] hello.txt lacks a trailing newline — hello.txt:1\n  Add one.",
                    )
                }
            })),
            ..Options::default()
        },
        script(|turn| async move {
            if turn.is_orchestrator() {
                match reports_in(&turn.input).last().copied() {
                    Some(1) => {
                        let reply = turn.call("start_verifier", json!({"task": "task-1"})).await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("[quiet]");
                    }
                    Some(n) => {
                        let reply = turn
                            .call("land_phase", json!({"task": format!("task-{n}")}))
                            .await;
                        assert!(reply.text.contains("Landed"), "{}", reply.text);
                        return Reply::text("Added the greeting, verified.");
                    }
                    None => {}
                }
                if turn.input.contains("[review ") {
                    return Reply::text("[quiet]");
                }
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "high", "title": "Add a greeting", "kind": "implement",
                               "spec": "Create hello.txt.", "provider": "claude"}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("[quiet]");
            }
            if turn.prompt.contains("You verify this phase") {
                let review = own_review(&turn).await;
                assert!(review.contains("found 1"), "{review}");
                assert!(review.contains("trailing newline"), "{review}");
                turn.write("hello.txt", "hello\n");
                turn.git(&["commit", "-qam", "End hello.txt with a newline"]);
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Checks pass. The review found a missing trailing newline in hello.txt; fixed.",
                               "changes": ["hello.txt"],
                               "done_when": "[met] hello.txt exists: cat shows it"}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Verified.");
            }
            turn.write("hello.txt", "hello");
            turn.git(&["add", "hello.txt"]);
            turn.git(&["commit", "-q", "-m", "Add hello.txt"]);
            let reply = turn
                .call(
                    "submit_report",
                    json!({"summary": "Added hello.txt.", "changes": ["hello.txt"]}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        }),
    )
    .await;
    flow.say("Add a greeting file and merge it.").await;
    flow.settled().await;
    let board = reviews_ended(&flow, 2).await;
    let lead = Flow::task(&board, 1);
    let verifier = Flow::task(&board, 2);
    assert_eq!(board.tasks.len(), 2, "a lead and the verifier it was given");
    assert_eq!(verifier.role, Some(crate::work::WorkerRole::Verifier));
    assert_eq!(verifier.subject.as_ref(), Some(&lead.id));
    assert_eq!(verifier.state, TaskState::Landed);
    let asked = board
        .reviews
        .values()
        .find(|review| {
            matches!(&review.notify, crate::work::ReviewFor::Worker { task_id } if *task_id == verifier.id)
        })
        .expect("the verifier's own review");
    assert_eq!(asked.state, crate::work::ReviewState::Findings { count: 1 });
    assert!(
        verifier
            .report
            .as_ref()
            .is_some_and(|report| report.summary.contains("trailing newline")),
        "the verifier reports what the review found"
    );
    let target = verifier.workspace.as_ref().unwrap().target.clone().unwrap();
    assert_eq!(
        super::git(
            &flow.repo,
            &["cat-file", "-s", &format!("{target}:hello.txt")]
        )
        .trim(),
        "6",
        "the verifier's fix landed"
    );
    assert!(board.approvals.is_empty(), "no cards");
    flow.stop().await;
}

/// A lead asks for its review and reports before it ends; the orchestrator starts a verifier on
/// the lead's last commit, the same range. The verifier's review_code finds that review still
/// running and takes it over: the findings reach the verifier, which fixes and reports.
#[tokio::test]
async fn a_verifier_takes_over_its_leads_review_still_running() {
    let release = Arc::new(tokio::sync::Notify::new());
    let held = release.clone();
    let first = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let flow = Flow::start(
        "verifier-takes-review",
        Options {
            reviews: Some(script(move |turn| {
                let held = held.clone();
                // Only the lead's review waits for the verifier: the landing's runs at once.
                let waits = first.swap(false, std::sync::atomic::Ordering::SeqCst);
                async move {
                    if waits {
                        held.notified().await;
                    }
                    let greeting = std::fs::read_to_string(turn.cwd.join("hello.txt")).unwrap();
                    if greeting.ends_with('\n') {
                        Reply::text("No findings.")
                    } else {
                        Reply::text(
                            "- [P2] hello.txt lacks a trailing newline — hello.txt:1\n  Add one.",
                        )
                    }
                }
            })),
            ..Options::default()
        },
        script(move |turn| {
            let release = release.clone();
            async move {
                if turn.is_orchestrator() {
                    match reports_in(&turn.input).last().copied() {
                        Some(1) => {
                            let reply =
                                turn.call("start_verifier", json!({"task": "task-1"})).await;
                            assert!(!reply.is_error, "{}", reply.text);
                            return Reply::text("[quiet]");
                        }
                        Some(n) => {
                            let reply = turn
                                .call("land_phase", json!({"task": format!("task-{n}")}))
                                .await;
                            assert!(reply.text.contains("Landed"), "{}", reply.text);
                            return Reply::text("Added the greeting, verified.");
                        }
                        None => {}
                    }
                    if turn.input.contains("[review ") {
                        return Reply::text("[quiet]");
                    }
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Add a greeting", "kind": "implement",
                                   "spec": "Create hello.txt.", "provider": "claude"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                if turn.prompt.contains("You verify this phase") {
                    let asked = turn.call("review_code", json!({})).await;
                    assert!(
                        asked.text.contains("being reviewed already; the findings arrive"),
                        "{}",
                        asked.text
                    );
                    release.notify_one();
                    let review = turn.steered().await.expect("the review's findings");
                    assert!(review.contains("trailing newline"), "{review}");
                    turn.write("hello.txt", "hello\n");
                    turn.git(&["commit", "-qam", "End hello.txt with a newline"]);
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "The lead's review found a missing trailing newline; fixed.",
                                   "changes": ["hello.txt"]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Verified.");
                }
                if turn.input.contains("[review of your work") {
                    return Reply::text("Reported already.");
                }
                turn.write("hello.txt", "hello");
                turn.git(&["add", "hello.txt"]);
                turn.git(&["commit", "-q", "-m", "Add hello.txt"]);
                let asked = turn.call("review_code", json!({})).await;
                assert!(asked.text.starts_with("Started a review"), "{}", asked.text);
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Added hello.txt; its review still runs.",
                               "changes": ["hello.txt"]}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Add a greeting file and merge it.").await;
    flow.settled().await;
    let board = flow
        .until("the verifier to land", |board| {
            board
                .tasks
                .values()
                .any(|task| task.number == 2 && task.state == TaskState::Landed)
        })
        .await;
    let board = reviews_ended(&flow, board.reviews.len()).await;
    let lead = Flow::task(&board, 1);
    let verifier = Flow::task(&board, 2);
    let taken = board
        .reviews
        .values()
        .find(|review| {
            review.kind == crate::work::ReviewKind::Code
                && review.task_id.as_ref() == Some(&lead.id)
        })
        .expect("the lead's review");
    assert_eq!(
        taken.notify,
        crate::work::ReviewFor::Worker {
            task_id: verifier.id.clone()
        },
        "the verifier took the lead's review over"
    );
    assert_eq!(taken.state, crate::work::ReviewState::Findings { count: 1 });
    assert!(
        verifier
            .report
            .as_ref()
            .is_some_and(|report| report.summary.contains("trailing newline")),
        "the verifier reports what the review found"
    );
    flow.stop().await;
}

/// Done when (2): a request of two phases. Phase 1's lead outlines and gets the go-ahead at
/// once (its plan review runs in the background), builds, asks for its own review and reports;
/// the orchestrator judges the phase big enough for a verifier and starts one, which asks for
/// the review of the phase (already done: the same range is reviewed once), fixes, commits and
/// reports; the orchestrator lands the verifier and starts phase 2, whose lead reviews its own
/// small change; then the final answer. Every landing has its review; no reviewer tasks, no
/// plan rounds, no cards.
#[tokio::test]
async fn an_outlined_phase_is_verified_and_landed_before_the_next_phase() {
    let heard: Heard = Arc::default();
    let log = heard.clone();
    let flow = Flow::start(
        "phases",
        Options::default(),
        script(move |turn| {
            log.lock().unwrap().push(turn.input.clone());
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("[outline task-1") {
                        let reply = turn
                            .call(
                                "approve_outline",
                                json!({"task": "task-1", "corrections": "Name the file p1.txt."}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("[quiet]");
                    }
                    match reports_in(&turn.input).last().copied() {
                        // Phase one's lead: big work, so the orchestrator starts a verifier.
                        Some(1) => {
                            let reply =
                                turn.call("start_verifier", json!({"task": "task-1"})).await;
                            assert!(!reply.is_error, "{}", reply.text);
                            let refused = turn.call("land_phase", json!({"task": "task-1"})).await;
                            assert!(refused.is_error, "the lead alone doesn't land");
                            return Reply::text("[quiet]");
                        }
                        Some(n) => {
                            let reply = turn
                                .call("land_phase", json!({"task": format!("task-{n}")}))
                                .await;
                            assert!(!reply.is_error, "{}", reply.text);
                            assert!(reply.text.contains("Landed"), "{}", reply.text);
                            if n == 2 {
                                let reply = turn
                                    .call(
                                        "delegate_task",
                                        json!({"effort": "high", "title": "Phase two", "kind": "implement",
                                               "spec": "Build phase two: create p2.txt.",
                                               "phase": 2, "provider": "codex"}),
                                    )
                                    .await;
                                assert!(!reply.is_error, "{}", reply.text);
                                return Reply::text("[quiet]");
                            }
                            return Reply::text(
                                "Both phases are done.\n\nWaiting on you: nothing.",
                            );
                        }
                        None => {}
                    }
                    let reply = turn
                        .call(
                            "plan_phases",
                            json!({"title": "Two files", "phases": [
                                {"title": "Phase one"}, {"title": "Phase two"}]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Phase one", "kind": "implement",
                                   "spec": "Build phase one.", "phase": 1,
                                   "provider": "claude"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                if turn.prompt.contains("You verify this phase") {
                    assert!(
                        turn.cwd.join("p1.txt").exists(),
                        "it starts from the lead's work"
                    );
                    assert!(
                        turn.prompt.contains("Name the file p1.txt."),
                        "it checks against the outline's corrections"
                    );
                    assert!(turn.prompt.contains("review_code"));
                    let review = own_review(&turn).await;
                    assert!(
                        review.contains("reviewed already"),
                        "the lead's review covered the same range: {review}"
                    );
                    turn.write("p1-fix.txt", "fixed\n");
                    turn.git(&["add", "p1-fix.txt"]);
                    turn.git(&["commit", "-q", "-m", "Fix what the checks found"]);
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "Phase one passes; fixed one thing.",
                                   "changes": ["p1-fix.txt"],
                                   "done_when": "[met] p1.txt exists: ls shows it"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Verified.");
                }
                if turn.prompt.contains("Build phase two") {
                    turn.write("p2.txt", "two\n");
                    turn.git(&["add", "p2.txt"]);
                    turn.git(&["commit", "-q", "-m", "Add p2.txt"]);
                    let review = own_review(&turn).await;
                    assert!(review.contains("found nothing"), "{review}");
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "Added p2.txt.", "changes": ["p2.txt"]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                // Phase one's lead.
                if turn.earlier == 0 {
                    let reply = turn
                        .call(
                            "submit_outline",
                            json!({"outline": "1. Create the file\n2. Check it"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Waiting for the go-ahead.");
                }
                assert!(turn.input.contains("Go ahead"), "{}", turn.input);
                turn.write("p1.txt", "one\n");
                turn.git(&["add", "p1.txt"]);
                turn.git(&["commit", "-q", "-m", "Add p1.txt"]);
                let review = own_review(&turn).await;
                assert!(review.contains("found nothing"), "{review}");
                // A clean review: report now (THREAD-UX-PLAN.md §4.1 a).
                assert!(
                    review.contains("report at once; don't verify again"),
                    "{review}"
                );
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Added p1.txt.", "changes": ["p1.txt"]}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Make two files, one phase each.").await;
    let board = flow
        .until("the final answer", |board| {
            board
                .tasks
                .values()
                .filter(|task| task.kind.writes())
                .count()
                == 3
                && board.tasks.values().all(|task| task.state.is_final())
                && board
                    .requests
                    .values()
                    .all(|request| request.state == RequestState::Done)
        })
        .await;
    use crate::work::{ReviewKind, ReviewState, WorkerRole as R};
    let roles: Vec<_> = {
        let mut tasks: Vec<_> = board.tasks.values().collect();
        tasks.sort_by_key(|task| task.number);
        tasks.iter().map(|task| (task.number, task.role)).collect()
    };
    let lead = Flow::task(&board, 1);
    let verifier = Flow::task(&board, 2);
    assert_eq!(verifier.role, Some(R::Verifier), "{roles:?}");
    assert_eq!(verifier.subject.as_ref(), Some(&lead.id));
    assert_eq!(lead.state, TaskState::Landed, "{roles:?}");
    assert_eq!(verifier.state, TaskState::Landed);
    assert!(
        board
            .tasks
            .values()
            .all(|task| task.role != Some(R::Reviewer)),
        "no reviewer tasks: {roles:?}"
    );
    // Every landing has its review, and no range is reviewed twice.
    let board = reviews_ended(&flow, 4).await;
    let code: Vec<_> = board
        .reviews
        .values()
        .filter(|review| review.kind == ReviewKind::Code)
        .collect();
    for task in board.tasks.values() {
        let Some(landed) = &task.landed else { continue };
        assert!(
            code.iter().any(|review| &review.tip == landed),
            "task-{} landed {landed} unreviewed",
            task.number
        );
    }
    for (i, review) in code.iter().enumerate() {
        assert!(
            code[i + 1..]
                .iter()
                .all(|other| (&other.base, &other.tip) != (&review.base, &review.tip)),
            "a range is reviewed once"
        );
        assert_ne!(review.reviewer, review.author, "the other vendor reviews");
    }
    let plan_reviews: Vec<_> = board
        .reviews
        .values()
        .filter(|review| review.kind == ReviewKind::Plan)
        .collect();
    assert_eq!(plan_reviews.len(), 1, "phase one's outline");
    assert_eq!(plan_reviews[0].task_id.as_ref(), Some(&lead.id));
    assert!(
        board
            .reviews
            .values()
            .all(|review| review.state == ReviewState::Clean)
    );
    // A Claude lead and a Codex verifier: the phase's reviews come from the vendor other than
    // the lead's (the phase's author).
    use brigadier_providers::ProviderKind;
    assert_eq!(lead.route.choice.provider, ProviderKind::Claude);
    assert!(
        board
            .reviews
            .values()
            .filter(|review| review.task_id.as_ref() == Some(&verifier.id))
            .all(|review| review.reviewer == ProviderKind::Codex)
    );
    assert!(board.tasks.values().all(|task| task.gate_link.is_none()));
    assert!(board.approvals.is_empty(), "no cards");
    assert_eq!(board.plans.len(), 1, "one plan, no rounds");
    let plan = board.plans.values().next().unwrap();
    assert!(
        plan.steps
            .iter()
            .all(|step| step.stage == crate::work::PhaseStage::Done),
        "{:?}",
        plan.steps.iter().map(|s| s.stage).collect::<Vec<_>>()
    );
    let target = lead.workspace.as_ref().unwrap().target.clone().unwrap();
    let files = super::git(&flow.repo, &["ls-tree", "-r", "--name-only", &target]);
    for file in ["p1.txt", "p1-fix.txt", "p2.txt"] {
        assert!(files.contains(file), "{file}: {files}");
    }
    let said = heard.lock().unwrap().join("\n");
    assert_eq!(said.matches("[outline task-1").count(), 1);
    assert!(!said.contains("[review task-"), "clean reviews wake nobody");
    flow.stop().await;
}

/// A worker's questions are answered by the orchestrator, never left for the user: one with
/// answer_worker and a reason, one by a steering message_worker. Each shows as an "Answered"
/// row and enters the ledger. Workers may ask the Project Brain too (read-only).
#[tokio::test]
async fn the_orchestrator_answers_a_workers_questions_and_logs_each_answer() {
    let flow = Flow::start(
        "answers",
        Options::default(),
        script(|turn| async move {
            if turn.is_orchestrator() {
                if turn.input.contains("Which greeting?") {
                    let reply = turn
                        .call(
                            "answer_worker",
                            json!({"task": "task-1", "answer": "Hello.",
                                   "why": "your recommendation fits the brief"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("Which language?") {
                    let reply = turn
                        .call(
                            "message_worker",
                            json!({"task": "task-1", "text": "English."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                if !reports_in(&turn.input).is_empty() {
                    // Nothing waits on an answer any more.
                    let reply = turn
                        .call(
                            "answer_worker",
                            json!({"task": "task-1", "answer": "x", "why": "y"}),
                        )
                        .await;
                    assert!(reply.is_error, "{}", reply.text);
                    return Reply::text("It says Hello, in English.");
                }
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "high", "title": "Pick a greeting", "kind": "scout",
                               "spec": "Pick the greeting."}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("[quiet]");
            }
            let brain = turn
                .call("query_brain", json!({"query": "greeting conventions"}))
                .await;
            assert!(!brain.is_error, "{}", brain.text);
            assert!(!brain.text.contains("Delegate a scout"), "{}", brain.text);
            let first = turn
                .call(
                    "ask_orchestrator",
                    json!({"question": "Which greeting? I recommend Hello."}),
                )
                .await;
            assert_eq!(first.text, "Hello.");
            let second = turn
                .call("ask_orchestrator", json!({"question": "Which language?"}))
                .await;
            assert_eq!(second.text, "English.");
            let reply = turn
                .call("submit_report", json!({"summary": "Hello, in English."}))
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        }),
    )
    .await;
    flow.say("Which greeting should we use?").await;
    let board = flow.settled().await;
    assert_eq!(Flow::task(&board, 1).state, TaskState::Done);
    assert!(board.approvals.is_empty() && board.questions.is_empty());
    let answered: Vec<(String, String, String)> = flow
        .events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            crate::model::DomainEvent::OrchestratorStepped { step } => match step.kind {
                crate::work::OrchestratorStepKind::Answered {
                    question,
                    answer,
                    why,
                    ..
                } => Some((question, answer, why)),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        answered,
        vec![
            (
                "Which greeting? I recommend Hello.".to_owned(),
                "Hello.".to_owned(),
                "your recommendation fits the brief".to_owned()
            ),
            (
                "Which language?".to_owned(),
                "English.".to_owned(),
                "steer".to_owned()
            ),
        ]
    );
    let logged = board
        .decisions
        .iter()
        .filter(|decision| decision.kind == crate::work::DecisionKind::Answer)
        .count();
    assert_eq!(logged, 2, "{:?}", board.decisions);
    flow.stop().await;
}

/// Archiving a session ends a review still running: its checkout goes with the rest, and
/// nothing more of it reaches anyone.
#[tokio::test]
async fn archiving_a_session_ends_its_running_review() {
    let checkout: Arc<std::sync::Mutex<Option<std::path::PathBuf>>> = Arc::default();
    let seen = checkout.clone();
    let flow = Flow::start(
        "archived-mid-review",
        Options {
            reviews: Some(script(move |turn| {
                let seen = seen.clone();
                async move {
                    *seen.lock().unwrap() = Some(turn.cwd.clone());
                    // Never ends on its own.
                    std::future::pending::<()>().await;
                    Reply::text("No findings.")
                }
            })),
            ..Options::default()
        },
        script(|turn| async move {
            if turn.is_orchestrator() {
                if let Some(n) = reports_in(&turn.input).first() {
                    let reply = turn
                        .call("land_phase", json!({"task": format!("task-{n}")}))
                        .await;
                    assert!(reply.text.contains("Landed"), "{}", reply.text);
                    return Reply::text("Landed the greeting.");
                }
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "high", "title": "Add a greeting", "kind": "implement",
                               "spec": "Create hello.txt.", "provider": "codex"}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("[quiet]");
            }
            turn.write("hello.txt", "hello");
            let reply = turn
                .call(
                    "submit_report",
                    json!({"summary": "Added hello.txt.", "changes": ["hello.txt"]}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        }),
    )
    .await;
    flow.say("Add a greeting file and merge it.").await;
    flow.until("the landing's review to run", |board| {
        board
            .reviews
            .values()
            .any(|review| review.state == crate::work::ReviewState::Running)
    })
    .await;
    flow.until("its checkout", |_| checkout.lock().unwrap().is_some())
        .await;
    let dir = checkout.lock().unwrap().clone().unwrap();
    assert!(dir.exists());
    flow.manager
        .archive(flow.conversation.clone())
        .await
        .unwrap();
    let board = reviews_ended(&flow, 1).await;
    let review = board.reviews.values().next().unwrap();
    assert!(
        matches!(review.state, crate::work::ReviewState::Failed { .. }),
        "{:?}",
        review.state
    );
    flow.until("the review's checkout to go", |_| !dir.exists())
        .await;
    flow.stop().await;
}

/// A review's outcome reaching a worker that waits on a question answers nothing: the
/// question stays open until the orchestrator answers it.
#[tokio::test]
async fn a_reviews_news_leaves_a_workers_question_open() {
    let answered: Heard = Arc::default();
    let got = answered.clone();
    let flow = Flow::start(
        "review-news-question",
        Options::default(),
        script(move |turn| {
            let got = got.clone();
            async move {
                if turn.is_orchestrator() {
                    if !reports_in(&turn.input).is_empty() {
                        return Reply::text("It says Hello.");
                    }
                    if turn.input.contains("Which greeting?") {
                        return Reply::text("[quiet]");
                    }
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Pick a greeting", "kind": "scout",
                                   "spec": "Pick the greeting."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                let answer = turn
                    .call("ask_orchestrator", json!({"question": "Which greeting?"}))
                    .await;
                got.lock().unwrap().push(answer.text);
                let reply = turn
                    .call("submit_report", json!({"summary": "Hello."}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Which greeting should we use?").await;
    let board = flow
        .until("the worker's question", |board| {
            board.tasks.values().any(|task| {
                task.blocked_reason
                    .as_deref()
                    .is_some_and(|why| why.starts_with("Asked the orchestrator"))
            })
        })
        .await;
    let task = Flow::task(&board, 1);
    let (_, answered_it) = flow
        .manager
        .tell_worker(
            &flow.conversation,
            task,
            "[review of your work] Codex found nothing.".into(),
            "Brigadier",
        )
        .await
        .unwrap();
    assert!(!answered_it, "a review's news is no answer");
    let (_, answered_it) = flow
        .manager
        .message_worker(&flow.conversation, task, "Hello.".into(), "orchestrator")
        .await
        .unwrap();
    assert!(answered_it, "the question was still open");
    let board = flow.settled().await;
    assert_eq!(Flow::task(&board, 1).state, TaskState::Done);
    assert_eq!(*answered.lock().unwrap(), ["Hello."]);
    flow.stop().await;
}

/// Done when (3): a worker whose context passes the hand-off size ends its turn with a handoff
/// note, and a fresh session of the same model carries on from it: with the note, the
/// orchestrator's earlier answer and the user's "Allow similar commands" grant, so nothing is
/// asked again.
#[tokio::test]
async fn a_worker_past_the_handoff_size_continues_in_a_fresh_session_that_keeps_its_grants() {
    use brigadier_providers::ApprovalDecision;
    let continued = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let carried_on = continued.clone();
    let flow = Flow::start(
        "handoff",
        Options {
            permission: crate::model::PermissionLevel::AskForApproval,
            ..Options::default()
        },
        script(move |turn| {
            let continued = carried_on.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("[question from task-1") {
                        let reply = turn
                            .call(
                                "answer_worker",
                                json!({"task": "task-1", "answer": "Tabs.",
                                       "why": "your recommendation fits"}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("[quiet]");
                    }
                    if let Some(n) = reports_in(&turn.input).first() {
                        let reply = turn
                            .call("land_phase", json!({"task": format!("task-{n}")}))
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("Both files are in.");
                    }
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Add two files", "kind": "implement",
                                   "spec": "Create a.txt and b.txt.", "provider": "claude"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                const NOTE: &str =
                    "Goal and where it stands: a.txt is committed.\nNext steps: write b.txt.";
                if turn.input.contains("[Brigadier] Your context is now about") {
                    return Reply::text(NOTE);
                }
                if turn
                    .input
                    .contains("You are continuing task-1 in a fresh session")
                {
                    continued.store(true, std::sync::atomic::Ordering::SeqCst);
                    assert!(turn.input.contains("note.md"), "{}", turn.input);
                    assert!(
                        turn.input.contains("Next steps: write b.txt."),
                        "{}",
                        turn.input
                    );
                    let dir = turn
                        .input
                        .split("Its hand-off is in ")
                        .nth(1)
                        .and_then(|rest| rest.split(": ").next())
                        .expect("the hand-off folder");
                    let spec = std::fs::read_to_string(std::path::Path::new(dir).join("spec.md"))
                        .expect("spec.md");
                    assert!(spec.contains("Tabs."), "the answer carries over: {spec}");
                    // The user's grant holds for the successor: no second card.
                    let decision = turn
                        .ask_approval("curl -sI https://example.com", "curl")
                        .await;
                    assert!(
                        matches!(
                            decision,
                            Some(ApprovalDecision::Allow | ApprovalDecision::AllowSimilar)
                        ),
                        "{decision:?}"
                    );
                    turn.write("b.txt", "b\n");
                    turn.git(&["add", "b.txt"]);
                    turn.git(&["commit", "-qm", "Add b.txt"]);
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "Added a.txt and b.txt.",
                                   "changes": ["a.txt", "b.txt"]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                // The first session: it starts small, asks one card (allowed for similar
                // commands) and a question, commits a step and grows past the size.
                turn.report_context(20_000).await;
                let decision = turn
                    .ask_approval("curl -sI https://example.com", "curl")
                    .await;
                assert!(
                    matches!(
                        decision,
                        Some(ApprovalDecision::Allow | ApprovalDecision::AllowSimilar)
                    ),
                    "{decision:?}"
                );
                let answer = turn
                    .call(
                        "ask_orchestrator",
                        json!({"question": "Tabs or spaces? I recommend tabs."}),
                    )
                    .await;
                assert_eq!(answer.text, "Tabs.");
                turn.write("a.txt", "a\n");
                turn.git(&["add", "a.txt"]);
                turn.git(&["commit", "-qm", "Add a.txt"]);
                Reply {
                    text: NOTE.into(),
                    context_tokens: Some(310_000),
                    limit: None,
                }
            }
        }),
    )
    .await;
    flow.say("Add a.txt and b.txt.").await;
    let board = flow
        .until("the approval card", |board| {
            board
                .approvals
                .values()
                .any(|card| card.state == CardState::Pending)
        })
        .await;
    let card = board
        .approvals
        .values()
        .find(|card| card.state == CardState::Pending)
        .unwrap()
        .id
        .clone();
    flow.manager
        .answer_card(
            flow.conversation.clone(),
            card,
            brigadier_providers::ApprovalDecision::AllowSimilar,
        )
        .await
        .unwrap();
    let board = flow.settled().await;
    assert!(
        continued.load(std::sync::atomic::Ordering::SeqCst),
        "a fresh session took over"
    );
    let lead = Flow::task(&board, 1);
    assert_eq!(lead.state, TaskState::Landed);
    assert_eq!(board.approvals.len(), 1, "the grant spared a second card");
    let target = lead.workspace.as_ref().unwrap().target.clone().unwrap();
    for file in ["a.txt", "b.txt"] {
        super::git(&flow.repo, &["cat-file", "-e", &format!("{target}:{file}")]);
    }
    flow.stop().await;
}

/// A request of one phase whose lead outlines it keeps a plan of its own: one phase with its
/// outline and stage (the app's "Phase 1 / 1" pill), landed like any phase. Only a small
/// request without an outline has no plan and no pill.
#[tokio::test]
async fn an_outlined_request_of_one_phase_keeps_its_phase_and_pill() {
    let flow = Flow::start(
        "one-phase",
        Options::default(),
        script(|turn| async move {
            if turn.is_orchestrator() {
                if turn.input.contains("[outline task-1") {
                    let reply = turn
                        .call("approve_outline", json!({"task": "task-1"}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                if let Some(n) = reports_in(&turn.input).last() {
                    let reply = turn
                        .call("land_phase", json!({"task": format!("task-{n}")}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Done.");
                }
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "high", "title": "Add the file", "kind": "implement",
                               "spec": "Create one.txt.", "provider": "claude"}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("[quiet]");
            }
            assert!(
                !turn.prompt.contains("You verify this phase"),
                "no verifier starts on its own"
            );
            if turn.earlier == 0 {
                let reply = turn
                    .call("submit_outline", json!({"outline": "1. Create one.txt"}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Waiting for the go-ahead.");
            }
            turn.write("one.txt", "one\n");
            turn.git(&["add", "one.txt"]);
            turn.git(&["commit", "-q", "-m", "Add one.txt"]);
            let reply = turn
                .call(
                    "submit_report",
                    json!({"summary": "Added one.txt.", "changes": ["one.txt"]}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        }),
    )
    .await;
    flow.say("Add one.txt.").await;
    let board = flow.settled().await;
    assert_eq!(board.tasks.len(), 1, "its lead only");
    assert_eq!(Flow::task(&board, 1).state, TaskState::Landed);
    assert_eq!(board.plans.len(), 1, "the outlined request has its plan");
    let plan = board.plans.values().next().unwrap();
    assert_eq!(plan.steps.len(), 1);
    let phase = &plan.steps[0];
    assert_eq!(phase.outline.as_deref(), Some("1. Create one.txt"));
    assert_eq!(phase.stage, crate::work::PhaseStage::Done);
    assert!(board.approvals.is_empty(), "no cards");
    flow.stop().await;
}

/// In plan mode a lead only outlines, even for small work: it is told so, and a report or a
/// review before its go-ahead is refused.
#[tokio::test]
async fn in_plan_mode_a_lead_outlines_and_builds_nothing() {
    let flow = Flow::start(
        "plan-mode",
        Options {
            plan_mode: true,
            ..Options::default()
        },
        script(|turn| async move {
            if turn.is_orchestrator() {
                if !turn.input.contains("[outline") {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Add a file", "kind": "implement",
                                   "spec": "Create one.txt.", "provider": "claude"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                }
                return Reply::text("[quiet]");
            }
            assert!(turn.prompt.contains("Plan mode is on: change nothing yet"));
            for (tool, args) in [
                ("review_code", json!({})),
                ("submit_report", json!({"summary": "Added one.txt."})),
            ] {
                let refused = turn.call(tool, args).await;
                assert!(refused.is_error, "{tool}: {}", refused.text);
                assert!(refused.text.contains("Plan mode is on"), "{}", refused.text);
            }
            let reply = turn
                .call("submit_outline", json!({"outline": "1. Create one.txt"}))
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Waiting for the go-ahead.")
        }),
    )
    .await;
    flow.say("Add one.txt.").await;
    let board = flow
        .until("the outline waits for the user", |board| {
            board.plans.values().any(|plan| {
                plan.steps
                    .iter()
                    .any(|step| step.stage == crate::work::PhaseStage::AwaitingGoAhead)
            })
        })
        .await;
    let task = Flow::task(&board, 1);
    assert!(task.report.is_none());
    // A task still waiting keeps its test data folder.
    assert!(test_data_dir(&task.id).is_dir());
    flow.stop().await;
}

/// A report whose work can't be committed (here a commit hook refuses it) is refused, so no
/// verifier or landing ever sees part of the work; once committed, it is taken.
#[tokio::test]
async fn a_report_whose_work_cannot_be_committed_is_refused() {
    let flow = Flow::start(
        "hook",
        Options::default(),
        script(|turn| async move {
            if turn.is_orchestrator() {
                if let Some(n) = reports_in(&turn.input).first() {
                    let reply = turn
                        .call("land_phase", json!({"task": format!("task-{n}")}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Added.");
                }
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "high", "title": "Add a file", "kind": "implement",
                               "spec": "Create one.txt.", "provider": "codex"}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("[quiet]");
            }
            let hook = std::path::PathBuf::from(turn.git(&["rev-parse", "--git-common-dir"]))
                .join("hooks/pre-commit");
            let hook = if hook.is_absolute() {
                hook
            } else {
                turn.cwd.join(hook)
            };
            if turn.earlier == 0 {
                std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
                std::fs::write(&hook, "#!/bin/sh\necho 'refused by the hook' >&2\nexit 1\n")
                    .unwrap();
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
                turn.write("one.txt", "one\n");
                let refused = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Added one.txt.", "changes": ["one.txt"]}),
                    )
                    .await;
                assert!(refused.is_error, "{}", refused.text);
                assert!(
                    refused.text.contains("could not be committed"),
                    "{}",
                    refused.text
                );
                return Reply::text("The hook refused my commit.");
            }
            // Asked again to report: it deals with the hook, and the report is taken.
            std::fs::remove_file(&hook).unwrap();
            let reply = turn
                .call(
                    "submit_report",
                    json!({"summary": "Added one.txt.", "changes": ["one.txt"]}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        }),
    )
    .await;
    flow.say("Add one.txt.").await;
    let board = flow.settled().await;
    let lead = Flow::task(&board, 1);
    assert_eq!(lead.state, TaskState::Landed);
    let target = lead.workspace.as_ref().unwrap().target.clone().unwrap();
    super::git(
        &flow.repo,
        &["cat-file", "-e", &format!("{target}:one.txt")],
    );
    flow.stop().await;
}

/// A fix of work that hasn't landed yet starts from that work and lands it with its own: the
/// fix's checkout holds the lead's file, and both files reach the branch.
#[tokio::test]
async fn a_fix_continues_the_unlanded_work_it_fixes_and_lands_both() {
    let flow = Flow::start(
        "fix",
        Options::default(),
        script(|turn| async move {
            if turn.is_orchestrator() {
                let reports = reports_in(&turn.input);
                if reports.contains(&2) {
                    let reply = turn.call("land_phase", json!({"task": "task-2"})).await;
                    assert!(!reply.is_error, "{}", reply.text);
                    assert!(reply.text.contains("Landed"), "{}", reply.text);
                    return Reply::text("Fixed.");
                }
                if reports.contains(&1) {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Fix it", "kind": "implement", "role": "fix",
                                   "subject": "task-1", "spec": "Fix: add b.txt.",
                                   "provider": "claude"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "high", "title": "Add a.txt", "kind": "implement",
                               "spec": "Create a.txt.", "provider": "claude"}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("[quiet]");
            }
            let (file, summary) = if turn.prompt.contains("Fix: add b.txt") {
                assert!(
                    turn.cwd.join("a.txt").exists(),
                    "the fix starts from the work it fixes"
                );
                ("b.txt", "Added b.txt.")
            } else {
                ("a.txt", "Added a.txt.")
            };
            turn.write(file, "x\n");
            turn.git(&["add", file]);
            turn.git(&["commit", "-q", "-m", summary]);
            let reply = turn
                .call(
                    "submit_report",
                    json!({"summary": summary, "changes": [file]}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        }),
    )
    .await;
    flow.say("Add a.txt.").await;
    let board = flow
        .until("both land", |board| {
            board.tasks.len() == 2 && board.tasks.values().all(|task| task.state.is_final())
        })
        .await;
    let lead = Flow::task(&board, 1);
    assert_eq!(lead.state, TaskState::Landed);
    assert_eq!(Flow::task(&board, 2).state, TaskState::Landed);
    let target = lead.workspace.as_ref().unwrap().target.clone().unwrap();
    let files = super::git(&flow.repo, &["ls-tree", "-r", "--name-only", &target]);
    for file in ["a.txt", "b.txt"] {
        assert!(files.contains(file), "{file}: {files}");
    }
    flow.stop().await;
}

/// In plan mode a lead given a phase of the request's plan is held to its outline too: being
/// assigned a phase is no go-ahead.
#[tokio::test]
async fn in_plan_mode_a_phase_lead_outlines_and_builds_nothing() {
    let flow = Flow::start(
        "plan-mode-phases",
        Options {
            plan_mode: true,
            ..Options::default()
        },
        script(|turn| async move {
            if turn.is_orchestrator() {
                if !turn.input.contains("[outline") {
                    let reply = turn
                        .call(
                            "plan_phases",
                            json!({"title": "Two files", "phases": [
                                {"title": "Phase one"}, {"title": "Phase two"}]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Phase one", "kind": "implement",
                                   "spec": "Build phase one.", "phase": 1,
                                   "provider": "claude"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                }
                return Reply::text("[quiet]");
            }
            assert!(turn.prompt.contains("Plan mode is on: change nothing yet"));
            for (tool, args) in [
                ("review_code", json!({})),
                ("submit_report", json!({"summary": "Built phase one."})),
            ] {
                let refused = turn.call(tool, args).await;
                assert!(refused.is_error, "{tool}: {}", refused.text);
                assert!(refused.text.contains("Plan mode is on"), "{}", refused.text);
            }
            let reply = turn
                .call("submit_outline", json!({"outline": "1. Create p1.txt"}))
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Waiting for the go-ahead.")
        }),
    )
    .await;
    flow.say("Make two files, one phase each.").await;
    let board = flow
        .until("the outline waits for the user", |board| {
            board.plans.values().any(|plan| {
                plan.steps
                    .iter()
                    .any(|step| step.stage == crate::work::PhaseStage::AwaitingGoAhead)
            })
        })
        .await;
    assert!(Flow::task(&board, 1).report.is_none());
    flow.stop().await;
}

/// A phase whose only commit is litter has nothing to land: the verifier the orchestrator
/// started and its lead both end, and the phase is done, so nothing is left open for the
/// session.
#[tokio::test]
async fn a_phase_with_only_litter_ends_its_lead_with_its_verifier() {
    let flow = Flow::start(
        "litter-only",
        Options::default(),
        script(|turn| async move {
            if turn.is_orchestrator() {
                if turn.input.contains("[outline task-1") {
                    let reply = turn
                        .call("approve_outline", json!({"task": "task-1"}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                if reports_in(&turn.input).last() == Some(&1) {
                    let reply = turn.call("start_verifier", json!({"task": "task-1"})).await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                if let Some(n) = reports_in(&turn.input).last() {
                    let reply = turn
                        .call("land_phase", json!({"task": format!("task-{n}")}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    assert!(reply.text.contains("nothing to land"), "{}", reply.text);
                    return Reply::text("Nothing to land.");
                }
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "high", "title": "Look into it", "kind": "implement",
                               "spec": "Look into the logs.", "provider": "claude"}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("[quiet]");
            }
            if turn.prompt.contains("You verify this phase") {
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Nothing to fix.",
                               "done_when": "[met] looked: read the log"}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Verified.");
            }
            if turn.earlier == 0 {
                let reply = turn
                    .call(
                        "submit_outline",
                        json!({"outline": "1. Read the logs\n2. Report"}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Waiting for the go-ahead.");
            }
            turn.write("debug.log", "scratch\n");
            turn.git(&["add", "-f", "debug.log"]);
            turn.git(&["commit", "-q", "-m", "Keep a log"]);
            let reply = turn
                .call(
                    "submit_report",
                    json!({"summary": "Looked; nothing to change."}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        }),
    )
    .await;
    flow.say("Look into the logs, carefully.").await;
    let board = flow
        .until("every task ends", |board| {
            board
                .tasks
                .values()
                .any(|task| task.role == Some(crate::work::WorkerRole::Verifier))
                && board.tasks.values().all(|task| task.state.is_final())
        })
        .await;
    let lead = Flow::task(&board, 1);
    assert_eq!(lead.state, TaskState::Done);
    let plan = board.plans.values().next().expect("its one-phase plan");
    assert_eq!(plan.steps[0].stage, crate::work::PhaseStage::Done);
    let target = lead.workspace.as_ref().unwrap().target.clone().unwrap();
    let files = super::git(&flow.repo, &["ls-tree", "-r", "--name-only", &target]);
    assert!(!files.contains("debug.log"), "{files}");
    flow.stop().await;
}

#[tokio::test]
async fn a_codex_thread_s_own_commands_and_edits_are_tool_steps() {
    use crate::work::OrchestratorStepKind;
    use brigadier_providers::{
        FileChange, FileChangeKind, ItemStatus, ProviderEvent, ProviderKind,
    };
    let flow = Flow::start(
        "codex-shell-steps",
        Options {
            thread: ProviderKind::Codex,
            ..Options::default()
        },
        script(|turn| async move {
            for status in [ItemStatus::InProgress, ItemStatus::Completed] {
                turn.events
                    .send(ProviderEvent::Command {
                        item_id: "cmd-1".into(),
                        command: "/bin/zsh -lc 'cargo test -p core'".into(),
                        cwd: None,
                        status,
                        exit_code: (status == ItemStatus::Completed).then_some(0),
                        output: None,
                        duration_ms: None,
                    })
                    .await
                    .unwrap();
            }
            turn.events
                .send(ProviderEvent::FileChanges {
                    item_id: "patch-1".into(),
                    changes: vec![
                        FileChange {
                            path: "src/a.rs".into(),
                            kind: FileChangeKind::Update,
                            diff: None,
                        },
                        FileChange {
                            path: "src/b.rs".into(),
                            kind: FileChangeKind::Add,
                            diff: None,
                        },
                    ],
                    status: ItemStatus::Completed,
                })
                .await
                .unwrap();
            Reply::text("Tests pass.")
        }),
    )
    .await;
    flow.say("Run the tests").await;
    let board = flow.settled().await;
    let tools: Vec<(String, Option<String>, ItemStatus)> = board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            OrchestratorStepKind::Tool {
                name,
                detail,
                status,
                ..
            } => Some((name.clone(), detail.clone(), *status)),
            _ => None,
        })
        .collect();
    assert_eq!(
        tools,
        vec![
            (
                "shell".to_owned(),
                Some("cargo test -p core".to_owned()),
                ItemStatus::Completed
            ),
            (
                "apply_patch".to_owned(),
                Some("src/a.rs and 1 more".to_owned()),
                ItemStatus::Completed
            ),
        ]
    );
    flow.stop().await;
}

#[tokio::test]
async fn a_finished_tool_step_says_when_it_ended_and_how_it_exited_and_opens_in_full() {
    use crate::work::{OrchestratorStepKind, OutputSource};
    use brigadier_providers::{ItemStatus, ProviderEvent};
    let digest = Arc::new(std::sync::Mutex::new(String::new()));
    let sent = digest.clone();
    let flow = Flow::start(
        "thread-items",
        Options::default(),
        script(move |turn| {
            let digest = sent.lock().unwrap().clone();
            async move {
                for status in [ItemStatus::InProgress, ItemStatus::Failed] {
                    let finished = status != ItemStatus::InProgress;
                    turn.events
                        .send(ProviderEvent::Command {
                            item_id: "cmd-1".into(),
                            command: "pnpm test".into(),
                            cwd: None,
                            status,
                            exit_code: finished.then_some(1),
                            output: finished.then(|| "1 failed".into()),
                            duration_ms: finished.then_some(41_000),
                        })
                        .await
                        .unwrap();
                }
                for (status, output) in [
                    (ItemStatus::InProgress, None),
                    (ItemStatus::Completed, Some(digest)),
                ] {
                    turn.events
                        .send(ProviderEvent::ToolCall {
                            item_id: "run-1".into(),
                            name: "mcp__brigadier__run".into(),
                            input: Some(json!({"command": "cargo test"}).to_string()),
                            status,
                            output,
                        })
                        .await
                        .unwrap();
                }
                Reply::text("One test fails.")
            }
        }),
    )
    .await;
    // A long `run` output the model got as a digest: its row opens to the whole output.
    let full = (0..3000)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let (_, stored) = flow
        .manager
        .store_output(
            &flow.conversation,
            OutputSource::Run,
            "exit 2",
            full.clone().into_bytes(),
            true,
        )
        .await
        .unwrap();
    assert!(stored.len() < full.len());
    *digest.lock().unwrap() = stored;
    flow.say("Run the tests").await;
    let board = flow.settled().await;
    let tools: Vec<_> = board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            OrchestratorStepKind::Tool {
                item_id,
                status,
                ended_at_ms,
                exit,
                ..
            } => Some((step.at_ms, item_id.clone(), *status, *ended_at_ms, *exit)),
            _ => None,
        })
        .collect();
    assert_eq!(tools.len(), 2, "{tools:?}");
    for (at_ms, item_id, status, ended_at_ms, exit) in &tools {
        let ended = ended_at_ms.expect("a finished call says when it ended");
        assert!(ended >= *at_ms, "{item_id} ended before it started");
        match item_id.as_str() {
            "cmd-1" => assert_eq!((*status, *exit), (ItemStatus::Failed, Some(1))),
            "run-1" => assert_eq!((*status, *exit), (ItemStatus::Completed, Some(2))),
            other => panic!("unexpected step {other}"),
        }
    }

    let command = flow
        .core
        .thread_item(&flow.conversation, "cmd-1")
        .await
        .unwrap();
    assert_eq!(
        command,
        crate::model::ThreadItem {
            input: Some("pnpm test".into()),
            output: Some("1 failed".into()),
            exit: Some(1),
            ms: Some(41_000),
        }
    );
    let run = flow
        .core
        .thread_item(&flow.conversation, "run-1")
        .await
        .unwrap();
    assert_eq!(
        run.input.as_deref(),
        Some(json!({"command": "cargo test"}).to_string().as_str())
    );
    assert_eq!(run.output.as_deref(), Some(full.as_str()));
    assert_eq!(run.exit, Some(2));
    assert!(run.ms.is_some_and(|ms| ms >= 0));
    let missing = flow
        .core
        .thread_item(&flow.conversation, "no-such-item")
        .await
        .unwrap();
    assert_eq!(missing, crate::model::ThreadItem::default());
    flow.stop().await;
}

#[tokio::test]
async fn orchestrator_tools_are_visible_while_running_and_keep_their_first_position() {
    use crate::work::OrchestratorStepKind;
    use brigadier_providers::{ItemStatus, ProviderEvent};
    let release = Arc::new(tokio::sync::Notify::new());
    let gate = release.clone();
    let mut flow = Flow::start(
        "live-tools",
        Options::default(),
        script(move |turn| {
            let gate = gate.clone();
            async move {
                turn.events
                    .send(ProviderEvent::ToolCall {
                        item_id: "brain-call".into(),
                        name: "mcp__brigadier__query_brain".into(),
                        input: None,
                        status: ItemStatus::InProgress,
                        output: None,
                    })
                    .await
                    .unwrap();
                gate.notified().await;
                turn.events
                    .send(ProviderEvent::ToolCall {
                        item_id: "brain-call".into(),
                        name: "mcp__brigadier__query_brain".into(),
                        input: Some(json!({"query":"composer attachments"}).to_string()),
                        status: ItemStatus::Completed,
                        output: Some("Found the composer".into()),
                    })
                    .await
                    .unwrap();
                Reply::text("The composer handles attachments.")
            }
        }),
    )
    .await;
    flow.say("Find the composer").await;
    let running = flow
        .until("a running tool in the thread", |board| {
            board.orchestrator_steps.iter().any(|step| {
                matches!(
                    step.kind,
                    OrchestratorStepKind::Tool {
                        status: ItemStatus::InProgress,
                        ..
                    }
                )
            })
        })
        .await;
    let first = &running.orchestrator_steps[0];
    assert!(first.request_id.is_some());
    assert!(
        running
            .requests
            .values()
            .any(|request| request.state == RequestState::Working)
    );
    release.notify_one();
    let finished = flow.settled().await;
    assert_eq!(finished.orchestrator_steps.len(), 1);
    let last = &finished.orchestrator_steps[0];
    assert_eq!(last.position, first.position);
    assert_eq!(last.at_ms, first.at_ms);
    assert!(
        matches!(&last.kind, OrchestratorStepKind::Tool { name, detail: Some(detail), status: ItemStatus::Completed, through_position, .. }
        if name == "query_brain" && detail == "composer attachments" && *through_position > last.position)
    );
    flow.restart().await;
    let restored = flow.board().await;
    assert_eq!(restored.orchestrator_steps, finished.orchestrator_steps);
    flow.stop().await;
}
