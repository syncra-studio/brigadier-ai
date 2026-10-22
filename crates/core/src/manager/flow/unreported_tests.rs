//! A worker's report is taken as it is (#249): a new file it doesn't name and a [met] line
//! with no evidence are marked for the thread, which judges, and nothing is refused.

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
/// the file it changed, with a [met] line that names no evidence. Its report is taken at once:
/// the thread reads each new file but its litter marked "new file not named", and the [met]
/// line marked unevidenced; it lands the work as reported, and the unnamed files stay out.
#[tokio::test]
async fn new_files_a_report_leaves_out_and_unevidenced_lines_are_marked_not_refused() {
    let reports = Arc::new(Mutex::new(Vec::new()));
    let seen = reports.clone();
    let flow = Flow::start(
        "unreported",
        Options::default(),
        script(move |turn| {
            let reports = reports.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("[report task-1 ") {
                        reports.lock().unwrap().push(turn.input.clone());
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
                    return Reply::text("");
                }
                turn.write("src/cart.ts", "export const cart = [];\n");
                turn.write("src/discounts.ts", "export const discount = 0.1;\n");
                turn.git(&["add", "src/cart.ts", "src/discounts.ts"]);
                turn.git(&["commit", "-q", "-m", "Add discount codes"]);
                // Staged, not committed.
                turn.write("src/discounts.test.ts", "test('discount', () => {});\n");
                turn.git(&["add", "src/discounts.test.ts"]);
                // Untracked.
                turn.write("src/rules.ts", "export const rules = [];\n");
                // Litter stays litter: not marked.
                turn.write("debug.log", "scratch\n");
                turn.write("NOTES.md", "thinking\n");
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Added discount codes.", "changes": ["src/cart.ts"],
                               "done_when": ["[met] discounts apply: they work"]}),
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
    let report = seen.lock().unwrap().join("\n");
    for path in ["src/discounts.ts", "src/discounts.test.ts", "src/rules.ts"] {
        assert!(
            report.contains(&format!(
                "new file not named: `{path}` (left out of the landing)"
            )),
            "{path}: {report}"
        );
    }
    for path in ["debug.log", "NOTES.md"] {
        assert!(!report.contains(path), "{path}: {report}");
    }
    assert!(
        report.contains("[met] discounts apply: they work [unevidenced]"),
        "{report}"
    );
    let task = Flow::task(&board, 1);
    let target = task.workspace.as_ref().unwrap().target.clone().unwrap();
    let files = super::git(&flow.repo, &["ls-tree", "-r", "--name-only", &target]);
    assert!(files.lines().any(|line| line == "src/cart.ts"), "{files}");
    for path in [
        "src/discounts.test.ts",
        "src/rules.ts",
        "debug.log",
        "NOTES.md",
    ] {
        assert!(!files.lines().any(|line| line == path), "{path}: {files}");
    }
    flow.settled().await;
    flow.stop().await;
}
