//! A merge is a plain command (strip-down S5): the thread runs `git merge` itself on the user's
//! words, Brigadier gates and records nothing of it, and the session reads merged from git.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use super::thread_tests::commit_in_workspace;
use super::{Flow, Options, Reply, Script, Turn, git};
use crate::work::{OrchestratorStepKind, TabState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// The thread commits a change on its branch, and the request settles ready. On "Merge it into
/// main." it merges the branch into `main` of the repository's checkout with plain git: the
/// session reads merged, with no `finish_session` call and no merge recorded by Brigadier.
#[tokio::test]
async fn the_thread_merges_with_git_and_the_session_reads_merged_from_git() {
    let checkout: Arc<OnceLock<PathBuf>> = Arc::default();
    let repo = checkout.clone();
    let flow = Flow::start(
        "plain-merge",
        Options::default(),
        script(move |turn| {
            let repo = repo.clone();
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("Noted.");
                }
                if turn.input.contains("Add the notes.") {
                    commit_in_workspace(&turn, "NOTES.md", "notes\n", "Add notes");
                    return Reply::text("Added the notes. Ready to merge.");
                }
                if turn.input.contains("Merge it into main.") {
                    let branch = git(&turn.cwd, &["branch", "--show-current"]);
                    let repo = repo.get().expect("the repository's checkout").clone();
                    git(
                        &repo,
                        &["merge", "-q", "--no-ff", "-m", "Merge the notes", &branch],
                    );
                    return Reply::text("Merged into main.");
                }
                Reply::text("Fine.")
            }
        }),
    )
    .await;
    checkout.set(flow.repo.clone()).unwrap();

    flow.say("Add the notes.").await;
    flow.settled().await;
    assert_eq!(
        flow.settled_state(&flow.conversation).await,
        Some(TabState::Ready)
    );
    let worktree = flow.manager.checkout_dir(&flow.conversation).unwrap();
    let head = git(std::path::Path::new(&worktree), &["rev-parse", "HEAD"]);

    flow.say("Merge it into main.").await;
    let board = flow.settled().await;
    assert_eq!(
        flow.settled_state(&flow.conversation).await,
        Some(TabState::Merged),
        "read from git"
    );
    assert_eq!(
        git(&flow.repo, &["show", "main:NOTES.md"]),
        "notes",
        "the thread's merge is on main"
    );
    let ancestor = crate::test_git_command()
        .args(["merge-base", "--is-ancestor", &head, "main"])
        .current_dir(&flow.repo)
        .status()
        .unwrap();
    assert!(ancestor.success());
    assert!(
        !board
            .orchestrator_steps
            .iter()
            .any(|step| match &step.kind {
                OrchestratorStepKind::Merged { .. } => true,
                OrchestratorStepKind::Tool { name, .. } => name.ends_with("finish_session"),
                _ => false,
            }),
        "no finish_session call and no merge recorded by Brigadier"
    );
    flow.stop().await;
}
