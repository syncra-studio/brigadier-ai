//! `brigadier why` (STRIP-DOWN-PLAN S39): two turns edit one file; each line names the turn
//! that wrote it, with that turn's message, reply and ledger entry, read from the CLI's own
//! session file; with the file gone it says so.

use std::sync::Arc;

use brigadier_cli::protocol::{CliReply, Tool};
use brigadier_providers::{ItemStatus, ProviderEvent};
use serde_json::{Value, json};

use super::cli_tests::{brigadier, tool};
use super::{FakeBehavior, Flow, Options, Reply, Script, git};

const FILE: &str = "app.rs";

fn script() -> Script {
    Arc::new(|turn| {
        Box::pin(async move {
            if !turn.is_orchestrator() {
                return Reply::text("Noted.");
            }
            let id = turn.conversation().unwrap();
            let (checkout, _) = turn.host.changes_checkout(&id).unwrap();
            let file = checkout.join(FILE);
            if turn.input.contains("Add x.") {
                std::fs::write(&file, "fn main() {\n    let x = 1;\n}\n").unwrap();
                let added = brigadier(
                    &turn,
                    tool(
                        Tool::Ledger,
                        "add",
                        json!({"text": "Decision: x starts at 1 (I decided)"}),
                    ),
                )
                .await;
                assert_eq!(added["text"], "L1\n", "{added:#}");
                // The CLI keeps the command it ran in its file, with what it printed.
                turn.emit(ProviderEvent::Command {
                    item_id: "toolu_ledger".into(),
                    command: "brigadier ledger add \"Decision: x starts at 1 (I decided)\"".into(),
                    cwd: None,
                    status: ItemStatus::Completed,
                    exit_code: Some(0),
                    output: Some("L1\n".into()),
                    duration_ms: Some(5),
                    kept: None,
                })
                .await;
                return Reply::text("Added x, starting at 1.");
            }
            if turn.input.contains("Add y.") {
                std::fs::write(&file, "fn main() {\n    let x = 1;\n    let y = 2;\n}\n").unwrap();
                return Reply::text("Added y below x.");
            }
            Reply::text("Looked.")
        })
    })
}

#[tokio::test]
async fn why_names_the_turn_that_wrote_each_line_from_the_cli_files() {
    let flow = Flow::start(
        "20261022-0805-take-over-5-sessions-w163-why",
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
        script(),
    )
    .await;
    flow.say("Add x.").await;
    flow.settled().await;
    flow.say("Add y.").await;
    flow.settled().await;
    let id = flow.conversation.clone();
    let (checkout, _) = flow.manager.changes_checkout(&id).unwrap();
    let path = checkout.join(FILE).display().to_string();
    let why = |line: u32| {
        let manager = flow.manager.clone();
        let path = path.clone();
        async move {
            match manager.cli_why(&path, line).await.unwrap() {
                CliReply::Ok { output } => {
                    let schema = brigadier_cli::commands::find("why").unwrap().schema;
                    let found = brigadier_cli::schema::violations(schema, &output);
                    assert!(found.is_empty(), "{}\n{output:#}", found.join("\n"));
                    output
                }
                CliReply::Error { error } => panic!("{error:?}"),
            }
        }
    };
    let short = id.short().to_owned();
    let x = why(2).await;
    assert_eq!(x["by"], "turn", "{x:#}");
    assert_eq!(x["text"], "    let x = 1;");
    assert_eq!(x["session"]["id"], short.as_str());
    assert_eq!(x["turn"]["number"], 1);
    assert_eq!(x["turn"]["line"], 2);
    assert!(text(&x["turn"]["message"]).contains("Add x."), "{x:#}");
    assert_eq!(x["turn"]["reply"], "Added x, starting at 1.");
    assert!(
        text(&x["turn"]["read"]).starts_with(&format!("brigadier brain read {short} ")),
        "{x:#}"
    );
    let ledger = x["turn"]["ledger"].as_array().unwrap();
    assert_eq!(ledger.len(), 1, "{x:#}");
    assert_eq!(ledger[0]["id"], "L1");
    assert!(text(&ledger[0]["text"]).ends_with("Decision: x starts at 1 (I decided)"));

    let y = why(3).await;
    assert_eq!(y["by"], "turn", "{y:#}");
    assert_eq!(y["turn"]["number"], 2);
    assert!(text(&y["turn"]["message"]).contains("Add y."), "{y:#}");
    assert_eq!(y["turn"]["reply"], "Added y below x.");
    // L1 was written in turn 1, though turn 2 began in the same minute.
    assert_eq!(y["turn"]["ledger"], json!([]), "{y:#}");

    // A line no turn wrote: the user's own, not committed.
    std::fs::write(
        checkout.join(FILE),
        "fn main() {\n    let x = 1;\n    let y = 2;\n}\n// mine\n",
    )
    .unwrap();
    let mine = why(5).await;
    assert_eq!(mine["by"], "uncommitted", "{mine:#}");
    assert!(mine.get("turn").is_none());

    // The CLI cleaned up its session file: the turn is still named, and the loss said.
    let record = flow.manager.session_dir(&id).record();
    let (_, files) = crate::manager::records::transcript_stamp(&record).unwrap();
    assert!(!files.is_empty());
    for (_, file) in &files {
        std::fs::remove_file(file).unwrap();
    }
    let gone = why(3).await;
    assert_eq!(gone["by"], "turn", "{gone:#}");
    assert_eq!(gone["turn"]["number"], 2);
    assert!(gone["turn"]["message"].is_null());
    assert!(
        text(&gone["turn"]["gone"]).starts_with("Its CLI session file is gone"),
        "{gone:#}"
    );
    flow.stop().await;
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

/// A worker's work lands in its session's checkout by a merge in a turn: its lines name the
/// worker and what it was asked, read from its own CLI file.
#[tokio::test]
async fn a_landed_workers_line_names_the_worker() {
    let script: Script = Arc::new(|turn| {
        Box::pin(async move {
            if turn.worker().is_some() {
                turn.write("z.txt", "zeta\n");
                turn.git(&["add", "z.txt"]);
                turn.git(&["commit", "-qm", "Add z"]);
                return Reply::text("Added z.txt on my branch.");
            }
            if turn.input.contains("Start a worker.") {
                turn.start_worker("Add z", "Create z.txt.").await;
                return Reply::text("Started.");
            }
            if turn.input.contains("Land it.") {
                let id = turn.conversation().unwrap();
                let task = turn.host.core.tasks(&id).await.unwrap().remove(0);
                let branch = task.workspace.unwrap().branch.unwrap();
                git(
                    &turn.cwd,
                    &["merge", "-q", "--no-ff", "-m", "Land z", &branch],
                );
                return Reply::text("Landed z.");
            }
            Reply::text("Fine.")
        })
    });
    let flow = Flow::start(
        "20261022-0805-take-over-5-sessions-w163-why-worker",
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
        script,
    )
    .await;
    flow.say("Start a worker.").await;
    flow.until("the worker reports", |board| {
        board.tasks.values().any(|task| {
            matches!(
                task.state,
                crate::work::TaskState::Reported | crate::work::TaskState::Done
            )
        })
    })
    .await;
    flow.settled().await;
    flow.say("Land it.").await;
    flow.settled().await;
    let id = flow.conversation.clone();
    let (checkout, _) = flow.manager.changes_checkout(&id).unwrap();
    let path = checkout.join("z.txt").display().to_string();
    let CliReply::Ok { output } = flow.manager.cli_why(&path, 1).await.unwrap() else {
        panic!("why failed");
    };
    let schema = brigadier_cli::commands::find("why").unwrap().schema;
    let found = brigadier_cli::schema::violations(schema, &output);
    assert!(found.is_empty(), "{}\n{output:#}", found.join("\n"));
    assert_eq!(output["by"], "worker", "{output:#}");
    assert_eq!(output["session"]["id"], id.short());
    assert_eq!(output["worker"]["title"], "Add z");
    assert!(
        text(&output["worker"]["task"]).contains("Create z.txt."),
        "{output:#}"
    );
    assert_eq!(output["worker"]["reply"], "Added z.txt on my branch.");
    assert_eq!(output["commit"]["summary"], "Add z");
    assert!(output["commit"]["merge"].is_string(), "{output:#}");
    // The turn that landed it: its number, and none of the parent's words.
    assert_eq!(output["turn"]["number"], 2, "{output:#}");
    assert!(output["turn"]["message"].is_null());
    flow.stop().await;
}
