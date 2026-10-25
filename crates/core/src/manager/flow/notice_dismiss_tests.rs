//! A dismissed notice stays dismissed (D51): a later turn, a fresh read of the conversation and a
//! restart don't bring it back; only the problem happening again is a new notice.

use std::sync::Arc;

use brigadier_providers::{NoticeLevel, ProviderEvent};

use super::{Flow, Options, Reply, Script, Turn};

const ACCOUNT: &str = "Your organization has disabled Claude subscription access for Claude Code \
                       · Use an Anthropic API key instead, or ask your admin to enable access";

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

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

#[tokio::test]
async fn a_dismissed_notice_stays_dismissed_until_the_problem_happens_again() {
    let mut flow = Flow::start(
        "notice-dismiss",
        Options::default(),
        script(|turn| async move {
            if turn.is_orchestrator() && turn.input.contains("lost access") {
                turn.emit(ProviderEvent::Notice {
                    level: NoticeLevel::Warning,
                    message: ACCOUNT.into(),
                })
                .await;
            }
            Reply::text("ok")
        }),
    )
    .await;
    flow.say("lost access").await;
    flow.idle().await;
    let first = read(&flow).await;
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(first[0].0, ACCOUNT);

    flow.core
        .dismiss_notice(&flow.conversation, first[0].1)
        .await
        .unwrap();
    assert!(read(&flow).await.is_empty());
    // A later turn that goes well, and a restart that reads it all from disk: still dismissed.
    flow.say("continue now").await;
    flow.idle().await;
    assert!(read(&flow).await.is_empty());
    flow.restart().await;
    assert!(read(&flow).await.is_empty());
    assert!(flow.board().await.notices.is_empty());

    // The problem happening again is a new notice, and it shows.
    flow.say("lost access again").await;
    flow.idle().await;
    let again = read(&flow).await;
    assert_eq!(again.len(), 1, "{again:?}");
    assert_eq!(again[0].0, ACCOUNT);
    assert_ne!(again[0].1, first[0].1);
    flow.stop().await;
}
