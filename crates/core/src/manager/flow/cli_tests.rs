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

use super::{Flow, Options, Reply, Script, Turn};
use crate::tools::Role;

/// What the sessions' `brigadier` calls answered, by name.
type Answers = Arc<Mutex<HashMap<String, Value>>>;

/// The id `brigadier` shows for a full id.
fn short(id: &str) -> String {
    id[id.len() - 8..].to_owned()
}

/// Runs `brigadier <request>` as a command of the turn would: as the session and with the grant
/// its CLI's environment names. Checks the answer against the command's schema; an error answer
/// comes back as `{"error": …}`.
async fn brigadier(turn: &Turn, request: CliRequest) -> Value {
    let session = turn.env(SESSION_ENV).expect("the CLI knows its session");
    let grant = turn
        .env(GRANT_ENV)
        .expect("the CLI has its session's grant");
    let command = match &request {
        CliRequest::Sessions => "sessions",
        CliRequest::Workers => "workers",
        CliRequest::Show { .. } => "show",
        CliRequest::Read { .. } => "read",
        CliRequest::Start(_) => "start",
        CliRequest::Send { .. } => "send",
        CliRequest::Wait { .. } => "wait",
        CliRequest::Close { .. } => "close",
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

/// Each thread starts one worker when asked; the main session's thread looks around with
/// `brigadier` when asked; each worker calls `brigadier` as itself, then reports.
fn looking(answers: Answers) -> Script {
    Arc::new(move |turn| {
        let answers = answers.clone();
        Box::pin(async move {
            let keep = |name: &str, value: Value| {
                answers.lock().unwrap().insert(name.into(), value);
            };
            if let Some(Role::Worker { task_id, .. }) = turn.host.grants().resolve(&turn.grant) {
                keep(&format!("worker env {}", task_id.0), seen_env(&turn));
                keep(
                    "worker sessions",
                    brigadier(&turn, CliRequest::Sessions).await,
                );
                keep(
                    "worker workers",
                    brigadier(&turn, CliRequest::Workers).await,
                );
                let reply = turn
                    .call("submit_report", json!({"summary": "Three lines."}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Reported: three lines.");
            }
            let Some(conversation) = turn.conversation() else {
                return Reply::text("Noted.");
            };
            let me = conversation.0.clone();
            if turn.input.contains("Start a worker.") {
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "medium", "title": "Count the lines",
                               "kind": "scout", "spec": "Count the lines in README.",
                               "provider": "claude"}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Started a worker.");
            }
            if turn.input.contains("Look around.") {
                keep("thread env", seen_env(&turn));
                let sessions = brigadier(&turn, CliRequest::Sessions).await;
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
    let flow = Flow::start("cli-reads", Options::default(), looking(answers.clone())).await;
    let docs = flow.add_session("Docs", None, ProviderKind::Codex).await;
    let elsewhere = flow
        .add_session("Elsewhere", Some("other"), ProviderKind::Claude)
        .await;

    for session in [&flow.conversation, &docs] {
        flow.say_to(session, "Start a worker.").await;
        flow.until_in(session, "its worker reported", |board| {
            board.tasks.values().any(|task| task.report.is_some())
        })
        .await;
        flow.settled_in(session).await;
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
    for task in [&mine, &theirs] {
        let seen = &answers[&format!("worker env {}", task.id.0)];
        assert_eq!(
            seen,
            &json!({"session": task.id.0, "grant is the turn's": true,
                    "data dir": data, "first on path": bin, "grant": seen["grant"]})
        );
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
    assert_eq!(shown["report"], "Three lines.");
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
    assert_eq!(other["agent"], "codex");
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

    // Another project's session isn't found by id.
    assert_eq!(answers["show elsewhere"]["error"]["code"], "not-found");
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
            if let Some(Role::Worker { .. }) = turn.host.grants().resolve(&turn.grant) {
                for n in 0..WORKER_RUNS {
                    turn.emit(command(n)).await;
                }
                let reply = turn
                    .call("submit_report", json!({"summary": "Ran them."}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Ran them all.");
            }
            if turn.input.contains("Start a worker.") {
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "medium", "title": "Run them",
                               "kind": "scout", "spec": "Run the commands.",
                               "provider": "claude"}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
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
    let flow = Flow::start("cli-read-back", Options::default(), script).await;
    flow.say("Run them.").await;
    flow.settled().await;
    let caller = crate::manager::cli::CliCaller {
        conversation_id: flow.conversation.clone(),
        task_id: None,
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

    // `--screen`: the request, the newest 200 of its commands, its answer; older ones left out.
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
    assert_eq!(commands.len(), 200);
    assert_eq!(commands[0], &format!("command echo {}", RUNS - 200));
    assert_eq!(commands[199], &format!("command echo {}", RUNS - 1));
    assert_eq!(screen["hasMore"], true);

    // A worker's `--last N` past one page of its events reads back as far as asked.
    flow.say("Start a worker.").await;
    flow.until("its worker reported", |board| {
        board.tasks.values().any(|task| task.report.is_some())
    })
    .await;
    flow.settled().await;
    let worker = Flow::task(&flow.board().await, 1).id.0.clone();
    let all = read(worker.clone(), Some(1050), false).await;
    let shown = texts(&all);
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
fn start(prompt: &str) -> StartRequest {
    StartRequest {
        title: None,
        agent: None,
        model: None,
        effort: None,
        local: false,
        base: None,
        permission: None,
        lean: false,
        skills: Vec::new(),
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

fn wait(ids: &[&str], timeout_secs: Option<u32>) -> CliRequest {
    CliRequest::Wait {
        ids: ids.iter().map(|id| (*id).to_owned()).collect(),
        timeout_secs,
    }
}

/// Waits through `brigadier wait` until `wanted` events came, keeping them all.
async fn wait_for(turn: &Turn, ids: &[&str], wanted: usize) -> Vec<Value> {
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

fn id_of(started: &Value) -> String {
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
                let other = answers.get("other thread turns").and_then(Value::as_u64);
                answers.insert("other thread turns".into(), json!(other.unwrap_or(0) + 1));
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
            keep("sessions", brigadier(&turn, CliRequest::Sessions).await);
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

    // Each worker's CLI knows itself for `brigadier`, and starts with its prompt alone.
    let bin = flow.manager.config.daemon_exe.parent().unwrap().to_owned();
    for task in [&a, &b, &c] {
        let seen = &answers[&format!("worker env {}", task.id.0)];
        assert_eq!(seen["session"], task.id.0.as_str(), "{seen:#}");
        assert_eq!(seen["first on path"], json!(bin));
        let prompt = &answers[&format!("worker prompt {}", task.id.0)];
        assert_eq!(prompt["input"], task.spec.as_str(), "{prompt:#}");
        assert_eq!(
            prompt["prompt"], "",
            "no Brigadier instructions: {prompt:#}"
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
    assert!(spec.append_system_prompt.is_none());
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
    assert_eq!(a_end["status"], "completed");
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

    // Nothing of theirs was pushed into the thread: it heard them only through `wait`.
    assert_eq!(answers.get("other thread turns"), None);
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
