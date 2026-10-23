//! Memory safeguards (BUILD-PLAN A11, Q38): what a worker reports under `learned` and what
//! the thread keeps with `remember` on its own are claims, unverified, labelled as such in a
//! later worker's brief, a `query_brain` answer and a rebirth briefing, never as settled
//! decisions; only a cited user message makes a user decision.

use std::sync::{Arc, Mutex};

use brigadier_brain::{NodeFilter, NodeKind, Origin, source_label};
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::model::{DomainEvent, MessageRole};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// The worker's false `learned` line: the notes are in NOTES.md, not a database.
const FALSE_LINE: &str = "Notes are stored in the notes database.";
/// What the thread keeps with `remember` on its own, with no user message and no evidence.
const THREAD_LINE: &str = "Note files are named in kebab case.";

#[derive(Default)]
struct Seen {
    /// Every input the thread got.
    thread: Vec<String>,
    /// The scout's first input: its brief.
    brief: Option<String>,
    /// The thread's `query_brain` answer.
    answer: Option<String>,
}

/// A `remember` that cites the user's real message is a user decision, and one citing a
/// message that isn't the user's, or words it doesn't hold, is refused, as is a preference
/// with no message.
#[tokio::test]
async fn remember_cites_the_users_message_or_is_refused() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let refs: Arc<Mutex<(i64, i64)>> = Arc::default();
    let (log, cited) = (seen.clone(), refs.clone());
    let mut flow = Flow::start(
        "memory-remember",
        Options::default(),
        script(move |turn| {
            let (log, cited) = (log.clone(), cited.clone());
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("Done.");
                }
                log.lock().unwrap().push(turn.input.clone());
                if !turn.input.contains("Keep it.") {
                    return Reply::text("Noted.");
                }
                let (user, mine) = *cited.lock().unwrap();
                let calls = [
                    json!({"kind": "decision", "title": "Use tabs in the Makefile",
                           "user_message": format!("#{user}"), "quote": "tabs in the Makefile, always"}),
                    json!({"kind": "decision", "title": "Use spaces in the Makefile",
                           "user_message": format!("#{mine}")}),
                    json!({"kind": "decision", "title": "Use spaces in the Makefile",
                           "user_message": format!("#{user}"), "quote": "spaces in the Makefile"}),
                    json!({"kind": "preference", "title": "Likes long answers", "personal": true}),
                ];
                for args in calls {
                    let reply = turn.call("remember", args).await;
                    log.lock()
                        .unwrap()
                        .push(format!("remember: {} {}", reply.is_error, reply.text));
                }
                Reply::text("Kept.")
            }
        }),
    )
    .await;
    flow.say("Use tabs in the Makefile, always.").await;
    flow.settled().await;
    // It was told the instructions of the revision before.
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
                    },
                },
            },
        )])
        .await
        .unwrap();
    flow.restart().await;
    let messages = flow
        .core
        .list_messages(flow.conversation.clone(), None, 50)
        .await
        .unwrap()
        .messages;
    let seq_of = |role: MessageRole| {
        messages
            .iter()
            .find(|message| message.role == role)
            .unwrap()
            .seq
    };
    *refs.lock().unwrap() = (seq_of(MessageRole::User), seq_of(MessageRole::Assistant));
    flow.say("Keep it.").await;
    flow.settled().await;
    flow.say("Thanks.").await;
    flow.settled().await;

    let seen = seen.lock().unwrap().clone();
    let replies: Vec<&String> = seen
        .iter()
        .filter(|line| line.starts_with("remember: "))
        .collect();
    assert_eq!(replies.len(), 4, "{replies:?}");
    assert!(
        replies[0].starts_with("remember: false ") && replies[0].contains("(a user decision)"),
        "{}",
        replies[0]
    );
    assert!(
        replies[1].starts_with("remember: true ") && replies[1].contains("isn't the user's"),
        "{}",
        replies[1]
    );
    assert!(
        replies[2].starts_with("remember: true ") && replies[2].contains("doesn't say"),
        "{}",
        replies[2]
    );
    assert!(
        replies[3].starts_with("remember: true ") && replies[3].contains("needs `user_message`"),
        "{}",
        replies[3]
    );
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
    let decisions = brain
        .nodes(&NodeFilter {
            kinds: vec![NodeKind::Decision],
            ..Default::default()
        })
        .unwrap();
    let kept: Vec<_> = decisions
        .iter()
        .filter(|node| node.title.contains("Makefile"))
        .collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept[0].title, "Use tabs in the Makefile");
    assert_eq!(kept[0].provenance.origin, Origin::User);
    assert_eq!(
        source_label(kept[0].kind, &kept[0].provenance),
        "user decision"
    );
    assert!(
        kept[0].body.contains("\"tabs in the Makefile, always\""),
        "{}",
        kept[0].body
    );
    flow.stop().await;
}
