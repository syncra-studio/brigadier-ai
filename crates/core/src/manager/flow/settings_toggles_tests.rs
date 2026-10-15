//! Settings › Configuration's behaviour toggles (BUILD-PLAN B4, §1.4): "Start workers on its
//! own" reaches a running thread once, and a user's old Short replies off is kept once as their
//! preference for longer answers, since the voice is fixed.

use std::sync::{Arc, Mutex};

use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::brains::LONGER_ANSWERS;
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

/// The thread's inputs, in order.
fn recording(inputs: &Arc<Mutex<Vec<String>>>) -> Script {
    let seen = inputs.clone();
    script(move |turn| {
        let seen = seen.clone();
        async move {
            if turn.is_orchestrator() {
                seen.lock().unwrap().push(turn.input.clone());
            }
            Reply::text("Done.")
        }
    })
}

async fn say(flow: &Flow, inputs: &Arc<Mutex<Vec<String>>>, text: &str) -> String {
    let before = inputs.lock().unwrap().len();
    flow.say(text).await;
    flow.until(text, |_| inputs.lock().unwrap().len() > before)
        .await;
    flow.settled().await;
    inputs.lock().unwrap()[before].clone()
}

const OFF_NOTE: &str = "[settings] The user turned \"Start workers on its own\" off.";

#[tokio::test]
async fn start_workers_switched_reaches_a_running_thread_once() {
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let flow = Flow::start("start-workers", Options::default(), recording(&inputs)).await;
    let first = say(&flow, &inputs, "First.").await;
    assert!(!first.contains("Start workers on its own"));
    let prompt = flow.thread_specs()[0]
        .1
        .append_system_prompt
        .clone()
        .unwrap();
    assert!(prompt.contains("say so in one line first"), "on by default");
    edit_settings(&flow, |settings| settings.start_workers_on_own = false).await;
    let second = say(&flow, &inputs, "Second.").await;
    assert_eq!(second.matches(OFF_NOTE).count(), 1, "{second}");
    assert!(second.contains("ask first with"));
    let third = say(&flow, &inputs, "Third.").await;
    assert!(!third.contains("Start workers on its own"), "told once");
    flow.stop().await;
}

/// A stored Short replies off becomes the preference "Longer answers", once, and reaches the
/// running thread as its preferences; the setting is saved as on, so no restart copies it again.
#[tokio::test]
async fn short_replies_off_is_kept_once_as_the_preference_for_longer_answers() {
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let mut flow = Flow::start("longer-answers", Options::default(), recording(&inputs)).await;
    let first = say(&flow, &inputs, "First.").await;
    assert!(!first.contains(LONGER_ANSWERS));
    edit_settings(&flow, |settings| settings.short_replies = false).await;
    flow.restart().await;
    let kept = |memories: Vec<brigadier_brain::Node>| {
        memories
            .iter()
            .filter(|node| node.title == LONGER_ANSWERS)
            .count()
    };
    assert_eq!(kept(flow.manager.list_memories().await.unwrap()), 1);
    assert!(flow.core.settings().short_replies, "copied once");
    let second = say(&flow, &inputs, "Second.").await;
    assert!(second.contains(LONGER_ANSWERS), "{second}");
    assert!(
        !second.contains("no longer has a Short replies setting"),
        "a thread started with the fixed voice hears nothing of it"
    );
    flow.restart().await;
    assert_eq!(kept(flow.manager.list_memories().await.unwrap()), 1);
    flow.stop().await;
}
