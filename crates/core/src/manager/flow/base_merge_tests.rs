//! Merge tasks (SESSION-FIXES-PLAN.md Q2): the worker merges with real git and commits; the
//! engine checks the merge (both sides merged, no marker left, nothing missing unless listed
//! under `deleted`) and fast-forwards the target, and sends the same worker back when the check
//! fails or the target moved. Session/base conflicts keep the user's yes across the merge.

use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};

use super::thread_tests::commit_in_workspace;
use super::{Flow, Options, Reply, Turn, git};
use crate::model::{Environment, Setup};
use crate::work::{OrchestratorStepKind, QuestionKind, Task, TaskState};

/// The files `main` adds while the session works.
const BASE_FILES: usize = 18;

/// What the scripted merge worker does with its merge.
#[derive(Clone, Copy, PartialEq)]
enum Worker {
    /// Resolves README.md to both sides.
    Resolve,
    /// Keeps the session's README.md as it is.
    KeepSession,
    /// Commits README.md with its markers, then fixes it in a commit on top when told.
    Markers,
    /// Removes one of `main`'s new files, then lists it under `deleted` when told.
    DropsBaseFile,
    /// Removes `.gitignore` (the session changed it, `main` deleted it), then lists it.
    DropsContested,
    /// Keeps the session's `.gitignore` that `main` deleted.
    KeepsContested,
}

/// How the session and `main` part ways before the merge.
#[derive(Clone, Copy, Default)]
struct Sides {
    /// The session changes `.gitignore`.
    session_ignores: bool,
    /// `main` deletes `.gitignore`.
    main_deletes_ignore: bool,
}

/// Runs git in `dir`, which may fail (a merge that stops at conflicts).
fn try_git(dir: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Flow Test")
        .env("GIT_AUTHOR_EMAIL", "flow@example.com")
        .env("GIT_COMMITTER_NAME", "Flow Test")
        .env("GIT_COMMITTER_EMAIL", "flow@example.com")
        .output()
        .expect("git runs")
        .status
        .success()
}

async fn report(turn: &Turn, extra: Value) -> Reply {
    let mut args = json!({
        "summary": "Merged and resolved the conflicts.", "changes": ["README.md"]
    });
    args.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let reply = turn.call("submit_report", args).await;
    assert!(!reply.is_error, "{}", reply.text);
    Reply::text("Reported.")
}

/// The merge worker's first turn: it merges `main` itself, with git.
async fn merge_base(turn: &Turn, worker: Worker) -> Reply {
    let brief = format!("{}\n{}", turn.prompt, turn.input);
    assert!(brief.contains("Run `git merge main` yourself"), "{brief}");
    assert!(brief.contains("keeping both sides' intent"), "{brief}");
    assert!(!try_git(&turn.cwd, &["merge", "-q", "--no-edit", "main"]));
    let marked = std::fs::read_to_string(turn.cwd.join("README.md")).unwrap();
    assert!(marked.contains("<<<<<<<"), "{marked}");
    turn.write(
        "README.md",
        match worker {
            Worker::KeepSession => "# Session\n",
            Worker::Markers => &marked,
            _ => "# Session and Base\n",
        },
    );
    match worker {
        Worker::DropsBaseFile => {
            turn.git(&["rm", "-q", "-f", "base-1.txt"]);
        }
        Worker::DropsContested => {
            turn.git(&["rm", "-q", "-f", ".gitignore"]);
        }
        _ => {}
    }
    turn.git(&["add", "-A"]);
    turn.git(&["commit", "-q", "--no-edit"]);
    report(turn, json!({})).await
}

/// The same worker, sent back by the landing's check.
async fn merge_again(turn: &Turn) -> Reply {
    let told = &turn.input;
    if told.contains("Conflict markers are left in: README.md") {
        turn.write("README.md", "# Session and Base\n");
        turn.git(&["commit", "-q", "-am", "Resolve README.md"]);
        return report(turn, json!({})).await;
    }
    if told.contains("Missing from the merge: base-1.txt (from `main`)") {
        return report(
            turn,
            json!({"deleted": "base-1.txt: the session replaced it"}),
        )
        .await;
    }
    if told.contains("changed it, `main` deleted it. Keep a version, or list it under deleted") {
        return report(turn, json!({"deleted": ".gitignore: main dropped it"})).await;
    }
    // The target moved: merge it in too.
    let target = told.split('`').nth(1).unwrap();
    assert!(told.contains("moved to"), "{told}");
    assert!(!try_git(&turn.cwd, &["merge", "-q", "--no-edit", target]));
    turn.write("README.md", "# Session and Base, again\n");
    turn.git(&["add", "-A"]);
    turn.git(&["commit", "-q", "--no-edit"]);
    report(turn, json!({})).await
}

async fn conflicting_session(worker: Worker, sides: Sides) -> Flow {
    // The scripted thread delegates the merge once, whatever its later turns hold.
    let delegated = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flow = Flow::start(
        "base-merge",
        Options::default(),
        Arc::new(move |turn| {
            let delegated = delegated.clone();
            Box::pin(async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("Add notes") {
                        commit_in_workspace(&turn, "README.md", "# Session\n", "Add notes");
                        commit_in_workspace(&turn, "notes-2.txt", "two\n", "Add notes two");
                        commit_in_workspace(&turn, "notes-3.txt", "three\n", "Add notes three");
                        if sides.session_ignores {
                            commit_in_workspace(&turn, ".gitignore", "*.log\n*.tmp\n", "Ignore");
                        }
                    } else if turn.input.contains("Resolve the conflict")
                        && !delegated.swap(true, std::sync::atomic::Ordering::SeqCst)
                    {
                        let reply = turn.call("delegate_task", json!({
                            "kind": "merge", "title": "Resolve the base conflict",
                            "effort": "high", "spec": "Keep both sides' intent and check the result."
                        })).await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("[quiet]");
                    } else if turn.input.contains("Merge task-1 again") {
                        // A merge task is finished by its own worker.
                        let reply = turn.call("delegate_task", json!({
                            "kind": "merge", "title": "Resolve task-1's conflict",
                            "effort": "high", "spec": "Resolve it.", "subject": "task-1"
                        })).await;
                        assert!(reply.is_error, "{}", reply.text);
                        assert!(
                            reply.text.contains("task-1 is a merge task: its own worker merges with git"),
                            "{}",
                            reply.text
                        );
                    }
                    return Reply::text("Ready.");
                }
                // Sent back by the landing's check (in the same session, or a new one).
                if turn.input.to_lowercase().contains("nothing landed") {
                    merge_again(&turn).await
                } else {
                    merge_base(&turn, worker).await
                }
            })
        }),
    )
    .await;
    flow.say("Add notes and merge it.").await;
    flow.settled().await;
    std::fs::write(flow.repo.join("README.md"), "# Base\n").unwrap();
    for n in 1..=BASE_FILES {
        std::fs::write(flow.repo.join(format!("base-{n}.txt")), format!("{n}\n")).unwrap();
    }
    if sides.main_deletes_ignore {
        git(&flow.repo, &["rm", "-q", ".gitignore"]);
    }
    git(&flow.repo, &["add", "-A"]);
    git(&flow.repo, &["commit", "-qm", "Base changes"]);
    flow
}

fn branch(flow: &Flow) -> String {
    session(flow).0
}

/// The session branch and its worktree.
fn session(flow: &Flow) -> (String, std::path::PathBuf) {
    match flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .setup
        .unwrap()
    {
        Setup::Session {
            environment:
                Environment::NewWorktree {
                    branch,
                    path: Some(path),
                    ..
                },
            ..
        } => (branch, path.into()),
        other => panic!("{other:?}"),
    }
}

async fn conflict(flow: &Flow, words: &str) {
    let before = git(&flow.repo, &["rev-parse", "main"]);
    let error = flow
        .manager
        .finish_session(&flow.conversation, words, None)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("delegate a merge task without `subject` now"),
        "{error}"
    );
    assert!(
        error.contains("consent to this merge still holds"),
        "{error}"
    );
    assert_eq!(git(&flow.repo, &["rev-parse", "main"]), before);
}

async fn reported_merge(flow: &Flow) -> Task {
    // A neutral follow-up must not consume the original consent, even with a "when" in it.
    flow.say("Resolve the conflict and tell me when it's done.")
        .await;
    let board = flow
        .until("the merge worker's report", |board| {
            board
                .tasks
                .values()
                .any(|task| task.state == TaskState::Reported)
        })
        .await;
    Flow::task(&board, 1).clone()
}

/// The merge task's landing, called by the thread: what it said.
async fn land(flow: &Flow) -> String {
    let task = Flow::task(&flow.board().await, 1).clone();
    flow.manager
        .land_phase(&flow.conversation, task)
        .await
        .unwrap()
}

/// Waits for task-1 to land on its own (after the worker was sent back): its landed tip.
async fn landed_on_its_own(flow: &Flow) -> String {
    let board = flow
        .until("the merge to land on its own", |board| {
            Flow::task(board, 1).state == TaskState::Landed
        })
        .await;
    Flow::task(&board, 1).landed.clone().unwrap()
}

/// The merge lands as the worker's own two-parent commit, with every file of both sides.
fn assert_merged(flow: &Flow, landed: &str, session_tip: &str, base_tip: &str) {
    let parents = git(&flow.repo, &["rev-list", "--parents", "-n", "1", landed]);
    assert_eq!(
        parents.split_whitespace().collect::<Vec<_>>(),
        [landed, session_tip, base_tip]
    );
    let files = git(&flow.repo, &["ls-tree", "-r", "--name-only", landed]);
    for n in 1..=BASE_FILES {
        assert!(files.contains(&format!("base-{n}.txt")), "{files}");
    }
    for file in ["README.md", "notes-2.txt", "notes-3.txt"] {
        assert!(files.lines().any(|line| line == file), "{files}");
    }
}

async fn resolve(flow: &Flow) -> String {
    let target = branch(flow);
    let session_tip = git(&flow.repo, &["rev-parse", &target]);
    let base_tip = git(&flow.repo, &["rev-parse", "main"]);
    let task = reported_merge(flow).await;
    assert!(task.subject.is_none());
    let merge = task.workspace.as_ref().unwrap().merge.clone().unwrap();
    assert_eq!(merge.incoming, base_tip);
    let reply = land(flow).await;
    assert!(reply.contains("Landed"), "{reply}");
    let board = flow.board().await;
    let task = Flow::task(&board, 1);
    assert_eq!(task.state, TaskState::Landed);
    let landed = task.landed.clone().unwrap();
    assert_merged(flow, &landed, &session_tip, &base_tip);
    assert_eq!(git(&flow.repo, &["rev-parse", &branch(flow)]), landed);
    flow.until("the merge's review", |board| {
        board
            .reviews
            .values()
            .any(|review| review.task_id.as_ref() == Some(&task.id))
    })
    .await;
    flow.settled().await;
    landed
}

async fn finish(flow: &Flow, words: &str) {
    let reply = flow
        .manager
        .finish_session(&flow.conversation, words, None)
        .await
        .unwrap();
    assert!(reply.contains("[finished]"), "{reply}");
    let board = flow.board().await;
    assert_eq!(
        board
            .orchestrator_steps
            .iter()
            .filter(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
            .count(),
        1
    );
    assert!(
        flow.manager
            .finish_session(&flow.conversation, words, None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn base_conflicts_resolve_and_finish_on_the_original_consent() {
    let flow = conflicting_session(Worker::Resolve, Sides::default()).await;
    conflict(&flow, "merge it").await;
    resolve(&flow).await;
    // The merge task that landed takes no merge task of its own.
    flow.say("Merge task-1 again.").await;
    flow.settled().await;
    finish(&flow, "merge it").await;
    assert_eq!(
        git(&flow.repo, &["show", "main:README.md"]),
        "# Session and Base"
    );
    let missing = git(
        &flow.repo,
        &["diff", "--stat", "--diff-filter=D", "main^1", "main"],
    );
    assert_eq!(missing, "", "nothing of main is missing");
    flow.stop().await;
}

#[tokio::test]
async fn a_wait_between_conflict_and_retry_revokes_consent() {
    let flow = conflicting_session(Worker::Resolve, Sides::default()).await;
    conflict(&flow, "merge it").await;
    flow.say("wait!").await;
    flow.settled().await;
    resolve(&flow).await;
    let error = flow
        .manager
        .finish_session(&flow.conversation, "merge it", None)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("wait"), "{error}");
    assert_eq!(git(&flow.repo, &["show", "main:README.md"]), "# Base");
    flow.stop().await;
}

#[tokio::test]
async fn a_wait_during_conflict_preparation_does_not_save_consent() {
    let flow = conflicting_session(Worker::Resolve, Sides::default()).await;
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    let reached = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    *conv.merge_pause.lock().unwrap() = Some((reached.clone(), release.clone()));
    let manager = flow.manager.clone();
    let id = flow.conversation.clone();
    let pending = tokio::spawn(async move { manager.finish_session(&id, "merge it", None).await });
    reached.notified().await;
    flow.say("wait!").await;
    release.notify_one();
    let error = pending.await.unwrap().unwrap_err().to_string();
    assert!(error.contains("wrote again"), "{error}");
    assert!(crate::manager::landing::pending_merge(&flow.board().await).is_none());
    *conv.merge_pause.lock().unwrap() = None;
    flow.stop().await;
}

#[tokio::test]
async fn an_unchanged_resolution_still_lands_two_parents() {
    let flow = conflicting_session(Worker::KeepSession, Sides::default()).await;
    conflict(&flow, "merge it").await;
    resolve(&flow).await;
    finish(&flow, "merge it").await;
    assert_eq!(git(&flow.repo, &["show", "main:README.md"]), "# Session");
    flow.stop().await;
}

#[tokio::test]
async fn the_conflicted_card_offers_resolution_and_its_answer_carries_over() {
    let flow = conflicting_session(Worker::Resolve, Sides::default()).await;
    flow.manager
        .propose_merge(&flow.conversation, None)
        .await
        .unwrap();
    let board = flow.board().await;
    let card = board.questions.values().find(|q| q.is_open()).unwrap();
    assert!(
        matches!(&card.kind, QuestionKind::Merge { conflicted: true, conflicts, .. } if conflicts == &["README.md"])
    );
    assert!(
        card.text
            .contains("1 file conflicts with `main`: `README.md`"),
        "{}",
        card.text
    );
    assert_eq!(
        card.round()[0]
            .options
            .iter()
            .map(|o| o.label.as_str())
            .collect::<Vec<_>>(),
        ["Merge & resolve conflicts", "Not yet"]
    );
    flow.manager
        .answer_question(
            flow.conversation.clone(),
            card.id.clone(),
            vec!["Merge & resolve conflicts".into()],
        )
        .await
        .unwrap();
    flow.settled().await;
    conflict(&flow, "").await;
    resolve(&flow).await;
    finish(&flow, "").await;
    let board = flow.board().await;
    assert!(board.orchestrator_steps.iter().any(|step| matches!(&step.kind, OrchestratorStepKind::Merged { asked_in: Some(id), .. } if *id == card.id.to_string())));
    flow.stop().await;
}

#[tokio::test]
async fn a_merge_with_markers_is_refused_and_lands_after_a_fix_on_top() {
    let flow = conflicting_session(Worker::Markers, Sides::default()).await;
    conflict(&flow, "merge it").await;
    let session_tip = git(&flow.repo, &["rev-parse", &branch(&flow)]);
    let base_tip = git(&flow.repo, &["rev-parse", "main"]);
    reported_merge(&flow).await;
    let reply = land(&flow).await;
    assert!(
        reply.contains("Conflict markers are left in: README.md"),
        "{reply}"
    );
    assert!(reply.contains("Its worker was sent back"), "{reply}");
    assert_eq!(git(&flow.repo, &["rev-parse", &branch(&flow)]), session_tip);
    // The worker fixed it in a commit on top of its merge: the merge is in HEAD's history.
    let landed = landed_on_its_own(&flow).await;
    assert_eq!(
        git(&flow.repo, &["log", "-1", "--format=%s", &landed]),
        "Resolve README.md"
    );
    let merge = git(&flow.repo, &["rev-parse", &format!("{landed}^")]);
    assert_merged(&flow, &merge, &session_tip, &base_tip);
    assert_eq!(git(&flow.repo, &["rev-parse", &branch(&flow)]), landed);
    flow.settled().await;
    // The fix on top is still the merge the held consent covers.
    finish(&flow, "merge it").await;
    assert_eq!(
        git(&flow.repo, &["show", "main:README.md"]),
        "# Session and Base"
    );
    flow.stop().await;
}

#[tokio::test]
async fn a_base_file_dropped_without_a_reason_is_refused_and_lands_once_listed() {
    let flow = conflicting_session(Worker::DropsBaseFile, Sides::default()).await;
    conflict(&flow, "merge it").await;
    let session_tip = git(&flow.repo, &["rev-parse", &branch(&flow)]);
    reported_merge(&flow).await;
    let reply = land(&flow).await;
    assert!(
        reply.contains("Missing from the merge: base-1.txt (from `main`)"),
        "{reply}"
    );
    assert_eq!(git(&flow.repo, &["rev-parse", &branch(&flow)]), session_tip);
    let landed = landed_on_its_own(&flow).await;
    let files = git(&flow.repo, &["ls-tree", "-r", "--name-only", &landed]);
    assert!(!files.contains("base-1.txt") && files.contains("base-2.txt"));
    let report = Flow::task(&flow.board().await, 1).report.clone().unwrap();
    assert_eq!(report.deleted, ["base-1.txt: the session replaced it"]);
    flow.stop().await;
}

#[tokio::test]
async fn a_file_one_side_changed_and_the_other_deleted_needs_a_version_or_a_reason() {
    // Dropped with no reason: refused as contested, then listed and landed.
    let sides = Sides {
        session_ignores: true,
        main_deletes_ignore: true,
    };
    let flow = conflicting_session(Worker::DropsContested, sides).await;
    conflict(&flow, "merge it").await;
    reported_merge(&flow).await;
    let reply = land(&flow).await;
    let target = branch(&flow);
    assert!(
        reply.contains(&format!(
            ".gitignore: `{target}` changed it, `main` deleted it. Keep a version"
        )),
        "{reply}"
    );
    let landed = landed_on_its_own(&flow).await;
    let files = git(&flow.repo, &["ls-tree", "-r", "--name-only", &landed]);
    assert!(!files.lines().any(|line| line == ".gitignore"), "{files}");
    flow.stop().await;
    // Kept in the session's version: it lands as it is.
    let flow = conflicting_session(Worker::KeepsContested, sides).await;
    conflict(&flow, "merge it").await;
    resolve(&flow).await;
    assert_eq!(
        git(
            &flow.repo,
            &["show", &format!("{}:.gitignore", branch(&flow))]
        ),
        "*.log\n*.tmp"
    );
    flow.stop().await;
}

#[tokio::test]
async fn a_file_main_deleted_that_the_session_never_touched_goes_with_no_reason() {
    let sides = Sides {
        session_ignores: false,
        main_deletes_ignore: true,
    };
    let flow = conflicting_session(Worker::Resolve, sides).await;
    conflict(&flow, "merge it").await;
    let landed = resolve(&flow).await;
    let files = git(&flow.repo, &["ls-tree", "-r", "--name-only", &landed]);
    assert!(!files.lines().any(|line| line == ".gitignore"), "{files}");
    flow.stop().await;
}

#[tokio::test]
async fn held_consent_does_not_cover_an_unrelated_merge_parent() {
    let flow = conflicting_session(Worker::Resolve, Sides::default()).await;
    conflict(&flow, "merge it").await;
    let branch = branch(&flow);
    let session = git(&flow.repo, &["rev-parse", &branch]);
    let common = git(&flow.repo, &["merge-base", &branch, "main"]);
    let tree = git(&flow.repo, &["rev-parse", &format!("{session}^{{tree}}")]);
    let unrelated = git(
        &flow.repo,
        &["commit-tree", &tree, "-p", &common, "-m", "Unrelated work"],
    );
    let merge = git(
        &flow.repo,
        &[
            "commit-tree",
            &tree,
            "-p",
            &session,
            "-p",
            &unrelated,
            "-m",
            "Unrelated merge",
        ],
    );
    git(
        &flow.repo,
        &[
            "update-ref",
            &format!("refs/heads/{branch}"),
            &merge,
            &session,
        ],
    );
    let error = flow
        .manager
        .finish_session(&flow.conversation, "merge it", None)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("consent doesn't cover it"), "{error}");
    assert_eq!(git(&flow.repo, &["show", "main:README.md"]), "# Base");
    flow.stop().await;
}

#[tokio::test]
async fn a_merge_whose_target_moved_goes_back_to_the_same_worker_and_lands_both() {
    let flow = conflicting_session(Worker::Resolve, Sides::default()).await;
    conflict(&flow, "merge it").await;
    let task = reported_merge(&flow).await;
    let native = task.native_session.clone();
    // Other work lands on the session branch meanwhile, over the same lines.
    let (target, worktree) = session(&flow);
    std::fs::write(worktree.join("README.md"), "# Session, again\n").unwrap();
    git(&worktree, &["commit", "-qam", "Other work"]);
    let other = git(&flow.repo, &["rev-parse", &target]);
    let base_tip = git(&flow.repo, &["rev-parse", "main"]);
    let reply = land(&flow).await;
    assert!(
        reply.contains("while task-1 merged, so its worker was sent back to merge it in too"),
        "{reply}"
    );
    // The expected-tip check refused the fast-forward: the other work stays.
    assert_eq!(git(&flow.repo, &["rev-parse", &target]), other);
    let landed = landed_on_its_own(&flow).await;
    let board = flow.board().await;
    let task = Flow::task(&board, 1);
    assert_eq!(task.native_session, native, "the same worker merged again");
    assert_eq!(board.tasks.len(), 1, "no second merge task");
    for side in [&other, &base_tip] {
        assert!(try_git(
            &flow.repo,
            &["merge-base", "--is-ancestor", side, &landed]
        ));
    }
    assert_eq!(
        git(&flow.repo, &["show", &format!("{landed}:README.md")]),
        "# Session and Base, again"
    );
    flow.settled().await;
    // The consent held over didn't cover the other work: the user's fresh yes does.
    let error = flow
        .manager
        .finish_session(&flow.conversation, "merge it", None)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("consent doesn't cover it"), "{error}");
    flow.say("merge it").await;
    flow.settled().await;
    finish(&flow, "merge it").await;
    assert_eq!(
        git(&flow.repo, &["show", "main:README.md"]),
        "# Session and Base, again"
    );
    flow.stop().await;
}

/// A task whose work conflicts with the session branch: the merge task with it as subject
/// starts from that work, its worker merges the branch in with git, and the merge lands with
/// both as parents, the conflicting task with it.
#[tokio::test]
async fn a_conflicting_task_is_merged_by_a_merge_task_on_its_work() {
    use std::sync::atomic::{AtomicU32, Ordering};
    // The scripted thread delegates each task once, whatever its later turns hold.
    let delegated = Arc::new(AtomicU32::new(0));
    let flow = Flow::start(
        "subject-merge",
        Options::default(),
        Arc::new(move |turn| {
            let delegated = delegated.clone();
            Box::pin(async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("Edit the readme.")
                        && delegated
                            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                            .is_ok()
                    {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({
                                    "kind": "implement", "title": "Edit the readme",
                                    "effort": "low", "spec": "Say Worker in README.md."
                                }),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("[quiet]");
                    }
                    if turn.input.contains("Merge task-1.")
                        && delegated
                            .compare_exchange(1, 2, Ordering::SeqCst, Ordering::SeqCst)
                            .is_ok()
                    {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({
                                    "kind": "merge", "title": "Merge the readme",
                                    "effort": "high", "spec": "Keep both.", "subject": "task-1"
                                }),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("[quiet]");
                    }
                    return Reply::text("Ready.");
                }
                let brief = format!("{}\n{}", turn.prompt, turn.input);
                if brief.contains("merge: merge the branch named below") {
                    let target = brief
                        .split("Run `git merge ")
                        .nth(1)
                        .and_then(|rest| rest.split('`').next())
                        .unwrap()
                        .to_owned();
                    assert!(brief.contains("holds task-1's work"), "{brief}");
                    assert!(!try_git(&turn.cwd, &["merge", "-q", "--no-edit", &target]));
                    turn.write("README.md", "# Worker and Thread\n");
                    turn.git(&["add", "-A"]);
                    turn.git(&["commit", "-q", "--no-edit"]);
                } else {
                    // A stray log committed with the work: the litter guard leaves it out.
                    turn.write("README.md", "# Worker\n");
                    turn.write("debug.log", "scratch\n");
                    turn.git(&["add", "-f", "debug.log"]);
                    turn.git(&["commit", "-q", "-am", "Say Worker"]);
                }
                report(&turn, json!({})).await
            })
        }),
    )
    .await;
    flow.say("Edit the readme.").await;
    let board = flow
        .until("the worker's report", |board| {
            board
                .tasks
                .values()
                .any(|task| task.state == TaskState::Reported)
        })
        .await;
    let work = Flow::task(&board, 1).clone();
    let work_branch = work.workspace.as_ref().unwrap().branch.clone().unwrap();
    // The thread changed the same line on the session branch meanwhile.
    let (target, worktree) = session(&flow);
    std::fs::write(worktree.join("README.md"), "# Thread\n").unwrap();
    git(&worktree, &["commit", "-qam", "Say Thread"]);
    let thread_tip = git(&flow.repo, &["rev-parse", &target]);
    let reply = flow
        .manager
        .land_phase(&flow.conversation, work)
        .await
        .unwrap();
    assert!(
        reply.contains(&format!(
            "Delegate a merge task with subject task-1: its worker merges `{target}` into task-1's work with git"
        )),
        "{reply}"
    );
    flow.settled().await;
    flow.say("Merge task-1.").await;
    let board = flow
        .until("the merge worker's report", |board| {
            board
                .tasks
                .values()
                .any(|task| task.number == 2 && task.state == TaskState::Reported)
        })
        .await;
    let merge = Flow::task(&board, 2).clone();
    let incoming = merge.workspace.as_ref().unwrap().merge.clone().unwrap();
    assert_eq!(incoming.label, "task-1's work");
    // Rebuilt without the log, in place of task-1's branch tip.
    let work_tip = git(&flow.repo, &["rev-parse", &work_branch]);
    assert!(
        incoming
            .rebuilt
            .iter()
            .any(|r| r.from == work_tip && r.to == incoming.incoming),
        "{incoming:?}"
    );
    let reply = flow
        .manager
        .land_phase(&flow.conversation, merge)
        .await
        .unwrap();
    assert!(reply.contains("Landed"), "{reply}");
    let board = flow.board().await;
    assert_eq!(Flow::task(&board, 1).state, TaskState::Landed);
    let landed = Flow::task(&board, 2).landed.clone().unwrap();
    let parents = git(&flow.repo, &["rev-list", "--parents", "-n", "1", &landed]);
    assert_eq!(
        parents.split_whitespace().collect::<Vec<_>>(),
        [landed.as_str(), &incoming.incoming, &thread_tip]
    );
    assert_eq!(
        git(&flow.repo, &["show", &format!("{target}:README.md")]),
        "# Worker and Thread"
    );
    // No landed commit holds the log, and task-1's branch went as landed.
    assert_eq!(
        git(
            &flow.repo,
            &["log", "--format=%H", &target, "--", "debug.log"]
        ),
        ""
    );
    assert!(!try_git(
        &flow.repo,
        &[
            "rev-parse",
            "--verify",
            "-q",
            &format!("refs/heads/{work_branch}")
        ]
    ));
    flow.stop().await;
}
