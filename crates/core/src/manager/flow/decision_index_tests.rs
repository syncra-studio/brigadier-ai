//! The decision index (BUILD-PLAN D1.2, Q41.1): a briefing carries one compact line per
//! decision of the conversation (its id, scope, status and what it replaced), replaced ones
//! too, on top of every active decision's whole body; nothing is paged out, and the briefing's
//! sections report the index's size for the Inspector.

use std::sync::{Arc, Mutex};

use brigadier_brain::{NewNode, NodeKind, Origin, Provenance};

use super::{Flow, Options, Reply, Script, Turn};
use crate::knowledge::RebirthRecord;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

fn decision(conversation: &str, title: String, body: String) -> NewNode {
    NewNode {
        kind: NodeKind::Decision,
        key: None,
        title,
        body,
        provenance: Provenance {
            origin: Origin::User,
            session_id: Some(conversation.to_owned()),
            task_id: None,
            job_id: None,
            worker: None,
            commit: None,
            recorded_at_ms: crate::now_ms(),
            claim: None,
        },
        files: Vec::new(),
        expires_at_ms: None,
    }
}

/// 300 decisions, 50 of them replaced: the size rebirth's briefing indexes all 300 with the
/// right status and links, carries the 250 active bodies whole and no replaced one, and its
/// record measures the index.
#[tokio::test]
async fn a_rebirth_indexes_three_hundred_decisions_and_carries_every_active_body() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let flow = Flow::start(
        "decision-index",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.input.contains("Write the handoff note") {
                    return Reply::text("Goal and open threads: none.\nWhat to do next: wait.");
                }
                log.lock().unwrap().push(turn.input.clone());
                if turn.input.contains("Grow.") && !turn.input.contains("[Brigadier briefing") {
                    return Reply {
                        text: "Grown.".into(),
                        context_tokens: Some(460_000),
                        ..Reply::default()
                    };
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("First.").await;
    flow.settled().await;
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id
        .unwrap();
    let brain = flow
        .manager
        .project_brain(&project)
        .await
        .unwrap()
        .brain
        .clone();
    let conversation = flow.conversation.0.clone();
    let (originals, revised) = tokio::task::spawn_blocking(move || {
        let originals: Vec<String> = (0..250)
            .map(|n| {
                brain
                    .record(decision(
                        &conversation,
                        format!("Decision {n}"),
                        format!("Body of decision {n}: keep part {n} as it is."),
                    ))
                    .unwrap()
            })
            .collect();
        let revised: Vec<String> = (0..50)
            .map(|n| {
                brain
                    .record_replacing(
                        decision(
                            &conversation,
                            format!("Decision {n}, revised"),
                            format!("Body of revised decision {n}: part {n} moves."),
                        ),
                        &originals[n],
                        "replaced by the user",
                    )
                    .unwrap()
            })
            .collect();
        (originals, revised)
    })
    .await
    .unwrap();
    flow.say("Grow.").await;
    flow.settled().await;
    flow.say("After.").await;
    flow.settled().await;

    let inputs = inputs.lock().unwrap().clone();
    let briefing = inputs
        .iter()
        .find(|input| input.contains("[Brigadier briefing"))
        .expect("a reborn session's briefing");
    let index = briefing
        .split("[Decision index:")
        .nth(1)
        .and_then(|rest| rest.split("\n\n").next())
        .expect("the index");
    assert_eq!(
        index.lines().filter(|l| l.starts_with("- ")).count(),
        300,
        "{index}"
    );
    for n in 0..250 {
        let id = &originals[n];
        if n < 50 {
            let new = &revised[n];
            assert!(
                index.contains(&format!(
                    "- {id} · session · replaced by {new} · Decision {n}\n"
                )),
                "{id}"
            );
            assert!(
                index.contains(&format!(
                    "- {new} · session · holds · replaces {id} · Decision {n}, revised"
                )),
                "{new}"
            );
            assert!(briefing.contains(&format!("Body of revised decision {n}: part {n} moves.")));
            assert!(
                !briefing.contains(&format!("Body of decision {n}: ")),
                "a replaced body: {n}"
            );
        } else {
            assert!(
                index.contains(&format!("- {id} · session · holds · Decision {n}")),
                "{id}"
            );
            assert!(briefing.contains(&format!("Body of decision {n}: keep part {n} as it is.")));
        }
    }
    let records: Vec<RebirthRecord> = flow
        .core
        .list_orchestrator_log(&flow.conversation, None, brigadier_store::MAX_PAGE)
        .await
        .unwrap()
        .entries
        .into_iter()
        .filter_map(|entry| match entry.entry {
            crate::work::OrchestratorEntry::Rebirth { record } => Some(*record),
            _ => None,
        })
        .collect();
    assert_eq!(records.len(), 1);
    let section = |name: &str| {
        records[0]
            .sections
            .iter()
            .find(|section| section.name == name)
            .unwrap_or_else(|| panic!("{name}: {:?}", records[0].sections))
            .clone()
    };
    let index = section("decision-index");
    assert_eq!(index.items, 300);
    assert!(index.tokens > 0);
    assert_eq!(index.note.as_deref(), Some("250 active, 50 replaced"));
    assert_eq!(section("decisions").items, 250);
    assert_eq!(records[0].decisions_in_full, 250);
    assert!(records[0].briefing_tokens >= index.tokens);
    flow.stop().await;
}
