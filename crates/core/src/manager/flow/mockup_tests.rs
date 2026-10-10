//! A mockup first (SESSION-FIXES-PLAN.md Q10): the thread delegates a mockup, its worker leaves
//! its screenshots in its outputs folder, and the real work, started only once they came,
//! gets them as files with the brief to match them.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::work::MockupPart;

const SHOT: &[u8] = b"\x89PNG mockup";

/// The worker's outputs folder, as its brief names it.
fn outputs(turn: &Turn) -> PathBuf {
    let line = turn
        .prompt
        .lines()
        .find_map(|line| {
            line.strip_prefix("Your outputs folder (files for the orchestrator and the user): ")
        })
        .expect("the brief names the outputs folder");
    PathBuf::from(line.trim())
}

#[tokio::test]
async fn the_real_work_starts_after_the_mockups_screenshots_and_gets_them() {
    let script: Script = Arc::new(|turn: Turn| {
        Box::pin(async move {
            if turn.is_orchestrator() {
                if turn.earlier == 0 {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Density toggle mockup",
                                   "kind": "implement", "mockup": true,
                                   "spec": "Mock up a compact density toggle in the sidebar."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    // The real work waits for the mockup's screenshots.
                    let early = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Density toggle",
                                   "kind": "implement", "matches_mockup": "task-1",
                                   "spec": "Add the compact density toggle."}),
                        )
                        .await;
                    assert!(
                        early.is_error && early.text.contains("no mockup images yet"),
                        "{}",
                        early.text
                    );
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[report task-1") {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Density toggle",
                                   "kind": "implement", "matches_mockup": "task-1",
                                   "spec": "Add the compact density toggle."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                return Reply::text("Done.");
            }
            let summary = match turn.task_number() {
                Some(1) => {
                    assert!(
                        turn.prompt.contains("This task builds a mockup"),
                        "{}",
                        turn.prompt
                    );
                    let outputs = outputs(&turn);
                    std::fs::create_dir_all(&outputs).unwrap();
                    std::fs::write(outputs.join("mockup-1.png"), SHOT).unwrap();
                    "Mockup: the toggle sits under the sidebar's header."
                }
                Some(2) => {
                    assert!(
                        turn.prompt.contains("The user approved a mockup"),
                        "{}",
                        turn.prompt
                    );
                    let scratch = outputs(&turn).parent().unwrap().to_path_buf();
                    let given = std::fs::read(scratch.join("attachments").join("mockup-1.png"))
                        .expect("the mockup's screenshot is among its files");
                    assert_eq!(given, SHOT);
                    "Built to match the mockup."
                }
                _ => return Reply::text("Done."),
            };
            let reply = turn
                .call("submit_report", json!({"summary": summary}))
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        })
    });
    let flow = Flow::start("mockup-first", Options::default(), script).await;
    flow.say("Add a compact density toggle to the sidebar.")
        .await;
    let board = flow.settled().await;
    let mockup = Flow::task(&board, 1);
    assert_eq!(mockup.mockup, Some(MockupPart::Builds));
    assert!(
        mockup
            .outputs
            .iter()
            .any(|output| output.file_name.as_deref() == Some("mockup-1.png")),
        "{:?}",
        mockup.outputs
    );
    let real = Flow::task(&board, 2);
    assert_eq!(
        real.mockup,
        Some(MockupPart::Matches {
            task: mockup.id.clone()
        })
    );
    let names: Vec<_> = real.attachments.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["mockup-1.png"]);
    flow.stop().await;
}
