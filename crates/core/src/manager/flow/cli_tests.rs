//! The `brigadier` command's read commands as a session calls them (STRIP-DOWN-PLAN S13):
//! `sessions` lists the project's sidebar sessions and never a worker, `workers` only the
//! calling session's own workers, and a grant that isn't the named session's is refused. Each
//! answer fits its command's checked-in `--json` schema.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use brigadier_cli::protocol::{CliCall, CliReply, CliRequest, GRANT_ENV, PROTOCOL, SESSION_ENV};
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
