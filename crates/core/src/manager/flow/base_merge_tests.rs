//! Session/base conflicts use the normal merge worker and landing, retaining the user's yes.

use std::sync::Arc;

use serde_json::json;

use super::thread_tests::commit_in_workspace;
use super::{Flow, Options, Reply, git};
use crate::model::{Environment, Setup};
use crate::work::{OrchestratorStepKind, QuestionKind, Task, TaskState};

async fn conflicting_session(keep_session: bool) -> Flow {
    let flow = Flow::start(
        "base-merge",
        Options::default(),
        Arc::new(move |turn| {
            Box::pin(async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("Add notes") {
                        commit_in_workspace(&turn, "README.md", "# Session\n", "Add notes");
                    } else if turn.input.contains("Resolve the conflict") {
                        let reply = turn.call("delegate_task", json!({
                        "kind": "merge", "title": "Resolve the base conflict",
                        "effort": "high", "spec": "Keep both sides' intent and check the result."
                    })).await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("[quiet]");
                    }
                    return Reply::text("Ready.");
                }
                let brief = format!("{}\n{}", turn.prompt, turn.input);
                assert!(brief.contains("current `main`"), "{brief}");
                assert!(brief.contains("keeping both sides' intent"), "{brief}");
                let marked = std::fs::read_to_string(turn.cwd.join("README.md")).unwrap();
                assert!(marked.contains("<<<<<<<"), "{marked}");
                assert!(
                    marked.contains("# Session") && marked.contains("# Base"),
                    "{marked}"
                );
                assert_eq!(
                    turn.git(&["rev-list", "--parents", "-n", "1", "HEAD"])
                        .split_whitespace()
                        .count(),
                    3
                );
                turn.write(
                    "README.md",
                    if keep_session {
                        "# Session\n"
                    } else {
                        "# Session and Base\n"
                    },
                );
                let reply = turn
                    .call(
                        "submit_report",
                        json!({
                            "summary": "Resolved the base conflict.", "changes": ["README.md"]
                        }),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            })
        }),
    )
    .await;
    flow.say("Add notes and merge it.").await;
    flow.settled().await;
    std::fs::write(flow.repo.join("README.md"), "# Base\n").unwrap();
    git(&flow.repo, &["add", "README.md"]);
    git(&flow.repo, &["commit", "-qm", "Base changes"]);
    flow
}

fn branch(flow: &Flow) -> String {
    match flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .setup
        .unwrap()
    {
        Setup::Session {
            environment: Environment::NewWorktree { branch, .. },
            ..
        } => branch,
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
    // A neutral follow-up must not consume the original consent.
    flow.say("Resolve the conflict.").await;
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

async fn resolve(flow: &Flow, hook_retry: bool) -> String {
    let target = branch(flow);
    let session_tip = git(&flow.repo, &["rev-parse", &target]);
    let base_tip = git(&flow.repo, &["rev-parse", "main"]);
    let task = reported_merge(flow).await;
    assert!(task.subject.is_none());
    let prepared = task
        .workspace
        .as_ref()
        .unwrap()
        .base_merge
        .as_ref()
        .unwrap();
    assert_eq!(prepared.base_tip, base_tip);
    let hook = flow.repo.join(".git/hooks/pre-commit");
    if hook_retry {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(
            &hook,
            "#!/bin/sh\necho 'base merge hook refused' >&2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let reply = flow
            .manager
            .land_phase(&flow.conversation, task.clone())
            .await
            .unwrap();
        assert!(reply.contains("base merge hook refused"), "{reply}");
        assert_eq!(git(&flow.repo, &["rev-parse", &target]), session_tip);
        std::fs::remove_file(&hook).unwrap();
    }
    let task = Flow::task(&flow.board().await, 1).clone();
    let reply = flow
        .manager
        .land_phase(&flow.conversation, task)
        .await
        .unwrap();
    assert!(reply.contains("Landed"), "{reply}");
    let board = flow.board().await;
    let task = Flow::task(&board, 1);
    assert_eq!(task.state, TaskState::Landed);
    let landed = task.landed.as_deref().unwrap();
    let parents = git(&flow.repo, &["rev-list", "--parents", "-n", "1", landed]);
    assert_eq!(
        parents.split_whitespace().collect::<Vec<_>>(),
        [landed, &session_tip, &base_tip]
    );
    flow.until("the resolution review", |board| {
        board.reviews.values().any(|review| {
            review.task_id.as_ref() == Some(&task.id) && review.base == prepared.start
        })
    })
    .await;
    flow.settled().await;
    landed.to_owned()
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
    let flow = conflicting_session(false).await;
    conflict(&flow, "merge it").await;
    resolve(&flow, false).await;
    finish(&flow, "merge it").await;
    assert_eq!(
        git(&flow.repo, &["show", "main:README.md"]),
        "# Session and Base"
    );
    flow.stop().await;
}

#[tokio::test]
async fn a_wait_between_conflict_and_retry_revokes_consent() {
    let flow = conflicting_session(false).await;
    conflict(&flow, "merge it").await;
    flow.say("wait!").await;
    flow.settled().await;
    resolve(&flow, false).await;
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
    let flow = conflicting_session(false).await;
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
    assert!(conv.merge_held.lock().unwrap().is_none());
    *conv.merge_pause.lock().unwrap() = None;
    flow.stop().await;
}

#[tokio::test]
async fn an_unchanged_resolution_still_lands_two_parents() {
    let flow = conflicting_session(true).await;
    conflict(&flow, "merge it").await;
    resolve(&flow, false).await;
    finish(&flow, "merge it").await;
    assert_eq!(git(&flow.repo, &["show", "main:README.md"]), "# Session");
    flow.stop().await;
}

#[tokio::test]
async fn a_hook_failure_keeps_the_base_parent_for_the_landing_retry() {
    let flow = conflicting_session(false).await;
    conflict(&flow, "merge it").await;
    resolve(&flow, true).await;
    finish(&flow, "merge it").await;
    flow.stop().await;
}

#[tokio::test]
async fn the_conflicted_card_offers_resolution_and_its_answer_carries_over() {
    let flow = conflicting_session(false).await;
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
    resolve(&flow, false).await;
    finish(&flow, "").await;
    let board = flow.board().await;
    assert!(board.orchestrator_steps.iter().any(|step| matches!(&step.kind, OrchestratorStepKind::Merged { asked_in: Some(id), .. } if *id == card.id.to_string())));
    flow.stop().await;
}

#[tokio::test]
async fn unresolved_markers_cannot_land_on_the_carried_consent() {
    let flow = conflicting_session(false).await;
    conflict(&flow, "merge it").await;
    let task = reported_merge(&flow).await;
    let workspace = task.workspace.as_ref().unwrap();
    let start = &workspace.base_merge.as_ref().unwrap().start;
    let marked = git(&flow.repo, &["show", &format!("{start}:README.md")]);
    let path = std::path::Path::new(workspace.worktree.as_ref().unwrap()).join("README.md");
    std::fs::write(&path, marked).unwrap();
    let before = git(&flow.repo, &["rev-parse", &branch(&flow)]);
    let error = flow
        .manager
        .land_phase(&flow.conversation, task.clone())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("Unresolved conflict markers remain in: README.md"),
        "{error}"
    );
    assert_eq!(git(&flow.repo, &["rev-parse", &branch(&flow)]), before);
    assert!(
        flow.manager
            .finish_session(&flow.conversation, "merge it", None)
            .await
            .is_err()
    );
    // Retry after removing every hunk. The rejected attempt changed no session/base ref.
    std::fs::write(&path, "# Session and Base\n").unwrap();
    let task = Flow::task(&flow.board().await, 1).clone();
    let reply = flow
        .manager
        .land_phase(&flow.conversation, task)
        .await
        .unwrap();
    assert!(reply.contains("Landed"), "{reply}");
    finish(&flow, "merge it").await;
    flow.stop().await;
}

#[tokio::test]
async fn excluded_edits_cannot_discard_incoming_base_files() {
    let flow = conflicting_session(false).await;
    std::fs::write(flow.repo.join("tracked.log"), "base history\n").unwrap();
    git(&flow.repo, &["add", "-f", "tracked.log"]);
    git(&flow.repo, &["commit", "-qm", "Track base history"]);
    conflict(&flow, "merge it").await;
    let task = reported_merge(&flow).await;
    let path = std::path::Path::new(task.workspace.as_ref().unwrap().worktree.as_ref().unwrap())
        .join("tracked.log");
    std::fs::write(&path, "worker litter\n").unwrap();
    let error = flow
        .manager
        .land_phase(&flow.conversation, task)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("Excluded worker edits overlap incoming base changes in: tracked.log"),
        "{error}"
    );
    std::fs::write(&path, "base history\n").unwrap();
    let worktree = path.parent().unwrap();
    git(worktree, &["mv", "tracked.log", "renamed.log"]);
    git(worktree, &["commit", "-qm", "Rename incoming log"]);
    let task = Flow::task(&flow.board().await, 1).clone();
    let error = flow
        .manager
        .land_phase(&flow.conversation, task)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("Excluded worker edits overlap incoming base changes in: renamed.log"),
        "{error}"
    );
    git(worktree, &["mv", "renamed.log", "tracked.log"]);
    git(worktree, &["commit", "-qm", "Keep incoming log"]);
    let task = Flow::task(&flow.board().await, 1).clone();
    let reply = flow
        .manager
        .land_phase(&flow.conversation, task)
        .await
        .unwrap();
    assert!(reply.contains("Landed"), "{reply}");
    finish(&flow, "merge it").await;
    assert_eq!(
        git(&flow.repo, &["show", "main:tracked.log"]),
        "base history"
    );
    flow.stop().await;
}

#[tokio::test]
async fn held_consent_does_not_cover_an_unrelated_merge_parent() {
    let flow = conflicting_session(false).await;
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
