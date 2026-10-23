//! Settings › Browser: with Brigadier's built-in browser off, the thread and its workers are
//! told it doesn't exist. A switch tells a running thread nothing: it applies when its next
//! fresh CLI conversation starts.

use std::sync::Arc;

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
