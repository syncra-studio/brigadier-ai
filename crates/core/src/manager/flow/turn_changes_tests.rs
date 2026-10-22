//! Turn cards through the real CLI pump, git, cleanup ledger and confirmation.

use super::{Flow, Options, Reply, Script, Turn, git};
use crate::work::ReviewScope;
use std::sync::Arc;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

fn git_out(repo: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[tokio::test]
async fn turn_snapshot_revert_restores_the_start_tree_and_refuses_newer_edits() {
    let flow = Flow::start(
        "20261022-0805-take-over-5-sessions-w45-revert",
        Options::default(),
        script(|turn| async move {
            if turn.is_orchestrator() {
                let id = turn.conversation().unwrap();
                let (checkout, _) = turn.host.changes_checkout(&id).unwrap();
                std::fs::write(checkout.join("untracked.txt"), "one\ntwo\n").unwrap();
                std::fs::write(checkout.join("binary"), [0, 1, 2]).unwrap();
                // No edit/command event: only the CLI's turn end can account for these files.
            }
            Reply::text("Done.")
        }),
    )
    .await;
    flow.say("Change files.").await;
    let board = flow.settled().await;
    let id = flow.conversation.clone();
    let request = board.latest_request().unwrap();
    let changes = request.changes.as_ref().expect("turn-end snapshot");
    assert!(changes.settled);
    assert_eq!(changes.stat.files.len(), 2);
    assert_eq!(changes.stat.insertions, 2);
    let review = flow
        .manager
        .review_diff(
            &id,
            ReviewScope::LastTurn {
                request_id: Some(request.id.clone()),
            },
            false,
            false,
        )
        .await
        .unwrap();
    assert_eq!(review.insertions, 2);
    assert!(review.patch.contains("+two"));
    let (checkout, _) = flow.manager.changes_checkout(&id).unwrap();
    std::fs::write(checkout.join("later.txt"), "after the turn\n").unwrap();
    let preview = flow
        .manager
        .prepare_turn_revert(id.clone(), request.id.clone())
        .await
        .unwrap();
    assert_eq!(preview.changed_after, ["later.txt"]);
    std::fs::write(checkout.join("later.txt"), "after confirmation\n").unwrap();
    assert!(
        flow.manager
            .revert_turn(id.clone(), request.id.clone(), preview.expected_tree)
            .await
            .is_err()
    );
    let preview = flow
        .manager
        .prepare_turn_revert(id.clone(), request.id.clone())
        .await
        .unwrap();
    flow.manager
        .revert_turn(id.clone(), request.id.clone(), preview.expected_tree)
        .await
        .unwrap();
    let start = format!("refs/brigadier/s/{id}/turn-1-start");
    assert!(git_out(&checkout, &["diff", &start, "--"]).is_empty());
    assert!(git_out(&checkout, &["ls-files", "--others", "--exclude-standard"]).is_empty());
    flow.stop().await;
}

#[tokio::test]
async fn archive_keeps_snapshot_refs_and_delete_removes_the_whole_namespace() {
    let flow = Flow::start(
        "20261022-0805-take-over-5-sessions-w45-refs",
        Options::default(),
        script(|_| async { Reply::text("Looked.") }),
    )
    .await;
    flow.say("Look around.").await;
    let board = flow.settled().await;
    assert!(
        board
            .latest_request()
            .unwrap()
            .changes
            .as_ref()
            .unwrap()
            .stat
            .files
            .is_empty()
    );
    let id = flow.conversation.clone();
    let namespace = format!("refs/brigadier/s/{id}/");
    let before = git_out(&flow.repo, &["for-each-ref", &namespace]);
    assert!(before.contains("turn-1-start"));
    assert!(before.contains("turn-1-end"));
    flow.manager.archive(id.clone()).await.unwrap();
    flow.manager.cleanup_finished(&id).await;
    assert_eq!(git_out(&flow.repo, &["for-each-ref", &namespace]), before);
    assert!(flow.manager.runtime.ledger().artifacts(&crate::manager::turn_changes::snapshots_owner(&id)).iter().any(|artifact| matches!(artifact, brigadier_providers::Artifact::GitRefs { namespace: found, .. } if found == &namespace)));
    flow.manager.delete(id.clone()).await.unwrap();
    super::eventually("the session to be deleted", || {
        flow.core.conversation(&id).is_err()
    })
    .await;
    flow.manager.cleanup_finished(&id).await;
    assert!(git_out(&flow.repo, &["for-each-ref", &namespace]).is_empty());
    flow.stop().await;
}

#[tokio::test]
async fn a_merge_groups_its_files_under_the_branch_and_keeps_only_parent_edits() {
    let flow = Flow::start(
        "20261022-0805-take-over-5-sessions-w45-merge",
        Options::default(),
        script(|turn| async move {
            if turn.is_orchestrator() {
                let id = turn.conversation().unwrap();
                let (checkout, branch) = turn.host.changes_checkout(&id).unwrap();
                git(&checkout, &["checkout", "-b", "worker-card"]);
                std::fs::write(checkout.join("from-worker"), "worker\n").unwrap();
                git(&checkout, &["add", "from-worker"]);
                git(&checkout, &["commit", "-m", "Worker"]);
                let worker = turn
                    .host
                    .git
                    .open(&checkout)
                    .unwrap()
                    .worker_diff(branch.as_deref().unwrap())
                    .unwrap();
                assert_eq!(worker.insertions, 1);
                git(&checkout, &["checkout", branch.as_deref().unwrap()]);
                git(
                    &checkout,
                    &["merge", "--no-ff", "worker-card", "-m", "Merge worker"],
                );
                std::fs::write(checkout.join("own"), "parent\n").unwrap();
            }
            Reply::text("Done.")
        }),
    )
    .await;
    flow.say("Merge worker and edit own file.").await;
    let board = flow.settled().await;
    let request = board.latest_request().unwrap();
    let changes = request.changes.as_ref().unwrap();
    assert_eq!(
        changes
            .stat
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["own"]
    );
    assert_eq!(changes.merged.len(), 1);
    assert_eq!(changes.merged[0].branch, "worker-card");
    assert_eq!(changes.merged[0].stat.insertions, 1);
    let review = flow
        .manager
        .review_diff(
            &flow.conversation,
            ReviewScope::LastTurn {
                request_id: Some(request.id.clone()),
            },
            false,
            false,
        )
        .await
        .unwrap();
    assert_eq!(review.files.len(), 1);
    assert!(!review.patch.contains("+worker"));
    flow.stop().await;
}

#[tokio::test]
async fn file_review_keeps_and_undoes_hunks_without_changing_the_turn_card() {
    let original = "old-a\n1\n2\n3\n4\n5\n6\nold-b\n";
    let flow = Flow::start(
        "20261022-0805-take-over-5-sessions-w45-hunks",
        Options {
            files: vec![("hunks.txt", original)],
            ..Default::default()
        },
        script(|turn| async move {
            if turn.is_orchestrator() {
                std::fs::write(
                    turn.cwd.join("hunks.txt"),
                    "new-a\n1\n2\n3\n4\n5\n6\nnew-b\n",
                )
                .unwrap();
            }
            Reply::text("Done.")
        }),
    )
    .await;
    flow.say("Edit both ends.").await;
    let board = flow.settled().await;
    let id = flow.conversation.clone();
    let request = board.latest_request().unwrap();
    let before = flow
        .manager
        .review_diff(
            &id,
            ReviewScope::LastTurn {
                request_id: Some(request.id.clone()),
            },
            false,
            false,
        )
        .await
        .unwrap();
    let pending = flow
        .manager
        .pending_changes(id.clone(), Some("hunks.txt".into()))
        .await
        .unwrap();
    assert_eq!(pending.files[0].hunks.len(), 2);
    let first = pending.files[0].hunks[0].id.clone();
    flow.manager
        .keep_changes(
            id.clone(),
            Some("hunks.txt".into()),
            Some(vec![first.clone()]),
        )
        .await
        .unwrap();
    assert!(
        flow.manager
            .keep_changes(id.clone(), Some("hunks.txt".into()), Some(vec![first]))
            .await
            .is_err()
    );
    let pending = flow
        .manager
        .pending_changes(id.clone(), Some("hunks.txt".into()))
        .await
        .unwrap();
    assert_eq!(pending.files[0].hunks.len(), 1);
    let second = pending.files[0].hunks[0].id.clone();
    flow.manager
        .undo_pending_changes(id.clone(), Some("hunks.txt".into()), Some(vec![second]))
        .await
        .unwrap();
    let (checkout, _) = flow.manager.changes_checkout(&id).unwrap();
    assert_eq!(
        std::fs::read_to_string(checkout.join("hunks.txt")).unwrap(),
        "new-a\n1\n2\n3\n4\n5\n6\nold-b\n"
    );
    assert!(
        flow.manager
            .pending_changes(id.clone(), None)
            .await
            .unwrap()
            .files
            .is_empty()
    );
    let after = flow
        .manager
        .review_diff(
            &id,
            ReviewScope::LastTurn {
                request_id: Some(request.id.clone()),
            },
            false,
            false,
        )
        .await
        .unwrap();
    assert_eq!(after.patch, before.patch);
    flow.stop().await;
}

#[tokio::test]
async fn steering_keeps_one_snapshot_pair_for_the_whole_cli_turn() {
    let ready = Arc::new(tokio::sync::Notify::new());
    let entered = ready.clone();
    let flow = Flow::start(
        "20261022-0805-take-over-5-sessions-w45-steer",
        Options::default(),
        script(move |turn| {
            let entered = entered.clone();
            async move {
                if turn.is_orchestrator() {
                    std::fs::write(turn.cwd.join("before-steer"), "before\n").unwrap();
                    entered.notify_one();
                    assert!(turn.steered().await.unwrap().contains("Second instruction"));
                    std::fs::write(turn.cwd.join("after-steer"), "after\n").unwrap();
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.say("First instruction.").await;
    ready.notified().await;
    flow.manager
        .send_message(
            flow.conversation.clone(),
            "Second instruction.".into(),
            Vec::new(),
            Vec::new(),
            true,
            None,
        )
        .await
        .unwrap();
    let board = flow.settled().await;
    let cards: Vec<_> = board
        .requests
        .values()
        .filter_map(|request| request.changes.as_ref())
        .collect();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].stat.files.len(), 2);
    let namespace = format!("refs/brigadier/s/{}/", flow.conversation);
    let refs = git_out(
        &flow.repo,
        &["for-each-ref", "--format=%(refname)", &namespace],
    );
    assert!(refs.contains("turn-1-end"));
    assert!(!refs.contains("turn-2-start"));
    flow.stop().await;
}
