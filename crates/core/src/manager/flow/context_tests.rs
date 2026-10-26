//! `brigadier context` (D13) for a session's thread and for one of its workers: the overview
//! stays small, every part answers in its schema, a worker reads its session's ledger and
//! checkpoint without changing them, and follows a hit in its own records to the whole item.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use brigadier_cli::protocol::{CliRequest, ContextPart, Tool};
use serde_json::{Value, json};

use super::cli_tests::{brigadier, tool};
use super::{Flow, Options, Reply, Script};
use crate::manager::prompts::LEDGER_ENV;

const REQUEST: &str = "Plan the move to the new store.";
const SPEC: &str = "Count the call sites of the old store.";
const LEDGER: &str = "- L1 · 10-25 11:00 · gen 1 · Rule from the user: \"no tabs\" (message 1)\n\
                      - L2 · 10-25 11:01 · gen 1 · Decision: keep the old store read-only (I decided)\n\
                      - L3 · 10-25 11:02 · gen 1 · Open: say when the call sites are counted\n";
const CHECKPOINT: &str = "Goal: move to the new store.\nNext: count the call sites.";
/// Planted in the worker's own records: the start of a long reply, and its end, which no
/// search snippet shows.
const HIT: &str = "quokkacompass";
const TAIL: &str = "lemurtrellis";

fn context(part: Option<ContextPart>) -> CliRequest {
    CliRequest::Context {
        part,
        name: None,
        grep: None,
        last: None,
        item: None,
    }
}

fn transcript(grep: Option<&str>, last: Option<u32>, item: Option<&str>) -> CliRequest {
    CliRequest::Context {
        part: Some(ContextPart::Transcript),
        name: None,
        grep: grep.map(Into::into),
        last,
        item: item.map(Into::into),
    }
}

fn ids(output: &Value, source: &str) -> Vec<String> {
    output["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry["source"] == source)
        .filter_map(|entry| entry["id"].as_str().map(str::to_owned))
        .collect()
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A Claude session file of one user message and one long reply, listed in `record`'s lineage.
fn plant_transcript(record: &Path) {
    let file = record.join("cli").join("w.jsonl");
    let at = "2026-10-25T10:00:00.000Z";
    let reply = format!("{HIT} {} {TAIL}", "filler ".repeat(1200));
    let lines = [
        json!({"type": "user", "timestamp": at, "uuid": "u-1",
               "message": {"role": "user", "content": SPEC}}),
        json!({"type": "assistant", "timestamp": at, "uuid": "a-1",
               "message": {"id": "m-1", "role": "assistant",
                           "content": [{"type": "text", "text": reply}]}}),
    ];
    let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
    write(&file, &text);
    write(
        &record.join("lineage.json"),
        &serde_json::to_string(&json!([{"provider": "claude", "sessionId": "w",
            "file": file.display().to_string(), "startedAt": 1}]))
        .unwrap(),
    );
}

/// Every part answers, in its schema (`brigadier` checks it), for the caller.
async fn every_part(turn: &super::Turn) -> Vec<Value> {
    let mut outputs = Vec::new();
    for part in ContextPart::ALL {
        let output = brigadier(turn, context(Some(part))).await;
        assert!(output.get("error").is_none(), "{part:?}: {output:#}");
        assert_eq!(output["part"], part.word());
        assert!(
            output["text"].as_str().unwrap().ends_with('\n'),
            "{output:#}"
        );
        outputs.push(output);
    }
    outputs
}

#[tokio::test]
async fn a_thread_and_its_worker_pull_their_context() {
    let session_ledger = Arc::new(Mutex::new(None::<PathBuf>));
    let worker_done = Arc::new(Mutex::new(false));
    let script: Script = {
        let (session_ledger, worker_done) = (session_ledger.clone(), worker_done.clone());
        Arc::new(move |turn| {
            let (session_ledger, worker_done) = (session_ledger.clone(), worker_done.clone());
            Box::pin(async move {
                let ledger = PathBuf::from(turn.env(LEDGER_ENV).unwrap());
                if turn.worker().is_none() {
                    write(&ledger, LEDGER);
                    *session_ledger.lock().unwrap() = Some(ledger.clone());
                    brigadier(
                        &turn,
                        tool(Tool::Checkpoint, "set", json!({"text": CHECKPOINT})),
                    )
                    .await;
                    turn.write("CLAUDE.md", "Use spaces.\n");

                    let overview = brigadier(&turn, context(None)).await;
                    assert_eq!(overview["part"], "overview");
                    let text = overview["text"].as_str().unwrap();
                    assert!(text.lines().count() < 40, "{text}");
                    let parts: Vec<&str> = overview["parts"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|part| part["part"].as_str().unwrap())
                        .collect();
                    assert_eq!(parts, ContextPart::ALL.map(ContextPart::word));
                    for part in ContextPart::ALL {
                        assert!(
                            text.contains(&format!("  {}", part.word())),
                            "{part:?}: {text}"
                        );
                    }

                    let [
                        brief,
                        waiting,
                        decisions,
                        open,
                        ledger_part,
                        checkpoint,
                        _,
                        _,
                        _,
                        project,
                        routing,
                    ] = <[Value; 11]>::try_from(every_part(&turn).await).unwrap();
                    assert_eq!(brief["text"], format!("{REQUEST}\n"));
                    assert_eq!(waiting["text"], "No explicit waits.\n");
                    assert_eq!(ids(&decisions, "session"), ["L1", "L2"]);
                    assert!(
                        decisions["text"]
                            .as_str()
                            .unwrap()
                            .contains("Rule from the user: \"no tabs\"")
                    );
                    assert_eq!(ids(&open, "session"), ["L3"]);
                    assert_eq!(ids(&ledger_part, "session"), ["L1", "L2", "L3"]);
                    assert_eq!(checkpoint["text"], format!("{CHECKPOINT}\n"));
                    assert_eq!(project["files"][0]["path"], "CLAUDE.md");
                    assert_eq!(project["files"][0]["text"], "Use spaces.\n");
                    assert!(routing["models"].is_object(), "{routing:#}");
                    let thread = brigadier(&turn, transcript(None, Some(3), None)).await;
                    assert!(thread.get("error").is_none(), "{thread:#}");

                    turn.start_worker("Count call sites", SPEC).await;
                    return Reply::text("Started.");
                }

                // The worker: its own brief, its session's ledger read-only, then its own.
                let [
                    brief,
                    waiting,
                    decisions,
                    open,
                    _,
                    checkpoint,
                    _,
                    _,
                    mcps,
                    _,
                    _,
                ] = <[Value; 11]>::try_from(every_part(&turn).await).unwrap();
                assert_eq!(waiting["text"], "No explicit waits.\n");
                assert_eq!(brief["text"], format!("{SPEC}\n"));
                assert_eq!(ids(&decisions, "session"), ["L1", "L2"]);
                assert!(
                    decisions["text"].as_str().unwrap().contains("read-only"),
                    "{decisions:#}"
                );
                assert_eq!(ids(&open, "session"), ["L3"]);
                assert_eq!(
                    checkpoint["text"],
                    format!("Your session's checkpoint:\n\n{CHECKPOINT}\n")
                );
                // Brigadier's test home has no servers: none to have.
                assert!(
                    mcps["items"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .all(|item| item["has"] == false),
                    "{mcps:#}"
                );
                let added = brigadier(
                    &turn,
                    tool(
                        Tool::Ledger,
                        "add",
                        json!({"text": "Decision: count by hand (I decided)"}),
                    ),
                )
                .await;
                assert_eq!(added["text"], "L1\n");
                let whole = brigadier(&turn, context(Some(ContextPart::Ledger))).await;
                assert_eq!(ids(&whole, "session"), ["L1", "L2", "L3"]);
                assert_eq!(ids(&whole, "own"), ["L1"]);
                let text = whole["text"].as_str().unwrap();
                assert!(
                    text.find("session's ledger, read-only").unwrap()
                        < text.find("Your own ledger").unwrap(),
                    "{text}"
                );

                // Its checkout's own instruction file, not its session's.
                turn.write("AGENTS.md", "Count twice.\n");
                let project = brigadier(&turn, context(Some(ContextPart::Project))).await;
                let files: Vec<&str> = project["files"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|file| file["path"].as_str().unwrap())
                    .collect();
                assert!(files.contains(&"AGENTS.md"), "{project:#}");

                // A hit in its own records, followed to the whole item by the command it names.
                plant_transcript(ledger.parent().unwrap());
                let hits = brigadier(&turn, transcript(Some(HIT), None, None)).await;
                let text = hits["text"].as_str().unwrap();
                assert!(!text.contains(TAIL), "the snippet stops short: {text}");
                let command = text
                    .lines()
                    .find_map(|line| line.strip_prefix("`brigadier transcript read "))
                    .unwrap_or_else(|| panic!("no read command in: {text}"));
                let words: Vec<&str> = command.trim_end_matches('`').split(' ').collect();
                let [key, "--item", item] = words[..] else {
                    panic!("{command}");
                };
                let read = brigadier(
                    &turn,
                    tool(
                        Tool::Transcript,
                        "read",
                        json!({"session": key, "item": item}),
                    ),
                )
                .await;
                assert!(read["text"].as_str().unwrap().contains(TAIL), "{read:#}");
                let read = brigadier(&turn, transcript(None, None, Some(item))).await;
                assert!(read["text"].as_str().unwrap().contains(TAIL), "{read:#}");
                let last = brigadier(&turn, transcript(None, Some(1), None)).await;
                assert!(last["text"].as_str().unwrap().contains(TAIL), "{last:#}");
                // Its session's records are not its own.
                let session = key.split('/').next().unwrap();
                let refused = brigadier(
                    &turn,
                    tool(
                        Tool::Transcript,
                        "read",
                        json!({"session": session, "item": item}),
                    ),
                )
                .await;
                assert!(refused.get("error").is_some(), "{refused:#}");
                let brain =
                    brigadier(&turn, tool(Tool::Brain, "search", json!({"query": HIT}))).await;
                assert_eq!(brain["error"]["code"], "refused", "{brain:#}");
                *worker_done.lock().unwrap() = true;
                Reply::text("Counted.")
            })
        })
    };
    let flow = Flow::start("context", Options::default(), script).await;
    flow.say(REQUEST).await;
    flow.settled().await;
    // The session's request settles at its own turn's end; its worker's turn may still run.
    flow.until("the worker's turn ran to its end", |_| {
        *worker_done.lock().unwrap()
    })
    .await;
    // The worker's `ledger add` went to its own ledger; its session's is as it was.
    let ledger = session_ledger.lock().unwrap().clone().unwrap();
    assert_eq!(std::fs::read_to_string(ledger).unwrap(), LEDGER);
    flow.stop().await;
}
