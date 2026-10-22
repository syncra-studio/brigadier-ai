//! Brigadier pushes nothing (STRIP-DOWN-PLAN.md S1, G1): an idle thread gets no turn from
//! engine events. Two hours of the engine's timers, a sibling session changing the same file
//! and merging, and the user's settings changing start nothing. A user message still starts a
//! turn, and so does a report of a worker the thread started. Sessions open when the
//! strip-down first starts are archived, not carried over.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::project_sessions_tests::{Inputs, sharing, sibling_of, worktree};
use super::{Flow, Options, Reply, Script};
use crate::model::{Lifecycle, PermissionLevel, Settings, Setup};

/// [`sharing`], plus: "Delegate." starts a worker, which reports at once.
fn delegating(inputs: Inputs) -> Script {
    let share = sharing(inputs.clone());
    Arc::new(move |turn| {
        let (inputs, share) = (inputs.clone(), share.clone());
        Box::pin(async move {
            if turn.task_number().is_some() {
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Nothing to change.", "changes": []}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Reported.");
            }
            if turn.is_orchestrator() && turn.input.trim_end().ends_with("Delegate.") {
                inputs
                    .lock()
                    .unwrap()
                    .push((turn.cwd.clone(), turn.input.clone()));
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "medium", "title": "Look around", "kind": "implement",
                               "spec": "Change nothing.", "provider": "claude"}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Started a worker.");
            }
            share(turn).await
        })
    })
}

async fn edit_settings(flow: &Flow, edit: impl FnOnce(&mut Settings)) {
    let mut settings = flow.core.settings();
    edit(&mut settings);
    flow.core.update_settings(settings).await.unwrap();
}

#[tokio::test]
async fn an_idle_thread_gets_no_turn_from_engine_events() {
    let inputs: Inputs = Arc::default();
    let flow = Flow::start("no-push", Options::default(), delegating(inputs.clone())).await;
    let a = flow.conversation.clone();
    let b = sibling_of(&flow, &a).await;
    flow.say_to(&a, "Share.").await;
    flow.settled_in(&a).await;
    let (Some(folder_a), _, _) = worktree(&flow, &a) else {
        panic!("a's worktree")
    };
    let turns = || {
        inputs
            .lock()
            .unwrap()
            .iter()
            .filter(|(at, _)| at == &folder_a)
            .count()
    };
    let idle = turns();
    assert_eq!(idle, 1);

    // Another session changes the same file and merges.
    flow.say_to(&b, "Share.").await;
    flow.settled_in(&b).await;
    flow.manager
        .merge_session(&b, super::click())
        .await
        .unwrap();
    flow.until_in(&b, "b's merge", |board| {
        board
            .orchestrator_steps
            .iter()
            .any(|step| matches!(step.kind, crate::work::OrchestratorStepKind::Merged { .. }))
    })
    .await;
    flow.settled_in(&b).await;

    // The user's settings change: starting workers, the browser, and A's permission.
    edit_settings(&flow, |settings| {
        settings.start_workers_on_own = !settings.start_workers_on_own;
        settings.browser_enabled = !settings.browser_enabled;
    })
    .await;
    let mut setup = flow.core.conversation(&a).unwrap().setup;
    let Some(Setup::Session { permission, .. }) = &mut setup else {
        panic!("a session");
    };
    *permission = PermissionLevel::Full;
    flow.manager
        .set_setup(a.clone(), setup.unwrap())
        .await
        .unwrap();

    // Two hours of the engine's own timers (status, hibernation, watchdog, checkpoints).
    tokio::time::pause();
    tokio::time::sleep(Duration::from_secs(2 * 60 * 60)).await;
    tokio::time::resume();
    flow.settled_in(&a).await;
    assert_eq!(turns(), idle, "{:#?}", inputs.lock().unwrap());

    // A user message starts a turn.
    flow.say_to(&a, "Write alpha.").await;
    flow.settled_in(&a).await;
    assert_eq!(turns(), idle + 1);

    // A worker the thread started reports: that starts a turn too.
    flow.say_to(&a, "Delegate.").await;
    flow.until_in(&a, "the worker's report", |_| {
        inputs
            .lock()
            .unwrap()
            .iter()
            .any(|(at, input)| at == &folder_a && input.contains("[report task-"))
    })
    .await;
    flow.settled_in(&a).await;
    flow.stop().await;
}

/// The strip-down's first start archives the sessions open then, through the normal archive
/// path, and so does the first start with session folders (S7); a later start leaves sessions
/// alone.
#[tokio::test]
async fn sessions_open_at_the_upgrade_are_archived() {
    let mut flow = Flow::start(
        "no-push-upgrade",
        Options::default(),
        Arc::new(|_| Box::pin(async { Reply::text("Hi.") })),
    )
    .await;
    flow.say("Hello.").await;
    flow.settled().await;
    // As a data folder from before session folders: their marker isn't there yet.
    let marker = flow
        .dir
        .join("data")
        .join(crate::manager::upgrade::MARKERS[1]);
    std::fs::remove_file(&marker).unwrap();
    flow.restart().await;
    assert_eq!(
        flow.core
            .conversation(&flow.conversation)
            .unwrap()
            .lifecycle,
        Lifecycle::Archived
    );
    assert!(marker.exists());
    // Once only: a session open at a later start stays open.
    let later = flow.manager.restore(flow.conversation.clone()).await;
    assert!(later.is_ok(), "{later:?}");
    flow.restart().await;
    assert_ne!(
        flow.core
            .conversation(&flow.conversation)
            .unwrap()
            .lifecycle,
        Lifecycle::Archived
    );
    flow.stop().await;
}
