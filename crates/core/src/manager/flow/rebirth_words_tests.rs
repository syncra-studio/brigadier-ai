//! A rebirth briefing ranks what it carries by how hard it is to look up again (issue #216):
//! past the verbatim exchanges, the user's own messages of older ones are carried word for
//! word without the replies, which `brigadier transcript search` still finds.

use std::sync::{Arc, Mutex};

use super::{Flow, Options, Reply, Script, Turn};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// Thirty exchanges whose replies are long: they don't all fit the briefing verbatim, yet every
/// one of the user's messages is carried word for word, the oldest ones without their replies.
#[tokio::test]
async fn a_rebirth_carries_the_users_older_words_without_the_replies() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let flow = Flow::start(
        "rebirth-words",
        Options::default(),
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
                        context_tokens: Some(460_000),
                        ..Reply::default()
                    };
                }
                let n = turn
                    .input
                    .split("Plant ")
                    .nth(1)
                    .and_then(|rest| rest.split(':').next())
                    .unwrap_or("x")
                    .to_owned();
                Reply::text(format!(
                    "Reply {n}: {}",
                    "the output of a long read. ".repeat(160)
                ))
            }
        }),
    )
    .await;
    for n in 0..30 {
        flow.say(&format!("Plant {n}: keep the code K-{n:03} as it is."))
            .await;
        flow.settled().await;
    }
    flow.say("Grow.").await;
    flow.settled().await;
    flow.say("After.").await;
    flow.settled().await;

    let inputs = inputs.lock().unwrap().clone();
    let briefing = inputs
        .iter()
        .find(|input| input.contains("[Brigadier briefing"))
        .expect("a reborn session's briefing");
    let words = briefing
        .split("[The user's earlier messages, verbatim;")
        .nth(1)
        .and_then(|rest| rest.split("\n\n").next())
        .expect("the user's earlier words");
    let recent = briefing
        .split("[The conversation's latest messages, verbatim]")
        .nth(1)
        .expect("the verbatim tail");
    // Every plant is carried word for word, in one section or the other.
    for n in 0..30 {
        let plant = format!("Plant {n}: keep the code K-{n:03} as it is.");
        assert!(
            words.contains(&plant) || recent.contains(&plant),
            "plant {n} lost:\n{briefing}"
        );
    }
    // The oldest exchange's reply isn't carried; its user's message is, with its number.
    assert!(words.contains("): Plant 0: keep the code K-000"), "{words}");
    assert!(words.lines().all(|line| !line.starts_with("You:")));
    assert!(!briefing.contains("Reply 0:"), "{briefing}");
    assert!(
        recent.contains(
            "aren't carried here; `brigadier transcript search` finds them. The user's own words among them are above"
        ),
        "{recent}"
    );
    // The briefing stays within its target.
    assert!(briefing.len() <= 20_000 * 4, "{}", briefing.len());
    flow.stop().await;
}
