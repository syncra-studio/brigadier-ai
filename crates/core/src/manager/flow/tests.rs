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
