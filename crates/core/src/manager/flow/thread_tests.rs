//! The session's thread (THREAD-PLAN.md Q1, Q6): its tools, workspace and access, approvals
//! under its permission level, and a restart for a new workspace or level.

use std::sync::{Arc, Mutex};

use brigadier_providers::model::{Access, Origin, ToolSet};
use brigadier_providers::{ApprovalDecision, Artifact, ProviderKind};

use serde_json::json;

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
fn commit_in_workspace(turn: &Turn, file: &str, text: &str, message: &str) -> String {
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
                        let deadline =
                            std::time::Instant::now() + std::time::Duration::from_secs(30);
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
                                json!({"title": "Add a greeting", "kind": "implement",
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

/// The thread commits and asks for the merge in the same turn: the review of its commit has
/// started (for the thread, with no task) before the merge card opens, so the card's review
/// line counts it, "Review running…" until it ends and its findings after.
#[tokio::test]
async fn the_merge_card_counts_the_review_of_the_threads_own_commit() {
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
                let reply = turn.call("finish_session", json!({})).await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("[quiet]");
            }
            Reply::text("Noted.")
        }),
    )
    .await;
    flow.say("Add notes and merge.").await;
    let board = flow
        .until("the merge card", |board| {
            board
                .approvals
                .values()
                .any(|card| matches!(card.subject, ApprovalSubject::FinishSession { .. }))
        })
        .await;
    let card = board.approvals.values().next().unwrap().clone();
    let reviews: Vec<_> = board.reviews.values().cloned().collect();
    assert_eq!(reviews.len(), 1, "{reviews:#?}");
    let review = &reviews[0];
    assert!(review.task_id.is_none(), "the thread's own commit");
    assert_eq!(review.kind, crate::work::ReviewKind::Code);
    assert_eq!(review.notify, crate::work::ReviewFor::Orchestrator);
    assert_eq!(review.state, crate::work::ReviewState::Running);
    assert!(
        review.started_at_ms <= card.created_at_ms,
        "started before the card, so the card speaks for it"
    );
    release.notify_one();
    let reviews = code_reviews(&flow, 1).await;
    assert_eq!(
        reviews[0].state,
        crate::work::ReviewState::Findings { count: 1 }
    );
    flow.stop().await;
}

/// A thread whose CLI started on the instructions from before the thread's (an older contract
/// in its log) isn't resumed, since it would keep its old role: it starts over from the
/// transcript with the thread's instructions.
#[tokio::test]
async fn a_thread_started_on_older_instructions_starts_over_instead_of_resuming() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let mut flow = Flow::start(
        "thread-old-role",
        Options::default(),
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
    // What a build before the thread logged when that CLI started.
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
                            contract: Some(1),
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
