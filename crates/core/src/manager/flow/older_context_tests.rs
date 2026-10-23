//! A reborn thread is told where to look up what its briefing doesn't carry (issue #112).

use std::sync::{Arc, Mutex};

use brigadier_providers::ProviderKind;

use super::{Flow, Options, Reply, Script, Turn};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// Past the 300k prepare and 375k swap lines of a 1M window.
const FULL: i64 = 460_000;

async fn rebirths(flow: &Flow) -> usize {
    flow.core
        .list_orchestrator_log(&flow.conversation, None, brigadier_store::MAX_PAGE)
        .await
        .unwrap()
        .entries
        .iter()
        .filter(|entry| matches!(entry.entry, crate::work::OrchestratorEntry::Rebirth { .. }))
        .count()
}

/// A reborn Codex thread is told to look up what its briefing doesn't carry before it answers
/// "unknown" (issue #112): nothing but the briefing tells a fresh session it can.
#[tokio::test]
async fn a_reborn_codex_thread_is_told_to_look_up_before_it_answers_unknown() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let flow = Flow::start(
        "transcript-codex-lookup",
        Options {
            thread: ProviderKind::Codex,
            ..Options::default()
        },
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.input.contains("Write the handoff note") {
                    return Reply::text("Goal and open threads: none.\nWhat to do next: wait.");
                }
                log.lock().unwrap().push(turn.input.clone());
                if turn.input.contains("Grow.") && !turn.input.contains("[Brigadier briefing") {
                    return Reply {
                        text: "Grown.".into(),
                        context_tokens: Some(FULL),
                        ..Reply::default()
                    };
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("Grow.").await;
    flow.settled().await;
    flow.say("After the rebirth.").await;
    flow.settled().await;
    assert_eq!(rebirths(&flow).await, 1, "reborn once");
    let inputs = inputs.lock().unwrap().clone();
    let briefing = inputs
        .iter()
        .find(|input| input.contains("[Brigadier briefing"))
        .expect("the reborn session's briefing");
    let older = briefing
        .split("[Older context]")
        .nth(1)
        .expect("what the briefing says of older context");
    assert!(
        older.contains("before you answer that something from it is unknown")
            && older.contains("look it up")
            && older.contains("`brigadier transcript search`")
            && older.contains("`brigadier brain search`"),
        "{briefing}"
    );
    flow.stop().await;
}
