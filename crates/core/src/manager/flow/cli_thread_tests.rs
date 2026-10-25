//! The thread read from the CLI's own session file (S9, G11): what a session showed before
//! the daemon restarts is what it shows after, turn ends included.

use std::sync::Arc;

use super::{FakeBehavior, Flow, Options, Reply, Script, Turn, eventually};
use crate::thread_view::{ThreadView, TurnEnd};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// Four turns end as done, stopped, failed and interrupted (the CLI killed mid-turn); the
/// daemon restarts; the thread reloaded from the session file and `record/turns.jsonl` has the
/// same items and turn ends.
#[tokio::test]
async fn a_restart_keeps_the_thread_and_how_each_turn_ended() {
    let running = Arc::new(tokio::sync::Notify::new());
    let ran = running.clone();
    let mut flow = Flow::start(
        "cli-thread-restart",
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
        script(move |turn| {
            let ran = ran.clone();
            async move {
                if turn.input.contains("Wait") {
                    ran.notify_one();
                    assert!(turn.stopped().await, "the user stops it");
                    return Reply::text("");
                }
                if turn.input.contains("Fail") {
                    return Reply::failed("The API refused it.");
                }
                if turn.input.contains("Crash") {
                    return Reply::crashed(9);
                }
                Reply::text("Looked.")
            }
        }),
    )
    .await;
    let id = flow.conversation.clone();
    let ended = |flow: &Flow, count: usize| {
        let view = flow.manager.thread_view(&flow.conversation).unwrap();
        view.turns.len() == count && view.turns.iter().all(|turn| turn.end.is_some())
    };

    flow.say("Look around.").await;
    eventually("the first turn ends", || ended(&flow, 1)).await;
    flow.settled().await;
    flow.say("Wait for my stop.").await;
    running.notified().await;
    flow.manager.interrupt(id.clone()).await.unwrap();
    eventually("the stopped turn ends", || ended(&flow, 2)).await;
    flow.settled().await;
    flow.say("Fail now.").await;
    eventually("the failed turn ends", || ended(&flow, 3)).await;
    flow.settled().await;
    flow.say("Crash now.").await;
    eventually("the killed turn ends", || ended(&flow, 4)).await;
    flow.settled().await;

    let before = flow.manager.thread_view(&id).unwrap();
    let ends: Vec<_> = before.turns.iter().map(|turn| turn.end).collect();
    assert_eq!(
        ends,
        [
            Some(TurnEnd::Done),
            Some(TurnEnd::Stopped),
            Some(TurnEnd::Failed),
            Some(TurnEnd::Interrupted)
        ],
        "{before:#?}"
    );
    assert_eq!(
        before.turns[2].error.as_deref(),
        Some("The API refused it.")
    );
    assert_eq!(
        texts(&before),
        [
            "Look around.",
            "Looked.",
            "Wait for my stop.",
            "Fail now.",
            "Crash now."
        ],
        "each item once, in order"
    );
    let errors = before
        .entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.event,
                brigadier_providers::ProviderEvent::Error { .. }
            )
        })
        .count();
    assert_eq!(errors, 1, "{before:#?}");

    flow.restart().await;
    let after = flow.manager.thread_view(&id).unwrap();
    assert_eq!(after, before);
    flow.stop().await;
}

/// Each turn names Brigadier's requests it carried: its user message's, and a steer's; a
/// restart reads them back from `record/turns.jsonl`.
#[tokio::test]
async fn a_turn_names_the_requests_it_carried() {
    let running = Arc::new(tokio::sync::Notify::new());
    let ran = running.clone();
    let mut flow = Flow::start(
        "cli-thread-requests",
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
        script(move |turn| {
            let ran = ran.clone();
            async move {
                if turn.input.contains("Steer me") {
                    ran.notify_one();
                    turn.until_steered().await;
                    return Reply::text("Did both.");
                }
                Reply::text("Looked.")
            }
        }),
    )
    .await;
    let id = flow.conversation.clone();
    let ended = |flow: &Flow, count: usize| {
        let view = flow.manager.thread_view(&flow.conversation).unwrap();
        view.turns.len() == count && view.turns.iter().all(|turn| turn.end.is_some())
    };
    flow.say("Look around.").await;
    eventually("the first turn ends", || ended(&flow, 1)).await;
    flow.settled().await;
    flow.say("Steer me.").await;
    running.notified().await;
    flow.manager
        .send_message(
            id.clone(),
            "Also this.".into(),
            Vec::new(),
            Vec::new(),
            true,
            None,
        )
        .await
        .unwrap();
    eventually("the steered turn ends", || ended(&flow, 2)).await;
    flow.settled().await;

    let messages = flow.core.all_messages(&id).await.unwrap();
    let request = |text: &str| {
        let message = messages
            .iter()
            .find(|message| message.text == text)
            .unwrap();
        message.request_id.clone().unwrap()
    };
    let view = flow.manager.thread_view(&id).unwrap();
    assert_eq!(
        view.turns[0].requests,
        [request("Look around.")],
        "{view:#?}"
    );
    assert_eq!(
        view.turns[1].requests,
        [request("Steer me."), request("Also this.")],
        "{view:#?}"
    );
    flow.restart().await;
    assert_eq!(flow.manager.thread_view(&id).unwrap().turns, view.turns);
    flow.stop().await;
}

fn texts(view: &ThreadView) -> Vec<String> {
    view.entries
        .iter()
        .filter_map(|entry| match &entry.event {
            brigadier_providers::ProviderEvent::Message { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// The daemon dies mid-turn, and the thread goes on with another turn: the cut turn, which
/// the CLI never ended, reads interrupted, not done (#557).
#[tokio::test]
async fn a_turn_cut_by_a_crash_reads_interrupted_after_later_turns() {
    let holding = Arc::new(tokio::sync::Notify::new());
    let held = holding.clone();
    let mut flow = Flow::start(
        "cli-thread-crash",
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
        script(move |turn| {
            let held = held.clone();
            async move {
                if turn.input.contains("Hold.") {
                    held.notify_one();
                    std::future::pending::<()>().await;
                }
                Reply::text("Looked.")
            }
        }),
    )
    .await;
    flow.say("Hold.").await;
    holding.notified().await;
    flow.crash().await;
    flow.settled().await;
    flow.say("Look again.").await;
    eventually("the next turn ends", || {
        let view = flow.manager.thread_view(&flow.conversation).unwrap();
        view.turns.len() == 2 && view.turns[1].end.is_some()
    })
    .await;
    let view = flow.manager.thread_view(&flow.conversation).unwrap();
    let ends: Vec<_> = view.turns.iter().map(|turn| turn.end).collect();
    assert_eq!(
        ends,
        [Some(TurnEnd::Interrupted), Some(TurnEnd::Done)],
        "{view:#?}"
    );
    flow.stop().await;
}

/// Session 82123c64's shape: after a turn ended, Claude began one on its own for a background
/// job's end, its stream saying nothing of the notice (only the turn's start, then its
/// commands); a queued message reached it mid-turn and joined it. That turn goes on with the
/// turn before: the message is its steer, its request one more of that turn's, never a turn's
/// opening message the thread has no turn of. Live, and after a restart.
#[tokio::test]
async fn a_message_joining_a_turn_the_cli_began_on_its_own_is_that_turns_request() {
    use brigadier_providers::{ItemStatus, ProviderEvent, Role, TurnStatus};

    let mut flow = Flow::start(
        "cli-thread-on-its-own",
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
        script(|_| async { Reply::text("Looked.") }),
    )
    .await;
    let id = flow.conversation.clone();
    flow.say("Start the dev app.").await;
    eventually("the first turn ends", || {
        let view = flow.manager.thread_view(&id).unwrap();
        view.turns.len() == 1 && view.turns[0].end.is_some()
    })
    .await;
    flow.settled().await;
    let first = flow.manager.thread_view(&id).unwrap().turns[0].clone();
    let [asked] = first.requests.as_slice() else {
        panic!("{first:#?}");
    };

    let conv = flow.manager.conv(&id).unwrap();
    let follow = |event: ProviderEvent| {
        let (manager, conv) = (flow.manager.clone(), conv.clone());
        async move { manager.follow_cli_thread(&conv, Some(&event)).await }
    };
    let command = |status| ProviderEvent::Command {
        item_id: "toolu_wait".into(),
        command: "cat /tmp/wait-output".into(),
        cwd: None,
        status,
        exit_code: None,
        output: None,
        duration_ms: None,
        kept: None,
    };
    follow(ProviderEvent::TurnStarted { turn_id: None }).await;
    follow(command(ItemStatus::InProgress)).await;
    follow(command(ItemStatus::Completed)).await;
    // The turn before reads open again while this one runs.
    let running = flow.manager.thread_view(&id).unwrap();
    assert_eq!(running.turns.len(), 1, "{running:#?}");
    assert_eq!(running.turns[0].end, None);
    follow(ProviderEvent::Message {
        item_id: "65ae8f04-ec73-40e2-8ebc-0283dd40a248".into(),
        role: Role::User,
        text: "[queued message 2f334bc5 · judge it: brigadier queue 2f334bc5 join|wait|worker \
               <id>|new-session; unjudged, it joins this work]\nsure, do"
            .into(),
    })
    .await;
    // Judged: it joins, and the rest of the turn answers its request.
    let joined = "01a171d2-7c9f-7787-bd17-63a32f334bc5";
    conv.answer_request(joined).await;
    follow(ProviderEvent::TurnCompleted {
        turn_id: None,
        status: TurnStatus::Completed,
        duration_ms: None,
        usage: None,
    })
    .await;

    let view = flow.manager.thread_view(&id).unwrap();
    assert_eq!(view.turns.len(), 1, "{view:#?}");
    assert_eq!(view.turns[0].user_message, first.user_message);
    assert_eq!(view.turns[0].requests, [asked.clone(), joined.to_owned()]);
    assert_eq!(view.turns[0].end, Some(TurnEnd::Done));
    let steer = view
        .entries
        .iter()
        .find(|entry| matches!(&entry.event, ProviderEvent::Message { item_id, .. } if item_id.starts_with("65ae8f04")))
        .expect("the steer shows");
    assert!(steer.steer, "{steer:#?}");
    // No turn is noted under the steer's id: the turn before's notes hold its request.
    let records = crate::thread_view::read_turns(&flow.manager.session_dir(&id).record());
    assert!(
        records
            .iter()
            .all(|record| record.user_message.as_deref() == Some(first.user_message.as_str())),
        "{records:#?}"
    );
    flow.restart().await;
    let after = flow.manager.thread_view(&id).unwrap();
    assert_eq!(after.turns.len(), 1, "{after:#?}");
    assert_eq!(after.turns[0].requests, [asked.clone(), joined.to_owned()]);
    flow.stop().await;
}
