//! `run_check` end to end (THREAD-PLAN.md Q8 lever 3): a worker's check runs once per tree,
//! the thread re-checking the same files gets the worker's result from the cache, `rerun` and
//! an edit run it again, a folder outside git leaves the cache out, and a pass is learned into
//! the project's Brain.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_brain::NodeKind;
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::model::DomainEvent;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

#[tokio::test]
async fn a_check_runs_once_per_tree_for_workers_and_the_thread() {
    let replies: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        "checks",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                let check = |args: serde_json::Value| {
                    let turn = &turn;
                    let log = log.clone();
                    async move {
                        let reply = turn.call("run_check", args).await;
                        assert!(!reply.is_error, "{}", reply.text);
                        log.lock().unwrap().push(reply.text);
                    }
                };
                if !turn.is_orchestrator() {
                    check(json!({})).await;
                    check(json!({ "command": "echo checked" })).await;
                    check(json!({ "command": "echo checked" })).await;
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "The check passes.", "verification": "echo checked"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                if turn.earlier == 0 {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"title": "Run the check", "kind": "scout",
                                   "spec": "Run the check."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                // The worker's report: the same files, so the same result.
                check(json!({ "command": "echo checked" })).await;
                check(json!({ "command": "echo checked", "rerun": true })).await;
                std::fs::write(turn.add_dirs[0].join("greeting.txt"), "hello\n").unwrap();
                check(json!({ "command": "echo checked" })).await;
                check(json!({ "command": "echo checked", "workdir": turn.cwd })).await;
                Reply::text("Checked.")
            }
        }),
    )
    .await;
    flow.say("Check the greeting.").await;
    flow.until("the thread's checks", |_| {
        replies.lock().unwrap().len() == 7
    })
    .await;
    flow.settled().await;

    let replies = replies.lock().unwrap().clone();
    assert!(replies[0].starts_with("Nothing changed"), "{}", replies[0]);
    assert_eq!(replies[1], "[exit 0]\nchecked\n");
    assert!(replies[2].starts_with("[cached: "), "{}", replies[2]);
    assert!(
        replies[2].ends_with("[exit 0]\nchecked\n"),
        "{}",
        replies[2]
    );
    assert!(replies[3].starts_with("[cached: "), "{}", replies[3]);
    for fresh in &replies[4..] {
        assert_eq!(fresh, "[exit 0]\nchecked\n");
    }

    let checks: Vec<_> = flow
        .events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            DomainEvent::CheckRan {
                task_id,
                tree,
                cached,
                bypassed,
                status,
                artifact,
                workdir,
                ..
            } => {
                assert_eq!(status, "exit 0");
                assert!(artifact.is_some());
                assert_eq!(workdir, "");
                Some((task_id.is_some(), cached, bypassed, tree))
            }
            _ => None,
        })
        .collect();
    let shape: Vec<_> = checks
        .iter()
        .map(|(worker, cached, bypassed, _)| (*worker, *cached, bypassed.clone()))
        .collect();
    assert_eq!(
        shape,
        [
            (true, false, None),
            (true, true, None),
            (false, true, None),
            (false, false, None),
            (false, false, None),
            (false, false, Some("not in a git repository".to_owned())),
        ]
    );
    // The thread's files were the worker's; after the edit they are not.
    assert_eq!(checks[0].3, checks[2].3);
    assert_eq!(checks[2].3, checks[3].3);
    assert_ne!(checks[3].3, checks[4].3);
    assert_eq!(checks[5].3, None);

    // The pass is learned for the root's folder, once.
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id
        .unwrap();
    let brain = flow.manager.project_brain(&project).await.unwrap();
    let mut learned = None;
    for _ in 0..100 {
        learned = brain
            .brain
            .node_by_key(NodeKind::Convention, "checks:.")
            .unwrap();
        if learned.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let learned = learned.expect("the pass was learned");
    assert_eq!(
        learned.body.matches("`echo checked`").count(),
        1,
        "{}",
        learned.body
    );
    flow.stop().await;
}
