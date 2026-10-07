//! The session's thread (THREAD-PLAN.md Q1, Q6): its tools, workspace and access, approvals
//! under its permission level, and a restart for a new workspace or level.

use std::sync::{Arc, Mutex};

use brigadier_providers::model::{Access, Origin, ToolSet};
use brigadier_providers::{ApprovalDecision, Artifact, ProviderKind};

use super::{Flow, Options, Reply, Script, Turn, git};
use crate::model::{Environment, PermissionLevel, Setup};
use crate::work::{ApprovalSubject, CardState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// A turn's input and the extra folders its session has.
type Seen = (String, Vec<std::path::PathBuf>);

/// The session worktree its first turn made.
fn session_worktree(flow: &Flow) -> std::path::PathBuf {
    match flow.core.conversation(&flow.conversation).unwrap().setup {
        Some(Setup::Session {
            environment:
                Environment::NewWorktree {
                    path: Some(path), ..
                },
            ..
        }) => std::path::PathBuf::from(path),
        other => panic!("no session worktree: {other:?}"),
    }
}

/// At each level and for both vendors, the thread starts with its own tools in its scratch
/// folder, its workspace (the session worktree, made before the CLI starts) as an extra
/// folder, and a worker's access for that level; the workspace is never the thread's to
/// clean up.
#[tokio::test]
async fn the_thread_works_in_its_workspace_with_the_levels_access() {
    for (permission, thread) in [
        (PermissionLevel::FullAccess, ProviderKind::Claude),
        (PermissionLevel::ApproveForMe, ProviderKind::Codex),
        (PermissionLevel::AskForApproval, ProviderKind::Claude),
    ] {
        let seen: Arc<Mutex<Vec<std::path::PathBuf>>> = Arc::default();
        let log = seen.clone();
        let flow = Flow::start(
            "thread-spec",
            Options {
                permission,
                thread,
                ..Options::default()
            },
            script(move |turn| {
                let log = log.clone();
                async move {
                    // The workspace is there when the thread starts.
                    for dir in &turn.add_dirs {
                        assert!(dir.join("README.md").is_file(), "{}", dir.display());
                    }
                    log.lock().unwrap().extend(turn.add_dirs.clone());
                    Reply::text("The README is all there is.")
                }
            }),
        )
        .await;
        flow.say("What is in the repository?").await;
        flow.settled().await;
        let worktree = session_worktree(&flow);
        assert_eq!(*seen.lock().unwrap(), vec![worktree.clone()]);
        let specs = flow.thread_specs();
        assert_eq!(specs.len(), 1, "{permission:?}");
        let (provider, spec) = &specs[0];
        assert_eq!(*provider, thread);
        assert_eq!(spec.tools, ToolSet::Thread);
        assert_eq!(spec.add_dirs, vec![worktree.clone()]);
        assert!(spec.cwd.ends_with(format!("orch/{}", flow.conversation.0)));
        let env: std::collections::HashMap<_, _> = spec.env.iter().cloned().collect();
        assert_eq!(
            env.get("TMPDIR").map(String::as_str),
            Some(spec.cwd.to_str().unwrap())
        );
        assert_eq!(
            env.get("BASH_MAX_OUTPUT_LENGTH").map(String::as_str),
            (thread == ProviderKind::Claude).then_some("150000")
        );
        match permission {
            PermissionLevel::FullAccess => {
                assert_eq!(spec.access, Access::Full);
                assert!(!spec.auto_review);
            }
            level => {
                let Access::Scoped {
                    write_cwd,
                    writable_roots,
                    network,
                    ..
                } = &spec.access
                else {
                    panic!("a sandbox under {level:?}: {:?}", spec.access);
                };
                assert!(write_cwd);
                assert!(writable_roots.contains(&worktree));
                assert!(writable_roots.contains(&spec.cwd));
                // It commits its tiny edits into the repository's git folder.
                let common = std::path::PathBuf::from(git(
                    &worktree,
                    &["rev-parse", "--path-format=absolute", "--git-common-dir"],
                ));
                let common = common.canonicalize().unwrap();
                assert!(
                    writable_roots.iter().any(|root| root
                        .canonicalize()
                        .is_ok_and(|root| root.starts_with(&common))),
                    "{writable_roots:?}"
                );
                assert_eq!(*network, level == PermissionLevel::ApproveForMe);
                assert_eq!(spec.auto_review, level == PermissionLevel::ApproveForMe);
            }
        }
        // Nothing of the workspace is under the thread's cleanup owner.
        let owned = flow
            .manager
            .runtime
            .ledger()
            .artifacts(&format!("orch:{}", flow.conversation));
        let workspace = worktree.to_string_lossy().into_owned();
        assert!(
            !owned.iter().any(|artifact| matches!(
                artifact,
                Artifact::ProcessesIn { dir } | Artifact::ScratchDir { path: dir } | Artifact::Worktree { path: dir, .. }
                    if dir.starts_with(&workspace)
            )),
            "{owned:?}"
        );
        flow.stop().await;
    }
}

/// Under Ask for approval, the thread's request to leave its sandbox reaches the user as a
/// card (no longer declined outright), and the user's answer reaches the thread.
#[tokio::test]
async fn under_ask_the_threads_request_is_the_users_card() {
    let answered: Arc<Mutex<Option<ApprovalDecision>>> = Arc::default();
    let log = answered.clone();
    let flow = Flow::start(
        "thread-ask",
        Options {
            permission: PermissionLevel::AskForApproval,
            ..Options::default()
        },
        script(move |turn| {
            let log = log.clone();
            async move {
                let decision = turn.ask_approval("curl https://example.com", "curl").await;
                *log.lock().unwrap() = decision;
                Reply::text("Fetched it.")
            }
        }),
    )
    .await;
    flow.say("Fetch example.com.").await;
    let board = flow
        .until("the thread's card", |board| {
            board
                .approvals
                .values()
                .any(|approval| approval.state == CardState::Pending)
        })
        .await;
    let card = board
        .approvals
        .values()
        .find(|approval| approval.state == CardState::Pending)
        .unwrap();
    assert!(card.task_id.is_none(), "the thread's own request");
    assert!(matches!(
        &card.subject,
        ApprovalSubject::Cli { request } if request.command.as_deref() == Some("curl https://example.com")
    ));
    flow.manager
        .answer_card(
            flow.conversation.clone(),
            card.id.clone(),
            ApprovalDecision::Allow,
        )
        .await
        .unwrap();
    flow.settled().await;
    assert_eq!(*answered.lock().unwrap(), Some(ApprovalDecision::Allow));
    flow.stop().await;
}

/// A new workspace or permission level restarts the thread between turns: the same native
/// session resumes with the new folder and access, and its next turn says what changed.
#[tokio::test]
async fn a_new_workspace_or_level_resumes_the_thread_with_a_note() {
    let inputs: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let log = inputs.clone();
    let flow = Flow::start(
        "thread-move",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                log.lock()
                    .unwrap()
                    .push((turn.input.clone(), turn.add_dirs.clone()));
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.say("First.").await;
    flow.settled().await;
    let first = session_worktree(&flow);
    // The session's checkout moves to another folder.
    let moved = first.with_file_name("session-moved");
    git(
        &flow.repo,
        &[
            "worktree",
            "move",
            first.to_str().unwrap(),
            moved.to_str().unwrap(),
        ],
    );
    let moved = moved.canonicalize().unwrap();
    let Some(Setup::Session {
        repo,
        permission,
        orchestrator,
        workers_see_uncommitted,
        plan_mode,
        environment:
            Environment::NewWorktree {
                base,
                branch,
                start,
                ..
            },
    }) = flow.core.conversation(&flow.conversation).unwrap().setup
    else {
        panic!("a new-worktree session");
    };
    flow.manager
        .set_setup(
            flow.conversation.clone(),
            Setup::Session {
                repo: repo.clone(),
                environment: Environment::NewWorktree {
                    base: base.clone(),
                    branch: branch.clone(),
                    path: Some(moved.display().to_string()),
                    start: start.clone(),
                },
                permission,
                orchestrator: orchestrator.clone(),
                workers_see_uncommitted,
                plan_mode,
            },
        )
        .await
        .unwrap();
    flow.say("Second.").await;
    flow.until("the second turn", |_| inputs.lock().unwrap().len() >= 2)
        .await;
    flow.settled().await;
    // Then the level.
    flow.manager
        .set_setup(
            flow.conversation.clone(),
            Setup::Session {
                repo,
                environment: Environment::NewWorktree {
                    base,
                    branch: branch.clone(),
                    path: Some(moved.display().to_string()),
                    start,
                },
                permission: PermissionLevel::ApproveForMe,
                orchestrator,
                workers_see_uncommitted,
                plan_mode,
            },
        )
        .await
        .unwrap();
    flow.say("Third.").await;
    flow.until("the third turn", |_| inputs.lock().unwrap().len() >= 3)
        .await;
    flow.settled().await;

    let specs = flow.thread_specs();
    assert_eq!(specs.len(), 3, "one start and two restarts");
    let Origin::Resume { native_id } = &specs[1].1.origin else {
        panic!("the second start resumes: {:?}", specs[1].1.origin);
    };
    assert_eq!(
        specs[2].1.origin, specs[1].1.origin,
        "the same native session"
    );
    assert!(!native_id.is_empty());
    assert_eq!(specs[0].1.add_dirs, vec![first]);
    assert_eq!(specs[1].1.add_dirs, vec![moved.clone()]);
    assert_eq!(specs[1].1.access, Access::Full);
    assert!(matches!(specs[2].1.access, Access::Scoped { .. }));
    assert!(specs[2].1.auto_review);

    let inputs = inputs.lock().unwrap().clone();
    assert!(!inputs[0].0.contains("[workspace]"), "{}", inputs[0].0);
    assert!(
        inputs[1].0.contains(&format!(
            "[workspace] From now on you work in {} (on branch `{branch}`)",
            moved.display()
        )),
        "{}",
        inputs[1].0
    );
    assert_eq!(inputs[1].1, vec![moved]);
    assert!(
        inputs[2]
            .0
            .contains("[settings] The user changed the permission level."),
        "{}",
        inputs[2].0
    );
    assert!(!inputs[2].0.contains("[workspace]"), "{}", inputs[2].0);
    flow.stop().await;
}
