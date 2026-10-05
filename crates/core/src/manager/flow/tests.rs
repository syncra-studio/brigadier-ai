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
    let mut tasks = 0;
    for conversation in flow.core.catalog().conversations {
        let board = flow.core.board(&conversation.id).await.unwrap();
        tasks += board.tasks.len();
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
    flow.stop().await;
}

/// What every scripted CLI was asked, in order.
type Heard = Arc<std::sync::Mutex<Vec<String>>>;

fn is_reviewer(turn: &Turn) -> bool {
    turn.prompt.contains("Kind: review")
}

/// A lead outlines big work and waits; one reviewer from the other vendor reads the outline;
/// the orchestrator sends the go-ahead with its corrections, and the lead builds.
#[tokio::test]
async fn an_outline_gets_one_review_from_the_other_vendor_and_a_go_ahead() {
    let heard: Heard = Arc::default();
    let log = heard.clone();
    let flow = Flow::start(
        "outline",
        Options::default(),
        script(move |turn| {
            log.lock().unwrap().push(turn.input.clone());
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("[outline review]") {
                        assert!(turn.input.contains("Step 2 misses the caller in b.rs"));
                        let reply = turn
                            .call(
                                "approve_outline",
                                json!({"task": "task-1", "corrections": "Also update b.rs."}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("[quiet]");
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
                if is_reviewer(&turn) {
                    assert!(
                        turn.prompt.contains("1. Read a.rs"),
                        "the reviewer reads the outline"
                    );
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "One finding.",
                                   "open_questions": "Step 2 misses the caller in b.rs"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reviewed.");
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
    let board = flow.settled().await;
    let lead = Flow::task(&board, 1);
    let reviewer = Flow::task(&board, 2);
    assert_eq!(board.tasks.len(), 2, "one lead and one reviewer");
    assert_eq!(lead.role, Some(crate::work::WorkerRole::Lead));
    assert_eq!(reviewer.role, Some(crate::work::WorkerRole::Reviewer));
    assert_ne!(
        reviewer.route.choice.provider, lead.route.choice.provider,
        "the review comes from the other vendor"
    );
    assert_eq!(lead.state, TaskState::Done);
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
    let reviews = heard
        .lock()
        .unwrap()
        .iter()
        .filter(|input| input.contains("[outline review]"))
        .count();
    assert_eq!(
        reviews, 1,
        "the orchestrator gets the outline once, with its one review"
    );
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
/// which can't commit from its sandbox, so Brigadier commits for it) asks for its own review,
/// which comes from the other vendor and reads its work; then the orchestrator lands it. No
/// verifier, no cards.
#[tokio::test]
async fn a_small_request_is_reviewed_by_its_lead_and_lands_without_a_verifier() {
    let flow = Flow::start(
        "small",
        Options::default(),
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
            if is_reviewer(&turn) {
                assert!(
                    turn.cwd.join("hello.txt").exists(),
                    "the reviewer's checkout holds the work"
                );
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "One finding.",
                               "open_questions": "hello.txt lacks a trailing newline"}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Reviewed.");
            }
            // The lead never commits: its sandbox can't.
            turn.write("hello.txt", "hello");
            let review = turn.call("request_review", json!({})).await;
            assert!(!review.is_error, "{}", review.text);
            assert!(review.text.contains("trailing newline"), "{}", review.text);
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
    let board = flow.settled().await;
    let lead = Flow::task(&board, 1);
    let reviewer = Flow::task(&board, 2);
    assert_eq!(
        board.tasks.len(),
        2,
        "a lead and its one reviewer, no verifier"
    );
    assert_eq!(lead.state, TaskState::Landed);
    assert_eq!(reviewer.role, Some(crate::work::WorkerRole::Reviewer));
    assert_ne!(reviewer.route.choice.provider, lead.route.choice.provider);
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

/// Done when (2): a request of two phases. Phase 1's lead outlines, gets one review of its
/// outline from the other vendor and the go-ahead, builds and reports; a fresh verifier asks
/// for one review of the phase (from the vendor other than the lead's), fixes, commits and
/// reports; the orchestrator lands the phase and starts phase 2, whose lead reviews its own
/// small change; then the final answer. No plan rounds, no per-change checks, no cards.
#[tokio::test]
async fn an_outlined_phase_is_verified_and_landed_before_the_next_phase() {
    let heard: Heard = Arc::default();
    let log = heard.clone();
    let landed = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let flow = Flow::start(
        "phases",
        Options::default(),
        script(move |turn| {
            log.lock().unwrap().push(turn.input.clone());
            let landed = landed.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("[outline review]") {
                        let reply = turn
                            .call(
                                "approve_outline",
                                json!({"task": "task-1", "corrections": "Name the file p1.txt."}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("[quiet]");
                    }
                    if turn.input.contains("[phase verifier]") {
                        // The lead's report: the verifier lands the phase.
                        let refused = turn.call("land_phase", json!({"task": "task-1"})).await;
                        assert!(refused.is_error, "the lead alone doesn't land");
                        return Reply::text("[quiet]");
                    }
                    if let Some(n) = reports_in(&turn.input).last() {
                        let reply = turn
                            .call("land_phase", json!({"task": format!("task-{n}")}))
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        assert!(reply.text.contains("Landed"), "{}", reply.text);
                        if landed.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
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
                        return Reply::text("Both phases are done.\n\nWaiting on you: nothing.");
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
                if is_reviewer(&turn) {
                    let reply = turn
                        .call("submit_report", json!({"summary": "No findings."}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reviewed.");
                }
                if turn.prompt.contains("You verify this phase") {
                    assert!(
                        turn.cwd.join("p1.txt").exists(),
                        "it starts from the lead's work"
                    );
                    let review = turn.call("request_review", json!({})).await;
                    assert!(!review.is_error, "{}", review.text);
                    turn.write("p1-fix.txt", "fixed\n");
                    turn.git(&["add", "p1-fix.txt"]);
                    turn.git(&["commit", "-q", "-m", "Fix what the review found"]);
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
                    let review = turn.call("request_review", json!({})).await;
                    assert!(!review.is_error, "{}", review.text);
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
    use crate::work::WorkerRole as R;
    let roles: Vec<_> = {
        let mut tasks: Vec<_> = board.tasks.values().collect();
        tasks.sort_by_key(|task| task.number);
        tasks.iter().map(|task| (task.number, task.role)).collect()
    };
    let lead = Flow::task(&board, 1);
    let verifier = board
        .tasks
        .values()
        .find(|task| task.role == Some(R::Verifier))
        .expect("a verifier");
    assert_eq!(verifier.subject.as_ref(), Some(&lead.id));
    assert_eq!(lead.state, TaskState::Landed, "{roles:?}");
    assert_eq!(verifier.state, TaskState::Landed);
    // Three reviews in all: the outline's, the verifier's and phase two's lead's.
    let reviewers: Vec<_> = board
        .tasks
        .values()
        .filter(|task| task.role == Some(R::Reviewer))
        .collect();
    assert_eq!(reviewers.len(), 3, "{roles:?}");
    let of_phase_one: Vec<_> = reviewers
        .iter()
        .filter(|task| task.phase == Some(1))
        .collect();
    assert_eq!(of_phase_one.len(), 2);
    for reviewer in of_phase_one {
        assert_ne!(
            reviewer.route.choice.provider, lead.route.choice.provider,
            "phase one is reviewed by the vendor other than its lead's"
        );
    }
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
    assert_eq!(said.matches("[outline review]").count(), 1);
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
