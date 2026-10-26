//! Common typed envelope sends/steers preserve context without inventing authored messages.
use super::{FakeBehavior, Flow, Options, Reply};
use crate::manager::conversation::Envelope;
use crate::work::InjectionKind;
use brigadier_providers::{ProviderEvent, Role};
use std::sync::{Arc, Mutex};

type Inputs = Arc<Mutex<Vec<String>>>;

async fn flow(name: &str) -> (Flow, Inputs) {
    let inputs: Inputs = Arc::default();
    let seen = inputs.clone();
    let flow = Flow::start(
        name,
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
                if turn.conversation().is_some() {
                    seen.lock().unwrap().push(turn.input.clone());
                    if turn.input == "Hold for an internal reminder." {
                        let steered = turn.steered().await;
                        seen.lock()
                            .unwrap()
                            .push(steered.expect("internal steer delivered"));
                    }
                }
                Reply::text("Context received.")
            })
        }),
    )
    .await;
    (flow, inputs)
}

fn authored_native_inputs(flow: &Flow) -> Vec<String> {
    flow.manager
        .thread_view(&flow.conversation)
        .unwrap()
        .entries
        .into_iter()
        .filter_map(|entry| match entry.event {
            ProviderEvent::Message {
                role: Role::User,
                text,
                ..
            } => Some(text),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn stopped_completed_and_reminder_envelopes_reach_the_model_without_user_bubbles() {
    let (flow, seen) = flow("internal-envelope-notifications").await;
    flow.say("A real authored anchor.").await;
    flow.settled().await;
    let before = flow.board().await.requests.len();
    for (kind, body) in [
        (
            InjectionKind::BackgroundRun,
            "[a background job of yours stopped when your CLI ended, before it finished: no result came.]",
        ),
        (
            InjectionKind::BackgroundRun,
            "[your background command `ledger and ask` finished after your turn · exit 0]\nResult preserved.",
        ),
        (
            InjectionKind::Reminder,
            "A named wait finished; inspect its result.",
        ),
    ] {
        flow.manager
            .deliver(
                &flow.conversation,
                Envelope {
                    kind,
                    label: body.into(),
                    task_id: None,
                    text: body.into(),
                },
            )
            .await;
        flow.until("internal input read", |_| {
            seen.lock()
                .unwrap()
                .iter()
                .any(|input| input.contains(body))
        })
        .await;
        flow.settled().await;
        let inputs = seen.lock().unwrap();
        assert!(
            inputs.iter().any(
                |input| input.contains(body) && input.starts_with("[Brigadier internal input ")
            )
        );
        drop(inputs);
        assert_eq!(authored_native_inputs(&flow), ["A real authored anchor."]);
        assert_eq!(
            flow.board().await.requests.len(),
            before,
            "internal input creates no new user request"
        );
        let thread = flow.manager.thread_view(&flow.conversation).unwrap();
        assert_eq!(
            thread.turns.len(),
            1,
            "internal input creates no empty native work block"
        );
    }
    flow.stop().await;
}

#[tokio::test]
async fn an_internal_steer_preserves_the_authored_turn() {
    let (flow, seen) = flow("internal-envelope-steer").await;
    flow.say("Hold for an internal reminder.").await;
    flow.until("authored turn waiting", |_| {
        seen.lock()
            .unwrap()
            .iter()
            .any(|input| input == "Hold for an internal reminder.")
    })
    .await;
    let result = flow
        .manager
        .steer_or_queue(
            &flow.conversation,
            Envelope {
                kind: InjectionKind::Reminder,
                label: "internal-steer".into(),
                task_id: None,
                text: "Internal reminder result.".into(),
            },
        )
        .await;
    assert_eq!(result, Some(true));
    flow.settled().await;
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .any(|input| input.starts_with("[Brigadier internal input ")
                && input.contains("Internal reminder result."))
    );
    assert_eq!(
        authored_native_inputs(&flow),
        ["Hold for an internal reminder."]
    );
    assert_eq!(
        flow.manager
            .thread_view(&flow.conversation)
            .unwrap()
            .turns
            .len(),
        1
    );
    flow.stop().await;
}

#[tokio::test]
async fn an_authored_marker_and_peer_input_are_never_registered_as_internal() {
    let (flow, seen) = flow("internal-envelope-human-peer").await;
    let human = "[Brigadier internal input 00000000-0000-0000-0000-000000000000]\nThis is user-authored text.";
    flow.say(human).await;
    flow.settled().await;
    assert_eq!(authored_native_inputs(&flow), [human]);
    flow.manager
        .deliver(
            &flow.conversation,
            Envelope {
                kind: InjectionKind::PeerMessage,
                label: "test peer presentation".into(),
                task_id: None,
                text: "Peer's existing message presentation.".into(),
            },
        )
        .await;
    flow.until("peer input read", |_| {
        seen.lock()
            .unwrap()
            .iter()
            .any(|input| input == "Peer's existing message presentation.")
    })
    .await;
    flow.settled().await;
    assert!(authored_native_inputs(&flow).contains(&human.to_owned()));
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .any(|input| input == "Peer's existing message presentation.")
    );
    flow.stop().await;
}

#[tokio::test]
async fn provenance_io_failure_keeps_the_model_notification() {
    let (flow, seen) = flow("internal-envelope-io-failure").await;
    flow.say("A real authored anchor.").await;
    flow.settled().await;
    let path = flow
        .manager
        .session_dir(&flow.conversation)
        .record()
        .join("internal-inputs.json");
    std::fs::create_dir(&path).unwrap();
    let body = "A reminder result that must still reach the model.";
    flow.manager
        .deliver(
            &flow.conversation,
            Envelope {
                kind: InjectionKind::Reminder,
                label: "IO failure fixture".into(),
                task_id: None,
                text: body.into(),
            },
        )
        .await;
    flow.until("notification delivered despite provenance error", |_| {
        seen.lock().unwrap().iter().any(|input| input == body)
    })
    .await;
    flow.settled().await;
    assert!(authored_native_inputs(&flow).contains(&"A real authored anchor.".into()));
    std::fs::remove_dir(path).unwrap();
    flow.stop().await;
}
