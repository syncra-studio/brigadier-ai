//! Rule R for BUILD-PLAN C10's instruction changes: a session told the instructions before
//! them (revision 35) hears revision 36 (its own shell's output whole up to 30 KB) once, at its
//! next turn, and goes on working.

use std::sync::{Arc, Mutex};

use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::prompts::{SHELL_WHOLE_REVISION, revision_note};
use crate::model::DomainEvent;

type Log = Arc<Mutex<Vec<String>>>;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// Marks the session as told the instructions of revision `told`.
async fn told(flow: &Flow, told: u32) {
    flow.core
        .record(vec![(
            crate::model::streams::orchestrator(&flow.conversation),
            DomainEvent::OrchestratorLogged {
                conversation_id: flow.conversation.clone(),
                entry: crate::work::OrchestratorEntry::Injection {
                    injection: crate::work::ContextInjection {
                        kind: crate::work::InjectionKind::Instructions,
                        bytes: 100,
                        tokens_estimate: 25,
                        label: "instructions revision".into(),
                        task_id: None,
                        told: Some(crate::work::Told {
                            revision: Some(told),
                            ..crate::work::Told::default()
                        }),
                    },
                },
            },
        )])
        .await
        .unwrap();
}

#[tokio::test]
async fn a_session_from_before_hears_the_gate_fixes_once_and_goes_on() {
    let inputs: Log = Arc::default();
    let heard = inputs.clone();
    let mut flow = Flow::start(
        "same-message-revision",
        Options::default(),
        script(move |turn| {
            let heard = heard.clone();
            async move {
                if turn.is_orchestrator() {
                    heard.lock().unwrap().push(turn.input.clone());
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.say("Look around.").await;
    flow.settled().await;
    told(&flow, SHELL_WHOLE_REVISION - 1).await;
    flow.restart().await;
    flow.say("Look again.").await;
    flow.settled().await;
    flow.say("And once more.").await;
    flow.settled().await;

    let note = revision_note(SHELL_WHOLE_REVISION - 1, false).unwrap();
    assert!(note.contains("comes back whole up to 30 KB"), "{note}");
    let inputs = inputs.lock().unwrap().clone();
    let told: usize = inputs
        .iter()
        .map(|input| input.matches(&note).count())
        .sum();
    assert_eq!(told, 1, "{inputs:#?}");
    assert!(inputs.iter().any(|input| input.contains("And once more.")));
    flow.stop().await;
}
