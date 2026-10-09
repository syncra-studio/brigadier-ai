//! Lossless trimming of the thread's command output (THREAD-PLAN.md Q4): the Claude hook's
//! grant and what it stores, `read_artifact` on an `out-<id>` alias (its owner only, paged
//! back byte for byte, gone with the conversation), and a Codex thread's `run`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_providers::ProviderKind;
use brigadier_providers::model::{Origin, SessionSpec};
use serde_json::json;

use super::{Flow, Options, Reply, Scratch, Script, Turn};
use crate::digest::DIGEST_MAX;
use crate::manager::{HOOK_GRANT_ENV, HookOutput};
use crate::model::{
    ConversationKind, EnvironmentRequest, ModelChoice, PermissionLevel, SetupRequest,
};
use crate::tools::{OrchestratorCall, ReadArtifact, Role, RunTools, ToolCall, ToolHost};
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
    // Its tools: `run` too (the user's ruling of 2026-10-07); its own Bash output is trimmed
    // by the hook. At Full access there is no sandbox to leave.
    assert_eq!(
        flow.manager.grants.resolve(&grant),
        Some(Role::Orchestrator {
            conversation_id: flow.conversation.clone(),
            run: RunTools::Run,
        })
    );
    // Its run_check waits for its command as a Codex thread's run does.
    assert_eq!(
        spec.mcp_servers[0].tool_timeout_secs,
        Some(super::super::run::RUN_TIMEOUT_MAX.as_secs() + 60)
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
                permission: PermissionLevel::FullAccess,
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
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while blobs.get(hash.clone()).await.unwrap().is_some() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the output outlived its conversation"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    flow.stop().await;
}

/// A Codex thread runs commands through `run`, with a 30-minute tool timeout; under Ask for
/// approval it also has `run_unsandboxed`, whose calls Codex asks about first. It has no hook.
#[tokio::test]
async fn a_codex_thread_runs_commands_through_brigadier() {
    for (permission, run, prompted) in [
        (
            PermissionLevel::FullAccess,
            RunTools::Run,
            Vec::<String>::new(),
        ),
        (PermissionLevel::ApproveForMe, RunTools::Run, Vec::new()),
        (
            PermissionLevel::AskForApproval,
            RunTools::WithEscalation,
            vec!["run_unsandboxed".to_owned()],
        ),
    ] {
        let flow = Flow::start(
            "trim-codex",
            Options {
                permission,
                thread: ProviderKind::Codex,
                ..Options::default()
            },
            script(|_| async { Reply::text("Done.") }),
        )
        .await;
        flow.say("Hello.").await;
        flow.settled().await;
        let (spec, grant, hook) = thread(&flow);
        assert!(
            hook.is_none() && spec.output_hook.is_none(),
            "{permission:?}"
        );
        // Longer than run's longest command, so the call returns its status and output.
        assert_eq!(
            spec.mcp_servers[0].tool_timeout_secs,
            Some(super::super::run::RUN_TIMEOUT_MAX.as_secs() + 60)
        );
        assert_eq!(spec.mcp_servers[0].prompt_tools, prompted, "{permission:?}");
        assert_eq!(
            flow.manager.grants.resolve(&grant),
            Some(Role::Orchestrator {
                conversation_id: flow.conversation.clone(),
                run,
            })
        );
        flow.stop().await;
    }
}

/// At Full access `run` runs the command itself: a short result comes back as it is, a long
/// failing one as its digest with the error lines first, its whole output stored.
#[tokio::test]
async fn run_returns_short_output_as_is_and_long_failures_as_a_digest() {
    let replies: Arc<Mutex<Vec<(String, bool)>>> = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        "trim-run",
        Options {
            thread: ProviderKind::Codex,
            ..Options::default()
        },
        script(move |turn| {
            let log = log.clone();
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("Done.");
                }
                for args in [
                    json!({ "command": "echo hello; echo oops >&2" }),
                    json!({
                        "command": "i=0; while [ $i -lt 3000 ]; do echo \"test case_$i ... ok\"; \
                                    i=$((i+1)); done; echo 'error: the build broke' >&2; exit 3",
                        "timeout_secs": 60,
                    }),
                    json!({ "command": "pwd", "workdir": "/" }),
                ] {
                    let reply = turn.call("run", args).await;
                    log.lock().unwrap().push((reply.text, reply.is_error));
                }
                Reply::text("Ran them.")
            }
        }),
    )
    .await;
    flow.say("Run the tests.").await;
    flow.settled().await;
    let replies = replies.lock().unwrap().clone();
    assert_eq!(replies.len(), 3);
    let (short, error) = &replies[0];
    assert!(!error);
    assert_eq!(short, "[exit 0]\nhello\noops\n");
    let (digest, error) = &replies[1];
    assert!(!error, "{digest}");
    assert!(digest.len() <= DIGEST_MAX, "{}", digest.len());
    let mut lines = digest.lines();
    assert!(
        lines
            .next()
            .unwrap()
            .starts_with("exit 3 [full output: read_artifact out-")
    );
    lines.next();
    assert_eq!(lines.next(), Some("error: the build broke"));
    let board = flow.board().await;
    let stored = &board.outputs[&alias(digest)];
    assert_eq!(stored.source, OutputSource::Run);
    assert_eq!(stored.status, "exit 3");
    assert_eq!(stored.lines, 3001);
    // Outside the workspace and the scratch folder: refused.
    let (refused, error) = &replies[2];
    assert!(error, "{refused}");
    assert!(refused.contains("outside the workspace"), "{refused}");
    // No process of its is left recorded under the thread.
    let owned = flow
        .manager
        .runtime
        .ledger()
        .artifacts(&format!("orch:{}", flow.conversation));
    let threads_cli = flow.thread_specs().len();
    assert!(threads_cli >= 1);
    assert!(
        !owned
            .iter()
            .any(|artifact| matches!(artifact, brigadier_providers::Artifact::Process { .. })),
        "{owned:?}"
    );
    flow.stop().await;
}

/// A command that outlives its timeout is killed with what it started, and so is one whose
/// thread's CLI ends.
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
    let mut spec = env.spec(std::path::Path::new("/bin/sh"));
    spec.args = vec![
        "-c".into(),
        format!(
            "sleep 60 & echo $! > {}; echo started; wait",
            pid_file.display()
        )
        .into(),
    ];
    spec.cwd = Some(dir.to_path_buf());
    let owner = "orch:test-kill";
    let started = tokio::time::Instant::now();
    let ran = super::super::run::run_command(
        platform.clone(),
        &spec,
        Duration::from_secs(1),
        tokio_util::sync::CancellationToken::new(),
        owner,
        flow.manager.runtime.ledger(),
    )
    .await
    .unwrap();
    assert_eq!(ran.status, "timed out after 1 s");
    assert_eq!(ran.output, b"started\n");
    assert!(started.elapsed() < Duration::from_secs(10));
    let child: u32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // Killed, its orphan is gone once launchd reaps it (a zombie still answers a signal).
    for _ in 0..50 {
        if !platform.processes().is_alive(child) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        !platform.processes().is_alive(child),
        "its sleep was killed too"
    );
    assert!(flow.manager.runtime.ledger().artifacts(owner).is_empty());

    // The thread's CLI ends: the command goes with it.
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
    assert_eq!(ran.status, "stopped: the thread's session ended");
    // The thread's session spec is untouched by any of it.
    assert!(
        flow.thread_specs()
            .iter()
            .all(|(_, spec)| matches!(spec.origin, Origin::New | Origin::Resume { .. }))
    );
    flow.stop().await;
}

/// Under Ask for approval `run_unsandboxed` runs only a command the user approved on the card
/// Codex's question became, and only once: its grant alone runs nothing.
#[tokio::test]
async fn run_unsandboxed_runs_only_what_the_user_approved_once() {
    let replies: Arc<Mutex<Vec<(String, bool)>>> = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        "trim-escalate",
        Options {
            permission: PermissionLevel::AskForApproval,
            thread: ProviderKind::Codex,
            ..Options::default()
        },
        script(move |turn| {
            let log = log.clone();
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("Done.");
                }
                let call = || {
                    json!({
                        "command": "echo outside",
                        "justification": "It needs the network.",
                    })
                };
                // Straight to the tool, with no approval: refused.
                let reply = turn.call("run_unsandboxed", call()).await;
                log.lock().unwrap().push((reply.text, reply.is_error));
                // Codex asks first; the user approves the card.
                let decision = turn
                    .ask_tool_approval("run_unsandboxed", "echo outside", None)
                    .await;
                assert_eq!(decision, Some(brigadier_providers::ApprovalDecision::Allow));
                for _ in 0..2 {
                    let reply = turn.call("run_unsandboxed", call()).await;
                    log.lock().unwrap().push((reply.text, reply.is_error));
                }
                Reply::text("Ran it.")
            }
        }),
    )
    .await;
    flow.say("Fetch it.").await;
    let board = flow
        .until("the escalation's card", |board| {
            board
                .approvals
                .values()
                .any(|approval| approval.state == crate::work::CardState::Pending)
        })
        .await;
    let card = board
        .approvals
        .values()
        .find(|approval| approval.state == crate::work::CardState::Pending)
        .unwrap();
    flow.manager
        .answer_card(
            flow.conversation.clone(),
            card.id.clone(),
            brigadier_providers::ApprovalDecision::Allow,
        )
        .await
        .unwrap();
    flow.settled().await;
    let replies = replies.lock().unwrap().clone();
    assert_eq!(replies.len(), 3);
    assert!(replies[0].1, "{}", replies[0].0);
    assert!(replies[0].0.contains("nothing approved this one"));
    assert_eq!(replies[1], ("[exit 0]\noutside\n".to_owned(), false));
    assert!(replies[2].1, "once: {}", replies[2].0);
    flow.stop().await;
}

/// What `run` prints of the project's secret files is hidden from the thread and from the
/// output it stores, in a short result and in a long one's digest and blob alike.
#[tokio::test]
async fn a_runs_output_hides_the_projects_secrets() {
    let replies: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        "trim-secret",
        Options {
            thread: ProviderKind::Codex,
            ..Options::default()
        },
        script(move |turn| {
            let log = log.clone();
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("Done.");
                }
                for args in [
                    // The session's worktree has no copy of the user's `.env`: read the checkout's.
                    json!({ "command": "cat \"$(git rev-parse --git-common-dir)/../.env\"" }),
                    // A byte that isn't UTF-8 doesn't let the rest through.
                    json!({ "command": "cat \"$(git rev-parse --git-common-dir)/../.env\"; printf '\\377\\n'" }),
                    json!({
                        "command": "i=0; while [ $i -lt 3000 ]; do echo \"line $i\"; i=$((i+1)); \
                                    done; echo \"error: TOKEN=$(cut -d= -f2 \"$(git rev-parse --git-common-dir)/../.env\")\"; \
                                    exit 1",
                        "timeout_secs": 60,
                    }),
                ] {
                    let reply = turn.call("run", args).await;
                    log.lock().unwrap().push(reply.text);
                }
                Reply::text("Ran them.")
            }
        }),
    )
    .await;
    std::fs::write(flow.repo.join(".env"), "TOKEN=s3cret-value-123\n").unwrap();
    let conversation = flow.manager.core.conversation(&flow.conversation).unwrap();
    let project = conversation.project_id.unwrap();
    let mut prefs = flow.manager.core.project(&project).unwrap().prefs;
    prefs.secret_files = vec![".env".into()];
    flow.manager
        .core
        .update_project(
            project,
            crate::model::ProjectPatch {
                name: None,
                repos: None,
                prefs: Some(prefs),
            },
        )
        .await
        .unwrap();
    flow.say("Show me the env file.").await;
    flow.settled().await;
    let replies = replies.lock().unwrap().clone();
    assert_eq!(replies.len(), 3, "{replies:?}");
    for reply in &replies {
        assert!(!reply.contains("s3cret-value-123"), "{reply}");
    }
    assert!(replies[0].starts_with("[exit 0]\nTOKEN="), "{}", replies[0]);
    assert!(replies[1].starts_with("[exit 0]\nTOKEN="), "{}", replies[1]);
    assert!(replies[2].contains("error: TOKEN="), "{}", replies[2]);
    let board = flow.board().await;
    let stored = &board.outputs[&alias(&replies[2])];
    let blob = flow
        .manager
        .core
        .store()
        .blobs()
        .get(stored.blob.parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    let text = String::from_utf8(blob).unwrap();
    assert!(
        text.contains("error: TOKEN="),
        "{}",
        &text[text.len() - 100..]
    );
    assert!(!text.contains("s3cret-value-123"));
    flow.stop().await;
}
