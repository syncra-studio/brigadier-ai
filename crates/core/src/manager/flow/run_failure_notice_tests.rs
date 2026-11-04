//! A failed run that names no request is said as a notice, whose dismissal persists (#639): a
//! turn of Brigadier's own whose CLI dies, and a failure stored before the daemon said such
//! failures as notices, which gets its notice once, on the first read. Every platform: the CLI
//! is the flow's in-process fake.

use std::sync::Arc;

use super::{Flow, Options, Reply};
use crate::manager::conversation::Envelope;
use crate::model::DomainEvent;
use crate::work::{InjectionKind, RunState};

const ENDED: &str = "The CLI session ended unexpectedly.";

/// The notices a fresh read of the session shows.
async fn read(flow: &Flow) -> Vec<(String, i64)> {
    flow.core
        .conversation_view(flow.conversation.clone(), 50)
        .await
        .unwrap()
        .notices
        .into_iter()
        .map(|notice| (notice.text, notice.at_ms))
        .collect()
}

async fn crashing(name: &str) -> Flow {
    Flow::start(
        name,
        Options::default(),
        Arc::new(|turn| {
            Box::pin(async move {
                if turn.input.contains("Crash on this reminder.") {
                    Reply::crashed(9)
                } else {
                    Reply::text("ok")
                }
            })
        }),
    )
    .await
}

#[tokio::test]
async fn a_turn_of_brigadiers_own_that_fails_is_one_notice_whose_dismissal_persists() {
    let mut flow = crashing("run-failure-notice").await;
    flow.say("Start.").await;
    flow.settled().await;
    flow.manager
        .deliver_for(
            &flow.conversation,
            Envelope {
                kind: InjectionKind::Reminder,
                label: "reminder".into(),
                task_id: None,
                text: "Crash on this reminder.".into(),
            },
            None,
        )
        .await;
    let board = flow
        .until("the run to fail", |board| board.run == RunState::Failed)
        .await;
    assert_eq!(board.run_request, None, "no request names the turn");
    flow.settled().await;
    let notices = read(&flow).await;
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].0, ENDED);

    flow.core
        .dismiss_notice(&flow.conversation, notices[0].1)
        .await
        .unwrap();
    assert!(read(&flow).await.is_empty());
    // A restart reads it all from disk: still dismissed, and not said again.
    flow.restart().await;
    assert_eq!(flow.board().await.run, RunState::Failed);
    assert!(read(&flow).await.is_empty());
    flow.restart().await;
    assert!(read(&flow).await.is_empty());
    flow.stop().await;
}

#[tokio::test]
async fn a_failure_stored_without_a_notice_gets_one_once_and_its_dismissal_persists() {
    let mut flow = crashing("run-failure-legacy").await;
    flow.say("Start.").await;
    flow.settled().await;
    // As an older daemon stored a failed run of its own: no notice says it.
    flow.core
        .record_conversation(
            &flow.conversation,
            vec![DomainEvent::RunStateChanged {
                conversation_id: flow.conversation.clone(),
                state: RunState::Failed,
                request_id: None,
                step: None,
                error: Some(ENDED.into()),
            }],
        )
        .await
        .unwrap();
    flow.restart().await;
    let notices = read(&flow).await;
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].0, ENDED);
    // Read again, and after another restart: still the one.
    flow.restart().await;
    assert_eq!(read(&flow).await, notices);

    flow.core
        .dismiss_notice(&flow.conversation, notices[0].1)
        .await
        .unwrap();
    flow.restart().await;
    assert!(read(&flow).await.is_empty());
    flow.stop().await;
}

/// Two reads of the board at once after a restart say a stored failure once, not twice. A
/// notice with its words that was dismissed before neither hides it nor comes back (#639).
#[tokio::test]
async fn reads_at_once_say_a_stored_failure_once_and_an_older_dismissal_hides_nothing() {
    let mut flow = crashing("run-failure-reads-at-once").await;
    flow.say("Start.").await;
    flow.settled().await;
    // An older failure with the same words, said and dismissed.
    flow.core
        .record_conversation(
            &flow.conversation,
            vec![DomainEvent::ConversationNotice {
                conversation_id: flow.conversation.clone(),
                notice: crate::model::Notice {
                    level: brigadier_providers::NoticeLevel::Warning,
                    text: ENDED.into(),
                    at_ms: 1,
                },
            }],
        )
        .await
        .unwrap();
    flow.core
        .dismiss_notice(&flow.conversation, 1)
        .await
        .unwrap();
    // As an older daemon stored a later failed run of its own: no notice says it.
    flow.core
        .record_conversation(
            &flow.conversation,
            vec![DomainEvent::RunStateChanged {
                conversation_id: flow.conversation.clone(),
                state: RunState::Failed,
                request_id: None,
                step: None,
                error: Some(ENDED.into()),
            }],
        )
        .await
        .unwrap();
    flow.restart().await;
    let (first, second) = tokio::join!(read(&flow), read(&flow));
    assert_eq!(first, second);
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(first[0].0, ENDED);
    assert_ne!(first[0].1, 1, "the dismissed one stays dismissed");
    let said = flow
        .events()
        .await
        .into_iter()
        .filter(|event| matches!(event, DomainEvent::ConversationNotice { .. }))
        .count();
    assert_eq!(said, 2, "the older notice and the stored failure's, once");

    flow.core
        .dismiss_notice(&flow.conversation, first[0].1)
        .await
        .unwrap();
    flow.restart().await;
    assert!(read(&flow).await.is_empty());
    flow.stop().await;
}
