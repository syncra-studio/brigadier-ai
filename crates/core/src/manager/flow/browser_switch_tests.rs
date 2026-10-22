//! Settings › Browser: with Brigadier's built-in browser off, the thread and its workers are
//! told it doesn't exist. A switch tells a running thread nothing: it applies when its next
//! fresh CLI conversation starts.

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
async fn off_reaches_the_thread_and_its_workers_and_a_switch_applies_at_the_next_fresh_conversation()
 {
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
                    return Reply::text("");
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

    // Back on: the running thread hears nothing of it, and its CLI goes on as it started.
    edit_settings(&flow, |settings| settings.browser_enabled = true).await;
    let started = flow.thread_specs().len();
    flow.say("And now?").await;
    flow.settled().await;
    let heard = inputs.lock().unwrap().clone();
    assert!(
        heard.iter().all(|input| !input.contains("[settings]")),
        "{heard:#?}"
    );
    let specs = flow.thread_specs();
    assert!(
        specs[started..].iter().all(|(_, spec)| spec
            .append_system_prompt
            .as_deref()
            .unwrap_or_default()
            .contains(OFF)),
        "a resumed CLI keeps the instructions it started with"
    );
    // The next fresh conversation starts on the setting as it is now.
    flow.manager
        .conv(&flow.conversation)
        .unwrap()
        .close_cli()
        .await;
    flow.manager.forget_native_session(&flow.conversation).await;
    flow.say("Once more.").await;
    flow.settled().await;
    let (_, fresh) = flow.thread_specs().pop().expect("a fresh CLI");
    assert!(
        !fresh.append_system_prompt.unwrap_or_default().contains(OFF),
        "the fresh conversation's instructions"
    );
    flow.stop().await;
}

/// On, as by default, the instructions say nothing of the browser.
#[tokio::test]
async fn on_leaves_the_instructions_as_they_were() {
    let flow = Flow::start(
        "browser-on-agents",
        Options::default(),
        script(|_turn| async { Reply::text("") }),
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
