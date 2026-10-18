//! A session's state and its reasons on its row (Q73): a failed run needs the user until the
//! next run starts, through a reconnect and a restart; its pull request as last seen on GitHub,
//! looked up when a turn ends and again at a connect for a ready session; a merge reverted on
//! the base reads reverted, and merged again once the revert is reverted.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use super::auto_merge_tests::{squash_merge, stub_gh};
use super::{Flow, Options, Reply, Script, Turn, git};
use crate::work::{ConversationActivity, PullRequestState, TabState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

fn failed_in(flow: &Flow, activity: &[ConversationActivity]) -> bool {
    activity
        .iter()
        .any(|entry| entry.conversation_id == flow.conversation && entry.failed)
}

/// A turn that fails reads Needs you, in the activity a reconnect reads and after a restart,
/// until the next run starts.
#[tokio::test]
async fn a_failed_run_needs_the_user_through_a_reconnect_and_a_restart_until_the_next_run() {
    let mut flow = Flow::start(
        "states-failed",
        Options::default(),
        script(|turn| async move {
            if turn.is_orchestrator() && turn.input.contains("Break.") {
                return Reply::failed("The model is unavailable.");
            }
            Reply::text("Fine.")
        }),
    )
    .await;
    flow.say("Break.").await;
    flow.until("the request failed", |board| {
        board.latest_request().is_some_and(|request| {
            matches!(request.state, crate::work::RequestState::Failed { .. })
        })
    })
    .await;
    flow.settled().await;
    assert!(
        failed_in(&flow, &flow.core.activity().await),
        "a reconnect reads it"
    );
    let tab = flow.core.conversation(&flow.conversation).unwrap();
    assert_eq!(flow.manager.tab_state(&tab).await, Some(TabState::NeedsYou));

    flow.restart().await;
    assert!(
        failed_in(&flow, &flow.core.activity().await),
        "a restart keeps it, though nothing of the session ran since"
    );

    flow.say("Again.").await;
    flow.settled().await;
    assert!(
        !failed_in(&flow, &flow.core.activity().await),
        "the next run cleared it"
    );
    flow.stop().await;
}

/// The thread pushes its work and opens a pull request: the turn's end sees it open. Closed on
/// GitHub while the session is idle, a connect looks it up again (once in a while, not at every
/// read) and records it closed, which a restart keeps. Merged on GitHub, the next message
/// records the merge: the session reads merged, reverted after a revert on the base, and merged
/// again once that revert is reverted.
#[cfg(unix)]
#[tokio::test]
async fn a_pull_request_is_seen_at_a_turns_end_and_a_connect_and_a_reverted_merge_reads_reverted() {
    let scratch: Arc<OnceLock<PathBuf>> = Arc::default();
    let dir = scratch.clone();
    let mut flow = Flow::start(
        "states-pr",
        Options::default(),
        script(move |turn| {
            let dir = dir.clone();
            async move {
                if !turn.is_orchestrator() || !turn.input.contains("Ship the notes.") {
                    return Reply::text("Noted.");
                }
                let scratch = dir.get().unwrap().clone();
                let workspace = turn.add_dirs[0].clone();
                std::fs::write(workspace.join("NOTES.md"), "notes\n").unwrap();
                git(&workspace, &["add", "NOTES.md"]);
                git(&workspace, &["commit", "-q", "-m", "Add notes"]);
                git(&workspace, &["push", "-q", "-u", "origin", "HEAD"]);
                let branch = git(&workspace, &["branch", "--show-current"]);
                let head = git(&workspace, &["rev-parse", "HEAD"]);
                std::fs::write(scratch.join("branch"), &branch).unwrap();
                std::fs::write(scratch.join("head"), &head).unwrap();
                std::fs::write(
                    scratch.join("gh").join("pr-list"),
                    pr_list("OPEN", &head, None),
                )
                .unwrap();
                Reply::text("It's ready: PR #7 is open.")
            }
        }),
    )
    .await;
    let root = flow.repo.parent().unwrap().to_path_buf();
    scratch.set(root.clone()).unwrap();
    let origin = root.join("origin.git");
    git(
        &flow.repo,
        &["clone", "-q", "--bare", ".", origin.to_str().unwrap()],
    );
    git(
        &flow.repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&flow.repo, &["fetch", "-q", "origin"]);
    git(
        &flow.repo,
        &["branch", "--set-upstream-to=origin/main", "main"],
    );
    let gh = root.join("gh");
    *flow.manager.gh_override.lock().unwrap() = Some(stub_gh(&gh));

    flow.say("Ship the notes.").await;
    let board = flow.settled().await;
    let seen = |board: &crate::board::Board| board.pull_request.as_ref().map(|pr| pr.state);
    assert_eq!(
        seen(&board),
        Some(PullRequestState::Open),
        "seen at the turn's end"
    );
    let id = flow.conversation.clone();
    let mine = |tabs: Vec<crate::work::SettledTab>| {
        tabs.into_iter()
            .find(|tab| tab.conversation_id == id)
            .unwrap()
    };
    let settled = mine(flow.manager.settled_tabs().await);
    assert_eq!(
        (settled.state, settled.pull_request),
        (TabState::Ready, Some(PullRequestState::Open))
    );

    // Closed on GitHub while idle: the connect above looked it up already, so the next one
    // waits its turn; a later one sees it closed.
    let head = std::fs::read_to_string(root.join("head")).unwrap();
    std::fs::write(gh.join("pr-list"), pr_list("CLOSED", &head, None)).unwrap();
    let calls = || {
        std::fs::read_to_string(gh.join("calls"))
            .unwrap_or_default()
            .lines()
            .count()
    };
    flow.until("the connect's lookup ran", |_| calls() >= 2)
        .await;
    // (The open pull request's own watch keeps asking `gh` meanwhile: the connect's lookup is
    // told apart by when it last ran.)
    let looked_up = || flow.manager.pr_looked_up.lock().unwrap().get(&id).copied();
    let before = looked_up();
    assert!(before.is_some());
    flow.manager.settled_tabs().await;
    assert_eq!(looked_up(), before, "not looked up at every read");
    flow.manager.pr_looked_up.lock().unwrap().clear();
    flow.manager.settled_tabs().await;
    let board = flow
        .until("seen closed", |board| {
            board.pull_request.as_ref().map(|pr| pr.state) == Some(PullRequestState::Closed)
        })
        .await;
    assert_eq!(seen(&board), Some(PullRequestState::Closed));
    flow.restart().await;
    assert_eq!(
        seen(&flow.board().await),
        Some(PullRequestState::Closed),
        "kept"
    );
    *flow.manager.gh_override.lock().unwrap() = Some(stub_gh(&gh));

    // Merged on GitHub: the next message records the merge before its turn.
    let branch = std::fs::read_to_string(root.join("branch")).unwrap();
    let squash = squash_merge(&origin, &root, &branch);
    std::fs::write(gh.join("pr-list"), pr_list("MERGED", &head, Some(&squash))).unwrap();
    flow.say("Thanks.").await;
    flow.settled().await;
    let tab = flow.core.conversation(&flow.conversation).unwrap();
    assert_eq!(flow.manager.tab_state(&tab).await, Some(TabState::Merged));
    assert_eq!(git(&flow.repo, &["rev-parse", "main"]), squash);

    git(&flow.repo, &["revert", "--no-edit", &squash]);
    assert_eq!(flow.manager.tab_state(&tab).await, Some(TabState::Reverted));
    assert_eq!(
        mine(flow.manager.settled_tabs().await).state,
        TabState::Reverted
    );
    git(&flow.repo, &["revert", "--no-edit", "HEAD"]);
    assert_eq!(
        flow.manager.tab_state(&tab).await,
        Some(TabState::Merged),
        "the revert reverted"
    );
    flow.stop().await;
}

/// `gh pr list`'s answer: pull request #7 of `head` in `state`, merged as `merge_commit`.
fn pr_list(state: &str, head: &str, merge_commit: Option<&str>) -> String {
    serde_json::json!([{
        "number": 7,
        "title": "Add notes",
        "url": "https://github.com/o/n/pull/7",
        "state": state,
        "isDraft": false,
        "baseRefName": "main",
        "headRefOid": head,
        "mergeCommit": merge_commit.map(|oid| serde_json::json!({ "oid": oid })),
    }])
    .to_string()
}
