//! The session's thread (THREAD-PLAN.md Q1, Q6): its tools, workspace and access, approvals
//! under its permission level, and a restart for a new workspace or level.

use std::sync::{Arc, Mutex};

use brigadier_providers::model::{Access, Origin, ToolSet};
use brigadier_providers::{ApprovalDecision, Artifact, ProviderKind};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn, git};
use crate::model::{Environment, PermissionLevel, Setup};
use crate::work::{ApprovalSubject, CardState, OrchestratorStepKind};

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

/// "For this session" on the thread's file change in its workspace allows its later changes
/// there, never one outside it or in its `.git`; on a host it asked to reach, later requests
/// for that host. Each card offers the grant only where it is safe.
#[tokio::test]
async fn a_session_grant_allows_later_edits_in_the_workspace_only() {
    let answered: Arc<Mutex<Vec<Option<ApprovalDecision>>>> = Arc::default();
    let log = answered.clone();
    let flow = Flow::start(
        "thread-session-grant",
        Options {
            permission: PermissionLevel::AskForApproval,
            ..Options::default()
        },
        script(move |turn| {
            let log = log.clone();
            async move {
                let workspace = turn.add_dirs[0].clone();
                for paths in [
                    vec![workspace.join("a.txt")],
                    vec![workspace.join("new/b.txt"), workspace.join("c.txt")],
                    vec![turn.cwd.join("outside.txt")],
                    vec![workspace.join("d.txt"), turn.cwd.join("outside.txt")],
                    vec![workspace.join(".git")],
                ] {
                    let paths: Vec<&std::path::Path> = paths.iter().map(|p| p.as_path()).collect();
                    let decision = turn.ask_file_approval(&paths).await;
                    log.lock().unwrap().push(decision);
                }
                for _ in 0..2 {
                    let decision = turn.ask_network_approval("example.com").await;
                    log.lock().unwrap().push(decision);
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.say("Edit the files.").await;
    let mut grants = Vec::new();
    for done in 0..5 {
        let board = flow
            .until("the next card", |board| {
                board
                    .approvals
                    .values()
                    .filter(|approval| approval.state != CardState::Pending)
                    .count()
                    == done
                    && board
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
        let ApprovalSubject::Cli { request } = &card.subject else {
            panic!("{:?}", card.subject);
        };
        grants.push(request.grant.clone());
        let decision = if request.grant.is_some() {
            ApprovalDecision::AllowSimilar
        } else {
            ApprovalDecision::Allow
        };
        flow.manager
            .answer_card(flow.conversation.clone(), card.id.clone(), decision)
            .await
            .unwrap();
    }
    let board = flow.settled().await;
    let workspace = session_worktree(&flow).display().to_string();
    assert_eq!(
        grants,
        vec![
            Some(workspace),
            None,
            None,
            None,
            Some("example.com".into())
        ]
    );
    assert_eq!(
        board.approvals.len(),
        5,
        "two requests were allowed by the grants"
    );
    assert_eq!(
        *answered.lock().unwrap(),
        vec![Some(ApprovalDecision::Allow); 7]
    );
    // A worker in another workspace cannot borrow the thread's file grant.
    let mut edit = board
        .approvals
        .values()
        .find_map(|card| match &card.subject {
            ApprovalSubject::Cli { request }
                if request.kind == brigadier_providers::ApprovalKind::FileChange =>
            {
                Some(request.clone())
            }
            _ => None,
        })
        .unwrap();
    let workspace = session_worktree(&flow);
    edit.paths = vec![workspace.join("later.txt").display().to_string()];
    assert_eq!(
        flow.manager
            .approval_route(
                &flow.conversation,
                &mut edit,
                &brigadier_providers::Access::ReadOnly,
                Some(&workspace)
            )
            .0,
        brigadier_providers::policy::Route::Allow
    );
    assert_eq!(
        flow.manager
            .approval_route(
                &flow.conversation,
                &mut edit,
                &brigadier_providers::Access::ReadOnly,
                Some(&flow.repo)
            )
            .0,
        brigadier_providers::policy::Route::AskUser
    );
    assert_eq!(edit.grant, None);
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

/// The thread asks for a review of its own plan and carries on at once; the other vendor's
/// findings reach it as a message. Its code index tools answer for the session's project.
#[tokio::test]
async fn the_threads_plan_review_runs_in_the_background() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let flow = Flow::start(
        "thread-plan",
        Options {
            reviews: Some(script(|turn| async move {
                assert!(turn.input.contains("Rename the README"), "{}", turn.input);
                Reply::text("- [P2] The plan never checks the links to README.md.")
            })),
            ..Options::default()
        },
        script(move |turn| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(turn.input.clone());
                if turn.input.contains("[plan review") {
                    return Reply::text("I'll check the links too.");
                }
                let map = turn.call("project_map", json!({})).await;
                assert!(!map.is_error, "{}", map.text);
                let started = turn
                    .call(
                        "review_plan",
                        json!({"plan": "1. Rename the README to README.txt.",
                               "brief": "The user wants a plain-text README."}),
                    )
                    .await;
                assert!(!started.is_error, "{}", started.text);
                assert!(
                    started.text.starts_with("Started a review of your plan"),
                    "{}",
                    started.text
                );
                Reply::text("Planned.")
            }
        }),
    )
    .await;
    flow.say("Make the README plain text.").await;
    flow.until("the plan review's findings", |_| {
        inputs
            .lock()
            .unwrap()
            .iter()
            .any(|input| input.contains("[plan review of your plan"))
    })
    .await;
    flow.settled().await;
    let board = flow.board().await;
    let review = board.reviews.values().next().expect("a plan review");
    assert_eq!(review.kind, crate::work::ReviewKind::Plan);
    assert!(review.task_id.is_none());
    assert_eq!(review.author, ProviderKind::Claude);
    assert_eq!(review.reviewer, ProviderKind::Codex);
    let heard = inputs.lock().unwrap().clone();
    let findings = heard
        .iter()
        .find(|input| input.contains("[plan review of your plan"))
        .unwrap();
    assert!(findings.contains("never checks the links"), "{findings}");
    flow.stop().await;
}

/// Commits a file in the thread's workspace, as the thread's own tiny edit would.
pub(super) fn commit_in_workspace(turn: &Turn, file: &str, text: &str, message: &str) -> String {
    let workspace = &turn.add_dirs[0];
    std::fs::write(workspace.join(file), text).unwrap();
    git(workspace, &["add", file]);
    git(workspace, &["commit", "-q", "-m", message]);
    git(workspace, &["rev-parse", "HEAD"])
}

/// The code reviews of the session, oldest first, once all have ended.
async fn code_reviews(flow: &Flow, count: usize) -> Vec<crate::work::ReviewRun> {
    let board = flow
        .until("the reviews to end", |board| {
            board.reviews.len() >= count
                && board
                    .reviews
                    .values()
                    .all(|review| review.state != crate::work::ReviewState::Running)
        })
        .await;
    let mut reviews: Vec<_> = board.reviews.values().cloned().collect();
    reviews.sort_by_key(|review| review.started_at_ms);
    reviews
}

/// A commit the thread makes in a turn gets one review by the other vendor, whose findings
/// reach the thread; a restart neither reviews it again nor misses it.
#[tokio::test]
async fn a_thread_commit_is_reviewed_once() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let commits: Arc<Mutex<Vec<String>>> = Arc::default();
    let made = commits.clone();
    let mut flow = Flow::start(
        "thread-commit",
        Options {
            reviews: Some(script(|turn| async move {
                assert!(turn.input.contains("git diff"), "{}", turn.input);
                Reply::text("- [P2] NOTES.md has no title — NOTES.md:1\n  Add one.")
            })),
            ..Options::default()
        },
        script(move |turn| {
            let (log, made) = (log.clone(), made.clone());
            async move {
                log.lock().unwrap().push(turn.input.clone());
                if turn.input.contains("Add notes.") {
                    let start = git(&turn.add_dirs[0], &["rev-parse", "HEAD"]);
                    let tip = commit_in_workspace(&turn, "NOTES.md", "notes\n", "Add notes");
                    made.lock().unwrap().extend([start, tip]);
                    return Reply::text("Added NOTES.md.");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("Add notes.").await;
    flow.settled().await;
    let reviews = code_reviews(&flow, 1).await;
    assert_eq!(reviews.len(), 1, "{reviews:#?}");
    let review = &reviews[0];
    let (start, tip) = {
        let commits = commits.lock().unwrap();
        (commits[0].clone(), commits[1].clone())
    };
    assert_eq!(review.kind, crate::work::ReviewKind::Code);
    assert!(review.task_id.is_none());
    assert_eq!(
        (review.base.as_str(), review.tip.as_str()),
        (start.as_str(), tip.as_str())
    );
    assert_eq!(review.author, ProviderKind::Claude);
    assert_eq!(review.reviewer, ProviderKind::Codex);
    flow.until("the findings to reach the thread", |_| {
        inputs
            .lock()
            .unwrap()
            .iter()
            .any(|input| input.contains("[review of your commits · Codex found 1"))
    })
    .await;
    flow.settled().await;
    // After a restart, more turns find nothing new.
    flow.restart().await;
    flow.say("Anything else?").await;
    flow.settled().await;
    let board = flow.board().await;
    assert_eq!(board.reviews.len(), 1, "{:#?}", board.reviews);
    let branch = git(
        &session_worktree(&flow),
        &["rev-parse", "--abbrev-ref", "HEAD"],
    );
    assert_eq!(board.thread_tips.get(&branch), Some(&tip));
    flow.stop().await;
}

/// A turn commits on the session branch and then lands a worker on top, within the same turn:
/// the thread's commit is reviewed from where the turn found the branch, and the landing on
/// its own, on top of it; nothing is reviewed twice.
#[tokio::test]
async fn a_thread_commit_before_a_landing_in_the_same_turn_is_reviewed_on_its_own() {
    let commits: Arc<Mutex<Vec<String>>> = Arc::default();
    let made = commits.clone();
    let flow = Flow::start(
        "thread-commit-land",
        Options::default(),
        script(move |turn| {
            let made = made.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("[report task-1") {
                        let start = git(&turn.add_dirs[0], &["rev-parse", "HEAD"]);
                        let tip = commit_in_workspace(
                            &turn,
                            "NOTES.md",
                            "notes\n",
                            "Add notes\n\nBrigadier-Author: thread",
                        );
                        made.lock().unwrap().extend([start, tip]);
                        // The branch moved under the worker: it is rebased, checks itself and
                        // lands on its own, while this turn still runs.
                        let landed = turn.call("land_phase", json!({"task": "task-1"})).await;
                        assert!(!landed.is_error, "{}", landed.text);
                        assert!(landed.text.contains("rebased"), "{}", landed.text);
                        let deadline = std::time::Instant::now() + super::PATIENCE;
                        loop {
                            let tasks = turn.call("list_tasks", json!({})).await;
                            if tasks.text.contains("Landed") {
                                break;
                            }
                            assert!(std::time::Instant::now() < deadline, "{}", tasks.text);
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        }
                        return Reply::text("Added the greeting and notes.");
                    }
                    if turn.input.contains("Add a greeting") {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "high", "title": "Add a greeting", "kind": "implement",
                                       "spec": "Create hello.txt.", "provider": "codex"}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("[quiet]");
                    }
                    return Reply::text("Noted.");
                }
                turn.write("hello.txt", "hello\n");
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Added hello.txt.", "changes": ["hello.txt"]}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Add a greeting.").await;
    flow.settled().await;
    let reviews = code_reviews(&flow, 2).await;
    let (start, thread_tip) = {
        let commits = commits.lock().unwrap();
        (commits[0].clone(), commits[1].clone())
    };
    let board = flow.board().await;
    let lead = Flow::task(&board, 1);
    let landed = lead.landed.clone().expect("the worker landed");
    let ranges: Vec<(Option<&crate::work::TaskId>, &str, &str)> = reviews
        .iter()
        .map(|review| {
            (
                review.task_id.as_ref(),
                review.base.as_str(),
                review.tip.as_str(),
            )
        })
        .collect();
    assert_eq!(reviews.len(), 2, "{ranges:#?}");
    assert!(
        ranges.contains(&(None, start.as_str(), thread_tip.as_str())),
        "the thread's commit, from where the turn found the branch: {ranges:#?}"
    );
    assert!(
        ranges.contains(&(Some(&lead.id), thread_tip.as_str(), landed.as_str())),
        "the landing, on top of the thread's commit: {ranges:#?}"
    );
    let branch = lead.workspace.as_ref().unwrap().target.clone().unwrap();
    assert_eq!(board.thread_tips.get(&branch), Some(&landed));
    // THREAD-PLAN.md Q13: the thread's own commit is its self-edit; the landing is not.
    let edits = |metrics: crate::model::ThreadMetrics| {
        let edits = metrics.edits.expect("the thread's edits are counted");
        (edits.commits, edits.added, edits.removed, edits.kept)
    };
    let metrics = flow
        .manager
        .thread_metrics(&flow.conversation)
        .await
        .unwrap();
    assert_eq!(edits(metrics), (1, 1, 0, false));
    // Once the branch is merged and removed, the last count stands.
    git(
        &flow.repo,
        &[
            "worktree",
            "remove",
            "--force",
            &session_worktree(&flow).display().to_string(),
        ],
    );
    git(&flow.repo, &["branch", "-D", &branch]);
    let metrics = flow
        .manager
        .thread_metrics(&flow.conversation)
        .await
        .unwrap();
    assert_eq!(edits(metrics), (1, 1, 0, true));
    flow.stop().await;
}

/// The thread commits and merges in the same turn, as the user asked: the review of its commit
/// has started (for the thread, with no task) before the merge, so it counts with the merged
/// work, and the merge's answer says it still runs. No card opens.
#[tokio::test]
async fn a_merge_counts_the_review_of_the_threads_own_commit() {
    let release = Arc::new(tokio::sync::Notify::new());
    let held = release.clone();
    let flow = Flow::start(
        "thread-commit-merge",
        Options {
            reviews: Some(script(move |_| {
                let held = held.clone();
                async move {
                    held.notified().await;
                    Reply::text("- [P2] NOTES.md has no title — NOTES.md:1\n  Add one.")
                }
            })),
            ..Options::default()
        },
        script(|turn| async move {
            if turn.input.contains("Add notes and merge.") {
                commit_in_workspace(
                    &turn,
                    "NOTES.md",
                    "notes\n",
                    "Add notes\n\nBrigadier-Author: thread",
                );
                let reply = turn
                    .call("finish_session", json!({"user_words": "merge"}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                assert!(reply.text.contains("review still runs"), "{}", reply.text);
                return Reply::text("Merged; the review still runs.");
            }
            Reply::text("Noted.")
        }),
    )
    .await;
    flow.say("Add notes and merge.").await;
    let board = flow
        .until("the merge", |board| {
            board
                .orchestrator_steps
                .iter()
                .any(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
        })
        .await;
    assert!(board.approvals.is_empty(), "no merge card");
    let merged = board
        .orchestrator_steps
        .iter()
        .find(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
        .unwrap()
        .clone();
    assert!(
        matches!(
            &merged.kind,
            OrchestratorStepKind::Merged {
                commits: 1,
                asked_in: Some(_),
                ..
            }
        ),
        "{merged:?}"
    );
    let reviews: Vec<_> = board.reviews.values().cloned().collect();
    assert_eq!(reviews.len(), 1, "{reviews:#?}");
    let review = &reviews[0];
    assert!(review.task_id.is_none(), "the thread's own commit");
    assert_eq!(review.kind, crate::work::ReviewKind::Code);
    assert_eq!(review.notify, crate::work::ReviewFor::Orchestrator);
    assert_eq!(review.state, crate::work::ReviewState::Running);
    assert!(
        review.started_at_ms <= merged.at_ms,
        "started before the merge, so it counts with the merged work"
    );
    release.notify_one();
    let reviews = code_reviews(&flow, 1).await;
    assert_eq!(
        reviews[0].state,
        crate::work::ReviewState::Findings { count: 1 }
    );
    flow.stop().await;
}

/// A thread whose CLI started on older instructions (the contract before this build's in its
/// log, as a session created before the last change of the thread's rules has) isn't resumed,
/// since it would keep its old rules: it starts over from the transcript with the current ones.
#[tokio::test]
async fn a_thread_started_on_older_instructions_starts_over_instead_of_resuming() {
    starts_over_on_older_instructions(ProviderKind::Claude).await;
}

#[tokio::test]
async fn a_codex_thread_started_on_older_instructions_starts_over_instead_of_resuming() {
    starts_over_on_older_instructions(ProviderKind::Codex).await;
}

async fn starts_over_on_older_instructions(thread: ProviderKind) {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let mut flow = Flow::start(
        &format!("thread-old-role-{thread:?}"),
        Options {
            thread,
            ..Options::default()
        },
        script(move |turn| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(turn.input.clone());
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.say("First, remember the word teal.").await;
    flow.settled().await;
    // What the build before logged when that CLI started.
    flow.core
        .record(vec![(
            crate::model::streams::orchestrator(&flow.conversation),
            crate::model::DomainEvent::OrchestratorLogged {
                conversation_id: flow.conversation.clone(),
                entry: crate::work::OrchestratorEntry::Injection {
                    injection: crate::work::ContextInjection {
                        kind: crate::work::InjectionKind::Instructions,
                        bytes: 11_371,
                        tokens_estimate: 2_843,
                        label: "role instructions, short replies".into(),
                        task_id: None,
                        told: Some(crate::work::Told {
                            contract: Some(crate::manager::prompts::CONTRACT - 1),
                            ..crate::work::Told::default()
                        }),
                    },
                },
            },
        )])
        .await
        .unwrap();
    flow.restart().await;
    flow.say("Second.").await;
    flow.until("the second turn", |_| inputs.lock().unwrap().len() >= 2)
        .await;
    flow.settled().await;
    let specs = flow.thread_specs();
    assert_eq!(specs.last().unwrap().1.origin, Origin::New, "{specs:#?}");
    let second = inputs.lock().unwrap()[1].clone();
    assert!(second.contains("remember the word teal"), "{second}");
    let rules = specs
        .last()
        .unwrap()
        .1
        .append_system_prompt
        .clone()
        .unwrap();
    assert!(rules.contains("started in one batch"), "{rules}");
    assert!(
        rules.contains("Interview the user when they invite questions"),
        "{rules}"
    );
    assert!(rules.contains("propose_merge, propose_plan"), "{rules}");
    assert!(
        specs.iter().all(|(provider, _)| *provider == thread),
        "{specs:#?}"
    );
    // Its new start is logged on the thread's contract: the next restart resumes it.
    flow.restart().await;
    flow.say("Third.").await;
    flow.until("the third turn", |_| inputs.lock().unwrap().len() >= 3)
        .await;
    flow.settled().await;
    assert!(
        matches!(
            flow.thread_specs().last().unwrap().1.origin,
            Origin::Resume { .. }
        ),
        "{:#?}",
        flow.thread_specs()
    );
    flow.stop().await;
}

/// After the merge the user asked for, the session's worktree and merged branch are gone
/// (THREAD-PLAN.md Q9). The rest of that turn doesn't bring them back; the user's next message does, at the
/// same path, on a fresh branch from the base's tip as it is then, and the thread goes on in
/// the same CLI session.
#[tokio::test]
async fn a_merge_removes_the_session_worktree_and_the_next_message_starts_fresh() {
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let log = seen.clone();
    let flow = Flow::start(
        "thread-merge-fresh",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                log.lock()
                    .unwrap()
                    .push((turn.input.clone(), turn.add_dirs.clone()));
                if turn.input.contains("Add notes.") {
                    commit_in_workspace(
                        &turn,
                        "NOTES.md",
                        "notes\n",
                        "Add notes\n\nBrigadier-Author: thread",
                    );
                    return Reply::text("Notes added. Merge them into `main`?");
                }
                if turn.input.contains("yes, merge it") {
                    let reply = turn
                        .call("finish_session", json!({"user_words": "yes, merge it"}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    log.lock().unwrap().push((reply.text.clone(), Vec::new()));
                    return Reply::text("Merged.");
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.say("Add notes.").await;
    flow.settled().await;
    let worktree = session_worktree(&flow);
    let branch = git(&worktree, &["rev-parse", "--abbrev-ref", "HEAD"]);
    flow.say("yes, merge it").await;
    flow.until("the [finished] answer", |_| {
        seen.lock()
            .unwrap()
            .iter()
            .any(|(input, _)| input.contains("[finished]"))
    })
    .await;
    flow.settled().await;
    let finished = seen
        .lock()
        .unwrap()
        .iter()
        .find(|(input, _)| input.contains("[finished]"))
        .unwrap()
        .0
        .clone();
    assert!(
        finished.contains("worktree and branch are removed"),
        "{finished}"
    );
    assert!(!worktree.exists(), "{}", worktree.display());
    assert_eq!(git(&flow.repo, &["branch", "--list", &branch]), "");
    assert!(git(&flow.repo, &["cat-file", "-e", "main:NOTES.md"]).is_empty());
    assert!(matches!(
        flow.core.conversation(&flow.conversation).unwrap().setup,
        Some(Setup::Session {
            environment: Environment::NewWorktree { path: None, .. },
            ..
        })
    ));
    // The base moves on after the merge; the next message starts from where it is then.
    std::fs::write(flow.repo.join("LATER.md"), "later\n").unwrap();
    git(&flow.repo, &["add", "LATER.md"]);
    git(&flow.repo, &["commit", "-q", "-m", "Later"]);
    let main_tip = git(&flow.repo, &["rev-parse", "main"]);
    let specs = flow.thread_specs().len();
    flow.say("Next.").await;
    flow.settled().await;
    assert_eq!(session_worktree(&flow), worktree, "the same path");
    assert_eq!(git(&worktree, &["rev-parse", "HEAD"]), main_tip);
    assert_eq!(
        git(&worktree, &["rev-parse", "--abbrev-ref", "HEAD"]),
        branch
    );
    let (input, dirs) = seen.lock().unwrap().last().unwrap().clone();
    assert!(input.contains("Next."), "{input}");
    assert!(!input.contains("[workspace]"), "{input}");
    assert_eq!(dirs[0], worktree);
    assert_eq!(
        flow.thread_specs().len(),
        specs,
        "the same CLI session goes on: no restart, no rebirth"
    );
    flow.stop().await;
}

/// A merge whose worktree removal failed (a folder it may not empty) says so, and the user's
/// next message removes it first, then starts fresh from the base's tip at the same path: the
/// next launch's sweep has nothing left to remove from under the session.
#[tokio::test]
async fn a_merge_whose_worktree_removal_failed_retries_it_at_the_next_message() {
    use std::os::unix::fs::PermissionsExt;
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let log = seen.clone();
    let flow = Flow::start(
        "thread-merge-unremovable",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                log.lock()
                    .unwrap()
                    .push((turn.input.clone(), turn.add_dirs.clone()));
                if turn.input.contains("Add notes.") {
                    commit_in_workspace(&turn, "NOTES.md", "notes\n", "Add notes");
                    return Reply::text("Notes added. Merge them into `main`?");
                }
                if turn.input.contains("yes, merge it") {
                    let reply = turn
                        .call("finish_session", json!({"user_words": "yes, merge it"}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    log.lock().unwrap().push((reply.text.clone(), Vec::new()));
                    return Reply::text("Merged.");
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.say("Add notes.").await;
    flow.settled().await;
    let worktree = session_worktree(&flow);
    let branch = git(&worktree, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let mode = |path: &std::path::Path, mode: u32| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    mode(&worktree, 0o555);
    flow.say("yes, merge it").await;
    flow.until("the [finished] answer", |_| {
        seen.lock()
            .unwrap()
            .iter()
            .any(|(input, _)| input.contains("[finished]"))
    })
    .await;
    flow.settled().await;
    mode(&worktree, 0o755);
    let finished = seen
        .lock()
        .unwrap()
        .iter()
        .find(|(input, _)| input.contains("[finished]"))
        .unwrap()
        .0
        .clone();
    assert!(finished.contains("couldn't be removed yet"), "{finished}");
    let owner = format!("session:{}", flow.conversation);
    assert!(flow.manager.runtime.ledger().disposing().contains(&owner));
    std::fs::write(flow.repo.join("LATER.md"), "later\n").unwrap();
    git(&flow.repo, &["add", "LATER.md"]);
    git(&flow.repo, &["commit", "-q", "-m", "Later"]);
    let main_tip = git(&flow.repo, &["rev-parse", "main"]);
    flow.say("Next.").await;
    flow.settled().await;
    assert_eq!(session_worktree(&flow), worktree, "the same path");
    assert_eq!(git(&worktree, &["rev-parse", "HEAD"]), main_tip);
    assert_eq!(
        git(&worktree, &["rev-parse", "--abbrev-ref", "HEAD"]),
        branch
    );
    assert!(
        !flow.manager.runtime.ledger().disposing().contains(&owner),
        "nothing left for the next launch's sweep"
    );
    let (input, _) = seen.lock().unwrap().last().unwrap().clone();
    assert!(input.contains("Next."), "{input}");
    flow.stop().await;
}

/// Uncommitted changes in the session's worktree, or a lock the user put on it, keep it and its
/// branch after a merge, and the thread is told why.
#[tokio::test]
async fn a_merge_keeps_a_session_worktree_with_uncommitted_changes() {
    for locked in [false, true] {
        merge_keeps_the_worktree(locked).await;
    }
}

async fn merge_keeps_the_worktree(locked: bool) {
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let log = seen.clone();
    let flow = Flow::start(
        if locked {
            "thread-merge-locked"
        } else {
            "thread-merge-dirty"
        },
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                log.lock()
                    .unwrap()
                    .push((turn.input.clone(), turn.add_dirs.clone()));
                if turn.input.contains("Add notes.") {
                    commit_in_workspace(&turn, "NOTES.md", "notes\n", "Add notes");
                    return Reply::text("Notes added. Merge them into `main`?");
                }
                if turn.input.contains("yes, merge it") {
                    let reply = turn
                        .call("finish_session", json!({"user_words": "yes, merge it"}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    log.lock().unwrap().push((reply.text.clone(), Vec::new()));
                    return Reply::text("Merged.");
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.say("Add notes.").await;
    flow.settled().await;
    let worktree = session_worktree(&flow);
    if locked {
        git(
            &flow.repo,
            &["worktree", "lock", &worktree.display().to_string()],
        );
    } else {
        std::fs::write(worktree.join("DRAFT.md"), "draft\n").unwrap();
    }
    let branch = git(&worktree, &["rev-parse", "--abbrev-ref", "HEAD"]);
    flow.say("yes, merge it").await;
    flow.until("the [finished] answer", |_| {
        seen.lock()
            .unwrap()
            .iter()
            .any(|(input, _)| input.contains("[finished]"))
    })
    .await;
    flow.settled().await;
    let finished = seen
        .lock()
        .unwrap()
        .iter()
        .find(|(input, _)| input.contains("[finished]"))
        .unwrap()
        .0
        .clone();
    if locked {
        assert!(finished.contains("is locked"), "{finished}");
        git(
            &flow.repo,
            &["worktree", "unlock", &worktree.display().to_string()],
        );
    } else {
        assert!(
            finished.contains("uncommitted changes (DRAFT.md)"),
            "{finished}"
        );
        assert!(worktree.join("DRAFT.md").exists());
    }
    assert!(worktree.exists());
    assert_eq!(
        git(&flow.repo, &["branch", "--list", &branch]).trim_start_matches(['*', '+', ' ']),
        branch
    );
    assert_eq!(session_worktree(&flow), worktree);
    flow.stop().await;
}

/// The user's Stop while the thread runs a command closes its CLI when the turn ends, so the
/// command's process tree ends with it (a CLI's interrupt can leave it running); the next
/// message resumes the same session. A Stop with no command running keeps the CLI.
#[tokio::test]
async fn a_stop_during_a_command_ends_it_and_the_next_message_resumes() {
    let flow = Flow::start(
        "thread-stop-command",
        Options::default(),
        script(move |turn| async move {
            if turn.input.contains("Run the long command.") {
                turn.emit(brigadier_providers::ProviderEvent::ToolCall {
                    item_id: "bash-1".into(),
                    name: "Bash".into(),
                    input: Some(json!({ "command": "sleep 45" }).to_string()),
                    status: brigadier_providers::ItemStatus::InProgress,
                    output: None,
                })
                .await;
                assert!(turn.stopped().await, "the user stops it");
                return Reply::text("");
            }
            if turn.input.contains("Think a while.") {
                assert!(turn.stopped().await, "the user stops it");
                return Reply::text("");
            }
            Reply::text("Done.")
        }),
    )
    .await;
    flow.say("Hello.").await;
    flow.settled().await;
    let started = flow.thread_specs().len();

    // No command runs: the CLI stays.
    flow.say("Think a while.").await;
    flow.until("the turn to run", |board| {
        board.run == crate::work::RunState::Running
    })
    .await;
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    flow.settled().await;
    flow.say("Hello again.").await;
    flow.settled().await;
    assert_eq!(flow.thread_specs().len(), started, "no restart");

    // A command runs: the CLI closes with it, and the next message resumes the session.
    flow.say("Run the long command.").await;
    flow.until("the command to run", |board| {
        board.orchestrator_steps.iter().any(|step| {
            matches!(&step.kind, crate::work::OrchestratorStepKind::Tool { item_id, .. } if item_id == "bash-1")
        })
    })
    .await;
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    flow.settled().await;
    flow.say("Hello once more.").await;
    flow.settled().await;
    let specs = flow.thread_specs();
    assert_eq!(specs.len(), started + 1, "one restart");
    let first = specs[0].1.clone();
    match &specs[started].1.origin {
        Origin::Resume { native_id } => {
            assert!(!native_id.is_empty());
            assert_eq!(specs[started].1.cwd, first.cwd);
        }
        other => panic!("a resume, not {other:?}"),
    }
    flow.stop().await;
}

/// A preview left running and a review still under way after the thread answered don't keep
/// the request working: its block is done and "Worked for" stops, and neither shows as a
/// worker. The review's findings start a turn for the same request, which works (and shows its
/// live line) until that turn ends.
#[tokio::test]
async fn a_running_preview_or_review_leaves_the_answer_done_and_findings_work_again() {
    let review_gate = Arc::new(tokio::sync::Notify::new());
    let findings_gate = Arc::new(tokio::sync::Notify::new());
    let (review_wait, findings_wait) = (review_gate.clone(), findings_gate.clone());
    let flow = Flow::start(
        "thread-background-work",
        Options {
            reviews: Some(script(move |_| {
                let gate = review_wait.clone();
                async move {
                    gate.notified().await;
                    Reply::text("- [P2] NOTES.md has no title — NOTES.md:1\n  Add one.")
                }
            })),
            ..Options::default()
        },
        script(move |turn| {
            let gate = findings_wait.clone();
            async move {
                if turn.input.contains("Add notes.") {
                    commit_in_workspace(&turn, "NOTES.md", "notes\n", "Add notes");
                    let reply = turn
                        .call(
                            "start_preview",
                            json!({ "command": "echo up; sleep 600", "name": "site" }),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Added NOTES.md; the site runs.");
                }
                if turn.input.contains("[review of your commits") {
                    gate.notified().await;
                    return Reply::text("Fixed the title.");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("Add notes.").await;
    let board = flow
        .until("the answer done, the preview and review running", |board| {
            board
                .requests
                .values()
                .all(|request| request.state == crate::work::RequestState::Done)
                && board
                    .previews
                    .values()
                    .any(|preview| preview.state.is_running())
                && board
                    .reviews
                    .values()
                    .any(|review| review.state == crate::work::ReviewState::Running)
        })
        .await;
    let request = board.requests.values().next().unwrap().clone();
    assert!(
        request.worked.iter().all(|span| span.to_ms.is_some()),
        "Worked for stops: {:?}",
        request.worked
    );
    assert!(board.tasks.is_empty(), "no worker: {:?}", board.tasks);
    // Still done a moment later, while both run.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let board = flow.board().await;
    assert_eq!(
        board.requests[&request.id].state,
        crate::work::RequestState::Done
    );
    assert_eq!(board.run, crate::work::RunState::Idle);

    // The findings arrive: the same request works while the thread takes them in.
    review_gate.notify_one();
    let board = flow
        .until("the findings turn to work", |board| {
            board.requests[&request.id].state == crate::work::RequestState::Working
                && board.run == crate::work::RunState::Running
        })
        .await;
    assert_eq!(board.requests.len(), 1, "the same request");
    assert!(
        board.requests[&request.id]
            .worked
            .last()
            .is_some_and(|span| span.to_ms.is_none()),
        "a new span: {:?}",
        board.requests[&request.id].worked
    );
    findings_gate.notify_one();
    let board = flow
        .until("the findings turn to end", |board| {
            board.requests[&request.id].state == crate::work::RequestState::Done
        })
        .await;
    assert!(
        board.requests[&request.id]
            .worked
            .iter()
            .all(|span| span.to_ms.is_some())
    );
    flow.manager
        .stop_preview(flow.conversation.clone(), None)
        .await
        .unwrap();
    flow.stop().await;
}

/// What the thread was told by its finish_session calls, in order.
type MergeReplies = Arc<Mutex<Vec<(String, bool)>>>;

async fn finish(turn: &Turn, words: &str, replies: &MergeReplies) -> bool {
    let reply = turn
        .call("finish_session", json!({ "user_words": words }))
        .await;
    replies
        .lock()
        .unwrap()
        .push((reply.text.clone(), reply.is_error));
    !reply.is_error
}

pub(super) fn on_main(flow: &Flow, path: &str) -> bool {
    std::process::Command::new("git")
        .args(["cat-file", "-e", &format!("main:{path}")])
        .current_dir(&flow.repo)
        .status()
        .is_ok_and(|status| status.success())
}

/// Merging is asked for in words, with no card: the thread proposes it in its reply, and
/// finish_session refuses until the user's latest message agrees (silence, the thread's own
/// guess at a yes, a "no", a "not yet"); then a "yes, merge it" merges, once.
#[tokio::test]
async fn the_merge_is_asked_in_words_and_happens_only_on_the_users_yes() {
    let replies: MergeReplies = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        "thread-merge-words",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                let input = turn.input.clone();
                if input.contains("Add notes.") {
                    commit_in_workspace(&turn, "NOTES.md", "notes\n", "Add notes");
                    // Before the user answered: neither the thread's guess nor their request
                    // for the work is consent.
                    assert!(!finish(&turn, "yes", &log).await);
                    assert!(!finish(&turn, "Add notes", &log).await);
                    return Reply::text(
                        "Notes added, and the review is clean. Merge them into `main`?",
                    );
                }
                if input.contains("No, not yet.") {
                    assert!(!finish(&turn, "No, not yet.", &log).await);
                    return Reply::text("OK, it stays on its branch. Merge it into `main` now?");
                }
                if input.contains("don't merge it") {
                    assert!(!finish(&turn, "merge it", &log).await);
                    return Reply::text("Left unmerged.");
                }
                if input.contains("yes, merge it") {
                    assert!(finish(&turn, "yes, merge it", &log).await);
                    // One yes, one merge.
                    assert!(!finish(&turn, "yes, merge it", &log).await);
                    return Reply::text("Merged.");
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.say("Add notes.").await;
    flow.settled().await;
    flow.say("No, not yet.").await;
    flow.settled().await;
    flow.say("Hmm, don't merge it").await;
    flow.settled().await;
    assert!(
        !on_main(&flow, "NOTES.md"),
        "nothing merged on silence or a no"
    );
    flow.say("OK, yes, merge it").await;
    flow.settled().await;
    assert!(on_main(&flow, "NOTES.md"), "merged on the yes");
    let replies = replies.lock().unwrap().clone();
    let errors: Vec<_> = replies.iter().filter(|(_, error)| *error).collect();
    assert_eq!(errors.len(), 5, "{replies:#?}");
    assert!(
        errors.iter().all(|(text, _)| text.contains("[not merged]")),
        "{replies:#?}"
    );
    assert!(
        replies[0].0.contains("is not in the user's latest message"),
        "{replies:#?}"
    );
    assert!(
        replies[5].0.contains("already asked for a merge"),
        "{replies:#?}"
    );
    let finished = replies.iter().find(|(_, error)| !error).unwrap();
    assert!(
        finished.0.contains("[finished] As the user asked"),
        "{replies:#?}"
    );
    let board = flow.board().await;
    assert!(board.approvals.is_empty(), "no merge card");
    let merged: Vec<_> = board
        .orchestrator_steps
        .iter()
        .filter(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
        .collect();
    assert_eq!(merged.len(), 1, "{merged:#?}");
    flow.stop().await;
}

/// A "wait" the user sends while the merge is being prepared stops it: the last look at their
/// consent, right before it lands, sees they wrote again.
#[tokio::test]
async fn a_wait_sent_while_the_merge_is_prepared_stops_it() {
    let replies: MergeReplies = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        "thread-merge-withdrawn",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.input.contains("Add notes and merge it.") {
                    commit_in_workspace(&turn, "NOTES.md", "notes\n", "Add notes");
                    assert!(!finish(&turn, "merge it", &log).await);
                    return Reply::text("You wrote again, so I didn't merge.");
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    let (reached, release) = (
        Arc::new(tokio::sync::Notify::new()),
        Arc::new(tokio::sync::Notify::new()),
    );
    *flow
        .manager
        .conv(&flow.conversation)
        .unwrap()
        .merge_pause
        .lock()
        .unwrap() = Some((reached.clone(), release.clone()));
    flow.say("Add notes and merge it.").await;
    reached.notified().await;
    flow.say("wait!").await;
    release.notify_one();
    flow.until("the refused merge", |_| !replies.lock().unwrap().is_empty())
        .await;
    flow.settled().await;
    let replies = replies.lock().unwrap().clone();
    assert!(replies[0].0.contains("wrote again"), "{replies:#?}");
    assert!(!on_main(&flow, "NOTES.md"), "nothing merged");
    flow.stop().await;
}

/// A "wait" the user is still sending (its message not yet stored) when the merge takes its
/// first look holds the merge until it is stored: the merge then sees it and refuses, instead of
/// counting it as seen with the old message as the latest.
#[tokio::test]
async fn a_wait_still_being_stored_when_the_merge_looks_stops_it() {
    let replies: MergeReplies = Arc::default();
    let log = replies.clone();
    let (started, release) = (
        Arc::new(tokio::sync::Notify::new()),
        Arc::new(tokio::sync::Notify::new()),
    );
    let writing = started.clone();
    let flow = Flow::start(
        "thread-merge-storing",
        Options::default(),
        script(move |turn| {
            let (log, writing) = (log.clone(), writing.clone());
            async move {
                if turn.input.contains("Add notes and merge it.") {
                    commit_in_workspace(&turn, "NOTES.md", "notes\n", "Add notes");
                    // The user's "wait!" is being stored now.
                    writing.notified().await;
                    assert!(!finish(&turn, "merge it", &log).await);
                    return Reply::text("You wrote again, so I didn't merge.");
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.say("Add notes and merge it.").await;
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    let (core, id) = (flow.core.clone(), flow.conversation.clone());
    let (signal, held) = (started.clone(), release.clone());
    let write = tokio::spawn(async move {
        conv.user_write(async {
            signal.notify_one();
            held.notified().await;
            core.append_user_message(id.clone(), "wait!".into(), Vec::new(), Vec::new())
                .await
        })
        .await
        .unwrap();
    });
    // Whenever the merge's look comes, the write is under way: it waits for it.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    release.notify_one();
    write.await.unwrap();
    flow.until("the refused merge", |_| !replies.lock().unwrap().is_empty())
        .await;
    let replies = replies.lock().unwrap().clone();
    // The merge's look saw "wait!" as the latest message: "merge it" isn't in it.
    assert!(
        replies[0].0.contains("is not in the user's latest message"),
        "{replies:#?}"
    );
    assert!(!on_main(&flow, "NOTES.md"), "nothing merged");
    flow.stop().await;
}

/// The user editing a queued message while the merge is being prepared stops it, as a new
/// message would: an older queued item doesn't hold the merge, its edit does.
#[tokio::test]
async fn a_queued_message_edited_while_the_merge_is_prepared_stops_it() {
    let replies: MergeReplies = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        "thread-merge-queue-edit",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.input.contains("Add notes and merge it.") {
                    commit_in_workspace(&turn, "NOTES.md", "notes\n", "Add notes");
                    assert!(!finish(&turn, "merge it", &log).await);
                    return Reply::text("You wrote again, so I didn't merge.");
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    let item = flow
        .core
        .enqueue(
            &flow.conversation,
            "Later: rename the flag".into(),
            Vec::new(),
            Vec::new(),
            None,
            false,
        )
        .await
        .unwrap();
    let (reached, release) = (
        Arc::new(tokio::sync::Notify::new()),
        Arc::new(tokio::sync::Notify::new()),
    );
    *flow
        .manager
        .conv(&flow.conversation)
        .unwrap()
        .merge_pause
        .lock()
        .unwrap() = Some((reached.clone(), release.clone()));
    // Sent after the item was queued: the queued item is older than the consent.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    flow.manager
        .send_message(
            flow.conversation.clone(),
            "Add notes and merge it.".into(),
            Vec::new(),
            Vec::new(),
            true,
            None,
        )
        .await
        .unwrap();
    reached.notified().await;
    flow.manager
        .edit_queued(
            &flow.conversation,
            &item.id,
            "wait, don't merge".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .unwrap();
    release.notify_one();
    flow.until("the refused merge", |_| !replies.lock().unwrap().is_empty())
        .await;
    let replies = replies.lock().unwrap().clone();
    assert!(replies[0].0.contains("wrote again"), "{replies:#?}");
    assert!(!on_main(&flow, "NOTES.md"), "nothing merged");
    flow.stop().await;
}

/// The lead picks a worker's effort per task (THREAD-UX-PLAN.md §4.1 b): `delegate_task`'s
/// `effort` is the worker's route, and its CLI starts at it.
#[tokio::test]
async fn the_effort_the_lead_picks_is_the_workers() {
    let flow = Flow::start(
        "thread-effort",
        Options::default(),
        script(|turn| async move {
            if turn
                .prompt
                .contains(crate::manager::prompts::THREAD_OPENING)
            {
                if turn.earlier == 0 {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"title": "Rename the label", "kind": "scout",
                                   "spec": "Find the label.", "effort": "lots"}),
                        )
                        .await;
                    assert!(reply.is_error, "{}", reply.text);
                    assert!(reply.text.contains("unknown effort"), "{}", reply.text);
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"title": "Rename the label", "kind": "scout",
                                   "spec": "Find the label.", "effort": "medium"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                return Reply::text("Found it.");
            }
            let reply = turn
                .call("submit_report", json!({"summary": "It is in README.md."}))
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        }),
    )
    .await;
    flow.say("Rename the label.").await;
    flow.settled().await;
    let board = flow.board().await;
    let task = Flow::task(&board, 1);
    assert_eq!(task.route.choice.effort.as_deref(), Some("medium"));
    let worker = flow
        .specs
        .lock()
        .unwrap()
        .iter()
        .find(|(_, spec)| {
            !spec
                .append_system_prompt
                .as_deref()
                .is_some_and(|prompt| prompt.contains(crate::manager::prompts::THREAD_OPENING))
        })
        .map(|(_, spec)| spec.effort.clone())
        .expect("the worker's CLI started");
    assert_eq!(worker.as_deref(), Some("medium"));
    flow.stop().await;
}

/// The orchestrator's control actions are rows: message_worker keeps the text it sent, and
/// stop_worker needs a one-line reason, which its "Stopped" row keeps.
#[tokio::test]
async fn messaging_and_stopping_a_worker_are_rows_with_their_text_and_reason() {
    let started = Arc::new(tokio::sync::Notify::new());
    let worker_started = started.clone();
    let flow = Flow::start(
        "control-rows",
        super::Options::default(),
        script(move |turn| {
            let started = worker_started.clone();
            async move {
                if !turn.is_orchestrator() {
                    started.notify_one();
                    std::future::pending::<()>().await;
                    return Reply::text("");
                }
                if !turn.input.contains("Scout the uploads.") {
                    return Reply::text("[quiet]");
                }
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "high", "title": "Fix uploads", "kind": "scout",
                               "spec": "Find why uploads fail."}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                started.notified().await;
                let reply = turn
                    .call(
                        "message_worker",
                        json!({"task": "task-1", "text": "Look at the retry path first."}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                let reply = turn
                    .call("stop_worker", json!({"task": "task-1", "reason": "  "}))
                    .await;
                assert!(reply.is_error, "{}", reply.text);
                assert!(
                    reply.text.contains("stop_worker needs a `reason`"),
                    "{}",
                    reply.text
                );
                let reply = turn
                    .call(
                        "stop_worker",
                        json!({"task": "task-1",
                               "reason": "No longer needed: the user dropped uploads"}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Stopped it.")
            }
        }),
    )
    .await;
    flow.say("Scout the uploads.").await;
    let board = flow.settled().await;
    let task = Flow::task(&board, 1).clone();
    assert_eq!(task.state, crate::work::TaskState::Stopped);
    let steps: Vec<OrchestratorStepKind> = flow
        .events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            crate::model::DomainEvent::OrchestratorStepped { step } => match step.kind {
                kind @ (OrchestratorStepKind::Messaged { .. }
                | OrchestratorStepKind::Stopped { .. }) => Some(kind),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        steps,
        vec![
            OrchestratorStepKind::Messaged {
                task_id: task.id.clone(),
                text: Some("Look at the retry path first.".into()),
            },
            OrchestratorStepKind::Stopped {
                task_id: task.id.clone(),
                reason: "No longer needed: the user dropped uploads".into(),
            },
        ]
    );
    flow.stop().await;
}

/// The user's Stop all stops every running worker, files a "Stopped" row for each, and the
/// orchestrator hears one note naming them all with the user's next message.
#[tokio::test]
async fn stop_all_stops_every_running_worker_and_tells_the_orchestrator_once() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let heard = inputs.clone();
    let flow = Flow::start(
        "stop-all",
        super::Options::default(),
        script(move |turn| {
            let heard = heard.clone();
            async move {
                if !turn.is_orchestrator() {
                    std::future::pending::<()>().await;
                    return Reply::text("");
                }
                heard.lock().unwrap().push(turn.input.clone());
                if !turn.input.contains("Scout both.") {
                    return Reply::text("Fine.");
                }
                for title in ["Fix uploads", "Thread rows"] {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": title, "kind": "scout", "spec": "Look around."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                }
                Reply::text("[quiet]")
            }
        }),
    )
    .await;
    flow.say("Scout both.").await;
    let board = flow
        .until("both workers to run", |board| {
            board.tasks.len() == 2
                && board
                    .tasks
                    .values()
                    .all(|task| task.state == crate::work::TaskState::Running)
        })
        .await;
    let (first, second) = (
        Flow::task(&board, 1).id.clone(),
        Flow::task(&board, 2).id.clone(),
    );
    // A look at the board from before they ended, as a stop racing their finish would have.
    let stale = Flow::task(&board, 1).clone();
    let stopped = flow
        .manager
        .stop_workers(flow.conversation.clone())
        .await
        .unwrap();
    assert_eq!(stopped, vec![first.clone(), second.clone()]);
    let board = flow.settled().await;
    for id in [&first, &second] {
        assert_eq!(board.tasks[id].state, crate::work::TaskState::Stopped);
    }
    // Stopping it again from that old look stops nothing and files no second row.
    assert!(
        !flow
            .manager
            .stop_worker(&flow.conversation, &stale, "Again".into())
            .await
            .unwrap()
    );
    let steps: Vec<OrchestratorStepKind> = flow
        .events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            crate::model::DomainEvent::OrchestratorStepped { step } => {
                matches!(step.kind, OrchestratorStepKind::Stopped { .. }).then_some(step.kind)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        steps,
        [&first, &second]
            .into_iter()
            .map(|id| OrchestratorStepKind::Stopped {
                task_id: id.clone(),
                reason: "Stopped by the user".into(),
            })
            .collect::<Vec<_>>()
    );
    // Nothing more to stop.
    assert!(
        flow.manager
            .stop_workers(flow.conversation.clone())
            .await
            .unwrap()
            .is_empty()
    );
    flow.say("What now?").await;
    flow.settled().await;
    let inputs = inputs.lock().unwrap().clone();
    let last = inputs.last().unwrap();
    assert!(last.contains("What now?"), "{last}");
    let note = "[worker] The user stopped all workers: task-1 \"Fix uploads\" and task-2 \
                \"Thread rows\". Don't start them again unless the user asks.";
    assert!(last.contains(note), "{last}");
    let told: usize = inputs
        .iter()
        .map(|input| input.matches("The user stopped all workers").count())
        .sum();
    assert_eq!(told, 1, "{inputs:?}");
    flow.stop().await;
}

/// When the sandbox stops what the user asked for, the thread shows a notice once per request,
/// with its reason, under the request it serves; at Full access it is refused. Changing the
/// default level leaves a started session's own level alone.
#[tokio::test]
async fn the_thread_suggests_full_access_once_and_never_at_full_access() {
    let replies: Arc<Mutex<Vec<(bool, String)>>> = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        "thread-suggest-full-access",
        Options {
            permission: PermissionLevel::AskForApproval,
            ..Options::default()
        },
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.is_orchestrator() {
                    let call = || {
                        turn.call(
                            "suggest_full_access",
                            json!({"reason": "Installing Homebrew writes outside the project."}),
                        )
                    };
                    let (first, second) = tokio::join!(call(), call());
                    for reply in [first, second] {
                        log.lock().unwrap().push((reply.is_error, reply.text));
                    }
                }
                Reply::text("It needs Full access.")
            }
        }),
    )
    .await;
    flow.say("Install Homebrew.").await;
    let board = flow.settled().await;
    {
        let replies = replies.lock().unwrap();
        assert_eq!(
            replies.iter().filter(|reply| !reply.0).count(),
            1,
            "{replies:?}"
        );
        assert_eq!(
            replies
                .iter()
                .filter(|reply| reply.0 && reply.1.contains("already shown"))
                .count(),
            1,
            "{replies:?}"
        );
    }
    let shown: Vec<_> = board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            OrchestratorStepKind::FullAccessSuggested { reason } => {
                Some((reason.clone(), step.request_id.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(
        shown[0].0,
        "Installing Homebrew writes outside the project."
    );
    assert!(shown[0].1.is_some(), "under the user's request");

    // A new default leaves this session at its own level.
    let mut settings = flow.core.settings();
    settings.default_permission = PermissionLevel::FullAccess;
    flow.core.update_settings(settings).await.unwrap();
    let Some(Setup::Session { permission, .. }) =
        flow.core.conversation(&flow.conversation).unwrap().setup
    else {
        panic!("a session");
    };
    assert_eq!(permission, PermissionLevel::AskForApproval);

    // The user switches the session to Full access: there is nothing left to suggest.
    let Some(Setup::Session {
        repo,
        environment,
        orchestrator,
        workers_see_uncommitted,
        plan_mode,
        ..
    }) = flow.core.conversation(&flow.conversation).unwrap().setup
    else {
        panic!("a session");
    };
    flow.manager
        .set_setup(
            flow.conversation.clone(),
            Setup::Session {
                repo,
                environment,
                permission: PermissionLevel::FullAccess,
                orchestrator,
                workers_see_uncommitted,
                plan_mode,
            },
        )
        .await
        .unwrap();
    flow.say("Install it now.").await;
    flow.settled().await;
    {
        let replies = replies.lock().unwrap();
        assert!(
            replies[2].0 && replies[2].1.contains("already has Full access"),
            "{replies:?}"
        );
    }
    flow.stop().await;
}
