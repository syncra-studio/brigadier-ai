//! Goal 11: each session's context is its own worktree's and branch's only. Two sessions of one
//! project, each on its own branch with its own ledger, transcript, project skill and MCP
//! server, and a worker of the first: every part each pulls holds its own word and never the
//! other session's. The worker pulls its session's thread with `--session`, and only that:
//! never its own records there, nor the other session's.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use brigadier_cli::protocol::{CliRequest, ContextPart};
use brigadier_providers::ProviderKind;
use serde_json::Value;

use super::cli_tests::brigadier;
use super::context_tests::{context, plant_transcript_of, write};
use super::{Flow, Options, Reply, Script};
use crate::manager::prompts::LEDGER_ENV;

const WORDS: [&str; 2] = ["alphaword", "bravoword"];

fn transcript(grep: Option<&str>, last: Option<u32>, session: bool) -> CliRequest {
    CliRequest::Context {
        part: Some(ContextPart::Transcript),
        name: None,
        grep: grep.map(Into::into),
        last,
        item: None,
        session,
    }
}

/// Every part the caller pulls, its branch's skill whole, and its session's thread.
async fn pull(turn: &super::Turn) -> Vec<Value> {
    let mut outputs = Vec::new();
    for part in ContextPart::ALL {
        outputs.push(brigadier(turn, context(Some(part))).await);
    }
    outputs.push(
        brigadier(
            turn,
            CliRequest::Context {
                part: Some(ContextPart::Skills),
                name: Some("branch-skill".into()),
                grep: None,
                last: None,
                item: None,
                session: false,
            },
        )
        .await,
    );
    outputs
}

#[tokio::test]
async fn two_sessions_on_two_branches_never_see_each_others_context() {
    let pulled: Arc<Mutex<HashMap<String, Vec<Value>>>> = Arc::default();
    let script: Script = {
        let pulled = pulled.clone();
        Arc::new(move |turn| {
            let pulled = pulled.clone();
            Box::pin(async move {
                let Some(word) = WORDS
                    .into_iter()
                    .find(|word| turn.input.contains(word) || turn.prompt.contains(word))
                else {
                    return Reply::text("No word.");
                };
                if let Some(_task) = turn.worker() {
                    let mut outputs = pull(&turn).await;
                    let thread = brigadier(&turn, transcript(Some(word), None, true)).await;
                    let own = brigadier(&turn, transcript(Some(word), None, false)).await;
                    let ledger = PathBuf::from(turn.env(LEDGER_ENV).unwrap());
                    plant_transcript_of(ledger.parent().unwrap(), "workerword");
                    let not_in_thread =
                        brigadier(&turn, transcript(Some("workerword"), None, true)).await;
                    let last = brigadier(&turn, transcript(None, Some(2), true)).await;
                    outputs.extend([thread, own, not_in_thread, last]);
                    pulled
                        .lock()
                        .unwrap()
                        .insert(format!("{word}-worker"), outputs);
                    return Reply::text("Pulled.");
                }
                if turn.input.contains("plant") {
                    let ledger = PathBuf::from(turn.env(LEDGER_ENV).unwrap());
                    write(
                        &ledger,
                        &format!(
                            "- L1 · 10-25 11:00 · gen 1 · Decision: {word} stays (I decided)\n\
                             - L2 · 10-25 11:01 · gen 1 · Open: say {word} when done\n"
                        ),
                    );
                    plant_transcript_of(ledger.parent().unwrap(), word);
                    turn.write(
                        ".claude/skills/branch-skill/SKILL.md",
                        &format!("---\nname: branch-skill\ndescription: d\n---\nSay {word}.\n"),
                    );
                    turn.write(
                        ".mcp.json",
                        &format!(
                            r#"{{"mcpServers":{{"{word}-api":{{"command":"/not-started"}}}}}}"#
                        ),
                    );
                    turn.write("AGENTS.md", &format!("Use {word}.\n"));
                    // On its branch, so a worker's worktree off it has them too.
                    super::git(&turn.cwd, &["add", "-A"]);
                    super::git(&turn.cwd, &["commit", "-q", "-m", word]);
                    return Reply::text("Planted.");
                }
                let outputs = pull(&turn).await;
                pulled.lock().unwrap().insert(word.to_owned(), outputs);
                if word == WORDS[0] {
                    turn.start_worker("Pull", &format!("Pull {word}.")).await;
                }
                Reply::text("Pulled.")
            })
        })
    };
    let flow = Flow::start("scope", Options::default(), script).await;
    let other = flow.add_session("Other", None, ProviderKind::Claude).await;
    flow.say(&format!("plant {}", WORDS[0])).await;
    flow.settled().await;
    flow.say_to(&other, &format!("plant {}", WORDS[1])).await;
    flow.settled_in(&other).await;
    flow.manager.registry().refresh();
    flow.say(&format!("pull {}", WORDS[0])).await;
    flow.settled().await;
    flow.say_to(&other, &format!("pull {}", WORDS[1])).await;
    flow.settled_in(&other).await;
    for word in WORDS {
        let pulled = pulled.lock().unwrap();
        for output in pulled.get(word).expect("pulled") {
            assert!(output.get("error").is_none(), "{word}: {output:#}");
        }
    }
    let worker = format!("{}-worker", WORDS[0]);
    flow.until("the worker pulled", |_| {
        pulled.lock().unwrap().contains_key(&worker)
    })
    .await;

    let pulled = pulled.lock().unwrap().clone();
    for output in &pulled[&worker] {
        assert!(output.get("error").is_none(), "worker: {output:#}");
    }
    for (own, theirs) in [(WORDS[0], WORDS[1]), (WORDS[1], WORDS[0])] {
        let outputs = &pulled[own];
        let all = Value::Array(outputs.clone()).to_string();
        assert!(!all.contains(theirs), "{own} saw {theirs}: {all}");
        let part = |part: ContextPart| {
            outputs[ContextPart::ALL.iter().position(|p| *p == part).unwrap()].to_string()
        };
        for part_of in [
            ContextPart::Decisions,
            ContextPart::Open,
            ContextPart::Ledger,
            ContextPart::Mcps,
            ContextPart::Project,
        ] {
            assert!(
                part(part_of).contains(own),
                "{part_of:?}: {}",
                part(part_of)
            );
        }
        // Its branch's own skill, whole.
        assert!(
            outputs[ContextPart::ALL.len()]["text"]
                .as_str()
                .unwrap()
                .contains(&format!("Say {own}.")),
            "{:#}",
            outputs[ContextPart::ALL.len()]
        );
    }

    // The worker: its session's everything, on hand, and nothing of the other session's.
    let outputs = &pulled[&worker];
    let all = Value::Array(outputs.clone()).to_string();
    assert!(
        !all.contains(WORDS[1]),
        "the worker saw {}: {all}",
        WORDS[1]
    );
    let n = ContextPart::ALL.len();
    let [thread, own, not_in_thread, last] =
        <[Value; 4]>::try_from(outputs[n + 1..].to_vec()).unwrap();
    assert!(
        outputs[n]["text"]
            .as_str()
            .unwrap()
            .contains("Say alphaword.")
    );
    let mcps = &outputs[ContextPart::ALL
        .iter()
        .position(|p| *p == ContextPart::Mcps)
        .unwrap()];
    assert!(
        mcps["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["name"] == "alphaword-api" && item["has"] == false),
        "{mcps:#}"
    );
    assert!(
        thread["text"].as_str().unwrap().contains("alphaword"),
        "{thread:#}"
    );
    assert!(
        !own["text"].as_str().unwrap().contains("alphaword"),
        "{own:#}"
    );
    assert!(
        !not_in_thread["text"]
            .as_str()
            .unwrap()
            .contains("workerword"),
        "{not_in_thread:#}"
    );
    assert!(
        last["text"].as_str().unwrap().contains("alphaword"),
        "{last:#}"
    );
    flow.stop().await;
}
