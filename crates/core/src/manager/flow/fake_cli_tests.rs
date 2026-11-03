//! A fourth CLI is one adapter and one registration line (core-goals D1): the test-only
//! `fakecli` adapter runs a whole conversation through the normal core paths, with no core
//! code naming it.

use std::sync::{Arc, Mutex};

use brigadier_providers::model::Origin;
use brigadier_providers::{ItemStatus, ProviderEvent, ProviderKind};

use super::{FakeBehavior, Flow, Options, Reply};
use crate::work::{OrchestratorStepKind, RequestState};

#[tokio::test]
async fn a_registered_fourth_cli_runs_a_conversation_end_to_end() {
    let kind = ProviderKind::parse("fakecli").unwrap();
    assert!(brigadier_providers::adapters::kinds().any(|known| known == kind));
    let natives: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = natives.clone();
    let script: super::Script = Arc::new(move |turn| {
        seen.lock().unwrap().push(turn.native_id.clone());
        Box::pin(async move {
            if !turn.input.contains("Work.") {
                return Reply::text("Back again.");
            }
            turn.emit(ProviderEvent::MessageDelta {
                item_id: "text-1".into(),
                text: "Looking".into(),
                phase: None,
            })
            .await;
            turn.emit(ProviderEvent::Command {
                item_id: "cmd-1".into(),
                command: "ls".into(),
                cwd: None,
                status: ItemStatus::InProgress,
                exit_code: None,
                output: None,
                duration_ms: None,
                kept: None,
            })
            .await;
            assert!(turn.stopped().await, "the user's stop");
            Reply::default()
        })
    });
    let mut flow = Flow::start(
        "fake-cli",
        Options {
            thread: kind,
            behavior: Arc::new(FakeBehavior {
                cleanup: true,
                ..Default::default()
            }),
            ..Options::default()
        },
        script,
    )
    .await;

    // Send; its text streams and its tool call is a row of the thread.
    flow.say("Work.").await;
    flow.until("the streamed text and the tool row", |board| {
        board.streaming.as_ref().is_some_and(|s| s.text == "Looking")
            && board.orchestrator_steps.iter().any(|step| {
                matches!(&step.kind, OrchestratorStepKind::Tool { item_id, .. } if item_id == "cmd-1")
            })
    })
    .await;
    assert!(
        flow.thread_specs()
            .iter()
            .all(|(provider, _)| *provider == kind)
    );
    let request = flow
        .board()
        .await
        .latest_request()
        .expect("a request")
        .id
        .clone();

    // Interrupt.
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    flow.until("the reply stopped", |board| {
        board.requests[&request].state == RequestState::Stopped
    })
    .await;

    // Restart on the same store and resume the same CLI session.
    let native_id = natives.lock().unwrap()[0].clone();
    flow.restart().await;
    flow.say("Again.").await;
    flow.until("the second answer", |board| {
        board
            .latest_request()
            .is_some_and(|r| r.id != request && r.state == RequestState::Done)
    })
    .await;
    let specs = flow.thread_specs();
    let (provider, resumed) = specs.last().unwrap();
    assert_eq!(*provider, kind);
    assert!(
        matches!(&resumed.origin, Origin::Resume { native_id: id } if *id == native_id),
        "{:?}",
        resumed.origin
    );

    // Delete: its CLI's own files go through its adapter's remove.
    let files = super::fake_files(&flow.dir.join("data/own").join(kind.id()), &native_id);
    assert!(files.iter().all(|file| file.exists()));
    flow.manager
        .delete(flow.conversation.clone())
        .await
        .unwrap();
    flow.manager.cleanup_finished(&flow.conversation).await;
    super::eventually("its files removed", || {
        files.iter().all(|file| !file.exists())
    })
    .await;
    assert!(
        flow.behavior
            .removals
            .lock()
            .unwrap()
            .iter()
            .any(|(provider, _, artifacts)| *provider == kind && !artifacts.is_empty())
    );
    flow.stop().await;
}
