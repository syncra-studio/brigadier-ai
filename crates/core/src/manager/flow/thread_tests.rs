//! The session's thread (THREAD-PLAN.md Q1, Q6): its tools, workspace and access, approvals
//! under its permission level, and a restart for a new workspace or level.

use std::sync::{Arc, Mutex};

use brigadier_providers::model::{Access, Origin, SessionSpec, ToolSet};
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

/// At each level and for both vendors, the thread starts with its own tools in its workspace
/// (the session worktree, made before the CLI starts, #523) and a worker's access for that
/// level; its scratch folder stays its sweep folder. Its environment sets only its temporary
/// folder: the CLI's own shell timeouts, output limits and background tasks hold (S2).
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
                    // The workspace is there when the thread starts, and it starts in it.
                    assert!(
                        turn.cwd.join("README.md").is_file(),
                        "{}",
                        turn.cwd.display()
                    );
                    log.lock().unwrap().push(turn.cwd.clone());
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
        assert!(spec.add_dirs.is_empty(), "{:?}", spec.add_dirs);
        assert_eq!(spec.cwd, worktree);
        assert!(spec.owned_cwd, "a New worktree is Brigadier's");
        let scratch = spec.sweep_dir.clone().expect("its sweep folder");
        assert!(scratch.ends_with(format!("orch/{}", flow.conversation.0)));
        let env: std::collections::HashMap<_, _> = spec.env.iter().cloned().collect();
        // Its temporary files go to its short folder under /tmp, never the data folder's.
        let tmp = crate::manager::workers::session_temp_dir(&flow.conversation);
        assert_eq!(env.get("TMPDIR").map(String::as_str), tmp.to_str());
        assert!(tmp.is_dir());
        for name in [
            "BASH_MAX_OUTPUT_LENGTH",
            "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS",
            "BASH_DEFAULT_TIMEOUT_MS",
            "BASH_MAX_TIMEOUT_MS",
        ] {
            assert!(!env.contains_key(name), "{name}");
        }
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
                assert!(writable_roots.contains(&scratch));
                assert!(writable_roots.contains(&tmp));
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

/// The user's Deny with no reason still tells the thread why (an empty one fails its whole
/// turn at the API), and the thread goes on to say it was declined: the request ends Done
/// with an answer, never Failed.
#[tokio::test]
async fn a_deny_without_a_reason_tells_the_thread_and_the_turn_goes_on() {
    let answered: Arc<Mutex<Option<ApprovalDecision>>> = Arc::default();
    let log = answered.clone();
    let flow = Flow::start(
        "thread-deny",
        Options {
            permission: PermissionLevel::AskForApproval,
            ..Options::default()
        },
        script(move |turn| {
            let log = log.clone();
            async move {
                let decision = turn.ask_approval("brew install cowsay", "brew").await;
                *log.lock().unwrap() = decision;
                Reply::text("You declined installing cowsay, so I didn't run it.")
            }
        }),
    )
    .await;
    flow.say("Install cowsay.").await;
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
    flow.manager
        .answer_card(
            flow.conversation.clone(),
            card.id.clone(),
            ApprovalDecision::Deny {
                message: String::new(),
            },
        )
        .await
        .unwrap();
    let board = flow.settled().await;
    assert_eq!(
        *answered.lock().unwrap(),
        Some(ApprovalDecision::Deny {
            message: brigadier_providers::DECLINED.into()
        })
    );
    // The card keeps what the user said: nothing.
    assert_eq!(
        board.approvals[&card.id].state,
        CardState::Denied {
            by: brigadier_providers::Decider::User,
            message: None
        }
    );
    let request = board.requests.values().next().unwrap();
    assert_eq!(request.state, crate::work::RequestState::Done);
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
                let workspace = turn.cwd.clone();
                let outside = workspace.parent().unwrap().join("outside.txt");
                for paths in [
                    vec![workspace.join("a.txt")],
                    vec![workspace.join("new/b.txt"), workspace.join("c.txt")],
                    vec![outside.clone()],
                    vec![workspace.join("d.txt"), outside.clone()],
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
                Some(&workspace),
                false
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
                Some(&flow.repo),
                false
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
                    .push((turn.input.clone(), vec![turn.cwd.clone()]));
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
    assert_eq!(specs[0].1.cwd, first);
    assert_eq!(specs[1].1.cwd, moved);
    assert_eq!(specs[1].1.access, Access::Full);
    assert!(matches!(specs[2].1.access, Access::Scoped { .. }));
    assert!(specs[2].1.auto_review);

    let inputs = inputs.lock().unwrap().clone();
    // Nothing is told mid-session (G1): the CLI starts with the new folder and access.
    for (input, _) in &inputs {
        assert!(
            !input.contains("[workspace]") && !input.contains("[settings]"),
            "{input}"
        );
    }
    assert_eq!(inputs[1].1, vec![moved]);
    flow.stop().await;
}

/// Notes of 150 lines: a big change.
pub(super) fn reviewed_notes() -> String {
    (1..=150).map(|n| format!("note {n}\n")).collect()
}

/// Commits a file in the thread's workspace, as the thread's own tiny edit would.
pub(super) fn commit_in_workspace(turn: &Turn, file: &str, text: &str, message: &str) -> String {
    let workspace = &turn.cwd;
    std::fs::write(workspace.join(file), text).unwrap();
    git(workspace, &["add", file]);
    git(workspace, &["commit", "-q", "-m", message]);
    git(workspace, &["rev-parse", "HEAD"])
}

/// The script of the merge tests below: commits NOTES.md on "Add notes.", merges on "yes,
/// merge it", and logs every input with its folders and finish_session's answer.
fn merging_thread(seen: Arc<Mutex<Vec<Seen>>>) -> Script {
    script(move |turn| {
        let log = seen.clone();
        async move {
            log.lock()
                .unwrap()
                .push((turn.input.clone(), vec![turn.cwd.clone()]));
            if turn.input.contains("Add notes.") {
                commit_in_workspace(
                    &turn,
                    "NOTES.md",
                    "notes\n",
                    "Add notes\n\nBrigadier-Author: thread",
                );
                return Reply::text("Notes added, ready to merge.");
            }
            if turn.input.contains("yes, merge it") {
                let reply = turn.call("finish_session", json!({})).await;
                assert!(!reply.is_error, "{}", reply.text);
                log.lock().unwrap().push((reply.text.clone(), Vec::new()));
                return Reply::text("Merged.");
            }
            Reply::text("Done.")
        }
    })
}

/// finish_session's answer, once the thread has it.
async fn finished(flow: &Flow, seen: &Arc<Mutex<Vec<Seen>>>) -> String {
    flow.until("the [finished] answer", |_| {
        seen.lock()
            .unwrap()
            .iter()
            .any(|(input, _)| input.contains("[finished]"))
    })
    .await;
    flow.settled().await;
    seen.lock()
        .unwrap()
        .iter()
        .find(|(input, _)| input.contains("[finished]"))
        .unwrap()
        .0
        .clone()
}

/// After the merge the user asked for, the session keeps its worktree and branch (BUILD-PLAN
/// A3): the branch is at the merge, and the user's next message goes on in the same folder, in
/// the same CLI session, with nothing made again.
#[tokio::test]
async fn a_merge_keeps_the_session_worktree_and_its_branch_moves_to_the_merge() {
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let flow = Flow::start(
        "thread-merge-kept",
        Options::default(),
        merging_thread(seen.clone()),
    )
    .await;
    flow.say("Add notes.").await;
    flow.settled().await;
    let worktree = session_worktree(&flow);
    let branch = git(&worktree, &["rev-parse", "--abbrev-ref", "HEAD"]);
    flow.say("yes, merge it").await;
    let finished = finished(&flow, &seen).await;
    assert!(
        finished.contains("The session keeps its worktree and branch"),
        "{finished}"
    );
    assert!(git(&flow.repo, &["cat-file", "-e", "main:NOTES.md"]).is_empty());
    assert!(worktree.join(".git").exists(), "{}", worktree.display());
    let main_tip = git(&flow.repo, &["rev-parse", "main"]);
    assert_eq!(git(&flow.repo, &["rev-parse", &branch]), main_tip);
    assert_eq!(git(&worktree, &["rev-parse", "HEAD"]), main_tip);
    assert_eq!(session_worktree(&flow), worktree);
    let specs = flow.thread_specs().len();
    flow.say("Next.").await;
    flow.settled().await;
    assert_eq!(session_worktree(&flow), worktree, "the same path");
    assert_eq!(
        git(&worktree, &["rev-parse", "--abbrev-ref", "HEAD"]),
        branch
    );
    let (input, dirs) = seen.lock().unwrap().last().unwrap().clone();
    assert!(input.contains("Next."), "{input}");
    assert!(!input.contains("[workspace] A fresh branch"), "{input}");
    assert_eq!(dirs[0], worktree);
    assert_eq!(
        flow.thread_specs().len(),
        specs,
        "the same CLI session goes on: no restart, no rebirth"
    );
    flow.stop().await;
}

/// Uncommitted changes in the session's worktree stay through a merge, with the branch at the
/// merge.
#[tokio::test]
async fn a_merge_keeps_uncommitted_changes_in_the_session_worktree() {
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let flow = Flow::start(
        "thread-merge-dirty",
        Options::default(),
        merging_thread(seen.clone()),
    )
    .await;
    flow.say("Add notes.").await;
    flow.settled().await;
    let worktree = session_worktree(&flow);
    std::fs::write(worktree.join("DRAFT.md"), "draft\n").unwrap();
    let branch = git(&worktree, &["rev-parse", "--abbrev-ref", "HEAD"]);
    flow.say("yes, merge it").await;
    let finished = finished(&flow, &seen).await;
    assert!(
        finished.contains("The session keeps its worktree and branch"),
        "{finished}"
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("DRAFT.md")).unwrap(),
        "draft\n"
    );
    assert_eq!(
        git(&flow.repo, &["rev-parse", &branch]),
        git(&flow.repo, &["rev-parse", "main"])
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
                    kept: None,
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

/// A call whose failure comes with the user's Stop (the CLI reports it failed because its
/// process was killed) was stopped: its row is marked so and reads stopped, not failed, for a
/// Claude tool call and a Codex command alike, its end kept as it came. One that failed before
/// the Stop still reads failed.
#[tokio::test]
async fn a_call_failing_with_the_stop_reads_stopped_not_failed() {
    use brigadier_providers::{ItemStatus, ProviderEvent};
    let bash = |id: &str, status: ItemStatus, output: Option<&str>| ProviderEvent::ToolCall {
        item_id: id.into(),
        name: "Bash".into(),
        input: Some(json!({ "command": "npm test" }).to_string()),
        status,
        output: output.map(str::to_owned),
        kept: None,
    };
    let command = |status: ItemStatus, exit_code: Option<i32>| ProviderEvent::Command {
        item_id: "cmd-1".into(),
        command: "cargo build".into(),
        cwd: None,
        status,
        exit_code,
        output: None,
        duration_ms: None,
        kept: None,
    };
    let flow = Flow::start(
        "thread-stop-failed-call",
        Options::default(),
        script(move |turn| async move {
            if turn.input.contains("Run the tests.") {
                turn.emit(bash("bash-0", ItemStatus::InProgress, None))
                    .await;
                turn.emit(bash(
                    "bash-0",
                    ItemStatus::Failed,
                    Some("Exit code 1\nfailed"),
                ))
                .await;
                turn.emit(bash("bash-1", ItemStatus::InProgress, None))
                    .await;
                turn.emit(command(ItemStatus::InProgress, None)).await;
                assert!(turn.stopped().await, "the user stops it");
                turn.emit(bash("bash-1", ItemStatus::Failed, Some("Exit code 143\n")))
                    .await;
                turn.emit(command(ItemStatus::Failed, Some(143))).await;
                return Reply::text("");
            }
            Reply::text("Done.")
        }),
    )
    .await;
    let row = |board: &crate::board::Board, id: &str| {
        board
            .orchestrator_steps
            .iter()
            .find_map(|step| match &step.kind {
                OrchestratorStepKind::Tool {
                    item_id,
                    status,
                    stopped,
                    exit,
                    ..
                } if item_id == id => Some((*status, *stopped, *exit)),
                _ => None,
            })
    };
    flow.say("Run the tests.").await;
    flow.until("the calls to run", |board| {
        row(board, "bash-0").is_some_and(|(status, ..)| status == ItemStatus::Failed)
            && row(board, "cmd-1").is_some()
    })
    .await;
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    let board = flow.settled().await;
    assert_eq!(
        row(&board, "bash-0"),
        Some((ItemStatus::Failed, false, Some(1)))
    );
    assert_eq!(
        row(&board, "bash-1"),
        Some((ItemStatus::Failed, true, Some(143)))
    );
    assert_eq!(
        row(&board, "cmd-1"),
        Some((ItemStatus::Failed, true, Some(143)))
    );
    flow.stop().await;
}

/// A step left running under a reply the user stopped (the quit came before the Stop closed
/// it) reads stopped after a restart, not failed; one left under a reply that ended otherwise
/// still reads failed (#184).
#[tokio::test]
async fn a_step_left_running_under_a_stopped_reply_reads_stopped_after_a_restart() {
    use brigadier_providers::{ItemStatus, ProviderEvent};
    let mut flow = Flow::start(
        "20261017-0213-issues-run-w49-stopped-restart",
        Options::default(),
        script(move |turn| async move {
            if turn.input.contains("Run the tests.") {
                turn.emit(ProviderEvent::ToolCall {
                    item_id: "bash-0".into(),
                    name: "Bash".into(),
                    input: Some(json!({ "command": "npm test" }).to_string()),
                    status: ItemStatus::InProgress,
                    output: None,
                    kept: None,
                })
                .await;
                assert!(turn.stopped().await, "the user stops it");
                return Reply::text("");
            }
            Reply::text("Done.")
        }),
    )
    .await;
    let row = |board: &crate::board::Board, id: &str| {
        board
            .orchestrator_steps
            .iter()
            .find_map(|step| match &step.kind {
                OrchestratorStepKind::Tool {
                    item_id,
                    status,
                    stopped,
                    ..
                } if item_id == id => Some((*status, *stopped)),
                _ => None,
            })
    };
    flow.say("Hello.").await;
    let done = flow.settled().await.latest_request().unwrap().id.clone();
    flow.say("Run the tests.").await;
    flow.until("the call to run", |board| row(board, "bash-0").is_some())
        .await;
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    let board = flow.settled().await;
    let stopped = board.latest_request().unwrap().id.clone();
    // The Stop closes its turn's calls itself (#61).
    assert_eq!(row(&board, "bash-0"), Some((ItemStatus::Failed, true)));
    // What a quit before that close leaves: a row still running under each reply.
    let step = board
        .orchestrator_steps
        .iter()
        .find(|step| matches!(&step.kind, OrchestratorStepKind::Tool { item_id, .. } if item_id == "bash-0"))
        .unwrap();
    for (id, request) in [("bash-1", &stopped), ("bash-2", &done)] {
        let mut kind = step.kind.clone();
        if let OrchestratorStepKind::Tool {
            item_id,
            status,
            stopped,
            ended_at_ms,
            turn,
            ..
        } = &mut kind
        {
            *item_id = id.into();
            *status = ItemStatus::InProgress;
            *stopped = false;
            *ended_at_ms = None;
            *turn = None;
        }
        flow.manager
            .orchestrator_step_in(&flow.conversation, Some(request.clone()), kind)
            .await;
    }
    flow.restart().await;
    let board = flow
        .until("the cut-off rows to end", |board| {
            [row(board, "bash-1"), row(board, "bash-2")]
                .iter()
                .all(|row| row.is_some_and(|(status, _)| status != ItemStatus::InProgress))
        })
        .await;
    assert_eq!(row(&board, "bash-1"), Some((ItemStatus::Failed, true)));
    assert_eq!(row(&board, "bash-2"), Some((ItemStatus::Failed, false)));
    flow.stop().await;
}

/// A preview the thread started holds nothing up: its answer is done while the preview runs.
#[tokio::test]
async fn a_running_preview_leaves_the_answer_done() {
    let flow = Flow::start(
        "thread-background-work",
        Options::default(),
        script(move |turn| async move {
            if turn.input.contains("Start the site.") {
                let reply = turn
                    .call(
                        "start_preview",
                        json!({ "command": "echo up; sleep 600", "name": "site" }),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("The site runs.");
            }
            Reply::text("Noted.")
        }),
    )
    .await;
    flow.say("Start the site.").await;
    let board = flow
        .until("the answer done, the preview running", |board| {
            board
                .requests
                .values()
                .all(|request| request.state == crate::work::RequestState::Done)
                && board
                    .previews
                    .values()
                    .any(|preview| preview.state.is_running())
        })
        .await;
    let first = board.requests.values().next().unwrap().clone();
    assert!(
        first.worked.iter().all(|span| span.to_ms.is_some()),
        "Worked for stops: {:?}",
        first.worked
    );
    assert!(board.tasks.is_empty(), "no worker: {:?}", board.tasks);
    flow.manager
        .stop_preview(flow.conversation.clone(), None)
        .await
        .unwrap();
    flow.stop().await;
}

/// What the thread was told by its finish_session calls, in order.
type MergeReplies = Arc<Mutex<Vec<(String, bool)>>>;

async fn finish(turn: &Turn, replies: &MergeReplies) -> bool {
    let reply = turn.call("finish_session", json!({})).await;
    replies
        .lock()
        .unwrap()
        .push((reply.text.clone(), reply.is_error));
    !reply.is_error
}

pub(super) fn on_main(flow: &Flow, path: &str) -> bool {
    crate::test_git_command()
        .args(["cat-file", "-e", &format!("main:{path}")])
        .current_dir(&flow.repo)
        .status()
        .is_ok_and(|status| status.success())
}

/// Merging is asked for in words, with no card: the thread proposes it in its reply, judges
/// the user's answers ("No, not yet." and "don't merge it" aren't a yes), and merges on their
/// "yes, merge it", once: Brigadier checks no consent, and a second call finds nothing left to
/// merge.
#[tokio::test]
async fn the_merge_is_asked_in_words_and_happens_on_the_users_yes() {
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
                    return Reply::text("Notes added. Merge them into `main`?");
                }
                if input.contains("No, not yet.") {
                    return Reply::text("OK, it stays on its branch. Merge it into `main` now?");
                }
                if input.contains("don't merge it") {
                    return Reply::text("Left unmerged.");
                }
                if input.contains("yes, merge it") {
                    assert!(finish(&turn, &log).await);
                    // One merge: nothing is left to merge.
                    assert!(!finish(&turn, &log).await);
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
    assert_eq!(replies.len(), 2, "{replies:#?}");
    assert!(
        replies[0].0.starts_with("[finished] `") && !replies[0].1,
        "{replies:#?}"
    );
    assert!(
        replies[1].1 && replies[1].0.contains("has no commits that `main` lacks"),
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
                    return Reply::text("");
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
                    return Reply::text("");
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
                Reply::text("")
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
                \"Thread rows\". You didn't stop them and nothing failed. Don't message, steer \
                or approve them, and don't start them again unless the user asks.";
    assert!(last.contains(note), "{last}");
    let told: usize = inputs
        .iter()
        .map(|input| input.matches("The user stopped all workers").count())
        .sum();
    assert_eq!(told, 1, "{inputs:?}");
    flow.stop().await;
}

/// The user stops a worker while the thread is idle and the request waits on that worker:
/// the thread gets a turn on the stop, so the request ends on its word, not silently (#65).
#[tokio::test]
async fn stopping_a_worker_while_the_thread_is_idle_gets_the_request_an_answer() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let heard = inputs.clone();
    let flow = Flow::start(
        "stop-while-idle",
        super::Options::default(),
        script(move |turn| {
            let heard = heard.clone();
            async move {
                if !turn.is_orchestrator() {
                    std::future::pending::<()>().await;
                    return Reply::text("");
                }
                heard.lock().unwrap().push(turn.input.clone());
                if turn.input.contains("Scout it.") {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Fix uploads", "kind": "scout", "spec": "Look around."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("");
                }
                if turn.input.contains("The user stopped task-1") {
                    return Reply::text("Stopped the scout before it found anything.");
                }
                Reply::text("Fine.")
            }
        }),
    )
    .await;
    flow.say("Scout it.").await;
    let board = flow
        .until("the worker to run", |board| {
            board.tasks.len() == 1
                && board
                    .tasks
                    .values()
                    .all(|task| task.state == crate::work::TaskState::Running)
        })
        .await;
    let request = board.requests.values().next().unwrap().id.clone();
    flow.manager
        .stop_task_by_user(Flow::task(&board, 1).id.clone())
        .await
        .unwrap();
    let said = inputs.clone();
    flow.until("the thread to hear the stop", move |_| {
        said.lock()
            .unwrap()
            .iter()
            .any(|input| input.contains("The user stopped task-1"))
    })
    .await;
    let board = flow.settled().await;
    assert_eq!(
        board.requests[&request].state,
        crate::work::RequestState::Done
    );
    let messages = flow.core.all_messages(&flow.conversation).await.unwrap();
    assert!(
        messages.iter().any(|message| {
            message.text == "Stopped the scout before it found anything."
                && message.request_id.as_deref() == Some(request.as_str())
        }),
        "{messages:#?}"
    );
    // Told once: the user's next message doesn't carry it again.
    flow.say("What now?").await;
    flow.settled().await;
    let inputs = inputs.lock().unwrap().clone();
    let told = inputs
        .iter()
        .filter(|input| input.contains("The user stopped task-1"))
        .count();
    assert_eq!(told, 1, "{inputs:#?}");
    flow.stop().await;
}

/// The composer's Stop while the thread is idle: the app stops the session, then each of its
/// workers. The request asked to stop; no turn starts on the workers' stop (#65 review).
#[tokio::test]
async fn the_composers_stop_of_an_idle_thread_starts_no_turn_on_its_workers_stop() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let heard = inputs.clone();
    let flow = Flow::start(
        "stop-session-while-idle",
        super::Options::default(),
        script(move |turn| {
            let heard = heard.clone();
            async move {
                if !turn.is_orchestrator() {
                    std::future::pending::<()>().await;
                    return Reply::text("");
                }
                heard.lock().unwrap().push(turn.input.clone());
                if turn.input.contains("Scout it.") {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Fix uploads", "kind": "scout", "spec": "Look around."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("");
                }
                Reply::text("Fine.")
            }
        }),
    )
    .await;
    flow.say("Scout it.").await;
    let board = flow
        .until("the worker to run", |board| {
            board.tasks.len() == 1
                && board
                    .tasks
                    .values()
                    .all(|task| task.state == crate::work::TaskState::Running)
        })
        .await;
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    flow.manager
        .stop_task_by_user(Flow::task(&board, 1).id.clone())
        .await
        .unwrap();
    flow.settled().await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let inputs = inputs.lock().unwrap().clone();
    assert_eq!(inputs.len(), 1, "{inputs:#?}");
    flow.stop().await;
}

/// The user's stops reach the thread inside its running turn (UI audit H2): one worker's Stop,
/// then Stop all, each steered in as it happens. From then on the thread can't message, answer
/// or approve a worker the user stopped, and `list_tasks` says who stopped it.
#[tokio::test]
async fn the_users_stop_reaches_the_running_turn_and_the_stopped_worker_is_refused() {
    let heard: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = heard.clone();
    let flow = Flow::start(
        "stop-into-turn",
        super::Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if !turn.is_orchestrator() {
                    std::future::pending::<()>().await;
                    return Reply::text("");
                }
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
                for _ in 0..2 {
                    let note = turn.steered().await.expect("told of the stop in this turn");
                    log.lock().unwrap().push(note);
                }
                for (tool, args) in [
                    ("message_worker", json!({"task": "task-1", "text": "Keep going."})),
                    ("answer_worker", json!({"task": "task-2", "answer": "Yes.", "why": "It asked."})),
                    ("approve_outline", json!({"task": "task-2"})),
                ] {
                    let reply = turn.call(tool, args).await;
                    assert!(reply.is_error, "{tool}: {}", reply.text);
                    log.lock().unwrap().push(reply.text);
                }
                let reply = turn.call("list_tasks", json!({})).await;
                log.lock().unwrap().push(reply.text);
                Reply::text("Both stopped.")
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
    let first = Flow::task(&board, 1).id.clone();
    flow.manager.stop_task_by_user(first.clone()).await.unwrap();
    flow.until("the first to stop", |board| {
        board.tasks[&first].state == crate::work::TaskState::Stopped
    })
    .await;
    let stopped = flow
        .manager
        .stop_workers(flow.conversation.clone())
        .await
        .unwrap();
    assert_eq!(stopped.len(), 1);
    flow.settled().await;
    let heard = heard.lock().unwrap().clone();
    assert_eq!(heard.len(), 6, "{heard:?}");
    assert_eq!(
        heard[0],
        "[worker] The user stopped task-1 \"Fix uploads\". You didn't stop it and nothing \
         failed. Don't message, steer or approve it, and don't start it again unless the user \
         asks."
    );
    assert!(
        heard[1].starts_with("[worker] The user stopped all workers: task-2 \"Thread rows\"."),
        "{}",
        heard[1]
    );
    for (refusal, number) in heard[2..5].iter().zip([1, 2, 2]) {
        assert!(
            refusal.contains(&format!("task-{number} was stopped by the user.")),
            "{refusal}"
        );
    }
    assert_eq!(
        heard[5].matches("(stopped by the user)").count(),
        2,
        "{}",
        heard[5]
    );
    // Told once each, in the turn: nothing waits for the next one.
    flow.say("What now?").await;
    flow.settled().await;
    let board = flow.board().await;
    let steps = board
        .orchestrator_steps
        .iter()
        .filter(|step| matches!(&step.kind, OrchestratorStepKind::Stopped { reason, .. } if reason == "Stopped by the user"))
        .count();
    assert_eq!(steps, 2);
    flow.stop().await;
}

/// The thread offers a drafted report to Brigadier's developers once per incident: once in a
/// request, none while the last offer is unanswered, again after the user sent it, never after
/// they dismissed one. The user's answer is kept once.
#[tokio::test]
async fn the_thread_offers_feedback_once_per_incident() {
    let replies: Arc<Mutex<Vec<(bool, String)>>> = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        "thread-offer-feedback",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.is_orchestrator() && turn.input.contains("Report this") {
                    let reply = turn
                        .call(
                            "offer_feedback",
                            json!({
                                "kind": "bug",
                                "title": "Stop leaves the tab spinning",
                                "body": "After Stop the tab keeps its spinner.",
                                "asked": true,
                            }),
                        )
                        .await;
                    log.lock().unwrap().push((reply.is_error, reply.text));
                } else if turn.is_orchestrator() {
                    let call = || {
                        turn.call(
                            "offer_feedback",
                            json!({
                                "kind": "bug",
                                "title": "Stop leaves the tab spinning",
                                "body": "After Stop the tab keeps its spinner.",
                            }),
                        )
                    };
                    let (first, second) = tokio::join!(call(), call());
                    for reply in [first, second] {
                        log.lock().unwrap().push((reply.is_error, reply.text));
                    }
                }
                Reply::text("Sorry about that.")
            }
        }),
    )
    .await;
    let taken =
        |replies: &Arc<Mutex<Vec<(bool, String)>>>| std::mem::take(&mut *replies.lock().unwrap());
    let offers = |board: &crate::board::Board| -> Vec<String> {
        board
            .orchestrator_steps
            .iter()
            .filter_map(|step| match &step.kind {
                OrchestratorStepKind::FeedbackOffered {
                    id,
                    kind,
                    title,
                    body,
                } => {
                    assert_eq!(*kind, crate::work::FeedbackKind::Bug);
                    assert_eq!(title, "Stop leaves the tab spinning");
                    assert_eq!(body, "After Stop the tab keeps its spinner.");
                    assert!(step.request_id.is_some(), "under the user's request");
                    Some(id.clone())
                }
                _ => None,
            })
            .collect()
    };

    flow.say("Stop is broken again.").await;
    let board = flow.settled().await;
    let first = taken(&replies);
    assert_eq!(
        first.iter().filter(|reply| !reply.0).count(),
        1,
        "{first:?}"
    );
    // The answer is written before the user may send it: the card, not the answer, says (#475).
    assert!(
        first
            .iter()
            .any(|reply| !reply.0 && reply.1.contains("nor say whether it is sent")),
        "{first:?}"
    );
    assert!(
        first
            .iter()
            .any(|reply| reply.0 && reply.1.contains("already offered for this request")),
        "{first:?}"
    );
    let shown = offers(&board);
    assert_eq!(shown.len(), 1);

    // Unanswered: no new offer.
    flow.say("Still broken.").await;
    let board = flow.settled().await;
    let second = taken(&replies);
    assert!(
        second
            .iter()
            .all(|reply| reply.0 && reply.1.contains("hasn't answered")),
        "{second:?}"
    );
    assert_eq!(offers(&board).len(), 1);

    // Sent: a later incident may be offered again; the answer is kept once, with the issues
    // it filed, so the card lists them after a restart too (#476). A Try again that files the
    // rest answers again, in its place.
    let filed = |number: u64| crate::feedback::FeedbackSent::Filed {
        kind: crate::work::FeedbackKind::Bug,
        title: format!("Report {number}"),
        number,
        url: format!("https://github.com/syncra-studio/brigadier-ai/issues/{number}"),
    };
    flow.manager
        .answer_feedback_offer(&flow.conversation, &shown[0], true, vec![filed(482)])
        .await
        .unwrap();
    flow.manager
        .answer_feedback_offer(
            &flow.conversation,
            &shown[0],
            true,
            vec![filed(482), filed(483)],
        )
        .await
        .unwrap();
    assert!(
        flow.manager
            .answer_feedback_offer(&flow.conversation, &shown[0], false, Vec::new())
            .await
            .is_err()
    );
    assert!(
        flow.manager
            .answer_feedback_offer(&flow.conversation, "nope", false, Vec::new())
            .await
            .is_err()
    );
    flow.say("And now the composer is stuck.").await;
    let board = flow.settled().await;
    let third = taken(&replies);
    assert_eq!(
        third.iter().filter(|reply| !reply.0).count(),
        1,
        "{third:?}"
    );
    let shown = offers(&board);
    assert_eq!(shown.len(), 2);
    let answers: Vec<_> = board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            OrchestratorStepKind::FeedbackAnswered { id, sent, results } if *id == shown[0] => {
                Some((*sent, results.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(answers, vec![(true, vec![filed(482), filed(483)])]);

    // Dismissed: none again in this session.
    flow.manager
        .answer_feedback_offer(&flow.conversation, &shown[1], false, Vec::new())
        .await
        .unwrap();
    flow.say("Ugh.").await;
    let board = flow.settled().await;
    let fourth = taken(&replies);
    assert!(
        fourth
            .iter()
            .all(|reply| reply.0 && reply.1.contains("dismissed")),
        "{fourth:?}"
    );
    assert_eq!(offers(&board).len(), 2);

    // Asked for ("report this"): offered even after a dismissal.
    flow.say("Report this to Brigadier, please.").await;
    let board = flow.settled().await;
    let fifth = taken(&replies);
    assert!(fifth.iter().all(|reply| !reply.0), "{fifth:?}");
    assert_eq!(offers(&board).len(), 3);
    flow.stop().await;
}

/// What the thread's offer_feedback calls answered: the title, an error, and its text.
type Offered = Arc<Mutex<Vec<(String, bool, String)>>>;

/// Asked to report several things at once, the thread offers a card for each (#483): up to ten
/// in a request, each title once, each answered on its own. Any card of that request still
/// unanswered, or dismissed, holds back the next unasked offer.
#[tokio::test]
async fn several_feedback_cards_are_offered_when_asked() {
    let replies: Offered = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        "thread-offer-feedback-several",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.is_orchestrator() && turn.input.contains("Report these") {
                    let titles = ["Bug 1", "bug 1"]
                        .into_iter()
                        .map(str::to_owned)
                        .chain((2..=11).map(|n| format!("Bug {n}")));
                    for title in titles {
                        let reply = turn
                            .call(
                                "offer_feedback",
                                json!({"kind": "bug", "title": title, "body": "It broke.", "asked": true}),
                            )
                            .await;
                        log.lock().unwrap().push((title, reply.is_error, reply.text));
                    }
                } else if turn.is_orchestrator() {
                    let reply = turn
                        .call(
                            "offer_feedback",
                            json!({"kind": "bug", "title": "Still broken", "body": "It broke again."}),
                        )
                        .await;
                    log.lock()
                        .unwrap()
                        .push(("Still broken".into(), reply.is_error, reply.text));
                }
                Reply::text("On it.")
            }
        }),
    )
    .await;
    let taken = |replies: &Offered| std::mem::take(&mut *replies.lock().unwrap());
    let offers = |board: &crate::board::Board| -> Vec<String> {
        board
            .orchestrator_steps
            .iter()
            .filter_map(|step| match &step.kind {
                OrchestratorStepKind::FeedbackOffered { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect()
    };

    flow.say("Report these to Brigadier: the tab spins, the composer sticks, and more.")
        .await;
    let board = flow.settled().await;
    let first = taken(&replies);
    let offered: Vec<_> = first
        .iter()
        .filter(|reply| !reply.1)
        .map(|reply| reply.0.as_str())
        .collect();
    assert_eq!(
        offered,
        (1..=10).map(|n| format!("Bug {n}")).collect::<Vec<_>>(),
        "{first:?}"
    );
    assert!(
        first[1].1 && first[1].2.contains("already offered"),
        "{first:?}"
    );
    assert!(
        first[11].1 && first[11].2.contains("At most 10"),
        "{first:?}"
    );
    let shown = offers(&board);
    assert_eq!(shown.len(), 10);
    assert_eq!(
        shown.iter().collect::<std::collections::HashSet<_>>().len(),
        10,
        "each its own card"
    );

    // Each card is answered on its own: the others stay unanswered.
    for card in shown.iter().filter(|card| **card != shown[5]) {
        flow.manager
            .answer_feedback_offer(&flow.conversation, card, true, Vec::new())
            .await
            .unwrap();
    }
    let board = flow.core.board(&flow.conversation).await.unwrap();
    let answered: std::collections::HashSet<_> = board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            OrchestratorStepKind::FeedbackAnswered { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(answered.len(), 9);
    assert!(!answered.contains(&shown[5]));

    // One card of that request unanswered, though the last one was sent: no unasked offer.
    flow.say("Ugh, still broken.").await;
    flow.settled().await;
    let second = taken(&replies);
    assert!(
        second.len() == 1 && second[0].1 && second[0].2.contains("hasn't answered"),
        "{second:?}"
    );

    // One of them dismissed: none either.
    flow.manager
        .answer_feedback_offer(&flow.conversation, &shown[5], false, Vec::new())
        .await
        .unwrap();
    flow.say("Ugh.").await;
    let board = flow.settled().await;
    let third = taken(&replies);
    assert!(
        third.len() == 1 && third[0].1 && third[0].2.contains("dismissed"),
        "{third:?}"
    );
    assert_eq!(offers(&board).len(), 10);
    flow.stop().await;
}

/// An offer from an attempt the user asked to redo is out of view, so it blocks no new one: the
/// new attempt may offer again.
#[tokio::test]
async fn a_feedback_offer_from_a_redone_attempt_blocks_nothing() {
    let replies: Arc<Mutex<Vec<bool>>> = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        "thread-offer-feedback-redo",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.is_orchestrator() {
                    let reply = turn
                        .call(
                            "offer_feedback",
                            json!({"kind": "idea", "title": "Pin chats", "body": "Let me pin a chat."}),
                        )
                        .await;
                    log.lock().unwrap().push(reply.is_error);
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("I can't pin chats, annoying.").await;
    let board = flow.settled().await;
    let request = board
        .orchestrator_steps
        .iter()
        .find(|step| matches!(step.kind, OrchestratorStepKind::FeedbackOffered { .. }))
        .and_then(|step| step.request_id.clone())
        .expect("an offer under the request");
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    flow.manager
        .regenerate(flow.conversation.clone(), request)
        .await
        .unwrap();
    flow.settled().await;
    assert_eq!(*replies.lock().unwrap(), vec![false, false]);
    flow.stop().await;
}

/// An unanswered offer under a message the user then edits leaves the thread with the edit:
/// it blocks nothing there.
#[tokio::test]
async fn a_feedback_offer_from_an_edited_message_blocks_nothing() {
    let replies: Arc<Mutex<Vec<bool>>> = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        "thread-offer-feedback-edit",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.is_orchestrator() {
                    let reply = turn
                        .call(
                            "offer_feedback",
                            json!({"kind": "idea", "title": "Pin chats", "body": "Let me pin a chat."}),
                        )
                        .await;
                    log.lock().unwrap().push(reply.is_error);
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("I can't pin chats, annoying.").await;
    flow.settled().await;
    let messages = flow.core.all_messages(&flow.conversation).await.unwrap();
    let original = messages
        .iter()
        .rfind(|message| message.role == crate::model::MessageRole::User)
        .unwrap()
        .id
        .clone();
    flow.manager
        .edit_message(
            flow.conversation.clone(),
            original,
            "Still can't pin chats.".into(),
            None,
        )
        .await
        .unwrap();
    flow.settled().await;
    assert_eq!(*replies.lock().unwrap(), vec![false, false]);
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

/// Logs `count` provider notices to the thread's orchestrator log, as a busy turn's events
/// would (a busy thread logs hundreds between two CLI starts).
async fn log_busy(flow: &Flow, count: usize) {
    for index in 0..count {
        flow.manager
            .log_provider(
                &flow.conversation,
                ProviderKind::Claude,
                brigadier_providers::ProviderEvent::Notice {
                    level: brigadier_providers::NoticeLevel::Info,
                    message: format!("busy {index}"),
                },
            )
            .await;
    }
}

/// The session's permission level changes to `level` (between turns).
async fn set_level(flow: &Flow, level: PermissionLevel) {
    let mut setup = flow.core.conversation(&flow.conversation).unwrap().setup;
    let Some(Setup::Session { permission, .. }) = &mut setup else {
        panic!("a session");
    };
    *permission = level;
    flow.manager
        .set_setup(flow.conversation.clone(), setup.unwrap())
        .await
        .unwrap();
}

/// The native id the thread's first CLI session started with, from the log.
async fn first_native_id(flow: &Flow) -> String {
    flow.manager
        .read_native_id(&flow.conversation, ProviderKind::Claude)
        .await
        .flatten()
        .expect("a logged session start")
}

/// How many rebirths the conversation's log records.
async fn rebirth_count(flow: &Flow) -> usize {
    flow.core
        .list_orchestrator_log(&flow.conversation, None, brigadier_store::MAX_PAGE)
        .await
        .unwrap()
        .entries
        .iter()
        .filter(|entry| matches!(entry.entry, crate::work::OrchestratorEntry::Rebirth { .. }))
        .count()
}

/// Bug 2: the turn after a closed CLI resumes its session however many events were logged
/// since it started, even with nothing remembered (a restarted daemon reads the log).
#[tokio::test]
async fn a_busy_thread_resumes_its_session_past_a_thousand_logged_events() {
    let flow = Flow::start(
        "busy-resume",
        Options::default(),
        script(|_| async { Reply::text("Done.") }),
    )
    .await;
    flow.say("First.").await;
    flow.settled().await;
    let native = first_native_id(&flow).await;
    // 1,000 events logged behind the manager's back, then the CLI closes and the live state
    // is gone, as after a restart.
    for index in 0..1_000 {
        flow.core
            .record(vec![(
                crate::model::streams::orchestrator(&flow.conversation),
                crate::model::DomainEvent::OrchestratorLogged {
                    conversation_id: flow.conversation.clone(),
                    entry: crate::work::OrchestratorEntry::Provider {
                        provider: ProviderKind::Claude,
                        event: brigadier_providers::ProviderEvent::Notice {
                            level: brigadier_providers::NoticeLevel::Info,
                            message: format!("busy {index}"),
                        },
                    },
                },
            )])
            .await
            .unwrap();
    }
    flow.manager
        .conv(&flow.conversation)
        .unwrap()
        .close_cli()
        .await;
    flow.manager.convs_lock().remove(&flow.conversation);

    flow.say("Second.").await;
    flow.until("the second start", |_| flow.thread_specs().len() >= 2)
        .await;
    flow.settled().await;
    let specs = flow.thread_specs();
    assert_eq!(
        specs[1].1.origin,
        Origin::Resume { native_id: native },
        "the same session resumes"
    );
    assert_eq!(rebirth_count(&flow).await, 0, "no rebirth");
    flow.stop().await;
}

/// Bug 3: Approve for me → Full access on a busy thread resumes the same session between
/// turns, with the permission note, and no rebirth.
#[tokio::test]
async fn a_permission_change_on_a_busy_thread_resumes_with_the_note() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let flow = Flow::start(
        "busy-level",
        Options {
            permission: PermissionLevel::ApproveForMe,
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
    flow.say("First.").await;
    flow.settled().await;
    let native = first_native_id(&flow).await;
    log_busy(&flow, 1_000).await;
    set_level(&flow, PermissionLevel::FullAccess).await;

    flow.say("Second.").await;
    flow.until("the second turn", |_| inputs.lock().unwrap().len() >= 2)
        .await;
    flow.settled().await;
    let specs = flow.thread_specs();
    assert_eq!(specs.len(), 2, "one start and one restart");
    assert_eq!(specs[1].1.origin, Origin::Resume { native_id: native });
    assert_eq!(specs[1].1.access, Access::Full);
    let second = inputs.lock().unwrap()[1].clone();
    assert!(!second.contains("[settings]"), "{second}");
    assert_eq!(rebirth_count(&flow).await, 0, "no rebirth");
    flow.stop().await;
}

/// A thread idle past its cache's lifetime, with a checkpoint covering its last request, at
/// `context` tokens of a 1M window, its CLI closed meanwhile: the first session's id, the
/// thread's settled decisions and every input the thread got.
async fn cold_turn(name: &str, context: i64) -> (Flow, String, Vec<String>, Vec<String>) {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let flow = Flow::start(
        name,
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
    flow.say("First.").await;
    flow.settled().await;
    let native = first_native_id(&flow).await;
    // Thirty settled decisions with long bodies: far past the briefing's old 4k-token budget
    // for them.
    let decisions: Vec<String> = (0..30)
        .map(|n| {
            format!(
                "Decision {n}: {}",
                format!("keep part {n} as it is; ").repeat(40)
            )
        })
        .collect();
    let kept = flow
        .manager
        .keep_note_decisions(&flow.conversation, decisions.clone(), 0)
        .await
        .unwrap();
    assert_eq!(kept, 30);
    let now = crate::now_ms();
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    conv.set_cache_mark(crate::manager::cold::CacheMark {
        native_id: native.clone(),
        provider: ProviderKind::Claude,
        at_ms: now - 2 * 3_600_000,
        context,
        window: Some(1_000_000),
    })
    .await;
    conv.set_checkpoint(crate::manager::rebirth::RebirthPrep::written(
        "Goal: the first ask.\nWhat to do next: the second.",
        &native,
        now - 2 * 3_600_000 + 60_000,
    ))
    .await;
    conv.close_cli().await;

    flow.say("Second.").await;
    flow.until("the second turn", |_| inputs.lock().unwrap().len() >= 2)
        .await;
    flow.settled().await;
    let inputs = inputs.lock().unwrap().clone();
    (flow, native, decisions, inputs)
}

/// Q5: a thread under 150k whose cache expired resumes its session.
#[tokio::test]
async fn a_cold_thread_under_150k_resumes() {
    if std::env::var_os("BRIGADIER_COLD_REBIRTH_TOKENS").is_some() {
        return;
    }
    let (flow, native, _, _) = cold_turn("cold-small", 120_000).await;
    let specs = flow.thread_specs();
    assert_eq!(specs[1].1.origin, Origin::Resume { native_id: native });
    assert_eq!(rebirth_count(&flow).await, 0);
    flow.stop().await;
}

/// Q5: a bigger one is reborn from its checkpoint, with every decision whole in the briefing.
#[tokio::test]
async fn a_cold_thread_over_150k_is_reborn_with_every_decision_whole() {
    if std::env::var_os("BRIGADIER_COLD_REBIRTH_TOKENS").is_some() {
        return;
    }
    let (flow, _, decisions, inputs) = cold_turn("cold-big", 200_000).await;
    let specs = flow.thread_specs();
    assert_eq!(specs[1].1.origin, Origin::New, "reborn");
    let records: Vec<_> = flow
        .core
        .list_orchestrator_log(&flow.conversation, None, brigadier_store::MAX_PAGE)
        .await
        .unwrap()
        .entries
        .into_iter()
        .filter_map(|entry| match entry.entry {
            crate::work::OrchestratorEntry::Rebirth { record } => Some(record),
            _ => None,
        })
        .collect();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].trigger,
        crate::knowledge::RebirthTrigger::CacheExpired
    );
    assert_eq!(records[0].decisions, 30);
    assert_eq!(records[0].decisions_in_full, 30);
    let briefing = &inputs[1];
    for line in &decisions {
        assert!(briefing.contains(line.as_str()), "a decision cut: {line}");
    }
    flow.stop().await;
}

/// A checkpoint's note fork of a `thread` session, whose model asks for `query_brain`: the
/// fork's spec, the thread's first spec, the note and what the call was answered.
async fn note_fork(name: &str, thread: ProviderKind) -> (Flow, SessionSpec, SessionSpec, String) {
    let answers: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = answers.clone();
    let flow = Flow::start(
        name,
        Options {
            thread,
            ..Options::default()
        },
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.input.contains("Write the handoff note") {
                    let reply = turn
                        .call("query_brain", json!({ "query": "the plan" }))
                        .await;
                    log.lock().unwrap().push(reply.text);
                    return Reply::text("Goal and open threads: the first ask.");
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.say("First.").await;
    flow.settled().await;
    let native = flow
        .manager
        .read_native_id(&flow.conversation, thread)
        .await
        .flatten()
        .expect("a logged session start");
    let Some(Setup::Session { orchestrator, .. }) =
        flow.core.conversation(&flow.conversation).unwrap().setup
    else {
        panic!("a session");
    };
    let prep = flow.manager.prepare_rebirth(
        &flow.conversation,
        thread,
        orchestrator,
        Some(native.clone()),
        (200_000, Some(1_000_000)),
        crate::manager::rebirth::HandoffPurpose::Checkpoint,
    );
    let note = prep
        .note(std::time::Duration::from_secs(30))
        .await
        .expect("a note");
    assert_eq!(note, "Goal and open threads: the first ask.");
    let fork = flow
        .specs()
        .into_iter()
        .map(|(_, spec)| spec)
        .find(|spec| matches!(spec.origin, Origin::Fork { .. }))
        .expect("a fork");
    assert_eq!(fork.origin, Origin::Fork { native_id: native });
    // It stays read-only, whatever the thread may do.
    assert_eq!(fork.access, Access::ReadOnly);
    assert!(fork.ephemeral && !fork.owned_cwd && fork.sweep_dir.is_none());
    let thread = flow.thread_specs()[0].1.clone();
    let answered = answers.lock().unwrap().clone();
    (flow, fork, thread, answered.join("\n"))
}

fn grant_of(spec: &SessionSpec) -> Option<String> {
    spec.mcp_servers
        .iter()
        .flat_map(|server| &server.env)
        .find(|(name, _)| name == "BRIGADIER_MCP_GRANT")
        .map(|(_, grant)| grant.clone())
}

/// B5: a Claude thread's note fork starts as the thread did (tools, MCP server, model,
/// instructions and the rest), so it reads the thread's prefix from the cache; only its
/// origin, its life and its folders' ownership change. It stays read-only, a hook denies every
/// tool, and Brigadier's server refuses its calls under a grant of its own, revoked when it ends.
#[tokio::test]
async fn a_claude_note_fork_starts_as_its_thread_and_every_call_is_refused() {
    let (flow, fork, thread, answered) = note_fork("note-fork-claude", ProviderKind::Claude).await;
    assert_eq!(
        fork.deny_tools.as_deref(),
        Some(crate::manager::rebirth::NOTE_FORK_REFUSAL)
    );
    assert_eq!(answered, crate::manager::rebirth::NOTE_FORK_REFUSAL);
    // The same server, under another grant.
    assert_eq!(fork.mcp_servers.len(), thread.mcp_servers.len());
    for (forked, own) in fork.mcp_servers.iter().zip(&thread.mcp_servers) {
        assert_eq!(
            (&forked.name, &forked.transport, forked.always_load),
            (&own.name, &own.transport, own.always_load)
        );
        assert_eq!(
            (
                &forked.prompt_tools,
                forked.tool_timeout_secs,
                forked.trusted
            ),
            (&own.prompt_tools, own.tool_timeout_secs, own.trusted)
        );
    }
    let grant = grant_of(&fork).expect("the fork's grant");
    assert_ne!(Some(&grant), grant_of(&thread).as_ref());
    assert!(
        flow.manager.grants().resolve(&grant).is_none(),
        "revoked once the fork ended"
    );
    // Everything else is the thread's.
    let unchanged = SessionSpec {
        origin: thread.origin.clone(),
        ephemeral: thread.ephemeral,
        access: thread.access.clone(),
        mcp_servers: thread.mcp_servers.clone(),
        record_to: thread.record_to.clone(),
        redactor: thread.redactor.clone(),
        owned_cwd: thread.owned_cwd,
        sweep_dir: thread.sweep_dir.clone(),
        output_hook: thread.output_hook.clone(),
        deny_tools: None,
        ..fork.clone()
    };
    assert_eq!(format!("{unchanged:?}"), format!("{thread:?}"));
    assert_eq!(fork.tools, ToolSet::Thread);
    flow.stop().await;
}

/// B5: a Codex thread's note fork is as before, with no tools beyond Codex's own read-only
/// ones and no MCP server, until a Codex hook probe shows its tools can be denied.
#[tokio::test]
async fn a_codex_note_fork_is_unchanged() {
    let (flow, fork, _, answered) = note_fork("note-fork-codex", ProviderKind::Codex).await;
    assert_eq!(fork.tools, ToolSet::None);
    assert!(fork.mcp_servers.is_empty());
    assert!(fork.deny_tools.is_none() && fork.output_hook.is_none());
    assert!(
        answered.contains("not available") || answered.contains("not valid"),
        "{answered}"
    );
    flow.stop().await;
}

/// A request a script's CLI asked, and the answer it got.
type Asked = (String, Option<ApprovalDecision>);

/// Brigadier's escalation reviewer in a script: refuses what deletes a branch or a worktree
/// (the protected set) or a cache folder, gives no verdict on a publish, allows the rest, and
/// keeps what it was asked.
fn escalation_reviewer(turn: &super::Turn, asked: &Mutex<Vec<String>>) -> Option<Reply> {
    if !turn
        .prompt
        .starts_with("You review one request of a coding agent")
    {
        return None;
    }
    asked.lock().unwrap().push(turn.input.clone());
    Some(Reply::text(
        // The command itself, not the rule (which names such commands too).
        if turn.input.contains("```\ngit branch -D")
            || turn.input.contains("```\ngit worktree remove")
        {
            r#"{"allow": false, "protected": true, "reason": "it deletes another session's work"}"#
        } else if turn.input.contains("```\nrm -rf ~/caches") {
            r#"{"allow": false, "reason": "it deletes outside the project."}"#
        } else if turn.input.contains("```\nnpm publish") {
            "I can't tell."
        } else {
            r#"{"allow": true, "reason": "a network read"}"#
        },
    ))
}

/// A Codex escalation whose review ends after the CLI that asked has closed (retired, taken
/// over) opens no card: its answer would have nowhere to go.
#[tokio::test]
async fn a_review_that_outlives_its_cli_opens_no_card() {
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let closed = Arc::new(tokio::sync::Notify::new());
    let reviewed = Arc::new(tokio::sync::Notify::new());
    let (reviewer_log, gate, done) = (asked.clone(), closed.clone(), reviewed.clone());
    let flow = Flow::start(
        "codex-review-outlives",
        Options {
            permission: PermissionLevel::ApproveForMe,
            thread: ProviderKind::Codex,
            ..Options::default()
        },
        script(move |turn| {
            let (asked, gate, done) = (reviewer_log.clone(), gate.clone(), done.clone());
            async move {
                if let Some(reply) = escalation_reviewer(&turn, &asked) {
                    gate.notified().await;
                    done.notify_one();
                    return reply;
                }
                if turn.is_orchestrator() {
                    turn.ask_approval("npm publish", "x").await;
                }
                Reply::text("")
            }
        }),
    )
    .await;

    flow.say("Publish.").await;
    super::eventually("the reviewer to be asked", || {
        asked.lock().unwrap().len() == 1
    })
    .await;
    flow.manager
        .conv(&flow.conversation)
        .unwrap()
        .close_cli()
        .await;
    closed.notify_one();
    reviewed.notified().await;
    // The review's verdict ("I can't tell") would be a card.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        flow.board()
            .await
            .approvals
            .values()
            .all(|approval| approval.state != CardState::Pending),
        "{:?}",
        flow.board().await.approvals
    );
    flow.stop().await;
}

/// Codex at Approve for me: its thread's and its workers' requests to leave the sandbox reach
/// Brigadier, whose reviewer settles them with the session's protected set, before any
/// "Allow similar" grant could. What would touch the protected set is declined to the model
/// with the reason, never a card; what it allows goes through.
#[tokio::test]
async fn brigadiers_reviewer_settles_a_codex_sessions_escalations_at_approve_for_me() {
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let decisions: Arc<Mutex<Vec<Asked>>> = Arc::default();
    let (reviewer_log, log) = (asked.clone(), decisions.clone());
    let flow = Flow::start(
        "codex-reviewed",
        Options {
            permission: PermissionLevel::ApproveForMe,
            thread: ProviderKind::Codex,
            ..Options::default()
        },
        script(move |turn| {
            let (asked, log) = (reviewer_log.clone(), log.clone());
            async move {
                if let Some(reply) = escalation_reviewer(&turn, &asked) {
                    return reply;
                }
                let ask = async |who: &str, command: &str| {
                    let decision = turn.ask_approval(command, "git").await;
                    log.lock()
                        .unwrap()
                        .push((format!("{who}: {command}"), decision));
                };
                if turn.is_orchestrator() {
                    if turn.input.contains("[task-1") {
                        return Reply::text("");
                    }
                    ask("thread", "git branch -D brigadier/other").await;
                    ask("thread", "curl https://example.com").await;
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Tidy up", "kind": "implement",
                                   "spec": "Tidy up.", "provider": "codex"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("");
                }
                ask("worker", "git worktree remove --force /elsewhere").await;
                ask("worker", "curl https://example.com/worker").await;
                Reply::text("Done.")
            }
        }),
    )
    .await;
    // Earlier "Allow … for this session" answers that would cover both deletions.
    let command = |command: &str| brigadier_providers::ApprovalRequest {
        id: String::new(),
        kind: brigadier_providers::ApprovalKind::Command,
        tool: "commandExecution".into(),
        command: Some(command.into()),
        cwd: None,
        paths: Vec::new(),
        reason: None,
        escalation: true,
        input: None,
        grant: None,
    };
    for granted in ["git branch --list", "git worktree list"] {
        flow.manager
            .waiters
            .allow_similar(&flow.conversation, &command(granted));
    }
    for deletion in [
        "git branch -D brigadier/other",
        "git worktree remove --force /elsewhere",
    ] {
        assert!(
            flow.manager
                .waiters
                .similar_allowed(&flow.conversation, &command(deletion), None),
            "{deletion}"
        );
    }
    flow.say("Clean up everything.").await;
    flow.until("four requests settled", |_| {
        decisions.lock().unwrap().len() >= 4
    })
    .await;

    let decisions = decisions.lock().unwrap().clone();
    let refused = |message: &str| {
        message.starts_with(
            "Brigadier's reviewer would not allow it: it deletes another session's work",
        )
    };
    for (asked, decision) in &decisions {
        match decision {
            Some(ApprovalDecision::Deny { message }) => {
                assert!(asked.contains("-D") || asked.contains("remove"), "{asked}");
                assert!(refused(message), "{message}");
            }
            Some(ApprovalDecision::Allow) => assert!(asked.contains("curl"), "{asked}"),
            other => panic!("{asked}: {other:?}"),
        }
    }
    let asked = asked.lock().unwrap().clone();
    // (The worker's next turn, a nudge to report, asks again.)
    assert!(
        asked.len() >= 4,
        "the reviewer saw each request: {:#?}",
        asked
            .iter()
            .map(|input| input.split("```").nth(1))
            .collect::<Vec<_>>()
    );
    assert!(
        asked
            .iter()
            .all(|input| input.contains("Never allow a command, even one the user asked for")),
        "the protected set reaches the reviewer: {asked:#?}"
    );
    let board = flow.board().await;
    assert!(
        board.approvals.is_empty(),
        "no card: {:#?}",
        board.approvals
    );
    flow.stop().await;
}

/// What Brigadier's reviewer refuses for another reason than the protected set, or gives no
/// verdict on, is the user's card with the reviewer's word while the user is around, and
/// declined to the model while an overnight run has nobody to ask.
#[tokio::test]
async fn an_unsettled_codex_escalation_is_a_card_unless_nobody_is_there() {
    use super::super::overnight::policy::ActiveRun;
    use crate::model::OvernightRunId;

    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let decisions: Arc<Mutex<Vec<Asked>>> = Arc::default();
    let (reviewer_log, log) = (asked.clone(), decisions.clone());
    let flow = Flow::start(
        "codex-unsettled",
        Options {
            permission: PermissionLevel::ApproveForMe,
            thread: ProviderKind::Codex,
            ..Options::default()
        },
        script(move |turn| {
            let (asked, log) = (reviewer_log.clone(), log.clone());
            async move {
                if let Some(reply) = escalation_reviewer(&turn, &asked) {
                    return reply;
                }
                if turn.is_orchestrator() {
                    for command in ["rm -rf ~/caches", "npm publish"] {
                        let decision = turn.ask_approval(command, "x").await;
                        log.lock().unwrap().push((command.into(), decision));
                    }
                }
                Reply::text("")
            }
        }),
    )
    .await;

    // The user is around: each is a card saying why the reviewer didn't settle it.
    flow.say("Tidy up and publish.").await;
    let mut reasons = Vec::new();
    for done in 0..2 {
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
        reasons.push(request.reason.clone().unwrap_or_default());
        flow.manager
            .answer_card(
                flow.conversation.clone(),
                card.id.clone(),
                ApprovalDecision::Allow,
            )
            .await
            .unwrap();
    }
    assert_eq!(
        reasons[0],
        "Brigadier's reviewer would not allow it: it deletes outside the project."
    );
    assert!(
        reasons[1].starts_with("Brigadier's reviewer gave no verdict"),
        "{}",
        reasons[1]
    );
    flow.until("both answered", |_| decisions.lock().unwrap().len() == 2)
        .await;
    assert!(
        decisions
            .lock()
            .unwrap()
            .iter()
            .all(|(_, decision)| decision == &Some(ApprovalDecision::Allow))
    );

    // The request ends first: a message sent while it works is a follow-up to it, which the
    // thread hears once as that and again as a request of its own.
    flow.settled().await;

    // An overnight run: nobody to ask, so both are declined with the reason.
    flow.manager.overnight.active.insert(
        flow.conversation.clone(),
        ActiveRun {
            id: OvernightRunId("codex-unsettled-run".into()),
            segment: 0,
            generation: 0,
            rules_hash: String::new(),
            workspace: None,
            max_workers: None,
            winding_down: false,
            wind_down_at_ms: None,
        },
    );
    flow.say("Again.").await;
    flow.until("both declined", |_| decisions.lock().unwrap().len() == 4)
        .await;
    let decisions = decisions.lock().unwrap().clone();
    for (command, decision) in &decisions[2..] {
        let Some(ApprovalDecision::Deny { message }) = decision else {
            panic!("{command}: {decision:?}");
        };
        assert!(
            message.starts_with("Brigadier's reviewer")
                && message.contains("nobody is here to ask")
                && !message.contains(".,"),
            "{message}"
        );
    }
    assert_eq!(
        flow.board()
            .await
            .approvals
            .values()
            .filter(|approval| approval.state == CardState::Pending)
            .count(),
        0
    );
    flow.stop().await;
}

/// The CI watch outcomes used by the push-review tests.
fn ci_watch_for_push_review(result: &str) -> OrchestratorStepKind {
    OrchestratorStepKind::CiWatch {
        repo: "o/n".into(),
        branch: "fix".into(),
        sha: format!("{result}-sha"),
        waiting: false,
        runs: Vec::new(),
        result: Some(result.into()),
    }
}

/// Q61: pushes are the agent's call before watching CI, while it is red and after it passes.
#[tokio::test]
async fn ci_push_review_allows_pushes_without_a_watch_and_after_failed_and_passed_watches() {
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let decisions: Arc<Mutex<Vec<Asked>>> = Arc::default();
    let pushes = Arc::new(tokio::sync::Semaphore::new(0));
    let (reviewer_log, log, gate) = (asked.clone(), decisions.clone(), pushes.clone());
    let flow = Flow::start(
        "20261018-1945-issues-run-2-w09-ci-repush",
        Options {
            permission: PermissionLevel::ApproveForMe,
            thread: ProviderKind::Codex,
            ..Options::default()
        },
        script(move |turn| {
            let (asked, log, gate) = (reviewer_log.clone(), log.clone(), gate.clone());
            async move {
                if turn.prompt.starts_with("You review one request of a coding agent") {
                    assert!(turn.prompt.contains("Pushing, opening pull requests, publishing and deploying are the agent's own call"));
                    asked.lock().unwrap().push(turn.input.clone());
                    return Reply::text(r#"{"allow": true, "reason": "pushing is the agent's call"}"#);
                }
                if turn.is_orchestrator() {
                    for _ in 0..3 {
                        gate.acquire().await.unwrap().forget();
                        let decision = turn.ask_approval("git push origin fix", "git").await;
                        log.lock().unwrap().push(("git push origin fix".into(), decision));
                    }
                }
                Reply::text("")
            }
        }),
    ).await;
    git(
        &flow.repo,
        &["remote", "add", "origin", "git@github.com:o/n.git"],
    );
    flow.say("Fix CI.").await;
    for (index, result) in [None, Some("failed"), Some("passed")]
        .into_iter()
        .enumerate()
    {
        if let Some(result) = result {
            flow.manager
                .orchestrator_step(&flow.conversation, ci_watch_for_push_review(result))
                .await;
        }
        pushes.add_permits(1);
        flow.until("the push reviewed", |_| {
            decisions.lock().unwrap().len() == index + 1
        })
        .await;
    }
    assert!(
        decisions
            .lock()
            .unwrap()
            .iter()
            .all(|(_, decision)| *decision == Some(ApprovalDecision::Allow))
    );
    assert!(flow.board().await.approvals.is_empty());
    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 3, "{asked:?}");
    for seen in asked {
        assert!(
            !seen.contains("The user's consent that may cover a push"),
            "{seen}"
        );
        assert!(!seen.contains("The folder's push remotes"), "{seen}");
    }
    flow.stop().await;
}

/// An unrelated message joining a CI fix's turn does not change the reviewer's push rule.
#[tokio::test]
async fn ci_push_review_allows_a_push_when_a_user_message_joins_the_ci_fix() {
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let decisions: Arc<Mutex<Vec<Option<ApprovalDecision>>>> = Arc::default();
    let pushes = Arc::new(tokio::sync::Semaphore::new(0));
    let (reviewer_log, log, gate) = (asked.clone(), decisions.clone(), pushes.clone());
    let flow = Flow::start(
        "20261018-1945-issues-run-2-w09-ci-repush-queued",
        Options {
            permission: PermissionLevel::ApproveForMe,
            thread: ProviderKind::Codex,
            ..Options::default()
        },
        script(move |turn| {
            let (asked, log, gate) = (reviewer_log.clone(), log.clone(), gate.clone());
            async move {
                if turn.prompt.starts_with("You review one request of a coding agent") {
                    assert!(turn.prompt.contains("Pushing, opening pull requests, publishing and deploying are the agent's own call"));
                    asked.lock().unwrap().push(turn.input.clone());
                    return Reply::text(r#"{"allow": true, "reason": "pushing is the agent's call"}"#);
                }
                if turn.is_orchestrator() && turn.input.contains("Fix CI.") {
                    gate.acquire().await.unwrap().forget();
                    let decision = turn.ask_approval("git push origin fix", "git").await;
                    log.lock().unwrap().push(decision);
                }
                Reply::text("")
            }
        }),
    ).await;
    flow.say("Fix CI.").await;
    flow.until("the turn runs", |board| {
        board.run == crate::work::RunState::Running
    })
    .await;
    flow.manager
        .orchestrator_step(&flow.conversation, ci_watch_for_push_review("failed"))
        .await;
    flow.say("Include the CI link in your report.").await;
    flow.until("the message joins the turn", |board| {
        board
            .requests
            .values()
            .any(|request| request.steered_into.is_some())
    })
    .await;
    pushes.add_permits(1);
    flow.until("the push reviewed", |_| {
        !decisions.lock().unwrap().is_empty()
    })
    .await;
    assert_eq!(*decisions.lock().unwrap(), [Some(ApprovalDecision::Allow)]);
    assert!(flow.board().await.approvals.is_empty());
    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(
        !asked[0].contains("The user's consent that may cover a push"),
        "{}",
        asked[0]
    );
    flow.stop().await;
}

/// An unrelated message arriving during review causes no extra review of a CI-watch answer.
#[tokio::test]
async fn ci_push_review_does_not_repeat_when_a_user_message_arrives_during_review() {
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let decisions: Arc<Mutex<Vec<Option<ApprovalDecision>>>> = Arc::default();
    let pushes = Arc::new(tokio::sync::Semaphore::new(0));
    let reviewing = Arc::new(tokio::sync::Semaphore::new(0));
    let verdict = Arc::new(tokio::sync::Semaphore::new(0));
    let (reviewer_log, log, gate, started, release) = (
        asked.clone(),
        decisions.clone(),
        pushes.clone(),
        reviewing.clone(),
        verdict.clone(),
    );
    let flow = Flow::start(
        "20261018-1945-issues-run-2-w09-ci-repush-mid-review",
        Options {
            permission: PermissionLevel::ApproveForMe,
            thread: ProviderKind::Codex,
            ..Options::default()
        },
        script(move |turn| {
            let (asked, log, gate, started, release) = (reviewer_log.clone(), log.clone(), gate.clone(), started.clone(), release.clone());
            async move {
                if turn.prompt.starts_with("You review one request of a coding agent") {
                    assert!(turn.prompt.contains("Pushing, opening pull requests, publishing and deploying are the agent's own call"));
                    let first = {
                        let mut asked = asked.lock().unwrap();
                        asked.push(turn.input.clone());
                        asked.len() == 1
                    };
                    if first {
                        started.add_permits(1);
                        release.acquire().await.unwrap().forget();
                    }
                    return Reply::text(r#"{"allow": true, "reason": "pushing is the agent's call"}"#);
                }
                if turn.is_orchestrator() && turn.input.contains("Fix CI.") {
                    gate.acquire().await.unwrap().forget();
                    let decision = turn.ask_approval("git push origin fix", "git").await;
                    log.lock().unwrap().push(decision);
                }
                Reply::text("")
            }
        }),
    ).await;
    flow.say("Fix CI.").await;
    flow.until("the turn runs", |board| {
        board.run == crate::work::RunState::Running
    })
    .await;
    flow.manager
        .orchestrator_step(&flow.conversation, ci_watch_for_push_review("failed"))
        .await;
    pushes.add_permits(1);
    reviewing.acquire().await.unwrap().forget();
    flow.say("Include the CI link in your report.").await;
    flow.until("the message joins the turn", |board| {
        board
            .requests
            .values()
            .any(|request| request.steered_into.is_some())
    })
    .await;
    verdict.add_permits(1);
    flow.until("the push reviewed", |_| {
        !decisions.lock().unwrap().is_empty()
    })
    .await;
    assert_eq!(*decisions.lock().unwrap(), [Some(ApprovalDecision::Allow)]);
    assert!(flow.board().await.approvals.is_empty());
    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(
        !asked[0].contains("The user's consent that may cover a push"),
        "{}",
        asked[0]
    );
    flow.stop().await;
}

/// `suggest_setting` (Q90): a card per change, which changes nothing on its own; Full access is
/// the sandbox notice; a setting that is that way already, or a value it doesn't take, is
/// refused, and a setting outside the few it may suggest doesn't parse.
#[tokio::test]
async fn the_thread_suggests_a_setting_on_a_card_that_changes_nothing_until_pressed() {
    let replies: Arc<Mutex<Vec<(bool, String)>>> = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        "thread-suggest-setting",
        Options {
            permission: PermissionLevel::AskForApproval,
            ..Options::default()
        },
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.is_orchestrator() {
                    for (setting, value) in [
                        ("browser", "off"),
                        ("browser", "off"),
                        ("browser", "on"),
                        ("permission", "approve_for_me"),
                        ("permission", "ask_for_approval"),
                        ("permission", "full_access"),
                        ("browser", "maybe"),
                    ] {
                        let reply = turn
                            .call(
                                "suggest_setting",
                                json!({"setting": setting, "value": value, "reason": "It helps."}),
                            )
                            .await;
                        log.lock().unwrap().push((reply.is_error, reply.text));
                    }
                }
                Reply::text("Asked.")
            }
        }),
    )
    .await;
    let before = flow.core.settings();
    assert!(before.browser_enabled);
    flow.say("Turn the browser off.").await;
    let board = flow.settled().await;
    let replies = replies.lock().unwrap().clone();
    let errors: Vec<bool> = replies.iter().map(|reply| reply.0).collect();
    assert_eq!(
        errors,
        [false, true, true, false, true, false, true],
        "{replies:#?}"
    );
    assert!(replies[1].1.contains("already shown"), "{}", replies[1].1);
    assert!(
        replies[2].1.contains("that way already"),
        "{}",
        replies[2].1
    );
    assert!(
        replies[6].1.contains("\"on\" or \"off\""),
        "{}",
        replies[6].1
    );
    let cards: Vec<_> = board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            OrchestratorStepKind::SettingSuggested { change, .. } => Some(change.clone()),
            OrchestratorStepKind::FullAccessSuggested { .. } => {
                Some(crate::work::SettingChange::Permission {
                    level: PermissionLevel::FullAccess,
                })
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        cards,
        [
            crate::work::SettingChange::Browser { on: false },
            crate::work::SettingChange::Permission {
                level: PermissionLevel::ApproveForMe
            },
            crate::work::SettingChange::Permission {
                level: PermissionLevel::FullAccess
            },
        ]
    );
    // Nothing changed: only the user's press does that.
    let after = flow.core.settings();
    assert_eq!(after.browser_enabled, before.browser_enabled);
    let Some(Setup::Session { permission, .. }) =
        flow.core.conversation(&flow.conversation).unwrap().setup
    else {
        panic!("a session");
    };
    assert_eq!(permission, PermissionLevel::AskForApproval);
    flow.stop().await;
}
