//! Checks and reviews are plain commands the thread runs in its
//! own shell (STRIP-DOWN-PLAN.md S4, S5): no check or review tool, no build lease. Brigadier starts
//! none of that work by itself; it stores what the tools say and sends it as events.

use std::sync::Arc;

use serde_json::json;

use brigadier_providers::{ItemStatus, ProviderEvent};

use super::{Flow, Options, Reply, Script, Turn};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// The project's full checks, as `tools/full-checks.sh`: they print what ran and fail.
const FULL_CHECKS: &str =
    "#!/bin/sh\necho 'core tests: 12 passed'\necho 'lint: 1 problem in src/a.rs' >&2\nexit 3\n";

/// Runs `command` with `sh -c` in `dir` as a CLI's own shell does, with `path` first on PATH:
/// its exit code, and its output (stdout, then stderr) as the CLI's `Bash` result reads it
/// ("Exit code N" first for a failure).
async fn shell(
    dir: &std::path::Path,
    command: &str,
    path: Option<&std::path::Path>,
) -> (i32, String) {
    let mut run = tokio::process::Command::new("/bin/sh");
    run.arg("-c").arg(command).current_dir(dir);
    if let Some(path) = path {
        let inherited = std::env::var("PATH").unwrap_or_default();
        run.env("PATH", format!("{}:{inherited}", path.display()));
    }
    let out = tokio::time::timeout(*super::PATIENCE, run.output())
        .await
        .expect("the command ended")
        .unwrap();
    let code = out.status.code().unwrap_or(-1);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let result = if code == 0 {
        text
    } else {
        format!("Exit code {code}\n{text}")
    };
    (code, result)
}

/// A `Bash` call of the thread's CLI, as Claude Code sends it.
fn bash(item: &str, command: &str, status: ItemStatus, output: Option<String>) -> ProviderEvent {
    ProviderEvent::ToolCall {
        item_id: item.into(),
        name: "Bash".into(),
        input: Some(json!({ "command": command }).to_string()),
        status,
        output,
        kept: None,
    }
}

/// S4: the thread runs the project's full checks itself, through its own shell, as a plain
/// command: its tool row shows the command's real exit code, and the row's item its real
/// output. No Brigadier check, lease or machine row is in between.
#[tokio::test]
async fn the_threads_full_checks_are_a_plain_command_with_their_real_exit_and_output() {
    let flow = Flow::start(
        "plain-full-checks",
        Options {
            files: vec![("tools/full-checks.sh", FULL_CHECKS)],
            ..Options::default()
        },
        script(|turn| async move {
            if !turn.is_orchestrator() || !turn.input.contains("Run the full checks.") {
                return Reply::text("Noted.");
            }
            let command = "sh tools/full-checks.sh";
            turn.emit(bash("full-checks", command, ItemStatus::InProgress, None))
                .await;
            let (code, output) = shell(&turn.cwd, command, None).await;
            assert_eq!(code, 3, "{output}");
            turn.emit(bash(
                "full-checks",
                command,
                ItemStatus::Failed,
                Some(output),
            ))
            .await;
            Reply::text("The full checks fail: lint has 1 problem in src/a.rs.")
        }),
    )
    .await;
    flow.say("Run the full checks.").await;
    let board = flow.settled().await;
    let request = board.latest_request().unwrap().id.clone();
    let exit = board
        .orchestrator_steps
        .iter()
        .find_map(|step| match &step.kind {
            crate::work::OrchestratorStepKind::Tool { item_id, exit, .. }
                if item_id == "full-checks" =>
            {
                Some(*exit)
            }
            _ => None,
        });
    assert_eq!(exit, Some(Some(3)), "{:#?}", board.orchestrator_steps);
    let page = flow
        .manager
        .thread_items(&flow.conversation, &request, None, None, 100)
        .await
        .unwrap();
    let output = page.entries.iter().find_map(|entry| match &entry.event {
        ProviderEvent::ToolCall {
            item_id, output, ..
        } if item_id == "full-checks" => output.clone(),
        _ => None,
    });
    assert_eq!(
        output.as_deref(),
        Some("Exit code 3\ncore tests: 12 passed\nlint: 1 problem in src/a.rs\n")
    );
    assert!(board.machine_steps.is_empty(), "{:#?}", board.machine_steps);
    flow.stop().await;
}

/// A stand-in `cargo`: `cargo test` marks its session in `$RENDEZVOUS` and waits until the
/// other session's has started too, so it ends well only while both run at once.
const CARGO: &str = r#"#!/bin/sh
touch "$RENDEZVOUS/$SESSION"
tries=0
while [ "$(ls "$RENDEZVOUS" | wc -l)" -lt 2 ]; do
  tries=$((tries + 1))
  [ "$tries" -gt 1200 ] && { echo "ran alone"; exit 1; }
  sleep 0.05
done
echo "test result: ok. 3 passed"
"#;

/// S4: two sessions' heavy commands (`cargo test`) start at once: nothing queues one behind
/// the other, and no lease or machine row shows in either thread.
#[tokio::test]
async fn two_sessions_heavy_commands_run_at_once_without_a_lease_row() {
    let bin = super::Scratch::new("plain-cargo-bin");
    let rendezvous = bin.join("rendezvous");
    std::fs::create_dir_all(&rendezvous).unwrap();
    let cargo = bin.join("cargo");
    std::fs::write(&cargo, CARGO).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (dir, meet) = (bin.to_path_buf(), rendezvous.clone());
    let flow = Flow::start(
        "plain-heavy-commands",
        Options::default(),
        script(move |turn| {
            let (dir, meet) = (dir.clone(), meet.clone());
            async move {
                if !turn.is_orchestrator() || !turn.input.contains("Run the tests.") {
                    return Reply::text("Noted.");
                }
                let session = turn.conversation().unwrap();
                let command = format!(
                    "RENDEZVOUS='{}' SESSION='{}' cargo test",
                    meet.display(),
                    session.0
                );
                turn.emit(bash("cargo-test", &command, ItemStatus::InProgress, None))
                    .await;
                let (code, output) = shell(&turn.cwd, &command, Some(&dir)).await;
                assert_eq!(code, 0, "{output}");
                turn.emit(bash(
                    "cargo-test",
                    &command,
                    ItemStatus::Completed,
                    Some(output),
                ))
                .await;
                Reply::text("The tests pass.")
            }
        }),
    )
    .await;
    let a = flow.conversation.clone();
    let b = flow
        .add_session("Second", None, brigadier_providers::ProviderKind::Claude)
        .await;
    flow.say_to(&a, "Run the tests.").await;
    flow.say_to(&b, "Run the tests.").await;
    for id in [&a, &b] {
        let board = flow.settled_in(id).await;
        // Claude Code names no exit code for a command that succeeded.
        let row = board
            .orchestrator_steps
            .iter()
            .find_map(|step| match &step.kind {
                crate::work::OrchestratorStepKind::Tool {
                    item_id,
                    status,
                    exit,
                    ..
                } if item_id == "cargo-test" => Some((*status, *exit)),
                _ => None,
            });
        assert_eq!(
            row,
            Some((ItemStatus::Completed, None)),
            "{:#?}",
            board.orchestrator_steps
        );
        assert!(board.machine_steps.is_empty(), "{:#?}", board.machine_steps);
        let leased = flow
            .events_of(id)
            .await
            .into_iter()
            .any(|event| matches!(event, crate::model::DomainEvent::MachineStepped { .. }));
        assert!(!leased, "a machine row in {id:?}");
    }
    assert_eq!(std::fs::read_dir(&rendezvous).unwrap().count(), 2);
    flow.stop().await;
}
