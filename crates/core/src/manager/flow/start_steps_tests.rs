//! A new session's setup line (core-goals D206/D208): its turn says each start step it reaches,
//! and a worktree that can't be made ends the turn with why instead of running the CLI in the
//! session's scratch folder.

use std::sync::Arc;

use super::{Flow, Options, Reply, Script, Turn};
use crate::model::DomainEvent;
use crate::work::{RunState, StartStep};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

async fn steps(flow: &Flow) -> Vec<(StartStep, Option<String>)> {
    flow.events_of(&flow.conversation)
        .await
        .into_iter()
        .filter_map(|event| match event {
            DomainEvent::RunStateChanged {
                step: Some(step),
                error,
                ..
            } => Some((step, error)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_new_worktree_turn_says_its_start_steps() {
    let flow = Flow::start(
        "start-steps",
        Options::default(),
        script(|_| async { Reply::text("Looked.") }),
    )
    .await;
    flow.say("Look around.").await;
    flow.settled().await;
    assert_eq!(
        steps(&flow).await,
        vec![
            (StartStep::Fetching, None),
            (StartStep::Worktree, None),
            (StartStep::Cli, None)
        ]
    );
    // The next turn has its worktree: no setup line.
    flow.say("Again.").await;
    flow.settled().await;
    assert_eq!(steps(&flow).await.len(), 3);
}

#[tokio::test]
async fn a_worktree_that_cant_be_made_fails_the_turn() {
    let flow = Flow::start(
        "start-steps-fail",
        Options::default(),
        script(|_| async { Reply::text("Looked.") }),
    )
    .await;
    // Something already sits where the worktree goes: `git worktree add` refuses.
    let work = flow.manager.session_dir(&flow.conversation).work();
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("in-the-way"), "x").unwrap();
    flow.say("Look around.").await;
    let board = flow.settled().await;
    assert_eq!(board.run, RunState::Failed);
    let steps = steps(&flow).await;
    let (last, error) = steps.last().expect("a start step");
    assert_eq!(*last, StartStep::Worktree, "{steps:?}");
    assert!(error.is_some(), "{steps:?}");
    assert!(
        flow.thread_specs().is_empty(),
        "no CLI starts in the scratch folder"
    );
}
