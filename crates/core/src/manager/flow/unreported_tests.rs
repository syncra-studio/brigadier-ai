//! A worker's new file that its report doesn't name never vanishes at landing: the report is
//! refused, naming it, until the worker lists it or deletes it.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::work::TaskState;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// The worker adds a module and its test, committed, staged or untracked, and reports only
/// the file it changed. Its report is refused, naming each new file but its litter; it lists
/// the work, deletes a stray file, and everything it listed lands.
#[tokio::test]
async fn new_files_a_report_leaves_out_are_handed_back_not_dropped() {
    let refusals = Arc::new(Mutex::new(Vec::new()));
    let seen = refusals.clone();
    let flow = Flow::start(
        "unreported",
        Options::default(),
        script(move |turn| {
            let refusals = refusals.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("[report task-1 ") {
                        let reply = turn.call("land_phase", json!({"task": "task-1"})).await;
                        assert!(!reply.is_error, "{}", reply.text);
                        assert!(reply.text.contains("Landed"), "{}", reply.text);
                        return Reply::text("Discount codes are in.");
                    }
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Discount codes", "kind": "implement",
                                   "spec": "Add discount codes to the cart."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                turn.write("src/cart.ts", "export const cart = [];\n");
                turn.write("src/discounts.ts", "export const discount = 0.1;\n");
                turn.git(&["add", "src/cart.ts", "src/discounts.ts"]);
                turn.git(&["commit", "-q", "-m", "Add discount codes"]);
                // Staged, not committed.
                turn.write("src/discounts.test.ts", "test('discount', () => {});\n");
                turn.git(&["add", "src/discounts.test.ts"]);
                // Untracked: one belongs to the work, one is a stray the worker deletes.
                turn.write("src/rules.ts", "export const rules = [];\n");
                turn.write("src/probe.ts", "console.log(1);\n");
                // Litter stays litter: not handed back.
                turn.write("debug.log", "scratch\n");
                turn.write("NOTES.md", "thinking\n");
                let refused = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Added discount codes.", "changes": ["src/cart.ts"]}),
                    )
                    .await;
                assert!(refused.is_error, "{}", refused.text);
                refusals.lock().unwrap().push(refused.text.clone());
                std::fs::remove_file(turn.cwd.join("src/probe.ts")).unwrap();
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Added discount codes.",
                               "changes": ["src/cart.ts", "src/discounts.ts",
                                           "src/discounts.test.ts", "src/rules.ts"]}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Add discount codes.").await;
    let board = flow
        .until("it to land", |board| {
            board
                .tasks
                .values()
                .any(|task| task.state == TaskState::Landed)
        })
        .await;
    let refused = seen.lock().unwrap().join("\n");
    for path in [
        "src/discounts.ts",
        "src/discounts.test.ts",
        "src/rules.ts",
        "src/probe.ts",
    ] {
        assert!(refused.contains(&format!("`{path}`")), "{path}: {refused}");
    }
    for path in ["src/cart.ts", "debug.log", "NOTES.md"] {
        assert!(!refused.contains(path), "{path}: {refused}");
    }
    assert!(refused.contains("Nothing was reported"), "{refused}");
    let task = Flow::task(&board, 1);
    let target = task.workspace.as_ref().unwrap().target.clone().unwrap();
    let files = super::git(&flow.repo, &["ls-tree", "-r", "--name-only", &target]);
    for path in [
        "src/cart.ts",
        "src/discounts.ts",
        "src/discounts.test.ts",
        "src/rules.ts",
    ] {
        assert!(files.lines().any(|line| line == path), "{path}: {files}");
    }
    for path in ["src/probe.ts", "debug.log", "NOTES.md"] {
        assert!(!files.lines().any(|line| line == path), "{path}: {files}");
    }
    flow.settled().await;
    flow.stop().await;
}
