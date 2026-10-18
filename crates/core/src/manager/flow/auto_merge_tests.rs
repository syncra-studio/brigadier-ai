//! Auto-merge (WORKSPACES-PLAN.md phase 3): with "Merge on its own" on, the thread merges with
//! no consent asked, and its instructions carry the bar as guidance; off, the merge needs the
//! user's consent. A pull request the thread merged on GitHub is recorded as the tab's merge,
//! and the session branch moves onto the new base, so the next request starts from it rather
//! than from the commits a squash merge rewrote.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn, git};
use crate::manager::ci::Gh;
use crate::manager::prompts::AUTO_MERGE;
use crate::work::{OrchestratorStepKind, TabState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// A stand-in `gh`: `pr list` prints the file `pr-list` next to it, or no pull request.
const GH: &str = r#"#!/bin/sh
D="$(dirname "$0")"
echo "$*" >> "$D/calls"
case "$1 $2" in
  "pr list") cat "$D/pr-list" 2>/dev/null || echo "[]" ;;
  *) echo "unknown call: $*" >&2; exit 1 ;;
esac
"#;

fn stub_gh(dir: &Path) -> Gh {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).unwrap();
    let program = dir.join("gh");
    std::fs::write(&program, GH).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    Gh {
        program,
        vars: vec![("PATH".into(), "/usr/bin:/bin".into())],
    }
}

/// Squash-merges `branch` of the bare `origin` into its main, as GitHub's "Squash and merge"
/// does, from a clone in `scratch`. Returns the squash commit.
fn squash_merge(origin: &Path, scratch: &Path, branch: &str) -> String {
    let clone = scratch.join("squash");
    git(
        scratch,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    git(
        &clone,
        &["merge", "-q", "--squash", &format!("origin/{branch}")],
    );
    git(&clone, &["commit", "-q", "-m", "Add notes (#7)"]);
    git(&clone, &["push", "-q", "origin", "main"]);
    git(&clone, &["rev-parse", "HEAD"])
}

/// The thread pushes its commit, a pull request is opened and squash-merged on GitHub in its
/// turn; when the request ends the merge is recorded (PR, head, merge commit, request), local
/// main is brought up to date, the tab reads merged, and the next request starts from the new
/// base with none of the old commits.
#[cfg(unix)]
#[tokio::test]
async fn a_squash_merged_pull_request_is_recorded_and_the_next_request_starts_from_base() {
    let scratch: Arc<OnceLock<PathBuf>> = Arc::default();
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let (dir, said) = (scratch.clone(), log.clone());
    let flow = Flow::start(
        "auto-merge-squash",
        Options {
            auto_merge: true,
            ..Options::default()
        },
        script(move |turn| {
            let (dir, said) = (dir.clone(), said.clone());
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("Done.");
                }
                let scratch = dir.get().unwrap().clone();
                let workspace = turn.add_dirs[0].clone();
                if turn.input.contains("Ship the notes.") {
                    std::fs::write(workspace.join("NOTES.md"), "notes\n").unwrap();
                    git(&workspace, &["add", "NOTES.md"]);
                    git(&workspace, &["commit", "-q", "-m", "Add notes"]);
                    let head = git(&workspace, &["rev-parse", "HEAD"]);
                    let branch = git(&workspace, &["branch", "--show-current"]);
                    // No `-u`: the branch has no upstream, only its remote-tracking ref.
                    git(&workspace, &["push", "-q", "origin", "HEAD"]);
                    let origin = scratch.join("origin.git");
                    let squash = squash_merge(&origin, &scratch, &branch);
                    std::fs::write(
                        scratch.join("gh").join("pr-list"),
                        serde_json::json!([{
                            "number": 7,
                            "title": "Add notes",
                            "url": "https://github.com/o/n/pull/7",
                            "state": "MERGED",
                            "isDraft": false,
                            "headRefOid": head,
                            "baseRefName": "main",
                            "mergeCommit": { "oid": squash },
                        }])
                        .to_string(),
                    )
                    .unwrap();
                    said.lock().unwrap().push(format!("head: {head}"));
                    said.lock().unwrap().push(format!("squash: {squash}"));
                    return Reply::text("PR #7 is merged.");
                }
                if turn.input.contains("Add more.") {
                    let parent = git(&workspace, &["rev-parse", "HEAD"]);
                    said.lock().unwrap().push(format!("started at: {parent}"));
                    std::fs::write(workspace.join("MORE.md"), "more\n").unwrap();
                    git(&workspace, &["add", "MORE.md"]);
                    git(&workspace, &["commit", "-q", "-m", "Add more"]);
                    return Reply::text("More is in.");
                }
                Reply::text("Noted.")
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
    *flow.manager.gh_override.lock().unwrap() = Some(stub_gh(&root.join("gh")));

    flow.say("Ship the notes.").await;
    let board = flow
        .until("the merge recorded", |board| {
            board
                .orchestrator_steps
                .iter()
                .any(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
        })
        .await;
    let said = |what: &str| {
        log.lock()
            .unwrap()
            .iter()
            .find_map(|line| line.strip_prefix(what).map(str::to_owned))
            .unwrap()
    };
    let (head, squash) = (said("head: "), said("squash: "));
    let request = board.latest_request().unwrap().id.clone();
    let merged: Vec<_> = board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            OrchestratorStepKind::Merged {
                head,
                merge_commit,
                pr,
                auto,
                requests,
                base,
                commits,
                ..
            } => Some((
                head.clone(),
                merge_commit.clone(),
                *pr,
                *auto,
                requests.clone(),
                base.clone(),
                *commits,
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        merged,
        [(
            Some(head.clone()),
            Some(squash.clone()),
            Some(7),
            true,
            vec![request],
            "main".to_owned(),
            1
        )]
    );
    flow.settled().await;
    // Local main is GitHub's, and the session branch sits on it.
    assert_eq!(git(&flow.repo, &["rev-parse", "main"]), squash);
    let tab = flow.core.conversation(&flow.conversation).unwrap();
    assert_eq!(flow.manager.tab_state(&tab).await, Some(TabState::Merged));

    flow.say("Add more.").await;
    flow.settled().await;
    assert_eq!(said("started at: "), squash);
    let ancestor = std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", &head, "HEAD"])
        .current_dir(flow.manager.checkout_dir(&flow.conversation).unwrap())
        .status()
        .unwrap();
    assert!(
        !ancestor.success(),
        "the old commit is gone from the branch"
    );
    // Recorded once, though the second request ended with the PR still merged.
    let board = flow.settled().await;
    assert_eq!(
        board
            .orchestrator_steps
            .iter()
            .filter(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
            .count(),
        1
    );
    flow.stop().await;
}

/// A merged pull request into another branch than the tab's base isn't its merge, and one
/// whose merge commit isn't here yet (the fetch failed) is left for later: nothing is recorded
/// and the branch stays where it is.
#[cfg(unix)]
#[tokio::test]
async fn a_pull_request_into_another_branch_or_not_fetched_yet_is_not_recorded() {
    let scratch: Arc<OnceLock<PathBuf>> = Arc::default();
    let dir = scratch.clone();
    let flow = Flow::start(
        "auto-merge-not-ours",
        Options::default(),
        script(move |turn| {
            let dir = dir.clone();
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("Done.");
                }
                let workspace = turn.add_dirs[0].clone();
                if turn.input.contains("Ship it.") {
                    std::fs::write(workspace.join("NOTES.md"), "notes\n").unwrap();
                    git(&workspace, &["add", "NOTES.md"]);
                    git(&workspace, &["commit", "-q", "-m", "Add notes"]);
                    git(&workspace, &["push", "-q", "-u", "origin", "HEAD"]);
                    let head = git(&workspace, &["rev-parse", "HEAD"]);
                    let pr = |base: &str, merged: &str| {
                        serde_json::json!([{
                            "number": 9, "title": "t", "url": "u", "state": "MERGED",
                            "isDraft": false, "headRefOid": head, "baseRefName": base,
                            "mergeCommit": { "oid": merged },
                        }])
                        .to_string()
                    };
                    let gh = dir.get().unwrap().join("gh");
                    std::fs::write(gh.join("pr-list"), pr("release", &head)).unwrap();
                    std::fs::write(
                        gh.join("pr-list.later"),
                        pr("main", "1111111111111111111111111111111111111111"),
                    )
                    .unwrap();
                }
                Reply::text("Done.")
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
    *flow.manager.gh_override.lock().unwrap() = Some(stub_gh(&root.join("gh")));
    let recorded = |board: &crate::board::Board| {
        board
            .orchestrator_steps
            .iter()
            .any(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
    };
    flow.say("Ship it.").await;
    let board = flow.settled().await;
    assert!(!recorded(&board), "a PR into release");
    let checkout = PathBuf::from(flow.manager.checkout_dir(&flow.conversation).unwrap());
    let branch = git(&checkout, &["branch", "--show-current"]);
    let tip = git(&flow.repo, &["rev-parse", &branch]);
    std::fs::rename(root.join("gh/pr-list.later"), root.join("gh/pr-list")).unwrap();
    flow.say("Anything else?").await;
    let board = flow.settled().await;
    assert!(!recorded(&board), "its merge commit isn't here");
    assert_eq!(git(&flow.repo, &["rev-parse", &branch]), tip);
    let calls = std::fs::read_to_string(root.join("gh/calls")).unwrap();
    // Looked up at each request's end (a request may settle more than once).
    assert!(calls.lines().count() >= 2, "{calls}");
    flow.stop().await;
}

type Log = Arc<Mutex<Vec<String>>>;

/// A session whose thread commits NOTES.md on "Ship it." and calls finish_session with no
/// consent; it logs whether its instructions carry the bar, and finish_session's reply.
async fn shipping(name: &str, auto_merge: bool) -> (Flow, Log) {
    let log: Log = Arc::default();
    let said = log.clone();
    let flow = Flow::start(
        name,
        Options {
            auto_merge,
            ..Options::default()
        },
        script(move |turn| {
            let said = said.clone();
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("Done.");
                }
                if turn.input.contains("Ship it.") {
                    said.lock()
                        .unwrap()
                        .push(format!("bar: {}", turn.prompt.contains(AUTO_MERGE)));
                    let workspace = &turn.add_dirs[0];
                    std::fs::write(workspace.join("NOTES.md"), "notes\n").unwrap();
                    git(workspace, &["add", "NOTES.md"]);
                    git(workspace, &["commit", "-q", "-m", "Add notes"]);
                    let reply = turn.call("finish_session", json!({})).await;
                    said.lock()
                        .unwrap()
                        .push(format!("finish_session: {} {}", reply.is_error, reply.text));
                    return Reply::text("Done.");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    (flow, log)
}

fn merges(board: &crate::board::Board) -> Vec<(bool, Option<String>)> {
    board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            OrchestratorStepKind::Merged {
                auto, merge_commit, ..
            } => Some((*auto, merge_commit.clone())),
            _ => None,
        })
        .collect()
}

/// On: the thread's instructions carry the bar, and its finish_session merges with no consent,
/// recorded as an auto merge.
#[tokio::test]
async fn with_merging_on_its_own_on_the_thread_merges_with_no_consent() {
    let (flow, log) = shipping("auto-merge-on", true).await;
    flow.say("Ship it.").await;
    let board = flow.settled().await;
    let log = log.lock().unwrap().clone();
    assert_eq!(log[0], "bar: true");
    assert!(
        log[1].starts_with(
            "finish_session: false [finished] On its own, as the user's setting has it: "
        ),
        "{}",
        log[1]
    );
    let main = git(&flow.repo, &["rev-parse", "main"]);
    assert_eq!(merges(&board), [(true, Some(main))]);
    assert!(
        board.questions.is_empty(),
        "no card: {:#?}",
        board.questions
    );
    flow.stop().await;
}

/// Off: no bar in the instructions, and finish_session with no consent merges nothing.
#[tokio::test]
async fn with_merging_on_its_own_off_a_merge_still_needs_consent() {
    let (flow, log) = shipping("auto-merge-off", false).await;
    flow.say("Ship it.").await;
    let board = flow.settled().await;
    let log = log.lock().unwrap().clone();
    assert_eq!(log[0], "bar: false");
    assert!(log[1].starts_with("finish_session: true "), "{}", log[1]);
    assert!(merges(&board).is_empty());
    assert!(!super::thread_tests::on_main(&flow, "NOTES.md"));
    flow.stop().await;
}

/// A merge held on an auto go-ahead (workers still wrote, or conflicts) doesn't outlive the
/// switch: once the user turns merging on its own off, it needs their consent.
#[tokio::test]
async fn a_merge_held_on_its_own_needs_consent_once_the_switch_is_off() {
    let flow = Flow::start(
        "auto-merge-held-off",
        Options {
            auto_merge: true,
            ..Options::default()
        },
        script(|turn| async move {
            if turn.is_orchestrator() && turn.input.contains("Add notes.") {
                let workspace = &turn.add_dirs[0];
                std::fs::write(workspace.join("NOTES.md"), "notes\n").unwrap();
                git(workspace, &["add", "NOTES.md"]);
                git(workspace, &["commit", "-q", "-m", "Add notes"]);
            }
            Reply::text("Done.")
        }),
    )
    .await;
    flow.say("Add notes.").await;
    flow.settled().await;
    let checkout = PathBuf::from(flow.manager.checkout_dir(&flow.conversation).unwrap());
    let branch = git(&checkout, &["branch", "--show-current"]);
    flow.manager
        .orchestrator_step(
            &flow.conversation,
            OrchestratorStepKind::MergeHeld {
                branch: branch.clone(),
                base: "main".into(),
                asked_in: "auto:1".into(),
                consented_at_ms: 1,
                condition: crate::work::MergeCondition::Now,
                waiting: Vec::new(),
                tip: git(&flow.repo, &["rev-parse", &branch]),
            },
        )
        .await;
    let mut settings = flow.core.settings();
    settings.auto_merge = false;
    flow.core.update_settings(settings).await.unwrap();
    let refused = flow
        .manager
        .finish_session(&flow.conversation, "", None)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("merging on its own was turned off since this merge was held"),
        "{refused}"
    );
    assert!(!super::thread_tests::on_main(&flow, "NOTES.md"));
    flow.stop().await;
}
