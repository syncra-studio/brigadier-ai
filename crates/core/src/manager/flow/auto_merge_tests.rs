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

pub(super) fn script<F, Fut>(f: F) -> Script
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

pub(super) fn stub_gh(dir: &Path) -> Gh {
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
pub(super) fn squash_merge(origin: &Path, scratch: &Path, branch: &str) -> String {
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
    squash_merged("auto-merge-squash", false).await;
}

/// A clone that fetches only main (`git clone --depth`, `--single-branch`) gets no
/// remote-tracking ref for a pushed branch: its configured upstream still says it was pushed.
#[cfg(unix)]
#[tokio::test]
async fn a_pull_request_from_a_single_branch_clone_is_recorded_too() {
    squash_merged("auto-merge-single-branch", true).await;
}

async fn squash_merged(name: &str, single_branch: bool) {
    let scratch: Arc<OnceLock<PathBuf>> = Arc::default();
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let (dir, said) = (scratch.clone(), log.clone());
    let flow = Flow::start(
        name,
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
                    if single_branch {
                        // An upstream, but no remote-tracking ref.
                        git(&workspace, &["push", "-q", "-u", "origin", "HEAD"]);
                        assert!(
                            git(
                                &workspace,
                                &["for-each-ref", "refs/remotes/origin/brigadier"]
                            )
                            .is_empty()
                        );
                    } else {
                        // No `-u`: the branch has no upstream, only its remote-tracking ref.
                        git(&workspace, &["push", "-q", "origin", "HEAD"]);
                    }
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
    if single_branch {
        git(
            &flow.repo,
            &[
                "config",
                "remote.origin.fetch",
                "+refs/heads/main:refs/remotes/origin/main",
            ],
        );
    }
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

/// The thread leaves its pushed work ready and the user squash-merges the pull request on
/// GitHub while the tab is idle (merging on its own off): the user's next message records the
/// merge on the ready request before its turn, which starts from the new base and hears that
/// GitHub may have kept the old branch.
#[cfg(unix)]
#[tokio::test]
async fn a_pull_request_the_user_merged_while_the_tab_was_idle_is_recorded_before_the_next_turn() {
    let scratch: Arc<OnceLock<PathBuf>> = Arc::default();
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let (dir, said) = (scratch.clone(), log.clone());
    let flow = Flow::start(
        "auto-merge-idle-pr",
        Options::default(),
        script(move |turn| {
            let (dir, said) = (dir.clone(), said.clone());
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("Done.");
                }
                let workspace = turn.add_dirs[0].clone();
                if turn.input.contains("Ship the notes.") {
                    std::fs::write(workspace.join("NOTES.md"), "notes\n").unwrap();
                    git(&workspace, &["add", "NOTES.md"]);
                    git(&workspace, &["commit", "-q", "-m", "Add notes"]);
                    git(&workspace, &["push", "-q", "-u", "origin", "HEAD"]);
                    let _ = dir.get();
                    return Reply::text("It's ready: PR #7 is open.");
                }
                if turn.input.contains("Add more.") {
                    let parent = git(&workspace, &["rev-parse", "HEAD"]);
                    said.lock().unwrap().push(format!("started at: {parent}"));
                    said.lock().unwrap().push(format!("input: {}", turn.input));
                    return Reply::text("More is in.");
                }
                said.lock().unwrap().push(format!("other: {}", turn.input));
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
    // A sibling session in the same project.
    let sibling = flow
        .manager
        .create_session_from(&flow.conversation, None)
        .await
        .unwrap()
        .id;

    flow.say("Ship the notes.").await;
    let board = flow.settled().await;
    let ready = board.latest_request().unwrap().id.clone();
    assert!(
        !board
            .orchestrator_steps
            .iter()
            .any(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
    );
    // The user merges it on GitHub while the tab is idle.
    let workspace = PathBuf::from(flow.manager.checkout_dir(&flow.conversation).unwrap());
    let head = git(&workspace, &["rev-parse", "HEAD"]);
    let branch = git(&workspace, &["branch", "--show-current"]);
    let squash = squash_merge(&origin, &root, &branch);
    std::fs::write(
        root.join("gh").join("pr-list"),
        json!([{
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

    flow.say("Add more.").await;
    let board = flow.settled().await;
    let said = |what: &str| {
        log.lock()
            .unwrap()
            .iter()
            .find_map(|line| line.strip_prefix(what).map(str::to_owned))
            .unwrap()
    };
    assert_eq!(said("started at: "), squash);
    let input = said("input: ");
    assert!(
        input.contains("[pull request] PR #7 was merged on GitHub"),
        "{input}"
    );
    assert!(input.contains("`--force-with-lease`"), "{input}");
    let merged: Vec<_> = board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            OrchestratorStepKind::Merged {
                pr, auto, requests, ..
            } => Some((*pr, *auto, requests.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(merged, [(Some(7), false, vec![ready])]);
    // The sibling tab hears of it as of a local merge.
    flow.say_to(&sibling, "Hello.").await;
    flow.settled_in(&sibling).await;
    let told = format!("merged into `main` at {}", &squash[..12]);
    assert!(
        log.lock()
            .unwrap()
            .iter()
            .any(|line| line.starts_with("other: ") && line.contains(&told)),
        "{told}"
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
    // Each ship writes new notes, so a second session has something to merge too.
    let shipped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let flow = Flow::start(
        name,
        Options {
            auto_merge,
            ..Options::default()
        },
        script(move |turn| {
            let (said, shipped) = (said.clone(), shipped.clone());
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("Done.");
                }
                if turn.input.contains("Ship it.") {
                    said.lock()
                        .unwrap()
                        .push(format!("bar: {}", turn.prompt.contains(AUTO_MERGE)));
                    let workspace = &turn.add_dirs[0];
                    let n = shipped.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    std::fs::write(workspace.join("NOTES.md"), format!("notes {n}\n")).unwrap();
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

/// A merge carries its record: the commit it merged, and what ran on it. The first merge on its
/// own in a project says so; the next one, from another session of the project, stays quiet.
#[tokio::test]
async fn the_first_merge_on_its_own_in_a_project_is_marked_once() {
    let (flow, _) = shipping("auto-merge-first", true).await;
    let record = |board: &crate::board::Board| {
        board
            .orchestrator_steps
            .iter()
            .find_map(|step| match &step.kind {
                OrchestratorStepKind::Merged { record, head, .. } => {
                    Some((record.clone().expect("a merge record"), head.clone()))
                }
                _ => None,
            })
            .expect("merged")
    };
    flow.say("Ship it.").await;
    let (first, head) = record(&flow.settled().await);
    assert!(first.first_auto);
    assert_eq!(Some(first.head.clone()), head);
    // Nothing ran on it: the record shows each part missing.
    assert!(first.checks.is_empty() && first.ci.is_none() && first.verifier.is_none());
    let sibling = flow
        .manager
        .create_session_from(&flow.conversation, None)
        .await
        .unwrap()
        .id;
    flow.say_to(&sibling, "Ship it.").await;
    let (second, _) = record(&flow.settled_in(&sibling).await);
    assert!(!second.first_auto, "quiet after the first");
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

/// The thread attaches a screenshot it made of the change: Brigadier keeps a copy, records it
/// for the merge message, and serves it by its id. Not an image or recording, or over 11 MB:
/// the call says so and nothing is attached; nor is anything from Brigadier's private folder.
#[cfg(unix)]
#[tokio::test]
async fn a_screenshot_the_thread_attaches_is_kept_for_the_merge_message() {
    let log: Log = Arc::default();
    let said = log.clone();
    let private: Arc<OnceLock<PathBuf>> = Arc::default();
    let run_dir = private.clone();
    let flow = Flow::start(
        "auto-merge-proof",
        Options::default(),
        script(move |turn| {
            let said = said.clone();
            let run_dir = run_dir.get().cloned();
            async move {
                if !turn.is_orchestrator() || !turn.input.contains("Show it.") {
                    return Reply::text("Done.");
                }
                let workspace = &turn.add_dirs[0];
                std::fs::write(workspace.join("shot.png"), b"\x89PNG proof").unwrap();
                std::fs::write(workspace.join("notes.txt"), "notes").unwrap();
                let big = std::fs::File::create(workspace.join("big.png")).unwrap();
                big.set_len(12 * 1024 * 1024).unwrap();
                // A link into Brigadier's private folder, named like a screenshot.
                std::os::unix::fs::symlink(run_dir.unwrap(), workspace.join("token.png")).unwrap();
                for (path, caption) in [
                    ("shot.png", Some("The new switch")),
                    ("notes.txt", None),
                    ("big.png", None),
                    ("token.png", None),
                ] {
                    let reply = turn
                        .call("attach_proof", json!({ "path": path, "caption": caption }))
                        .await;
                    said.lock()
                        .unwrap()
                        .push(format!("{path}: {} {}", reply.is_error, reply.text));
                }
                Reply::text("Merged.")
            }
        }),
    )
    .await;
    let run_dir = flow.manager.runtime.platform().paths().run_dir.clone();
    std::fs::create_dir_all(&run_dir).unwrap();
    private.set(run_dir).unwrap();
    flow.say("Show it.").await;
    let board = flow.settled().await;
    let log = log.lock().unwrap().clone();
    assert!(
        log[0].starts_with("shot.png: false Attached shot.png"),
        "{log:?}"
    );
    assert!(log[1].starts_with("notes.txt: true "), "{log:?}");
    assert!(log[2].contains("over 11 MB"), "{log:?}");
    assert!(log[3].contains("private folder"), "{log:?}");
    let proofs: Vec<_> = board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            OrchestratorStepKind::Proof {
                id,
                name,
                mime,
                caption,
                ..
            } => Some((id.clone(), name.clone(), mime.clone(), caption.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(proofs.len(), 1, "{proofs:?}");
    let (id, name, mime, caption) = proofs[0].clone();
    assert_eq!(
        (name.as_str(), mime.as_str(), caption.as_deref()),
        ("shot.png", "image/png", Some("The new switch"))
    );
    // Served from Brigadier's copy, even once the thread's own file is gone.
    let checkout = PathBuf::from(flow.manager.checkout_dir(&flow.conversation).unwrap());
    std::fs::remove_file(checkout.join("shot.png")).ok();
    let (data, served) = flow
        .manager
        .read_thread_image(&flow.conversation, &id)
        .await
        .unwrap();
    assert_eq!(served, "image/png");
    assert_eq!(data, brigadier_providers::image_base64(b"\x89PNG proof"));
    flow.stop().await;
}

/// Revert on the merge message sends the thread "Revert merge ‹sha› on ‹base› the way it went
/// in" as the user's message: one message, and the thread's revert lands on the base the way
/// the merge went in (here, merged on this Mac on its own).
#[tokio::test]
async fn a_revert_from_the_merge_message_is_one_request_and_the_threads_revert_lands() {
    let flow = Flow::start(
        "auto-merge-revert",
        Options {
            auto_merge: true,
            ..Options::default()
        },
        script(|turn| async move {
            if !turn.is_orchestrator() {
                return Reply::text("Done.");
            }
            let workspace = &turn.add_dirs[0];
            if turn.input.contains("Ship it.") {
                std::fs::write(workspace.join("NOTES.md"), "notes\n").unwrap();
                git(workspace, &["add", "NOTES.md"]);
                git(workspace, &["commit", "-q", "-m", "Add notes"]);
            } else if let Some(rest) = turn.input.split("Revert merge ").nth(1) {
                let sha = rest.split_whitespace().next().unwrap();
                git(workspace, &["revert", "--no-edit", sha]);
            } else {
                return Reply::text("Noted.");
            }
            let reply = turn.call("finish_session", json!({})).await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Merged.")
        }),
    )
    .await;
    flow.say("Ship it.").await;
    let board = flow.settled().await;
    assert!(super::thread_tests::on_main(&flow, "NOTES.md"));
    let merged = merges(&board)[0].1.clone().unwrap();
    // The request the merge message's Revert sends.
    let revert = format!("Revert merge {} on main the way it went in", &merged[..7]);
    flow.say(&revert).await;
    let board = flow.settled().await;
    assert!(!super::thread_tests::on_main(&flow, "NOTES.md"));
    assert_eq!(merges(&board).len(), 2, "{:?}", merges(&board));
    let messages = flow
        .core
        .list_messages(flow.conversation.clone(), None, 50)
        .await
        .unwrap()
        .messages;
    let asked = messages
        .iter()
        .filter(|message| message.role == crate::model::MessageRole::User && message.text == revert)
        .count();
    assert_eq!(asked, 1);
    flow.stop().await;
}
