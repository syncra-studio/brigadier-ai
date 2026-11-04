//! A mockup's worker reads drawing (`Task::mockup_drawing`, its Mockup tab's spinner) only through
//! the turns that draw it: its first, and each round of the user's notes, whether that turn is
//! held open or over at once. Later work `brigadier send` gives the same worker draws no mockup
//! (#636). Every platform: the CLI is the flow's in-process fake, and `brigadier` is called in
//! process.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use brigadier_cli::protocol::{
    CliCall, CliReply, CliRequest, GRANT_ENV, PROTOCOL, SESSION_ENV, StartRequest,
};
use serde_json::Value;

use super::{Flow, Options, Reply, Script, Turn};
use crate::tools::Role;
use crate::work::TaskState;

/// `brigadier start --mockup`, as the session's CLI runs it, in process.
async fn start_mockup(turn: &Turn, prompt: &str) -> Value {
    let session = turn.env(SESSION_ENV).expect("the CLI knows its session");
    let grant = turn
        .env(GRANT_ENV)
        .expect("the CLI has its session's grant");
    let caller = turn
        .host
        .cli_caller(session, grant)
        .expect("the session's own grant is accepted");
    let request = CliRequest::Start(Box::new(StartRequest {
        title: None,
        agent: None,
        model: None,
        effort: None,
        work: None,
        reviewing: Vec::new(),
        local: false,
        repo: None,
        base: None,
        permission: None,
        mcps: Vec::new(),
        with: Vec::new(),
        mockup: true,
        prompt: prompt.into(),
    }));
    match turn
        .host
        .cli_call(
            &caller,
            CliCall {
                protocol: PROTOCOL,
                request,
            },
        )
        .await
    {
        CliReply::Ok { output } => output,
        CliReply::Error { error } => panic!("brigadier start failed: {error:?}"),
    }
}

#[tokio::test]
async fn a_mockup_reads_drawing_only_while_a_turn_draws_it() {
    let heard = Arc::new(Mutex::new(HashMap::<String, String>::new()));
    // Holds a worker's turn open until the test lets it end.
    let gate = Arc::new(tokio::sync::Notify::new());
    let script: Script = {
        let heard = heard.clone();
        let gate = gate.clone();
        Arc::new(move |turn| {
            let heard = heard.clone();
            let gate = gate.clone();
            Box::pin(async move {
                let keep = |name: &str| {
                    heard
                        .lock()
                        .unwrap()
                        .insert(name.into(), turn.input.clone());
                };
                let worker = turn
                    .env(GRANT_ENV)
                    .and_then(|grant| turn.host.grants().resolve(grant));
                if let Some(Role::Worker { .. }) = worker {
                    if turn.input.starts_with("Notes on the mockup (version 1)") {
                        keep("notes");
                        gate.notified().await;
                        return Reply::text("Revised.");
                    }
                    // The second round's turn is over at once.
                    if turn.input.starts_with("Notes on the mockup") {
                        keep("notes again");
                        return Reply::text("Revised again.");
                    }
                    if turn.input.starts_with("Build the approved design") {
                        keep("build");
                        gate.notified().await;
                        return Reply::text("Built it.");
                    }
                    // Its first turn: the screen goes where the prompt says.
                    keep("first");
                    gate.notified().await;
                    let folder = turn
                        .input
                        .split_once("mockup-2.png… in ")
                        .and_then(|(_, rest)| rest.split_once("; they show"))
                        .map(|(folder, _)| std::path::PathBuf::from(folder))
                        .expect("the folder is named");
                    std::fs::write(folder.join("mockup-1.png"), b"png").unwrap();
                    return Reply::text("Made it.");
                }
                let started = start_mockup(&turn, "Mock the density toggle.").await;
                assert!(started.get("error").is_none(), "{started:#}");
                Reply::text("Started the mockup.")
            })
        })
    };
    let flow = Flow::start("mockup-drawing", Options::default(), script).await;
    let heard_now = |name: &str| heard.lock().unwrap().contains_key(name);
    flow.say("Mock it.").await;

    // Its first turn draws it.
    super::eventually("the worker to start its mockup", || heard_now("first")).await;
    let board = flow.board().await;
    assert!(
        Flow::task(&board, 1).mockup_drawing,
        "its first turn draws it"
    );
    gate.notify_one();
    let board = flow
        .until("the mockup's screen is kept", |board| {
            let task = Flow::task(board, 1);
            task.state == TaskState::Reported && !task.outputs.is_empty()
        })
        .await;
    let task = Flow::task(&board, 1).clone();
    assert!(!task.mockup_drawing, "its first turn is over");

    // A round of notes, its turn held open: it draws while that turn runs.
    flow.manager
        .send_mockup_notes(
            task.id.clone(),
            "Notes on the mockup (version 1): make it blue.".into(),
            Vec::new(),
        )
        .await
        .unwrap();
    super::eventually("the worker to hear the notes", || heard_now("notes")).await;
    let board = flow.board().await;
    assert_eq!(Flow::task(&board, 1).rework_rounds, 1);
    assert!(
        Flow::task(&board, 1).mockup_drawing,
        "the notes' turn draws it"
    );
    gate.notify_one();
    let board = flow
        .until("the revision is reported", |board| {
            Flow::task(board, 1).state == TaskState::Reported
        })
        .await;
    assert!(
        !Flow::task(&board, 1).mockup_drawing,
        "the revision is drawn"
    );

    // A round whose turn is over at once: its end, however soon, leaves it drawn, never marked
    // drawing after it.
    flow.manager
        .send_mockup_notes(
            task.id.clone(),
            "Notes on the mockup (version 2): a little darker.".into(),
            Vec::new(),
        )
        .await
        .unwrap();
    super::eventually("the worker to hear the second notes", || {
        heard_now("notes again")
    })
    .await;
    let board = flow
        .until("the second revision is reported", |board| {
            let task = Flow::task(board, 1);
            task.state == TaskState::Reported && task.rework_rounds == 2
        })
        .await;
    assert!(
        !Flow::task(&board, 1).mockup_drawing,
        "the second revision is drawn"
    );
    flow.settled().await;
    assert!(
        !Flow::task(&flow.board().await, 1).mockup_drawing,
        "and stays drawn"
    );

    // The same worker's later work, as `brigadier send` gives it: it runs and draws no mockup.
    let worker = Flow::task(&flow.board().await, 1).clone();
    flow.manager
        .message_worker(
            &worker.conversation_id,
            &worker,
            "Build the approved design for real.".into(),
        )
        .await
        .unwrap();
    super::eventually("the worker to take the build", || heard_now("build")).await;
    let board = flow
        .until("the worker runs the build", |board| {
            Flow::task(board, 1).state == TaskState::Running
        })
        .await;
    assert!(
        !Flow::task(&board, 1).mockup_drawing,
        "other work draws no mockup"
    );
    gate.notify_one();
    let board = flow
        .until("the build is reported", |board| {
            Flow::task(board, 1).state == TaskState::Reported
        })
        .await;
    assert!(!Flow::task(&board, 1).mockup_drawing);
    assert_eq!(Flow::task(&board, 1).rework_rounds, 2);
    flow.stop().await;
}

/// A notes turn a restart cuts off goes on by itself, still drawing; cut off again, it waits
/// for its session's message, and that message's turn draws no mockup (#636).
#[tokio::test]
async fn a_notes_turn_cut_off_twice_leaves_the_mockup_drawn() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let heard = Arc::new(Mutex::new(Vec::<String>::new()));
    let script: Script = {
        let heard = heard.clone();
        Arc::new(move |turn| {
            let heard = heard.clone();
            Box::pin(async move {
                let worker = turn
                    .env(GRANT_ENV)
                    .and_then(|grant| turn.host.grants().resolve(grant));
                if let Some(Role::Worker { .. }) = worker {
                    heard.lock().unwrap().push(turn.input.clone());
                    // The notes' turn, and its going on after a restart, run until cut off.
                    if turn.input.starts_with("Notes on the mockup")
                        || turn
                            .input
                            .contains("Brigadier restarted while you were working")
                    {
                        let stops: &AtomicBool = &turn.stops.0;
                        while !stops.load(Ordering::SeqCst) {
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        }
                        return Reply::text("Cut off.");
                    }
                    if turn.input.starts_with("Go on") {
                        return Reply::text("Went on.");
                    }
                    let folder = turn
                        .input
                        .split_once("mockup-2.png… in ")
                        .and_then(|(_, rest)| rest.split_once("; they show"))
                        .map(|(folder, _)| std::path::PathBuf::from(folder))
                        .expect("the folder is named");
                    std::fs::write(folder.join("mockup-1.png"), b"png").unwrap();
                    return Reply::text("Made it.");
                }
                let started = start_mockup(&turn, "Mock the density toggle.").await;
                assert!(started.get("error").is_none(), "{started:#}");
                Reply::text("Started the mockup.")
            })
        })
    };
    let mut flow = Flow::start("mockup-drawing-cut", Options::default(), script).await;
    let heard_with = |words: &str| {
        heard
            .lock()
            .unwrap()
            .iter()
            .filter(|input| input.contains(words))
            .count()
    };
    flow.say("Mock it.").await;
    super::eventually("the worker to start its mockup", || {
        !heard.lock().unwrap().is_empty()
    })
    .await;
    let board = flow
        .until("the mockup's screen is kept", |board| {
            let task = Flow::task(board, 1);
            task.state == TaskState::Reported && !task.outputs.is_empty()
        })
        .await;
    let task = Flow::task(&board, 1).clone();
    flow.manager
        .send_mockup_notes(
            task.id.clone(),
            "Notes on the mockup (version 1): make it blue.".into(),
            Vec::new(),
        )
        .await
        .unwrap();
    super::eventually("the worker to hear the notes", || {
        heard_with("Notes on the mockup") == 1
    })
    .await;
    flow.settled().await;

    // Cut off once: the notes' turn goes on by itself, still drawing.
    flow.crash().await;
    super::eventually("the notes' turn to go on", || {
        heard_with("Brigadier restarted while you were working") == 1
    })
    .await;
    let board = flow.settled().await;
    assert!(
        Flow::task(&board, 1).mockup_drawing,
        "the notes' turn goes on drawing"
    );

    // Cut off again: it waits for a message, its turn over.
    flow.crash().await;
    let board = flow
        .until("the worker waits for a message", |board| {
            Flow::task(board, 1).state == TaskState::Reported
        })
        .await;
    assert!(
        !Flow::task(&board, 1).mockup_drawing,
        "a turn cut off for good draws no more"
    );
    assert_eq!(heard_with("Brigadier restarted while you were working"), 1);
    flow.stop().await;
}
