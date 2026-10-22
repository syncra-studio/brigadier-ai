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
    tokio::time::timeout(*super::PATIENCE, async {
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

/// Waits until the event loop took the turn's edits so far: queued for their snapshot, or in
/// one that counts `files`. A CLI reports an edit well before its next call reaches Brigadier;
/// a scripted turn sends both at once, so the call could overtake the report (#185).
async fn taken(turn: &Turn, files: usize) {
    let id = turn.conversation().unwrap();
    tokio::time::timeout(*super::PATIENCE, async {
        while !turn.host.change_records.pending(&id)
            && !turn
                .host
                .core
                .board(&id)
                .await
                .unwrap()
                .latest_request()
                .and_then(|r| r.changes.as_ref())
                .is_some_and(|c| c.stat.files.len() == files)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
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
                tokio::time::timeout(*super::PATIENCE, async {
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
            "a.txt also changed during this request in a way Brigadier didn't see (by you, or by one of the thread's own tools), so Undo leaves it as it is."
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
                tokio::time::timeout(*super::PATIENCE, async {
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
                tokio::time::timeout(*super::PATIENCE, async {
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
            Reply::text("")
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
        let cwd = flow.manager.thread_cwd(&flow.conversation);
        std::fs::remove_file(cwd.join("generated.png")).unwrap();
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
        std::fs::remove_file(cwd.join("viewed.png")).unwrap();
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
                tokio::time::timeout(*super::PATIENCE, async {
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
                Reply::text("")
            }
        })).await;
        flow.background_commands();
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
            tokio::time::timeout(*super::PATIENCE, async {
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
                Reply::text("")
            }
        }),
    )
    .await;
    flow.background_commands();
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
            tokio::time::timeout(*super::PATIENCE, async {
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
                return Reply::text("");
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

/// Two delegations at once on a checkout with the user's changes open one question, and the
/// one answer starts both workers (#179).
#[tokio::test]
async fn two_delegations_at_once_open_one_uncommitted_changes_question() {
    let mut flow = Flow::start(
        "20261017-0213-issues-run-w49-one-card",
        options(ProviderKind::Claude),
        script(move |turn| async move {
            if !turn.is_orchestrator() {
                std::future::pending::<()>().await;
                return Reply::text("");
            }
            if turn.input.contains("Scout twice.") {
                let delegate = |title: &str| {
                    turn.call(
                        "delegate_task",
                        serde_json::json!({"effort": "high", "title": title, "kind": "scout", "spec": "Look around."}),
                    )
                };
                let (one, two) = tokio::join!(delegate("Look here"), delegate("Look there"));
                assert!(!one.is_error, "{}", one.text);
                assert!(!two.is_error, "{}", two.text);
                return Reply::text("");
            }
            Reply::text("Fine.")
        }),
    )
    .await;
    local(&mut flow).await;
    std::fs::write(flow.repo.join("a.txt"), "the user's\n").unwrap();
    flow.say("Scout twice.").await;
    let asked = |board: &Board| {
        board
            .questions
            .values()
            .filter(|question| {
                matches!(
                    question.kind,
                    crate::work::QuestionKind::UncommittedChanges { .. }
                )
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    let board = flow
        .until("both tasks and the question", |board| {
            board.tasks.len() == 2 && !asked(board).is_empty()
        })
        .await;
    let card = asked(&board).remove(0);
    flow.manager
        .answer_question(
            flow.conversation.clone(),
            card.id.clone(),
            vec!["No, use the last commit".into()],
            None,
        )
        .await
        .unwrap();
    // Both start on the one answer; a second card would hold its task back.
    let board = flow
        .until("both workers running, or a second question", |board| {
            asked(board).len() > 1
                || board
                    .tasks
                    .values()
                    .all(|task| task.state == crate::work::TaskState::Running)
        })
        .await;
    assert_eq!(asked(&board).len(), 1, "{:#?}", asked(&board));
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
                taken(&turn, 1).await;
                let reply = turn
                    .call(
                        "delegate_task",
                        serde_json::json!({"effort": "high", "title": "Look", "kind": "scout", "spec": "Look around."}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("");
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
        reason("b.txt")
            .starts_with("b.txt also changed during this request in a way Brigadier didn't see"),
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

/// A move's two ends go together: a quoted name is still the command's, a move whose names the
/// shell makes stays the user's, two moves in a row through a file no snapshot saw hold each
/// other's files, and so does a move of a file of the user's no snapshot keeps, so Undo never
/// takes the only copy of a file away (#87).
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
                (".gitignore", "*.tmp\n"),
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
            // An ignored file of the user's, moved where the checkout keeps it.
            std::fs::write(workspace.join("scratch.tmp"), "scratch\n").unwrap();
            let move_ignored = "mv scratch.tmp result.txt";
            shell(&turn, "bash-6", move_ignored, |dir| {
                std::fs::rename(dir.join("scratch.tmp"), dir.join("result.txt")).unwrap();
            })
            .await;
            shell_reported(&turn, "bash-6", move_ignored).await;
            observed(&turn, 5).await;
            // An ignored file of the user's, moved twice, the second time where the checkout
            // keeps it.
            std::fs::write(workspace.join("first.tmp"), "first\n").unwrap();
            let (to_middle, to_kept) = ("mv first.tmp middle.tmp", "mv middle.tmp kept.txt");
            shell(&turn, "bash-7", to_middle, |dir| {
                std::fs::rename(dir.join("first.tmp"), dir.join("middle.tmp")).unwrap();
            })
            .await;
            shell_reported(&turn, "bash-7", to_middle).await;
            shell(&turn, "bash-8", to_kept, |dir| {
                std::fs::rename(dir.join("middle.tmp"), dir.join("kept.txt")).unwrap();
            })
            .await;
            shell_reported(&turn, "bash-8", to_kept).await;
            observed(&turn, 6).await;
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
            "kept.txt",
            "moved.txt",
            "result.txt",
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
    assert_eq!(read("result.txt").as_deref(), Some("scratch\n"));
    assert_eq!(read("kept.txt").as_deref(), Some("first\n"));
    flow.stop().await;
}

const LINES: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n";
const EDITED: &str = "ONE\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\n";

fn review_options() -> Options {
    Options {
        thread: ProviderKind::Claude,
        files: vec![("a.txt", LINES), ("b.txt", "base\n")],
        ..Default::default()
    }
}

/// One request's edits to `a.txt` (two runs of lines) and `b.txt`, settled.
async fn reviewed(flow: &Flow) -> Board {
    flow.say("Edit a and b.").await;
    flow.until("its changes in review", |board| {
        board
            .latest_request()
            .is_some_and(|request| request.review.len() == 2)
    })
    .await
}

fn edit_a_and_b() -> Script {
    script(move |turn| async move {
        let id = turn.conversation().unwrap();
        let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
        edit(&turn, &workspace.join("a.txt"), "edit-a", LINES, EDITED).await;
        observed(&turn, 1).await;
        edit(
            &turn,
            &workspace.join("b.txt"),
            "edit-b",
            "base\n",
            "thread b\n",
        )
        .await;
        observed(&turn, 2).await;
        Reply::text("Changed a and b.")
    })
}

/// Keep one change and the other stays to review; Undo puts back only its own lines; a whole
/// file's Undo puts it back as it was; each card row says where the review left it, and the
/// user's checkout keeps the restores uncommitted.
#[tokio::test]
async fn pending_changes_keep_and_undo_change_by_change_on_the_users_checkout() {
    use crate::work::FileReview;
    let mut flow = Flow::start(
        "20261017-0213-issues-run-w41-pending-local",
        review_options(),
        edit_a_and_b(),
    )
    .await;
    local(&mut flow).await;
    let board = reviewed(&flow).await;
    let request = board.latest_request().unwrap().id.clone();
    let review = |board: &Board| board.requests[&request].review.clone();
    assert_eq!(
        review(&board).into_iter().collect::<Vec<_>>(),
        [
            ("a.txt".to_owned(), FileReview::Pending),
            ("b.txt".to_owned(), FileReview::Pending)
        ]
    );
    let head = git(&flow.repo, &["rev-parse", "HEAD"]);
    let id = flow.conversation.clone();
    let pending = flow
        .manager
        .pending_changes(id.clone(), None)
        .await
        .unwrap();
    assert_eq!(pending.files.len(), 2);
    let a = &pending.files[0];
    assert_eq!(
        (a.path.as_str(), a.request_id.as_str()),
        ("a.txt", request.as_str())
    );
    assert_eq!(a.base.as_deref(), Some(LINES));
    assert_eq!(a.current.as_deref(), Some(EDITED));
    assert_eq!((a.insertions, a.deletions, a.hunks.len()), (2, 1, 2));
    flow.manager
        .keep_changes(
            id.clone(),
            Some("a.txt".into()),
            Some(vec![a.hunks[0].id.clone()]),
        )
        .await
        .unwrap();
    let a = flow
        .manager
        .pending_changes(id.clone(), Some("a.txt".into()))
        .await
        .unwrap()
        .files
        .remove(0);
    assert_eq!(a.hunks.len(), 1);
    assert_eq!(
        a.base.as_deref(),
        Some("ONE\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n")
    );
    assert_eq!(
        std::fs::read_to_string(flow.repo.join("a.txt")).unwrap(),
        EDITED
    );
    let undone = flow
        .manager
        .undo_pending_changes(
            id.clone(),
            Some("a.txt".into()),
            Some(vec![a.hunks[0].id.clone()]),
        )
        .await
        .unwrap();
    assert_eq!(undone.restored, ["a.txt"]);
    assert_eq!(
        std::fs::read_to_string(flow.repo.join("a.txt")).unwrap(),
        "ONE\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n"
    );
    flow.manager
        .undo_pending_changes(id.clone(), Some("b.txt".into()), None)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(flow.repo.join("b.txt")).unwrap(),
        "base\n"
    );
    let board = flow
        .until("both files reviewed", |board| {
            !board.requests[&request]
                .review
                .values()
                .any(|state| *state == FileReview::Pending)
        })
        .await;
    assert_eq!(review(&board)["a.txt"], FileReview::Kept);
    assert_eq!(review(&board)["b.txt"], FileReview::Undone);
    assert!(
        flow.manager
            .pending_changes(id.clone(), None)
            .await
            .unwrap()
            .files
            .is_empty()
    );
    assert_eq!(git(&flow.repo, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        git(
            &flow.repo,
            &["status", "--porcelain", "--", "a.txt", "b.txt"]
        )
        .trim(),
        "M a.txt"
    );
    flow.stop().await;
}

/// A change named from before the user edited its lines is refused, and the file stays as the
/// user left it.
#[tokio::test]
async fn pending_changes_refuse_a_change_the_file_no_longer_has() {
    let mut flow = Flow::start(
        "20261017-0213-issues-run-w41-pending-refused",
        review_options(),
        edit_a_and_b(),
    )
    .await;
    local(&mut flow).await;
    reviewed(&flow).await;
    let id = flow.conversation.clone();
    let a = flow
        .manager
        .pending_changes(id.clone(), Some("a.txt".into()))
        .await
        .unwrap()
        .files
        .remove(0);
    let mine = "MINE\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\n";
    std::fs::write(flow.repo.join("a.txt"), mine).unwrap();
    let error = flow
        .manager
        .undo_pending_changes(
            id.clone(),
            Some("a.txt".into()),
            Some(vec![a.hunks[0].id.clone()]),
        )
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "The file changed since; review it again");
    assert_eq!(
        std::fs::read_to_string(flow.repo.join("a.txt")).unwrap(),
        mine
    );
    flow.stop().await;
}

/// In a worktree, Undo of a change the thread committed lands as a commit; a file a later
/// request changed again moves to that request's card, and Keep All keeps it for both.
#[tokio::test]
async fn pending_changes_undo_a_committed_change_by_a_commit_and_pass_files_on() {
    use crate::work::FileReview;
    let flow = Flow::start(
        "20261017-0213-issues-run-w41-pending-worktree",
        review_options(),
        script(move |turn| async move {
            let id = turn.conversation().unwrap();
            let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
            if turn.input.contains("again") {
                edit(
                    &turn,
                    &workspace.join("b.txt"),
                    "edit-b2",
                    "thread b\n",
                    "thread b\nagain\n",
                )
                .await;
                observed(&turn, 1).await;
                return Reply::text("Changed b again.");
            }
            edit(&turn, &workspace.join("a.txt"), "edit-a", LINES, EDITED).await;
            observed(&turn, 1).await;
            edit(
                &turn,
                &workspace.join("b.txt"),
                "edit-b",
                "base\n",
                "thread b\n",
            )
            .await;
            observed(&turn, 2).await;
            git(&workspace, &["add", "a.txt", "b.txt"]);
            git(&workspace, &["commit", "-q", "-m", "Thread edits"]);
            Reply::text("Changed a and b.")
        }),
    )
    .await;
    let board = reviewed(&flow).await;
    let first = board.latest_request().unwrap().id.clone();
    let id = flow.conversation.clone();
    let (workspace, _) = flow.manager.changes_checkout(&id).unwrap();
    let head = git(&workspace, &["rev-parse", "HEAD"]);
    flow.manager
        .undo_pending_changes(id.clone(), Some("a.txt".into()), None)
        .await
        .unwrap();
    assert_ne!(git(&workspace, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        git(&workspace, &["log", "-1", "--format=%s"]),
        "Undo changes in a.txt"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
        LINES
    );
    assert!(git(&workspace, &["status", "--porcelain"]).is_empty());

    flow.say("Edit b again.").await;
    let board = flow
        .until("b on the later card", |board| {
            board
                .latest_request()
                .is_some_and(|request| request.id != first && !request.review.is_empty())
        })
        .await;
    let second = board.latest_request().unwrap().id.clone();
    assert_eq!(board.requests[&first].review["b.txt"], FileReview::Later);
    assert_eq!(board.requests[&second].review["b.txt"], FileReview::Pending);
    let pending = flow
        .manager
        .pending_changes(id.clone(), None)
        .await
        .unwrap();
    assert_eq!(pending.files.len(), 1);
    assert_eq!(pending.files[0].request_id, second);
    assert_eq!(pending.files[0].base.as_deref(), Some("base\n"));
    flow.manager
        .keep_changes(id.clone(), None, None)
        .await
        .unwrap();
    let board = flow
        .until("b kept on both cards", |board| {
            board.requests[&second].review["b.txt"] == FileReview::Kept
        })
        .await;
    assert_eq!(board.requests[&first].review["b.txt"], FileReview::Kept);
    assert_eq!(board.requests[&first].review["a.txt"], FileReview::Undone);
    assert_eq!(
        std::fs::read_to_string(workspace.join("b.txt")).unwrap(),
        "thread b\nagain\n"
    );
    flow.stop().await;
}

/// A file the user edited after the agents keeps those edits from a whole file's Undo (refused
/// for the file, left as it is by Undo All), while its changes still undo one at a time; a base
/// Keep made of some changes outlasts Git's garbage collection.
#[tokio::test]
async fn pending_changes_leave_the_users_later_edits_and_outlast_gc() {
    let mut flow = Flow::start(
        "20261017-0213-issues-run-w41-pending-edited",
        review_options(),
        edit_a_and_b(),
    )
    .await;
    local(&mut flow).await;
    reviewed(&flow).await;
    let id = flow.conversation.clone();
    let a = flow
        .manager
        .pending_changes(id.clone(), Some("a.txt".into()))
        .await
        .unwrap()
        .files
        .remove(0);
    flow.manager
        .keep_changes(
            id.clone(),
            Some("a.txt".into()),
            Some(vec![a.hunks[0].id.clone()]),
        )
        .await
        .unwrap();
    git(&flow.repo, &["gc", "-q", "--prune=now"]);
    let mine = format!("{EDITED}mine\n");
    std::fs::write(flow.repo.join("a.txt"), &mine).unwrap();
    let error = flow
        .manager
        .undo_pending_changes(id.clone(), Some("a.txt".into()), None)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "You changed a.txt after the agents did, so undo its changes one at a time."
    );
    let all = flow
        .manager
        .undo_pending_changes(id.clone(), None, None)
        .await
        .unwrap();
    assert_eq!(all.restored, ["b.txt"]);
    assert_eq!(
        all.held
            .iter()
            .map(|held| held.path.as_str())
            .collect::<Vec<_>>(),
        ["a.txt"]
    );
    assert_eq!(
        std::fs::read_to_string(flow.repo.join("a.txt")).unwrap(),
        mine
    );
    let a = flow
        .manager
        .pending_changes(id.clone(), Some("a.txt".into()))
        .await
        .unwrap()
        .files
        .remove(0);
    assert_eq!(a.hunks.len(), 1);
    flow.manager
        .undo_pending_changes(
            id.clone(),
            Some("a.txt".into()),
            Some(vec![a.hunks[0].id.clone()]),
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(flow.repo.join("a.txt")).unwrap(),
        "ONE\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n"
    );
    flow.stop().await;
}

/// #233: the thread merges the base into its branch with a command. What the merge brings from
/// the other side is that side's work, not the request's: the request counts its own files
/// only: a file of its own it deleted alongside the merge, and a topic branch it merged.
#[tokio::test]
async fn a_merge_of_the_base_counts_none_of_its_files_as_the_requests() {
    let flow = Flow::start(
        "request-changes-merge-in",
        options(ProviderKind::Claude),
        script(|turn| async move {
            if !turn.input.contains("Edit a and merge the base in.") || turn.earlier > 1 {
                return Reply::text("Finished.");
            }
            let id = turn.conversation().unwrap();
            let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
            edit(
                &turn,
                &workspace.join("a.txt"),
                "edit-a",
                "base\n",
                "thread a\n",
            )
            .await;
            observed(&turn, 1).await;
            git(&workspace, &["commit", "-q", "-am", "Thread edit"]);
            // A file the other side never had, deleted in the same command as the merge.
            git(&workspace, &["rm", "-q", "own.txt"]);
            git(&workspace, &["commit", "-q", "-m", "Drop own"]);
            // A topic branch of its own, merged in: its file is the request's.
            let session = git(&workspace, &["branch", "--show-current"]);
            git(&workspace, &["checkout", "-q", "-b", "topic"]);
            std::fs::write(workspace.join("topic.txt"), "topic\n").unwrap();
            git(&workspace, &["add", "topic.txt"]);
            git(&workspace, &["commit", "-q", "-m", "Topic"]);
            git(&workspace, &["checkout", "-q", &session]);
            git(
                &workspace,
                &["merge", "-q", "--no-ff", "--no-edit", "topic"],
            );
            git(&workspace, &["merge", "-q", "--no-edit", "upstream"]);
            turn.emit(ProviderEvent::Command {
                kept: None,
                item_id: "merge".into(),
                command: "git merge upstream".into(),
                cwd: None,
                status: ItemStatus::Completed,
                exit_code: Some(0),
                output: Some("Merge made".into()),
                duration_ms: None,
            })
            .await;
            Reply::text("Merged the base in.")
        }),
    )
    .await;
    // The base moves on with three files of its own.
    let main = git(&flow.repo, &["rev-parse", "HEAD"]);
    git(&flow.repo, &["branch", "upstream", &main]);
    let side = flow.repo.join("../request-changes-merge-in-side");
    git(
        &flow.repo,
        &["worktree", "add", "-q", side.to_str().unwrap(), "upstream"],
    );
    for n in 1..=3 {
        std::fs::write(side.join(format!("base-{n}.txt")), "base work\n").unwrap();
    }
    git(&side, &["add", "-A"]);
    git(&side, &["commit", "-q", "-m", "Base work"]);
    flow.say("Get ready.").await;
    flow.settled().await;
    let (workspace, _) = flow.manager.changes_checkout(&flow.conversation).unwrap();
    std::fs::write(workspace.join("own.txt"), "session only\n").unwrap();
    git(&workspace, &["add", "own.txt"]);
    git(&workspace, &["commit", "-q", "-m", "Own file"]);
    // `upstream` stands in for the base's upstream (GitHub's main).
    git(
        &flow.repo,
        &["branch", "--set-upstream-to=upstream", "main"],
    );
    flow.say("Edit a and merge the base in.").await;
    let board = flow
        .until("the settled record", |board| {
            board
                .latest_request()
                .and_then(|r| r.changes.as_ref())
                .is_some_and(|c| c.settled)
        })
        .await;
    let changes = board.latest_request().unwrap().changes.clone().unwrap();
    let files: Vec<&str> = changes.stat.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(files, ["a.txt", "own.txt", "topic.txt"], "{changes:?}");
    git(
        &flow.repo,
        &["worktree", "remove", "--force", side.to_str().unwrap()],
    );
    flow.stop().await;
}

/// The thread's merge script: on "Edit a and b." it edits both and commits them (only `a.txt`
/// when the words say "commit only a"); on "[merge]" it merges; on "...and merge it" it merges
/// in the same turn, then puts `b.txt` back as it was.
fn edit_and_merge() -> Script {
    script(move |turn| async move {
        let id = turn.conversation().unwrap();
        let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
        if turn.input.contains("[merge] The user clicked") {
            turn.call("run_checks", serde_json::json!({})).await;
            let reply = turn.call("finish_session", serde_json::json!({})).await;
            assert!(!reply.is_error, "{}", reply.text);
            return Reply::text("Merged.");
        }
        edit(&turn, &workspace.join("a.txt"), "edit-a", LINES, EDITED).await;
        observed(&turn, 1).await;
        edit(
            &turn,
            &workspace.join("b.txt"),
            "edit-b",
            "base\n",
            "thread b\n",
        )
        .await;
        observed(&turn, 2).await;
        if turn.input.contains("commit only a") {
            git(&workspace, &["add", "a.txt"]);
        } else {
            git(&workspace, &["add", "a.txt", "b.txt"]);
        }
        git(&workspace, &["commit", "-q", "-m", "Thread edits"]);
        if turn.input.contains("merge it") {
            turn.call("run_checks", serde_json::json!({})).await;
            let reply = turn
                .call(
                    "finish_session",
                    serde_json::json!({ "user_words": "merge it" }),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            edit(
                &turn,
                &workspace.join("b.txt"),
                "edit-b2",
                "thread b\n",
                "base\n",
            )
            .await;
            taken(&turn, 1).await;
        }
        Reply::text("Changed a and b.")
    })
}

fn merged(board: &Board) -> bool {
    board
        .orchestrator_steps
        .iter()
        .any(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
}

/// A session whose thread ran `words` and whose user then clicked Merge, once merged and its
/// review settled as `done` says.
async fn merged_after(name: &str, words: &str, done: impl Fn(&Board) -> bool) -> (Flow, Board) {
    let flow = Flow::start(name, review_options(), edit_and_merge()).await;
    flow.say(words).await;
    flow.until("its changes in review", |board| {
        board
            .latest_request()
            .is_some_and(|request| request.review.len() == 2)
    })
    .await;
    flow.checks_passed().await;
    flow.manager
        .merge_session(&flow.conversation, super::click(), false)
        .await
        .unwrap();
    flow.until("the merge", merged).await;
    let board = flow.until("its review after the merge", done).await;
    (flow, board)
}

/// #288: a merge takes its requests' changes out of the review. Merged after they settled,
/// each file's row says Merged and nothing waits for Undo or Keep; a file the merge didn't
/// take (left uncommitted) still waits, until a later merge takes it. Merged in the turn that made them, the same, and what
/// the turn changed after the merge waits, from the merged file, even a file put back as it
/// was.
#[tokio::test]
async fn pending_changes_leave_the_review_once_merged() {
    use crate::work::FileReview;
    let review = |board: &Board| {
        let request = board.latest_request().unwrap();
        request.review.clone().into_iter().collect::<Vec<_>>()
    };
    let (flow, board) = merged_after(
        "20261018-1945-issues-run-2-w07-merged-review",
        "Edit a and b.",
        |board| {
            board.latest_request().is_some_and(|request| {
                request
                    .review
                    .values()
                    .all(|state| *state == FileReview::Merged)
            })
        },
    )
    .await;
    assert_eq!(review(&board).len(), 2);
    assert!(
        flow.manager
            .pending_changes(flow.conversation.clone(), None)
            .await
            .unwrap()
            .files
            .is_empty()
    );
    flow.stop().await;

    let (flow, board) = merged_after(
        "20261018-1945-issues-run-2-w07-merged-uncommitted",
        "Edit a and b, commit only a.",
        |board| {
            board
                .latest_request()
                .is_some_and(|request| request.review.get("a.txt") == Some(&FileReview::Merged))
        },
    )
    .await;
    assert_eq!(
        review(&board),
        [
            ("a.txt".to_owned(), FileReview::Merged),
            ("b.txt".to_owned(), FileReview::Pending)
        ]
    );
    let pending = flow
        .manager
        .pending_changes(flow.conversation.clone(), None)
        .await
        .unwrap();
    assert_eq!(pending.files.len(), 1);
    assert_eq!(pending.files[0].path, "b.txt");
    assert_eq!(pending.files[0].base.as_deref(), Some("base\n"));
    assert_eq!(pending.files[0].current.as_deref(), Some("thread b\n"));
    // Committed since and merged again, it is merged too, though no request of this merge
    // changed it.
    let (workspace, _) = flow.manager.changes_checkout(&flow.conversation).unwrap();
    git(&workspace, &["commit", "-q", "-am", "The rest"]);
    let first = board.latest_request().unwrap().id.clone();
    flow.checks_passed().await;
    flow.manager
        .merge_session(&flow.conversation, super::click(), false)
        .await
        .unwrap();
    flow.until("b merged", |board| {
        board.requests[&first].review.get("b.txt") == Some(&FileReview::Merged)
    })
    .await;
    assert!(
        flow.manager
            .pending_changes(flow.conversation.clone(), None)
            .await
            .unwrap()
            .files
            .is_empty()
    );
    flow.stop().await;

    let flow = Flow::start(
        "20261018-1945-issues-run-2-w07-merged-in-turn",
        review_options(),
        edit_and_merge(),
    )
    .await;
    flow.say("Edit a and b and merge it.").await;
    let board = flow
        .until("its review after the merge", |board| {
            merged(board)
                && board
                    .latest_request()
                    .is_some_and(|request| request.review.len() == 2)
        })
        .await;
    assert_eq!(
        review(&board),
        [
            ("a.txt".to_owned(), FileReview::Merged),
            ("b.txt".to_owned(), FileReview::Pending)
        ]
    );
    let pending = flow
        .manager
        .pending_changes(flow.conversation.clone(), None)
        .await
        .unwrap();
    assert_eq!(pending.files.len(), 1);
    assert_eq!(pending.files[0].path, "b.txt");
    assert_eq!(pending.files[0].base.as_deref(), Some("thread b\n"));
    assert_eq!(pending.files[0].current.as_deref(), Some("base\n"));
    flow.stop().await;
}

/// A fast-forward merge's View changes shows all its commits, not only the last (#411).
#[tokio::test]
async fn a_fast_forward_merges_change_spans_its_commits() {
    let flow = Flow::start(
        "20261019-2108-workspaces-rework-w102-ff",
        Options::default(),
        script(|_turn| async move { Reply::text("Hi.") }),
    )
    .await;
    flow.say("Hi.").await;
    for (file, text) in [("one.txt", "one\n"), ("two.txt", "two\n")] {
        std::fs::write(flow.repo.join(file), text).unwrap();
        git(&flow.repo, &["add", file]);
        git(&flow.repo, &["commit", "-qm", file]);
    }
    let head = git(&flow.repo, &["rev-parse", "HEAD"]).trim().to_owned();
    let diff = |commits| {
        let (manager, conversation, commit) = (&flow.manager, &flow.conversation, head.clone());
        async move {
            manager
                .review_diff(
                    conversation,
                    ReviewScope::Commit { commit, commits },
                    false,
                    false,
                )
                .await
                .unwrap()
        }
    };
    let both = diff(Some(2)).await;
    let paths: Vec<&str> = both.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["one.txt", "two.txt"]);
    let last = diff(None).await;
    let paths: Vec<&str> = last.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["two.txt"]);
    flow.stop().await;
}

/// Writes `count` files under `dir` in the checkout, as a build or a scratch copy does.
fn scratch_folder(workspace: &Path, dir: &str, count: usize) {
    let dir = workspace.join(dir);
    std::fs::create_dir_all(dir.join("deps")).unwrap();
    for n in 0..count {
        std::fs::write(dir.join(format!("deps/f{n}.txt")), format!("built {n}\n")).unwrap();
    }
}

/// The thread's own scratch and build folders are no change to the project (#492): one it
/// makes and removes within a turn, a gitignored one and one tagged as a cache never count; an
/// untagged one it leaves counts in that turn, and the later turn that removes it shows only
/// its own edit, on the card, in its review and in Last Turn.
#[tokio::test]
async fn a_removed_scratch_folder_of_the_threads_is_no_change() {
    let flow = Flow::start(
        "20261021-1551-changes-card-492-w01-scratch",
        Options {
            thread: ProviderKind::Claude,
            files: vec![
                ("a.txt", "base\n"),
                ("b.txt", "base\n"),
                (".gitignore", "build/\n"),
            ],
            ..Default::default()
        },
        script(|turn| async move {
            let id = turn.conversation().unwrap();
            let (workspace, _) = turn.host.changes_checkout(&id).unwrap();
            if turn.input.contains("Build") {
                edit(
                    &turn,
                    &workspace.join("a.txt"),
                    "edit-a",
                    "base\n",
                    "thread a\n",
                )
                .await;
                observed(&turn, 1).await;
                scratch_folder(&workspace, "scratch", 40);
                scratch_folder(&workspace, "target-base", 40);
                std::fs::write(
                    workspace.join("target-base/CACHEDIR.TAG"),
                    "Signature: 8a477f597d28d172789f06886806bc55\n# cargo\n",
                )
                .unwrap();
                scratch_folder(&workspace, "build", 10);
                scratch_folder(&workspace, "tmp-run", 10);
                shell_reported(&turn, "build", "cargo build").await;
                observed(&turn, 1 + 40 + 10).await;
                std::fs::remove_dir_all(workspace.join("tmp-run")).unwrap();
                shell_reported(&turn, "clean-tmp", "rm -rf tmp-run").await;
                observed(&turn, 1 + 40).await;
                Reply::text("Built it.")
            } else {
                for dir in ["scratch", "target-base", "build"] {
                    std::fs::remove_dir_all(workspace.join(dir)).unwrap();
                }
                shell_reported(&turn, "clean", "rm -rf scratch target-base build").await;
                edit(
                    &turn,
                    &workspace.join("b.txt"),
                    "edit-b",
                    "base\n",
                    "thread b\n",
                )
                .await;
                taken(&turn, 1).await;
                Reply::text("Cleaned up.")
            }
        }),
    )
    .await;
    let settled = |after: Option<String>| {
        move |board: &Board| {
            board.latest_request().is_some_and(|r| {
                Some(&r.id) != after.as_ref() && r.changes.as_ref().is_some_and(|c| c.settled)
            })
        }
    };
    flow.say("Build it.").await;
    let board = flow.until("the build's changes", settled(None)).await;
    let built = board.latest_request().unwrap();
    let paths: Vec<_> = built
        .changes
        .as_ref()
        .unwrap()
        .stat
        .files
        .iter()
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(paths.len(), 41, "{paths:?}");
    assert!(paths.contains(&"a.txt"));
    assert!(
        paths
            .iter()
            .all(|path| *path == "a.txt" || path.starts_with("scratch/")),
        "{paths:?}"
    );
    let built = built.id.clone();
    flow.say("Clean up and edit b.").await;
    let board = flow
        .until("the clean-up's changes", settled(Some(built)))
        .await;
    let cleaned = board.latest_request().unwrap();
    let paths: Vec<_> = cleaned
        .changes
        .as_ref()
        .unwrap()
        .stat
        .files
        .iter()
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(paths, ["b.txt"]);
    assert!(cleaned.changes.as_ref().unwrap().held.is_empty());
    let cleaned = cleaned.id.clone();
    let board = flow
        .until("the clean-up's review", |board| {
            board
                .requests
                .get(&cleaned)
                .is_some_and(|r| !r.review.is_empty())
        })
        .await;
    let review: Vec<_> = board.requests[&cleaned]
        .review
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(review, ["b.txt"]);
    let last = flow
        .manager
        .review_diff(
            &flow.conversation,
            ReviewScope::LastTurn {
                request_id: Some(cleaned),
            },
            false,
            false,
        )
        .await
        .unwrap();
    let files: Vec<_> = last.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(files, ["b.txt"]);
    assert_eq!((last.insertions, last.deletions), (1, 1));
    flow.stop().await;
}
