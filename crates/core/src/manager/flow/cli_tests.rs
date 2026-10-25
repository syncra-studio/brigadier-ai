//! The `brigadier` command's read commands as a session calls them (STRIP-DOWN-PLAN S13):
//! `sessions` lists the project's sidebar sessions and never a worker, `workers` only the
//! calling session's own workers, and a grant that isn't the named session's is refused. Each
//! answer fits its command's checked-in `--json` schema.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use brigadier_cli::protocol::{
    Agent, Answer, CliCall, CliReply, CliRequest, GRANT_ENV, PROTOCOL, Permission, SESSION_ENV,
    StartRequest,
};
use brigadier_providers::ProviderKind;
use serde_json::{Value, json};

use super::{FakeBehavior, Flow, Options, Reply, Script, Turn};
use crate::tools::Role;

/// A flow whose Claude CLIs keep session files, which `read` of a session reads.
fn with_session_files() -> Options {
    Options {
        behavior: Arc::new(FakeBehavior {
            session_file: true,
            ..FakeBehavior::default()
        }),
        ..Options::default()
    }
}

/// What the sessions' `brigadier` calls answered, by name.
pub(super) type Answers = Arc<Mutex<HashMap<String, Value>>>;

/// The id `brigadier` shows for a full id.
pub(super) fn short(id: &str) -> String {
    id[id.len() - 8..].to_owned()
}

/// Runs `brigadier <request>` as a command of the turn would: as the session and with the grant
/// its CLI's environment names. Checks the answer against the command's schema; an error answer
/// comes back as `{"error": …}`.
pub(super) async fn brigadier(turn: &Turn, request: CliRequest) -> Value {
    let session = turn.env(SESSION_ENV).expect("the CLI knows its session");
    let grant = turn
        .env(GRANT_ENV)
        .expect("the CLI has its session's grant");
    let command = match &request {
        CliRequest::Models => "models",
        CliRequest::Sessions { .. } => "sessions",
        CliRequest::Workers => "workers",
        CliRequest::Show { .. } => "show",
        CliRequest::Read { .. } => "read",
        CliRequest::Start(_) => "start",
        CliRequest::Send { .. } => "send",
        CliRequest::Wait { .. } => "wait",
        CliRequest::Close { .. } => "close",
        CliRequest::Queue { .. } => "queue",
        CliRequest::ContinueOn { .. } => "continue-on",
        CliRequest::Tool { tool, .. } => tool.command(),
        CliRequest::Ask { .. } => "ask",
        CliRequest::Why { .. } => "why",
        CliRequest::Context { .. } => "context",
    };
    let caller = turn
        .host
        .cli_caller(session, grant)
        .expect("the session's own grant is accepted");
    let call = CliCall {
        protocol: PROTOCOL,
        request,
    };
    let (schema, value) = match turn.host.cli_call(&caller, call).await {
        CliReply::Ok { output } => (
            brigadier_cli::commands::find(command).unwrap().schema,
            output,
        ),
        CliReply::Error { error } => (
            brigadier_cli::commands::ERROR_SCHEMA,
            json!({ "error": serde_json::to_value(error).unwrap() }),
        ),
    };
    let found = brigadier_cli::schema::violations(schema, &value);
    assert!(
        found.is_empty(),
        "{command}: {}\n{value:#}",
        found.join("\n")
    );
    value
}

/// What a command in the turn finds of `brigadier`'s environment: who calls, the grant, the
/// data folder and the first folder on `PATH`.
fn seen_env(turn: &Turn) -> Value {
    let path = turn.env("PATH").unwrap_or_default();
    json!({
        "session": turn.env(SESSION_ENV),
        "grant": turn.env(GRANT_ENV),
        "data dir": turn.env(brigadier_sandbox::DATA_DIR_ENV),
        "first on path": std::env::split_paths(path).next(),
        "grant is the turn's": turn.env(GRANT_ENV) == Some(turn.grant.as_str()),
    })
}

/// Each thread starts one worker with `brigadier start` when asked; the main session's thread
/// looks around with `brigadier` when asked; each worker calls `brigadier` as itself, then
/// answers.
fn looking(answers: Answers) -> Script {
    Arc::new(move |turn| {
        let answers = answers.clone();
        Box::pin(async move {
            let keep = |name: &str, value: Value| {
                answers.lock().unwrap().insert(name.into(), value);
            };
            let worker = turn
                .env(GRANT_ENV)
                .and_then(|grant| turn.host.grants().resolve(grant));
            if let Some(Role::Worker { task_id, .. }) = worker {
                keep(&format!("worker env {}", task_id.0), seen_env(&turn));
                keep(
                    "worker sessions",
                    brigadier(&turn, CliRequest::Sessions { all: false }).await,
                );
                keep(
                    "worker workers",
                    brigadier(&turn, CliRequest::Workers).await,
                );
                return Reply {
                    context_tokens: Some(42_000),
                    ..Reply::text("Reported: three lines.")
                };
            }
            let Some(conversation) = turn.conversation() else {
                return Reply::text("Noted.");
            };
            let me = conversation.0.clone();
            if turn.input.contains("Start a worker.") {
                let started = brigadier(
                    &turn,
                    CliRequest::Start(Box::new(StartRequest {
                        title: Some("Count the lines".into()),
                        agent: Some(Agent::Claude),
                        ..start("Count the lines in README.")
                    })),
                )
                .await;
                assert!(started.get("error").is_none(), "{started:#}");
                return Reply::text("Started a worker.");
            }
            if turn.input.contains("Look around.") {
                keep("thread env", seen_env(&turn));
                let sessions = brigadier(&turn, CliRequest::Sessions { all: false }).await;
                let workers = brigadier(&turn, CliRequest::Workers).await;
                let worker = workers["workers"][0]["id"].as_str().unwrap().to_owned();
                let other = sessions["sessions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row["caller"] == false)
                    .unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                let read = |id: &str, last, screen| CliRequest::Read {
                    id: id.into(),
                    last,
                    screen,
                };
                keep(
                    "show worker",
                    brigadier(&turn, CliRequest::Show { id: worker.clone() }).await,
                );
                keep(
                    "read worker",
                    brigadier(&turn, read(&worker, None, false)).await,
                );
                keep(
                    "read worker last",
                    brigadier(&turn, read(&worker, Some(3), false)).await,
                );
                keep(
                    "read worker screen",
                    brigadier(&turn, read(&worker, None, true)).await,
                );
                keep(
                    "show other",
                    brigadier(&turn, CliRequest::Show { id: other.clone() }).await,
                );
                keep(
                    "read other",
                    brigadier(&turn, read(&other, None, false)).await,
                );
                keep(
                    "read me screen",
                    brigadier(&turn, read(&short(&me), None, true)).await,
                );
                let elsewhere = answers.lock().unwrap()["elsewhere"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                keep(
                    "show elsewhere",
                    brigadier(&turn, CliRequest::Show { id: elsewhere }).await,
                );
                // The session's grant names this session only.
                assert_eq!(turn.host.cli_caller(&me, "brg_not_a_grant"), None);
                let task = turn.host.core.tasks(&conversation).await.unwrap()[0]
                    .id
                    .0
                    .clone();
                assert_eq!(turn.host.cli_caller(&task, &turn.grant), None);
                assert_eq!(turn.host.cli_caller("", &turn.grant), None);
                keep("sessions", sessions);
                keep("workers", workers);
                return Reply::text("Looked around.");
            }
            Reply::text("Noted.")
        })
    })
}

#[tokio::test]
async fn sessions_lists_sidebar_sessions_and_workers_only_the_callers_own() {
    let answers = Answers::default();
    let flow = Flow::start("cli-reads", with_session_files(), looking(answers.clone())).await;
    // Claude, whose fake keeps a session file for `read`.
    let docs = flow.add_session("Docs", None, ProviderKind::Claude).await;
    let elsewhere = flow
        .add_session("Elsewhere", Some("other"), ProviderKind::Claude)
        .await;

    for session in [&flow.conversation, &docs] {
        flow.say_to(session, "Start a worker.").await;
        flow.until_in(session, "its worker answered", |board| {
            board
                .tasks
                .values()
                .any(|task| task.state == crate::work::TaskState::Reported)
        })
        .await;
        // The report's turn too, so `read other` below sees the session's last answer.
        flow.idle_in(session).await;
    }
    answers
        .lock()
        .unwrap()
        .insert("elsewhere".into(), json!(short(&elsewhere.0)));
    flow.say("Look around.").await;
    flow.until("it looked around", |_| {
        answers.lock().unwrap().contains_key("sessions")
    })
    .await;
    let answers = answers.lock().unwrap().clone();

    let mine = Flow::task(&flow.board_of(&flow.conversation).await, 1).clone();
    let theirs = Flow::task(&flow.board_of(&docs).await, 1).clone();

    // Every thread and worker CLI is started knowing who it is for `brigadier`, with its own
    // live grant, the (non-default) data folder and `brigadier`'s folder first on `PATH`.
    let bin = flow.manager.config.daemon_exe.parent().unwrap().to_owned();
    let data = flow.manager.data_dir.clone();
    assert_eq!(
        answers["thread env"],
        json!({"session": flow.conversation.0, "grant is the turn's": true,
               "data dir": data, "first on path": bin,
               "grant": answers["thread env"]["grant"]})
    );
    // A plain worker's only grant is the one its `brigadier` calls carry.
    for task in [&mine, &theirs] {
        let seen = &answers[&format!("worker env {}", task.id.0)];
        assert_eq!(
            seen,
            &json!({"session": task.id.0, "grant is the turn's": seen["grant is the turn's"],
                    "data dir": data, "first on path": bin, "grant": seen["grant"]})
        );
        assert!(matches!(
            flow.manager.grants().resolve(seen["grant"].as_str().unwrap()),
            Some(Role::Worker { task_id, .. }) if task_id == task.id
        ));
    }

    // `sessions`: this project's two sidebar sessions, this one marked, no worker, nothing of
    // the other project.
    let sessions = &answers["sessions"];
    assert_eq!(sessions["project"], "Flow", "{sessions:#}");
    let rows = sessions["sessions"].as_array().unwrap();
    let ids: Vec<&str> = rows.iter().map(|row| row["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), 2, "{sessions:#}");
    assert!(ids.contains(&short(&flow.conversation.0).as_str()));
    assert!(ids.contains(&short(&docs.0).as_str()));
    for absent in [&elsewhere.0, &mine.id.0, &theirs.id.0] {
        assert!(!ids.contains(&short(absent).as_str()), "{absent} listed");
    }
    let me = rows
        .iter()
        .find(|row| row["id"] == short(&flow.conversation.0))
        .unwrap();
    assert_eq!(me["caller"], true);
    assert_eq!(me["workers"], 1);
    assert_eq!(
        me["title"],
        flow.core.conversation(&flow.conversation).unwrap().title
    );
    assert!(
        me["branch"]
            .as_str()
            .unwrap()
            .contains(&short(&flow.conversation.0))
    );
    // The sidebar has no row for a worker: workers are never conversations.
    assert!(
        flow.core
            .visible_catalog()
            .conversations
            .iter()
            .all(|conversation| conversation.id.0 != mine.id.0 && conversation.id.0 != theirs.id.0)
    );

    // `workers`: this session's worker only.
    let workers = &answers["workers"];
    assert_eq!(workers["session"], short(&flow.conversation.0));
    let rows = workers["workers"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{workers:#}");
    assert_eq!(rows[0]["id"], short(&mine.id.0));
    assert_eq!(rows[0]["number"], 1);
    assert_eq!(rows[0]["title"], "Count the lines");
    assert_eq!(rows[0]["agent"], "claude");
    // How full its context is, as its CLI said, for the parent to weigh reusing it (run2-D42).
    assert_eq!(
        rows[0]["context"],
        json!({"usedTokens": 42_000, "windowTokens": 1_000_000})
    );

    // A worker calls as itself: it sees the sidebar sessions, none as its own, and has no
    // workers.
    assert_eq!(answers["worker workers"]["workers"], json!([]));
    let seen = answers["worker sessions"]["sessions"].as_array().unwrap();
    assert!(seen.iter().all(|row| row["caller"] == false), "{seen:?}");

    // `show` and `read` of the worker.
    let shown = &answers["show worker"];
    assert_eq!(shown["kind"], "worker");
    assert_eq!(shown["parent"], short(&flow.conversation.0));
    assert_eq!(shown["task"], "Count the lines in README.");
    assert!(shown["report"].is_null(), "{shown:#}");
    let read = &answers["read worker"];
    assert_eq!(read["view"], "last");
    assert_eq!(
        read["items"][0]["text"], "Reported: three lines.",
        "{read:#}"
    );
    assert_eq!(read["items"][0]["role"], "assistant");
    assert!(
        answers["read worker last"]["items"]
            .as_array()
            .unwrap()
            .len()
            <= 3
    );
    assert_eq!(answers["read worker screen"]["view"], "screen");

    // The other session, in full and its last answer.
    let other = &answers["show other"];
    assert_eq!(other["kind"], "session");
    assert_eq!(other["id"], short(&docs.0));
    assert_eq!(other["agent"], "claude");
    assert_eq!(other["checkout"], "worktree");
    assert_eq!(other["base"], "main");
    assert_eq!(other["workers"][0]["id"], short(&theirs.id.0));
    let last_answer = flow
        .core
        .list_messages(docs.clone(), None, 50)
        .await
        .unwrap()
        .messages
        .into_iter()
        .rfind(|message| message.role == crate::model::MessageRole::Assistant)
        .unwrap();
    assert_eq!(
        answers["read other"]["items"][0]["text"],
        last_answer.text.as_str()
    );
    assert_eq!(answers["read other"]["items"].as_array().unwrap().len(), 1);

    // This session's screen: from the message being answered on.
    let screen = answers["read me screen"]["items"].as_array().unwrap();
    assert_eq!(screen[0]["role"], "user");
    assert!(screen[0]["text"].as_str().unwrap().contains("Look around."));

    // Another project's session is found by id too (S30).
    assert_eq!(answers["show elsewhere"]["kind"], "session");
    assert_eq!(answers["show elsewhere"]["id"], short(&elsewhere.0));
    flow.stop().await;
}

/// A completed command, as a CLI reports one.
fn command(n: usize) -> brigadier_providers::ProviderEvent {
    brigadier_providers::ProviderEvent::Command {
        item_id: format!("c{n}"),
        command: format!("echo {n}"),
        cwd: None,
        status: brigadier_providers::model::ItemStatus::Completed,
        exit_code: Some(0),
        output: Some(format!("{n}\n")),
        duration_ms: Some(1),
        kept: None,
    }
}

/// `read` of a session shows its own commands with its messages, `--screen` keeps the newest
/// of a long request's commands and says older ones were left out, and a worker's `--last N`
/// reads back as far as it takes (STRIP-DOWN-PLAN S13; Codex review of part 1).
#[tokio::test]
async fn read_shows_commands_and_reads_back_as_far_as_asked() {
    const RUNS: usize = 205;
    const WORKER_RUNS: usize = 1100;
    let script: Script = Arc::new(|turn| {
        Box::pin(async move {
            if turn.worker().is_some() {
                for n in 0..WORKER_RUNS {
                    turn.emit(command(n)).await;
                }
                return Reply::text("Ran them all.");
            }
            if turn.input.contains("Start a worker.") {
                turn.start_worker("Run them", "Run the commands.").await;
                return Reply::text("Started a worker.");
            }
            if turn.input.contains("Run them.") {
                for n in 0..RUNS {
                    turn.emit(command(n)).await;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                return Reply::text("Ran them.");
            }
            Reply::text("Noted.")
        })
    });
    let flow = Flow::start("cli-read-back", with_session_files(), script).await;
    flow.say("Run them.").await;
    flow.settled().await;
    let caller = crate::manager::cli::CliCaller {
        conversation_id: flow.conversation.clone(),
        task_id: None,
        grant: String::new(),
    };
    let read = |id: String, last, screen| {
        let (flow, caller) = (&flow, &caller);
        async move {
            let call = CliCall {
                protocol: PROTOCOL,
                request: CliRequest::Read { id, last, screen },
            };
            match flow.manager.cli_call(caller, call).await {
                CliReply::Ok { output } => output,
                CliReply::Error { error } => panic!("{error:?}"),
            }
        }
    };
    let me = flow.conversation.0.clone();
    let texts = |read: &Value| -> Vec<String> {
        read["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                format!(
                    "{} {}",
                    item["kind"].as_str().unwrap(),
                    item["text"].as_str().unwrap()
                )
            })
            .collect()
    };

    // `--last 3`: its last two commands, then its answer.
    let last = read(me.clone(), Some(3), false).await;
    assert_eq!(
        texts(&last),
        [
            format!("command echo {}", RUNS - 2),
            format!("command echo {}", RUNS - 1),
            "message Ran them.".to_owned()
        ],
        "{last:#}"
    );
    assert_eq!(last["hasMore"], true);

    // `--screen`: the request, then the newest 200 items after it (its commands and answer);
    // older ones left out.
    let screen = read(me.clone(), None, true).await;
    let shown = texts(&screen);
    assert!(
        shown[0].starts_with("message ") && shown[0].contains("Run them."),
        "{shown:?}"
    );
    assert_eq!(shown.last().unwrap(), "message Ran them.");
    let commands: Vec<&String> = shown
        .iter()
        .filter(|text| text.starts_with("command "))
        .collect();
    assert_eq!(commands.len(), 199);
    assert_eq!(commands[0], &format!("command echo {}", RUNS - 199));
    assert_eq!(commands[198], &format!("command echo {}", RUNS - 1));
    assert_eq!(screen["hasMore"], true);

    // A worker's `--last N` past one page of its events reads back as far as asked.
    flow.say("Start a worker.").await;
    flow.until("its worker answered", |board| {
        board
            .tasks
            .values()
            .any(|task| task.state == crate::work::TaskState::Reported)
    })
    .await;
    flow.settled().await;
    let worker = Flow::task(&flow.board().await, 1).id.0.clone();
    // Its last events may still be on their way to its log under load: read until they are.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let (all, shown) = loop {
        let all = read(worker.clone(), Some(1050), false).await;
        let shown = texts(&all);
        if shown.len() == 1050 || std::time::Instant::now() > deadline {
            break (all, shown);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    assert_eq!(shown.len(), 1050, "{:?}", &shown[..3]);
    assert!(
        shown.contains(&"message Ran them all.".to_owned()),
        "{:?}",
        &shown[1040..]
    );
    assert!(shown[0].starts_with("command echo "), "{}", shown[0]);
    assert_eq!(all["hasMore"], true);
    let answer = read(worker, None, false).await;
    assert_eq!(texts(&answer), ["message Ran them all."]);
    flow.stop().await;
}

/// `brigadier start`'s request for `prompt`, with nothing else given.
pub(super) fn start(prompt: &str) -> StartRequest {
    StartRequest {
        title: None,
        agent: None,
        model: None,
        effort: None,
        work: None,
        local: false,
        repo: None,
        base: None,
        permission: None,
        lean: false,
        skills: Vec::new(),
        with: Vec::new(),
        mockup: false,
        prompt: prompt.into(),
    }
}

fn send(id: &str, text: Option<&str>, answer: Option<Answer>) -> CliRequest {
    CliRequest::Send {
        id: id.into(),
        text: text.map(Into::into),
        answer,
    }
}

pub(super) fn wait(ids: &[&str], timeout_secs: Option<u32>) -> CliRequest {
    CliRequest::Wait {
        ids: ids.iter().map(|id| (*id).to_owned()).collect(),
        timeout_secs,
    }
}

/// Waits through `brigadier wait` until `wanted` events came, keeping them all.
pub(super) async fn wait_for(turn: &Turn, ids: &[&str], wanted: usize) -> Vec<Value> {
    let mut events = Vec::new();
    while events.len() < wanted {
        let got = brigadier(turn, wait(ids, Some(60))).await;
        assert_eq!(got["timedOut"], false, "{got:#}");
        let got = got["events"].as_array().unwrap().clone();
        assert!(!got.is_empty(), "wait ended with no event: {events:#?}");
        events.extend(got);
    }
    events
}

pub(super) fn id_of(started: &Value) -> String {
    started["worker"]["id"].as_str().unwrap().to_owned()
}

/// The session's thread starts plain workers with `brigadier start` and talks to them with
/// `send`, `wait` and `close`; each worker is a plain CLI session that calls `brigadier` as
/// itself.
fn starting(answers: Answers) -> Script {
    Arc::new(move |turn| {
        let answers = answers.clone();
        Box::pin(async move {
            let keep = |name: &str, value: Value| {
                answers.lock().unwrap().insert(name.into(), value);
            };
            let worker = turn
                .env(GRANT_ENV)
                .and_then(|grant| turn.host.grants().resolve(grant));
            if let Some(Role::Worker { task_id, .. }) = worker {
                keep(&format!("worker env {}", task_id.0), seen_env(&turn));
                keep(
                    &format!("worker prompt {}", task_id.0),
                    json!({"input": turn.input, "prompt": turn.prompt, "cwd": turn.cwd}),
                );
                if turn.input.contains("Count to three.") {
                    keep("a running", json!(true));
                    let heard = turn.steered().await.unwrap_or_default();
                    return Reply::text(format!("Counted: 1 2 3. Heard: {heard}"));
                }
                if turn.input.contains("Ask first.") {
                    return match turn.ask_approval("curl https://example.com", "curl").await {
                        Some(brigadier_providers::ApprovalDecision::Allow) => {
                            Reply::text("Allowed: done.")
                        }
                        other => Reply::text(format!("Not allowed: {other:?}")),
                    };
                }
                keep(
                    "worker starts",
                    brigadier(&turn, CliRequest::Start(Box::new(start("More.")))).await,
                );
                keep(
                    "worker workers",
                    brigadier(&turn, CliRequest::Workers).await,
                );
                return Reply::text("Hi.");
            }
            if turn.input.contains("Start one.") {
                let started = brigadier(&turn, CliRequest::Start(Box::new(start("Say hi.")))).await;
                keep("theirs", started["worker"]["id"].clone());
                return Reply::text("Started one.");
            }
            if !turn.input.contains("Start them.") {
                let mut answers = answers.lock().unwrap();
                let mut other = answers
                    .get("other thread turns")
                    .cloned()
                    .unwrap_or_else(|| json!([]));
                other.as_array_mut().unwrap().push(json!(turn.input));
                answers.insert("other thread turns".into(), other);
                return Reply::text("Noted.");
            }
            let a = brigadier(
                &turn,
                CliRequest::Start(Box::new(StartRequest {
                    title: Some("Count lines".into()),
                    agent: Some(Agent::Claude),
                    ..start("Count to three.")
                })),
            )
            .await;
            let b = brigadier(
                &turn,
                CliRequest::Start(Box::new(StartRequest {
                    agent: Some(Agent::Codex),
                    model: Some("gpt-test".into()),
                    effort: Some("low".into()),
                    ..start("Ask first.\nThen the rest.")
                })),
            )
            .await;
            let (a_id, b_id) = (id_of(&a), id_of(&b));
            keep("start a", a);
            keep("start b", b);
            // A message for a worker mid-turn goes into that turn.
            super::eventually("the counter to run", || {
                answers.lock().unwrap().contains_key("a running")
            })
            .await;
            keep(
                "send a",
                brigadier(&turn, send(&a_id, Some("And stop."), None)).await,
            );
            keep("events", json!(wait_for(&turn, &[], 2).await));
            keep(
                "allow b",
                brigadier(&turn, send(&b_id, None, Some(Answer::Allow))).await,
            );
            keep(
                "allow b again",
                brigadier(&turn, send(&b_id, None, Some(Answer::Allow))).await,
            );
            keep("events b", json!(wait_for(&turn, &[&b_id], 1).await));
            keep(
                "read a",
                brigadier(
                    &turn,
                    CliRequest::Read {
                        id: a_id.clone(),
                        last: None,
                        screen: false,
                    },
                )
                .await,
            );
            keep("workers", brigadier(&turn, CliRequest::Workers).await);
            keep(
                "sessions",
                brigadier(&turn, CliRequest::Sessions { all: false }).await,
            );
            keep(
                "close a",
                brigadier(&turn, CliRequest::Close { id: a_id.clone() }).await,
            );
            keep(
                "workers after close",
                brigadier(&turn, CliRequest::Workers).await,
            );
            keep(
                "send closed",
                brigadier(&turn, send(&a_id, Some("Again."), None)).await,
            );
            let theirs = answers.lock().unwrap()["theirs"]
                .as_str()
                .unwrap()
                .to_owned();
            keep(
                "send theirs",
                brigadier(&turn, send(&theirs, Some("Hello."), None)).await,
            );
            keep(
                "wait theirs",
                brigadier(&turn, wait(&[&theirs], Some(1))).await,
            );
            keep(
                "close theirs",
                brigadier(&turn, CliRequest::Close { id: theirs }).await,
            );
            let me = short(turn.env(SESSION_ENV).unwrap());
            keep(
                "send me",
                brigadier(&turn, send(&me, Some("Hello."), None)).await,
            );
            // b still lives, between turns: nothing more comes from it in a second.
            keep("wait idle", brigadier(&turn, wait(&[], Some(1))).await);
            // A lean worker in the session's own checkout, at a level of its own.
            let c = brigadier(
                &turn,
                CliRequest::Start(Box::new(StartRequest {
                    local: true,
                    lean: true,
                    skills: vec!["kept-skill".into()],
                    permission: Some(Permission::FullAccess),
                    ..start("Say hi.")
                })),
            )
            .await;
            let c_id = id_of(&c);
            keep("start c", c);
            keep("events c", json!(wait_for(&turn, &[&c_id], 1).await));
            Reply::text("Started them.")
        })
    })
}

/// STRIP-DOWN-PLAN S13 part 2: `brigadier start` starts a plain worker of the calling session
/// (G19: in its Workers row, never in the sidebar), on its session's level (Q8) and from its
/// session's branch (G21); `wait` hears its turn's end with its last message and its CLI's
/// permission prompt, which `send --allow` answers through the CLI's own protocol; `send`
/// reaches a worker mid-turn; `close` ends it; another session's worker is refused.
#[tokio::test]
async fn start_send_wait_and_close_a_sessions_own_plain_workers() {
    let answers = Answers::default();
    let options = Options {
        permission: crate::model::PermissionLevel::AcceptEdits,
        ..Options::default()
    };
    let flow = Flow::start("cli-workers", options, starting(answers.clone())).await;
    let docs = flow.add_session("Docs", None, ProviderKind::Claude).await;
    flow.say_to(&docs, "Start one.").await;
    flow.until_in(&docs, "its worker said hi", |board| {
        board
            .tasks
            .values()
            .any(|task| task.state == crate::work::TaskState::Reported)
    })
    .await;
    flow.say("Start them.").await;
    flow.until("it started them", |_| {
        answers.lock().unwrap().contains_key("events c")
    })
    .await;
    flow.settled().await;
    let answers = answers.lock().unwrap().clone();
    let board = flow.board().await;
    let (a, b, c) = (
        Flow::task(&board, 1).clone(),
        Flow::task(&board, 2).clone(),
        Flow::task(&board, 3).clone(),
    );

    // Started as asked: a title from the prompt when none is given, the session's agent and
    // level when none is given, and the CLI's own default model.
    assert_eq!(answers["start a"]["worker"]["title"], "Count lines");
    assert_eq!(answers["start a"]["worker"]["agent"], "claude");
    assert_eq!(answers["start a"]["permission"], "accept-edits");
    assert_eq!(answers["start b"]["worker"]["title"], "Ask first.");
    assert_eq!(answers["start b"]["worker"]["agent"], "codex");
    assert_eq!(a.route.choice.model, None);
    assert_eq!(b.route.choice.model.as_deref(), Some("gpt-test"));
    assert_eq!(b.route.choice.effort.as_deref(), Some("low"));
    let plain = |task: &crate::work::Task| task.plain.clone().unwrap();
    assert_eq!(
        plain(&b).permission,
        crate::model::PermissionLevel::AcceptEdits
    );
    assert_eq!(plain(&c).permission, crate::model::PermissionLevel::Full);

    // Each worker's CLI knows itself for `brigadier`, and starts with its prompt.
    let bin = flow.manager.config.daemon_exe.parent().unwrap().to_owned();
    for task in [&a, &b, &c] {
        let seen = &answers[&format!("worker env {}", task.id.0)];
        assert_eq!(seen["session"], task.id.0.as_str(), "{seen:#}");
        assert_eq!(seen["first on path"], json!(bin));
        let prompt = &answers[&format!("worker prompt {}", task.id.0)];
        assert_eq!(prompt["input"], task.spec.as_str(), "{prompt:#}");
        // Brigadier's instructions are the standing ones alone, with its own paths (S17), less
        // the line on running workers.
        let given = prompt["prompt"].as_str().unwrap_or_default();
        assert!(
            [ProviderKind::Claude, ProviderKind::Codex]
                .iter()
                .any(|provider| {
                    given
                        == crate::manager::prompts::standing_text(
                            &crate::manager::prompts::worker_instructions(
                                &flow.core.settings(),
                                *provider,
                            ),
                        )
                }),
            "only the standing instructions: {prompt:#}"
        );
    }
    let specs = flow.specs();
    let spec_of = |task: &crate::work::Task| {
        specs
            .iter()
            .find(|(_, spec)| {
                spec.env
                    .iter()
                    .any(|(key, value)| key == SESSION_ENV && *value == task.id.0)
            })
            .map(|(kind, spec)| (*kind, spec.clone()))
            .unwrap()
    };
    let (kind, spec) = spec_of(&b);
    assert_eq!(kind, ProviderKind::Codex);
    assert!(spec.mcp_servers.is_empty(), "no Brigadier server: {spec:?}");
    assert_eq!(
        spec.append_system_prompt,
        Some(crate::manager::prompts::standing_text(
            &crate::manager::prompts::worker_instructions(
                &flow.core.settings(),
                ProviderKind::Codex
            ),
        ))
    );
    assert!(spec.stop_hook.is_none());
    assert!(!spec.lean_worker);
    assert_eq!(
        spec.access,
        crate::model::PermissionLevel::AcceptEdits.access()
    );
    let (_, lean) = spec_of(&c);
    assert!(lean.lean_worker);
    assert_eq!(lean.access, crate::model::PermissionLevel::Full.access());

    // G21: its worktree's branch starts from the session's current branch and merges back
    // into it, as its folder's record says.
    let checkout = flow.manager.checkout_dir(&flow.conversation).unwrap();
    let parent = super::git(
        std::path::Path::new(&checkout),
        &["branch", "--show-current"],
    );
    let parent = parent.trim();
    assert_eq!(plain(&a).base.as_deref(), Some(parent));
    assert_eq!(answers["start a"]["base"], parent);
    let workspace = a.workspace.clone().unwrap();
    let branch = workspace.branch.clone().unwrap();
    assert_ne!(branch, parent);
    let merges_into = super::git(
        &flow.repo,
        &["config", "--get", &format!("branch.{branch}.gh-merge-base")],
    );
    assert_eq!(merges_into.trim(), parent);
    let record = flow
        .manager
        .session_dir(&flow.conversation)
        .worker(&a)
        .join("record/workspace.json");
    let record: Value = serde_json::from_str(&std::fs::read_to_string(record).unwrap()).unwrap();
    assert_eq!(
        record,
        json!({"environment": "newWorktree", "branch": branch, "base": parent})
    );
    let worktree = workspace.worktree.unwrap();
    let (_, a_spec) = spec_of(&a);
    assert_eq!(a_spec.cwd, std::path::PathBuf::from(&worktree));
    // `--local`: the session's own checkout, no branch of its own.
    assert_eq!(lean.cwd, std::path::PathBuf::from(&checkout));
    assert_eq!(c.workspace.as_ref().unwrap().branch, None);

    // `send` mid-turn reached the running turn; `wait` heard a's end with its last message
    // and b's permission prompt.
    assert_eq!(answers["send a"]["queued"], true, "{:#}", answers["send a"]);
    let events = answers["events"].as_array().unwrap();
    let of = |id: &str| {
        events
            .iter()
            .find(|event| event["id"] == id)
            .unwrap_or_else(|| panic!("no event of {id}: {events:#?}"))
    };
    let a_end = of(&short(&a.id.0));
    assert_eq!(a_end["event"], "turn-ended");
    assert!(
        a_end["eventId"]
            .as_str()
            .is_some_and(|id| id.starts_with("ev-")),
        "{a_end:#}"
    );
    assert_eq!(a_end["message"], "Counted: 1 2 3. Heard: And stop.");
    let asks = of(&short(&b.id.0));
    assert_eq!(asks["event"], "needs-input");
    assert!(asks["asks"].as_str().unwrap().contains("curl"), "{asks:#}");
    // Its card was under Waiting on you until `send --allow` answered it; then b went on.
    assert_eq!(answers["allow b"]["answered"], "allow");
    assert_eq!(answers["allow b again"]["error"]["code"], "not-found");
    let card = board
        .approvals
        .values()
        .find(|card| card.task_id.as_ref() == Some(&b.id))
        .unwrap();
    assert_ne!(card.state, crate::work::CardState::Pending);
    let b_end = &answers["events b"][0];
    assert_eq!(b_end["event"], "turn-ended");
    assert_eq!(b_end["message"], "Allowed: done.");
    assert_eq!(answers["events c"][0]["message"], "Hi.");
    assert_eq!(
        answers["read a"]["items"][0]["text"],
        "Counted: 1 2 3. Heard: And stop."
    );

    // G19: its workers are in its Workers row and `workers`, never in `sessions` or the
    // sidebar.
    let listed: Vec<&str> = answers["workers"]["workers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert_eq!(listed, [short(&a.id.0), short(&b.id.0)]);
    let sessions: Vec<&str> = answers["sessions"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    for task in [&a, &b, &c] {
        assert!(!sessions.contains(&short(&task.id.0).as_str()));
        assert!(
            flow.core
                .visible_catalog()
                .conversations
                .iter()
                .all(|conversation| conversation.id.0 != task.id.0)
        );
    }

    // Closed: gone from `workers`, and nothing more reaches it.
    assert_eq!(answers["close a"]["id"], short(&a.id.0));
    assert!(plain(&a).closed);
    assert!(a.state.is_final());
    let after: Vec<&str> = answers["workers after close"]["workers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert!(!after.contains(&short(&a.id.0).as_str()), "{after:?}");
    assert_eq!(answers["send closed"]["error"]["code"], "not-found");

    // Another session's worker, and a session, are refused; so is `start` from a worker.
    for refused in ["send theirs", "wait theirs", "close theirs", "send me"] {
        assert_eq!(
            answers[refused]["error"]["code"], "refused",
            "{refused}: {:#}",
            answers[refused]
        );
    }
    assert_eq!(answers["worker starts"]["error"]["code"], "refused");
    assert_eq!(answers["worker workers"]["workers"], json!([]));
    assert_eq!(answers["wait idle"]["timedOut"], true);
    assert_eq!(answers["wait idle"]["events"], json!([]));

    // What `wait` took was not sent again; the one event no wait took (Docs's worker said hi)
    // reached Docs's thread once, from the safety net (S29).
    let others = answers["other thread turns"].as_array().unwrap();
    assert_eq!(others.len(), 1, "{others:#?}");
    let net = others[0].as_str().unwrap();
    assert!(
        net.contains("[worker events wn-") && net.contains("finished · Hi."),
        "{net}"
    );

    // The thread shows "Messaged ‹worker›" for the message that reached one, and "Stopped
    // ‹worker›" for the close; an answer to a permission prompt, and a refused send or close,
    // file nothing.
    use crate::work::OrchestratorStepKind;
    let steps: Vec<OrchestratorStepKind> = flow
        .events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            crate::model::DomainEvent::OrchestratorStepped { step } => match step.kind {
                kind @ (OrchestratorStepKind::Messaged { .. }
                | OrchestratorStepKind::Stopped { .. }) => Some(kind),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        steps,
        vec![
            OrchestratorStepKind::Messaged {
                task_id: a.id.clone(),
                text: Some("And stop.".into()),
            },
            OrchestratorStepKind::Stopped {
                task_id: a.id.clone(),
                reason: "Closed by its session.".into(),
            },
        ]
    );
    flow.stop().await;
}

/// `brigadier start --mockup`: its worker is told where its screens go by name; what it saved
/// there is its outputs once its turn ends (the session's Mockup tab), and the user's notes
/// reach it with the marked screenshots' paths, a new version of the mockup.
#[tokio::test]
async fn a_mockup_worker_shows_its_screens_as_each_turn_ends_and_hears_the_notes() {
    let answers = Answers::default();
    let script: Script = {
        let answers = answers.clone();
        Arc::new(move |turn| {
            let answers = answers.clone();
            Box::pin(async move {
                let keep = |name: &str, value: Value| {
                    answers.lock().unwrap().insert(name.into(), value);
                };
                let worker = turn
                    .env(GRANT_ENV)
                    .and_then(|grant| turn.host.grants().resolve(grant));
                if let Some(Role::Worker { .. }) = worker {
                    if turn.input.starts_with("Notes on the mockup") {
                        keep("notes", json!(turn.input));
                        return Reply::text("Revised.");
                    }
                    keep("first", json!(turn.input));
                    let folder = turn
                        .input
                        .split_once("mockup-2.png… in ")
                        .and_then(|(_, rest)| rest.split_once("; they show"))
                        .map(|(folder, _)| std::path::PathBuf::from(folder))
                        .expect("the folder is named");
                    std::fs::write(folder.join("mockup-1.png"), b"png").unwrap();
                    return Reply::text("Made it.");
                }
                let started = brigadier(
                    &turn,
                    CliRequest::Start(Box::new(StartRequest {
                        mockup: true,
                        ..start("Mock the density toggle.")
                    })),
                )
                .await;
                keep("start", started);
                Reply::text("Started the mockup.")
            })
        })
    };
    let flow = Flow::start("cli-mockup", Options::default(), script).await;
    flow.say("Mock it.").await;
    flow.until("the mockup's screen is kept", |board| {
        board
            .tasks
            .values()
            .any(|task| task.state == crate::work::TaskState::Reported && !task.outputs.is_empty())
    })
    .await;
    let board = flow.board().await;
    let task = Flow::task(&board, 1).clone();
    assert_eq!(task.mockup, Some(crate::work::MockupPart::Builds));
    assert_eq!(task.spec, "Mock the density toggle.");
    let names: Vec<_> = task
        .outputs
        .iter()
        .map(|output| output.file_name.clone().unwrap())
        .collect();
    assert_eq!(names, ["mockup-1.png"]);
    assert_eq!(task.outputs[0].mime, "image/png");

    // The Mockup tab's Send: the notes and the marked screenshot reach the worker.
    let hash = flow
        .core
        .store()
        .blobs()
        .put(b"marked".to_vec())
        .await
        .unwrap();
    let marked = crate::work::AttachmentRef {
        id: hash.to_string(),
        name: "mockup-1-notes.png".into(),
        mime: "image/png".into(),
        bytes: 6,
        pasted: false,
        inline: None,
    };
    flow.manager
        .send_mockup_notes(
            task.id.clone(),
            "Notes on the mockup (version 1): make it blue.".into(),
            vec![marked],
        )
        .await
        .unwrap();
    super::eventually("the worker to hear the notes", || {
        answers.lock().unwrap().contains_key("notes")
    })
    .await;
    let notes = answers.lock().unwrap()["notes"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = notes
        .split_once("The marked screenshots:\n- ")
        .map(|(_, path)| path.trim())
        .unwrap_or_else(|| panic!("no marked screenshot: {notes}"));
    assert!(path.ends_with("attachments/mockup-1-notes.png"), "{path}");
    assert_eq!(std::fs::read(path).unwrap(), b"marked");
    let board = flow.board().await;
    assert_eq!(Flow::task(&board, 1).rework_rounds, 1);
    flow.stop().await;
}

/// A plain worker's CLI dying is a `gone` event and a usage limit a `usage-limit` one, for its
/// session's `brigadier wait`; neither is handed to another model.
#[tokio::test]
async fn wait_hears_a_worker_gone_and_its_usage_limit() {
    let answers = Answers::default();
    let script: Script = {
        let answers = answers.clone();
        Arc::new(move |turn| {
            let answers = answers.clone();
            Box::pin(async move {
                let worker = turn
                    .env(GRANT_ENV)
                    .and_then(|grant| turn.host.grants().resolve(grant));
                if let Some(Role::Worker { .. }) = worker {
                    return match turn.input.contains("Crash.") {
                        true => Reply::crashed(9),
                        false => Reply::limited(),
                    };
                }
                if !turn.input.contains("Start them.") {
                    return Reply::text("Noted.");
                }
                let crash = brigadier(&turn, CliRequest::Start(Box::new(start("Crash.")))).await;
                let limit = brigadier(&turn, CliRequest::Start(Box::new(start("Limit.")))).await;
                let (crash, limit) = (id_of(&crash), id_of(&limit));
                let crashed = wait_for(&turn, &[&crash], 1).await;
                let limited = wait_for(&turn, &[&limit], 1).await;
                let mut answers = answers.lock().unwrap();
                answers.insert("crashed".into(), json!(crashed));
                answers.insert("limited".into(), json!(limited));
                Reply::text("Done.")
            })
        })
    };
    let flow = Flow::start("cli-workers-gone", Options::default(), script).await;
    flow.say("Start them.").await;
    flow.until("it heard them", |_| {
        answers.lock().unwrap().contains_key("limited")
    })
    .await;
    flow.settled().await;
    let answers = answers.lock().unwrap().clone();
    let crashed = answers["crashed"].as_array().unwrap();
    assert!(
        crashed.iter().any(|event| event["event"] == "gone"),
        "{crashed:#?}"
    );
    let limited = &answers["limited"][0];
    assert_eq!(limited["event"], "usage-limit", "{limited:#}");
    assert!(limited["resetsAtMs"].as_i64().is_some());
    let board = flow.board().await;
    assert_eq!(Flow::task(&board, 1).state, crate::work::TaskState::Failed);
    assert_eq!(Flow::task(&board, 2).state, crate::work::TaskState::Paused);
    // Nobody took either over.
    assert_eq!(board.tasks.len(), 2);
    assert!(board.tasks.values().all(|task| task.attempts.len() == 1));
    flow.stop().await;
}

pub(super) fn tool(tool: brigadier_cli::protocol::Tool, action: &str, args: Value) -> CliRequest {
    CliRequest::Tool {
        tool,
        action: action.into(),
        args,
    }
}

/// STRIP-DOWN-PLAN S13: the tools a session keeps, behind `brigadier`, run as the caller's own
/// calls of them: the thread's previews, app, browser and pages; `ask` waits for the user's
/// answer on its card (free text taken next to its options) and the answer goes back to it
/// alone. A worker keeps its previews, and doesn't ask the user or act in the app.
#[tokio::test]
async fn the_kept_tools_run_behind_brigadier_as_the_callers_own() {
    use brigadier_cli::protocol::{AskItem, Tool};
    let answers = Answers::default();
    let script: Script = {
        let answers = answers.clone();
        Arc::new(move |turn| {
            let answers = answers.clone();
            Box::pin(async move {
                let keep = |name: &str, value: Value| {
                    answers.lock().unwrap().insert(name.into(), value);
                };
                let worker = turn
                    .env(GRANT_ENV)
                    .and_then(|grant| turn.host.grants().resolve(grant));
                if let Some(Role::Worker { .. }) = worker {
                    keep(
                        "worker preview log",
                        brigadier(&turn, tool(Tool::Preview, "log", json!({}))).await,
                    );
                    keep(
                        "worker app",
                        brigadier(&turn, tool(Tool::App, "state", json!({}))).await,
                    );
                    keep(
                        "worker ask",
                        brigadier(
                            &turn,
                            CliRequest::Ask {
                                questions: vec![AskItem {
                                    question: "May I?".into(),
                                    options: Vec::new(),
                                }],
                                title: None,
                            },
                        )
                        .await,
                    );
                    return Reply::text("Tried them.");
                }
                if !turn.input.contains("Use the tools.") {
                    let mut answers = answers.lock().unwrap();
                    let other = answers.get("other thread turns").and_then(Value::as_u64);
                    answers.insert("other thread turns".into(), json!(other.unwrap_or(0) + 1));
                    return Reply::text("Noted.");
                }
                keep(
                    "preview start",
                    brigadier(
                        &turn,
                        tool(
                            Tool::Preview,
                            "start",
                            json!({"command": "echo ready; sleep 30", "name": "site",
                                   "kind": "test"}),
                        ),
                    )
                    .await,
                );
                keep(
                    "preview log",
                    brigadier(
                        &turn,
                        tool(
                            Tool::Preview,
                            "log",
                            json!({"tail_lines": 5, "wait_secs": 5}),
                        ),
                    )
                    .await,
                );
                keep(
                    "app state",
                    brigadier(&turn, tool(Tool::App, "state", json!({}))).await,
                );
                keep(
                    "browser read",
                    brigadier(
                        &turn,
                        tool(Tool::Browser, "read", json!({"tab": "browser:x"})),
                    )
                    .await,
                );
                keep(
                    "page read",
                    brigadier(&turn, tool(Tool::Page, "read", json!({"page_id": "nope"}))).await,
                );
                keep(
                    "bad args",
                    brigadier(&turn, tool(Tool::Page, "read", json!({"page": "nope"}))).await,
                );
                keep(
                    "ask options",
                    brigadier(
                        &turn,
                        CliRequest::Ask {
                            questions: vec![AskItem {
                                question: "Ship it today?".into(),
                                options: vec!["Yes".into(), "Not yet".into()],
                            }],
                            title: Some("Release".into()),
                        },
                    )
                    .await,
                );
                keep(
                    "ask free",
                    brigadier(
                        &turn,
                        CliRequest::Ask {
                            questions: vec![AskItem {
                                question: "Which colour?".into(),
                                options: vec!["Red".into(), "Green".into()],
                            }],
                            title: None,
                        },
                    )
                    .await,
                );
                keep(
                    "ask round",
                    brigadier(
                        &turn,
                        CliRequest::Ask {
                            questions: vec![
                                AskItem {
                                    question: "Ship it today?".into(),
                                    options: vec!["Yes".into(), "Not yet".into()],
                                },
                                AskItem {
                                    question: "Which size?".into(),
                                    options: Vec::new(),
                                },
                            ],
                            title: Some("Round 2".into()),
                        },
                    )
                    .await,
                );
                let started =
                    brigadier(&turn, CliRequest::Start(Box::new(start("Try the tools.")))).await;
                let id = id_of(&started);
                wait_for(&turn, &[&id], 1).await;
                keep(
                    "preview stop",
                    brigadier(&turn, tool(Tool::Preview, "stop", json!({}))).await,
                );
                Reply::text("Used them.")
            })
        })
    };
    let flow = Flow::start("cli-tools", Options::default(), script).await;
    flow.say("Use the tools.").await;
    for given in [&["Yes"][..], &["Blue, please"], &["Not yet", "Large"]] {
        let board = flow
            .until("a card", |board| {
                board.questions.values().any(|question| question.is_open())
            })
            .await;
        let card = board
            .questions
            .values()
            .find(|question| question.is_open())
            .unwrap()
            .clone();
        // Options as given, the recommended (first) one marked.
        assert_eq!(
            card.items[0].options[0].label,
            if given[0] == "Blue, please" {
                "Red"
            } else {
                "Yes"
            }
        );
        assert_eq!(card.items[0].recommended, Some(0));
        // A round's questions on one card, each with its own options.
        assert_eq!(card.items.len(), given.len());
        if given.len() > 1 {
            assert_eq!(card.title.as_deref(), Some("Round 2"));
            assert_eq!(card.items[1].text, "Which size?");
            assert!(card.items[1].options.is_empty());
        }
        flow.manager
            .answer_question(
                flow.conversation.clone(),
                card.id,
                given.iter().map(|answer| answer.to_string()).collect(),
                None,
            )
            .await
            .unwrap();
    }
    flow.until("it used them", |_| {
        answers.lock().unwrap().contains_key("preview stop")
    })
    .await;
    flow.settled().await;
    let answers = answers.lock().unwrap().clone();

    let text = |name: &str| {
        answers[name]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    assert_eq!(
        answers["preview start"]["action"], "start",
        "{:#}",
        answers["preview start"]
    );
    assert!(
        text("preview start").contains("preview-1"),
        "{}",
        text("preview start")
    );
    assert!(
        text("preview log").contains("ready"),
        "{}",
        text("preview log")
    );
    assert_eq!(answers["preview stop"]["action"], "stop");
    assert_eq!(answers["app state"]["action"], "state");
    assert!(!text("app state").is_empty());
    // With no app window, the browser tool says it can't be used, as before.
    assert!(
        answers["browser read"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("its browser can't be used"),
        "{:#}",
        answers["browser read"]
    );
    assert_eq!(answers["page read"]["error"]["code"], "failed");
    assert_eq!(answers["bad args"]["error"]["code"], "usage");

    // The answers came back to `ask`, the chosen option's index or none for own words, and
    // never into the thread as a message.
    assert_eq!(
        answers["ask options"],
        json!({"answer": "Yes", "option": 0,
               "answers": [{"question": "Ship it today?", "answer": "Yes", "option": 0}]})
    );
    assert_eq!(
        answers["ask free"],
        json!({"answer": "Blue, please", "option": null,
               "answers": [{"question": "Which colour?", "answer": "Blue, please",
                            "option": null}]})
    );
    // A round's answers come back together, each with its question.
    assert_eq!(
        answers["ask round"],
        json!({"answer": "Not yet", "option": 1,
               "answers": [{"question": "Ship it today?", "answer": "Not yet", "option": 1},
                           {"question": "Which size?", "answer": "Large", "option": null}]})
    );
    assert_eq!(answers.get("other thread turns"), None);
    let messages = flow
        .core
        .list_messages(flow.conversation.clone(), None, 50)
        .await
        .unwrap()
        .messages;
    assert!(
        messages
            .iter()
            .all(|message| !message.text.contains("[answer]")),
        "{messages:#?}"
    );

    // A worker keeps its previews; the app and the user are its session's.
    assert_eq!(
        answers["worker preview log"]["action"], "log",
        "{:#}",
        answers["worker preview log"]
    );
    assert_eq!(answers["worker app"]["error"]["code"], "refused");
    assert_eq!(answers["worker ask"]["error"]["code"], "refused");
    flow.stop().await;
}

/// A fresh rules file admits every model and effort offered by either CLI.
#[tokio::test]
async fn models_fresh_rules_admit_every_offered_model() {
    let answers = Answers::default();
    let seen = answers.clone();
    let script: Script = Arc::new(move |turn| {
        let seen = seen.clone();
        Box::pin(async move {
            if matches!(
                turn.host.grants().resolve(&turn.grant),
                Some(Role::Worker { .. })
            ) {
                return Reply::text("The requested model started.");
            }
            if turn.input.contains("Try every offered model.") {
                let models = brigadier(&turn, CliRequest::Models).await;
                assert_eq!(models["rules"], json!([]));
                let offered = models["models"].as_array().unwrap();
                assert!(offered.iter().any(|m| m["agent"] == "claude"));
                assert!(offered.iter().any(|m| m["agent"] == "codex"));
                for model in offered {
                    for effort in model["efforts"].as_array().unwrap() {
                        let request = StartRequest {
                            agent: Some(serde_json::from_value(model["agent"].clone()).unwrap()),
                            model: Some(model["model"].as_str().unwrap().into()),
                            effort: Some(effort.as_str().unwrap().into()),
                            local: true,
                            ..start("Reply once.")
                        };
                        let result = brigadier(&turn, CliRequest::Start(Box::new(request))).await;
                        assert!(result.get("error").is_none(), "{model:#}: {result:#}");
                        seen.lock().unwrap().insert(
                            format!("{}:{}:{}", model["agent"], model["model"], effort),
                            result,
                        );
                    }
                }
                let count: usize = offered
                    .iter()
                    .map(|model| model["efforts"].as_array().unwrap().len())
                    .sum();
                seen.lock()
                    .unwrap()
                    .insert("offered count".into(), json!(count));
            }
            Reply::text("Checked.")
        })
    });
    let flow = Flow::start(
        "cli-models-fresh",
        Options {
            behavior: Arc::new(FakeBehavior {
                light_model: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
        script,
    )
    .await;
    let path = flow.manager.models_rules_path();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        super::super::models::DEFAULT_RULES.as_bytes(),
        "the isolated home starts with the shipped rules file"
    );
    flow.say("Try every offered model.").await;
    flow.until("every offered model ran", |b| {
        let count = answers
            .lock()
            .unwrap()
            .get("offered count")
            .and_then(Value::as_u64);
        count.is_some_and(|count| count > 0 && b.tasks.len() == count as usize)
            && b.tasks
                .values()
                .all(|t| t.state == crate::work::TaskState::Reported)
    })
    .await;
    flow.settled().await;
    assert_eq!(
        std::fs::read(&path).unwrap(),
        super::super::models::DEFAULT_RULES.as_bytes()
    );
    assert!(
        flow.specs()
            .iter()
            .all(|(_, spec)| spec.allowed_models.is_none())
    );
    flow.stop().await;
}

/// `brigadier models` carries the user's "Workers at once" (run2-D31): 3 by default, then what
/// Settings saves. It is advice: a start past it is not refused (G18).
#[tokio::test]
async fn models_carry_the_workers_at_once_setting_and_start_ignores_it() {
    let answers = Answers::default();
    let seen = answers.clone();
    let script: Script = Arc::new(move |turn| {
        let seen = seen.clone();
        Box::pin(async move {
            if matches!(
                turn.host.grants().resolve(&turn.grant),
                Some(Role::Worker { .. })
            ) {
                return Reply::text("Started.");
            }
            if let Some(round) = turn.input.strip_prefix("Count workers ") {
                let models = brigadier(&turn, CliRequest::Models).await;
                seen.lock()
                    .unwrap()
                    .insert(format!("models {round}"), models);
            }
            if turn.input.contains("Start two.") {
                for title in ["one", "two"] {
                    let request = StartRequest {
                        title: Some(title.into()),
                        local: true,
                        ..start("Reply once.")
                    };
                    let result = brigadier(&turn, CliRequest::Start(Box::new(request))).await;
                    seen.lock()
                        .unwrap()
                        .insert(format!("start {title}"), result);
                }
            }
            Reply::text("Checked.")
        })
    });
    let flow = Flow::start("cli-workers-at-once", Options::default(), script).await;
    flow.say("Count workers 1").await;
    flow.settled().await;
    let mut settings = flow.core.settings();
    settings.workers_at_once = 1;
    flow.core.update_settings(settings).await.unwrap();
    flow.say("Count workers 2").await;
    flow.settled().await;
    flow.say("Start two.").await;
    flow.settled().await;
    let seen = answers.lock().unwrap().clone();
    assert_eq!(seen["models 1"]["workersAtOnce"], 3);
    assert_eq!(seen["models 2"]["workersAtOnce"], 1);
    for title in ["one", "two"] {
        assert!(
            seen[&format!("start {title}")].get("error").is_none(),
            "{seen:#?}"
        );
    }
    flow.stop().await;
}

/// User rules refuse and quote Fable; removing only that directive admits the exact request.
#[tokio::test]
async fn models_rules_are_the_only_gate_on_start() {
    let answers = Answers::default();
    let seen = answers.clone();
    let script: Script = Arc::new(move |turn| {
        let seen = seen.clone();
        Box::pin(async move {
            if matches!(
                turn.host.grants().resolve(&turn.grant),
                Some(Role::Worker { .. })
            ) {
                return Reply::text("The requested model started.");
            }
            if turn.input.contains("Try the model.") {
                let request = StartRequest {
                    model: Some("claude-fable-5-1".into()),
                    effort: Some("high".into()),
                    local: true,
                    ..start("Reply once.")
                };
                let at = std::time::Instant::now();
                let models = brigadier(&turn, CliRequest::Models).await;
                assert!(at.elapsed() < std::time::Duration::from_millis(200));
                let settings_usage = turn.host.usage_view().await.unwrap();
                for provider in settings_usage.providers {
                    for account in provider.accounts {
                        let row = models["accounts"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|row| {
                                row["id"] == account.account.account.as_deref().unwrap_or("own")
                                    && row["agent"]
                                        == serde_json::to_value(provider.provider).unwrap()
                            })
                            .unwrap();
                        if let Some(quota) = account.quota {
                            for window in quota.windows {
                                let listed = row["usage"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .find(|w| w["window"] == window.window.id)
                                    .unwrap();
                                assert_eq!(listed["usedPercent"], window.window.used_percent);
                                assert_eq!(
                                    listed["resetsAtMs"],
                                    serde_json::to_value(window.window.resets_at_ms).unwrap()
                                );
                            }
                        }
                    }
                }
                let result = brigadier(&turn, CliRequest::Start(Box::new(request))).await;
                seen.lock().unwrap().insert("models".into(), models);
                seen.lock().unwrap().insert("start".into(), result);
            }
            Reply::text("Checked.")
        })
    });
    let flow = Flow::start("cli-model-rules", Options::default(), script).await;
    let path = flow.manager.models_rules_path();
    let rules = "# My rules\r\n\r\n- forbid: fable\r\n- effort-ceiling: high\r\n";
    std::fs::write(&path, rules).unwrap();
    flow.say("Try the model.").await;
    flow.settled().await;
    assert_eq!(std::fs::read(&path).unwrap(), rules.as_bytes());
    {
        let seen = answers.lock().unwrap();
        assert_eq!(seen["start"]["error"]["code"], "refused");
        assert!(
            seen["start"]["error"]["message"]
                .as_str()
                .unwrap()
                .contains("\"forbid: fable\"")
        );
        assert!(
            seen["models"]["rules"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["rule"] == "forbid: fable")
        );
    }
    assert!(flow.board().await.tasks.is_empty());
    let rules = std::fs::read_to_string(&path)
        .unwrap()
        .replace("- forbid: fable\r\n", "");
    std::fs::write(&path, &rules).unwrap();
    flow.say("Try the model.").await;
    flow.until("the allowed worker ran", |b| {
        b.tasks
            .values()
            .any(|t| t.state == crate::work::TaskState::Reported)
    })
    .await;
    flow.settled().await;
    assert_eq!(std::fs::read(&path).unwrap(), rules.as_bytes());
    let seen = answers.lock().unwrap().clone();
    assert!(seen["start"].get("error").is_none(), "{seen:#?}");
    assert_eq!(seen["start"]["worker"]["model"], "claude-fable-5-1");
    let specs = flow.specs();
    let worker = specs
        .iter()
        .find(|(_, spec)| spec.model.as_deref() == Some("claude-fable-5-1"))
        .unwrap();
    assert!(
        worker.1.allowed_models.is_none(),
        "an effort ceiling creates no model list"
    );
    std::fs::write(&path, "").unwrap();
    assert!(
        flow.manager
            .rules_models(ProviderKind::Claude)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "",
        "an empty file stays empty"
    );
    flow.stop().await;
}

/// A rule and a row saved through Settings › Routing (S32) land in the rules file in its own
/// format: `brigadier models` lists them (a kind's order first to last, a rule for one kind of
/// work as written) and `brigadier start` refuses by the rule.
#[tokio::test]
async fn models_page_rules_show_in_models_and_refuse_start() {
    let answers = Answers::default();
    let seen = answers.clone();
    let script: Script = Arc::new(move |turn| {
        let seen = seen.clone();
        Box::pin(async move {
            if turn.input.contains("Try the model.") {
                let models = brigadier(&turn, CliRequest::Models).await;
                let request = StartRequest {
                    model: Some("claude-fable-5-1".into()),
                    local: true,
                    ..start("Reply once.")
                };
                let result = brigadier(&turn, CliRequest::Start(Box::new(request))).await;
                seen.lock().unwrap().insert("models".into(), models);
                seen.lock().unwrap().insert("start".into(), result);
            }
            Reply::text("Checked.")
        })
    });
    let flow = Flow::start("cli-model-page", Options::default(), script).await;
    let row = crate::GuidanceRow {
        work: "Bounded implementation".into(),
        agent: ProviderKind::Claude,
        model: "sonnet".into(),
        effort: Some("medium".into()),
        reason: "My preference.".into(),
    };
    let next = crate::GuidanceRow {
        agent: ProviderKind::Codex,
        model: "gpt-6".into(),
        effort: None,
        reason: "Then this.".into(),
        ..row.clone()
    };
    let page = flow
        .manager
        .set_model_rules(
            &[row.clone(), next.clone()],
            &[
                crate::ModelRule::Forbid {
                    names: vec!["fable".into()],
                    work: None,
                },
                crate::ModelRule::Forbid {
                    names: vec!["haiku".into()],
                    work: Some("Bounded implementation".into()),
                },
            ],
        )
        .unwrap();
    assert_eq!(page.rows, vec![row, next]);
    assert_eq!(
        page.rules[1],
        crate::ModelRule::Forbid {
            names: vec!["haiku".into()],
            work: Some("Bounded implementation".into())
        }
    );
    let text = std::fs::read_to_string(flow.manager.models_rules_path()).unwrap();
    assert!(text.starts_with(super::super::models::DEFAULT_RULES.trim_end()));
    assert!(
        text.contains("\n- forbid: fable\n- forbid for Bounded implementation: haiku\n"),
        "{text}"
    );
    assert!(text.contains(
        "| Bounded implementation | claude | sonnet | medium | My preference. |\n| Bounded implementation | codex | gpt-6 |  | Then this. |\n"
    ));
    flow.say("Try the model.").await;
    flow.settled().await;
    let seen = answers.lock().unwrap().clone();
    assert_eq!(seen["start"]["error"]["code"], "refused", "{seen:#?}");
    assert!(
        seen["start"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("\"forbid: fable\"")
    );
    assert_eq!(
        seen["models"]["rules"],
        json!([
            { "rule": "forbid: fable" },
            { "rule": "forbid for Bounded implementation: haiku" }
        ])
    );
    let guidance = seen["models"]["guidance"].as_array().unwrap();
    let mine: Vec<_> = guidance
        .iter()
        .filter(|g| g["work"] == "Bounded implementation")
        .map(|g| (g["model"].clone(), g["source"].clone()))
        .collect();
    assert_eq!(
        mine,
        [
            (json!("sonnet"), json!("user")),
            (json!("gpt-6"), json!("user"))
        ]
    );
    assert!(flow.board().await.tasks.is_empty());
    flow.stop().await;
}

/// `brigadier start --work` (S32): with no flags the row of that work (the user's, else the
/// shipped one) picks agent, model and effort, even the other provider's; of a work's order, the
/// first row its rules allow; each flag given wins over its field; the user's rules (for every
/// work, or that one) judge the choice that results; the receipt names the row.
#[tokio::test]
async fn start_by_work_follows_the_row_and_the_flags_win() {
    let answers = Answers::default();
    let seen = answers.clone();
    let script: Script = Arc::new(move |turn| {
        let seen = seen.clone();
        Box::pin(async move {
            if matches!(
                turn.host.grants().resolve(&turn.grant),
                Some(Role::Worker { .. })
            ) {
                return Reply::text("Done.");
            }
            if turn.input.contains("Start by work.") {
                let by = |work: &str| StartRequest {
                    work: Some(work.into()),
                    local: true,
                    ..start("Reply once.")
                };
                let starts = [
                    ("row", by("review")),
                    ("shipped", by("Architecture and broad changes")),
                    (
                        "model flag",
                        StartRequest {
                            model: Some("gpt-6-luna".into()),
                            ..by("Review")
                        },
                    ),
                    (
                        "agent flag",
                        StartRequest {
                            agent: Some(brigadier_cli::protocol::Agent::Claude),
                            ..by("Review")
                        },
                    ),
                    (
                        "effort flag",
                        StartRequest {
                            effort: Some("xhigh".into()),
                            ..by("Review")
                        },
                    ),
                    ("unknown", by("Nothing like it")),
                    ("order", by("docs")),
                    (
                        "work rule",
                        StartRequest {
                            agent: Some(brigadier_cli::protocol::Agent::Claude),
                            model: Some("claude-sonnet-5-5".into()),
                            ..by("Docs")
                        },
                    ),
                ];
                for (name, request) in starts {
                    let result = brigadier(&turn, CliRequest::Start(Box::new(request))).await;
                    seen.lock().unwrap().insert(name.into(), result);
                }
            }
            Reply::text("Checked.")
        })
    });
    let flow = Flow::start("cli-start-by-work", Options::default(), script).await;
    std::fs::write(
        flow.manager.models_rules_path(),
        "- effort-ceiling: high\n- forbid for Docs: sonnet\n\n| Work | Agent | Model | Effort | Reason |\n| --- | --- | --- | --- | --- |\n| Review | codex | astra | medium | Mine. |\n| Docs | claude | sonnet | low | First. |\n| Docs | codex | gpt-6-luna | low | Next. |\n",
    )
    .unwrap();
    flow.say("Start by work.").await;
    flow.until("the started workers reported", |b| {
        b.tasks.len() == 5
            && b.tasks
                .values()
                .all(|t| t.state == crate::work::TaskState::Reported)
    })
    .await;
    flow.settled().await;
    let seen = answers.lock().unwrap().clone();
    let worker = |name: &str| {
        let w = &seen[name]["worker"];
        (w["agent"].clone(), w["model"].clone(), w["effort"].clone())
    };
    // The user's row, for the other provider than the Claude session's: a Codex worker, its
    // family word ("Newest Codex astra") the newest model listed of it.
    assert_eq!(
        worker("row"),
        (json!("codex"), json!("gpt-6-astra"), json!("medium")),
        "{seen:#?}"
    );
    assert_eq!(
        seen["row"]["row"],
        json!({ "work": "Review", "source": "user", "picked": ["agent", "model", "effort"] })
    );
    // No row of the user's: the shipped one.
    assert_eq!(
        worker("shipped"),
        (json!("claude"), json!("claude-opus-5-5"), json!("high"))
    );
    assert_eq!(seen["shipped"]["row"]["source"], "shipped");
    // A flag wins over its field only.
    assert_eq!(
        worker("model flag"),
        (json!("codex"), json!("gpt-6-luna"), json!("medium"))
    );
    assert_eq!(
        seen["model flag"]["row"]["picked"],
        json!(["agent", "effort"])
    );
    // Another agent than the row's keeps its own default model; the row's effort still applies.
    assert_eq!(
        worker("agent flag"),
        (json!("claude"), Value::Null, json!("medium"))
    );
    assert_eq!(seen["agent flag"]["row"]["picked"], json!(["effort"]));
    // The rules judge what results.
    assert_eq!(seen["effort flag"]["error"]["code"], "refused");
    assert!(
        seen["effort flag"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("\"effort-ceiling: high\"")
    );
    assert_eq!(seen["unknown"]["error"]["code"], "not-found");
    // The work's order: its first row is kept out by the rule for that work, so the next starts.
    assert_eq!(
        worker("order"),
        (json!("codex"), json!("gpt-6-luna"), json!("low"))
    );
    assert_eq!(seen["order"]["row"]["work"], "Docs");
    assert_eq!(seen["order"]["row"]["source"], "user");
    // A rule for that work refuses the model a flag asks for; it holds for that work only.
    assert_eq!(seen["work rule"]["error"]["code"], "refused");
    assert!(
        seen["work rule"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("\"forbid for Docs: sonnet\"")
    );
    let specs = flow.specs();
    assert!(
        specs
            .iter()
            .any(|(provider, spec)| *provider == ProviderKind::Codex
                && spec.model.as_deref() == Some("gpt-6-astra")
                && spec.effort.as_deref() == Some("medium"))
    );
    flow.stop().await;
}

/// The session's thread starts two workers: one asks permission, one commits a file.
fn rows_script(answers: Answers) -> Script {
    Arc::new(move |turn| {
        let answers = answers.clone();
        Box::pin(async move {
            let keep = |name: &str, value: Value| {
                answers.lock().unwrap().insert(name.into(), value);
            };
            let worker = turn
                .env(GRANT_ENV)
                .and_then(|grant| turn.host.grants().resolve(grant));
            if let Some(Role::Worker { .. }) = worker {
                if turn.input.contains("Ask first.") {
                    return match turn.ask_approval("curl https://example.com", "curl").await {
                        Some(brigadier_providers::ApprovalDecision::Allow) => {
                            Reply::text("Allowed: done.")
                        }
                        other => Reply::text(format!("Not allowed: {other:?}")),
                    };
                }
                std::fs::write(turn.cwd.join("notes.txt"), "one\ntwo\nthree\n").unwrap();
                super::git(&turn.cwd, &["add", "notes.txt"]);
                super::git(&turn.cwd, &["commit", "-qm", "Notes"]);
                return Reply::text("Wrote notes.");
            }
            if !turn.input.contains("Start them.") {
                return Reply::text("Noted.");
            }
            let a = brigadier(
                &turn,
                CliRequest::Start(Box::new(StartRequest {
                    title: Some("Asker".into()),
                    ..start("Ask first.")
                })),
            )
            .await;
            let b = brigadier(
                &turn,
                CliRequest::Start(Box::new(StartRequest {
                    title: Some("Writer".into()),
                    agent: Some(Agent::Codex),
                    ..start("Write notes.")
                })),
            )
            .await;
            let (a_id, b_id) = (id_of(&a), id_of(&b));
            let mut events = wait_for(&turn, &[], 2).await;
            keep("asking", json!(true));
            // The test reads the rows while a's prompt is open.
            super::eventually("the rows to be read", || {
                answers.lock().unwrap().contains_key("read while asking")
            })
            .await;
            brigadier(&turn, send(&a_id, None, Some(Answer::Allow))).await;
            events.extend(wait_for(&turn, &[&a_id], 1).await);
            keep("events", json!(events));
            keep("ids", json!([a_id, b_id]));
            Reply::text("Both done.")
        })
    })
}

/// STRIP-DOWN-PLAN S14: a session's own workers reach its thread view as rows, from their
/// CLIs' events and git: working, needs you with the CLI's permission prompt (answered by
/// the session's `send --allow`), done, +/− and commits against the session's branch, and
/// merged once that branch has them. They come as thread news too, and never as sidebar rows.
#[tokio::test]
async fn a_sessions_workers_are_rows_of_its_view_from_their_cli_and_git() {
    let answers = Answers::default();
    let options = Options {
        permission: crate::model::PermissionLevel::AcceptEdits,
        ..Options::default()
    };
    let flow = Flow::start("cli-worker-rows", options, rows_script(answers.clone())).await;
    let mut news = flow.manager.subscribe_thread_news();
    flow.say("Start them.").await;
    flow.until("a asks", |_| answers.lock().unwrap().contains_key("asking"))
        .await;
    let rows = |view: &crate::thread_view::ThreadView, title: &str| {
        view.workers
            .iter()
            .find(|row| row.title.starts_with(title))
            .cloned()
            .unwrap_or_else(|| panic!("no row {title}: {:#?}", view.workers))
    };
    super::eventually_async("a's prompt in its row", || {
        let manager = flow.manager.clone();
        let id = flow.conversation.clone();
        async move {
            let view = manager.thread_view_with_workers(&id).await.unwrap();
            let row = view
                .workers
                .iter()
                .find(|row| row.title.starts_with("Asker"))
                .cloned();
            row.filter(|row| !row.waiting.is_empty()).is_some()
        }
    })
    .await;
    let view = flow
        .manager
        .thread_view_with_workers(&flow.conversation)
        .await
        .unwrap();
    let a = rows(&view, "Asker");
    assert!(a.working, "{a:#?}");
    assert_eq!(a.waiting.len(), 1);
    assert_eq!(
        a.waiting[0].kind,
        crate::thread_view::WorkerWaitKind::Permission
    );
    assert!(a.waiting[0].what.contains("curl"), "{a:#?}");
    assert!(a.waiting[0].card.is_some());
    assert_eq!(a.agent, ProviderKind::Claude);
    // The prompt came as thread news too, before it is answered (news is debounced).
    let mut sent = Vec::new();
    while !sent
        .iter()
        .any(|rows: &Vec<crate::thread_view::WorkerRow>| {
            rows.iter().any(|row| !row.waiting.is_empty())
        })
    {
        let item = tokio::time::timeout(std::time::Duration::from_secs(20), news.recv())
            .await
            .expect("the prompt's rows as news")
            .unwrap();
        if let Some(rows) = item.workers {
            sent.push(rows);
        }
    }
    answers
        .lock()
        .unwrap()
        .insert("read while asking".into(), json!(true));
    flow.until("both are done", |_| {
        answers.lock().unwrap().contains_key("events")
    })
    .await;
    flow.settled().await;

    let checkout = flow.manager.checkout_dir(&flow.conversation).unwrap();
    let checkout = std::path::PathBuf::from(checkout);
    let parent = super::git(&checkout, &["branch", "--show-current"]);
    let parent = parent.trim().to_owned();
    let read = || async {
        flow.manager
            .worker_rows_of(&flow.conversation, true)
            .await
            .unwrap()
    };
    let now = read().await;
    let by = |rows: &[crate::thread_view::WorkerRow], title: &str| {
        rows.iter()
            .find(|row| row.title.starts_with(title))
            .cloned()
            .unwrap()
    };
    let (a, b) = (by(&now, "Asker"), by(&now, "Writer"));
    // Answered: nothing waits, and its turn ended done.
    assert!(a.waiting.is_empty(), "{a:#?}");
    assert!(!a.working);
    assert_eq!(a.last_end, Some(crate::thread_view::TurnEnd::Done));
    assert_eq!(a.base.as_deref(), Some(parent.as_str()));
    assert!(!a.merged, "nothing of a's to merge");
    // b committed three lines on its branch: not merged yet.
    assert_eq!(b.agent, ProviderKind::Codex);
    assert_eq!(b.last_end, Some(crate::thread_view::TurnEnd::Done));
    assert_eq!(b.ahead, 1, "{b:#?}");
    assert_eq!(b.diff.as_ref().map(|diff| diff.insertions), Some(3));
    assert!(!b.merged);
    // Newest first.
    assert!(now[0].title.starts_with("Writer"));

    // The session merges b's branch: b reads merged.
    let branch = b.branch.clone().unwrap();
    super::git(
        &checkout,
        &["merge", "-q", "--no-ff", "-m", "Merge notes", &branch],
    );
    let b = by(&read().await, "Writer");
    assert!(b.merged, "{b:#?}");
    assert_eq!(b.ahead, 0);
    // Its worktree removed and nothing held in memory, as after a close and a restart: b
    // still reads merged, from the session's checkout.
    let worktree = flow
        .board()
        .await
        .tasks
        .values()
        .find(|task| task.title.starts_with("Writer"))
        .and_then(|task| task.workspace.as_ref()?.worktree.clone())
        .unwrap();
    super::git(&checkout, &["worktree", "remove", "--force", &worktree]);
    flow.manager.forget_worker_reads();
    let b = by(&read().await, "Writer");
    assert!(b.merged, "{b:#?}");
    assert_eq!(b.ahead, 0);

    // The rows came as thread news too, the prompt included, ending with none working (news
    // is debounced: the last may come after the turn settled).
    while !sent
        .last()
        .is_some_and(|rows| rows.iter().all(|row| !row.working))
    {
        let item = tokio::time::timeout(std::time::Duration::from_secs(20), news.recv())
            .await
            .expect("the finished rows as news")
            .unwrap();
        if let Some(rows) = item.workers {
            sent.push(rows);
        }
    }
    assert!(
        sent.iter()
            .any(|rows| rows.iter().any(|row| !row.waiting.is_empty())),
        "{sent:#?}"
    );
    // Never sidebar rows.
    let tasks: Vec<String> = flow
        .board()
        .await
        .tasks
        .keys()
        .map(|id| id.0.clone())
        .collect();
    assert!(
        flow.core
            .visible_catalog()
            .conversations
            .iter()
            .all(|c| !tasks.contains(&c.id.0))
    );
    flow.stop().await;
}

/// S14: a closed worker's thread still opens from its session's Workers row: its CLI's
/// session files go to the session's record, kept until the session is deleted, not with
/// the worker.
#[tokio::test]
async fn a_closed_workers_thread_still_reads_from_its_session() {
    let answers = Answers::default();
    let script: Script = {
        let answers = answers.clone();
        Arc::new(move |turn| {
            let answers = answers.clone();
            Box::pin(async move {
                let worker = turn
                    .env(GRANT_ENV)
                    .and_then(|grant| turn.host.grants().resolve(grant));
                if let Some(Role::Worker { .. }) = worker {
                    return Reply::text("Wrote it.");
                }
                if !turn.input.contains("Start one.") {
                    return Reply::text("Noted.");
                }
                let started =
                    brigadier(&turn, CliRequest::Start(Box::new(start("Write it.")))).await;
                let id = id_of(&started);
                wait_for(&turn, &[&id], 1).await;
                let closed = brigadier(&turn, CliRequest::Close { id: id.clone() }).await;
                answers.lock().unwrap().insert("closed".into(), closed);
                Reply::text("Closed it.")
            })
        })
    };
    let options = Options {
        behavior: Arc::new(FakeBehavior {
            session_file: true,
            cleanup: true,
            ..FakeBehavior::default()
        }),
        ..Options::default()
    };
    let flow = Flow::start("cli-closed-worker-thread", options, script).await;
    flow.say("Start one.").await;
    flow.until("it is closed", |_| {
        answers.lock().unwrap().contains_key("closed")
    })
    .await;
    flow.settled().await;
    let rows = flow
        .manager
        .worker_rows_of(&flow.conversation, true)
        .await
        .unwrap();
    let row = rows.first().cloned().expect("its row stays");
    assert!(row.gone, "{row:#?}");
    assert_eq!(row.last_end, Some(crate::thread_view::TurnEnd::Done));
    // Closed after a turn that ended done, it reads done after a restart too, with none of
    // its CLI's events held.
    flow.manager.forget_worker_reads();
    let rows = flow
        .manager
        .worker_rows_of(&flow.conversation, true)
        .await
        .unwrap();
    assert_eq!(
        rows[0].last_end,
        Some(crate::thread_view::TurnEnd::Done),
        "{rows:#?}"
    );
    super::eventually_async("its thread to read after close", || {
        let manager = flow.manager.clone();
        let id = flow.conversation.clone();
        let task = row.id.clone();
        async move {
            let view = manager.worker_thread(&id, &task).await.unwrap();
            view.entries
                .iter()
                .any(|entry| format!("{entry:?}").contains("Wrote it."))
        }
    })
    .await;
    // Its turn reads done, as its row does: its CLI's file says no turn's end (#556).
    let view = flow
        .manager
        .worker_thread(&flow.conversation, &row.id)
        .await
        .unwrap();
    let ends: Vec<_> = view.turns.iter().map(|turn| turn.end).collect();
    assert_eq!(ends, [Some(crate::thread_view::TurnEnd::Done)], "{view:#?}");
    let lineage = crate::ledger::lineage_owner(&flow.conversation.0);
    let held = flow.manager.runtime.ledger().artifacts(&lineage);
    let sessions = held
        .iter()
        .filter(|artifact| {
            matches!(
                artifact,
                brigadier_providers::Artifact::ClaudeSession { .. }
            )
        })
        .count();
    assert_eq!(sessions, 2, "the session's own and its worker's: {held:#?}");
}

/// A CLI whose end comes after the next CLI of its session started (a seam's predecessor still
/// closing as its successor takes the waiting message) revokes only its own grant: the
/// successor's `brigadier` calls still work (#555).
#[tokio::test]
async fn a_cli_that_ends_after_the_next_one_started_leaves_it_its_grant() {
    let grants: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let seen = grants.clone();
    let flow = Flow::start(
        "cli-late-end",
        Options::default(),
        Arc::new(move |turn| {
            seen.lock().unwrap().push((
                turn.env(SESSION_ENV).unwrap().to_owned(),
                turn.env(GRANT_ENV).unwrap().to_owned(),
            ));
            Box::pin(async { Reply::text("Noted.") })
        }),
    )
    .await;
    flow.say("First.").await;
    flow.idle().await;
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    let old = conv.detach_cli().await.expect("its CLI");
    flow.say("Second.").await;
    flow.idle().await;
    let (session, grant) = grants.lock().unwrap().last().cloned().unwrap();
    assert!(
        !old.grants.contains(&grant),
        "the second turn ran on a new CLI"
    );
    old.session.close().await;
    old.ended.cancelled().await;
    assert!(
        flow.manager.cli_caller(&session, &grant).is_some(),
        "the new CLI's grant was revoked with the old one's"
    );
    assert!(flow.manager.cli_caller(&session, &old.grants[0]).is_none());
    flow.stop().await;
}
