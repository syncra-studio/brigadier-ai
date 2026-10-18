//! Merge queues: a pull request `gh pr merge` only queued (a required merge queue, or
//! auto-merge waiting for its checks) is watched once the request ends, and its merge is
//! recorded when GitHub lands it, with no message from the user; never under a running
//! command, whose request's end records it instead.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use brigadier_providers::{ItemStatus, ProviderEvent};
use serde_json::json;
use tokio::sync::Notify;

use super::auto_merge_tests::{script, squash_merge, stub_gh};
use super::{Flow, Options, Reply, git};
use crate::board::Board;
use crate::work::OrchestratorStepKind;

/// Writes what the stand-in `gh` lists for the branch: PR #7 into main at `head`.
fn list(root: &Path, state: &str, head: &str, auto: bool, merge_commit: Option<&str>) {
    std::fs::write(
        root.join("gh").join("pr-list"),
        json!([{
            "number": 7,
            "title": "Add notes",
            "url": "https://github.com/o/n/pull/7",
            "state": state,
            "isDraft": false,
            "headRefOid": head,
            "baseRefName": "main",
            "mergeCommit": merge_commit.map(|oid| json!({ "oid": oid })),
            "autoMergeRequest": auto.then(|| json!({ "enabledAt": "2026-10-18T12:00:00Z" })),
        }])
        .to_string(),
    )
    .unwrap();
}

fn merges(board: &Board) -> Vec<(Option<u32>, Vec<String>)> {
    board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            OrchestratorStepKind::Merged { pr, requests, .. } => Some((*pr, requests.clone())),
            _ => None,
        })
        .collect()
}

/// A flow whose repository has a local bare `origin` and the stand-in `gh`; returns the root
/// that holds both.
fn with_origin(flow: &Flow) -> PathBuf {
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
    *flow.manager.gh_override.lock().unwrap() = Some(stub_gh(&root.join("gh")));
    root
}

/// The thread's turn commits and pushes the notes; the pull request is queued, not merged.
async fn ship(workspace: &Path, root: &Path, auto: bool) -> String {
    std::fs::write(workspace.join("NOTES.md"), "notes\n").unwrap();
    git(workspace, &["add", "NOTES.md"]);
    git(workspace, &["commit", "-q", "-m", "Add notes"]);
    git(workspace, &["push", "-q", "-u", "origin", "HEAD"]);
    let head = git(workspace, &["rev-parse", "HEAD"]);
    list(root, "OPEN", &head, auto, None);
    head
}

/// Queued at once (a required merge queue: no auto-merge request shows) or waiting for its
/// checks with auto-merge on: either way, GitHub's merge is recorded on its own once it lands.
#[cfg(unix)]
#[tokio::test]
async fn a_pull_request_queued_at_once_is_recorded_when_github_merges_it() {
    queued_then_merged("merge-queue-admitted", false).await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_pull_request_waiting_for_its_checks_is_recorded_when_github_merges_it() {
    queued_then_merged("merge-queue-auto", true).await;
}

async fn queued_then_merged(name: &str, auto: bool) {
    let scratch: Arc<OnceLock<PathBuf>> = Arc::default();
    let dir = scratch.clone();
    let flow = Flow::start(
        name,
        Options {
            auto_merge: true,
            ..Options::default()
        },
        script(move |turn| {
            let dir = dir.clone();
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("Done.");
                }
                if turn.input.contains("Ship the notes.") {
                    ship(&turn.add_dirs[0], dir.get().unwrap(), auto).await;
                    return Reply::text("PR #7 is queued to merge.");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    let root = with_origin(&flow);
    scratch.set(root.clone()).unwrap();

    flow.say("Ship the notes.").await;
    let board = flow.settled().await;
    let ready = board.latest_request().unwrap().id.clone();
    assert!(merges(&board).is_empty());
    // GitHub merges it a while later.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let workspace = PathBuf::from(flow.manager.checkout_dir(&flow.conversation).unwrap());
    let head = git(&workspace, &["rev-parse", "HEAD"]);
    let branch = git(&workspace, &["branch", "--show-current"]);
    let squash = squash_merge(&root.join("origin.git"), &root, &branch);
    list(&root, "MERGED", &head, false, Some(&squash));

    let board = flow
        .until("the merge recorded", |board| !merges(board).is_empty())
        .await;
    assert_eq!(merges(&board), [(Some(7), vec![ready])]);
    flow.settled().await;
    // Local main is GitHub's, and the session branch sits on it.
    assert_eq!(git(&flow.repo, &["rev-parse", "main"]), squash);
    assert_eq!(git(&workspace, &["rev-parse", "HEAD"]), squash);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(merges(&flow.settled().await).len(), 1, "recorded once");
    flow.stop().await;
}

/// The queued pull request merges while the next request's command runs: nothing moves under
/// the command; the request's end records the merge once, and the branch, with the new
/// commit on it, stays where it is.
#[cfg(unix)]
#[tokio::test]
async fn a_queued_pull_request_merging_while_a_command_runs_is_recorded_after_it() {
    let scratch: Arc<OnceLock<PathBuf>> = Arc::default();
    let (running, finish) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let (dir, ran, done) = (scratch.clone(), running.clone(), finish.clone());
    let flow = Flow::start(
        "merge-queue-busy",
        Options {
            auto_merge: true,
            ..Options::default()
        },
        script(move |turn| {
            let (dir, ran, done) = (dir.clone(), ran.clone(), done.clone());
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("Done.");
                }
                let workspace = turn.add_dirs[0].clone();
                if turn.input.contains("Ship the notes.") {
                    ship(&workspace, dir.get().unwrap(), true).await;
                    return Reply::text("PR #7 is queued to merge.");
                }
                if turn.input.contains("Add more.") {
                    let bash = |status| ProviderEvent::ToolCall {
                        item_id: "bash-1".into(),
                        name: "Bash".into(),
                        input: Some(json!({ "command": "npm test" }).to_string()),
                        status,
                        output: (status != ItemStatus::InProgress).then(|| "[exit 0]\nok".into()),
                        kept: None,
                    };
                    turn.emit(bash(ItemStatus::InProgress)).await;
                    ran.notify_one();
                    done.notified().await;
                    std::fs::write(workspace.join("MORE.md"), "more\n").unwrap();
                    git(&workspace, &["add", "MORE.md"]);
                    git(&workspace, &["commit", "-q", "-m", "Add more"]);
                    turn.emit(bash(ItemStatus::Completed)).await;
                    return Reply::text("More is in.");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    let root = with_origin(&flow);
    scratch.set(root.clone()).unwrap();

    flow.say("Ship the notes.").await;
    let board = flow.settled().await;
    let ready = board.latest_request().unwrap().id.clone();
    flow.say("Add more.").await;
    running.notified().await;
    let workspace = PathBuf::from(flow.manager.checkout_dir(&flow.conversation).unwrap());
    let head = git(&workspace, &["rev-parse", "HEAD"]);
    let branch = git(&workspace, &["branch", "--show-current"]);
    let squash = squash_merge(&root.join("origin.git"), &root, &branch);
    list(&root, "MERGED", &head, false, Some(&squash));
    // The watch saw it merged (and stops looking); nothing moves under the running command.
    let calls = || std::fs::read_to_string(root.join("gh/calls")).unwrap_or_default();
    let before = calls().lines().count();
    while calls().lines().count() < before + 1 {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        merges(&flow.board().await).is_empty(),
        "not under the command"
    );
    assert_eq!(git(&workspace, &["rev-parse", "HEAD"]), head);

    finish.notify_one();
    let board = flow
        .until("the merge recorded", |board| !merges(board).is_empty())
        .await;
    assert_eq!(merges(&board)[0].0, Some(7));
    assert!(merges(&board)[0].1.contains(&ready), "{:?}", merges(&board));
    flow.settled().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(merges(&flow.settled().await).len(), 1, "recorded once");
    assert_eq!(git(&flow.repo, &["rev-parse", "main"]), squash);
    // The new commit stays: the branch is ahead of the merged head, the thread's to rebase.
    assert_eq!(git(&workspace, &["rev-parse", "HEAD~1"]), head);
    flow.stop().await;
}
