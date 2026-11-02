//! The drift note (D167): a block that took more `brigadier` look-ups than one `brigadier state`
//! call gets one plain note on its next turn, once a block; the thread shows only the user's
//! words, and nothing is refused.
use std::sync::{Arc, Mutex};

use brigadier_cli::protocol::CliRequest;
use brigadier_providers::session_file::Said;

use super::cli_tests::brigadier;
use super::{FakeBehavior, Flow, Options, Reply};

#[tokio::test]
async fn a_block_of_many_look_ups_gets_one_note_on_its_next_turn() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = inputs.clone();
    let flow = Flow::start(
        "drift-note",
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
        Arc::new(move |turn| {
            let seen = seen.clone();
            Box::pin(async move {
                if turn.is_orchestrator() {
                    seen.lock().unwrap().push(turn.input.clone());
                    let look_ups = if turn.input.contains("Look around.") {
                        4
                    } else {
                        3
                    };
                    for _ in 0..look_ups {
                        brigadier(&turn, CliRequest::Workers).await;
                    }
                }
                Reply::text("Done.")
            })
        }),
    )
    .await;
    let noted = |input: &String| input.contains("Brigadier note: your last request took 4");

    flow.say("Look around.").await;
    flow.settled().await;
    flow.say("Go on.").await;
    flow.settled().await;
    flow.say("And again.").await;
    flow.settled().await;

    let inputs = inputs.lock().unwrap().clone();
    assert_eq!(inputs.len(), 3, "{inputs:?}");
    assert!(!noted(&inputs[0]), "{inputs:?}");
    assert!(
        noted(&inputs[1]) && inputs[1].contains("`brigadier state`"),
        "{inputs:?}"
    );
    // Three look-ups are fine: no note.
    assert!(!inputs[2].contains("Brigadier note"), "{inputs:?}");
    assert_eq!(
        flow.said_in_thread("Go on."),
        [Some(Said::Both {
            user: "Go on.".into()
        })]
    );
    flow.stop().await;
}
