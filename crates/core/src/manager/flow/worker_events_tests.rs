//! STRIP-DOWN-PLAN S29 (O1): the parent hears everything of its workers, once. A `brigadier
//! wait` it keeps running in the background takes their events; with none running, the
//! safety net sends them in one message once its turn ended. Neither hears an event the other
//! took, and none is lost or heard twice across a restart, a crash or a seam.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_cli::protocol::{CliCall, CliReply, GRANT_ENV, PROTOCOL, SESSION_ENV};
use brigadier_providers::{
    ItemStatus, McpServerState, McpStatus, ProviderEvent, ProviderKind, Role, TurnStatus,
};
use serde_json::Value;

use super::cli_tests::{brigadier, wait};
use super::{FakeBehavior, Flow, Options, Reply, Script, Turn};
use crate::manager::seam::SeamConfig;
use crate::model::PermissionLevel;
use crate::thread_view::NUDGE_MARK;

/// What the test sees of a flow's turns.
#[derive(Default)]
struct Seen {
    /// The parent's turns: its CLI session and what each was given.
    inputs: Mutex<Vec<(String, String)>>,
    /// The parent's `brigadier` caller: its session and grant.
    caller: Mutex<Option<(String, String)>>,
    /// What a `brigadier wait` the parent ran in the background printed.
    waited: Mutex<Vec<Value>>,
    /// Lets the hanging workers end.
    release: AtomicBool,
    /// A worker's turn that goes on after a restart hangs too.
    hold_resumes: AtomicBool,
    /// Every worker's input, in order.
    worker_inputs: Mutex<Vec<String>>,
}

impl Seen {
    /// The net's messages the parent was given, with its CLI session.
    fn nets(&self) -> Vec<(String, String)> {
        self.inputs
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, input)| input.contains("[worker events wn-"))
            .cloned()
            .collect()
    }

    /// Every (worker title, word) the net's messages told, in order.
    fn told(&self) -> Vec<(String, String)> {
        self.nets()
            .iter()
            .flat_map(|(_, input)| told_in(input))
            .collect()
    }

    /// The event ids the net's messages carried.
    fn net_ids(&self) -> Vec<String> {
        self.nets()
            .iter()
            .flat_map(|(_, input)| ids_in(input))
            .collect()
    }

    /// What every background `wait` printed, by event id.
    fn waited_ids(&self) -> Vec<String> {
        self.waited
            .lock()
            .unwrap()
            .iter()
            .flat_map(|got| got["events"].as_array().cloned().unwrap_or_default())
            .map(|event| event["eventId"].as_str().unwrap().to_owned())
            .collect()
    }
}

/// The (title, word) lines of one net message (a worker's title as given to `start`, without
/// the " task" the fake titles add).
fn told_in(input: &str) -> Vec<(String, String)> {
    let start = input.find("[worker events wn-").unwrap();
    let body = &input[start..];
    let body = &body[body.find("] Worker \"").unwrap() + 10..body.rfind(" (").unwrap()];
    body.split("; Worker \"")
        .map(|line| {
            let (title, rest) = line.split_once("\" (").unwrap();
            let word = rest.split_once("): ").unwrap().1;
            let word = word.split(" · ").next().unwrap();
            (title.trim_end_matches(" task").to_owned(), word.to_owned())
        })
        .collect()
}

/// The event ids one net message carries.
fn ids_in(input: &str) -> Vec<String> {
    let ids = &input[input.rfind(" (").unwrap() + 2..input.rfind(')').unwrap()];
    ids.split(", ").map(str::to_owned).collect()
}

fn script(seen: Arc<Seen>) -> Script {
    Arc::new(move |turn| {
        let seen = seen.clone();
        Box::pin(async move {
            if turn.worker().is_some() {
                return worker(&turn, &seen).await;
            }
            if turn.conversation().is_none() {
                return Reply::text("ok");
            }
            parent(turn, seen).await
        })
    })
}

/// A worker does what its prompt names.
async fn worker(turn: &Turn, seen: &Seen) -> Reply {
    let said = turn.input.clone();
    seen.worker_inputs.lock().unwrap().push(said.clone());
    if said.contains("Ask-first.") {
        return match turn.ask_approval("curl https://example.com", "curl").await {
            Some(decision) => Reply::text(format!("Answered: {decision:?}.")),
            None => Reply::text("No answer."),
        };
    }
    if said.contains("Fail-now.") {
        return Reply::failed("The API said no.");
    }
    if said.contains("Crash-now.") {
        return Reply::crashed(9);
    }
    if said.contains("Limit-now.") {
        return Reply::limited();
    }
    if said.contains("Compact-now.") {
        turn.emit(ProviderEvent::CompactionStarted { automatic: true })
            .await;
        return Reply::text("Compacted, then finished.");
    }
    if said.contains("Sign-in.") {
        turn.emit(ProviderEvent::McpServers {
            servers: vec![McpServerState {
                name: "linear".into(),
                status: McpStatus::NeedsAuth,
                tools: Vec::new(),
                transport: None,
            }],
            at_start: true,
        })
        .await;
        return Reply::text("Signed out of linear; finished the rest.");
    }
    let resumed = said.contains("Brigadier restarted while you were working");
    if said.contains("Hang-on.") || (resumed && seen.hold_resumes.load(Ordering::SeqCst)) {
        turn.emit(ProviderEvent::Command {
            item_id: "hang".into(),
            command: "sleep 600".into(),
            cwd: None,
            status: ItemStatus::InProgress,
            exit_code: None,
            output: None,
            duration_ms: None,
            kept: None,
        })
        .await;
        while !seen.release.load(Ordering::SeqCst) && !turn.stops.0.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        return Reply::text("Released.");
    }
    if said.contains("Commit-and-test.") {
        turn.write("feature.txt", "done\n");
        turn.git(&["add", "feature.txt"]);
        turn.git(&["commit", "-q", "-m", "Add the feature"]);
        turn.write("scratch.txt", "left behind\n");
        for (status, exit) in [
            (ItemStatus::InProgress, None),
            (ItemStatus::Failed, Some(101)),
        ] {
            turn.emit(ProviderEvent::Command {
                item_id: "test".into(),
                command: "cargo test -p demo".into(),
                cwd: None,
                status,
                exit_code: exit,
                output: exit.map(|_| "error: test failed".to_owned()),
                duration_ms: None,
                kept: None,
            })
            .await;
        }
        return Reply::text("All green, done.");
    }
    if said.contains("Soon.") {
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    Reply::text("Finished it.")
}

/// The parent: `Start A.|B.` starts a worker per prompt (titled by it); ` then hold` keeps
/// its turn running until its CLI closes; ` then wait` runs `brigadier wait` in the
/// background as Claude does, the CLI starting a turn of its own once it ends. A seam's
/// nudge writes the note; `Grow.` passes the seam's threshold.
async fn parent(turn: Turn, seen: Arc<Seen>) -> Reply {
    seen.inputs
        .lock()
        .unwrap()
        .push((turn.native_id.clone(), turn.input.clone()));
    *seen.caller.lock().unwrap() = Some((
        turn.env(SESSION_ENV).unwrap().to_owned(),
        turn.env(GRANT_ENV).unwrap().to_owned(),
    ));
    if turn.input.starts_with(NUDGE_MARK) {
        let note = turn
            .input
            .split("write the hand-off note at ")
            .nth(1)
            .and_then(|rest| rest.split(": doing now").next())
            .unwrap();
        std::fs::write(note, "Doing now: nothing.\nNext: wait for the workers.").unwrap();
        return Reply::text("Handed off.");
    }
    let said = crate::manager::seam::without_handoff(&turn.input);
    if said.contains("Grow.") {
        turn.report_context(70_000).await;
        turn.until_steered().await;
        return Reply::text("Grown.");
    }
    let Some(line) = said
        .lines()
        .find_map(|line| line.trim().strip_prefix("Start "))
    else {
        return Reply::text("Heard.");
    };
    let (line, hold) = match line.strip_suffix(" then hold") {
        Some(line) => (line, true),
        None => (line, false),
    };
    let (line, background) = match line.strip_suffix(" then wait") {
        Some(line) => (line, true),
        None => (line, false),
    };
    for prompt in line.split('|') {
        turn.start_worker(prompt.trim_end_matches('.'), prompt)
            .await;
    }
    if background {
        wait_in_background(&turn, seen).await;
        return Reply::text("Waiting in the background.");
    }
    if hold {
        turn.stopped().await;
    }
    Reply::text("Started.")
}

/// Runs `brigadier wait` as a background task of the turn's Claude CLI: once it ends the CLI
/// starts a turn of its own with its result.
async fn wait_in_background(turn: &Turn, seen: Arc<Seen>) {
    let events = turn.events.clone();
    let _ = events
        .send(ProviderEvent::BackgroundJobs {
            running: vec!["brigadier wait".into()],
        })
        .await;
    let turn = turn.clone();
    tokio::spawn(async move {
        let got = brigadier(&turn, wait(&[], None)).await;
        let text = format!("The wait printed: {got}");
        seen.waited.lock().unwrap().push(got);
        for event in [
            ProviderEvent::BackgroundJobs {
                running: Vec::new(),
            },
            ProviderEvent::TurnStarted { turn_id: None },
            ProviderEvent::Message {
                item_id: "after-wait".into(),
                role: Role::Assistant,
                text,
            },
            ProviderEvent::TurnCompleted {
                turn_id: None,
                status: TurnStatus::Completed,
                duration_ms: Some(1),
                usage: None,
            },
        ] {
            let _ = events.send(event).await;
        }
    });
}

fn options(permission: PermissionLevel) -> Options {
    Options {
        permission,
        behavior: Arc::new(FakeBehavior {
            session_file: true,
            take_in_steers: true,
            ..FakeBehavior::default()
        }),
        ..Options::default()
    }
}

async fn flow(name: &str, permission: PermissionLevel) -> (Flow, Arc<Seen>) {
    let seen = Arc::new(Seen::default());
    let flow = Flow::start(name, options(permission), script(seen.clone())).await;
    (flow, seen)
}

/// Long enough for the net to have sent anything it would (five times its delay).
async fn settle_net() {
    tokio::time::sleep(Duration::from_millis(1500)).await;
}

/// The parent's events file.
fn events_file(flow: &Flow) -> String {
    let file = flow
        .manager
        .session_dir(&flow.conversation)
        .record()
        .join(crate::manager::worker_events::FILE);
    std::fs::read_to_string(file).unwrap_or_default()
}

/// The lines of the events file of kind `line`.
fn lines_of(flow: &Flow, line: &str) -> Vec<Value> {
    events_file(flow)
        .lines()
        .filter_map(|text| serde_json::from_str::<Value>(text).ok())
        .filter(|value| value["line"] == line)
        .collect()
}

/// Waits until the events file has `n` events.
async fn until_events(flow: &Flow, n: usize) -> Vec<Value> {
    super::eventually(&format!("{n} worker events kept"), || {
        lines_of(flow, "event").len() >= n
    })
    .await;
    lines_of(flow, "event")
}

async fn until_nets(seen: &Seen, n: usize) {
    super::eventually(&format!("{n} net messages"), || seen.nets().len() >= n).await;
}

/// The parent's `brigadier wait`, called from outside its turns: answered, its events not
/// yet heard.
async fn wait_held(
    flow: &Flow,
    seen: &Seen,
    timeout_secs: u32,
) -> (CliReply, Option<crate::manager::HeldEvents>) {
    let (session, grant) = seen.caller.lock().unwrap().clone().unwrap();
    let caller = flow.manager.cli_caller(&session, &grant).unwrap();
    flow.manager
        .cli_call_held(
            &caller,
            CliCall {
                protocol: PROTOCOL,
                request: wait(&[], Some(timeout_secs)),
            },
        )
        .await
}

fn event_ids(reply: &CliReply) -> Vec<String> {
    match reply {
        CliReply::Ok { output } => output["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["eventId"].as_str().unwrap().to_owned())
            .collect(),
        CliReply::Error { error } => panic!("wait: {error:?}"),
    }
}

/// The plan's first: the parent ends its turn with a `wait` running in the background; its
/// worker finishes; the CLI's next turn starts with the event, once, and the net sends
/// nothing.
#[tokio::test]
async fn a_background_wait_takes_the_event_and_the_net_sends_nothing() {
    let (flow, seen) = flow("events-bg-wait", PermissionLevel::Full).await;
    flow.say("Start Soon. then wait").await;
    super::eventually("the background wait to end", || {
        !seen.waited.lock().unwrap().is_empty()
    })
    .await;
    flow.settled().await;
    settle_net().await;

    let waited = seen.waited.lock().unwrap().clone();
    let events = waited[0]["events"].as_array().unwrap();
    assert_eq!(events.len(), 1, "{waited:#?}");
    assert_eq!(events[0]["event"], "turn-ended");
    assert_eq!(events[0]["message"], "Finished it.");
    assert!(seen.nets().is_empty(), "{:#?}", seen.nets());
    // The CLI's own turn after the wait shows its result.
    let view = flow
        .core
        .conversation_view(flow.conversation.clone(), 50)
        .await
        .unwrap();
    assert!(
        view.messages
            .messages
            .iter()
            .any(|message| message.text.starts_with("The wait printed:")),
    );
    let id = events[0]["eventId"].as_str().unwrap();
    let done = lines_of(&flow, "done");
    assert_eq!(done.len(), 1, "{}", events_file(&flow));
    assert_eq!(done[0]["ids"][0], id);
    assert!(done[0]["by"].as_str().unwrap().starts_with("wait:"));
    flow.stop().await;
}

/// The plan's second: no `wait` runs; a worker asks permission; the parent gets one message
/// with it, once, and nothing more while the prompt stays open.
#[tokio::test]
async fn with_no_wait_a_workers_prompt_reaches_the_parent_in_one_message_once() {
    let (flow, seen) = flow("events-net-asks", PermissionLevel::AcceptEdits).await;
    flow.say("Start Ask-first.").await;
    until_nets(&seen, 1).await;
    settle_net().await;

    let nets = seen.nets();
    assert_eq!(nets.len(), 1, "{nets:#?}");
    assert_eq!(seen.told(), [("Ask-first".into(), "asks".into())]);
    assert!(nets[0].1.contains("asks · curl"), "{}", nets[0].1);
    flow.stop().await;
}

/// The plan's third: an event kept while the parent works survives a crash, and the net
/// sends it once after it, and not again after another restart.
#[tokio::test]
async fn an_event_survives_a_crash_and_is_heard_once_after_it() {
    let (mut flow, seen) = flow("events-crash-pending", PermissionLevel::Full).await;
    flow.say("Start Now. then hold").await;
    let kept = until_events(&flow, 1).await;
    let id = kept[0]["id"].as_str().unwrap().to_owned();
    assert!(seen.nets().is_empty());

    flow.crash().await;
    until_nets(&seen, 1).await;
    flow.settled().await;
    assert_eq!(seen.net_ids(), std::slice::from_ref(&id));
    flow.restart().await;
    settle_net().await;
    assert_eq!(seen.net_ids(), [id], "{:#?}", seen.nets());
    flow.stop().await;
}

/// A wait got the event but its `brigadier` never said it printed it, and the daemon
/// crashed: the event is the parent's again, and the net sends it once.
#[tokio::test]
async fn an_event_handed_to_a_wait_that_never_printed_it_comes_again_after_a_crash() {
    let (mut flow, seen) = flow("events-crash-handed", PermissionLevel::Full).await;
    flow.say("Start Now. then hold").await;
    until_events(&flow, 1).await;
    let (reply, held) = wait_held(&flow, &seen, 5).await;
    let ids = event_ids(&reply);
    assert_eq!(ids.len(), 1);
    // Its `brigadier` never says it printed them.
    assert!(held.is_some());

    flow.crash().await;
    until_nets(&seen, 1).await;
    settle_net().await;
    assert_eq!(seen.net_ids(), ids);
    flow.stop().await;
}

/// A wait printed the event and said so: after a crash nothing comes again.
#[tokio::test]
async fn an_event_a_wait_printed_is_not_heard_again_after_a_crash() {
    let (mut flow, seen) = flow("events-crash-heard", PermissionLevel::Full).await;
    flow.say("Start Now. then hold").await;
    until_events(&flow, 1).await;
    let (reply, held) = wait_held(&flow, &seen, 5).await;
    assert_eq!(event_ids(&reply).len(), 1);
    flow.manager.events_heard(held.unwrap());

    flow.crash().await;
    flow.settled().await;
    settle_net().await;
    assert!(seen.nets().is_empty(), "{:#?}", seen.nets());
    flow.stop().await;
}

/// The net's message reached the parent's CLI but the daemon died before it wrote so: after
/// the crash its CLI's own session file shows it was taken, and it is not sent again.
#[tokio::test]
async fn a_net_message_the_cli_took_is_not_sent_again_after_a_crash() {
    let (mut flow, seen) = flow("events-net-taken", PermissionLevel::Full).await;
    flow.say("Start Now.").await;
    until_nets(&seen, 1).await;
    flow.settled().await;
    let message = seen.nets()[0]
        .1
        .split("[worker events ")
        .nth(1)
        .unwrap()
        .split(']')
        .next()
        .unwrap()
        .to_owned();
    super::eventually("the net's message to be done", || {
        lines_of(&flow, "done")
            .iter()
            .any(|done| done["by"] == format!("net:{message}"))
    })
    .await;

    flow.crash().await;
    // As if the old daemon died before it wrote the done line.
    let file = flow
        .manager
        .session_dir(&flow.conversation)
        .record()
        .join(crate::manager::worker_events::FILE);
    let text = std::fs::read_to_string(&file).unwrap();
    let without: Vec<&str> = text
        .lines()
        .filter(|line| !(line.contains("\"done\"") && line.contains(&message)))
        .collect();
    std::fs::write(&file, without.join("\n") + "\n").unwrap();
    flow.restart().await;
    settle_net().await;
    assert_eq!(seen.nets().len(), 1, "{:#?}", seen.nets());
    assert!(
        lines_of(&flow, "done")
            .iter()
            .any(|done| done["by"] == format!("net:{message}")),
        "{}",
        events_file(&flow)
    );
    flow.stop().await;
}

/// The net held an event for a message its CLI never took (the daemon died first): after the
/// crash it is sent once, in a message of its own.
#[tokio::test]
async fn a_net_message_the_cli_never_took_is_sent_once_after_a_crash() {
    let (mut flow, seen) = flow("events-net-lost", PermissionLevel::Full).await;
    flow.say("Start Now. then hold").await;
    let kept = until_events(&flow, 1).await;
    let id = kept[0]["id"].as_str().unwrap().to_owned();
    let file = flow
        .manager
        .session_dir(&flow.conversation)
        .record()
        .join(crate::manager::worker_events::FILE);
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str(&format!(
        "{{\"line\":\"held\",\"ids\":[\"{id}\"],\"by\":\"net:wn-000000000000\"}}\n"
    ));
    std::fs::write(&file, text).unwrap();

    flow.crash().await;
    until_nets(&seen, 1).await;
    settle_net().await;
    assert_eq!(seen.net_ids(), [id]);
    assert!(!seen.nets()[0].1.contains("wn-000000000000"));
    flow.stop().await;
}

/// Two waits run at once: the event goes to one of them, and the net sends nothing.
#[tokio::test]
async fn two_waits_at_once_hear_an_event_once_between_them() {
    let (flow, seen) = flow("events-two-waits", PermissionLevel::Full).await;
    flow.say("Start Hang-on.").await;
    flow.settled().await;
    let flow = Arc::new(flow);
    let seen2 = seen.clone();
    let both: Vec<_> = (0..2)
        .map(|_| {
            let (flow, seen) = (flow.clone(), seen2.clone());
            tokio::spawn(async move {
                let (reply, held) = wait_held(&flow, &seen, 3).await;
                if let Some(held) = held {
                    flow.manager.events_heard(held);
                }
                event_ids(&reply)
            })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(100)).await;
    seen.release.store(true, Ordering::SeqCst);
    let mut got = Vec::new();
    for wait in both {
        got.extend(wait.await.unwrap());
    }
    settle_net().await;
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(seen.nets().is_empty(), "{:#?}", seen.nets());
    Arc::try_unwrap(flow).ok().unwrap().stop().await;
}

/// A wait the parent starts just after its turn ended, before or after the net's delay:
/// each event is heard once, by the wait or by the net.
#[tokio::test]
async fn a_wait_started_late_and_the_net_hear_each_event_once() {
    let (flow, seen) = flow("events-late-wait", PermissionLevel::Full).await;
    let mut waited = Vec::new();
    for (n, delay) in [0_u64, 150, 300, 450].into_iter().enumerate() {
        flow.say("Start Now.").await;
        until_events(&flow, n + 1).await;
        tokio::time::sleep(Duration::from_millis(delay)).await;
        let (reply, held) = wait_held(&flow, &seen, 2).await;
        if let Some(held) = held {
            flow.manager.events_heard(held);
        }
        waited.extend(event_ids(&reply));
        flow.settled().await;
        settle_net().await;
    }
    let kept: Vec<String> = lines_of(&flow, "event")
        .iter()
        .map(|event| event["id"].as_str().unwrap().to_owned())
        .collect();
    let mut heard = waited.clone();
    heard.extend(seen.net_ids());
    heard.sort();
    let mut all = kept.clone();
    all.sort();
    assert_eq!(heard, all, "waited {waited:?}, nets {:#?}", seen.nets());
    flow.stop().await;
}

/// The parent seams while its worker runs: the worker's event reaches the successor's CLI
/// session, once.
#[tokio::test]
async fn an_event_after_the_parents_seam_reaches_its_successor_once() {
    let (mut flow, seen) = flow("events-seam", PermissionLevel::Full).await;
    let config = SeamConfig {
        tokens: 60_000,
        hold: Duration::from_millis(200),
        long: None,
        tokens_set: false,
    };
    flow.seam = Some(config);
    flow.manager.set_seam(config);
    flow.say("Start Hang-on.").await;
    flow.settled().await;
    let first = seen.inputs.lock().unwrap()[0].0.clone();
    flow.say("Grow.").await;
    super::eventually("the successor's turn", || {
        seen.inputs
            .lock()
            .unwrap()
            .iter()
            .any(|(native, _)| *native != first)
    })
    .await;
    flow.idle().await;
    seen.release.store(true, Ordering::SeqCst);
    until_nets(&seen, 1).await;
    settle_net().await;

    let nets = seen.nets();
    assert_eq!(nets.len(), 1, "{nets:#?}");
    assert_ne!(
        nets[0].0, first,
        "the net went to the CLI session before the seam"
    );
    assert_eq!(seen.told(), [("Hang-on".into(), "finished".into())]);
    flow.stop().await;
}

/// O1's faults with no wait running: a permission prompt, a failed turn, a CLI killed, a
/// usage limit, a compaction, a server wanting a sign-in and a silent command each reach the
/// idle parent once; a restart cutting off the silent one and the one asking goes on by itself
/// (D9 a), each finishing once.
#[tokio::test]
async fn every_fault_of_a_worker_reaches_an_idle_parent_once() {
    let (mut flow, seen) = flow("events-faults", PermissionLevel::AcceptEdits).await;
    flow.manager.set_worker_quiet(Duration::from_millis(400));
    flow.say("Start Ask-first.|Fail-now.|Crash-now.|Limit-now.|Compact-now.|Sign-in.|Hang-on.")
        .await;
    let expected: Vec<(String, String)> = [
        ("Ask-first", "asks"),
        ("Fail-now", "failed"),
        ("Crash-now", "ended"),
        ("Limit-now", "hit a usage limit"),
        ("Compact-now", "compacted its context"),
        ("Compact-now", "finished"),
        ("Sign-in", "asks"),
        ("Sign-in", "finished"),
        ("Hang-on", "is quiet"),
    ]
    .iter()
    .map(|(title, word)| ((*title).to_owned(), (*word).to_owned()))
    .collect();
    super::eventually("every fault told", || {
        let told = seen.told();
        expected.iter().all(|want| told.contains(want))
    })
    .await;
    flow.settled().await;
    settle_net().await;
    let mut told = seen.told();
    told.sort();
    let mut want = expected.clone();
    want.sort();
    assert_eq!(told, want, "{:#?}", seen.nets());
    let nets = seen.nets();
    assert!(
        nets.iter()
            .any(|(_, input)| input.contains("sign in to MCP server linear")),
        "{nets:#?}"
    );
    assert!(
        nets.iter()
            .any(|(_, input)| input.contains("running: `sleep 600`")),
        "{nets:#?}"
    );

    // The restart cuts off the silent worker and the one waiting on its prompt: they go on.
    flow.crash().await;
    let cut_off = [
        ("Ask-first".to_owned(), "finished".to_owned()),
        ("Hang-on".to_owned(), "finished".to_owned()),
    ];
    super::eventually("the cut-off workers told", || {
        let told = seen.told();
        cut_off.iter().all(|want| told.contains(want))
    })
    .await;
    flow.settled().await;
    settle_net().await;
    let told = seen.told();
    for want in &cut_off {
        let times = told.iter().filter(|told| *told == want).count();
        assert_eq!(times, 1, "{want:?}: {:#?}", seen.nets());
    }
    assert_eq!(told.len(), expected.len() + 2, "{:#?}", seen.nets());
    flow.stop().await;
}

/// How many of the workers' inputs said `what`.
fn worker_inputs(seen: &Seen, what: &str) -> usize {
    seen.worker_inputs
        .lock()
        .unwrap()
        .iter()
        .filter(|input| input.contains(what))
        .count()
}

/// Starts the `Hang-on` worker and waits until it works.
async fn hanging_worker(flow: &Flow) {
    flow.say("Start Hang-on.").await;
    flow.until("the worker works", |board| {
        board
            .tasks
            .values()
            .any(|task| task.state == crate::work::TaskState::Running)
    })
    .await;
    flow.settled().await;
}

/// A worker cut off by a crash, or by a quit that closed its CLI, goes on by itself once, told
/// to check git status and git log first (D9 a): its parent hears it finish, never that it
/// stopped.
#[tokio::test]
async fn a_worker_cut_off_by_a_restart_goes_on_by_itself() {
    for crash in [true, false] {
        let (mut flow, seen) = flow("events-restart-resume", PermissionLevel::Full).await;
        hanging_worker(&flow).await;
        if crash {
            flow.crash().await;
        } else {
            flow.restart().await;
        }
        let finished = ("Hang-on".to_owned(), "finished".to_owned());
        super::eventually("the worker finished", || seen.told().contains(&finished)).await;
        let board = flow.settled().await;
        settle_net().await;
        let told = seen.told();
        assert!(
            !told.iter().any(|(_, word)| word == "stopped"),
            "crash {crash}: {told:#?}"
        );
        assert_eq!(worker_inputs(&seen, "Hang-on."), 1, "crash {crash}");
        assert_eq!(
            worker_inputs(&seen, "check git status and git log first"),
            1,
            "crash {crash}"
        );
        let task = Flow::task(&board, 1);
        assert_eq!(task.state, crate::work::TaskState::Reported);
        assert!(
            !task.plain.as_ref().unwrap().auto_resumed,
            "its turn finished"
        );
        flow.stop().await;
    }
}

/// Cut off again while it went on by itself (by a crash, or a quit closing its CLI): it waits
/// for its session's next message, as an interrupted turn leaves it, never paused for good;
/// its parent hears it stopped once, with how it goes on, and a message carries it on (#559).
#[tokio::test]
async fn a_worker_cut_off_twice_goes_on_when_its_session_sends_it_more() {
    for crash in [true, false] {
        let (mut flow, seen) = flow("events-restart-cut", PermissionLevel::Full).await;
        seen.hold_resumes.store(true, Ordering::SeqCst);
        hanging_worker(&flow).await;
        flow.crash().await;
        super::eventually("the worker going on", || {
            worker_inputs(&seen, "Brigadier restarted while you were working") == 1
        })
        .await;
        flow.settled().await;
        if crash {
            flow.crash().await;
        } else {
            flow.restart().await;
        }
        let stopped = ("Hang-on".to_owned(), "stopped".to_owned());
        super::eventually("the parent told", || seen.told().contains(&stopped)).await;
        let board = flow.settled().await;
        let task = Flow::task(&board, 1).clone();
        assert_eq!(
            task.state,
            crate::work::TaskState::Reported,
            "crash {crash}"
        );
        assert_eq!(task.blocked_reason, None);
        let nets = seen.nets();
        assert!(
            nets.iter()
                .any(|(_, input)| input.contains("Send it a message to carry on.")),
            "{nets:#?}"
        );
        assert_eq!(
            worker_inputs(&seen, "Brigadier restarted while you were working"),
            1,
            "crash {crash}: went on by itself twice"
        );
        // What git and its CLI showed of the turn cut off, the one that went on (harness P1).
        assert!(
            nets.iter().any(|(_, input)| input.contains("in turn 2")
                && input.contains("full facts: `brigadier show")),
            "{nets:#?}"
        );

        seen.hold_resumes.store(false, Ordering::SeqCst);
        flow.manager
            .message_worker(&flow.conversation, &task, "Go on.".into())
            .await
            .unwrap();
        let finished = ("Hang-on".to_owned(), "finished".to_owned());
        super::eventually("the worker finished", || seen.told().contains(&finished)).await;
        flow.settled().await;
        settle_net().await;
        let told = seen.told();
        assert_eq!(
            told.iter().filter(|told| **told == stopped).count(),
            1,
            "{told:#?}"
        );
        flow.stop().await;
    }
}

/// A worker cut off before its CLI session began starts again on its prompt, once, in the
/// worktree it had (D9 a).
#[tokio::test]
async fn a_worker_cut_off_as_it_starts_starts_again_in_its_worktree() {
    let (mut flow, seen) = flow("events-restart-starting", PermissionLevel::Full).await;
    flow.say("Start Quick.").await;
    // Its parent's turn, then its own CLI's start, which never goes on.
    super::eventually("the parent's turn", || {
        !seen.inputs.lock().unwrap().is_empty()
    })
    .await;
    let reached = Arc::new(tokio::sync::Notify::new());
    *flow.behavior.hold_start.lock().unwrap() =
        Some((reached.clone(), Arc::new(tokio::sync::Notify::new())));
    reached.notified().await;
    let starting = flow.board().await;
    let task = Flow::task(&starting, 1).clone();
    assert!(task.native_session.is_none());
    let worktree = task.workspace.clone().unwrap().worktree;
    flow.crash().await;
    let finished = ("Quick".to_owned(), "finished".to_owned());
    super::eventually("the worker finished", || seen.told().contains(&finished)).await;
    let board = flow.settled().await;
    assert_eq!(worker_inputs(&seen, "Quick."), 1);
    let again = Flow::task(&board, 1);
    assert_eq!(again.workspace.clone().unwrap().worktree, worktree);
    let worktree = worktree.expect("a worktree");
    // The worker's own start: its CLI runs in the session's worker folder, its worktree added.
    let started: Vec<_> = flow
        .specs()
        .into_iter()
        .map(|(_, spec)| spec)
        .filter(|spec| {
            spec.env
                .iter()
                .any(|(key, value)| key == SESSION_ENV && *value == task.id.0)
        })
        .collect();
    assert_eq!(started.len(), 1, "{started:#?}");
    let worktree = std::path::Path::new(&worktree);
    assert!(
        started[0].cwd == worktree || started[0].add_dirs.iter().any(|dir| dir == worktree),
        "{:?} {:?} vs {worktree:?}",
        started[0].cwd,
        started[0].add_dirs
    );
    flow.stop().await;
}

/// A Codex parent: its `brigadier wait` still runs when its turn ends; its end starts the
/// parent's next turn with what it printed, and the net sends nothing.
#[tokio::test]
async fn a_codex_parents_background_wait_starts_its_next_turn() {
    let seen = Arc::new(Seen::default());
    let log = seen.clone();
    let codex: Script = Arc::new(move |turn| {
        let seen = log.clone();
        Box::pin(async move {
            if turn.worker().is_some() {
                return worker(&turn, &seen).await;
            }
            seen.inputs
                .lock()
                .unwrap()
                .push((turn.native_id.clone(), turn.input.clone()));
            if !turn.input.contains("Start Soon.") {
                return Reply::text("Heard.");
            }
            turn.start_worker("Soon", "Soon.").await;
            let events = turn.events.clone();
            let command = |status, output: Option<String>| ProviderEvent::Command {
                item_id: "wait-1".into(),
                command: "brigadier wait".into(),
                cwd: None,
                status,
                exit_code: output.as_ref().map(|_| 0),
                output,
                duration_ms: None,
                kept: None,
            };
            let _ = events.send(command(ItemStatus::InProgress, None)).await;
            let bg = turn.clone();
            tokio::spawn(async move {
                let got = brigadier(&bg, wait(&[], None)).await;
                seen.waited.lock().unwrap().push(got.clone());
                let _ = events
                    .send(command(ItemStatus::Completed, Some(got.to_string())))
                    .await;
            });
            Reply::text("Started one; waiting in the background.")
        })
    });
    let flow = Flow::start(
        "events-codex-wait",
        Options {
            thread: ProviderKind::Codex,
            ..Options::default()
        },
        codex,
    )
    .await;
    flow.say("Start Soon.").await;
    super::eventually("the turn the wait's end started", || {
        seen.inputs
            .lock()
            .unwrap()
            .iter()
            .any(|(_, input)| input.contains("[your background command `brigadier wait` finished"))
    })
    .await;
    flow.settled().await;
    settle_net().await;
    assert_eq!(seen.waited_ids().len(), 1);
    let after = seen.inputs.lock().unwrap().clone();
    let woke: Vec<_> = after
        .iter()
        .filter(|(_, input)| input.contains("[your background command `brigadier wait` finished"))
        .collect();
    assert_eq!(woke.len(), 1, "{after:#?}");
    assert!(woke[0].1.contains(&seen.waited_ids()[0]), "{after:#?}");
    assert!(seen.nets().is_empty(), "{after:#?}");
    flow.stop().await;
}

/// A turn's end carries what git and the worker's CLI showed, beside its own words (harness
/// P1, D1): its commit not merged yet, the file it left uncommitted, the test run that failed
/// at that commit; `brigadier workers` shows the same and that no wait runs.
#[tokio::test]
async fn a_turns_end_carries_the_facts_git_and_the_cli_showed() {
    let (flow, seen) = flow("events-facts", PermissionLevel::Full).await;
    flow.say("Start Commit-and-test. then wait").await;
    super::eventually("the background wait to end", || {
        !seen.waited.lock().unwrap().is_empty()
    })
    .await;
    flow.settled().await;

    let waited = seen.waited.lock().unwrap().clone();
    let event = &waited[0]["events"][0];
    assert_eq!(event["message"], "All green, done.");
    let git = &event["git"];
    assert_eq!(git["merge"], "clean", "{event:#}");
    assert_eq!(
        (git["ahead"].as_u64(), git["behind"].as_u64()),
        (Some(1), Some(0))
    );
    assert_eq!(git["commits"][0]["subject"], "Add the feature");
    assert_eq!(git["uncommitted"], 1);
    assert_eq!(git["baseCheckoutClean"], true);
    let commands = &event["commands"];
    assert_eq!(
        (commands["turn"].as_u64(), commands["total"].as_u64()),
        (Some(1), Some(1))
    );
    let ran = &commands["commands"][0];
    assert_eq!(ran["command"], "cargo test -p demo");
    assert_eq!(ran["status"], "failed");
    assert_eq!(ran["exit"], 101);
    assert_eq!(
        ran["head"], git["commits"][0]["hash"],
        "it ran at the worker's commit"
    );

    let (session, grant) = seen.caller.lock().unwrap().clone().unwrap();
    let caller = flow.manager.cli_caller(&session, &grant).unwrap();
    let (reply, _) = flow
        .manager
        .cli_call_held(
            &caller,
            CliCall {
                protocol: PROTOCOL,
                request: brigadier_cli::protocol::CliRequest::Workers,
            },
        )
        .await;
    let CliReply::Ok { output } = reply else {
        panic!("workers: {reply:?}");
    };
    assert_eq!(output["waitRunning"], false);
    assert_eq!(output["workers"][0]["git"], *git);
    assert_eq!(output["workers"][0]["commands"], *commands);
    flow.stop().await;
}
