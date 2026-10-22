//! Durable raw output (BUILD-PLAN D2.1, D2.2): a worker's long command output and the file a
//! Claude preview names are kept whole when the events are seen, and the fresh session a
//! hand-off starts finds them in its hand-off folder, whatever happened to the CLI's own
//! session files; what couldn't be kept is said, never passed off as whole.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use brigadier_providers::{ItemStatus, KeptFile, KeptOutput, ProviderEvent};
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::model::DomainEvent;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

const MARKER: &str = "PLANTED-MARKER-7f3a9c";
const SAVED_MARKER: &str = "SAVED-MARKER-41bd";

/// 200 KiB of output with `marker` 50 KiB in.
fn big(marker: &str) -> String {
    let line = "line filler text filler text filler text\n";
    let mut text = line.repeat(50 * 1024 / line.len());
    text.push_str(marker);
    text.push('\n');
    while text.len() < 200 * 1024 {
        text.push_str(line);
    }
    text
}

/// What the adapter's event sender hands on for a long output: cut, with the whole waiting.
fn cut(whole: &str) -> String {
    format!(
        "{}… [{} more bytes]",
        &whole[..8 * 1024],
        whole.len() - 8 * 1024
    )
}

/// The first session's two long results: a command's, and an MCP tool's that Claude saved
/// to its `tool-results/` and previewed.
async fn long_results(turn: &Turn) {
    let whole = big(MARKER);
    turn.emit(ProviderEvent::Command {
        item_id: "cmd-1".into(),
        command: "python3 gen.py".into(),
        cwd: None,
        status: ItemStatus::Completed,
        exit_code: Some(0),
        output: Some(cut(&whole)),
        duration_ms: None,
        kept: Some(Box::new(KeptOutput {
            bytes: whole.len() as u64,
            whole: Some(whole.as_str().into()),
            ..KeptOutput::default()
        })),
    })
    .await;
    let saved = big(SAVED_MARKER);
    turn.emit(ProviderEvent::ToolCall {
        item_id: "mcp-1".into(),
        name: "mcp__probe__big_dump".into(),
        input: Some("{}".into()),
        status: ItemStatus::Completed,
        output: Some(
            "Error: result exceeds maximum allowed tokens. Output has been saved to \
             /cli/session/tool-results/mcp-probe-big_dump-1.txt."
                .into(),
        ),
        kept: Some(Box::new(KeptOutput {
            file: Some(KeptFile {
                path: "/cli/session/tool-results/mcp-probe-big_dump-1.txt".into(),
                bytes: saved.len() as u64,
                text: Some(saved.as_str().into()),
                ..KeptFile::default()
            }),
            ..KeptOutput::default()
        })),
    })
    .await;
}

/// A fresh session's first message, and the files of the hand-off folder it names as they
/// were when it started: the folder is in the task's scratch, which goes once the task reports.
struct Fresh {
    first: String,
    files: BTreeMap<String, String>,
}

/// Every regular file under `dir`, by its path from `dir` (a link would be no copy).
fn snapshot(dir: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(folder) = todo.pop() {
        for entry in std::fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            let kind = std::fs::symlink_metadata(&path).unwrap().file_type();
            if kind.is_dir() {
                todo.push(path);
            } else if kind.is_file() {
                let name = path.strip_prefix(dir).unwrap().to_string_lossy();
                let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned();
                files.insert(name.replace('\\', "/"), text);
            }
        }
    }
    files
}

/// The thread delegates one scout. Its first session prints the long results and grows past
/// the hand-off size; the fresh one's first message and hand-off folder are logged, and it
/// reports.
fn handing_over(firsts: &Arc<Mutex<Vec<Fresh>>>) -> Script {
    handing_over_with(firsts, None)
}

/// [`handing_over`], whose first session, given `saved`, shows instead one result recorded
/// before results were kept: a preview naming the file `saved` its CLI saved the output to.
fn handing_over_with(firsts: &Arc<Mutex<Vec<Fresh>>>, saved: Option<std::path::PathBuf>) -> Script {
    let firsts = firsts.clone();
    script(move |turn| {
        let (firsts, saved) = (firsts.clone(), saved.clone());
        async move {
            if turn.is_orchestrator() {
                if turn.input.contains("Find the marker.") {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Find the marker",
                                   "kind": "scout", "spec": "Find the marker in gen.py's output.",
                                   "provider": "claude"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("");
                }
                return Reply::text("Noted.");
            }
            const NOTE: &str = "Goal: find the marker.\nNext steps: read the outputs.";
            if turn.input.contains("[Brigadier] Your context is now about") {
                return Reply::text(NOTE);
            }
            if turn.input.contains("You are continuing task-") {
                let files = snapshot(&handoff_dir(&turn.input));
                firsts.lock().unwrap().push(Fresh {
                    first: turn.input.clone(),
                    files,
                });
                let reply = turn
                    .call("submit_report", json!({"summary": "Found it."}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Reported.");
            }
            // It starts small, prints the long results and grows past the hand-off size.
            turn.report_context(20_000).await;
            match saved {
                Some(saved) => {
                    turn.emit(ProviderEvent::ToolCall {
                        item_id: "old-1".into(),
                        name: "mcp__probe__big_dump".into(),
                        input: Some("{}".into()),
                        status: ItemStatus::Completed,
                        output: Some(format!(
                            "Error: result exceeds maximum allowed tokens. Output has been saved \
                             to {}.",
                            saved.display()
                        )),
                        kept: None,
                    })
                    .await
                }
                None => long_results(&turn).await,
            }
            Reply {
                text: NOTE.into(),
                context_tokens: Some(310_000),
                ..Reply::default()
            }
        }
    })
}

/// The hand-off folder a fresh session's first message names.
fn handoff_dir(first: &str) -> std::path::PathBuf {
    first
        .split("Its hand-off is in ")
        .nth(1)
        .and_then(|rest| rest.split(": ").next())
        .expect("the hand-off folder")
        .into()
}

/// The kept parts of the worker's recorded long results, and its other kept-copy events.
async fn recorded_kept(flow: &Flow) -> (Vec<KeptOutput>, Vec<DomainEvent>) {
    let board = flow.board().await;
    let task = board.tasks.keys().next().expect("the scout").clone();
    let page = flow
        .manager
        .core
        .store()
        .read_stream_since(crate::model::streams::task(&task), 0, 10_000)
        .await
        .unwrap();
    let mut kept = Vec::new();
    let mut late = Vec::new();
    for stored in &page {
        match serde_json::from_str::<DomainEvent>(stored.payload.get()).unwrap() {
            DomainEvent::WorkerEvent {
                event:
                    ProviderEvent::Command {
                        kept: Some(one), ..
                    }
                    | ProviderEvent::ToolCall {
                        kept: Some(one), ..
                    },
                ..
            } => kept.push(*one),
            event @ DomainEvent::OutputKept { .. } => late.push(event),
            _ => {}
        }
    }
    (kept, late)
}

#[tokio::test]
async fn a_long_output_and_a_saved_file_reach_the_fresh_session_whole() {
    let firsts: Arc<Mutex<Vec<Fresh>>> = Arc::default();
    let flow = Flow::start("kept-whole", Options::default(), handing_over(&firsts)).await;
    flow.say("Find the marker.").await;
    flow.until("the scout reported", |board| {
        board.tasks.values().any(|task| task.report.is_some())
    })
    .await;
    // Recorded cut, naming the blob that holds the whole, and nothing waiting.
    let (kept, _) = recorded_kept(&flow).await;
    assert_eq!(kept.len(), 2, "{kept:?}");
    assert!(kept.iter().all(KeptOutput::is_complete), "{kept:?}");
    let blob = kept[0].blob.clone().expect("the output's blob");
    let whole = flow
        .manager
        .core
        .store()
        .blobs()
        .get(blob.parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8(whole).unwrap().contains(MARKER));

    let Fresh { first, files } = firsts.lock().unwrap().remove(0);
    assert!(first.contains("transcript.md is complete."), "{first}");
    let transcript = &files["transcript.md"];
    assert!(!transcript.contains(MARKER), "the transcript itself is cut");
    // The CLI's session files are not needed: the copies are in the hand-off folder.
    let named: Vec<&str> = transcript
        .split("outputs/")
        .skip(1)
        .filter_map(|rest| rest.split(')').next())
        .filter(|name| name.ends_with(".txt"))
        .collect();
    assert_eq!(named.len(), 2, "{transcript}");
    let read = |name: &str| &files[&format!("outputs/{name}")];
    assert!(read(named[0]).contains(MARKER));
    assert!(read(named[1]).contains(SAVED_MARKER));
    flow.stop().await;
}

#[tokio::test]
async fn an_output_the_disk_refused_is_said_missing_to_the_fresh_session() {
    let firsts: Arc<Mutex<Vec<Fresh>>> = Arc::default();
    let flow = Flow::start("kept-refused", Options::default(), handing_over(&firsts)).await;
    flow.manager.debug_fail_kept_writes(true);
    flow.say("Find the marker.").await;
    flow.until("the scout reported", |board| {
        board.tasks.values().any(|task| task.report.is_some())
    })
    .await;
    let (kept, _) = recorded_kept(&flow).await;
    assert!(kept[0].error.as_deref().unwrap().contains("disk refused"));
    let Fresh { first, files } = firsts.lock().unwrap().remove(0);
    assert!(first.contains("transcript.md is NOT complete"), "{first}");
    assert!(first.contains("couldn't be stored"), "{first}");
    assert!(first.contains("say it is unknown"), "{first}");
    assert!(files["transcript.md"].contains("Missing from this transcript:"));
    flow.stop().await;
}

#[tokio::test]
async fn a_saved_file_not_copied_yet_is_copied_before_the_hand_off_and_outlives_its_folder() {
    let root = std::env::temp_dir().join(format!("brigadier-kept-cli-{}", uuid::Uuid::new_v4()));
    let folder = root.join("projects/-tmp-p/s1");
    std::fs::create_dir_all(folder.join("tool-results")).unwrap();
    let saved = folder.join("tool-results/mcp-probe-big_dump-2.txt");
    std::fs::write(&saved, big(SAVED_MARKER)).unwrap();
    let firsts: Arc<Mutex<Vec<Fresh>>> = Arc::default();
    let flow = Flow::start(
        "kept-late",
        Options::default(),
        handing_over_with(&firsts, Some(saved.clone())),
    )
    .await;
    flow.say("Find the marker.").await;
    flow.until("the scout reported", |board| {
        board.tasks.values().any(|task| task.report.is_some())
    })
    .await;
    let (_, late) = recorded_kept(&flow).await;
    assert!(
        matches!(&late[..], [DomainEvent::OutputKept { item_id, file, .. }]
            if item_id == "old-1" && file.blob.is_some()),
        "{late:?}"
    );
    // The CLI's session folder goes; the hand-off held a copy of its own (a file, not a link).
    std::fs::remove_dir_all(&root).unwrap();
    let Fresh { first, files } = firsts.lock().unwrap().remove(0);
    assert!(first.contains("transcript.md is complete."), "{first}");
    let copied = files
        .iter()
        .filter(|(name, _)| name.starts_with("outputs/"))
        .map(|(_, text)| text)
        .collect::<Vec<_>>();
    assert_eq!(copied.len(), 1);
    assert!(copied[0].contains(SAVED_MARKER));
    flow.stop().await;
}

/// A sandboxed thread can read the files its search results and a stopped turn's briefing
/// name (#122): a kept output's blob and the stopped session's transcript lie outside the one
/// folder its sandbox denies reads of (Brigadier's run folder), and a file tool's read of them
/// is allowed without asking anyone, at either sandboxed level.
#[tokio::test]
async fn a_sandboxed_thread_may_read_the_kept_files_it_is_pointed_to() {
    use brigadier_providers::model::ApprovalKind;
    use brigadier_providers::policy::Route;
    use brigadier_providers::{Access, ApprovalRequest};

    use crate::model::PermissionLevel;

    let flow = Flow::start(
        "kept-readable",
        Options::default(),
        script(|_| async { Reply::text("ok") }),
    )
    .await;
    let blobs = flow.core.store().blobs();
    let blob = blobs.put(b"whole output".to_vec()).await.unwrap();
    let scratch = flow.manager.owned_dir("orch", &flow.conversation.0);
    let named = [
        blobs.file_of(&blob),
        scratch.join("stopped").join("transcript.md"),
    ];
    for permission in [
        PermissionLevel::ApproveForMe,
        PermissionLevel::AskForApproval,
    ] {
        let (access, _) =
            flow.manager
                .thread_access(&flow.conversation, None, &scratch, permission);
        let Access::Scoped { deny_read, .. } = &access else {
            panic!("{permission:?} is sandboxed: {access:?}");
        };
        for path in &named {
            let real = path.parent().unwrap().canonicalize().unwrap_or_default();
            assert!(
                !deny_read
                    .iter()
                    .any(|denied| path.starts_with(denied) || real.starts_with(denied)),
                "{} is denied by {deny_read:?}",
                path.display()
            );
            let mut read = ApprovalRequest {
                id: "1".into(),
                kind: ApprovalKind::Tool,
                tool: "Read".into(),
                command: None,
                cwd: None,
                paths: vec![path.display().to_string()],
                reason: None,
                escalation: false,
                input: Some(json!({"file_path": path}).to_string()),
                grant: None,
            };
            let (route, _) =
                flow.manager
                    .approval_route(&flow.conversation, &mut read, &access, None, false);
            assert_eq!(route, Route::Allow, "{permission:?} {}", path.display());
        }
    }
    flow.stop().await;
}
