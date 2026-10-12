//! Settings › Browser: with Brigadier's built-in browser off, the thread and its workers are
//! told it doesn't exist; a switch reaches a running thread once, before its next turn, and a
//! worker at work at once.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::model::Settings;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

async fn edit_settings(flow: &Flow, edit: impl FnOnce(&mut Settings)) {
    let mut settings = flow.core.settings();
    edit(&mut settings);
    flow.core.update_settings(settings).await.unwrap();
}

const OFF: &str = "Brigadier's built-in browser is off";

#[tokio::test]
async fn off_reaches_the_thread_and_its_workers_and_a_switch_reaches_a_running_thread() {
    let inputs = Arc::new(Mutex::new(Vec::<String>::new()));
    let briefs = Arc::new(Mutex::new(Vec::<String>::new()));
    let (seen, told) = (inputs.clone(), briefs.clone());
    let flow = Flow::start(
        "browser-off-agents",
        Options::default(),
        script(move |turn| {
            let (seen, told) = (seen.clone(), told.clone());
            async move {
                if turn.is_orchestrator() {
                    seen.lock().unwrap().push(turn.input.clone());
                    if turn.input.contains("Add a greeting.") {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "medium", "title": "Add a greeting",
                                       "kind": "implement", "spec": "Create hello.txt.",
                                       "provider": "claude"}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                    }
                    return Reply::text("[quiet]");
                }
                if turn.task_number().is_some() && turn.earlier == 0 {
                    told.lock().unwrap().push(turn.input.clone());
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "Nothing to change.", "changes": []}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    edit_settings(&flow, |settings| settings.browser_enabled = false).await;
    flow.say("Hi.").await;
    flow.settled().await;
    let prompt = flow.thread_specs()[0]
        .1
        .append_system_prompt
        .clone()
        .unwrap_or_default();
    assert!(prompt.contains(OFF), "{prompt}");

    flow.say("Add a greeting.").await;
    flow.settled().await;
    let briefs = briefs.lock().unwrap().clone();
    assert_eq!(briefs.len(), 1, "{briefs:#?}");
    assert!(briefs[0].contains(OFF), "{}", briefs[0]);

    // Back on: the running thread hears it once, with its next message.
    edit_settings(&flow, |settings| settings.browser_enabled = true).await;
    flow.say("And now?").await;
    flow.settled().await;
    flow.say("Once more.").await;
    flow.settled().await;
    let inputs = inputs.lock().unwrap().clone();
    let heard: Vec<_> = inputs
        .iter()
        .filter(|input| input.contains("[settings] The user turned Brigadier's built-in browser"))
        .collect();
    assert_eq!(heard.len(), 1, "{inputs:#?}");
    assert!(heard[0].contains("And now?"), "{}", heard[0]);
    assert!(heard[0].contains("back on"), "{}", heard[0]);
    flow.stop().await;
}

/// On, as by default, the instructions say nothing of the browser.
#[tokio::test]
async fn on_leaves_the_instructions_as_they_were() {
    let flow = Flow::start(
        "browser-on-agents",
        Options::default(),
        script(|_turn| async { Reply::text("[quiet]") }),
    )
    .await;
    flow.say("Hi.").await;
    flow.settled().await;
    let prompt = flow.thread_specs()[0]
        .1
        .append_system_prompt
        .clone()
        .unwrap_or_default();
    assert!(
        !prompt.to_lowercase().contains("built-in browser"),
        "{prompt}"
    );
    flow.stop().await;
}

/// A worker at work hears the browser turned off at once, and only once.
#[tokio::test]
async fn a_working_worker_hears_the_browser_turned_off_at_once() {
    let steered = Arc::new(Mutex::new(Vec::<String>::new()));
    let heard = steered.clone();
    let flow = Flow::start(
        "browser-off-worker",
        Options::default(),
        script(move |turn| {
            let heard = heard.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("Add a greeting.") {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "medium", "title": "Add a greeting",
                                       "kind": "implement", "spec": "Create hello.txt.",
                                       "provider": "claude"}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                    }
                    return Reply::text("[quiet]");
                }
                if turn.task_number().is_some() && turn.earlier == 0 {
                    assert!(!turn.input.contains(OFF), "{}", turn.input);
                    let note = turn.steered().await.expect("told of the switch");
                    heard.lock().unwrap().push(note);
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "Nothing to change.", "changes": []}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.say("Add a greeting.").await;
    flow.until("the lead to work", |board| {
        board
            .tasks
            .values()
            .any(|task| task.state == crate::work::TaskState::Running)
    })
    .await;
    // Its turn has started: what it was told is its brief.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    edit_settings(&flow, |settings| settings.browser_enabled = false).await;
    flow.manager.browser_switched().await;
    // A second call tells it nothing new.
    flow.manager.browser_switched().await;
    flow.settled().await;
    let notes = steered.lock().unwrap().clone();
    assert_eq!(notes.len(), 1, "{notes:#?}");
    assert!(
        notes[0].contains("[settings] The user turned Brigadier's built-in browser off."),
        "{}",
        notes[0]
    );
    flow.stop().await;
}
