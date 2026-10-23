use std::sync::Arc;
use std::time::Duration;

use brigadier_providers::{ProviderEvent, TurnStatus};
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::work::RequestState;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// Hooks and bisect export another checkout's git paths. A flow must still create and
/// commit only its own repository. Use a child so no other test's environment is changed.
#[tokio::test]
async fn a_flow_leaves_the_callers_git_repository_untouched() {
    const CHILD: &str = "BRIGADIER_FLOW_GIT_ENV_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let flow = Flow::start(
            "20261018-1945-issues-run-2-w38-git-env-child",
            Options::default(),
            script(|_| async { Reply::text("Done.") }),
        )
        .await;
        assert_eq!(
            super::git(&flow.repo, &["rev-list", "--count", "HEAD"]),
            "1"
        );
        assert_eq!(
            super::git(&flow.repo, &["log", "-1", "--format=%s"]),
            "Start"
        );
        flow.stop().await;
        return;
    }
    let scratch = super::Scratch::new("20261018-1945-issues-run-2-w38-git-env");
    super::git(&scratch, &["init", "-q", "-b", "main"]);
    std::fs::write(scratch.join("caller.txt"), "keep this\n").unwrap();
    super::git(&scratch, &["add", "caller.txt"]);
    super::git(&scratch, &["commit", "-q", "-m", "Caller"]);
    let head = super::git(&scratch, &["rev-parse", "HEAD"]);
    let index = std::fs::read(scratch.join(".git/index")).unwrap();
    for all_paths in [false, true] {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                &format!(
                    "{}::a_flow_leaves_the_callers_git_repository_untouched",
                    module_path!().split_once("::").unwrap().1
                ),
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("GIT_DIR", scratch.join(".git"));
        if all_paths {
            command
                .env("GIT_COMMON_DIR", scratch.join(".git"))
                .env("GIT_WORK_TREE", &*scratch)
                .env("GIT_INDEX_FILE", scratch.join(".git/index"))
                .env("GIT_OBJECT_DIRECTORY", scratch.join(".git/objects"))
                .env(
                    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                    scratch.join(".git/objects"),
                )
                .env("GIT_PREFIX", "caller/");
        }
        let output = command.output().unwrap();
        assert_eq!(super::git(&scratch, &["rev-parse", "HEAD"]), head);
        assert_eq!(std::fs::read(scratch.join(".git/index")).unwrap(), index);
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }
}

/// The orchestrator's side of a run where it ends its turn while `jobs` commands it started in
/// the background still run. Once `release` is notified one ends: with `exits`, the CLI dies
/// with it; otherwise its end starts a turn of the CLI's own, which answers.
fn orchestrator_waits_for_its_command(
    jobs: u32,
    exits: bool,
    release: Arc<tokio::sync::Notify>,
) -> Script {
    script(move |turn| {
        let release = release.clone();
        async move {
            // Its CLI died with the command: told so, it answers.
            if turn.input.contains("stopped when your CLI ended") {
                return Reply::text("The app was cut off.");
            }
            let events = turn.events.clone();
            let _ = events
                .send(ProviderEvent::BackgroundJobs {
                    running: (0..jobs).map(|job| format!("job-{job}")).collect(),
                })
                .await;
            tokio::spawn(async move {
                release.notified().await;
                if exits {
                    let _ = events
                        .send(ProviderEvent::Exited {
                            code: None,
                            signal: Some(9),
                            stderr_tail: None,
                        })
                        .await;
                    return;
                }
                let _ = events
                    .send(ProviderEvent::BackgroundJobs {
                        running: (1..jobs).map(|job| format!("job-{job}")).collect(),
                    })
                    .await;
                let _ = events
                    .send(ProviderEvent::TurnStarted { turn_id: None })
                    .await;
                let _ = events
                    .send(ProviderEvent::Message {
                        item_id: "after-the-command".into(),
                        role: brigadier_providers::Role::Assistant,
                        text: "The app runs.".into(),
                    })
                    .await;
                let _ = events
                    .send(ProviderEvent::TurnCompleted {
                        turn_id: None,
                        status: TurnStatus::Completed,
                        duration_ms: Some(1),
                        usage: None,
                    })
                    .await;
            });
            Reply::text("")
        }
    })
}

/// Waits until the request works on with its turn over, for its `jobs` commands alone.
async fn waiting_for_the_command(flow: &Flow, jobs: u32) -> String {
    let board = flow
        .until("the request to wait for its command", |board| {
            board.run == crate::work::RunState::Idle
                && board.requests.values().any(|request| {
                    request.state == RequestState::Working && request.background_jobs == Some(jobs)
                })
        })
        .await;
    board.requests.keys().next().unwrap().clone()
}

/// A request whose orchestrator ended its turn while its own background command runs still
/// works, saying it waits for the command, until the turn the command's end starts is over:
/// that turn's answer is the request's.
#[tokio::test]
async fn a_request_works_until_its_background_commands_turn_is_over() {
    let release = Arc::new(tokio::sync::Notify::new());
    let flow = Flow::start(
        "orchestrator-background",
        Options::default(),
        orchestrator_waits_for_its_command(1, false, release.clone()),
    )
    .await;
    flow.say("Start the app.").await;
    let request = waiting_for_the_command(&flow, 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        flow.board().await.requests[&request].state,
        RequestState::Working
    );
    release.notify_one();
    let board = flow.settled().await;
    let done = &board.requests[&request];
    assert_eq!(done.state, RequestState::Done);
    assert_eq!(done.background_jobs, None);
    let messages = flow.core.all_messages(&flow.conversation).await.unwrap();
    let answer = messages
        .iter()
        .find(|message| message.text == "The app runs.")
        .expect("the answer is stored");
    assert_eq!(answer.request_id.as_deref(), Some(request.as_str()));
    flow.stop().await;
}

/// A CLI that dies while its background command runs takes the wait along: the thread hears
/// the command was cut off, in its request, and the request ends on its answer.
#[tokio::test]
async fn a_cli_that_exits_ends_its_requests_wait_for_its_command() {
    let release = Arc::new(tokio::sync::Notify::new());
    let flow = Flow::start(
        "orchestrator-background-exit",
        Options::default(),
        orchestrator_waits_for_its_command(1, true, release.clone()),
    )
    .await;
    flow.say("Start the app.").await;
    let request = waiting_for_the_command(&flow, 1).await;
    release.notify_one();
    let board = flow.settled().await;
    let ended = &board.requests[&request];
    assert_ne!(ended.state, RequestState::Working);
    assert_eq!(ended.background_jobs, None);
    let messages = flow.core.all_messages(&flow.conversation).await.unwrap();
    let answer = messages
        .iter()
        .find(|message| message.text == "The app was cut off.")
        .expect("the thread heard its command was cut off");
    assert_eq!(answer.request_id.as_deref(), Some(request.as_str()));
    flow.stop().await;
}

/// The user's Stop ends a request that waits for its commands, and one of them ending later
/// doesn't make it work again: the turn its end starts is the CLI's own.
#[tokio::test]
async fn a_stopped_request_stays_stopped_when_its_command_ends() {
    let release = Arc::new(tokio::sync::Notify::new());
    let flow = Flow::start(
        "orchestrator-background-stop",
        Options::default(),
        orchestrator_waits_for_its_command(2, false, release.clone()),
    )
    .await;
    flow.say("Start the app.").await;
    let request = waiting_for_the_command(&flow, 2).await;
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    flow.until("the request to stop", |board| {
        board.requests[&request].state == RequestState::Stopped
    })
    .await;
    release.notify_one();
    super::eventually_async("the command's turn to answer", || async {
        let messages = flow.core.all_messages(&flow.conversation).await.unwrap();
        messages
            .iter()
            .any(|message| message.text == "The app runs.")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let board = flow.board().await;
    assert_eq!(board.requests[&request].state, RequestState::Stopped);
    assert_eq!(board.requests[&request].background_jobs, None);
    flow.stop().await;
}

/// An error that fails a request's turn is said once, by the failed request: no notice above
/// the composer repeats it, and the failed run names the request so the app adds no line of
/// its own.
#[tokio::test]
async fn a_turns_error_is_said_once_by_its_request() {
    const ERROR: &str = "API Error: 400 messages.15.content.0.tool_result: content cannot be empty";
    let flow = Flow::start(
        "turn-error-once",
        Options::default(),
        script(|_turn| async { Reply::failed(ERROR) }),
    )
    .await;
    flow.say("Install cowsay.").await;
    let board = flow
        .until("the request failed", |board| {
            board
                .requests
                .values()
                .any(|request| matches!(request.state, RequestState::Failed { .. }))
        })
        .await;
    let request = board.requests.values().next().unwrap();
    assert_eq!(
        request.state,
        RequestState::Failed {
            error: ERROR.into()
        }
    );
    flow.settled().await;
    let events = flow.events().await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, crate::model::DomainEvent::ConversationNotice { .. })),
        "no notice repeats it"
    );
    flow.stop().await;
}

/// A copy of a real store (`BRIGADIER_FLOW_STORE`, made read-only with `sqlite3 "file:…?mode=ro"
/// ".backup …"`): the thread engine's first start deletes every conversation in it, and they
/// all load long enough to be deleted. Run by hand: `BRIGADIER_FLOW_STORE=/tmp/brig-store-copy.db
/// cargo test -p brigadier-core --lib real_store -- --ignored`.
#[tokio::test]
#[ignore = "needs a copy of a real store"]
async fn a_real_store_is_cleared_on_the_first_start() {
    let store = std::env::var_os("BRIGADIER_FLOW_STORE").expect("BRIGADIER_FLOW_STORE");
    let flow = Flow::start(
        "real-store",
        Options {
            store: Some(store.into()),
            ..Options::default()
        },
        script(|_| async { Reply::text("") }),
    )
    .await;
    let old = flow.core.catalog().conversations.len() - 1;
    let visible = flow.core.visible_catalog().conversations;
    assert_eq!(
        visible.len(),
        1,
        "only the session made after the first start"
    );
    let deadline = tokio::time::Instant::now() + *super::PATIENCE;
    while flow.core.catalog().conversations.len() > 1 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{} conversations are still being deleted",
            flow.core.catalog().conversations.len() - 1
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    eprintln!("deleted {old} conversations");
    flow.stop().await;
}

/// What every scripted CLI was asked, in order.
type Heard = Arc<std::sync::Mutex<Vec<String>>>;

/// The task numbers of the reports in an orchestrator's input, in order.
fn reports_in(input: &str) -> Vec<u32> {
    input
        .split("[report task-")
        .skip(1)
        .filter_map(|rest| {
            rest.split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse()
                .ok()
        })
        .collect()
}

#[tokio::test]
async fn a_codex_thread_s_own_commands_and_edits_are_tool_steps() {
    use crate::work::OrchestratorStepKind;
    use brigadier_providers::{
        FileChange, FileChangeKind, ItemStatus, ProviderEvent, ProviderKind,
    };
    let flow = Flow::start(
        "codex-shell-steps",
        Options {
            thread: ProviderKind::Codex,
            ..Options::default()
        },
        script(|turn| async move {
            for status in [ItemStatus::InProgress, ItemStatus::Completed] {
                turn.events
                    .send(ProviderEvent::Command {
                        item_id: "cmd-1".into(),
                        command: "/bin/zsh -lc 'cargo test -p core'".into(),
                        cwd: None,
                        status,
                        exit_code: (status == ItemStatus::Completed).then_some(0),
                        output: None,
                        duration_ms: None,
                        kept: None,
                    })
                    .await
                    .unwrap();
            }
            turn.events
                .send(ProviderEvent::FileChanges {
                    item_id: "patch-1".into(),
                    changes: vec![
                        FileChange {
                            original: None,
                            path: "src/a.rs".into(),
                            moved_to: None,
                            kind: FileChangeKind::Update,
                            diff: None,
                            added: None,
                            removed: None,
                        },
                        FileChange {
                            original: None,
                            path: "src/b.rs".into(),
                            moved_to: None,
                            kind: FileChangeKind::Add,
                            diff: None,
                            added: None,
                            removed: None,
                        },
                    ],
                    status: ItemStatus::Completed,
                })
                .await
                .unwrap();
            Reply::text("Tests pass.")
        }),
    )
    .await;
    flow.say("Run the tests").await;
    let board = flow.settled().await;
    let tools: Vec<(String, Option<String>, ItemStatus)> = board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            OrchestratorStepKind::Tool {
                name,
                detail,
                status,
                ..
            } => Some((name.clone(), detail.clone(), *status)),
            _ => None,
        })
        .collect();
    assert_eq!(
        tools,
        vec![
            (
                "shell".to_owned(),
                Some("cargo test -p core".to_owned()),
                ItemStatus::Completed
            ),
            (
                "apply_patch".to_owned(),
                Some("src/a.rs and 1 more".to_owned()),
                ItemStatus::Completed
            ),
        ]
    );
    flow.stop().await;
}

#[tokio::test]
async fn a_finished_tool_step_says_when_it_ended_and_how_it_exited_and_opens_in_full() {
    use crate::work::{OrchestratorStepKind, OutputSource};
    use brigadier_providers::{ItemStatus, ProviderEvent};
    let digest = Arc::new(std::sync::Mutex::new(String::new()));
    let sent = digest.clone();
    let flow = Flow::start(
        "thread-items",
        Options::default(),
        script(move |turn| {
            let digest = sent.lock().unwrap().clone();
            async move {
                for status in [ItemStatus::InProgress, ItemStatus::Failed] {
                    let finished = status != ItemStatus::InProgress;
                    turn.events
                        .send(ProviderEvent::Command {
                            item_id: "cmd-1".into(),
                            command: "pnpm test".into(),
                            cwd: None,
                            status,
                            exit_code: finished.then_some(1),
                            output: finished.then(|| "1 failed".into()),
                            duration_ms: finished.then_some(41_000),
                            kept: None,
                        })
                        .await
                        .unwrap();
                }
                for (status, output) in [
                    (ItemStatus::InProgress, None),
                    (ItemStatus::Completed, Some(digest)),
                ] {
                    turn.events
                        .send(ProviderEvent::ToolCall {
                            item_id: "run-1".into(),
                            name: "mcp__brigadier__run".into(),
                            input: Some(json!({"command": "cargo test"}).to_string()),
                            status,
                            output,
                            kept: None,
                        })
                        .await
                        .unwrap();
                }
                Reply::text("One test fails.")
            }
        }),
    )
    .await;
    // A long `run` output the model got as a digest: its row opens to the whole output.
    let full = (0..3000)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let (_, stored) = flow
        .manager
        .store_output(
            &flow.conversation,
            OutputSource::Run,
            "exit 2",
            full.clone().into_bytes(),
            true,
        )
        .await
        .unwrap();
    assert!(stored.len() < full.len());
    // What the reply says after the output stays with it (a run_check's commit, say).
    let note = "Committed 1a2b3c4 \"Fix\" with your trailer: 1 file changed (a.rs). Nothing is left uncommitted.";
    *digest.lock().unwrap() = format!("{stored}\n{note}");
    flow.say("Run the tests").await;
    let board = flow.settled().await;
    let tools: Vec<_> = board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            OrchestratorStepKind::Tool {
                item_id,
                status,
                ended_at_ms,
                exit,
                ..
            } => Some((step.at_ms, item_id.clone(), *status, *ended_at_ms, *exit)),
            _ => None,
        })
        .collect();
    assert_eq!(tools.len(), 2, "{tools:?}");
    for (at_ms, item_id, status, ended_at_ms, exit) in &tools {
        let ended = ended_at_ms.expect("a finished call says when it ended");
        assert!(ended >= *at_ms, "{item_id} ended before it started");
        match item_id.as_str() {
            "cmd-1" => assert_eq!((*status, *exit), (ItemStatus::Failed, Some(1))),
            "run-1" => assert_eq!((*status, *exit), (ItemStatus::Completed, Some(2))),
            other => panic!("unexpected step {other}"),
        }
    }

    flow.stop().await;
}

#[tokio::test]
async fn orchestrator_tools_are_visible_while_running_and_keep_their_first_position() {
    use crate::work::OrchestratorStepKind;
    use brigadier_providers::{ItemStatus, ProviderEvent};
    let release = Arc::new(tokio::sync::Notify::new());
    let gate = release.clone();
    let mut flow = Flow::start(
        "live-tools",
        Options::default(),
        script(move |turn| {
            let gate = gate.clone();
            async move {
                turn.events
                    .send(ProviderEvent::ToolCall {
                        item_id: "brain-call".into(),
                        name: "mcp__brigadier__query_brain".into(),
                        input: None,
                        status: ItemStatus::InProgress,
                        output: None,
                        kept: None,
                    })
                    .await
                    .unwrap();
                gate.notified().await;
                turn.events
                    .send(ProviderEvent::ToolCall {
                        item_id: "brain-call".into(),
                        name: "mcp__brigadier__query_brain".into(),
                        input: Some(json!({"query":"composer attachments"}).to_string()),
                        status: ItemStatus::Completed,
                        output: Some("Found the composer".into()),
                        kept: None,
                    })
                    .await
                    .unwrap();
                Reply::text("The composer handles attachments.")
            }
        }),
    )
    .await;
    flow.say("Find the composer").await;
    let running = flow
        .until("a running tool in the thread", |board| {
            board.orchestrator_steps.iter().any(|step| {
                matches!(
                    step.kind,
                    OrchestratorStepKind::Tool {
                        status: ItemStatus::InProgress,
                        ..
                    }
                )
            })
        })
        .await;
    let first = &running.orchestrator_steps[0];
    assert!(first.request_id.is_some());
    assert!(
        running
            .requests
            .values()
            .any(|request| request.state == RequestState::Working)
    );
    release.notify_one();
    let finished = flow.settled().await;
    assert_eq!(finished.orchestrator_steps.len(), 1);
    let last = &finished.orchestrator_steps[0];
    assert_eq!(last.position, first.position);
    assert_eq!(last.at_ms, first.at_ms);
    assert!(
        matches!(&last.kind, OrchestratorStepKind::Tool { name, detail: Some(detail), status: ItemStatus::Completed, through_position, .. }
        if name == "query_brain" && detail == "composer attachments" && *through_position > last.position)
    );
    flow.restart().await;
    let restored = flow.board().await;
    assert_eq!(restored.orchestrator_steps, finished.orchestrator_steps);
    flow.stop().await;
}
