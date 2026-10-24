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

/// At each level and for both vendors, the thread starts with its own tools in its workspace
/// (the session worktree, made before the CLI starts, #523) at that level; its scratch folder stays its sweep folder. Its environment sets only its temporary
/// folder: the CLI's own shell timeouts, output limits and background tasks hold (S2).
#[tokio::test]
async fn the_thread_works_in_its_workspace_with_the_levels_access() {
    for (permission, thread) in [
        (PermissionLevel::Full, ProviderKind::Claude),
        (PermissionLevel::AcceptEdits, ProviderKind::Codex),
        (PermissionLevel::Ask, ProviderKind::Claude),
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
        // Only its ledger's folder besides its workspace (S17).
        assert_eq!(
            spec.add_dirs,
            vec![flow.manager.session_dir(&flow.conversation).record()]
        );
        assert_eq!(spec.cwd, worktree);
        assert!(spec.owned_cwd, "a New worktree is Brigadier's");
        let scratch = spec.sweep_dir.clone().expect("its sweep folder");
        let folder = flow.manager.session_dir(&flow.conversation);
        assert_eq!(scratch, folder.dev());
        let env: std::collections::HashMap<_, _> = spec.env.iter().cloned().collect();
        // Its temporary files go to its session folder's `tmp/`, never the data folder's.
        let tmp = folder.tmp();
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
        // The level passes straight through as the CLI's own permission mode.
        assert_eq!(spec.access, permission.access(), "{permission:?}");
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

/// Under Ask, on both CLIs: the thread's CLI runs in its own `default`/`untrusted` mode, its
/// permission prompt becomes the user's card (Brigadier answers nothing itself), and the
/// user's answer reaches the CLI, whose turn then goes on to its answer (G5).
#[tokio::test]
async fn under_ask_a_cli_permission_prompt_is_a_card_and_its_answer_continues_the_turn() {
    for thread in [ProviderKind::Claude, ProviderKind::Codex] {
        let answered: Arc<Mutex<Option<ApprovalDecision>>> = Arc::default();
        let log = answered.clone();
        let flow = Flow::start(
            "thread-ask",
            Options {
                permission: PermissionLevel::Ask,
                thread,
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
        // The CLI waits on the card: nothing answered it.
        assert_eq!(*answered.lock().unwrap(), None, "{thread:?}");
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
        let board = flow.settled().await;
        assert_eq!(*answered.lock().unwrap(), Some(ApprovalDecision::Allow));
        assert_eq!(
            board.approvals[&card.id].state,
            CardState::Allowed {
                by: brigadier_providers::Decider::User,
                similar: false
            }
        );
        // The turn went on past the prompt to its answer.
        let request = board.requests.values().next().unwrap();
        assert_eq!(request.state, crate::work::RequestState::Done, "{thread:?}");
        let messages = flow.core.all_messages(&flow.conversation).await.unwrap();
        assert!(
            messages
                .iter()
                .any(|message| message.text.contains("Fetched it.")),
            "{messages:#?}"
        );
        let specs = flow.thread_specs();
        assert_eq!(specs[0].1.access, Access::Ask, "{thread:?}");
        flow.stop().await;
    }
}

/// Raised to Full access mid-turn, on both CLIs: the running CLI keeps the mode it started
/// with until the turn ends, so the request it is waiting on goes at once, and what it asks
/// after goes without a card, as Full access would let it.
#[tokio::test]
async fn raised_to_full_mid_turn_what_the_thread_asks_goes() {
    for thread in [ProviderKind::Claude, ProviderKind::Codex] {
        let answered: Arc<Mutex<Vec<Option<ApprovalDecision>>>> = Arc::default();
        let log = answered.clone();
        let flow = Flow::start(
            "thread-raised",
            Options {
                permission: PermissionLevel::AcceptEdits,
                thread,
                ..Options::default()
            },
            script(move |turn| {
                let log = log.clone();
                async move {
                    let first = turn.ask_approval("curl https://example.com", "curl").await;
                    log.lock().unwrap().push(first);
                    let second = turn.ask_approval("curl https://example.org", "curl").await;
                    log.lock().unwrap().push(second);
                    Reply::text("Fetched both.")
                }
            }),
        )
        .await;
        flow.say("Fetch both.").await;
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
            .unwrap()
            .id
            .clone();
        set_level(&flow, PermissionLevel::Full).await;
        let board = flow.settled().await;
        assert_eq!(
            *answered.lock().unwrap(),
            vec![Some(ApprovalDecision::Allow), Some(ApprovalDecision::Allow)],
            "{thread:?}"
        );
        assert_eq!(
            board.approvals[&card].state,
            CardState::Allowed {
                by: brigadier_providers::Decider::Policy,
                similar: false
            }
        );
        // The second request opened no card.
        assert_eq!(board.approvals.len(), 1, "{thread:?}");
        assert_eq!(
            flow.thread_specs()[0].1.access,
            PermissionLevel::AcceptEdits.access()
        );
        flow.stop().await;
    }
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
            permission: PermissionLevel::Ask,
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
            permission: PermissionLevel::Ask,
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
    assert!(
        flow.manager
            .allowed_before(&flow.conversation, &mut edit, Some(&workspace))
    );
    assert!(
        !flow
            .manager
            .allowed_before(&flow.conversation, &mut edit, Some(&flow.repo))
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
                permission: PermissionLevel::AcceptEdits,
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
    assert_eq!(specs[2].1.access, Access::AcceptEdits);

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

/// The thread's merge of its branch into `main`, with plain git in the repository's checkout
/// (there is no merge tool, S16c); the branch merged.
pub(super) fn merge_with_git(turn: &Turn) -> String {
    let branch = git(&turn.cwd, &["branch", "--show-current"]);
    let common = git(
        &turn.cwd,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    );
    let repo = std::path::Path::new(&common).parent().unwrap();
    git(
        repo,
        &["merge", "-q", "--no-ff", "-m", "Merge the work", &branch],
    );
    branch
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

pub(super) fn on_main(flow: &Flow, path: &str) -> bool {
    crate::test_git_command()
        .args(["cat-file", "-e", &format!("main:{path}")])
        .current_dir(&flow.repo)
        .status()
        .is_ok_and(|status| status.success())
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

/// How many seams the session went through.
fn seams(flow: &Flow) -> usize {
    crate::manager::seam::read_seam(&flow.manager.session_dir(&flow.conversation).record())
        .seams
        .len()
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
    assert_eq!(seams(&flow), 0, "no seam");
    flow.stop().await;
}

/// Bug 3: Accept edits → Full access on a busy thread resumes the same session between
/// turns, with the permission note, and no seam.
#[tokio::test]
async fn a_permission_change_on_a_busy_thread_resumes_with_the_note() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let flow = Flow::start(
        "busy-level",
        Options {
            permission: PermissionLevel::AcceptEdits,
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
    set_level(&flow, PermissionLevel::Full).await;

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
    assert_eq!(seams(&flow), 0, "no seam");
    flow.stop().await;
}
