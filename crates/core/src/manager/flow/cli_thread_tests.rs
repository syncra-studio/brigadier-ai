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
                if turn.input.contains("Hold.") && !turn.input.contains("Brigadier restarted") {
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
    // The cut-off turn goes on by itself (D9 a), then the next.
    eventually("the cut-off turn goes on", || {
        let view = flow.manager.thread_view(&flow.conversation).unwrap();
        view.turns.len() == 2 && view.turns[1].end.is_some()
    })
    .await;
    flow.say("Look again.").await;
    eventually("the next turn ends", || {
        let view = flow.manager.thread_view(&flow.conversation).unwrap();
        view.turns.len() == 3 && view.turns[2].end.is_some()
    })
    .await;
    let view = flow.manager.thread_view(&flow.conversation).unwrap();
    let ends: Vec<_> = view.turns.iter().map(|turn| turn.end).collect();
    assert_eq!(
        ends,
        [
            Some(TurnEnd::Interrupted),
            Some(TurnEnd::Done),
            Some(TurnEnd::Done)
        ],
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

/// A background command the session started holds its turn, as its workers do (D21): Claude's
/// stream (`fixtures/claude-background-watch.jsonl`, D9(c)) starts a watcher and a
/// `brigadier wait` it moves to the background, then ends its turn. The view and the sidebar's
/// snapshot hold the watcher, never the wait; once the watcher ends its end owes a turn (still
/// Working) until Claude starts it; a command whose end starts none stops being waited for
/// after the grace.
#[tokio::test]
async fn a_sessions_background_command_holds_its_turn_until_the_turn_its_end_starts() {
    use brigadier_providers::ProviderEvent;
    use brigadier_providers::claude::parse::{Output, Parser};

    let flow = Flow::start(
        "cli-thread-background-holds",
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
    let conv = flow.manager.conv(&id).unwrap();
    let mut news = flow.manager.subscribe_thread_news();
    let mut parser = Parser::live();
    let mut phases = Vec::new();
    let mut phase = Vec::new();
    for line in include_str!("fixtures/claude-background-watch.jsonl").lines() {
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        if let Some(name) = value.get("phase").and_then(serde_json::Value::as_str) {
            phases.push((name.to_owned(), std::mem::take(&mut phase)));
        } else if value.get("fixture").is_none() {
            phase.extend(
                parser
                    .feed(line)
                    .into_iter()
                    .filter_map(|output| match output {
                        Output::Event(event) => Some(event),
                        _ => None,
                    }),
            );
        }
    }
    let commands_working = || async {
        flow.manager
            .activity()
            .await
            .into_iter()
            .find(|entry| entry.conversation_id == id)
            .is_some_and(|entry| entry.commands_working)
    };
    let mut said = Vec::new();
    let mut follow = async |events: Vec<ProviderEvent>| {
        for event in events {
            flow.manager.follow_cli_thread(&conv, Some(&event)).await;
        }
        while let Ok(item) = news.try_recv() {
            if item.conversation_id == id
                && let Some(jobs) = item.jobs
            {
                said.push(jobs);
            }
        }
    };
    let mut phases = phases.into_iter();

    let (name, events) = phases.next().unwrap();
    assert_eq!(name, "watching");
    follow(events).await;
    let view = flow.manager.thread_view(&id).unwrap();
    assert_eq!(
        view.turns.last().and_then(|turn| turn.end),
        Some(TurnEnd::Done)
    );
    // The watcher holds the turn; the wait, moved to the background, doesn't.
    assert_eq!(view.jobs.holding, ["bwatch12"], "{:#?}", view.background);
    assert!(!view.jobs.waking);
    assert!(commands_working().await, "the sidebar says Working");

    let (name, events) = phases.next().unwrap();
    assert_eq!(name, "owed");
    follow(events).await;
    let view = flow.manager.thread_view(&id).unwrap();
    assert!(view.jobs.holding.is_empty());
    assert!(view.jobs.waking, "its end owes the turn it starts");
    assert!(
        commands_working().await,
        "still Working until that turn starts"
    );

    let (name, events) = phases.next().unwrap();
    assert_eq!(name, "woken");
    follow(events).await;
    let view = flow.manager.thread_view(&id).unwrap();
    assert!(
        view.jobs.is_empty(),
        "the turn started: its own open turn says Working"
    );
    assert_eq!(view.turns.last().and_then(|turn| turn.end), None);

    let (name, events) = phases.next().unwrap();
    assert_eq!(name, "answered");
    follow(events).await;
    assert!(!commands_working().await, "done once that turn ends");

    // A command whose end starts no turn: waited for only the grace.
    let (name, events) = phases.next().unwrap();
    assert_eq!(name, "no wake");
    follow(events).await;
    assert!(flow.manager.thread_view(&id).unwrap().jobs.waking);
    tokio::time::sleep(super::super::conversation::BACKGROUND_TURN_GRACE * 2).await;
    follow(Vec::new()).await;
    assert!(flow.manager.thread_view(&id).unwrap().jobs.is_empty());
    assert!(!commands_working().await);
    // Each change was said once, as it came: held, owed, started, held again, owed, given up.
    let holding = |jobs: &crate::thread_view::CliJobs| (jobs.holding.clone(), jobs.waking);
    assert_eq!(
        said.iter().map(holding).collect::<Vec<_>>(),
        [
            (vec!["bwatch12".to_owned()], false),
            (vec![], true),
            (vec![], false),
            (vec!["b13".to_owned()], false),
            (vec![], true),
            (vec![], false),
        ]
    );
    flow.stop().await;
}

/// Brigadier's `/compact` after a turn: Claude says the compaction, then `/compact` as the
/// user's message. That message starts a turn of its own, noted under its own id, so the
/// thread read back after the turn (the file holds `/compact` as a turn) never reads it as cut
/// off, and the turn before keeps its end.
#[tokio::test]
async fn brigadiers_compact_is_a_turn_of_its_own_never_one_more_of_the_turn_before() {
    use brigadier_providers::{ProviderEvent, Role, TurnStatus};

    let flow = Flow::start(
        "cli-thread-compact",
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
    flow.say("Look around.").await;
    eventually("the first turn ends", || {
        let view = flow.manager.thread_view(&id).unwrap();
        view.turns.len() == 1 && view.turns[0].end.is_some()
    })
    .await;
    flow.settled().await;
    let first = flow.manager.thread_view(&id).unwrap().turns[0].clone();

    let conv = flow.manager.conv(&id).unwrap();
    let follow = |event: ProviderEvent| {
        let (manager, conv) = (flow.manager.clone(), conv.clone());
        async move { manager.follow_cli_thread(&conv, Some(&event)).await }
    };
    follow(ProviderEvent::TurnStarted { turn_id: None }).await;
    follow(ProviderEvent::CompactionStarted { automatic: false }).await;
    // While it runs the live thread says so, the turn before still over: the app's "Refreshing
    // context" divider, which its end takes over.
    let running = flow.manager.thread_view(&id).unwrap();
    assert_eq!(running.turns.len(), 1, "{running:#?}");
    assert_eq!(running.turns[0].end, Some(TurnEnd::Done));
    assert!(
        running.entries.last().is_some_and(|entry| matches!(
            entry.event,
            ProviderEvent::CompactionStarted { automatic: false }
        )),
        "{running:#?}"
    );
    follow(ProviderEvent::CompactionEnded {
        automatic: false,
        tokens_before: Some(900_000),
        tokens_after: Some(40_000),
        error: None,
        native_id: None,
    })
    .await;
    let compact = "549fa9d0-9a52-4c1d-ae9a-2bdac6bb2e53";
    follow(ProviderEvent::Message {
        item_id: compact.into(),
        role: Role::User,
        text: "/compact Besides your usual summary, keep these exactly".into(),
    })
    .await;
    follow(ProviderEvent::TurnCompleted {
        turn_id: None,
        status: TurnStatus::Completed,
        duration_ms: None,
        usage: None,
    })
    .await;

    let view = flow.manager.thread_view(&id).unwrap();
    let ends: Vec<_> = view
        .turns
        .iter()
        .map(|turn| (turn.user_message.as_str(), turn.end))
        .collect();
    assert_eq!(
        ends,
        [
            (first.user_message.as_str(), Some(TurnEnd::Done)),
            (compact, Some(TurnEnd::Done))
        ],
        "{view:#?}"
    );
    assert_eq!(view.turns[0].ended_at_ms, first.ended_at_ms);
    // Over, the thread is its file's: no word of it running is left.
    assert!(
        !view
            .entries
            .iter()
            .any(|entry| matches!(entry.event, ProviderEvent::CompactionStarted { .. })),
        "{view:#?}"
    );
    let records = crate::thread_view::read_turns(&flow.manager.session_dir(&id).record());
    assert!(
        records
            .iter()
            .any(|record| record.user_message.as_deref() == Some(compact)
                && record.end == Some(TurnEnd::Done)),
        "{records:#?}"
    );
    flow.stop().await;
}

/// #648: a fork's thread is the source's, as its CLI kept it, up to the answer it forks: its
/// messages and answers, each turn ended as it did, and nothing after. It stays so after a
/// restart, and the fork's own turns follow it.
#[tokio::test]
async fn a_fork_shows_the_sources_thread_up_to_its_answer() {
    use crate::model::{ForkPlace, MessageRole};
    let mut flow = Flow::start(
        "cli-thread-fork",
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
        script(|turn| async move {
            if turn.input.contains("Two.") {
                return Reply::text("Answered two.");
            }
            if turn.input.contains("Three.") {
                return Reply::text("Answered three.");
            }
            Reply::text("Answered one.")
        }),
    )
    .await;
    let id = flow.conversation.clone();
    let ended = |flow: &Flow, id: &crate::model::ConversationId, count: usize| {
        let view = flow.manager.thread_view(id).unwrap();
        view.turns.len() == count && view.turns.iter().all(|turn| turn.end.is_some())
    };
    for (n, text) in ["One.", "Two.", "Three."].into_iter().enumerate() {
        flow.say(text).await;
        eventually("the turn ends", || ended(&flow, &id, n + 1)).await;
        flow.settled().await;
    }
    let messages = flow.core.all_messages(&id).await.unwrap();
    let request = |text: &str| {
        messages
            .iter()
            .find(|message| message.text == text)
            .unwrap()
            .request_id
            .clone()
            .unwrap()
    };
    let two = request("Two.");
    let answer = messages
        .iter()
        .rfind(|message| {
            message.role == MessageRole::Assistant && message.request_id.as_ref() == Some(&two)
        })
        .unwrap()
        .id
        .clone();
    let source = flow.manager.thread_view(&id).unwrap();

    let fork = flow
        .manager
        .fork(id.clone(), answer, ForkPlace::NewWorktree)
        .await
        .unwrap();
    let view = flow.manager.thread_view(&fork.id).unwrap();
    assert_eq!(
        texts(&view),
        ["One.", "Answered one.", "Two.", "Answered two."],
        "{view:#?}"
    );
    assert_eq!(view.turns, source.turns[..2], "{view:#?}");
    assert_eq!(view.turns[1].requests, [two]);
    flow.restart().await;
    assert_eq!(flow.manager.thread_view(&fork.id).unwrap(), view);

    flow.say_to(&fork.id, "Four.").await;
    eventually("the fork's own turn ends", || ended(&flow, &fork.id, 3)).await;
    let after = flow.manager.thread_view(&fork.id).unwrap();
    assert_eq!(
        texts(&after)[..4],
        ["One.", "Answered one.", "Two.", "Answered two."],
        "{after:#?}"
    );
    assert!(
        texts(&after).iter().any(|text| text == "Four."),
        "{after:#?}"
    );
    assert!(
        !texts(&after).iter().any(|text| text.contains("three")),
        "{after:#?}"
    );
    flow.stop().await;
}
