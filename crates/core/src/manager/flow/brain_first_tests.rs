//! Brain first (SESSION-FIXES-PLAN.md Q6.6): a worker's brief carries what the project's Brain
//! knows about its task, what it reports under `learned` becomes Brain facts, its Brain use is
//! counted on its task, and a worker that doesn't ask the Brain in its first calls is told to.

use std::sync::{Arc, Mutex};

use brigadier_brain::{NewNode, NodeFilter, NodeKind, Origin, Provenance};
use brigadier_providers::{ItemStatus, ProviderEvent};
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::brain_first::{BRAIN_FIRST, STEER_AFTER};
use crate::manager::context_pack::BRAIN_HEADING;
use crate::work::TaskState;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// `count` finished commands, as the worker's CLI reports them.
async fn commands(turn: &Turn, count: u32) {
    for n in 0..count {
        turn.emit(ProviderEvent::Command {
            item_id: format!("cmd-{n}"),
            command: "ls".into(),
            cwd: None,
            status: ItemStatus::Completed,
            exit_code: Some(0),
            output: None,
            duration_ms: None,
            kept: None,
        })
        .await;
    }
}

/// The orchestrator delegates one scout when the user asks; the worker does `work`.
fn session<W, Fut>(work: W) -> Script
where
    W: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    let work = Arc::new(work);
    script(move |turn| {
        let work = work.clone();
        async move {
            if turn.is_orchestrator() {
                if turn.input.contains("Where do notes go?") {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Find where notes are kept",
                                   "kind": "scout", "spec": "Find where the notes are kept and how."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                return Reply::text("Noted.");
            }
            work(turn).await
        }
    })
}

#[tokio::test]
async fn a_brief_carries_what_the_brain_knows_and_what_the_worker_learned_is_kept() {
    let first: Arc<Mutex<Option<String>>> = Arc::default();
    let log = first.clone();
    let flow = Flow::start(
        "brain-brief",
        Options::default(),
        session(move |turn| {
            let log = log.clone();
            async move {
                log.lock().unwrap().get_or_insert(turn.input.clone());
                let reply = turn.call("query_brain", json!({"query": "notes"})).await;
                assert!(!reply.is_error, "{}", reply.text);
                commands(&turn, STEER_AFTER + 2).await;
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Notes go in NOTES.md.",
                               "decisions": "Keep one note per line.",
                               "learned": "NOTES.md sits at the repository root.\nThe notes test is notes_roundtrip."}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id
        .unwrap();
    let brain = flow
        .manager
        .project_brain(&project)
        .await
        .unwrap()
        .brain
        .clone();
    let known = brain
        .record(NewNode {
            kind: NodeKind::Convention,
            key: None,
            title: "Notes are kept in NOTES.md".into(),
            body: "Each note is one line of NOTES.md.\nNever a database.".into(),
            provenance: Provenance {
                origin: Origin::Orchestrator,
                session_id: None,
                task_id: None,
                job_id: None,
                worker: None,
                commit: None,
                recorded_at_ms: 0,
                claim: None,
            },
            files: Vec::new(),
            expires_at_ms: None,
        })
        .unwrap();

    flow.say("Where do notes go?").await;
    let board = flow
        .until("the scout to end", |board| {
            board
                .tasks
                .values()
                .any(|task| task.number == 1 && task.state.is_final())
        })
        .await;
    let input = first.lock().unwrap().clone().unwrap();
    let section = &input[input.find(BRAIN_HEADING).expect("a Brain section")..];
    assert!(
        section.contains(&format!(
            "- convention [thread claim, unverified]: Notes are kept in NOTES.md. Each note is one line of NOTES.md. Never a database. (node {known})"
        )),
        "{input}"
    );
    // Before the thread's pack and the task.
    assert!(input.find(BRAIN_HEADING) < input.find("Start the task."));
    let task = Flow::task(&board, 1);
    assert_eq!(task.state, TaskState::Done);
    assert_eq!((task.brain.facts_in, task.brain.queries), (1, 1));
    assert_eq!(task.brain.calls, STEER_AFTER + 2);

    // Its decision and both learned items are Brain facts from the task.
    let task_id = task.id.clone();
    let mut kept = Vec::new();
    super::eventually("what it learned to be kept", || {
        kept = brain
            .nodes(&NodeFilter {
                kinds: vec![NodeKind::Convention, NodeKind::Decision],
                ..Default::default()
            })
            .unwrap()
            .into_iter()
            .filter(|node| node.provenance.task_id.as_deref() == Some(task_id.0.as_str()))
            .map(|node| (node.kind, node.title))
            .collect();
        kept.len() == 3
    })
    .await;
    kept.sort_by(|a, b| a.1.cmp(&b.1));
    assert_eq!(
        kept,
        [
            (NodeKind::Decision, "Keep one note per line.".to_owned()),
            (
                NodeKind::Convention,
                "NOTES.md sits at the repository root.".to_owned()
            ),
            (
                NodeKind::Convention,
                "The notes test is notes_roundtrip.".to_owned()
            ),
        ]
    );
    flow.until("its write-backs to be counted", |board| {
        Flow::task(board, 1).brain.facts_out == 3
    })
    .await;
    flow.stop().await;
}

#[tokio::test]
async fn a_worker_that_does_not_ask_the_brain_first_is_told_to() {
    let steered: Arc<Mutex<Option<String>>> = Arc::default();
    let log = steered.clone();
    let flow = Flow::start(
        "brain-steer",
        Options::default(),
        session(move |turn| {
            let log = log.clone();
            async move {
                commands(&turn, STEER_AFTER).await;
                *log.lock().unwrap() = turn.steered().await;
                let reply = turn
                    .call("submit_report", json!({"summary": "Notes go in NOTES.md."}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Where do notes go?").await;
    let board = flow
        .until("the scout to end", |board| {
            board
                .tasks
                .values()
                .any(|task| task.number == 1 && task.state.is_final())
        })
        .await;
    assert_eq!(steered.lock().unwrap().as_deref(), Some(BRAIN_FIRST));
    let task = Flow::task(&board, 1);
    // An empty Brain gives the brief nothing.
    assert_eq!(
        (task.brain.queries, task.brain.facts_in, task.brain.calls),
        (0, 0, STEER_AFTER)
    );
    flow.stop().await;
}
