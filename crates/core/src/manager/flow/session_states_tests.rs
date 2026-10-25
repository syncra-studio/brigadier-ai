//! A session's state and its reasons on its row (Q73): a CLI turn that failed needs the user
//! until the next turn, through a reconnect and a restart; its pull request as last seen on GitHub,
//! looked up when a turn ends and again at a connect for a ready session; a merge reverted on
//! the base reads reverted, and merged again once the revert is reverted; a session that
//! changed nothing never reads ready or merged.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use super::auto_merge_tests::{squash_merge, stub_gh};
use super::{FakeBehavior, Flow, Options, Reply, Script, Turn, git};
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

/// A CLI turn that fails reads Needs you, in the activity a reconnect reads and after a
/// restart (from `record/turns.jsonl`), until the next turn.
#[tokio::test]
async fn a_failed_turn_needs_the_user_through_a_reconnect_and_a_restart_until_the_next_turn() {
    let mut flow = Flow::start(
        "states-failed",
        Options {
            // Claude echoes the user message, which records the turn.
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
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
        failed_in(&flow, &flow.manager.activity().await),
        "a reconnect reads it"
    );

    flow.restart().await;
    assert!(
        failed_in(&flow, &flow.manager.activity().await),
        "a restart keeps it, though nothing of the session ran since"
    );

    flow.say("Again.").await;
    flow.settled().await;
    assert!(
        !failed_in(&flow, &flow.manager.activity().await),
        "the next turn cleared it"
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
                let workspace = turn.cwd.clone();
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
    assert_eq!(settled.ahead, Some(1), "one commit main lacks");

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
    assert_eq!(flow.settled_state(&tab.id).await, Some(TabState::Merged));
    assert_eq!(git(&flow.repo, &["rev-parse", "main"]), squash);

    let undo = || async { flow.manager.merge_undo(&id, &squash).await.unwrap() };
    assert_eq!(undo().await, None, "never reverted");
    git(&flow.repo, &["revert", "--no-edit", &squash]);
    assert_eq!(flow.settled_state(&tab.id).await, Some(TabState::Reverted));
    assert_eq!(
        mine(flow.manager.settled_tabs().await).state,
        TabState::Reverted
    );
    let revert = git(&flow.repo, &["rev-parse", "HEAD"]);
    let reverted = undo().await.unwrap();
    assert_eq!(
        (reverted.reverted_by.as_str(), reverted.redone_by.as_deref()),
        (revert.as_str(), None)
    );
    let committed: i64 = git(&flow.repo, &["log", "-1", "--format=%ct", "HEAD"])
        .parse()
        .unwrap();
    assert_eq!(reverted.reverted_at_ms, committed * 1000, "when, on main");
    git(&flow.repo, &["revert", "--no-edit", "HEAD"]);
    assert_eq!(
        flow.settled_state(&tab.id).await,
        Some(TabState::Merged),
        "the revert reverted"
    );
    let redo = git(&flow.repo, &["rev-parse", "HEAD"]);
    let redone = undo().await.unwrap();
    assert_eq!(
        (redone.reverted_by, redone.redone_by),
        (revert, Some(redo)),
        "redone"
    );
    assert_eq!(
        flow.manager.merge_undo(&id, "0123456").await.unwrap(),
        None,
        "not a merge of this session's"
    );
    flow.stop().await;
}

/// A session whose branch starts from the base as its remote has it, ahead of the local base,
/// and that changes nothing: the remote's commits aren't its own, so it never reads ready, nor
/// merged once the local base catches up (#495).
#[tokio::test]
async fn a_session_that_changed_nothing_never_reads_ready_or_merged() {
    let flow = Flow::start(
        "states-nothing",
        Options::default(),
        script(|_| async { Reply::text("Nothing to change.") }),
    )
    .await;
    let root = flow.repo.parent().unwrap().to_path_buf();
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
    // Someone pushes to origin's main: the local main is behind it.
    let other = root.join("other");
    git(
        &flow.repo,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    std::fs::write(other.join("UPSTREAM.md"), "upstream\n").unwrap();
    git(&other, &["add", "-A"]);
    git(&other, &["commit", "-q", "-m", "Upstream work"]);
    git(&other, &["push", "-q", "origin", "HEAD:main"]);

    flow.say("Look around.").await;
    let board = flow.settled().await;
    assert!(board.ready.is_empty(), "{:?}", board.ready);
    let id = flow.conversation.clone();
    let state = || async {
        let tab = flow.core.conversation(&id).unwrap();
        flow.settled_state(&tab.id).await
    };
    assert_eq!(state().await, None, "nothing to merge");
    // The local main catches up with its remote: still nothing merged.
    git(&flow.repo, &["merge", "-q", "--ff-only", "origin/main"]);
    assert_eq!(state().await, None, "never merged");
    assert!(
        !flow
            .manager
            .settled_tabs()
            .await
            .iter()
            .any(|tab| tab.conversation_id == id)
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

/// The activity snapshot used on connect knows working workers even without a thread-view
/// read, and keeps them working after their session's own turn ended.
#[tokio::test]
async fn activity_keeps_a_sessions_live_workers_after_its_own_turn_ends() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    let released = Arc::new(AtomicUsize::new(0));
    let gate = released.clone();
    let flow = Flow::start(
        "states-live-workers",
        Options::default(),
        script(move |turn| {
            let gate = gate.clone();
            async move {
                if turn.worker().is_some() {
                    let index = if turn.input.contains("First.") { 1 } else { 2 };
                    while gate.load(Ordering::SeqCst) < index {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    return Reply::text("Finished.");
                }
                if turn.input.contains("Start workers.") {
                    turn.start_worker("First", "First.").await;
                    turn.start_worker("Second", "Second.").await;
                    turn.start_worker("Third", "Third.").await;
                }
                Reply::text("Waiting for the workers.")
            }
        }),
    )
    .await;
    let mut news = flow.manager.subscribe_thread_news();
    flow.say("Start workers.").await;
    flow.idle().await;
    super::eventually_async("the activity snapshot to see live workers", || async {
        flow.manager.activity().await.iter().any(|entry| {
            entry.conversation_id == flow.conversation && !entry.turn_open && entry.workers_working
        })
    })
    .await;
    let before = flow.manager.activity().await;
    let own = before
        .iter()
        .find(|entry| entry.conversation_id == flow.conversation)
        .unwrap();
    let wire = serde_json::to_value(own).unwrap();
    assert_eq!(
        wire["workersWorking"], true,
        "the reconnect's wire snapshot carries the bit"
    );

    released.store(1, Ordering::SeqCst);
    flow.until("the first worker's turn to end", |board| {
        board.tasks.values().any(|task| {
            task.title.starts_with("First") && task.state == crate::work::TaskState::Reported
        })
    })
    .await;
    assert!(
        flow.manager
            .activity()
            .await
            .iter()
            .any(|entry| { entry.conversation_id == flow.conversation && entry.workers_working }),
        "the other workers still work"
    );

    released.store(2, Ordering::SeqCst);
    super::eventually_async(
        "all workers to become idle in the activity snapshot",
        || async {
            !flow
                .manager
                .activity()
                .await
                .iter()
                .any(|entry| entry.conversation_id == flow.conversation && entry.workers_working)
        },
    )
    .await;
    // The same daemon rows reach an unopened sidebar session through live news.
    loop {
        let sent = tokio::time::timeout(Duration::from_secs(20), news.recv())
            .await
            .unwrap()
            .unwrap();
        if sent.conversation_id == flow.conversation
            && let Some(rows) = sent.workers
            && rows.len() == 3
            && rows.iter().all(|row| !row.working)
        {
            break;
        }
    }
    flow.stop().await;
}
