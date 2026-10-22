//! Evidence references (BUILD-PLAN E2's producer side, G3): every `run_check` result ends with
//! the `out-…` id a report or a claim cites, recorded with its status, tree and input
//! fingerprint; an integration, smoke or live check runs every time unless it declares its
//! inputs.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::model::DomainEvent;

type Log = Arc<Mutex<Vec<String>>>;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// The `out-…` id a check's reply ends with.
fn cited(reply: &str) -> String {
    let (_, cite) = reply
        .rsplit_once("[evidence: ")
        .unwrap_or_else(|| panic!("no evidence reference: {reply}"));
    cite.trim_end().trim_end_matches(']').to_owned()
}

#[tokio::test]
async fn every_check_cites_its_output_and_a_live_check_reruns_unless_it_declares_inputs() {
    let replies: Log = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        "evidence-refs",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                let check = |args: serde_json::Value| {
                    let turn = &turn;
                    let log = log.clone();
                    async move {
                        let reply = turn.call("run_check", args).await;
                        assert!(!reply.is_error, "{}", reply.text);
                        log.lock().unwrap().push(reply.text);
                    }
                };
                // An ignored fixture: git's tree doesn't see it change.
                std::fs::write(turn.cwd.join(".gitignore"), "fixture.txt\n").unwrap();
                let fixture = turn.cwd.join("fixture.txt");
                std::fs::write(&fixture, "one\n").unwrap();
                // An ordinary check: run, then from the cache.
                check(json!({ "command": "echo checked" })).await;
                check(json!({ "command": "echo checked" })).await;
                // A smoke check: run both times.
                check(json!({ "command": "echo smoke" })).await;
                check(json!({ "command": "echo smoke" })).await;
                // With its inputs declared: kept, until an input changes.
                check(json!({ "command": "echo smoke", "inputs": ["fixture.txt"] })).await;
                check(json!({ "command": "echo smoke", "inputs": ["fixture.txt"] })).await;
                std::fs::write(&fixture, "two\n").unwrap();
                check(json!({ "command": "echo smoke", "inputs": "fixture.txt" })).await;
                Reply::text("Checked.")
            }
        }),
    )
    .await;
    flow.say("Check it.").await;
    flow.settled().await;

    let replies = replies.lock().unwrap().clone();
    assert_eq!(replies.len(), 7, "{replies:#?}");
    let cached: Vec<bool> = replies
        .iter()
        .map(|reply| reply.starts_with("[cached: "))
        .collect();
    assert_eq!(cached, [false, true, false, false, false, true, false]);
    // A cached answer cites the output it answers with.
    assert_eq!(cited(&replies[0]), cited(&replies[1]));
    assert_ne!(cited(&replies[2]), cited(&replies[3]));

    let checks: Vec<_> = flow
        .events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            DomainEvent::CheckRan {
                command,
                tree,
                cached,
                bypassed,
                status,
                artifact,
                fingerprint,
                ..
            } => Some((
                command,
                tree,
                cached,
                bypassed,
                status,
                artifact,
                fingerprint,
            )),
            _ => None,
        })
        .collect();
    assert_eq!(checks.len(), 7);
    for (index, (_, tree, _, _, status, artifact, _)) in checks.iter().enumerate() {
        assert_eq!(status, "exit 0");
        assert!(tree.is_some(), "every run knows its tree");
        assert_eq!(artifact.as_deref(), Some(cited(&replies[index]).as_str()));
    }
    // A fingerprint only where every input is known: not for an undeclared smoke check.
    let fingerprinted: Vec<bool> = checks.iter().map(|check| check.6.is_some()).collect();
    assert_eq!(fingerprinted, [true, true, false, false, true, true, true]);
    assert!(
        checks[2]
            .3
            .as_deref()
            .is_some_and(|why| why.contains("unless it declares its inputs")),
        "{:?}",
        checks[2].3
    );
    // The declared input is in the fingerprint; git's tree didn't change.
    assert_eq!(checks[4].1, checks[6].1);
    assert_ne!(checks[4].6, checks[6].6);
    flow.stop().await;
}
