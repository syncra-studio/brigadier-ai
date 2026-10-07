//! The thread's reads and searches (THREAD-PLAN.md Q8 lever 1): what its own tool calls read
//! fills `thread_reads`, for both vendors, recorded once per turn and kept across a restart;
//! a worker's reads never count.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use brigadier_providers::policy::real_path;
use brigadier_providers::{FileRead, LineRange, ProviderEvent, ProviderKind, SearchKind};
use serde_json::{Value, json};

use super::{Flow, Options, Reply, Script, Turn};
use crate::model::DomainEvent;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// What the thread's CLI prints when it reads lines 1–2 of the workspace's README, searches it
/// for "Flow" and reads a note in its own scratch folder, in `provider`'s own words.
fn cli_lines(provider: ProviderKind, scratch: &Path, workspace: &Path) -> Vec<Value> {
    let readme = workspace.join("README.md").display().to_string();
    let note = scratch.join("note.txt").display().to_string();
    match provider {
        ProviderKind::Claude => {
            let call = |id: &str, name: &str, input: Value| {
                json!({"type": "assistant", "message": {"id": format!("m-{id}"), "content": [
                    {"type": "tool_use", "id": id, "name": name, "input": input}]}})
            };
            let result = |id: &str, structured: Value| {
                json!({"type": "user", "message": {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": id, "content": "…"}]},
                    "tool_use_result": structured})
            };
            vec![
                json!({"type": "system", "subtype": "init", "session_id": "s",
                       "cwd": scratch.display().to_string()}),
                call(
                    "r1",
                    "Read",
                    json!({"file_path": readme, "offset": 1, "limit": 2}),
                ),
                result(
                    "r1",
                    json!({"type": "text", "file": {"filePath": readme, "content": "…",
                           "numLines": 2, "startLine": 1, "totalLines": 40}}),
                ),
                call(
                    "g1",
                    "Grep",
                    json!({"pattern": "Flow", "path": workspace.display().to_string()}),
                ),
                result(
                    "g1",
                    json!({"mode": "files_with_matches", "filenames": [readme],
                           "numFiles": 1}),
                ),
                call("r2", "Read", json!({"file_path": note})),
                result(
                    "r2",
                    json!({"type": "text", "file": {"filePath": note, "content": "note\n",
                           "numLines": 2, "startLine": 1, "totalLines": 2}}),
                ),
            ]
        }
        ProviderKind::Codex => {
            let command = |id: &str, command: &str, actions: Value, output: &str| {
                json!({"method": "item/completed", "params": {"threadId": "t", "turnId": "u",
                    "completedAtMs": 1, "item": {"type": "commandExecution", "id": id,
                    "command": format!("/bin/zsh -lc '{command}'"),
                    "cwd": workspace.display().to_string(), "commandActions": actions,
                    "status": "completed", "exitCode": 0, "aggregatedOutput": output,
                    "durationMs": 1, "source": "unifiedExecStartup"}}})
            };
            vec![
                command(
                    "c1",
                    "sed -n 1,2p README.md",
                    json!([{"type": "read", "command": "sed -n 1,2p README.md",
                            "name": "README.md", "path": readme}]),
                    "# Flow\n\n",
                ),
                command(
                    "c2",
                    "rg -n Flow",
                    json!([{"type": "search", "command": "rg -n Flow", "query": "Flow",
                            "path": null}]),
                    "README.md:1:# Flow\n",
                ),
                command(
                    "c3",
                    &format!("cat {note}"),
                    json!([{"type": "read", "command": format!("cat {note}"),
                            "name": "note.txt", "path": note}]),
                    "note\n",
                ),
            ]
        }
    }
}

/// `lines` through `provider`'s own parser.
fn parsed(provider: ProviderKind, lines: Vec<Value>) -> Vec<ProviderEvent> {
    use brigadier_providers::{claude, codex};
    let mut claude = claude::parse::Parser::live();
    let mut codex = codex::parse::Parser::live();
    lines
        .into_iter()
        .flat_map(|line| match provider {
            ProviderKind::Claude => claude
                .feed(&line.to_string())
                .into_iter()
                .filter_map(|output| match output {
                    claude::parse::Output::Event(event) => Some(event),
                    claude::parse::Output::Control(_) => None,
                })
                .collect::<Vec<_>>(),
            ProviderKind::Codex => codex
                .feed(&line.to_string())
                .into_iter()
                .filter_map(|output| match output {
                    codex::parse::Output::Event(event) => Some(event),
                    codex::parse::Output::Control(_) => None,
                })
                .collect(),
        })
        .collect()
}

#[tokio::test]
async fn the_thread_s_reads_and_searches_fill_thread_reads_and_a_worker_s_do_not() {
    for vendor in [ProviderKind::Claude, ProviderKind::Codex] {
        let worker_file: Arc<std::sync::Mutex<Option<PathBuf>>> = Arc::default();
        let worker_input: Arc<std::sync::Mutex<Option<String>>> = Arc::default();
        let seen = worker_file.clone();
        let first_input = worker_input.clone();
        let mut flow = Flow::start(
            &format!("thread-reads-{vendor}"),
            Options {
                thread: vendor,
                ..Options::default()
            },
            script(move |turn| {
                let seen = seen.clone();
                let first_input = first_input.clone();
                async move {
                    if turn.is_orchestrator() {
                        if turn.input.contains("Look around") {
                            turn.write("note.txt", "note\n");
                            let events = parsed(
                                turn.provider,
                                cli_lines(turn.provider, &turn.cwd, &turn.add_dirs[0]),
                            );
                            assert_eq!(
                                events
                                    .iter()
                                    .filter(|event| matches!(event, ProviderEvent::Looked { .. }))
                                    .count(),
                                3,
                                "{events:#?}"
                            );
                            for event in events {
                                turn.emit(event).await;
                            }
                            let reply = turn
                                .call(
                                    "delegate_task",
                                    json!({"title": "Add a greeting", "kind": "implement",
                                           "spec": "Create hello.txt."}),
                                )
                                .await;
                            assert!(!reply.is_error, "{}", reply.text);
                            return Reply::text("[quiet]");
                        }
                        return Reply::text("Added the greeting.");
                    }
                    first_input
                        .lock()
                        .unwrap()
                        .get_or_insert_with(|| turn.input.clone());
                    // The worker reads in its own worktree: none of it is the thread's.
                    turn.write("worker-only.txt", "mine\n");
                    let path = turn.cwd.join("worker-only.txt");
                    *seen.lock().unwrap() = Some(path.clone());
                    turn.emit(ProviderEvent::Looked {
                        item_id: "w1".into(),
                        cwd: Some(turn.cwd.display().to_string()),
                        reads: vec![FileRead {
                            path: path.display().to_string(),
                            lines: None,
                        }],
                        searches: Vec::new(),
                    })
                    .await;
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
        flow.say("Look around, then add a greeting.").await;
        flow.settled().await;

        let spec = flow.thread_specs()[0].1.clone();
        let scratch = real_path(&spec.cwd).unwrap();
        let workspace = real_path(&spec.add_dirs[0]).unwrap();
        let readme = workspace.join("README.md").display().to_string();
        let check = |reads: &crate::manager::reads::ThreadReads| {
            let files: Vec<(&str, bool, &[LineRange], bool)> = reads
                .files
                .iter()
                .map(|file| {
                    (
                        file.path.as_str(),
                        file.whole,
                        file.lines.as_slice(),
                        file.outside,
                    )
                })
                .collect();
            let note = scratch.join("note.txt").display().to_string();
            assert_eq!(
                files,
                [
                    // Most recently read first; the scratch folder is not the workspace.
                    (note.as_str(), true, &[][..], true),
                    (
                        readme.as_str(),
                        false,
                        &[LineRange {
                            start: 1,
                            end: Some(2)
                        }][..],
                        false
                    ),
                ],
                "{vendor}"
            );
            assert_eq!(reads.searches.len(), 1, "{vendor}: {:#?}", reads.searches);
            let search = &reads.searches[0];
            assert_eq!(search.kind, SearchKind::Content);
            assert_eq!(search.pattern.as_deref(), Some("Flow"));
            assert_eq!(search.scope, workspace.display().to_string());
            assert_eq!(search.hits, std::slice::from_ref(&readme));
            assert!(!search.outside);
            assert_eq!((reads.dropped_files, reads.dropped_searches), (0, 0));
        };
        let reads = flow.manager.thread_reads(&flow.conversation).await.unwrap();
        check(&reads);
        let worker_file = worker_file.lock().unwrap().clone().expect("the worker ran");
        assert!(
            !reads
                .files
                .iter()
                .any(|file| file.path.ends_with("worker-only.txt")),
            "{}",
            worker_file.display()
        );
        // The thread read and delegated in one turn: the reads it made before the call, still
        // held until the turn ends, are in the worker's pack.
        let input = worker_input
            .lock()
            .unwrap()
            .clone()
            .expect("the worker ran");
        assert!(
            input.contains("## Files the orchestrator read")
                && input.contains("### README.md (lines 1–1 of 1)"),
            "{vendor}: {input}"
        );
        // All of it is in the message: no file to read again.
        assert!(!input.contains("context.md"), "{vendor}: {input}");
        // The turn's calls are recorded once, when it ends.
        let recorded = flow
            .events()
            .await
            .into_iter()
            .filter(|event| matches!(event, DomainEvent::ThreadLooked { .. }))
            .count();
        assert_eq!(recorded, 1, "{vendor}");

        flow.restart().await;
        let again = flow.manager.thread_reads(&flow.conversation).await.unwrap();
        assert_eq!(again, reads, "{vendor}: kept across a restart");
        flow.stop().await;
    }
}
