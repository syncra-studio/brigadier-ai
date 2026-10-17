//! Request records through the real event loop, store, git and restart, with both provider shapes.

use std::path::Path;
use std::sync::Arc;

use brigadier_providers::{FileChange, FileChangeKind, ItemStatus, ProviderEvent, ProviderKind};

use super::{Flow, Options, Reply, Script, Turn, git};
use crate::board::Board;
use crate::manager::shell_changes::ShellKey;
use crate::model::Setup;
use crate::work::{OrchestratorStepKind, ReviewScope};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

async fn observed(turn: &Turn, files: usize) -> Board {
    let id = turn.conversation().unwrap();
    tokio::time::timeout(super::PATIENCE, async {
        loop {
            let board = turn.host.core.board(&id).await.unwrap();
            if board
                .latest_request()
                .and_then(|r| r.changes.as_ref())
                .is_some_and(|c| c.stat.files.len() == files)
            {
                return board;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

async fn edit(turn: &Turn, path: &Path, item: &str, before: &str, after: &str) {
    std::fs::write(path, after).unwrap();
    let diff = format!(
        "@@ -1,{} +1,{} @@\n{}{}",
        before.lines().count(),
        after.lines().count(),
        before
            .lines()
            .map(|line| format!("-{line}\n"))
            .collect::<String>(),
        after
            .lines()
            .map(|line| format!("+{line}\n"))
            .collect::<String>()
    );
    let mut change = FileChange::counted(
        path.to_string_lossy().into_owned(),
        FileChangeKind::Update,
        Some(&diff),
        usize::MAX,
    );
    if turn.provider == ProviderKind::Claude {
        change.original = Some(before.into());
    }
    turn.emit(ProviderEvent::FileChanges {
        item_id: item.into(),
        changes: vec![change],
        status: ItemStatus::Completed,
    })
    .await;
}

async fn local(flow: &mut Flow) {
    let conversation = flow.core.conversation(&flow.conversation).unwrap();
    let Setup::Session {
        repo,
        permission,
        orchestrator,
        plan_mode,
        ..
    } = conversation.setup.unwrap()
    else {
        unreachable!()
    };
    flow.conversation = flow
        .manager
        .create_conversation(
            crate::model::ConversationKind::Session,
            conversation.project_id,
            Some("Local changes".into()),
            Some(crate::model::SetupRequest::Session {
                repo,
                permission,
                orchestrator,
                plan_mode,
                environment: crate::model::EnvironmentRequest::LocalCheckout {
                    branch: "main".into(),
                    create_from: None,
                },
            }),
        )
        .await
        .unwrap()
        .id;
}

fn options(provider: ProviderKind) -> Options {
    Options {
        thread: provider,
        files: vec![
            ("a.txt", "base\n"),
            ("b.txt", "base\n"),
            ("other.txt", "base\n"),
        ],
        ..Default::default()
    }
}

/// The live pill, pre-request saves, command ownership, whole output, direct diffs, restart,
/// private retention and a later-save refusal all exercise the same request in each provider.
#[tokio::test]
async fn request_changes_local_edits_commands_restart_and_undo_preserve_user_files_and_index() {
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        let mut flow = Flow::start(
            "20261015-0226-rebuild-wow-w69-record-local",
            options(provider),
            script(move |turn| async move {
                let id = turn.conversation().unwrap();
                let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
                edit(
                    &turn,
                    &workspace.join("a.txt"),
                    "edit-a",
                    "user before\n",
                    "user before\nthread a\n",
                )
                .await;
                let board = observed(&turn, 1).await;
                let request = board.latest_request().unwrap();
                assert_eq!(request.changes.as_ref().unwrap().stat.insertions, 1);
                assert_eq!(request.changes.as_ref().unwrap().stat.deletions, 0);
                let refused = turn
                    .host
                    .undo_request(id.clone(), request.id.clone(), false)
                    .await
                    .unwrap_err();
                assert!(refused.to_string().contains("its work is still going"));
                edit(
                    &turn,
                    &workspace.join("b.txt"),
                    "edit-b",
                    "base\n",
                    "base\nthread b\n",
                )
                .await;
                observed(&turn, 2).await;
                std::fs::write(workspace.join("command.txt"), "command wrote this\n").unwrap();
                let output = "command output\n".repeat(1000);
                if provider == ProviderKind::Codex {
                    turn.emit(ProviderEvent::CommandOutputDelta {
                        item_id: "command".into(),
                        text: output.clone(),
                    })
                    .await;
                }
                turn.emit(ProviderEvent::Command {
                    kept: None,
                    item_id: "command".into(),
                    command: "print and write".into(),
                    cwd: None,
                    status: ItemStatus::Completed,
                    exit_code: Some(0),
                    output: Some(if provider == ProviderKind::Codex {
                        output[..8192].to_owned()
                    } else {
                        output
                    }),
                    duration_ms: None,
                })
                .await;
                observed(&turn, 3).await;
                Reply::text("Changed the two files.")
            }),
        )
        .await;
        local(&mut flow).await;
        std::fs::write(flow.repo.join("a.txt"), "user before\n").unwrap();
        std::fs::write(flow.repo.join("other.txt"), "unrelated user save\n").unwrap();
        git(&flow.repo, &["add", "a.txt"]);
        let index_path = flow.repo.join(".git/index");
        let index = std::fs::read(&index_path).unwrap();
        let head = git(&flow.repo, &["rev-parse", "HEAD"]);
        let branches = git(&flow.repo, &["branch"]);
        let log = git(&flow.repo, &["log", "--oneline"]);
        flow.say("Edit a and b, and print.").await;
        let board = flow
            .until("settled changes", |board| {
                board
                    .latest_request()
                    .and_then(|r| r.changes.as_ref())
                    .is_some_and(|c| c.settled)
            })
            .await;
        let request = board.latest_request().unwrap().id.clone();
        let changes = board.latest_request().unwrap().changes.as_ref().unwrap();
        assert_eq!(changes.stat.files.len(), 3);
        assert_eq!(changes.held.len(), 1);
        assert!(
            changes.held[0]
                .reason
                .starts_with("A command changed command.txt")
        );
        let items = flow
            .manager
            .thread_items(&flow.conversation, &request, None, None, 100)
            .await
            .unwrap();
        assert!(items.entries.iter().any(|entry| matches!(&entry.event, ProviderEvent::Command { output: Some(output), .. } if output == &"command output\n".repeat(1000))));
        let scope = ReviewScope::LastTurn {
            request_id: Some(request.clone()),
        };
        let diff = flow
            .manager
            .review_diff(&flow.conversation, scope.clone(), false, false)
            .await
            .unwrap();
        let record_ref = format!(
            "refs/brigadier/changes/{}/{request}/record",
            flow.conversation
        );
        let actual_patch = git(
            &flow.repo,
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--find-renames",
                "--unified=3",
                &format!("{record_ref}^"),
                &record_ref,
                "--",
            ],
        );
        assert_eq!(diff.patch.trim(), actual_patch);
        assert_eq!(
            flow.manager
                .review_diff(
                    &flow.conversation,
                    ReviewScope::LastTurn { request_id: None },
                    false,
                    false
                )
                .await
                .unwrap()
                .patch,
            diff.patch
        );
        assert_eq!(diff.files.len(), 3);
        assert!(diff.patch.contains("+thread a"));
        assert!(!diff.patch.contains("-base\n+user before"));
        assert!(!diff.patch.contains("other.txt"));
        assert_eq!(git(&flow.repo, &["branch"]), branches);
        assert_eq!(git(&flow.repo, &["log", "--oneline"]), log);
        git(&flow.repo, &["gc", "--prune=now"]);
        flow.restart().await;
        assert_eq!(
            flow.manager
                .review_diff(&flow.conversation, scope, false, false)
                .await
                .unwrap()
                .patch,
            diff.patch
        );
        let outcome = flow
            .manager
            .undo_request(flow.conversation.clone(), request.clone(), false)
            .await
            .unwrap();
        assert_eq!(outcome.restored, ["a.txt", "b.txt"]);
        assert_eq!(outcome.held.len(), 1);
        assert_eq!(
            std::fs::read_to_string(flow.repo.join("a.txt")).unwrap(),
            "user before\n"
        );
        assert_eq!(
            std::fs::read_to_string(flow.repo.join("other.txt")).unwrap(),
            "unrelated user save\n"
        );
        assert!(
            flow.board().await.requests[&request]
                .undo
                .as_ref()
                .unwrap()
                .reverted
        );
        let outcome = flow
            .manager
            .undo_request(flow.conversation.clone(), request.clone(), true)
            .await
            .unwrap();
        assert_eq!(outcome.restored, ["a.txt", "b.txt"]);
        assert!(
            !flow.board().await.requests[&request]
                .undo
                .as_ref()
                .unwrap()
                .reverted
        );
        assert_eq!(
            std::fs::read_to_string(flow.repo.join("a.txt")).unwrap(),
            "user before\nthread a\n"
        );
        assert_eq!(git(&flow.repo, &["rev-parse", "HEAD"]), head);
        assert_eq!(std::fs::read(&index_path).unwrap(), index);
        std::fs::write(flow.repo.join("a.txt"), "later save\n").unwrap();
        let err = flow
            .manager
            .undo_request(flow.conversation.clone(), request.clone(), false)
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("Later changes touch the same files (a.txt)")
        );
        assert_eq!(
            std::fs::read_to_string(flow.repo.join("b.txt")).unwrap(),
            "base\nthread b\n"
        );
        let refs = git(
            &flow.repo,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/brigadier/changes/",
            ],
        );
        assert!(refs.contains("/record") && refs.contains("/latest"));
        flow.manager
            .delete_conversation(flow.conversation.clone(), false, false)
            .await
            .unwrap();
        assert!(
            git(
                &flow.repo,
                &[
                    "for-each-ref",
                    "--format=%(refname)",
                    "refs/brigadier/changes/"
                ]
            )
            .is_empty()
        );
        flow.stop().await;
    }
}

#[tokio::test]
async fn request_changes_mid_request_saves_are_held_or_unowned() {
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        let flow = Flow::start(
            "20261015-0226-rebuild-wow-w69-record-held",
            options(provider),
            script(|turn| async move {
                let id = turn.conversation().unwrap();
                let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
                edit(
                    &turn,
                    &workspace.join("a.txt"),
                    "first",
                    "base\n",
                    "base\nthread\n",
                )
                .await;
                observed(&turn, 1).await;
                std::fs::write(workspace.join("other.txt"), "user unrelated\n").unwrap();
                std::fs::write(workspace.join("a.txt"), "user and thread\n").unwrap();
                edit(
                    &turn,
                    &workspace.join("a.txt"),
                    "second",
                    "user and thread\n",
                    "user and thread\nthread again\n",
                )
                .await;
                tokio::time::timeout(super::PATIENCE, async {
                    loop {
                        let board = turn.host.core.board(&id).await.unwrap();
                        if board
                            .latest_request()
                            .and_then(|r| r.changes.as_ref())
                            .is_some_and(|c| !c.held.is_empty())
                        {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
                edit(
                    &turn,
                    &workspace.join("b.txt"),
                    "third",
                    "base\n",
                    "thread b\n",
                )
                .await;
                observed(&turn, 2).await;
                Reply::text("Finished.")
            }),
        )
        .await;
        flow.say("Edit files.").await;
        let board = flow
            .until("settled held record", |b| {
                b.latest_request()
                    .and_then(|r| r.changes.as_ref())
                    .is_some_and(|c| c.settled)
            })
            .await;
        let request = board.latest_request().unwrap();
        assert_eq!(request.changes.as_ref().unwrap().stat.files.len(), 2);
        let (workspace, _) = flow.manager.changes_checkout(&flow.conversation).unwrap();
        let outcome = flow
            .manager
            .undo_request(flow.conversation.clone(), request.id.clone(), false)
            .await
            .unwrap();
        assert_eq!(outcome.restored, ["b.txt"]);
        assert_eq!(
            outcome.held[0].reason,
            "You changed a.txt during this request too, so Undo leaves it as it is."
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
            "user and thread\nthread again\n"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("other.txt")).unwrap(),
            "user unrelated\n"
        );
        flow.manager
            .undo_request(flow.conversation.clone(), request.id.clone(), true)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.join("b.txt")).unwrap(),
            "thread b\n"
        );
        flow.stop().await;
    }
}

/// A slow snapshot never holds up the thread: its events go on being handled meanwhile, and two
/// edits to one file reported while it waits are both the thread's.
#[tokio::test]
async fn request_changes_snapshots_run_behind_the_thread_in_order() {
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        let flow = Flow::start(
            "20261015-0226-rebuild-wow-w70-record-behind",
            options(provider),
            script(|turn| async move {
                let id = turn.conversation().unwrap();
                let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
                edit(&turn, &workspace.join("a.txt"), "a", "base\n", "thread a\n").await;
                let board = observed(&turn, 1).await;
                let request = board.latest_request().unwrap().id.clone();
                let lock = turn.host.change_records.lock(&id);
                let slow = lock.lock().await;
                edit(&turn, &workspace.join("b.txt"), "b1", "base\n", "base\none\n").await;
                // The second edit comes once the thread has read the first.
                tokio::time::timeout(super::PATIENCE, async {
                    while !turn.host.change_records.pending(&id) {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
                edit(
                    &turn,
                    &workspace.join("b.txt"),
                    "b2",
                    "base\none\n",
                    "base\none\ntwo\n",
                )
                .await;
                turn.emit(ProviderEvent::Command { kept: None,
                    item_id: "probe".into(),
                    command: "true".into(),
                    cwd: None,
                    status: ItemStatus::Completed,
                    exit_code: Some(0),
                    output: None,
                    duration_ms: None,
                })
                .await;
                tokio::time::timeout(super::PATIENCE, async {
                    loop {
                        let board = turn.host.core.board(&id).await.unwrap();
                        if board.orchestrator_steps.iter().any(|step| {
                            matches!(&step.kind, OrchestratorStepKind::Tool { item_id, .. } if item_id == "probe")
                        }) {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("the thread's events wait for a snapshot");
                let board = turn.host.core.board(&id).await.unwrap();
                assert_eq!(
                    board.requests[&request].changes.as_ref().unwrap().stat.files.len(),
                    1
                );
                drop(slow);
                observed(&turn, 2).await;
                Reply::text("Finished.")
            }),
        )
        .await;
        flow.say("Edit a and b.").await;
        let board = flow
            .until("settled record", |b| {
                b.latest_request()
                    .and_then(|r| r.changes.as_ref())
                    .is_some_and(|c| c.settled)
            })
            .await;
        let request = board.latest_request().unwrap();
        let changes = request.changes.as_ref().unwrap();
        assert_eq!(changes.stat.files.len(), 2);
        assert!(changes.held.is_empty(), "{:?}", changes.held);
        let outcome = flow
            .manager
            .undo_request(flow.conversation.clone(), request.id.clone(), false)
            .await
            .unwrap();
        assert_eq!(outcome.restored, ["a.txt", "b.txt"]);
        let (workspace, _) = flow.manager.changes_checkout(&flow.conversation).unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.join("b.txt")).unwrap(),
            "base\n"
        );
        flow.stop().await;
    }
}

#[tokio::test]
async fn request_changes_worktree_commits_and_late_worker_landing_round_trip() {
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        let mut flow = Flow::start("20261015-0226-rebuild-wow-w69-record-landing", options(provider), script(|turn| async move {
            if !turn.is_orchestrator() {
                turn.write("worker.txt", "worker landed\n");
                let reply = turn.call("submit_report", serde_json::json!({"summary": "Added worker.txt", "changes": ["worker.txt"]})).await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Reported.");
            }
            if turn.input.contains("[report task-1") {
                let reply = turn.call("land_phase", serde_json::json!({"task": "task-1"})).await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Both edits and worker are in.");
            }
            if turn.earlier > 0 { return Reply::text("Finished."); }
            let id = turn.conversation().unwrap();
            let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
            edit(&turn, &workspace.join("a.txt"), "edit-a", "base\n", "thread a\n").await;
            observed(&turn, 1).await;
            edit(&turn, &workspace.join("b.txt"), "edit-b", "base\n", "thread b\n").await;
            observed(&turn, 2).await;
            git(&workspace, &["add", "a.txt", "b.txt"]);
            git(&workspace, &["commit", "-q", "-m", "Thread edits"]);
            turn.emit(ProviderEvent::Command { kept: None, item_id: "last-command".into(), command: "commit edits".into(), cwd: None, status: ItemStatus::Completed, exit_code: Some(0), output: Some("done".into()), duration_ms: None }).await;
            let reply = turn.call("delegate_task", serde_json::json!({"effort": "high", "title": "Add worker file", "kind": "implement", "spec": "Create worker.txt.", "provider": "codex"})).await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("[quiet]")
        })).await;
        flow.say("Edit a and b and start a worker.").await;
        let board = flow
            .until("late landing and settled record", |b| {
                b.tasks.values().any(|t| t.landed.is_some())
                    && b.latest_request()
                        .and_then(|r| r.changes.as_ref())
                        .is_some_and(|c| c.settled)
            })
            .await;
        let request = board.latest_request().unwrap();
        assert_eq!(request.changes.as_ref().unwrap().stat.files.len(), 3);
        assert!(
            request.changes.as_ref().unwrap().held.is_empty(),
            "{:?}",
            request.changes
        );
        let (workspace, _) = flow.manager.changes_checkout(&flow.conversation).unwrap();
        git(&workspace, &["gc", "--prune=now"]);
        flow.restart().await;
        let before = git(&workspace, &["rev-parse", "HEAD"]);
        let outcome = flow
            .manager
            .undo_request(flow.conversation.clone(), request.id.clone(), false)
            .await
            .unwrap();
        assert_eq!(outcome.restored, ["a.txt", "b.txt", "worker.txt"]);
        assert_ne!(git(&workspace, &["rev-parse", "HEAD"]), before);
        assert!(git(&workspace, &["log", "-1", "--format=%s"]).starts_with("Undo \""));
        assert_eq!(
            std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
            "base\n"
        );
        assert!(!workspace.join("worker.txt").exists());
        flow.manager
            .undo_request(flow.conversation.clone(), request.id.clone(), true)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.join("worker.txt")).unwrap(),
            "worker landed\n"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("b.txt")).unwrap(),
            "thread b\n"
        );
        assert!(git(&workspace, &["status", "--porcelain"]).is_empty());
        flow.stop().await;
    }
}

#[tokio::test]
async fn request_changes_generated_and_viewed_images_are_conversation_scoped_and_retained() {
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        let flow = Flow::start(
            "20261015-0226-rebuild-wow-w69-images",
            options(provider),
            script(|turn| async move {
                let generated = turn.cwd.join("generated.png");
                let viewed = turn.cwd.join("viewed.png");
                std::fs::write(&generated, b"generated image").unwrap();
                std::fs::write(&viewed, b"viewed image").unwrap();
                turn.emit(ProviderEvent::Image {
                    item_id: "generated".into(),
                    status: ItemStatus::Completed,
                    path: Some(generated.display().to_string()),
                    prompt: Some("A picture".into()),
                })
                .await;
                let (name, input) = if turn.provider == ProviderKind::Claude {
                    (
                        "Read",
                        serde_json::json!({"file_path": viewed.display().to_string()}).to_string(),
                    )
                } else {
                    (
                        "view_image",
                        serde_json::json!({"path": viewed.display().to_string()}).to_string(),
                    )
                };
                turn.emit(ProviderEvent::ToolCall {
                    kept: None,
                    item_id: "viewed".into(),
                    name: name.into(),
                    input: Some(input),
                    status: ItemStatus::Completed,
                    output: None,
                })
                .await;
                let oversized = turn.cwd.join("oversized.png");
                std::fs::File::create(&oversized)
                    .unwrap()
                    .set_len(11 * 1024 * 1024 + 1)
                    .unwrap();
                turn.emit(ProviderEvent::ToolCall {
                    kept: None,
                    item_id: "oversized".into(),
                    name: "view_image".into(),
                    input: Some(oversized.display().to_string()),
                    status: ItemStatus::Completed,
                    output: None,
                })
                .await;
                Reply::text("Images ready.")
            }),
        )
        .await;
        flow.say("Make and view images.").await;
        let board = flow
            .until("images settled", |b| {
                b.latest_request()
                    .and_then(|r| r.changes.as_ref())
                    .is_some_and(|c| c.settled)
            })
            .await;
        let request = board.latest_request().unwrap();
        let items = flow
            .manager
            .thread_items(&flow.conversation, &request.id, None, None, 100)
            .await
            .unwrap();
        let generated = items
            .entries
            .iter()
            .find_map(|entry| match &entry.event {
                ProviderEvent::Image { path, .. } => path.clone(),
                _ => None,
            })
            .unwrap();
        assert!(generated.contains("thread-images/"));
        assert!(board.orchestrator_steps.iter().any(|step| step.request_id.as_deref() == Some(&request.id) && matches!(&step.kind, OrchestratorStepKind::Tool { name, detail, turn: Some(_), .. } if name == "imagegen" && detail.as_deref() == Some("A picture"))));
        let scratch = flow.manager.owned_dir("orch", &flow.conversation.0);
        std::fs::remove_file(scratch.join("generated.png")).unwrap();
        let (data, mime) = flow
            .manager
            .read_thread_image(&flow.conversation, "generated")
            .await
            .unwrap();
        assert_eq!(data, brigadier_providers::image_base64(b"generated image"));
        assert_eq!(mime, "image/png");
        let (data, _) = flow
            .manager
            .read_thread_image(&flow.conversation, "viewed")
            .await
            .unwrap();
        assert_eq!(data, brigadier_providers::image_base64(b"viewed image"));
        std::fs::remove_file(scratch.join("viewed.png")).unwrap();
        assert!(
            flow.manager
                .read_thread_image(&flow.conversation, "viewed")
                .await
                .unwrap_err()
                .to_string()
                .contains("This image is no longer there")
        );
        assert!(
            flow.manager
                .read_thread_image(&flow.conversation, "oversized")
                .await
                .unwrap_err()
                .to_string()
                .contains("over 11 MB")
        );
        assert!(
            flow.manager
                .read_thread_image(&flow.conversation, "unknown")
                .await
                .is_err()
        );
        let peer = flow.add_session("Other", None, provider).await;
        assert!(
            flow.manager
                .read_thread_image(&peer, "generated")
                .await
                .is_err()
        );
        flow.manager
            .delete_conversation(flow.conversation.clone(), false, false)
            .await
            .unwrap();
        assert!(!Path::new(&generated).exists());
        flow.stop().await;
    }
}

/// Recovery may see the request's terminal state before its final file refresh was recorded.
#[tokio::test]
async fn request_changes_recovery_finalizes_an_unsettled_record_without_owning_user_saves() {
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        let mut flow = Flow::start(
            "20261015-0226-rebuild-wow-w69-recovery",
            options(provider),
            script(|turn| async move {
                let (workspace, _) = turn
                    .host
                    .changes_checkout(&turn.conversation().unwrap())
                    .unwrap();
                edit(
                    &turn,
                    &workspace.join("a.txt"),
                    "edit",
                    "base\n",
                    "thread\n",
                )
                .await;
                observed(&turn, 1).await;
                Reply::text("Finished.")
            }),
        )
        .await;
        local(&mut flow).await;
        flow.say("Edit a.").await;
        let board = flow
            .until("final record", |b| {
                b.latest_request()
                    .and_then(|r| r.changes.as_ref())
                    .is_some_and(|c| c.settled)
            })
            .await;
        let mut request = board.latest_request().unwrap().clone();
        let mut unpublished = request.clone();
        unpublished.changes = None;
        flow.core
            .record_conversation(
                &flow.conversation,
                vec![crate::model::DomainEvent::RequestUpdated {
                    request: unpublished,
                }],
            )
            .await
            .unwrap();
        flow.restart().await;
        assert!(
            flow.board().await.requests[&request.id]
                .changes
                .as_ref()
                .unwrap()
                .settled
        );

        request.changes.as_mut().unwrap().settled = false;
        let stream = format!("changes:{}:{}", flow.conversation, request.id);
        let events = flow
            .core
            .store()
            .read_stream(
                stream.clone(),
                brigadier_store::StreamPage {
                    limit: 1,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let mut record: serde_json::Value = serde_json::from_str(events[0].payload.get()).unwrap();
        record["settled"] = serde_json::json!(false);
        flow.core
            .store()
            .append(vec![
                brigadier_store::NewEvent::new(
                    stream,
                    "request.change_record",
                    crate::now_ms(),
                    &record,
                )
                .unwrap(),
            ])
            .await
            .unwrap();
        flow.core
            .record_conversation(
                &flow.conversation,
                vec![crate::model::DomainEvent::RequestUpdated {
                    request: request.clone(),
                }],
            )
            .await
            .unwrap();
        std::fs::write(flow.repo.join("a.txt"), "user saved after edit\n").unwrap();
        std::fs::write(flow.repo.join("other.txt"), "another user save\n").unwrap();
        flow.restart().await;
        let board = flow.board().await;
        let changes = board.requests[&request.id].changes.as_ref().unwrap();
        assert!(changes.settled);
        assert_eq!(changes.stat.files.len(), 1);
        assert_eq!(changes.held[0].path, "a.txt");
        let outcome = flow
            .manager
            .undo_request(flow.conversation.clone(), request.id, false)
            .await
            .unwrap();
        assert!(outcome.restored.is_empty());
        assert_eq!(outcome.held.len(), 1);
        assert_eq!(
            std::fs::read_to_string(flow.repo.join("a.txt")).unwrap(),
            "user saved after edit\n"
        );
        assert_eq!(
            std::fs::read_to_string(flow.repo.join("other.txt")).unwrap(),
            "another user save\n"
        );
        flow.stop().await;
    }
}

#[tokio::test]
async fn request_changes_background_runs_keep_the_starting_item_and_return_the_final_tail() {
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        let release = Arc::new(tokio::sync::Notify::new());
        let proceed = release.clone();
        let flow = Flow::start("20261015-0226-rebuild-wow-w69-background", options(provider), script(move |turn| {
            let release = proceed.clone();
            async move {
                if turn.earlier > 0 { return Reply::text("The background run finished."); }
                let id = turn.conversation().unwrap();
                turn.emit(ProviderEvent::ToolCall { kept: None, item_id: "background-call".into(), name: "mcp__brigadier__run".into(), input: Some("{\"command\":\"controlled output\"}".into()), status: ItemStatus::InProgress, output: None }).await;
                tokio::time::timeout(super::PATIENCE, async {
                    loop {
                        let board = turn.host.core.board(&id).await.unwrap();
                        if board.orchestrator_steps.iter().any(|step| matches!(&step.kind, OrchestratorStepKind::Tool { item_id, .. } if item_id == "background-call")) { break; }
                        tokio::task::yield_now().await;
                    }
                }).await.unwrap();
                let reply = turn.host.in_foreground(&id, ("run", None), "controlled output", tokio_util::sync::CancellationToken::new(), tokio_util::sync::CancellationToken::new(), async move {
                    release.notified().await;
                    Ok(("exit 0".into(), "é".repeat(160_000)))
                }).await.unwrap();
                turn.emit(ProviderEvent::ToolCall { kept: None, item_id: "background-call".into(), name: "mcp__brigadier__run".into(), input: None, status: ItemStatus::Completed, output: Some(reply) }).await;
                Reply::text("[quiet]")
            }
        })).await;
        flow.say("Run the controlled background command.").await;
        flow.until("background item", |b| {
            b.background_runs
                .get("run-1")
                .is_some_and(|r| r.item_id.as_deref() == Some("background-call"))
        })
        .await;
        assert_eq!(
            flow.manager
                .read_background_run(&flow.conversation, "run-1")
                .await
                .unwrap(),
            (String::new(), 0)
        );
        assert!(
            flow.manager
                .read_background_run(&flow.conversation, "missing")
                .await
                .is_err()
        );
        release.notify_one();
        flow.until("background finished", |b| {
            b.background_runs.get("run-1").is_some_and(|r| {
                matches!(r.state, crate::work::BackgroundRunState::Finished { .. })
            })
        })
        .await;
        let (output, bytes) = flow
            .manager
            .read_background_run(&flow.conversation, "run-1")
            .await
            .unwrap();
        assert_eq!(bytes, 320_000);
        assert!(output.len() <= 256 * 1024);
        assert_eq!(output, "é".repeat(128 * 1024));
        flow.stop().await;
    }
}

/// A request that only answered has a record that changed nothing: Last Turn with no request
/// named still shows the latest request that changed files.
#[tokio::test]
async fn last_turn_skips_a_later_request_that_changed_nothing() {
    let flow = Flow::start(
        "20261015-0226-rebuild-wow-w77-last-turn",
        options(ProviderKind::Codex),
        script(move |turn| async move {
            if turn.input.contains("Edit a") {
                let id = turn.conversation().unwrap();
                let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
                edit(
                    &turn,
                    &workspace.join("a.txt"),
                    "edit-a",
                    "base\n",
                    "base\nthread a\n",
                )
                .await;
                observed(&turn, 1).await;
                Reply::text("Changed a.")
            } else {
                Reply::text("Nothing to change.")
            }
        }),
    )
    .await;
    flow.say("Edit a.").await;
    let board = flow
        .until("settled changes", |board| {
            board
                .latest_request()
                .and_then(|r| r.changes.as_ref())
                .is_some_and(|c| c.settled)
        })
        .await;
    let edited = board.latest_request().unwrap().id.clone();
    flow.say("Just answer.").await;
    let board = flow
        .until("the answer's record", |board| {
            board
                .latest_request()
                .is_some_and(|r| r.id != edited && r.changes.as_ref().is_some_and(|c| c.settled))
        })
        .await;
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
    let latest = flow
        .manager
        .review_diff(
            &flow.conversation,
            ReviewScope::LastTurn { request_id: None },
            false,
            false,
        )
        .await
        .unwrap();
    let named = flow
        .manager
        .review_diff(
            &flow.conversation,
            ReviewScope::LastTurn {
                request_id: Some(edited),
            },
            false,
            false,
        )
        .await
        .unwrap();
    assert!(latest.patch.contains("+thread a"));
    assert_eq!(latest.patch, named.patch);
    flow.stop().await;
}

/// A move whose old contents can't be verified (its diff was cut) holds both ends, so Undo
/// never removes the file where it went while leaving where it came from empty.
#[tokio::test]
async fn request_changes_hold_both_ends_of_a_move_they_cannot_verify() {
    let mut flow = Flow::start(
        "20261015-0226-rebuild-wow-w79-move-unverified",
        options(ProviderKind::Codex),
        script(move |turn| async move {
            let id = turn.conversation().unwrap();
            let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
            std::fs::remove_file(workspace.join("a.txt")).unwrap();
            std::fs::write(workspace.join("moved.txt"), "base\nmoved\n").unwrap();
            let mut change = FileChange::counted(
                workspace.join("a.txt").to_string_lossy().into_owned(),
                FileChangeKind::Update,
                Some("@@ -1,1 +1,2 @@\n base\n+moved\n[… 4096 more bytes]\n"),
                usize::MAX,
            );
            change.moved_to = Some(workspace.join("moved.txt").to_string_lossy().into_owned());
            turn.emit(ProviderEvent::FileChanges {
                item_id: "move-a".into(),
                changes: vec![change],
                status: ItemStatus::Completed,
            })
            .await;
            observed(&turn, 2).await;
            Reply::text("Moved a.")
        }),
    )
    .await;
    local(&mut flow).await;
    flow.say("Move a.").await;
    let board = flow
        .until("settled changes", |board| {
            board
                .latest_request()
                .and_then(|r| r.changes.as_ref())
                .is_some_and(|c| c.settled)
        })
        .await;
    let request = board.latest_request().unwrap();
    let held: Vec<&str> = request
        .changes
        .as_ref()
        .unwrap()
        .held
        .iter()
        .map(|h| h.path.as_str())
        .collect();
    assert!(
        held.contains(&"a.txt") && held.contains(&"moved.txt"),
        "{held:?}"
    );
    flow.manager
        .undo_request(flow.conversation.clone(), request.id.clone(), false)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(flow.repo.join("moved.txt")).unwrap(),
        "base\nmoved\n"
    );
    flow.stop().await;
}

/// A Codex patch that only renames a file reports a diff with no hunks: the file went with
/// its bytes, so both ends are the thread's and Undo puts it back where it was.
#[tokio::test]
async fn request_changes_undo_a_codex_rename_without_hunks() {
    let mut flow = Flow::start(
        "20261015-0226-rebuild-wow-w79-rename",
        options(ProviderKind::Codex),
        script(move |turn| async move {
            let id = turn.conversation().unwrap();
            let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
            std::fs::rename(workspace.join("a.txt"), workspace.join("renamed.txt")).unwrap();
            // What Codex reports for a pure rename.
            let diff = format!("\n\nMoved to: {}", workspace.join("renamed.txt").display());
            let mut change = FileChange::counted(
                workspace.join("a.txt").to_string_lossy().into_owned(),
                FileChangeKind::Update,
                Some(&diff),
                usize::MAX,
            );
            change.moved_to = Some(workspace.join("renamed.txt").to_string_lossy().into_owned());
            turn.emit(ProviderEvent::FileChanges {
                item_id: "rename-a".into(),
                changes: vec![change],
                status: ItemStatus::Completed,
            })
            .await;
            // git counts an exact rename as one file.
            observed(&turn, 1).await;
            Reply::text("Renamed a.")
        }),
    )
    .await;
    local(&mut flow).await;
    flow.say("Rename a.").await;
    let board = flow
        .until("settled changes", |board| {
            board
                .latest_request()
                .and_then(|r| r.changes.as_ref())
                .is_some_and(|c| c.settled)
        })
        .await;
    let request = board.latest_request().unwrap();
    let changes = request.changes.as_ref().unwrap();
    assert!(changes.held.is_empty(), "{:?}", changes.held);
    flow.manager
        .undo_request(flow.conversation.clone(), request.id.clone(), false)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(flow.repo.join("a.txt")).unwrap(),
        "base\n"
    );
    assert!(!flow.repo.join("renamed.txt").exists());
    flow.stop().await;
}

/// A Codex patch that moves a file records both ends: the diff shows where it went and what
/// changed, and Undo puts it back where it was.
#[tokio::test]
async fn request_changes_record_both_ends_of_a_codex_move() {
    let mut flow = Flow::start(
        "20261015-0226-rebuild-wow-w77-move",
        options(ProviderKind::Codex),
        script(move |turn| async move {
            let id = turn.conversation().unwrap();
            let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
            std::fs::remove_file(workspace.join("a.txt")).unwrap();
            std::fs::write(workspace.join("moved.txt"), "base\nmoved\n").unwrap();
            let mut change = FileChange::counted(
                workspace.join("a.txt").to_string_lossy().into_owned(),
                FileChangeKind::Update,
                Some("@@ -1,1 +1,2 @@\n base\n+moved\n"),
                usize::MAX,
            );
            change.moved_to = Some(workspace.join("moved.txt").to_string_lossy().into_owned());
            turn.emit(ProviderEvent::FileChanges {
                item_id: "move-a".into(),
                changes: vec![change],
                status: ItemStatus::Completed,
            })
            .await;
            observed(&turn, 2).await;
            Reply::text("Moved a.")
        }),
    )
    .await;
    local(&mut flow).await;
    flow.say("Move a.").await;
    let board = flow
        .until("settled changes", |board| {
            board
                .latest_request()
                .and_then(|r| r.changes.as_ref())
                .is_some_and(|c| c.settled)
        })
        .await;
    let request = board.latest_request().unwrap();
    let changes = request.changes.as_ref().unwrap();
    let paths: Vec<&str> = changes.stat.files.iter().map(|f| f.path.as_str()).collect();
    assert!(paths.contains(&"moved.txt"), "{paths:?}");
    assert!(changes.held.is_empty(), "{:?}", changes.held);
    let request = request.id.clone();
    let diff = flow
        .manager
        .review_diff(
            &flow.conversation,
            ReviewScope::LastTurn {
                request_id: Some(request.clone()),
            },
            false,
            false,
        )
        .await
        .unwrap();
    assert!(diff.patch.contains("moved.txt"), "{}", diff.patch);
    assert!(diff.patch.contains("+moved"), "{}", diff.patch);
    flow.manager
        .undo_request(flow.conversation.clone(), request, false)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(flow.repo.join("a.txt")).unwrap(),
        "base\n"
    );
    assert!(!flow.repo.join("moved.txt").exists());
    flow.stop().await;
}

/// A Claude edit to a file with a secret in it comes with its original redacted: that isn't
/// what the file held, so the record reads the file's before from the diff instead, and the
/// file stays the thread's own to undo, its diff only the edited line.
#[tokio::test]
async fn request_changes_read_a_redacted_original_from_the_diff() {
    let options = Options {
        thread: ProviderKind::Claude,
        files: vec![("config.txt", "token = hidden-value\nmode = a\n")],
        ..Default::default()
    };
    let flow = Flow::start(
        "20261015-0226-rebuild-wow-w79-redacted",
        options,
        script(move |turn| async move {
            let id = turn.conversation().unwrap();
            let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
            let path = workspace.join("config.txt");
            std::fs::write(&path, "token = hidden-value\nmode = b\n").unwrap();
            let mut change = FileChange::counted(
                path.to_string_lossy().into_owned(),
                FileChangeKind::Update,
                Some("@@ -2,1 +2,1 @@\n-mode = a\n+mode = b\n"),
                usize::MAX,
            );
            change.original = Some(format!(
                "token = {}\nmode = a\n",
                brigadier_providers::REDACTED
            ));
            turn.emit(ProviderEvent::FileChanges {
                item_id: "edit-config".into(),
                changes: vec![change],
                status: ItemStatus::Completed,
            })
            .await;
            observed(&turn, 1).await;
            Reply::text("Changed the mode.")
        }),
    )
    .await;
    flow.say("Change the mode.").await;
    let board = flow
        .until("settled changes", |board| {
            board
                .latest_request()
                .and_then(|r| r.changes.as_ref())
                .is_some_and(|c| c.settled)
        })
        .await;
    let request = board.latest_request().unwrap();
    let changes = request.changes.as_ref().unwrap();
    assert!(changes.held.is_empty(), "{:?}", changes.held);
    let request = request.id.clone();
    let diff = flow
        .manager
        .review_diff(
            &flow.conversation,
            ReviewScope::LastTurn {
                request_id: Some(request.clone()),
            },
            false,
            false,
        )
        .await
        .unwrap();
    assert!(diff.patch.contains("+mode = b"), "{}", diff.patch);
    // The page of the thread's calls shows the edit without the whole file it came from.
    let page = flow
        .manager
        .thread_items(&flow.conversation, &request, None, None, 100)
        .await
        .unwrap();
    let edits: Vec<_> = page
        .entries
        .iter()
        .filter_map(|entry| match &entry.event {
            ProviderEvent::FileChanges { changes, .. } => Some(changes),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(!edits.is_empty());
    assert!(
        edits.iter().all(|change| change.original.is_none()),
        "{edits:?}"
    );
    // Nor does the stored log the app and the transcript read.
    let logged = flow
        .core
        .store()
        .read_stream(
            crate::model::streams::orchestrator(&flow.conversation),
            brigadier_store::StreamPage {
                limit: 500,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        logged
            .iter()
            .any(|event| event.payload.get().contains("fileChanges"))
    );
    assert!(
        logged
            .iter()
            .all(|event| !event.payload.get().contains("\"original\"")),
        "an original was stored"
    );
    assert!(
        !diff.patch.contains("-token") && !diff.patch.contains(brigadier_providers::REDACTED),
        "{}",
        diff.patch
    );
    flow.manager
        .undo_request(flow.conversation.clone(), request, false)
        .await
        .unwrap();
    let (workspace, _) = flow.manager.changes_checkout(&flow.conversation).unwrap();
    assert_eq!(
        std::fs::read_to_string(workspace.join("config.txt")).unwrap(),
        "token = hidden-value\nmode = a\n"
    );
    flow.stop().await;
}

/// A command's snapshot runs behind the thread: one reported just before a patch can read the
/// checkout after the patch landed and claim its change. The patch's own evidence of that same
/// change takes the file back, so Undo reverts it.
#[tokio::test]
async fn request_changes_give_an_edit_back_from_a_command_snapshot_that_read_it() {
    let flow = Flow::start(
        "20261015-0226-rebuild-wow-w79-late-snapshot",
        options(ProviderKind::Codex),
        script(move |turn| async move {
            let id = turn.conversation().unwrap();
            let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
            let lock = turn.host.change_records.lock(&id);
            let held = lock.lock().await;
            turn.emit(ProviderEvent::Command {
                kept: None,
                item_id: "look".into(),
                command: "ls".into(),
                cwd: None,
                status: ItemStatus::Completed,
                exit_code: Some(0),
                output: Some("a.txt\n".into()),
                duration_ms: Some(1),
            })
            .await;
            // The patch lands before the command's snapshot runs.
            std::fs::write(workspace.join("a.txt"), "base\nthread a\n").unwrap();
            drop(held);
            tokio::time::timeout(super::PATIENCE, async {
                while turn.host.change_records.pending(&id) {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            edit(
                &turn,
                &workspace.join("a.txt"),
                "edit-a",
                "base\n",
                "base\nthread a\n",
            )
            .await;
            observed(&turn, 1).await;
            Reply::text("Changed a.")
        }),
    )
    .await;
    flow.say("Edit a.").await;
    let board = flow
        .until("settled changes", |board| {
            board
                .latest_request()
                .and_then(|r| r.changes.as_ref())
                .is_some_and(|c| c.settled)
        })
        .await;
    let request = board.latest_request().unwrap();
    let changes = request.changes.as_ref().unwrap();
    assert!(changes.held.is_empty(), "{:?}", changes.held);
    flow.manager
        .undo_request(flow.conversation.clone(), request.id.clone(), false)
        .await
        .unwrap();
    let (workspace, _) = flow.manager.changes_checkout(&flow.conversation).unwrap();
    assert_eq!(
        std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
        "base\n"
    );
    flow.stop().await;
}

/// In a work folder, an edit the thread left uncommitted stays uncommitted through Undo and
/// Reapply: after the Undo the file equals HEAD again, and Reapply must not take that for a
/// committed change and commit it.
#[tokio::test]
async fn request_changes_reapply_leaves_an_uncommitted_edit_uncommitted() {
    let flow = Flow::start(
        "20261015-0226-rebuild-wow-w79-reapply-uncommitted",
        options(ProviderKind::Codex),
        script(move |turn| async move {
            let id = turn.conversation().unwrap();
            let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
            edit(
                &turn,
                &workspace.join("a.txt"),
                "edit-a",
                "base\n",
                "base\nthread a\n",
            )
            .await;
            observed(&turn, 1).await;
            Reply::text("Changed a.")
        }),
    )
    .await;
    flow.say("Edit a.").await;
    let board = flow
        .until("settled changes", |board| {
            board
                .latest_request()
                .and_then(|r| r.changes.as_ref())
                .is_some_and(|c| c.settled)
        })
        .await;
    let request = board.latest_request().unwrap().id.clone();
    let (workspace, _) = flow.manager.changes_checkout(&flow.conversation).unwrap();
    let head = git(&workspace, &["rev-parse", "HEAD"]);
    let undone = flow
        .manager
        .undo_request(flow.conversation.clone(), request.clone(), false)
        .await
        .unwrap();
    assert_eq!(undone.restored, ["a.txt"]);
    assert_eq!(
        std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
        "base\n"
    );
    flow.manager
        .undo_request(flow.conversation.clone(), request, true)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
        "base\nthread a\n"
    );
    assert_eq!(git(&workspace, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        git(&workspace, &["status", "--porcelain", "--", "a.txt"]).trim(),
        "M a.txt"
    );
    flow.stop().await;
}

/// A Codex patch that deletes an executable file: the file rebuilt from the patch has the
/// snapshot's mode, so the deletion stays the thread's own and Undo brings the file back
/// executable.
#[tokio::test]
async fn request_changes_undo_a_deleted_executable_with_its_mode() {
    use std::os::unix::fs::PermissionsExt;
    let mut flow = Flow::start(
        "20261015-0226-rebuild-wow-w79-deleted-executable",
        options(ProviderKind::Codex),
        script(move |turn| async move {
            let id = turn.conversation().unwrap();
            let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
            let path = workspace.join("a.txt");
            std::fs::remove_file(&path).unwrap();
            turn.emit(ProviderEvent::FileChanges {
                item_id: "delete-a".into(),
                changes: vec![FileChange::counted(
                    path.to_string_lossy().into_owned(),
                    FileChangeKind::Delete,
                    Some("@@ -1,1 +0,0 @@\n-base\n"),
                    usize::MAX,
                )],
                status: ItemStatus::Completed,
            })
            .await;
            observed(&turn, 1).await;
            Reply::text("Deleted a.")
        }),
    )
    .await;
    local(&mut flow).await;
    let file = flow.repo.join("a.txt");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    flow.say("Delete a.").await;
    let board = flow
        .until("settled changes", |board| {
            board
                .latest_request()
                .and_then(|r| r.changes.as_ref())
                .is_some_and(|c| c.settled)
        })
        .await;
    let request = board.latest_request().unwrap();
    let changes = request.changes.as_ref().unwrap();
    assert!(changes.held.is_empty(), "{:?}", changes.held);
    flow.manager
        .undo_request(flow.conversation.clone(), request.id.clone(), false)
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "base\n");
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o111,
        0o111
    );
    flow.stop().await;
}

/// A background run whose call is linked only after the handoff (its row came late) keeps that
/// link when it ends, so its row still opens its output.
#[tokio::test]
async fn request_changes_background_runs_keep_a_late_link_to_their_call() {
    let release = Arc::new(tokio::sync::Notify::new());
    let proceed = release.clone();
    let flow = Flow::start(
        "20261015-0226-rebuild-wow-w79-late-link",
        options(ProviderKind::Claude),
        script(move |turn| {
            let release = proceed.clone();
            async move {
                if turn.earlier > 0 {
                    return Reply::text("The background run finished.");
                }
                let id = turn.conversation().unwrap();
                let reply = turn
                    .host
                    .in_foreground(
                        &id,
                        ("run", None),
                        "late output",
                        tokio_util::sync::CancellationToken::new(),
                        tokio_util::sync::CancellationToken::new(),
                        async move {
                            release.notified().await;
                            Ok(("exit 0".into(), "late output\n".into()))
                        },
                    )
                    .await
                    .unwrap();
                for status in [ItemStatus::InProgress, ItemStatus::Completed] {
                    turn.emit(ProviderEvent::ToolCall {
                        kept: None,
                        item_id: "late-call".into(),
                        name: "mcp__brigadier__run".into(),
                        input: Some("{\"command\":\"late output\"}".into()),
                        output: (status == ItemStatus::Completed).then(|| reply.clone()),
                        status,
                    })
                    .await;
                }
                Reply::text("[quiet]")
            }
        }),
    )
    .await;
    flow.say("Run the late background command.").await;
    flow.until("the late link", |b| {
        b.background_runs
            .get("run-1")
            .is_some_and(|r| r.item_id.as_deref() == Some("late-call"))
    })
    .await;
    release.notify_one();
    let board = flow
        .until("background finished", |b| {
            b.background_runs.get("run-1").is_some_and(|r| {
                matches!(r.state, crate::work::BackgroundRunState::Finished { .. })
            })
        })
        .await;
    assert_eq!(
        board.background_runs["run-1"].item_id.as_deref(),
        Some("late-call")
    );
    flow.stop().await;
}

/// A message steered into a running turn starts its own request: the edits the rest of the
/// turn makes are that request's, with their counts and Undo.
#[tokio::test]
async fn request_changes_record_a_steered_requests_edits() {
    let flow = Flow::start(
        "20261015-0226-rebuild-wow-w79-steered",
        options(ProviderKind::Claude),
        script(move |turn| async move {
            let id = turn.conversation().unwrap();
            let call = |status| ProviderEvent::ToolCall {
                kept: None,
                item_id: "bash-1".into(),
                name: "Bash".into(),
                input: Some("{\"command\":\"sleep 5\"}".into()),
                output: (status == ItemStatus::Completed).then(|| "ok".into()),
                status,
            };
            turn.emit(call(ItemStatus::InProgress)).await;
            tokio::time::timeout(super::PATIENCE, async {
                loop {
                    let board = turn.host.core.board(&id).await.unwrap();
                    if board.requests.values().any(|r| r.steered_into.is_some()) {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            turn.emit(call(ItemStatus::Completed)).await;
            let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
            edit(
                &turn,
                &workspace.join("a.txt"),
                "edit-a",
                "base\n",
                "base\nthread a\n",
            )
            .await;
            observed(&turn, 1).await;
            Reply::text("Changed a, as you asked.")
        }),
    )
    .await;
    flow.say("Look around.").await;
    flow.until("the turn to go on working", |board| {
        board.orchestrator_steps.iter().any(|step| {
            matches!(&step.kind, OrchestratorStepKind::Tool { status, .. } if *status == ItemStatus::InProgress)
        })
    })
    .await;
    flow.manager
        .send_message(
            flow.conversation.clone(),
            "Also edit a.".into(),
            Vec::new(),
            Vec::new(),
            true,
            None,
        )
        .await
        .unwrap();
    let board = flow
        .until("settled changes", |board| {
            board
                .latest_request()
                .and_then(|r| r.changes.as_ref())
                .is_some_and(|c| c.settled && !c.stat.files.is_empty())
        })
        .await;
    let request = board.latest_request().unwrap();
    assert!(request.steered_into.is_some(), "{request:?}");
    let changes = request.changes.as_ref().unwrap();
    assert_eq!(changes.stat.files[0].path, "a.txt");
    assert!(changes.held.is_empty(), "{:?}", changes.held);
    flow.manager
        .undo_request(flow.conversation.clone(), request.id.clone(), false)
        .await
        .unwrap();
    let (workspace, _) = flow.manager.changes_checkout(&flow.conversation).unwrap();
    assert_eq!(
        std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
        "base\n"
    );
    flow.stop().await;
}

/// On the user's checkout the thread leaves its edit uncommitted; the user has a change of
/// their own. The question before a worker starts tells them apart: the user's file is theirs,
/// the thread's is the thread's, never listed as the user's (#88). A file the user changed
/// before the thread edited it stays the user's: their change is in it too.
#[tokio::test]
async fn the_uncommitted_changes_question_tells_the_threads_edits_from_the_users() {
    let mut flow = Flow::start(
        "20261017-0213-issues-run-w03-uncommitted",
        options(ProviderKind::Claude),
        script(move |turn| async move {
            if !turn.is_orchestrator() {
                std::future::pending::<()>().await;
                return Reply::text("");
            }
            if turn.input.contains("Edit b.") {
                let id = turn.conversation().unwrap();
                let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
                edit(
                    &turn,
                    &workspace.join("b.txt"),
                    "edit-b",
                    "base\n",
                    "base\nthread b\n",
                )
                .await;
                edit(
                    &turn,
                    &workspace.join("other.txt"),
                    "edit-other",
                    "base\nthe user's\n",
                    "base\nthe user's\nthread other\n",
                )
                .await;
                observed(&turn, 2).await;
                return Reply::text("Changed 2 files, not committed.");
            }
            if turn.input.contains("Scout it.") {
                let reply = turn
                    .call(
                        "delegate_task",
                        serde_json::json!({"effort": "high", "title": "Look", "kind": "scout", "spec": "Look around."}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("[quiet]");
            }
            Reply::text("Fine.")
        }),
    )
    .await;
    local(&mut flow).await;
    std::fs::write(flow.repo.join("other.txt"), "base\nthe user's\n").unwrap();
    flow.say("Edit b.").await;
    flow.settled().await;
    std::fs::write(flow.repo.join("a.txt"), "the user's\n").unwrap();
    flow.say("Scout it.").await;
    let board = flow
        .until("the uncommitted-changes question", |board| {
            board.questions.values().any(|question| {
                matches!(
                    question.kind,
                    crate::work::QuestionKind::UncommittedChanges { .. }
                )
            })
        })
        .await;
    let question = board.questions.values().next().unwrap();
    assert_eq!(
        question.kind,
        crate::work::QuestionKind::UncommittedChanges {
            files: vec!["a.txt".into(), "other.txt".into()],
            thread_files: vec!["b.txt".into()],
        }
    );
    assert!(
        question.text.starts_with(
            "Your checkout has 3 uncommitted changes: 2 of yours and 1 made by the thread."
        ),
        "{}",
        question.text
    );
    flow.stop().await;
}

/// The thread edits a file and delegates in the same turn, before the edit's snapshot ran: the
/// question still counts the edit as the thread's, not the user's (#88).
#[tokio::test]
async fn an_edit_just_before_the_delegation_is_the_threads_on_the_question() {
    let mut flow = Flow::start(
        "20261017-0213-issues-run-w08-same-turn",
        options(ProviderKind::Claude),
        script(move |turn| async move {
            if !turn.is_orchestrator() {
                std::future::pending::<()>().await;
                return Reply::text("");
            }
            if turn.input.contains("Edit and scout.") {
                let id = turn.conversation().unwrap();
                let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
                edit(
                    &turn,
                    &workspace.join("b.txt"),
                    "edit-b",
                    "base\n",
                    "base\nthread b\n",
                )
                .await;
                let reply = turn
                    .call(
                        "delegate_task",
                        serde_json::json!({"effort": "high", "title": "Look", "kind": "scout", "spec": "Look around."}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("[quiet]");
            }
            Reply::text("Fine.")
        }),
    )
    .await;
    local(&mut flow).await;
    flow.say("Edit and scout.").await;
    let board = flow
        .until("the uncommitted-changes question", |board| {
            board.questions.values().any(|question| {
                matches!(
                    question.kind,
                    crate::work::QuestionKind::UncommittedChanges { .. }
                )
            })
        })
        .await;
    let question = board.questions.values().next().unwrap();
    assert_eq!(
        question.kind,
        crate::work::QuestionKind::UncommittedChanges {
            files: vec![],
            thread_files: vec!["b.txt".into()],
        }
    );
    flow.stop().await;
}

/// A shell call of the thread's: the files its command names are read before it runs, `run`
/// changes them, and they are read again once it ended (as Claude's tool hook does, #87).
async fn shell(turn: &Turn, item: &str, command: &str, run: impl FnOnce(&Path)) {
    let id = turn.conversation().unwrap();
    let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
    let key = ShellKey::Call(item.into());
    turn.host
        .capture_shell(&id, key.clone(), command, Some(&workspace), None)
        .await;
    run(&workspace);
    turn.host
        .capture_shell(&id, key, command, None, Some(None))
        .await;
}

async fn shell_reported(turn: &Turn, item: &str, command: &str) {
    for status in [ItemStatus::InProgress, ItemStatus::Completed] {
        turn.emit(ProviderEvent::Command {
            kept: None,
            item_id: item.into(),
            command: command.into(),
            cwd: None,
            status,
            exit_code: (status == ItemStatus::Completed).then_some(0),
            output: None,
            duration_ms: None,
        })
        .await;
    }
}

/// A file long enough that git finds it again where it moved with a line more.
const MOVED: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n";

fn append(path: &Path, text: &str) {
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    std::io::Write::write_all(&mut file, text.as_bytes()).unwrap();
}

/// A shell command's change is the thread's when the files its command names were read around
/// it, the change starts from the file as the request last knew it and the file still holds
/// what the command left: Undo and Reapply take it, and it shows as an edit row after the
/// command's. A save of the user's just before or after the command, a file the command
/// doesn't name, and a move one of whose ends is held stay as they are (#87).
#[tokio::test]
async fn request_changes_own_a_shell_edit_by_its_files_read_around_it() {
    let flow = Flow::start(
        "20261017-0213-issues-run-w24-shell-edits",
        Options {
            thread: ProviderKind::Claude,
            files: vec![
                ("a.txt", "base\n"),
                ("b.txt", "base\n"),
                ("other.txt", "base\n"),
                ("m.txt", MOVED),
                ("p.txt", "base\n"),
            ],
            ..Default::default()
        },
        script(|turn| async move {
            // Its start reported only once the command wrote: the reads around it decide.
            let append_a = "echo 'thread a' >> a.txt";
            shell(&turn, "bash-1", append_a, |dir| {
                append(&dir.join("a.txt"), "thread a\n");
            })
            .await;
            shell_reported(&turn, "bash-1", append_a).await;
            observed(&turn, 1).await;
            // The user saved b.txt just before the command.
            let id = turn.conversation().unwrap();
            let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
            std::fs::write(workspace.join("b.txt"), "user b\n").unwrap();
            let edit_b = "perl -pi -e 's/user/thread/' b.txt";
            shell(&turn, "bash-2", edit_b, |dir| {
                std::fs::write(dir.join("b.txt"), "thread b\n").unwrap();
            })
            .await;
            shell_reported(&turn, "bash-2", edit_b).await;
            observed(&turn, 2).await;
            // The user saved other.txt once the command ended, before its snapshot.
            let edit_other = "sed -i '' s/base/thread/ other.txt";
            shell(&turn, "bash-3", edit_other, |dir| {
                std::fs::write(dir.join("other.txt"), "thread\n").unwrap();
            })
            .await;
            std::fs::write(workspace.join("other.txt"), "user other\n").unwrap();
            shell_reported(&turn, "bash-3", edit_other).await;
            observed(&turn, 3).await;
            // A file the command doesn't name.
            shell(&turn, "bash-4", "make", |dir| {
                std::fs::write(dir.join("generated.txt"), "made\n").unwrap();
            })
            .await;
            shell_reported(&turn, "bash-4", "make").await;
            observed(&turn, 4).await;
            // A move with an edit: both ends are the thread's.
            let move_m = "printf 'more\\n' >> m.txt && mv m.txt n.txt";
            shell(&turn, "bash-5", move_m, |dir| {
                append(&dir.join("m.txt"), "more\n");
                std::fs::rename(dir.join("m.txt"), dir.join("n.txt")).unwrap();
            })
            .await;
            shell_reported(&turn, "bash-5", move_m).await;
            // One file, renamed.
            observed(&turn, 5).await;
            // A move of a file the user had just changed: neither end is the thread's.
            std::fs::write(workspace.join("p.txt"), "user p\n").unwrap();
            let move_p = "mv p.txt q.txt";
            shell(&turn, "bash-6", move_p, |dir| {
                std::fs::rename(dir.join("p.txt"), dir.join("q.txt")).unwrap();
            })
            .await;
            shell_reported(&turn, "bash-6", move_p).await;
            observed(&turn, 6).await;
            Reply::text("Changed the files through the shell.")
        }),
    )
    .await;
    flow.say("Change the files with shell commands.").await;
    let board = flow
        .until("settled changes", |board| {
            board
                .latest_request()
                .and_then(|r| r.changes.as_ref())
                .is_some_and(|c| c.settled)
        })
        .await;
    let request = board.latest_request().unwrap().id.clone();
    let changes = board.latest_request().unwrap().changes.as_ref().unwrap();
    let held: Vec<_> = changes.held.iter().map(|held| held.path.as_str()).collect();
    assert_eq!(
        held,
        ["b.txt", "generated.txt", "other.txt", "p.txt", "q.txt"]
    );
    let reason = |path: &str| {
        changes
            .held
            .iter()
            .find(|held| held.path == path)
            .unwrap()
            .reason
            .clone()
    };
    assert!(
        reason("b.txt").starts_with("You changed b.txt"),
        "{}",
        reason("b.txt")
    );
    assert!(reason("other.txt").starts_with("A command changed other.txt"));
    assert!(reason("q.txt").contains("p.txt"), "{}", reason("q.txt"));
    // The owned commands' files show as edit rows after them; the held ones' don't.
    let files = |item: &str| {
        board
            .orchestrator_steps
            .iter()
            .find_map(|step| match &step.kind {
                OrchestratorStepKind::Tool { item_id, files, .. } if item_id == item => {
                    files.clone()
                }
                _ => None,
            })
    };
    let a = files("bash-1/edits").expect("a.txt's edit row");
    assert_eq!(a.len(), 1);
    assert!(a[0].path.ends_with("a.txt"));
    assert_eq!((a[0].insertions, a[0].deletions), (1, 0));
    let moved = files("bash-5/edits").expect("the move's edit row");
    assert_eq!(moved.len(), 2);
    for item in [
        "bash-2/edits",
        "bash-3/edits",
        "bash-4/edits",
        "bash-6/edits",
    ] {
        assert!(files(item).is_none(), "{item}");
    }
    let (workspace, _) = flow.manager.changes_checkout(&flow.conversation).unwrap();
    let read = |path: &str| std::fs::read_to_string(workspace.join(path)).ok();
    let outcome = flow
        .manager
        .undo_request(flow.conversation.clone(), request.clone(), false)
        .await
        .unwrap();
    assert_eq!(outcome.restored, ["a.txt", "m.txt", "n.txt"]);
    assert_eq!(read("a.txt").as_deref(), Some("base\n"));
    assert_eq!(read("m.txt").as_deref(), Some(MOVED));
    assert_eq!(read("n.txt"), None);
    assert_eq!(read("b.txt").as_deref(), Some("thread b\n"));
    assert_eq!(read("other.txt").as_deref(), Some("user other\n"));
    assert_eq!(read("generated.txt").as_deref(), Some("made\n"));
    assert_eq!(read("p.txt"), None);
    assert_eq!(read("q.txt").as_deref(), Some("user p\n"));
    let outcome = flow
        .manager
        .undo_request(flow.conversation.clone(), request, true)
        .await
        .unwrap();
    assert_eq!(outcome.restored, ["a.txt", "m.txt", "n.txt"]);
    assert_eq!(read("a.txt").as_deref(), Some("base\nthread a\n"));
    assert_eq!(read("m.txt"), None);
    assert_eq!(read("n.txt"), Some(format!("{MOVED}more\n")));
    flow.stop().await;
}

/// Brigadier's own `run` reads the files its command names right before it starts and once it
/// ended, so its change is the thread's, in either provider's thread, with its edit row (#87).
#[tokio::test]
async fn request_changes_own_a_run_commands_edit() {
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        let flow = Flow::start(
            "20261017-0213-issues-run-w24-run-edits",
            options(provider),
            script(move |turn| async move {
                let id = turn.conversation().unwrap();
                let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
                let command = format!(
                    "printf 'thread a\\n' >> '{}'",
                    workspace.join("a.txt").display()
                );
                let name = match provider {
                    ProviderKind::Claude => "mcp__brigadier__run",
                    _ => "brigadier/run",
                };
                let input = Some(serde_json::json!({ "command": command }).to_string());
                let call = |status, output| ProviderEvent::ToolCall {
                    kept: None,
                    item_id: "own-run".into(),
                    name: name.into(),
                    input: input.clone(),
                    status,
                    output,
                };
                turn.emit(call(ItemStatus::InProgress, None)).await;
                let reply = turn
                    .call("run", serde_json::json!({ "command": command }))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                turn.emit(call(ItemStatus::Completed, Some(reply.text)))
                    .await;
                observed(&turn, 1).await;
                Reply::text("Appended to a.txt.")
            }),
        )
        .await;
        flow.say("Append to a.txt.").await;
        let board = flow
            .until("settled changes", |board| {
                board
                    .latest_request()
                    .and_then(|r| r.changes.as_ref())
                    .is_some_and(|c| c.settled)
            })
            .await;
        let request = board.latest_request().unwrap();
        assert_eq!(request.changes.as_ref().unwrap().held, []);
        assert!(board.orchestrator_steps.iter().any(|step| matches!(
            &step.kind,
            OrchestratorStepKind::Tool { item_id, files: Some(files), .. }
                if item_id == "own-run/edits" && files.len() == 1
        )));
        let (workspace, _) = flow.manager.changes_checkout(&flow.conversation).unwrap();
        let outcome = flow
            .manager
            .undo_request(flow.conversation.clone(), request.id.clone(), false)
            .await
            .unwrap();
        assert_eq!(outcome.restored, ["a.txt"]);
        assert_eq!(
            std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
            "base\n"
        );
        flow.stop().await;
    }
}

/// A move's two ends go together: a quoted name is still the command's, the end of a move its
/// words don't name holds the other, and two moves in a row through a file no snapshot saw
/// hold each other's files, so Undo never takes the only copy of a file away (#87).
#[tokio::test]
async fn request_changes_never_split_a_shell_move() {
    const GLOBBED: &str = "alpha\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\ntheta\n";
    let flow = Flow::start(
        "20261017-0213-issues-run-w27-shell-moves",
        Options {
            thread: ProviderKind::Claude,
            files: vec![
                ("old name.txt", MOVED),
                ("g1.txt", GLOBBED),
                ("c.txt", "base\n"),
            ],
            ..Default::default()
        },
        script(|turn| async move {
            let quoted = "mv 'old name.txt' new.txt";
            shell(&turn, "bash-1", quoted, |dir| {
                std::fs::rename(dir.join("old name.txt"), dir.join("new.txt")).unwrap();
            })
            .await;
            shell_reported(&turn, "bash-1", quoted).await;
            observed(&turn, 1).await;
            let globbed = "mv g*.txt h.txt";
            shell(&turn, "bash-2", globbed, |dir| {
                std::fs::rename(dir.join("g1.txt"), dir.join("h.txt")).unwrap();
            })
            .await;
            shell_reported(&turn, "bash-2", globbed).await;
            observed(&turn, 2).await;
            // The user saved c.txt just before two moves that one snapshot sees.
            let id = turn.conversation().unwrap();
            let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
            std::fs::write(workspace.join("c.txt"), format!("user\n{MOVED}")).unwrap();
            let (first, second) = ("mv c.txt d.txt", "mv d.txt e.txt");
            shell(&turn, "bash-3", first, |dir| {
                std::fs::rename(dir.join("c.txt"), dir.join("d.txt")).unwrap();
            })
            .await;
            shell(&turn, "bash-4", second, |dir| {
                std::fs::rename(dir.join("d.txt"), dir.join("e.txt")).unwrap();
            })
            .await;
            shell_reported(&turn, "bash-3", first).await;
            shell_reported(&turn, "bash-4", second).await;
            observed(&turn, 3).await;
            // A file the user made since the last snapshot, moved.
            let yours = format!("yours\n{GLOBBED}");
            std::fs::write(workspace.join("user-new.txt"), &yours).unwrap();
            let move_new = "mv user-new.txt moved.txt";
            shell(&turn, "bash-5", move_new, |dir| {
                std::fs::rename(dir.join("user-new.txt"), dir.join("moved.txt")).unwrap();
            })
            .await;
            shell_reported(&turn, "bash-5", move_new).await;
            observed(&turn, 4).await;
            Reply::text("Moved the files.")
        }),
    )
    .await;
    flow.say("Move the files.").await;
    let board = flow
        .until("settled changes", |board| {
            board
                .latest_request()
                .and_then(|r| r.changes.as_ref())
                .is_some_and(|c| c.settled)
        })
        .await;
    let request = board.latest_request().unwrap().id.clone();
    let changes = board.latest_request().unwrap().changes.as_ref().unwrap();
    let held: Vec<_> = changes.held.iter().map(|held| held.path.as_str()).collect();
    assert_eq!(
        held,
        [
            "c.txt",
            "e.txt",
            "g1.txt",
            "h.txt",
            "moved.txt",
            "user-new.txt"
        ]
    );
    let (workspace, _) = flow.manager.changes_checkout(&flow.conversation).unwrap();
    let read = |path: &str| std::fs::read_to_string(workspace.join(path)).ok();
    let outcome = flow
        .manager
        .undo_request(flow.conversation.clone(), request, false)
        .await
        .unwrap();
    assert_eq!(outcome.restored, ["new.txt", "old name.txt"]);
    assert_eq!(read("old name.txt").as_deref(), Some(MOVED));
    assert_eq!(read("new.txt"), None);
    assert_eq!(read("h.txt").as_deref(), Some(GLOBBED));
    assert_eq!(read("e.txt"), Some(format!("user\n{MOVED}")));
    assert_eq!(read("moved.txt"), Some(format!("yours\n{GLOBBED}")));
    flow.stop().await;
}
