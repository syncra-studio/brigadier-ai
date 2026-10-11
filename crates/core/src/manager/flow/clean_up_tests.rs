//! `clean_up` (SESSION-FIXES-PLAN.md bug 6): it goes only by this conversation's records, so
//! it removes its own merged leftovers and never another session's branch.

use std::sync::Arc;

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn, git};
use crate::work::TaskState;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

#[tokio::test]
async fn clean_up_removes_its_own_merged_branch_and_keeps_another_sessions() {
    let flow = Flow::start(
        "clean-up",
        Options::default(),
        script(|turn| async move {
            if turn.is_review() {
                return Reply::text("No findings.");
            }
            if !turn.is_orchestrator() {
                if !turn.cwd.join("A.md").exists() {
                    turn.write("A.md", "note\n");
                    turn.git(&["add", "A.md"]);
                    turn.git(&["commit", "-qm", "Write A.md"]);
                }
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Wrote A.md.", "changes": ["A.md"]}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Reported.");
            }
            if turn.input.starts_with("Add a note") {
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "medium", "title": "Write note A", "kind": "implement",
                               "spec": "Write the note.", "provider": "claude"}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("[quiet]");
            }
            if turn.input.contains("[report task-1") {
                let reply = turn.call("land_phase", json!({"task": "task-1"})).await;
                assert!(!reply.is_error, "{}", reply.text);
            }
            Reply::text("[quiet]")
        }),
    )
    .await;
    flow.say("Add a note.").await;
    let board = flow
        .until("the landing", |board| {
            board
                .tasks
                .values()
                .any(|task| task.number == 1 && task.state == TaskState::Landed)
        })
        .await;
    flow.settled().await;
    let task = Flow::task(&board, 1).clone();
    let workspace = task.workspace.clone().unwrap();
    let (branch, target) = (workspace.branch.unwrap(), workspace.target.unwrap());
    // The landing deleted its branch; a leftover of it, merged, is this session's to remove.
    let tip = git(&flow.repo, &["rev-parse", &target]);
    git(&flow.repo, &["branch", "-f", &branch, &tip]);
    // Another session's branch at the same commit is never this one's.
    let other = "brigadier/0000beef/task-1-other-work";
    git(&flow.repo, &["branch", other, &tip]);
    let text = flow.manager.clean_up(&flow.conversation).await.unwrap();
    assert!(
        text.contains(&format!("Removed: branch `{branch}` (task-1, merged)")),
        "{text}"
    );
    assert!(text.contains("never touched"), "{text}");
    let branches = git(&flow.repo, &["branch", "--format=%(refname:short)"]);
    assert!(!branches.lines().any(|line| line == branch), "{branches}");
    assert!(branches.lines().any(|line| line == other), "{branches}");
    // Nothing more to remove the second time.
    let again = flow.manager.clean_up(&flow.conversation).await.unwrap();
    assert!(again.starts_with("Nothing to remove"), "{again}");
    flow.stop().await;
}
