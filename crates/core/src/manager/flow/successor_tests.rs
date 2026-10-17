//! Honest successor seams (BUILD-PLAN D2.4, research A2): a worker that another model takes
//! over after an error, or that a fresh session continues after a restart found its CLI's files
//! gone, gets what a size hand-off gets: the streamed transcript with every whole output, the
//! decisions, checks.md, and without a note the state of work built from the transcript and
//! marked unchecked. A restart whose CLI files are missing no longer stops the task.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use brigadier_brain::{NewNode, NodeKind, Origin, Provenance};
use brigadier_providers::{ItemStatus, KeptOutput, ProviderEvent};
use serde_json::json;
use tokio::sync::Notify;

use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::fault::{FaultKey, InjectedError};
use crate::work::TaskState;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// What the first session read: only its transcript keeps it.
const PLANTED: &str = "The vendor's ticket is VT-55107.";
const DECIDED: &str = "Tickets are filed under the vendor's own number.";
/// The last line of a long output, past the 8 KiB its event keeps.
const LAST_LINE: &str = "CHECK-END-7731";

/// A long output ending with [`LAST_LINE`], and its event's text: cut, with the whole waiting.
fn long_output() -> (String, String) {
    let mut whole = "file line filler text filler text\n".repeat(600);
    whole.push_str(LAST_LINE);
    whole.push('\n');
    let cut = format!(
        "{}… [{} more bytes]",
        &whole[..8 * 1024],
        whole.len() - 8 * 1024
    );
    (whole, cut)
}

/// The successor's first message and its hand-off folder's files.
#[derive(Default, Clone)]
struct Successor {
    input: String,
    files: Vec<(String, String)>,
}

impl Successor {
    fn file(&self, name: &str) -> Option<&str> {
        self.files
            .iter()
            .find(|(file, _)| file == name)
            .map(|(_, text)| text.as_str())
    }
}

/// The hand-off folder a successor's first message names.
fn handoff_dir(input: &str) -> PathBuf {
    let dir = input
        .split("Its hand-off is in ")
        .nth(1)
        .and_then(|rest| rest.split(": ").next())
        .expect("the hand-off folder");
    PathBuf::from(dir)
}

fn read_files(dir: &Path) -> Vec<(String, String)> {
    let mut files = Vec::new();
    for name in [
        "spec.md",
        "progress.md",
        "transcript.md",
        "state.md",
        "note.md",
        "checks.md",
        "decisions.md",
    ] {
        if let Ok(text) = std::fs::read_to_string(dir.join(name)) {
            files.push((name.to_owned(), text));
        }
    }
    files
}

/// The thread delegates one scout. Its first session reads the planted detail, runs a check,
/// says it is ready and waits (for a fault, or for the daemon to go); a successor's session
/// (`marker` in its first message) records what it was given and reports.
fn scout(
    marker: &'static str,
    pin: Option<&'static str>,
    ready: &Arc<Notify>,
    armed: &Arc<Notify>,
    seen: &Arc<Mutex<Option<Successor>>>,
    native: &Arc<Mutex<Option<String>>>,
) -> Script {
    let (ready, armed, seen, native) = (ready.clone(), armed.clone(), seen.clone(), native.clone());
    script(move |turn| {
        let (ready, armed, seen, native) =
            (ready.clone(), armed.clone(), seen.clone(), native.clone());
        async move {
            if turn.is_review() {
                return Reply::text("No findings.");
            }
            if turn.is_orchestrator() {
                if turn.input.contains("Find the ticket.") {
                    let mut args = json!({"effort": "medium", "title": "Find the ticket",
                                          "kind": "scout", "spec": "Find the vendor's ticket."});
                    if let Some(pin) = pin {
                        args["provider"] = pin.into();
                    }
                    let reply = turn.call("delegate_task", args).await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                return Reply::text("Noted.");
            }
            if turn.input.contains(marker) {
                let dir = handoff_dir(&turn.input);
                *seen.lock().unwrap() = Some(Successor {
                    input: turn.input.clone(),
                    files: read_files(&dir),
                });
                let reply = turn
                    .call("submit_report", json!({"summary": "Found it."}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Reported.");
            }
            *native.lock().unwrap() = Some(turn.native_id.clone());
            turn.emit(ProviderEvent::Command {
                item_id: "cmd-1".into(),
                command: "cat ticket.txt".into(),
                cwd: None,
                status: ItemStatus::Completed,
                exit_code: Some(0),
                output: Some(PLANTED.into()),
                duration_ms: None,
                kept: None,
            })
            .await;
            let (whole, cut) = long_output();
            turn.emit(ProviderEvent::Command {
                item_id: "cmd-2".into(),
                command: "cat bulk.txt".into(),
                cwd: None,
                status: ItemStatus::Completed,
                exit_code: Some(0),
                output: Some(cut),
                duration_ms: None,
                kept: Some(Box::new(KeptOutput {
                    bytes: whole.len() as u64,
                    whole: Some(whole.as_str().into()),
                    ..KeptOutput::default()
                })),
            })
            .await;
            let check = turn
                .call("run_check", json!({"command": "echo checked"}))
                .await;
            assert!(!check.is_error, "{}", check.text);
            ready.notify_one();
            armed.notified().await;
            turn.emit(ProviderEvent::Notice {
                level: brigadier_providers::NoticeLevel::Info,
                message: "reading".into(),
            })
            .await;
            Reply::text("Still reading.")
        }
    })
}

/// Records one of the user's decisions for the session, in its project's Brain.
async fn decide(flow: &Flow) {
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id
        .clone()
        .expect("the session's project");
    let brain = flow
        .manager
        .project_brain(&project)
        .await
        .unwrap()
        .brain
        .clone();
    let conversation = flow.conversation.0.clone();
    tokio::task::spawn_blocking(move || {
        brain
            .record(NewNode {
                kind: NodeKind::Decision,
                key: None,
                title: "Ticket numbers".into(),
                body: DECIDED.into(),
                provenance: Provenance {
                    origin: Origin::User,
                    session_id: Some(conversation),
                    task_id: None,
                    job_id: None,
                    worker: None,
                    commit: None,
                    recorded_at_ms: crate::now_ms(),
                    claim: None,
                },
                files: Vec::new(),
                expires_at_ms: None,
            })
            .unwrap()
    })
    .await
    .unwrap();
}

/// What every successor seam must carry: the whole transcript with the planted output, the
/// state of work marked unchecked (no note), checks.md and decisions.md, all named in its
/// first message.
fn assert_whole(successor: &Successor) {
    let input = &successor.input;
    for name in [
        "transcript.md (",
        "state.md (",
        "checks.md (",
        "decisions.md (",
    ] {
        assert!(input.contains(name), "{name} in: {input}");
    }
    assert!(input.contains("transcript.md is complete."), "{input}");
    assert!(input.contains("read what you need in full"), "{input}");
    assert!(!input.contains("note.md ("), "no note was written: {input}");
    let transcript = successor.file("transcript.md").expect("transcript.md");
    assert!(transcript.contains(PLANTED), "{transcript}");
    // The progress log ends a cut output with its real last line, read from the whole copy.
    let progress = successor.file("progress.md").expect("progress.md");
    assert!(progress.contains(LAST_LINE), "{progress}");
    let state = successor.file("state.md").expect("state.md");
    assert!(state.contains("unchecked"), "{state}");
    assert!(state.contains("The last step it took:"), "{state}");
    let checks = successor.file("checks.md").expect("checks.md");
    assert!(checks.contains("echo checked"), "{checks}");
    let decisions = successor.file("decisions.md").expect("decisions.md");
    assert!(decisions.contains(DECIDED), "{decisions}");
    assert!(successor.file("note.md").is_none());
}

/// Another model takes the task over after its first one ran out of credits with no other
/// account to go on: its successor gets the whole hand-off, not a thin brief.
#[tokio::test]
async fn a_task_another_model_takes_over_gets_the_whole_hand_off() {
    let (ready, armed): (Arc<Notify>, Arc<Notify>) = Default::default();
    let seen: Arc<Mutex<Option<Successor>>> = Arc::default();
    let native: Arc<Mutex<Option<String>>> = Arc::default();
    let flow = Flow::start(
        "successor-reroute",
        Options::default(),
        scout(
            "You are taking over task-1",
            None,
            &ready,
            &armed,
            &seen,
            &native,
        ),
    )
    .await;
    decide(&flow).await;
    flow.say("Find the ticket.").await;
    ready.notified().await;
    let board = flow.board().await;
    let task = board.tasks.values().next().unwrap();
    let provider = task.route.choice.provider;
    flow.manager
        .debug_inject_error(
            FaultKey::Task(task.id.clone()),
            provider,
            InjectedError::Billing,
            0,
        )
        .await
        .unwrap();
    armed.notify_one();
    let board = flow
        .until("the scout reported", |board| {
            board.tasks.values().any(|task| task.report.is_some())
        })
        .await;
    let successor = seen.lock().unwrap().clone().expect("a successor took over");
    assert_whole(&successor);
    let task = board.tasks.values().next().unwrap();
    assert_eq!(task.attempts.len(), 2);
    // Another model: the one that ran out of credits was ruled out.
    assert_ne!(
        task.attempts[0].route.choice, task.attempts[1].route.choice,
        "another model took over"
    );
    flow.stop().await;
}

/// After a restart the worker's session can't be resumed (its CLI's files are gone): a fresh
/// session of the same model continues from the whole hand-off, and the task goes on instead
/// of stopping.
#[tokio::test]
async fn a_restart_without_the_cli_files_hands_the_worker_over_instead_of_stopping() {
    let (ready, armed): (Arc<Notify>, Arc<Notify>) = Default::default();
    let seen: Arc<Mutex<Option<Successor>>> = Arc::default();
    let native: Arc<Mutex<Option<String>>> = Arc::default();
    let mut flow = Flow::start(
        "successor-restart",
        Options::default(),
        scout(
            "You are continuing task-1 in a fresh session",
            Some("claude"),
            &ready,
            &armed,
            &seen,
            &native,
        ),
    )
    .await;
    decide(&flow).await;
    flow.say("Find the ticket.").await;
    ready.notified().await;
    let gone = native.lock().unwrap().clone().expect("the first session");
    super::REFUSED_RESUMES.lock().unwrap().push(gone.clone());
    flow.restart().await;
    let board = flow
        .until("the scout reported", |board| {
            board.tasks.values().any(|task| task.report.is_some())
        })
        .await;
    super::REFUSED_RESUMES
        .lock()
        .unwrap()
        .retain(|id| *id != gone);
    let successor = seen
        .lock()
        .unwrap()
        .clone()
        .expect("a fresh session took over");
    assert!(
        successor
            .input
            .contains("Brigadier restarted while your earlier session was working"),
        "{}",
        successor.input
    );
    assert_whole(&successor);
    let task = board.tasks.values().next().unwrap();
    assert_ne!(task.state, TaskState::Stopped);
    assert!(task.error.is_none(), "{:?}", task.error);
    flow.stop().await;
}
