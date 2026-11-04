//! A turn a usage limit cuts, whose work goes on by itself on another CLI, ends handed over and
//! never failed (#614, D180): the thread doesn't say "This turn stopped because of an error".
//! Every platform: the CLIs are the flow's in-process fakes.

use std::sync::Arc;

use brigadier_providers::{ProviderKind, QuotaSnapshot, QuotaSource, QuotaWindow};

use super::{FakeBehavior, Flow, Options, Reply};
use crate::thread_view::TurnEnd;
use crate::work::RequestState;

#[tokio::test]
async fn a_limit_that_switches_to_another_cli_ends_its_turn_handed_over_not_failed() {
    let flow = Flow::start(
        "limit-switch-cli",
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
        Arc::new(|turn| {
            Box::pin(async move {
                match turn.provider {
                    ProviderKind::Claude => Reply::limited(),
                    _ => Reply::text("Done on Codex."),
                }
            })
        }),
    )
    .await;
    let mut settings = flow.core.settings();
    settings.switch_providers = true;
    flow.core.update_settings(settings).await.unwrap();
    let now = crate::now_ms();
    flow.manager
        .runtime
        .note_quota_snapshot(
            &crate::accounts::AccountRef::new(ProviderKind::Codex, None),
            QuotaSnapshot {
                provider: ProviderKind::Codex,
                windows: vec![QuotaWindow {
                    id: "primary".into(),
                    label: "primary".into(),
                    used_percent: 10.0,
                    resets_at_ms: Some(now + 3_600_000),
                    window_minutes: Some(300),
                    bucket: None,
                    model: None,
                }],
                limit: None,
                observed_at_ms: now,
                source: QuotaSource::Read,
            },
        )
        .await;

    flow.say("Finish the work.").await;
    flow.until("Codex finishes", |board| {
        board
            .latest_request()
            .is_some_and(|request| request.state == RequestState::Done)
    })
    .await;
    flow.settled().await;

    // Each turn's end as noted: the limited one on Claude, then the one Codex carries on in.
    let record = flow.manager.session_dir(&flow.conversation).record();
    let ends: Vec<_> = std::fs::read_to_string(record.join("turns.jsonl"))
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<crate::thread_view::TurnRecord>(line)
                .unwrap()
                .end
        })
        .collect();
    assert!(
        ends.contains(&Some(TurnEnd::HandedOver)),
        "the limited turn hands over: {ends:?}"
    );
    assert!(
        !ends.contains(&Some(TurnEnd::Failed)),
        "no turn reads failed: {ends:?}"
    );
    // The thread reads the work done, with no failed turn in it.
    let view = flow.manager.thread_view(&flow.conversation).unwrap();
    assert!(
        view.turns
            .iter()
            .all(|turn| turn.end != Some(TurnEnd::Failed)),
        "{:?}",
        view.turns
    );
    flow.stop().await;
}
