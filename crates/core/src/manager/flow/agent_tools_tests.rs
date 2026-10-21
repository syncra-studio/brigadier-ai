//! The agents' own tools for the work that has a component of its own (thread-truth grill,
//! 10-21): the session's headline, the checks and the review. Brigadier starts none of that
//! work by itself; it stores what the tools say and sends it as events.

use std::sync::{Arc, Mutex};

use brigadier_providers::model::Origin;
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::prompts::{REVISIONS, Revision};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// An open thread whose CLI was told the revision before `number` resumes its same session on
/// its next turn and hears that revision's note once. Returns the note's turn input.
async fn an_open_thread_hears_once(name: &str, number: u32) -> (&'static Revision, String) {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let mut flow = Flow::start(
        name,
        Options::default(),
        Arc::new(move |turn| {
            log.lock().unwrap().push(turn.input.clone());
            Box::pin(async { Reply::text("Done.") })
        }),
    )
    .await;
    flow.say("First.").await;
    flow.settled().await;
    let added = REVISIONS
        .iter()
        .find(|revision| revision.number == number)
        .unwrap();
    // What the build before logged: its instructions were one revision older.
    flow.core
        .record(vec![(
            crate::model::streams::orchestrator(&flow.conversation),
            crate::model::DomainEvent::OrchestratorLogged {
                conversation_id: flow.conversation.clone(),
                entry: crate::work::OrchestratorEntry::Injection {
                    injection: crate::work::ContextInjection {
                        kind: crate::work::InjectionKind::Instructions,
                        bytes: 100,
                        tokens_estimate: 25,
                        label: "instructions revision".into(),
                        task_id: None,
                        told: Some(crate::work::Told {
                            revision: Some(number - 1),
                            ..crate::work::Told::default()
                        }),
                    },
                },
            },
        )])
        .await
        .unwrap();
    flow.restart().await;
    flow.say("Second.").await;
    flow.until("the second turn", |_| inputs.lock().unwrap().len() >= 2)
        .await;
    flow.settled().await;
    let specs = flow.thread_specs();
    assert!(
        matches!(specs.last().unwrap().1.origin, Origin::Resume { .. }),
        "{specs:#?}"
    );
    let second = inputs.lock().unwrap()[1].clone();
    assert!(
        second.contains(&format!("[instructions] {}", added.thread)),
        "{second}"
    );
    flow.say("Third.").await;
    flow.until("the third turn", |_| inputs.lock().unwrap().len() >= 3)
        .await;
    flow.settled().await;
    let third = inputs.lock().unwrap()[2].clone();
    assert!(!third.contains("[instructions]"), "{third}");
    flow.stop().await;
    (added, second)
}

/// `set_headline` stores the session's headline and detail line and sends them as an event;
/// an empty one is refused, and the user's next message clears it.
#[tokio::test]
async fn the_threads_headline_stays_until_the_users_next_message() {
    let replies: Arc<Mutex<Vec<(bool, String)>>> = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        "thread-headline",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.is_orchestrator() && turn.input.contains("Start the server") {
                    for arguments in [
                        json!({ "headline": "  " }),
                        json!({
                            "headline": "Starting   the dev server",
                            "detail": "Next: open the window",
                        }),
                    ] {
                        let reply = turn.call("set_headline", arguments).await;
                        log.lock().unwrap().push((reply.is_error, reply.text));
                    }
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;

    flow.say("Start the server").await;
    let board = flow.settled().await;
    let said = std::mem::take(&mut *replies.lock().unwrap());
    assert!(said[0].0, "an empty headline is refused: {said:?}");
    assert!(!said[1].0, "{said:?}");
    let headline = board.headline.clone().expect("the headline is stored");
    assert_eq!(headline.text, "Starting the dev server");
    assert_eq!(headline.detail.as_deref(), Some("Next: open the window"));
    let events = flow.events().await;
    assert!(
        events.iter().any(|event| matches!(
            event,
            crate::model::DomainEvent::HeadlineSet {
                headline: Some(_),
                ..
            }
        )),
        "sent as an event"
    );

    // The turn ending leaves it; the user's next message clears it.
    flow.say("Thanks").await;
    let board = flow.settled().await;
    assert_eq!(board.headline, None);
    flow.stop().await;
}

/// The headline's rollout: an open thread hears once that it has set_headline.
#[tokio::test]
async fn an_open_thread_hears_once_that_it_sets_the_headline() {
    let (added, _) = an_open_thread_hears_once(
        "headline-revision",
        crate::manager::prompts::HEADLINE_REVISION,
    )
    .await;
    assert!(added.thread.contains("set_headline"));
    assert!(added.worker.is_empty());
}
