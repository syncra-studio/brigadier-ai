//! A mockup first (SESSION-FIXES-PLAN.md Q10): the thread delegates a mockup, its worker leaves
//! its screenshots in its outputs folder, the user's marked notes reach that same worker as
//! files, and the real work, started only once screenshots came, gets them with the brief to
//! match them.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::work::MockupPart;

const SHOT: &[u8] = b"\x89PNG mockup";
const REVISED: &[u8] = b"\x89PNG mockup, revised";
const MARKED: &[u8] = b"\x89PNG marked";

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

fn real_work(matches: &str) -> serde_json::Value {
    json!({"effort": "medium", "title": "Density toggle", "kind": "implement",
           "matches_mockup": matches, "spec": "Add the compact density toggle."})
}

#[tokio::test]
async fn notes_reach_the_mockups_worker_and_the_real_work_gets_its_screenshots() {
    // The marked screenshot the user sends with their notes.
    let marked: Arc<Mutex<String>> = Arc::default();
    let reports = Arc::new(Mutex::new(0));
    let script: Script = {
        let (marked, reports) = (marked.clone(), reports.clone());
        Arc::new(move |turn: Turn| {
            let (marked, reports) = (marked.clone(), reports.clone());
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
                        let early = turn.call("delegate_task", real_work("task-1")).await;
                        assert!(
                            early.is_error && early.text.contains("no mockup images yet"),
                            "{}",
                            early.text
                        );
                        return Reply::text("[quiet]");
                    }
                    if turn.input.contains("Notes on the mockup") {
                        let id = marked.lock().unwrap().clone();
                        let reply = turn
                            .call(
                                "message_worker",
                                json!({"task": "task-1", "attachments": [id],
                                       "text": "The user's note 1: move the toggle up."}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("[quiet]");
                    }
                    if turn.input.contains("[report task-1") {
                        let second = {
                            let mut reports = reports.lock().unwrap();
                            *reports += 1;
                            *reports == 2
                        };
                        if second {
                            let reply = turn.call("delegate_task", real_work("task-1")).await;
                            assert!(!reply.is_error, "{}", reply.text);
                            return Reply::text("[quiet]");
                        }
                        return Reply::text("Here is the mockup.");
                    }
                    return Reply::text("Done.");
                }
                let summary = match turn.task_number() {
                    Some(1) if turn.input.contains("The user's note 1") => {
                        let file = turn
                            .input
                            .split("Files with this message (open each one): ")
                            .nth(1)
                            .expect("the message names its files")
                            .trim();
                        assert_eq!(std::fs::read(file).unwrap(), MARKED, "{file}");
                        std::fs::write(outputs(&turn).join("mockup-1.png"), REVISED).unwrap();
                        "Mockup revised: the toggle moved up."
                    }
                    Some(1) => {
                        assert!(
                            turn.prompt.contains("This task builds a mockup"),
                            "{}",
                            turn.prompt
                        );
                        let outputs = outputs(&turn);
                        std::fs::create_dir_all(&outputs).unwrap();
                        std::fs::write(outputs.join("mockup-1.png"), SHOT).unwrap();
                        // Its preview page: a change that waits to land, so the task stays.
                        std::fs::write(turn.cwd.join("mockup.html"), "<main></main>").unwrap();
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
                        assert_eq!(given, REVISED, "the newest screenshot");
                        "Built to match the mockup."
                    }
                    _ => return Reply::text("Done."),
                };
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": summary, "changes": ["mockup.html: the preview page"]}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            })
        })
    };
    let flow = Flow::start("mockup-first", Options::default(), script).await;
    flow.say("Add a compact density toggle to the sidebar.")
        .await;
    flow.settled().await;

    let attachment = flow
        .core
        .add_attachment(
            "mockup-1-notes.png".into(),
            "image/png".into(),
            MARKED.to_vec(),
            false,
        )
        .await
        .unwrap();
    *marked.lock().unwrap() = attachment.id.clone();
    flow.manager
        .send_message(
            flow.conversation.clone(),
            "Notes on the mockup:\n1. Move the toggle up.".into(),
            vec![attachment],
            Vec::new(),
            false,
            None,
        )
        .await
        .unwrap();
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
