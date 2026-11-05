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
    // Nothing holds the work after the crash: its block closes.
    eventually("the block closes", || {
        let view = flow.manager.thread_view(&flow.conversation).unwrap();
        view.block.is_some_and(|block| block.ended_at_ms.is_some())
    })
    .await;

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
    // The news it holds is numbered on from the restart.
    assert!(after.seq > before.seq);
    assert_eq!(
        // The block record lives in memory: a restart starts with none.
        ThreadView {
            seq: before.seq,
            block: before.block.clone(),
            ..after
        },
        before
    );
    flow.stop().await;
}

/// A stopped turn's answer streamed so far, which the CLI's file never got, still reads in
/// that turn after the next turn ends.
#[tokio::test]
async fn a_stopped_turns_streamed_answer_stays_after_the_next_turn() {
    let running = Arc::new(tokio::sync::Notify::new());
    let ran = running.clone();
    let flow = Flow::start(
        "cli-thread-stopped-text",
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
                if turn.input.contains("Write lines") {
                    turn.emit(brigadier_providers::ProviderEvent::MessageDelta {
                        item_id: "partial".into(),
                        text: "1. Peaks".into(),
                        phase: None,
                    })
                    .await;
                    ran.notify_one();
                    assert!(turn.stopped().await, "the user stops it");
                    return Reply::text("");
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
    flow.say("Write lines.").await;
    running.notified().await;
    flow.manager.interrupt(id.clone()).await.unwrap();
    eventually("the stopped turn ends", || ended(&flow, 1)).await;
    flow.settled().await;
    flow.say("Look around.").await;
    eventually("the next turn ends", || ended(&flow, 2)).await;
    flow.settled().await;
    let view = flow.manager.thread_view(&id).unwrap();
    let texts: Vec<_> = view
        .entries
        .iter()
        .filter_map(|entry| match &entry.event {
            brigadier_providers::ProviderEvent::Message { text, .. }
            | brigadier_providers::ProviderEvent::MessageDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        texts,
        ["Write lines.", "1. Peaks", "Look around.", "Looked."],
        "{view:#?}"
    );
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
/// the CLI never ended, reads interrupted, not done (#557). It had begun (its run recorded
/// running), so it goes on by itself in a turn of its own, never merged into the cut one.
#[tokio::test]
async fn a_turn_cut_by_a_crash_reads_interrupted_after_later_turns() {
    a_crash_mid_turn_keeps_the_cut_turn_interrupted(false).await;
}

/// The same with a follow-up queued behind the cut turn: it runs after the resumed turn.
#[tokio::test]
async fn a_turn_cut_by_a_crash_with_a_queued_follow_up_reads_interrupted() {
    a_crash_mid_turn_keeps_the_cut_turn_interrupted(true).await;
}

async fn a_crash_mid_turn_keeps_the_cut_turn_interrupted(queued: bool) {
    use crate::work::RunState;

    let holding = Arc::new(tokio::sync::Notify::new());
    let held = holding.clone();
    let mut flow = Flow::start(
        "cli-thread-crash",
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                // The follow-up waits for its own turn.
                judge: std::sync::Mutex::new(Some("WAITS")),
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
    flow.until("the turn running", |board| board.run == RunState::Running)
        .await;
    if queued {
        flow.say("Look again.").await;
        flow.until("the follow-up queued", |board| {
            !board.queue.items.is_empty()
        })
        .await;
    }
    flow.crash().await;
    flow.settled().await;
    if !queued {
        // The cut-off turn goes on by itself (D9 a), then the next.
        eventually("the cut-off turn goes on", || {
            let view = flow.manager.thread_view(&flow.conversation).unwrap();
            view.turns.len() == 2 && view.turns[1].end.is_some()
        })
        .await;
        flow.say("Look again.").await;
    }
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
    // The resumed turn starts with Brigadier's word, which the thread draws as no bubble.
    let resumed = view
        .entries
        .iter()
        .find(|entry| {
            matches!(&entry.event, brigadier_providers::ProviderEvent::Message { item_id, .. }
                if *item_id == view.turns[1].user_message)
        })
        .unwrap();
    assert_eq!(
        resumed.said,
        Some(brigadier_providers::session_file::Said::Brigadier),
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
    own_turn_steer(false).await;
}

/// The same, with Claude's file holding the message as a turn's opening one (O38, seen live):
/// that turn goes on in the open block, joining it, and the turn before ends where it ended.
#[tokio::test]
async fn a_message_steered_into_the_clis_own_turn_its_file_opens_a_turn_with_joins_it() {
    own_turn_steer(true).await;
}

async fn own_turn_steer(file_line: bool) {
    use brigadier_providers::{ItemStatus, ProviderEvent, Role, TurnStatus};

    let file: Arc<std::sync::Mutex<Option<std::path::PathBuf>>> = Arc::default();
    let kept = file.clone();
    let mut flow = Flow::start(
        "cli-thread-on-its-own",
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
        script(move |turn| {
            kept.lock().unwrap().clone_from(&turn.session_file);
            async { Reply::text("Looked.") }
        }),
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
    let steered = "[queued message 2f334bc5 · judge it: brigadier queue 2f334bc5 join|wait|worker \
                   <id>|new-session; unjudged, it joins this work]\nsure, do";
    if file_line {
        super::session_line(
            file.lock().unwrap().as_ref().expect("a session file"),
            serde_json::json!({"type": "user", "uuid": "65ae8f04-ec73-40e2-8ebc-0283dd40a248",
                "message": {"role": "user", "content": steered}}),
        );
    }
    follow(ProviderEvent::Message {
        item_id: "65ae8f04-ec73-40e2-8ebc-0283dd40a248".into(),
        role: Role::User,
        text: steered.into(),
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
    if file_line {
        let joined_turn = |view: &ThreadView| {
            assert_eq!(view.turns.len(), 2, "{view:#?}");
            assert_eq!(view.turns[0].requests, [asked.clone(), joined.to_owned()]);
            assert!(!view.turns[0].joins && view.turns[1].joins, "{view:#?}");
            assert_eq!(view.turns[1].end, Some(TurnEnd::Done), "{view:#?}");
            assert!(
                view.turns[0].ended_at_ms <= view.turns[1].started_at_ms,
                "{view:#?}"
            );
        };
        joined_turn(&view);
        flow.restart().await;
        joined_turn(&flow.manager.thread_view(&id).unwrap());
        flow.stop().await;
        return;
    }
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
    // The turn before's notes hold its request; the steer's id is noted only as joining, for
    // a file that opens a turn with it.
    let records = crate::thread_view::read_turns(&flow.manager.session_dir(&id).record());
    assert!(
        records.iter().all(|record| {
            record.user_message.as_deref() == Some(first.user_message.as_str())
                || (record.joins && record.requests.is_empty() && record.end.is_none())
        }),
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

/// A background command that ended while its turn ran (Codex's `brigadier wait`, when a worker
/// was paused or stopped) may start a turn of its CLI's right after that turn: the turn's end
/// owes it, said with the end itself, so its block never folds between the two (D153).
#[tokio::test]
async fn a_command_that_ended_during_the_turn_owes_the_next_turn_from_the_turns_end() {
    use brigadier_providers::{
        BackgroundTask, BackgroundTaskKind, BackgroundTaskStatus, ProviderEvent, TurnStatus,
    };

    let flow = Flow::start(
        "cli-thread-ended-in-turn",
        Options::default(),
        script(|_| async { Reply::text("Paused.") }),
    )
    .await;
    let id = flow.conversation.clone();
    let conv = flow.manager.conv(&id).unwrap();
    let mut news = flow.manager.subscribe_thread_news();
    let wait = |status| ProviderEvent::BackgroundTask {
        task: BackgroundTask {
            command: None,
            provider: None,
            id: "proc-3".into(),
            item_id: Some("item_wait".into()),
            kind: BackgroundTaskKind::Terminal,
            label: "brigadier wait".into(),
            status,
            started_at_ms: 100,
            ended_at_ms: (status != BackgroundTaskStatus::Running).then(crate::now_ms),
            output_file: None,
            summary: None,
            last_line: None,
        },
    };
    let turn = |ended: bool| {
        let mut events = vec![ProviderEvent::TurnStarted { turn_id: None }];
        if ended {
            events.push(wait(BackgroundTaskStatus::Completed));
        }
        events.push(ProviderEvent::TurnCompleted {
            turn_id: None,
            status: TurnStatus::Completed,
            duration_ms: None,
            usage: None,
        });
        events
    };
    // The wait runs in the background from an earlier turn.
    flow.manager
        .follow_cli_thread(&conv, Some(&wait(BackgroundTaskStatus::Running)))
        .await;
    for event in turn(true) {
        flow.manager.follow_cli_thread(&conv, Some(&event)).await;
    }
    // The news that ends the turn says the turn owed, in the same message.
    let mut ended_owing = None;
    while let Ok(item) = news.try_recv() {
        if item.conversation_id == id && item.ended.is_some() {
            ended_owing = Some(item.jobs.is_some_and(|jobs| jobs.waking));
        }
    }
    assert_eq!(ended_owing, Some(true), "the turn's end owes the next turn");
    assert!(flow.manager.thread_view(&id).unwrap().jobs.waking);
    // No turn comes (its CLI took that end in the turn): it holds only the settle.
    tokio::time::sleep(super::super::conversation::WAKE_SETTLE * 2).await;
    assert!(!flow.manager.thread_view(&id).unwrap().jobs.waking);
    // A turn in which nothing ended owes nothing.
    for event in turn(false) {
        flow.manager.follow_cli_thread(&conv, Some(&event)).await;
    }
    assert!(!flow.manager.thread_view(&id).unwrap().jobs.waking);
    flow.stop().await;
}

/// A subagent its lead's turn left running holds that turn as a command does (D24): its end
/// owes the turn it starts, so the turn never reads done between the two. Claude Code 2.1.308
/// stream lines (an `Agent` call with `run_in_background`).
#[tokio::test]
async fn a_subagent_ending_after_its_turn_owes_the_turn_its_end_starts() {
    use brigadier_providers::claude::parse::{Output, Parser};

    let flow = Flow::start(
        "cli-thread-subagent-owes",
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
    let mut parser = Parser::live();
    let mut follow = async |lines: &[&str]| {
        for line in lines {
            for output in parser.feed(line) {
                if let Output::Event(event) = output {
                    flow.manager.follow_cli_thread(&conv, Some(&event)).await;
                }
            }
        }
        flow.manager.thread_view(&id).unwrap()
    };
    let view = follow(&[
        r#"{"type":"user","isReplay":true,"uuid":"u-sub","message":{"role":"user","content":"Start a subagent in the background."}}"#,
        r#"{"type":"assistant","parent_tool_use_id":null,"message":{"id":"m1","content":[{"type":"tool_use","id":"toolu_sub","name":"Agent","input":{"description":"Run sleep and ls","prompt":"Run sleep 15 && ls.","run_in_background":true}}]}}"#,
        r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"a1","task_type":"local_agent","description":"Run sleep and ls"}]}"#,
        r#"{"type":"system","subtype":"task_started","task_id":"a1","tool_use_id":"toolu_sub","description":"Run sleep and ls","is_backgrounded":true,"task_type":"local_agent"}"#,
        r#"{"type":"user","parent_tool_use_id":null,"message":{"role":"user","content":[{"tool_use_id":"toolu_sub","type":"tool_result","content":[{"type":"text","text":"Async agent launched successfully."}]}]}}"#,
        r#"{"type":"assistant","parent_tool_use_id":null,"message":{"id":"m2","content":[{"type":"text","text":"Started it."}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"result":"Started it."}"#,
    ])
    .await;
    assert_eq!(
        view.turns.last().and_then(|turn| turn.end),
        Some(TurnEnd::Done)
    );
    assert!(
        view.jobs.holding.is_empty(),
        "a running subagent shows as a worker, not a command"
    );
    assert!(view.jobs.subagents, "it holds the turn's work");

    // The subagent works: its own items, in the lead's stream.
    let view = follow(&[
        r#"{"type":"assistant","parent_tool_use_id":"toolu_sub","message":{"id":"c1","content":[{"type":"tool_use","id":"toolu_child","name":"Bash","input":{"command":"sleep 15 && ls"}}]}}"#,
        r#"{"type":"system","subtype":"task_progress","task_id":"a1","tool_use_id":"toolu_sub","description":"Running sleep 15 && ls","usage":{"total_tokens":20926,"tool_uses":1,"duration_ms":1808},"last_tool_name":"Bash"}"#,
        r#"{"type":"system","subtype":"task_started","task_id":"b1","owned_by_subagent":true,"tool_use_id":"toolu_child","description":"sleep 15 && ls","is_backgrounded":false,"parent_task_id":"a1","task_type":"local_bash"}"#,
        r#"{"type":"system","subtype":"task_notification","task_id":"b1","tool_use_id":"toolu_child","status":"completed","output_file":"","summary":"sleep 15 && ls"}"#,
        r#"{"type":"user","parent_tool_use_id":"toolu_sub","message":{"role":"user","content":[{"tool_use_id":"toolu_child","type":"tool_result","content":"LICENSE","is_error":false}]}}"#,
        r#"{"type":"assistant","parent_tool_use_id":"toolu_sub","message":{"id":"c2","content":[{"type":"text","text":"It printed LICENSE."}]}}"#,
    ])
    .await;
    assert_eq!(
        view.turns.last().and_then(|turn| turn.end),
        Some(TurnEnd::Done),
        "a subagent's own items open no turn of its lead's"
    );

    let view = follow(&[
        r#"{"type":"system","subtype":"task_updated","task_id":"a1","patch":{"status":"completed","end_time":2000}}"#,
        r#"{"type":"system","subtype":"task_notification","task_id":"a1","tool_use_id":"toolu_sub","status":"completed","output_file":"/private/tmp/claude-501/-w/s1/tasks/a1.output","summary":"Listed two files."}"#,
        r#"{"type":"system","subtype":"background_tasks_changed","tasks":[]}"#,
    ])
    .await;
    assert!(view.jobs.waking, "its end owes the turn it starts");

    let view = follow(&[
        r#"{"type":"user","isReplay":true,"uuid":"n-sub","message":{"role":"user","content":"<task-notification>\n<task-id>a1</task-id>\n<status>completed</status>\n<summary>Listed two files.</summary>\n</task-notification>"}}"#,
    ])
    .await;
    assert!(
        view.jobs.is_empty(),
        "the turn started: its own open turn says Working"
    );
    assert_eq!(view.turns.last().and_then(|turn| turn.end), None);
    flow.stop().await;
}

/// The same hold on OpenCode: a command its `shell` tool moves to the background holds the
/// turn from its call's result until OpenCode says its end, which owes the turn that word
/// starts (still Working) until that turn starts; done once it ends. Lines in the shapes
/// `opencode serve` 2.2.3 sends, through the adapter's own parser.
#[tokio::test]
async fn an_opencode_background_command_holds_its_turn_until_the_turn_its_end_starts() {
    use brigadier_providers::opencode::parse::Parser;
    use serde_json::{Value, json};

    let flow = Flow::start(
        "cli-thread-opencode-background",
        Options {
            thread: brigadier_providers::ProviderKind::OpenCode,
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
    let mut parser = Parser::new(Some("ses_1".into()), None);
    let line = |kind: &str, data: Value| {
        let mut data = data;
        data["sessionID"] = json!("ses_1");
        json!({"id": "evt", "created": 1_000, "type": kind, "data": data}).to_string()
    };
    let commands_working = || async {
        flow.manager
            .activity()
            .await
            .into_iter()
            .find(|entry| entry.conversation_id == id)
            .is_some_and(|entry| entry.commands_working)
    };
    let mut follow = async |lines: Vec<String>| {
        for line in lines {
            for event in parser.feed(&line) {
                flow.manager.follow_cli_thread(&conv, Some(&event)).await;
            }
        }
    };

    follow(vec![
        line("session.inbox.enqueued", json!({"inboxID": "msg_u", "item": {"type": "user", "payload": {"text": "Start sleep 25 in the background, tell me when it ends."}}})),
        line("session.execution.started", json!({})),
        line("session.inbox.delivered", json!({"inboxID": "msg_u"})),
        line("session.tool.input.started", json!({"assistantMessageID": "msg_a", "id": "call_1", "name": "shell"})),
        line("session.tool.called", json!({"assistantMessageID": "msg_a", "id": "call_1", "input": {"command": "sleep 25 && echo done", "background": true}})),
        line("session.tool.success", json!({"assistantMessageID": "msg_a", "id": "call_1", "content": [{"type": "text", "text": "Command moved to the background."}], "metadata": {"shellID": "sh_1", "status": "running", "truncated": false}})),
        line("session.text.ended", json!({"assistantMessageID": "msg_b", "ordinal": 0, "text": "It runs; I'll tell you when it ends."})),
        line("session.execution.succeeded", json!({})),
    ])
    .await;
    let view = flow.manager.thread_view(&id).unwrap();
    assert_eq!(
        view.turns.last().and_then(|turn| turn.end),
        Some(TurnEnd::Done)
    );
    assert_eq!(view.jobs.holding, ["sh_1"], "{:#?}", view.background);
    assert!(commands_working().await, "the sidebar says Working");

    follow(vec![line("session.inbox.enqueued", json!({"inboxID": "msg_s", "item": {"type": "synthetic", "payload": {"text": "<shell>done</shell>", "description": "sleep 25 && echo done", "metadata": {"source": "shell", "jobID": "sh_1", "shellID": "sh_1", "state": "completed", "exit": 0}}, "delivery": "steer"}}))]).await;
    let view = flow.manager.thread_view(&id).unwrap();
    assert!(view.jobs.holding.is_empty());
    assert!(view.jobs.waking, "its end owes the turn it starts");
    assert!(
        commands_working().await,
        "still Working until that turn starts"
    );

    // Begun, the turn hasn't said yet that it is the end's: the turn before still waits for it,
    // never reads done meanwhile.
    follow(vec![line("session.execution.started", json!({}))]).await;
    let view = flow.manager.thread_view(&id).unwrap();
    assert!(
        view.jobs.waking || view.turns.last().is_some_and(|turn| turn.end.is_none()),
        "{:#?} {:#?}",
        view.jobs,
        view.turns
    );

    follow(vec![line(
        "session.text.ended",
        json!({"assistantMessageID": "msg_c", "ordinal": 0, "text": "Done: it printed done."}),
    )])
    .await;
    let view = flow.manager.thread_view(&id).unwrap();
    assert!(
        view.jobs.is_empty(),
        "the turn started: its own open turn says Working"
    );
    assert_eq!(view.turns.last().and_then(|turn| turn.end), None);

    follow(vec![line("session.execution.succeeded", json!({}))]).await;
    assert!(!commands_working().await, "done once that turn ends");
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

/// A Read of `picture` in Claude Code's session file `file`, its result holding the picture.
#[cfg(unix)]
fn read_with_image(file: &str, item: &str, picture: &std::path::Path) {
    use std::io::Write as _;
    let data = brigadier_providers::image_base64(super::chat_images_tests::PNG);
    let lines = [
        serde_json::json!({"type": "assistant", "message": {"id": format!("msg_{item}"), "role": "assistant",
            "content": [{"type": "tool_use", "id": item, "name": "Read", "input": {"file_path": picture}}]}}),
        serde_json::json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": item,
            "content": [{"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": data}}]}]}}),
    ];
    let mut file = std::fs::OpenOptions::new().append(true).open(file).unwrap();
    for line in lines {
        writeln!(file, "{line}").unwrap();
    }
}

/// #792: a fork reads its copied thread's images: the one a session file of its source holds
/// and the copy its source kept of one viewed, whose file is gone. A file its thread never
/// named stays refused.
#[cfg(unix)]
#[tokio::test]
async fn a_fork_reads_the_images_of_the_thread_it_copied() {
    use crate::model::{ForkPlace, MessageRole};
    use brigadier_providers::{ItemStatus, ProviderEvent, session_file::FileImage};
    let folder = std::env::temp_dir().join(format!("fork-images-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&folder).unwrap();
    let picture = folder.join("viewed.png");
    std::fs::write(&picture, super::chat_images_tests::PNG).unwrap();
    let input = serde_json::json!({ "file_path": picture }).to_string();
    let flow = Flow::start(
        "cli-thread-fork-images",
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
        script(move |turn| {
            let input = input.clone();
            async move {
                turn.emit(ProviderEvent::ToolCall {
                    item_id: "view_1".into(),
                    name: "Read".into(),
                    input: Some(input),
                    status: ItemStatus::Completed,
                    output: None,
                    kept: None,
                })
                .await;
                Reply::text("Looked.")
            }
        }),
    )
    .await;
    let id = flow.conversation.clone();
    flow.say("Look.").await;
    eventually("the turn ends", || {
        let view = flow.manager.thread_view(&id).unwrap();
        view.turns.len() == 1 && view.turns[0].end.is_some()
    })
    .await;
    flow.settled().await;
    let record = flow.manager.session_dir(&id).record();
    let file = crate::manager::session_dir::read_lineage(&record)
        .last()
        .and_then(|session| session.file.clone())
        .unwrap();
    read_with_image(&file, "view_1", &picture);
    let answer = flow
        .core
        .all_messages(&id)
        .await
        .unwrap()
        .into_iter()
        .rfind(|message| message.role == MessageRole::Assistant)
        .unwrap()
        .id;
    let fork = flow
        .manager
        .fork(id.clone(), answer, ForkPlace::NewWorktree)
        .await
        .unwrap();
    std::fs::remove_dir_all(&folder).unwrap();

    let view = flow.manager.thread_view(&fork.id).unwrap();
    let image = view
        .entries
        .iter()
        .find_map(|entry| entry.images.first().cloned())
        .expect("its copied thread names the image");
    let data = brigadier_providers::image_base64(super::chat_images_tests::PNG);
    let (read, _) = flow.manager.thread_image(&fork.id, &image).unwrap();
    assert_eq!(read, data);
    let (read, _) = flow
        .manager
        .read_thread_image(&fork.id, "view_1")
        .await
        .unwrap();
    assert_eq!(read, data);
    let elsewhere = FileImage {
        file: "/elsewhere/session.jsonl".into(),
        ..image
    };
    assert!(flow.manager.thread_image(&fork.id, &elsewhere).is_err());
}

/// #792: a worker's own transcript reads its images through its session: the one its session
/// file holds and the one at the path its item names. Another session's item stays refused.
#[cfg(unix)]
#[tokio::test]
async fn a_workers_transcript_reads_its_images() {
    use super::cli_tests::{brigadier, id_of, start, wait_for};
    use brigadier_cli::protocol::CliRequest;
    let folder = std::env::temp_dir().join(format!("worker-images-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&folder).unwrap();
    let picture = folder.join("viewed.png");
    std::fs::write(&picture, super::chat_images_tests::PNG).unwrap();
    let flow = Flow::start(
        "cli-thread-worker-images",
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
        script(|turn| async move {
            if !turn.input.contains("Start one.") {
                return Reply::text("Noted.");
            }
            let started = brigadier(&turn, CliRequest::Start(Box::new(start("Write it.")))).await;
            wait_for(&turn, &[&id_of(&started)], 1).await;
            Reply::text("Done.")
        }),
    )
    .await;
    let id = flow.conversation.clone();
    flow.say("Start one.").await;
    flow.until("its worker exists", |board| !board.tasks.is_empty())
        .await;
    flow.settled().await;
    let board = flow.board().await;
    let task = board.tasks.values().next().unwrap();
    let record = flow.manager.session_dir(&id).worker(task).join("record");
    let file = crate::manager::session_dir::read_lineage(&record)
        .last()
        .and_then(|session| session.file.clone())
        .expect("its session file");
    read_with_image(&file, "view_w", &picture);

    let view = flow.manager.worker_thread(&id, &task.id).await.unwrap();
    let image = view
        .entries
        .iter()
        .find_map(|entry| entry.images.first().cloned())
        .expect("its transcript names the image");
    let data = brigadier_providers::image_base64(super::chat_images_tests::PNG);
    let (read, _) = flow.manager.thread_image(&id, &image).unwrap();
    assert_eq!(read, data);
    let (read, _) = flow.manager.read_thread_image(&id, "view_w").await.unwrap();
    assert_eq!(read, data);
    assert!(
        flow.manager
            .read_thread_image(&id, "no_such_item")
            .await
            .is_err()
    );
    std::fs::remove_dir_all(&folder).unwrap();
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
    let restarted = flow.manager.thread_view(&fork.id).unwrap();
    assert_eq!(
        ThreadView {
            seq: view.seq,
            ..restarted
        },
        view
    );

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

/// Open speed (goal 10): the first `threadView` after a daemon start, with its workers' rows and
/// their git read, on a long session file (65 turns, about 6.5 MB). A timing, not a check: run
/// it in release with `--ignored --nocapture`.
#[tokio::test]
#[ignore]
async fn the_first_thread_read_after_a_start_is_timed() {
    let mut flow = Flow::start(
        "cli-thread-open-speed",
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
        script(|turn| async move {
            if turn.worker().is_none() && turn.input.contains("Start them.") {
                turn.start_worker("A", "Fix a.").await;
                turn.start_worker("B", "Fix b.").await;
                return Reply::text("Started two.");
            }
            Reply::text("Done.")
        }),
    )
    .await;
    let id = flow.conversation.clone();
    flow.say("Start them.").await;
    super::eventually_async("two workers", || {
        let core = flow.core.clone();
        let id = id.clone();
        async move { core.tasks(&id).await.is_ok_and(|tasks| tasks.len() == 2) }
    })
    .await;
    flow.settled().await;
    // The session file grows to a long session's: each turn a question, an answer and 15 calls
    // with 6 KB of output each.
    let record = flow.manager.session_dir(&id).record();
    let file = crate::manager::session_dir::read_lineage(&record)
        .last()
        .and_then(|session| session.file.clone())
        .expect("its session file");
    let output = "x".repeat(6_000);
    let mut lines = String::new();
    for turn in 0..65 {
        let line = |value: serde_json::Value| format!("{value}\n");
        lines += &line(
            serde_json::json!({"type": "user", "uuid": format!("u{turn}"),
            "message": {"role": "user", "content": format!("Question {turn}")}}),
        );
        for call in 0..15 {
            let tool = format!("toolu_{turn}_{call}");
            lines += &line(serde_json::json!({"type": "assistant", "message": {
                "id": format!("msg_{turn}_{call}"), "role": "assistant",
                "content": [{"type": "tool_use", "id": tool, "name": "Bash", "input": {"command": "ls"}}]}}));
            lines += &line(
                serde_json::json!({"type": "user", "uuid": format!("r{turn}_{call}"),
                "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": tool, "content": output}]}}),
            );
        }
        lines += &line(serde_json::json!({"type": "assistant", "message": {
            "id": format!("msg_{turn}_end"), "role": "assistant",
            "content": [{"type": "text", "text": format!("Answer {turn}")}]}}));
    }
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap()
        .write_all(lines.as_bytes())
        .unwrap();
    let size = std::fs::metadata(&file).unwrap().len();

    let ms = |since: std::time::Instant| since.elapsed().as_secs_f64() * 1000.0;
    flow.restart().await;
    let started = std::time::Instant::now();
    let view = flow.manager.thread_view_with_workers(&id).await.unwrap();
    let first = ms(started);
    let started = std::time::Instant::now();
    flow.manager.thread_view_with_workers(&id).await.unwrap();
    let second = ms(started);
    // Once git's last read is old, a read doesn't wait for the next: it comes as news.
    tokio::time::sleep(std::time::Duration::from_millis(2_100)).await;
    let started = std::time::Instant::now();
    flow.manager.thread_view_with_workers(&id).await.unwrap();
    let stale = ms(started);
    // Its parts, each cold again after another start.
    flow.restart().await;
    let started = std::time::Instant::now();
    flow.manager.worker_rows_of(&id, false).await.unwrap();
    let rows = ms(started);
    let started = std::time::Instant::now();
    flow.manager.thread_view(&id).unwrap();
    let thread = ms(started);
    let started = std::time::Instant::now();
    flow.manager.worker_rows_of(&id, true).await.unwrap();
    let rows_again = ms(started);
    let started = std::time::Instant::now();
    let bytes = serde_json::to_vec(&view).unwrap().len();
    let serialize = ms(started);
    eprintln!(
        "open speed: file {:.1} MB, {} entries, {} workers; first threadView {first:.1} ms, \
         second {second:.1} ms, with git due {stale:.1} ms; cold parts: worker rows with git {rows:.1} ms (git again {rows_again:.1} ms), \
         thread {thread:.1} ms; serialize {serialize:.1} ms for {:.2} MB",
        size as f64 / 1e6,
        view.entries.len(),
        view.workers.len(),
        bytes as f64 / 1e6,
    );
    flow.stop().await;
}

/// D5: every piece of a session's news is numbered, rising, and a read of its thread says the
/// last number it holds: what was said before the read is in it, what comes after isn't.
#[tokio::test]
async fn a_read_says_which_news_it_holds() {
    let flow = Flow::start(
        "cli-thread-news-seq",
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
        script(|_| async move { Reply::text("Looked.") }),
    )
    .await;
    let mut news = flow.manager.subscribe_thread_news();
    let heard = |news: &mut tokio::sync::broadcast::Receiver<crate::thread_view::ThreadNews>| {
        std::iter::from_fn(|| news.try_recv().ok())
            .filter(|piece| piece.conversation_id == flow.conversation && piece.workers.is_none())
            .map(|piece| piece.seq)
            .collect::<Vec<_>>()
    };
    flow.say("Look around.").await;
    eventually("the turn ends", || {
        let view = flow.manager.thread_view(&flow.conversation).unwrap();
        view.turns.len() == 1 && view.turns[0].end.is_some()
    })
    .await;
    flow.settled().await;
    let before = heard(&mut news);
    let view = flow.manager.thread_view(&flow.conversation).unwrap();
    assert!(!before.is_empty());
    assert!(before.iter().all(|seq| *seq > 0), "{before:?}");
    assert!(
        before.windows(2).all(|two| two[0] < two[1]),
        "rising: {before:?}"
    );
    assert!(
        before.iter().all(|seq| *seq <= view.seq),
        "{before:?} in {}",
        view.seq
    );
    flow.say("Again.").await;
    eventually("the second turn ends", || {
        let view = flow.manager.thread_view(&flow.conversation).unwrap();
        view.turns.len() == 2 && view.turns[1].end.is_some()
    })
    .await;
    let after = heard(&mut news);
    assert!(!after.is_empty());
    assert!(
        after.iter().all(|seq| *seq > view.seq),
        "{after:?} after {}",
        view.seq
    );
    flow.stop().await;
}

/// A turn's own message the CLI says after the turn opened (Claude's comes with the first token,
/// seconds on) says the turn opened again: the read the open asked for had no such turn, so the
/// thread read its newest turn as the one before, over, and its live line said "Thinking" over
/// a running `brigadier start … && brigadier wait` (demo13).
#[tokio::test]
async fn a_turns_own_message_said_after_it_opened_reads_its_turns_again() {
    use brigadier_providers::{ProviderEvent, Role};

    let flow = Flow::start(
        "cli-thread-message-after-open",
        Options::default(),
        script(|_| async { Reply::text("Started.") }),
    )
    .await;
    let id = flow.conversation.clone();
    let conv = flow.manager.conv(&id).unwrap();
    let mut news = flow.manager.subscribe_thread_news();
    let mut opened = async |event: ProviderEvent| {
        flow.manager.follow_cli_thread(&conv, Some(&event)).await;
        let mut said = false;
        while let Ok(item) = news.try_recv() {
            said |= item.conversation_id == id && item.opened;
        }
        said
    };
    assert!(opened(ProviderEvent::TurnStarted { turn_id: None }).await);
    let message = |item_id: &str| ProviderEvent::Message {
        item_id: item_id.into(),
        role: Role::User,
        text: "Start another worker".into(),
    };
    assert!(
        opened(message("u2")).await,
        "its own message reads the turns again"
    );
    // A message steered into it once it has its own is no new turn.
    assert!(!opened(message("u3")).await);
    flow.stop().await;
}
