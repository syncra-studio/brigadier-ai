//! Lossless trimming of the thread's command output (THREAD-PLAN.md Q4): the Claude hook's
//! grant and what it stores, `read_artifact` on an `out-<id>` alias (its owner only, paged
//! back byte for byte, gone with the conversation), and Brigadier's own command runner.

use std::sync::Arc;
use std::time::Duration;

use brigadier_providers::ProviderKind;
use brigadier_providers::model::{Origin, SessionSpec};

use super::{Flow, Options, Reply, Scratch, Script, Turn};
use crate::digest::DIGEST_MAX;
use crate::manager::{HOOK_GRANT_ENV, HookOutput};
use crate::model::{
    ConversationKind, EnvironmentRequest, ModelChoice, PermissionLevel, SetupRequest,
};
use crate::tools::{OrchestratorCall, ReadArtifact, Role, ToolCall, ToolHost};
use crate::work::OutputSource;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// The thread's first session, and the grants it holds: its tools' and its hook's.
fn thread(flow: &Flow) -> (SessionSpec, String, Option<String>) {
    let specs = flow.thread_specs();
    let (_, spec) = specs.first().expect("the thread started").clone();
    let grant = spec.mcp_servers[0]
        .env
        .iter()
        .find(|(key, _)| key == "BRIGADIER_MCP_GRANT")
        .map(|(_, value)| value.clone())
        .expect("a tools grant");
    let hook = spec.output_hook.as_ref().map(|hook| {
        hook.env
            .iter()
            .find(|(key, _)| key == HOOK_GRANT_ENV)
            .map(|(_, value)| value.clone())
            .expect("a hook grant")
    });
    (spec, grant, hook)
}

/// About 50 KB of a test log: passing lines with a few warnings and errors, and characters of
/// two, three and four bytes so that pages end inside them.
fn fifty_kb() -> Vec<u8> {
    let mut log = String::new();
    let mut n = 0;
    while log.len() < 50_000 {
        let line = match n % 23 {
            13 => format!("warning: unused variable `é{n}`\n"),
            20 => format!("error[E0308]: mismatched types → {n} 😀\n"),
            _ => format!("test tests::case_{n:05} … ok\n"),
        };
        log.push_str(&line);
        n += 1;
    }
    log.into_bytes()
}

/// The `out-<id>` a digest names.
fn alias(digest: &str) -> String {
    digest
        .split("read_artifact ")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .expect("an alias")
        .to_owned()
}

/// Reads `alias` back page by page as the thread would, and joins the pages.
async fn read_back(flow: &Flow, grant: &str, alias: &str) -> String {
    let mut whole = String::new();
    let mut offset = 0u64;
    loop {
        let reply = ToolHost::call(
            &*flow.manager,
            grant,
            ToolCall::Orchestrator(OrchestratorCall::ReadArtifact(ReadArtifact {
                id: alias.to_owned(),
                offset: Some(offset),
                limit: None,
            })),
        )
        .await;
        assert!(!reply.is_error, "{}", reply.text);
        let (header, rest) = reply.text.split_once('\n').unwrap();
        // "[artifact out-… bytes A–B of T]"
        let range = header.split(" bytes ").nth(1).unwrap();
        let (from, rest_of) = range.split_once('–').unwrap();
        let (to, total) = rest_of.trim_end_matches(']').split_once(" of ").unwrap();
        assert_eq!(from.parse::<u64>().unwrap(), offset);
        let to: u64 = to.parse().unwrap();
        assert!(to - offset <= 16_000, "a page holds at most 16,000 bytes");
        let page = match rest.rsplit_once("\n[more: read_artifact with offset ") {
            Some((page, _)) => page,
            None => rest,
        };
        whole.push_str(page);
        offset = to;
        if to == total.parse::<u64>().unwrap() {
            return whole;
        }
    }
}

/// A Claude thread's hook stores a long successful output and gets its digest back; the
/// thread reads it whole, page by page, byte for byte; another session can't; and it goes
/// with the conversation. A failure's excerpt is stored as such and replaces nothing.
#[tokio::test]
async fn a_trimmed_output_is_read_back_whole_by_its_own_session_only() {
    let flow = Flow::start(
        "trim-hook",
        Options::default(),
        script(|_| async { Reply::text("Done.") }),
    )
    .await;
    flow.say("Run the tests.").await;
    flow.settled().await;
    let (spec, grant, hook) = thread(&flow);
    let hook = hook.expect("a Claude thread has its output hook");
    let output_hook = spec.output_hook.as_ref().unwrap();
    assert_eq!(output_hook.args[..2], ["hook", "post-tool-use"]);
    assert_eq!(
        flow.manager.grants.resolve(&hook),
        Some(Role::OutputHook {
            conversation_id: flow.conversation.clone()
        })
    );
    // Its own Bash output is trimmed by the hook.
    assert_eq!(
        flow.manager.grants.resolve(&grant),
        Some(Role::Orchestrator {
            conversation_id: flow.conversation.clone(),
        })
    );
    // Its blocking calls (a wait for replies) may take half an hour.
    assert_eq!(
        spec.mcp_servers[0].tool_timeout_secs,
        Some(super::super::conversation::RUNNER_TOOL_TIMEOUT_SECS)
    );

    // Only its hook's grant stores output; a short output is left alone.
    let log = fifty_kb();
    assert!(
        flow.manager
            .hook_output(&grant, HookOutput::Full, "exit 0", log.clone())
            .await
            .is_none()
    );
    assert!(matches!(
        flow.manager
            .hook_output(&hook, HookOutput::Full, "exit 0", b"short\n".to_vec())
            .await,
        Some(Ok(None))
    ));
    // A `cat` of a few source files, 20 kB, reaches the model whole, as the CLI's own limit
    // has it (BUILD-PLAN C10).
    let source = "export const x = 1;\n".repeat(1_000).into_bytes();
    assert!(source.len() > 16_000 && source.len() <= crate::digest::BASH_WHOLE_MAX);
    assert!(matches!(
        flow.manager
            .hook_output(&hook, HookOutput::Full, "exit 0", source)
            .await,
        Some(Ok(None))
    ));
    assert!(flow.board().await.outputs.is_empty());

    let digest = flow
        .manager
        .hook_output(&hook, HookOutput::Full, "exit 0", log.clone())
        .await
        .expect("a hook grant")
        .expect("stored")
        .expect("a digest");
    assert!(digest.len() <= DIGEST_MAX, "{}", digest.len());
    assert!(digest.starts_with("exit 0 [full output: read_artifact out-"));
    let lines = log.iter().filter(|byte| **byte == b'\n').count();
    assert!(digest.contains(&format!(", {lines} lines, {} bytes]", log.len())));
    assert!(digest.contains("\nerror[E0308]: mismatched types → 20 😀"));
    assert!(digest.contains("more matching lines in the full output]"));
    let alias = alias(&digest);
    let board = flow.board().await;
    let stored = &board.outputs[&alias];
    assert_eq!(stored.source, OutputSource::Bash);
    assert_eq!(stored.bytes, log.len() as u64);
    assert_eq!(stored.shown_bytes, Some(digest.len() as u64));

    // Read back whole, byte for byte.
    let whole = read_back(&flow, &grant, &alias).await;
    assert_eq!(whole.as_bytes(), &log[..]);

    // A failure's excerpt: stored, labelled as such, nothing replaced.
    let excerpt = format!("Exit code 1\n{}", "FAIL: case\n".repeat(900));
    assert!(matches!(
        flow.manager
            .hook_output(
                &hook,
                HookOutput::Excerpt,
                "exit 1",
                excerpt.clone().into_bytes()
            )
            .await,
        Some(Ok(None))
    ));
    let board = flow.board().await;
    let failed = board
        .outputs
        .values()
        .find(|output| output.source == OutputSource::BashExcerpt)
        .expect("the excerpt is stored");
    assert_eq!(failed.bytes, excerpt.len() as u64);
    assert_eq!(failed.shown_bytes, None);
    assert_eq!(read_back(&flow, &grant, &failed.alias).await, excerpt);

    // Another session of the same project can't read it, by alias or by blob.
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id;
    let other = flow
        .manager
        .create_conversation(
            ConversationKind::Session,
            project,
            Some("Other".into()),
            Some(SetupRequest::Session {
                repo: flow.repo.display().to_string(),
                environment: EnvironmentRequest::LocalCheckout {
                    branch: "main".into(),
                    create_from: None,
                },
                permission: PermissionLevel::Full,
                orchestrator: ModelChoice {
                    provider: ProviderKind::Claude,
                    model: Some("claude-opus-5-5".into()),
                    effort: None,
                    fast: None,
                    account: None,
                },
                plan_mode: false,
            }),
        )
        .await
        .unwrap();
    for id in [alias.clone(), stored.blob.clone()] {
        let reply = flow
            .manager
            .orchestrator_call(
                other.id.clone(),
                OrchestratorCall::ReadArtifact(ReadArtifact {
                    id,
                    offset: None,
                    limit: None,
                }),
            )
            .await;
        assert!(reply.is_error, "{}", reply.text);
    }

    // Deleting the conversation takes its outputs.
    let blobs = flow.core.store().blobs().clone();
    let hash: brigadier_store::BlobHash = stored.blob.parse().unwrap();
    assert!(blobs.get(hash.clone()).await.unwrap().is_some());
    flow.manager
        .delete(flow.conversation.clone())
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + *super::PATIENCE;
    while blobs.get(hash.clone()).await.unwrap().is_some() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the output outlived its conversation"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    flow.stop().await;
}

/// A command Brigadier runs (a setup script) that outlives its timeout is killed with what it
/// started, and so is one whose caller goes.
#[tokio::test]
async fn a_run_is_killed_at_its_timeout_and_when_the_thread_ends() {
    let flow = Flow::start(
        "trim-kill",
        Options::default(),
        script(|_| async { Reply::text("Done.") }),
    )
    .await;
    let platform = flow.manager.runtime.platform().clone();
    let env = flow.manager.runtime.cli_env().clone();
    let dir = Scratch::new("run");
    let pid_file = dir.join("child.pid");
    // Made once it has printed: its timeout fires only after that, however slow the machine.
    let printed = dir.join("printed");
    let mut spec = env.spec(std::path::Path::new("/bin/sh"));
    spec.args = vec![
        "-c".into(),
        format!(
            "sleep 60 & echo $! > {}; echo started; : > {}; wait",
            pid_file.display(),
            printed.display()
        )
        .into(),
    ];
    spec.cwd = Some(dir.to_path_buf());
    let owner = "orch:test-kill";
    let gate = Arc::new(tokio::sync::Notify::new());
    super::super::run::hold_timeout(owner, gate.clone());
    let (ran, opened) = tokio::join!(
        super::super::run::run_command(
            platform.clone(),
            &spec,
            Duration::from_secs(1),
            tokio_util::sync::CancellationToken::new(),
            owner,
            flow.manager.runtime.ledger(),
        ),
        async {
            super::eventually("the command to print", || printed.exists()).await;
            gate.notify_one();
            tokio::time::Instant::now()
        }
    );
    let ran = ran.unwrap();
    assert_eq!(ran.status, "timed out after 1 s");
    assert_eq!(ran.output, b"started\n");
    // Killed at its timeout, not waited for.
    assert!(opened.elapsed() < Duration::from_secs(10));
    let child: u32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // Killed, its orphan is gone once launchd reaps it (a zombie still answers a signal).
    super::eventually("its sleep, killed too, to go", || {
        !platform.processes().is_alive(child)
    })
    .await;
    assert!(flow.manager.runtime.ledger().artifacts(owner).is_empty());

    // Its caller goes: the command goes with it.
    let ended = tokio_util::sync::CancellationToken::new();
    let cancel = ended.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        cancel.cancel();
    });
    let ran = super::super::run::run_command(
        platform,
        &spec,
        Duration::from_secs(60),
        ended,
        owner,
        flow.manager.runtime.ledger(),
    )
    .await
    .unwrap();
    assert_eq!(ran.status, "stopped: its caller went away");
    // The thread's session spec is untouched by any of it.
    assert!(
        flow.thread_specs()
            .iter()
            .all(|(_, spec)| matches!(spec.origin, Origin::New | Origin::Resume { .. }))
    );
    flow.stop().await;
}
