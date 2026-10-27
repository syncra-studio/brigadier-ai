//! `brigadier context` (D13) for a session's thread and for one of its workers: the overview
//! stays small, every part answers in its schema, a worker reads its session's ledger and
//! checkpoint without changing them, and follows a hit in its own records to the whole item.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use brigadier_cli::protocol::{CliRequest, ContextPart, Tool};
use serde_json::{Value, json};

use super::cli_tests::{brigadier, tool};
use super::{Flow, Options, Reply, Script};
use crate::manager::prompts::LEDGER_ENV;

const REQUEST: &str = "Plan the move to the new store.";
const SPEC: &str = "Count the call sites of the old store.";
const LEDGER: &str = "- L1 · 10-25 11:00 · gen 1 · Rule from the user: \"no tabs\" (message 1)\n\
                      - L2 · 10-25 11:01 · gen 1 · Decision: keep the old store read-only (I decided)\n\
                      - L3 · 10-25 11:02 · gen 1 · Open: say when the call sites are counted\n";
const CHECKPOINT: &str = "Goal: move to the new store.\nNext: count the call sites.";
/// Planted in the worker's own records: the start of a long reply, and its end, which no
/// search snippet shows.
const HIT: &str = "quokkacompass";
const TAIL: &str = "lemurtrellis";

fn context(part: Option<ContextPart>) -> CliRequest {
    CliRequest::Context {
        part,
        name: None,
        grep: None,
        last: None,
        item: None,
    }
}

fn transcript(grep: Option<&str>, last: Option<u32>, item: Option<&str>) -> CliRequest {
    CliRequest::Context {
        part: Some(ContextPart::Transcript),
        name: None,
        grep: grep.map(Into::into),
        last,
        item: item.map(Into::into),
    }
}

fn ids(output: &Value, source: &str) -> Vec<String> {
    output["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry["source"] == source)
        .filter_map(|entry| entry["id"].as_str().map(str::to_owned))
        .collect()
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A Claude session file of one user message and one long reply, listed in `record`'s lineage.
fn plant_transcript(record: &Path) {
    let file = record.join("cli").join("w.jsonl");
    let at = "2026-10-25T10:00:00.000Z";
    let reply = format!("{HIT} {} {TAIL}", "filler ".repeat(1200));
    let lines = [
        json!({"type": "user", "timestamp": at, "uuid": "u-1",
               "message": {"role": "user", "content": SPEC}}),
        json!({"type": "assistant", "timestamp": at, "uuid": "a-1",
               "message": {"id": "m-1", "role": "assistant",
                           "content": [{"type": "text", "text": reply}]}}),
    ];
    let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
    write(&file, &text);
    write(
        &record.join("lineage.json"),
        &serde_json::to_string(&json!([{"provider": "claude", "sessionId": "w",
            "file": file.display().to_string(), "startedAt": 1}]))
        .unwrap(),
    );
}

/// Every part answers, in its schema (`brigadier` checks it), for the caller.
async fn every_part(turn: &super::Turn) -> Vec<Value> {
    let mut outputs = Vec::new();
    for part in ContextPart::ALL {
        let output = brigadier(turn, context(Some(part))).await;
        assert!(output.get("error").is_none(), "{part:?}: {output:#}");
        assert_eq!(output["part"], part.word());
        assert!(
            output["text"].as_str().unwrap().ends_with('\n'),
            "{output:#}"
        );
        outputs.push(output);
    }
    outputs
}

#[tokio::test]
async fn a_thread_and_its_worker_pull_their_context() {
    let session_ledger = Arc::new(Mutex::new(None::<PathBuf>));
    let worker_done = Arc::new(Mutex::new(false));
    let script: Script = {
        let (session_ledger, worker_done) = (session_ledger.clone(), worker_done.clone());
        Arc::new(move |turn| {
            let (session_ledger, worker_done) = (session_ledger.clone(), worker_done.clone());
            Box::pin(async move {
                let ledger = PathBuf::from(turn.env(LEDGER_ENV).unwrap());
                if turn.worker().is_none() {
                    write(&ledger, LEDGER);
                    *session_ledger.lock().unwrap() = Some(ledger.clone());
                    brigadier(
                        &turn,
                        tool(Tool::Checkpoint, "set", json!({"text": CHECKPOINT})),
                    )
                    .await;
                    turn.write("CLAUDE.md", "Use spaces.\n");

                    let overview = brigadier(&turn, context(None)).await;
                    assert_eq!(overview["part"], "overview");
                    let text = overview["text"].as_str().unwrap();
                    assert!(text.lines().count() < 40, "{text}");
                    let parts: Vec<&str> = overview["parts"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|part| part["part"].as_str().unwrap())
                        .collect();
                    assert_eq!(parts, ContextPart::ALL.map(ContextPart::word));
                    for part in ContextPart::ALL {
                        assert!(
                            text.contains(&format!("  {}", part.word())),
                            "{part:?}: {text}"
                        );
                    }

                    let [
                        brief,
                        waiting,
                        decisions,
                        open,
                        ledger_part,
                        checkpoint,
                        _,
                        _,
                        _,
                        project,
                        routing,
                    ] = <[Value; 11]>::try_from(every_part(&turn).await).unwrap();
                    assert_eq!(brief["text"], format!("{REQUEST}\n"));
                    assert_eq!(waiting["text"], "No explicit waits.\n");
                    assert_eq!(ids(&decisions, "session"), ["L1", "L2"]);
                    assert!(
                        decisions["text"]
                            .as_str()
                            .unwrap()
                            .contains("Rule from the user: \"no tabs\"")
                    );
                    assert_eq!(ids(&open, "session"), ["L3"]);
                    assert_eq!(ids(&ledger_part, "session"), ["L1", "L2", "L3"]);
                    assert_eq!(checkpoint["text"], format!("{CHECKPOINT}\n"));
                    assert_eq!(project["files"][0]["path"], "CLAUDE.md");
                    assert_eq!(project["files"][0]["text"], "Use spaces.\n");
                    assert!(routing["models"].is_object(), "{routing:#}");
                    let thread = brigadier(&turn, transcript(None, Some(3), None)).await;
                    assert!(thread.get("error").is_none(), "{thread:#}");

                    turn.start_worker("Count call sites", SPEC).await;
                    return Reply::text("Started.");
                }

                // The worker: its own brief, its session's ledger read-only, then its own.
                let [
                    brief,
                    waiting,
                    decisions,
                    open,
                    _,
                    checkpoint,
                    _,
                    _,
                    mcps,
                    _,
                    _,
                ] = <[Value; 11]>::try_from(every_part(&turn).await).unwrap();
                assert_eq!(waiting["text"], "No explicit waits.\n");
                assert_eq!(brief["text"], format!("{SPEC}\n"));
                assert_eq!(ids(&decisions, "session"), ["L1", "L2"]);
                assert!(
                    decisions["text"].as_str().unwrap().contains("read-only"),
                    "{decisions:#}"
                );
                assert_eq!(ids(&open, "session"), ["L3"]);
                assert_eq!(
                    checkpoint["text"],
                    format!("Your session's checkpoint:\n\n{CHECKPOINT}\n")
                );
                // Brigadier's test home has no servers: none to have.
                assert!(
                    mcps["items"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .all(|item| item["has"] == false),
                    "{mcps:#}"
                );
                let added = brigadier(
                    &turn,
                    tool(
                        Tool::Ledger,
                        "add",
                        json!({"text": "Decision: count by hand (I decided)"}),
                    ),
                )
                .await;
                assert_eq!(added["text"], "L1\n");
                let whole = brigadier(&turn, context(Some(ContextPart::Ledger))).await;
                assert_eq!(ids(&whole, "session"), ["L1", "L2", "L3"]);
                assert_eq!(ids(&whole, "own"), ["L1"]);
                let text = whole["text"].as_str().unwrap();
                assert!(
                    text.find("session's ledger, read-only").unwrap()
                        < text.find("Your own ledger").unwrap(),
                    "{text}"
                );

                // Its checkout's own instruction file, not its session's.
                turn.write("AGENTS.md", "Count twice.\n");
                let project = brigadier(&turn, context(Some(ContextPart::Project))).await;
                let files: Vec<&str> = project["files"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|file| file["path"].as_str().unwrap())
                    .collect();
                assert!(files.contains(&"AGENTS.md"), "{project:#}");

                // A hit in its own records, followed to the whole item by the command it names.
                plant_transcript(ledger.parent().unwrap());
                let hits = brigadier(&turn, transcript(Some(HIT), None, None)).await;
                let text = hits["text"].as_str().unwrap();
                assert!(!text.contains(TAIL), "the snippet stops short: {text}");
                let command = text
                    .lines()
                    .find_map(|line| line.strip_prefix("`brigadier transcript read "))
                    .unwrap_or_else(|| panic!("no read command in: {text}"));
                let words: Vec<&str> = command.trim_end_matches('`').split(' ').collect();
                let [key, "--item", item] = words[..] else {
                    panic!("{command}");
                };
                let read = brigadier(
                    &turn,
                    tool(
                        Tool::Transcript,
                        "read",
                        json!({"session": key, "item": item}),
                    ),
                )
                .await;
                assert!(read["text"].as_str().unwrap().contains(TAIL), "{read:#}");
                let read = brigadier(&turn, transcript(None, None, Some(item))).await;
                assert!(read["text"].as_str().unwrap().contains(TAIL), "{read:#}");
                let last = brigadier(&turn, transcript(None, Some(1), None)).await;
                assert!(last["text"].as_str().unwrap().contains(TAIL), "{last:#}");
                // Its session's records are not its own.
                let session = key.split('/').next().unwrap();
                let refused = brigadier(
                    &turn,
                    tool(
                        Tool::Transcript,
                        "read",
                        json!({"session": session, "item": item}),
                    ),
                )
                .await;
                assert!(refused.get("error").is_some(), "{refused:#}");
                let brain =
                    brigadier(&turn, tool(Tool::Brain, "search", json!({"query": HIT}))).await;
                assert_eq!(brain["error"]["code"], "refused", "{brain:#}");
                *worker_done.lock().unwrap() = true;
                Reply::text("Counted.")
            })
        })
    };
    let flow = Flow::start("context", Options::default(), script).await;
    flow.say(REQUEST).await;
    flow.settled().await;
    // The session's request settles at its own turn's end; its worker's turn may still run.
    flow.until("the worker's turn ran to its end", |_| {
        *worker_done.lock().unwrap()
    })
    .await;
    // The worker's `ledger add` went to its own ledger; its session's is as it was.
    let ledger = session_ledger.lock().unwrap().clone().unwrap();
    assert_eq!(std::fs::read_to_string(ledger).unwrap(), LEDGER);
    flow.stop().await;
}

#[tokio::test]
async fn context_reports_the_same_worktree_and_cross_agent_set_as_launch() {
    use brigadier_providers::ProviderKind;
    for provider in ProviderKind::ALL {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let script: Script = {
            let captured = captured.clone();
            Arc::new(move |turn| {
                let captured = captured.clone();
                Box::pin(async move {
                    if turn.is_orchestrator() {
                        let skills = brigadier(&turn, context(Some(ContextPart::Skills))).await;
                        let mcps = brigadier(&turn, context(Some(ContextPart::Mcps))).await;
                        captured.lock().unwrap().extend([skills, mcps]);
                    }
                    Reply::text("Read context.")
                })
            })
        };
        let flow = Flow::start(
            &format!("context-resolver-{provider:?}"),
            Options {
                thread: provider,
                ..Options::default()
            },
            script,
        )
        .await;
        write(
            &flow.repo.join(".git/info/exclude"),
            ".mcp.json\n.claude/\n",
        );
        write(
            &flow.repo.join(".mcp.json"),
            r#"{"mcpServers":{"project-api":{"command":"/not-started"}}}"#,
        );
        write(
            &flow.repo.join(".claude/skills/project-skill/SKILL.md"),
            "---\nname: project-skill\n---\nProject skill.\n",
        );
        let roots = flow.manager.registry().roots().clone();
        write(
            &roots.codex_home.join("config.toml"),
            "[mcp_servers.codex-only]\ncommand='/not-started'\n",
        );
        write(
            &roots.codex_home.join("skills/codex-only/SKILL.md"),
            "---\nname: codex-only\n---\nCodex source skill.\n",
        );
        flow.manager.registry().refresh();
        flow.say("Read the context.").await;
        flow.settled().await;
        let output = captured.lock().unwrap().clone();
        assert_eq!(output.len(), 2);
        let specs = flow.thread_specs();
        let spec = &specs[0].1;
        assert!(!spec.cwd.join(".mcp.json").exists());
        assert!(
            spec.mcp_servers
                .iter()
                .any(|server| server.name == "project-api")
        );
        for name in ["project-api", "codex-only"] {
            assert!(
                output[1]["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| item["name"] == name && item["has"] == true),
                "{provider:?}: {}",
                output[1]
            );
        }
        for name in ["project-skill", "codex-only"] {
            assert!(
                output[0]["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| item["name"] == name && item["has"] == true),
                "{provider:?}: {}",
                output[0]
            );
        }
        let resolved = flow
            .manager
            .resolve_plugins(
                &flow.manager.runtime.launch_account(provider, None),
                Some(&flow.repo),
                &spec.cwd,
                true,
                crate::manager::plugins::resolve::Role::Thread,
            )
            .await
            .unwrap();
        let names = |value: &Value| {
            value["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["name"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(&output[0]),
            resolved
                .skills
                .iter()
                .map(|skill| skill.display.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            names(&output[1]),
            resolved
                .servers
                .iter()
                .map(|server| server.name.clone())
                .collect::<Vec<_>>()
        );
        flow.stop().await;
    }
}

#[tokio::test]
async fn context_uses_the_callers_active_account_home_for_per_agent_skill_versions() {
    use brigadier_providers::ProviderKind;
    for provider in ProviderKind::ALL {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let script: Script = {
            let captured = captured.clone();
            Arc::new(move |turn| {
                let captured = captured.clone();
                Box::pin(async move {
                    if turn.is_orchestrator() {
                        let output = brigadier(
                            &turn,
                            CliRequest::Context {
                                part: Some(ContextPart::Skills),
                                name: Some("same".into()),
                                grep: None,
                                last: None,
                                item: None,
                            },
                        )
                        .await;
                        captured
                            .lock()
                            .unwrap()
                            .push((turn.account.clone(), output));
                    }
                    Reply::text("Read skill.")
                })
            })
        };
        let flow = Flow::start(
            &format!("context-account-{provider:?}"),
            Options {
                thread: provider,
                ..Options::default()
            },
            script,
        )
        .await;
        flow.add_accounts(&[(provider, "context-extra")], false)
            .await;
        let roots = flow.manager.registry().roots().clone();
        write(
            &roots.claude_dir.join("skills/same/SKILL.md"),
            "---\nname: same\n---\nPRIMARY_CLAUDE\n",
        );
        write(
            &roots.codex_home.join("skills/same/SKILL.md"),
            "---\nname: same\n---\nPRIMARY_CODEX\n",
        );
        let extra = flow.manager.runtime.account_home("context-extra");
        write(
            &extra.join("skills/same/SKILL.md"),
            "---\nname: same\n---\nEXTRA_ACCOUNT_VERSION\n",
        );
        flow.manager.registry().refresh();
        let mut setup = flow
            .core
            .conversation(&flow.conversation)
            .unwrap()
            .setup
            .unwrap();
        let crate::model::Setup::Session { orchestrator, .. } = &mut setup else {
            panic!("session");
        };
        orchestrator.account = Some("context-extra".into());
        flow.manager
            .set_setup(flow.conversation.clone(), setup)
            .await
            .unwrap();
        flow.say("Read the skill.").await;
        flow.settled().await;
        let output = captured.lock().unwrap().clone();
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].0.as_deref(), Some("context-extra"));
        assert!(
            output[0].1["text"]
                .as_str()
                .unwrap()
                .contains("EXTRA_ACCOUNT_VERSION"),
            "{provider:?}: {}",
            output[0].1
        );
        flow.stop().await;
    }
}

#[tokio::test]
async fn library_sign_in_status_uses_the_managers_data_dir_without_an_env_override() {
    let script: Script = Arc::new(|_| Box::pin(async { Reply::text("Ready.") }));
    let flow = Flow::start("library-sign-in-data-dir", Options::default(), script).await;
    assert!(
        !flow
            .manager
            .registry()
            .roots()
            .vars
            .contains_key("BRIGADIER_DATA_DIR")
    );
    write(
        &flow
            .manager
            .data_dir
            .join("library/servers/review-oauth.json"),
        &json!({"mcpServers":{"review-oauth":{"type":"http","url":"https://review.invalid/mcp"}}})
            .to_string(),
    );
    let catalog = flow.manager.plugin_catalog().unwrap();
    let row = catalog
        .rows
        .iter()
        .find(|row| row.name == "review-oauth")
        .unwrap();
    assert_eq!(
        flow.manager
            .get_plugin_sign_in_status(&row.id)
            .await
            .unwrap(),
        crate::manager::plugins::CatalogSignIn::SignedOut
    );
    flow.manager.cancel_plugin_sign_in(&row.id).await.unwrap();
    flow.stop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn saving_a_library_skill_cannot_follow_a_skills_root_into_a_project() {
    let script: Script = Arc::new(|_| Box::pin(async { Reply::text("Ready.") }));
    let flow = Flow::start("library-skill-root-link", Options::default(), script).await;
    let original = "---\nname: review-linked\n---\nUSER_CONTENT\n";
    let source = flow.repo.join(".claude/skills/review-linked/SKILL.md");
    write(&source, original);
    let library = flow.manager.data_dir.join("library");
    std::fs::create_dir_all(&library).unwrap();
    std::os::unix::fs::symlink(flow.repo.join(".claude/skills"), library.join("skills")).unwrap();
    let catalog = flow.manager.plugin_catalog().unwrap();
    let row = catalog
        .rows
        .iter()
        .find(|row| row.name == "review-linked")
        .unwrap();
    let details = flow.manager.catalog_item_details(row.id.clone()).unwrap();
    let file = details
        .skills
        .iter()
        .find(|file| file.read_only.is_none())
        .unwrap();
    assert!(
        flow.manager
            .save_catalog_skill(
                row.id.clone(),
                file.path.clone(),
                "CHANGED".into(),
                original.into()
            )
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(source).unwrap(), original);
    flow.stop().await;
}
