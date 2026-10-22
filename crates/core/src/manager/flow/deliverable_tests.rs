//! Deliverables survive their task (BUILD-PLAN A1): a scout's output linked by its scratch
//! path, in its report, in what it wrote after the report and in the thread's answer, still
//! opens Brigadier's stored copy once the task ended and its scratch folder went, and the
//! addendum says the folder is gone.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::model::MessageRole;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

const LIST: &str = "# Rounding\n\n- round_cents (src/money.ts:12)\n- to_minor (src/money.ts:30)\n";

/// The worker's outputs folder, as its brief names it.
fn outputs_folder(turn: &Turn) -> PathBuf {
    const SAYS: &str = "Your outputs folder (files for the thread and the user): ";
    let text = format!("{}\n{}", turn.prompt, turn.input);
    let rest = &text[text.find(SAYS).expect("the brief names the outputs folder") + SAYS.len()..];
    PathBuf::from(rest.lines().next().unwrap().trim())
}

/// The first path ending in `/rounding.md` that `text` links, as a model writes it.
fn link_in(text: &str) -> String {
    let end = text.find("/rounding.md").expect("a link to rounding.md") + "/rounding.md".len();
    let start = text[..end]
        .rfind(|c: char| c.is_whitespace() || matches!(c, '(' | '`' | '['))
        .map_or(0, |i| i + 1);
    text[start..end].to_owned()
}

#[tokio::test]
async fn a_scouts_output_linked_by_its_scratch_path_opens_the_stored_copy_after_the_task_ended() {
    let thread_inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let worker_link: Arc<Mutex<Option<String>>> = Arc::default();
    let worker_brief: Arc<Mutex<Option<String>>> = Arc::default();
    let (heard, written, brief) = (
        thread_inputs.clone(),
        worker_link.clone(),
        worker_brief.clone(),
    );
    let flow = Flow::start(
        "deliverable-links",
        Options::default(),
        script(move |turn| {
            let (heard, written, briefed) = (heard.clone(), written.clone(), brief.clone());
            async move {
                if turn.is_orchestrator() {
                    heard.lock().unwrap().push(turn.input.clone());
                    if turn.input.contains("List the rounding functions") {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "low", "title": "List rounding functions",
                                       "kind": "scout", "spec": "List them in outputs/rounding.md."}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("");
                    }
                    if turn.input.contains("[report task-1") && !turn.input.contains("· addendum]") {
                        let link = link_in(&turn.input);
                        return Reply::text(format!("The list is in [rounding.md]({link})."));
                    }
                    return Reply::text("");
                }
                let outputs = outputs_folder(&turn);
                *briefed.lock().unwrap() = Some(format!("{}\n{}", turn.prompt, turn.input));
                std::fs::create_dir_all(&outputs).unwrap();
                let file = outputs.join("rounding.md");
                std::fs::write(&file, LIST).unwrap();
                let link = file.display().to_string();
                *written.lock().unwrap() = Some(link.clone());
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": format!("Listed the rounding functions in {link}; the details are below.")}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text(format!(
                    "Details: {link} lists two functions. round_cents rounds half up; to_minor truncates."
                ))
            }
        }),
    )
    .await;
    flow.say("List the rounding functions.").await;
    let board = flow
        .until("the scout to end", |board| {
            board
                .tasks
                .values()
                .any(|task| task.number == 1 && task.state.is_final())
        })
        .await;
    let task = Flow::task(&board, 1).clone();
    let scratch = PathBuf::from(&task.workspace.as_ref().unwrap().scratch);
    flow.settled().await;
    flow.until("the scratch folder to go", |_| !scratch.exists())
        .await;

    // Where the brief gives the worker its outputs folder's path, it says how to link a file
    // there instead (issue #42).
    let brief = worker_brief.lock().unwrap().clone().unwrap();
    let named = brief
        .split("Your outputs folder (files for the thread and the user): ")
        .nth(1)
        .and_then(|rest| rest.lines().nth(1))
        .unwrap_or_default();
    assert_eq!(
        named,
        "Link a file there by its name as outputs/<name> (outputs/notes.md), never by that folder's path, in your report and messages too.",
        "{brief}"
    );
    let worker_link = worker_link.lock().unwrap().clone().unwrap();
    let inputs = thread_inputs.lock().unwrap().clone();
    let addendum = inputs
        .iter()
        .find_map(|input| {
            input
                .split_once("· addendum]")
                .map(|(_, rest)| rest.to_owned())
        })
        .expect("the thread got the addendum");
    assert!(
        addendum.contains("Its scratch folder is gone, so paths in it no longer open"),
        "{addendum}"
    );
    // The report tells the thread how to link an output without its scratch path.
    let report = inputs
        .iter()
        .find(|input| input.contains("[report task-1") && !input.contains("· addendum]"))
        .unwrap();
    assert!(
        report.contains(
            "link one once, inline where your answer speaks of it, as [notes.md](outputs/notes.md), which keeps opening Brigadier's copy after its scratch folder goes, never by its scratch path"
        ),
        "{report}"
    );
    let answer = flow
        .core
        .all_messages(&flow.conversation)
        .await
        .unwrap()
        .into_iter()
        .rev()
        .find(|message| {
            message.role == MessageRole::Assistant && message.text.contains("rounding.md")
        })
        .expect("the thread's answer links the list")
        .text;
    for (whose, link) in [
        ("the worker's report", worker_link.clone()),
        ("the addendum", link_in(&addendum)),
        ("the thread's answer", link_in(&answer)),
    ] {
        assert!(
            link.contains(&*scratch.display().to_string()),
            "{whose}: {link}"
        );
        assert!(!PathBuf::from(&link).exists(), "{whose}: {link} is gone");
        let stored = flow
            .manager
            .stored_output(&flow.conversation, &link, None)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("{whose}: {link} has no stored copy"));
        assert_eq!(stored.task_id, task.id, "{whose}");
        let copy = flow
            .manager
            .artifact_copy(stored.artifact.id.clone(), "rounding.md".into())
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(copy).unwrap(), LIST, "{whose}");
    }
    // The link form the report teaches opens the same copy.
    let stored = flow
        .manager
        .stored_output(&flow.conversation, "outputs/rounding.md", None)
        .await
        .unwrap()
        .expect("outputs/rounding.md has a stored copy");
    assert_eq!(stored.task_id, task.id);
    flow.stop().await;
}

/// The thread fixes a defect in an ended scout's output (issue #27): store_output keeps its
/// corrected copy as that task's output in place of the worker's, so `outputs/rounding.md`
/// opens the fixed file after the scratch folder went, and the card lists it once.
#[tokio::test]
async fn the_threads_corrected_copy_of_an_ended_tasks_output_is_kept_in_its_place() {
    const FIXED: &str =
        "# Rounding\n\n- round_cents (src/money.ts:14)\n- to_minor (src/money.ts:30)\n";
    let stored_reply: Arc<Mutex<Option<String>>> = Arc::default();
    let reply_log = stored_reply.clone();
    let flow = Flow::start(
        "deliverable-corrected",
        Options::default(),
        script(move |turn| {
            let reply_log = reply_log.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("List the rounding functions") {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "low", "title": "List rounding functions",
                                       "kind": "scout", "spec": "List them in outputs/rounding.md."}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("");
                    }
                    if let Some(private) = turn.input.lines().find_map(|line| line.strip_prefix("Keep ")) {
                        let reply = turn
                            .call("store_output", json!({"task": "task-1", "path": private.trim()}))
                            .await;
                        *reply_log.lock().unwrap() = Some(format!("{}{}", reply.is_error, reply.text));
                        return Reply::text("No.");
                    }
                    if let Some(fixed) = turn.input.lines().find_map(|line| line.strip_prefix("Fix it in ")) {
                        let fixed = fixed.trim().to_owned();
                        std::fs::write(&fixed, FIXED).unwrap();
                        let reply = turn
                            .call("store_output", json!({"task": "task-1", "path": fixed}))
                            .await;
                        *reply_log.lock().unwrap() = Some(format!("{}{}", reply.is_error, reply.text));
                        return Reply::text("Fixed the line number: [rounding.md](outputs/rounding.md).");
                    }
                    return Reply::text("");
                }
                let outputs = outputs_folder(&turn);
                std::fs::create_dir_all(&outputs).unwrap();
                std::fs::write(outputs.join("rounding.md"), LIST).unwrap();
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Listed the rounding functions in outputs/rounding.md."}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.say("List the rounding functions.").await;
    let board = flow
        .until("the scout to end", |board| {
            board
                .tasks
                .values()
                .any(|task| task.number == 1 && task.state.is_final())
        })
        .await;
    let task = Flow::task(&board, 1).clone();
    let scratch = PathBuf::from(&task.workspace.as_ref().unwrap().scratch);
    flow.settled().await;
    flow.until("the scratch folder to go", |_| !scratch.exists())
        .await;
    let fixed_dir = flow.repo.parent().unwrap().join("thread-fix");
    std::fs::create_dir_all(&fixed_dir).unwrap();
    let fixed = fixed_dir.join("rounding.md");
    flow.say(&format!("Line 12 is wrong.\nFix it in {}", fixed.display()))
        .await;
    flow.settled().await;

    let reply = stored_reply
        .lock()
        .unwrap()
        .clone()
        .expect("store_output ran");
    assert!(
        reply.starts_with("false")
            && reply.contains("Kept rounding.md as task-1's output, in place of the worker's")
            && reply.contains("[rounding.md](outputs/rounding.md)"),
        "{reply}"
    );
    let board = flow.core.board(&flow.conversation).await.unwrap();
    let outputs = &Flow::task(&board, 1).outputs;
    let named: Vec<_> = outputs
        .iter()
        .filter(|output| output.file_name.as_deref() == Some("rounding.md"))
        .collect();
    assert_eq!(named.len(), 1, "{outputs:#?}");
    let stored = flow
        .manager
        .stored_output(&flow.conversation, "outputs/rounding.md", None)
        .await
        .unwrap()
        .expect("outputs/rounding.md has a stored copy");
    assert_eq!(stored.task_id, task.id);
    assert_eq!(stored.artifact.id, named[0].id);
    let copy = flow
        .manager
        .artifact_copy(stored.artifact.id.clone(), "rounding.md".into())
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(copy).unwrap(), FIXED);

    // Brigadier's private folder is never read into an output, as the sandbox never reads it.
    let run_dir = flow.manager.runtime.platform().paths().run_dir.clone();
    std::fs::create_dir_all(&run_dir).unwrap();
    let token = run_dir.join("secret.token");
    std::fs::write(&token, "s3cret").unwrap();
    flow.say(&format!("Keep {}", token.display())).await;
    flow.settled().await;
    let refused = stored_reply.lock().unwrap().clone().unwrap();
    assert!(
        refused.starts_with("true") && refused.contains("Brigadier's private folder"),
        "{refused}"
    );
    let board = flow.core.board(&flow.conversation).await.unwrap();
    assert_eq!(Flow::task(&board, 1).outputs, *outputs);
    flow.stop().await;
}
