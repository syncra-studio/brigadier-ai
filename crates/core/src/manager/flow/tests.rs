use std::sync::Arc;

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::work::{ApprovalSubject, CardState, RequestState, TaskState};

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
                        json!({"title": "Look around", "kind": "scout", "spec": "List the files."}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("[quiet]");
            }
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
    flow.stop().await;
}

/// A store an earlier version left behind (real records, their text redacted, plus a landing
/// that waited for the user's approval) loads, and nothing in it acts again.
#[tokio::test]
async fn a_store_from_before_the_phase_flow_loads_and_recovers() {
    let told: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    let heard = told.clone();
    let flow = Flow::start(
        "legacy",
        Options {
            seed: Some(include_str!("fixtures/pre-flow-events.jsonl")),
            ..Options::default()
        },
        script(move |turn| {
            heard.lock().unwrap().push(turn.input.clone());
            async { Reply::text("[quiet]") }
        }),
    )
    .await;
    let seeded = [
        "01a10853-80c6-75fc-9788-21c770d02049",
        "01a108f1-a812-73cd-98ec-4eee23126abe",
    ];
    let conversations: Vec<_> = flow
        .core
        .catalog()
        .conversations
        .into_iter()
        .filter(|conversation| seeded.contains(&conversation.id.0.as_str()))
        .collect();
    assert_eq!(conversations.len(), 2, "both conversations load");
    for conversation in &conversations {
        let board = flow.core.board(&conversation.id).await.unwrap();
        assert!(!board.tasks.is_empty());
        // A landing that waited for the user's click is over: landings don't ask now.
        assert!(
            board.approvals.values().all(|approval| !matches!(
                approval.subject,
                ApprovalSubject::Landing { .. }
            ) || approval.state != CardState::Pending),
            "no landing card is live"
        );
        // Nothing is left mid-landing: an interrupted landing is the orchestrator's again.
        assert!(
            board
                .tasks
                .values()
                .filter(|task| task.kind.writes())
                .all(|task| task.state.is_final()
                    || matches!(task.state, TaskState::Reported | TaskState::ReadyToLand)),
            "{:?}",
            board
                .tasks
                .values()
                .map(|task| (task.number, task.state))
                .collect::<Vec<_>>()
        );
    }
    let legacy = "01a10888-d9e7-764b-9843-72f98b3b7d1d";
    let board = flow.core.board(&conversations[0].id).await.unwrap();
    let board = if board.tasks.keys().any(|id| id.0 == legacy) {
        board
    } else {
        flow.core.board(&conversations[1].id).await.unwrap()
    };
    let task = board
        .tasks
        .values()
        .find(|task| task.id.0 == legacy)
        .unwrap();
    assert_eq!(task.state, TaskState::Reported);
    let number = task.number;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    while !told
        .lock()
        .unwrap()
        .iter()
        .any(|input| input.contains(&format!("[not landed task-{number} ")))
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the orchestrator is told"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    flow.stop().await;
}

/// A copy of a real store (`BRIGADIER_FLOW_STORE`, made read-only with `sqlite3 "file:…?mode=ro"
/// ".backup …"`): every conversation loads after a restart, and no landing card is live.
/// Run by hand: `BRIGADIER_FLOW_STORE=/tmp/brig-store-copy.db cargo test -p brigadier-core
/// --lib real_store -- --ignored`.
#[tokio::test]
#[ignore = "needs a copy of a real store"]
async fn a_real_store_loads_and_recovers() {
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
    let (mut sessions, mut tasks, mut plans, mut runs) = (0, 0, 0, 0);
    for conversation in flow.core.catalog().conversations {
        let board = flow.core.board(&conversation.id).await.unwrap();
        sessions += 1;
        tasks += board.tasks.len();
        plans += board.plans.len();
        runs += board.runs.len();
        for approval in board.approvals.values() {
            assert!(
                !matches!(approval.subject, ApprovalSubject::Landing { .. })
                    || approval.state != CardState::Pending
            );
        }
        for task in board.tasks.values().filter(|task| task.kind.writes()) {
            assert!(
                task.state.is_final()
                    || matches!(task.state, TaskState::Reported | TaskState::ReadyToLand),
                "task-{} is {:?}",
                task.number,
                task.state
            );
        }
    }
    assert!(tasks > 0, "the store holds tasks");
    eprintln!(
        "loaded {sessions} conversations, {tasks} tasks, {plans} plans, {runs} overnight runs"
    );
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
                            json!({"title": "Rework the parser", "kind": "implement",
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
                                json!({"title": title, "kind": "implement",
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
                        json!({"title": "Add a greeting", "kind": "implement",
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
    flow.say("Add a greeting file.").await;
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
                                        json!({"title": "Phase two", "kind": "implement",
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
                            json!({"title": "Phase one", "kind": "implement",
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
                        json!({"title": "Pick a greeting", "kind": "scout",
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
                            json!({"title": "Add two files", "kind": "implement",
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
                        json!({"title": "Add the file", "kind": "implement",
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
                            json!({"title": "Add a file", "kind": "implement",
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
    assert!(Flow::task(&board, 1).report.is_none());
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
                        json!({"title": "Add a file", "kind": "implement",
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
                            json!({"title": "Fix it", "kind": "implement", "role": "fix",
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
                        json!({"title": "Add a.txt", "kind": "implement",
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
                            json!({"title": "Phase one", "kind": "implement",
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
                        json!({"title": "Look into it", "kind": "implement",
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
