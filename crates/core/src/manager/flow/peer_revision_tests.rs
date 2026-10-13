//! Messaging other sessions reaches open sessions (SESSION-MESSAGING-PLAN.md §5.7): a thread
//! whose CLI was told the revision before it resumes its same session at its next turn, with
//! the new tools, and hears exactly that revision's note once; a thread told it hears nothing.

use std::sync::{Arc, Mutex};

use brigadier_providers::ProviderKind;
use brigadier_providers::model::Origin;
use serde_json::json;

use super::peer_kit::Peers;
use super::{Flow, Options, Reply};
use crate::manager::prompts::{PEER_MESSAGING_REVISION, revision_note};
use crate::model::DomainEvent;
use crate::model::streams::orchestrator;
use crate::work::{ContextInjection, InjectionKind, OrchestratorEntry, Told};

async fn an_open_thread_resumes_able_to_message(name: &str, thread: ProviderKind) {
    let peers = Peers::new();
    let mut flow = Flow::start(
        name,
        Options {
            thread,
            ..Options::default()
        },
        peers.script(),
    )
    .await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let listed: Arc<Mutex<Vec<String>>> = Arc::default();
    let kept = listed.clone();
    peers.on(&flow.conversation, move |turn| {
        let kept = kept.clone();
        async move {
            let list = turn.call("list_sessions", json!({})).await;
            assert!(!list.is_error, "{}", list.text);
            kept.lock().unwrap().push(list.text);
            Reply::text("Done.")
        }
    });
    flow.say("First.").await;
    flow.settled().await;
    // A new thread starts on the current instructions: no note.
    assert!(!peers.inputs_of(&flow.conversation)[0].contains("[instructions]"));
    // What the build before logged: its instructions were one revision older.
    flow.core
        .record(vec![(
            orchestrator(&flow.conversation),
            DomainEvent::OrchestratorLogged {
                conversation_id: flow.conversation.clone(),
                entry: OrchestratorEntry::Injection {
                    injection: ContextInjection {
                        kind: InjectionKind::Instructions,
                        bytes: 100,
                        tokens_estimate: 25,
                        label: "instructions revision".into(),
                        task_id: None,
                        told: Some(Told {
                            revision: Some(PEER_MESSAGING_REVISION - 1),
                            ..Told::default()
                        }),
                    },
                },
            },
        )])
        .await
        .unwrap();
    flow.restart().await;
    flow.say("Second.").await;
    flow.until("the second turn", |_| {
        peers.inputs_of(&flow.conversation).len() >= 2
    })
    .await;
    flow.settled().await;
    let specs: Vec<_> = flow
        .thread_specs()
        .into_iter()
        .filter(|(provider, _)| *provider == thread)
        .collect();
    assert!(
        matches!(specs.last().unwrap().1.origin, Origin::Resume { .. }),
        "{specs:#?}"
    );
    let second = peers.inputs_of(&flow.conversation)[1].clone();
    let note = revision_note(PEER_MESSAGING_REVISION - 1, false).unwrap();
    assert!(second.contains(&note), "{second}");
    assert_eq!(second.matches("[instructions]").count(), 1, "{second}");
    // The resumed thread's new tools answer.
    assert!(listed.lock().unwrap()[1].contains(&api.0));
    flow.say("Third.").await;
    flow.until("the third turn", |_| {
        peers.inputs_of(&flow.conversation).len() >= 3
    })
    .await;
    flow.settled().await;
    let third = peers.inputs_of(&flow.conversation)[2].clone();
    assert!(!third.contains("[instructions]"), "{third}");
    flow.stop().await;
}

#[tokio::test]
async fn an_open_claude_thread_resumes_able_to_message_with_the_note() {
    an_open_thread_resumes_able_to_message("peer-revision-claude", ProviderKind::Claude).await;
}

#[tokio::test]
async fn an_open_codex_thread_resumes_able_to_message_with_the_note() {
    an_open_thread_resumes_able_to_message("peer-revision-codex", ProviderKind::Codex).await;
}
