//! "Open in terminal" (THREAD-PLAN.md Q5, [`crate::manager::takeover`]): a worker's own session
//! continues in a terminal, one writer at a time, and is handed back for its report.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use brigadier_providers::model::Origin;
use brigadier_providers::{ProviderKind, SessionSpec, TerminalCommand};
use serde_json::json;
use tokio::sync::oneshot;

use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::{HostedTerminal, TerminalHost};
use crate::tools::{ToolCall, ToolHost, WorkerCall};
use crate::work::{TaskId, TaskState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// A terminal host that runs nothing: it records what it was asked to start, and a test ends
/// a terminal ([`FakeHost::exit`]). Ending one on purpose ends it too.
#[derive(Default)]
struct FakeHost {
    started: Mutex<Vec<(String, TerminalCommand)>>,
    exits: Mutex<HashMap<String, oneshot::Sender<()>>>,
    terminated: Mutex<Vec<String>>,
    next: AtomicU32,
    /// A real process to stand for the next terminal's (its pid is recorded).
    process: Mutex<Option<u32>>,
}

impl FakeHost {
    /// The terminal's process ends (the user typed `/exit`).
    fn exit(&self, id: &str) {
        if let Some(exit) = self.exits.lock().unwrap().remove(id) {
            let _ = exit.send(());
        }
    }

    fn started(&self) -> Vec<(String, TerminalCommand)> {
        self.started.lock().unwrap().clone()
    }
}

impl TerminalHost for FakeHost {
    fn start(
        &self,
        _conversation: &str,
        _key: &str,
        command: TerminalCommand,
        _cols: u16,
        _rows: u16,
    ) -> crate::Result<HostedTerminal> {
        let id = format!("t{}", self.next.fetch_add(1, Ordering::SeqCst) + 1);
        let (tx, exited) = oneshot::channel();
        self.exits.lock().unwrap().insert(id.clone(), tx);
        self.started.lock().unwrap().push((id.clone(), command));
        let pid = self.process.lock().unwrap().take();
        Ok(HostedTerminal {
            id,
            pid,
            started_at_ms: None,
            exited,
        })
    }

    fn terminate(&self, id: &str) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        self.terminated.lock().unwrap().push(id.to_owned());
        self.exit(id);
        Box::pin(async {})
    }
}

/// What the worker of task-1 was told, turn by turn.
type Heard = Arc<Mutex<Vec<String>>>;

/// A session whose thread delegates one implement task to `provider` and leaves its report
/// waiting ("LAND NOW" lands it), and whose worker reports each turn, as many tokens in as
/// `context` says.
async fn start(
    name: &str,
    provider: ProviderKind,
    context: i64,
) -> (Flow, Arc<FakeHost>, Heard, Heard) {
    let worker_heard: Heard = Arc::default();
    let thread_heard: Heard = Arc::default();
    let (worker_log, thread_log) = (worker_heard.clone(), thread_heard.clone());
    // Later turns quote the request too: it delegates once.
    let delegated = Arc::new(AtomicBool::new(false));
    let flow = Flow::start(
        name,
        Options::default(),
        script(move |turn| {
            let (worker_log, thread_log) = (worker_log.clone(), thread_log.clone());
            let delegated = delegated.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("LAND NOW") {
                        let reply = turn.call("land_phase", json!({"task": "task-1"})).await;
                        thread_log.lock().unwrap().push(reply.text.clone());
                        return Reply::text("[quiet]");
                    }
                    if turn.input.contains("Add the notes.") && !delegated.swap(true, Ordering::SeqCst) {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"title": "Add notes", "kind": "implement",
                                       "spec": "Create notes.txt.", "provider": provider.to_string()}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                    }
                    return Reply::text("[quiet]");
                }
                worker_log.lock().unwrap().push(turn.input.clone());
                if worker_log.lock().unwrap().len() == 1 {
                    turn.write("notes.txt", "notes\n");
                    turn.git(&["add", "notes.txt"]);
                    turn.git(&["commit", "-q", "-m", "Add notes"]);
                }
                // A session grows to its size: one that started there isn't handed off.
                turn.report_context(1_000).await;
                turn.report_context(context).await;
                let reply = turn
                    .call("submit_report", json!({"summary": "notes.txt is in."}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    let host = Arc::new(FakeHost::default());
    flow.manager.set_terminal_host(host.clone());
    flow.say("Add the notes.").await;
    flow.until("task-1's report", |board| {
        board
            .tasks
            .values()
            .any(|task| task.number == 1 && task.state == TaskState::Reported)
    })
    .await;
    flow.settled().await;
    (flow, host, worker_heard, thread_heard)
}

async fn task_id(flow: &Flow) -> TaskId {
    Flow::task(&flow.board().await, 1).id.clone()
}

/// The specs task-1's worker CLIs were started with, in order.
fn worker_specs(flow: &Flow) -> Vec<SessionSpec> {
    flow.specs
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, spec)| {
            spec.append_system_prompt
                .as_deref()
                .is_some_and(|prompt| prompt.starts_with("You are a Brigadier worker."))
        })
        .map(|(_, spec)| spec.clone())
        .collect()
}

fn grant_of(command: &TerminalCommand) -> String {
    command
        .env
        .iter()
        .find(|(name, _)| name == "BRIGADIER_MCP_GRANT")
        .map(|(_, value)| value.to_string_lossy().into_owned())
        .expect("the terminal has a grant")
}

/// An authenticated Brigadier tool call with `grant`, as the terminal's MCP server makes it.
async fn call_with(flow: &Flow, grant: &str) -> bool {
    let reply = ToolHost::call(
        &*flow.manager,
        grant,
        ToolCall::Worker(WorkerCall::ProjectMap),
    )
    .await;
    !reply.is_error
}

/// A check run from the terminal with `grant`: its reply.
async fn check_with(flow: &Flow, grant: &str) -> crate::tools::ToolReply {
    ToolHost::call(
        &*flow.manager,
        grant,
        ToolCall::Worker(WorkerCall::RunCheck(crate::tools::RunCheck {
            command: Some("cat notes.txt".into()),
            workdir: None,
            timeout_secs: None,
            rerun: true,
        })),
    )
    .await
}
/// For each vendor: the worker's own session opens in a terminal (its headless CLI closed
/// first, a fresh grant that works for Brigadier's tools), a second open finds it, messages
/// wait; when it ends, the same session resumes headless, never handed off for its size first,
/// and reports what was done there with the messages that waited.
#[tokio::test]
async fn a_worker_opens_in_a_terminal_and_reports_what_was_done_there() {
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        // Past the hand-off size: a revival would start a fresh session.
        let (flow, host, heard, _) =
            start(&format!("takeover-{provider}"), provider, 950_000).await;
        let id = task_id(&flow).await;
        let before = worker_specs(&flow);
        let native = flow.board().await.tasks[&id]
            .native_session
            .clone()
            .unwrap();

        let opened = flow
            .manager
            .open_worker_terminal(id.clone(), 120, 40)
            .await
            .unwrap();
        assert!(opened.fresh);
        assert_eq!(opened.provider, provider);
        let task = &flow.board().await.tasks[&id];
        assert_eq!(task.state, TaskState::TakenOver);
        assert_eq!(task.takeover.as_ref().unwrap().native_id, native);
        let live = flow.manager.existing_task_live(&id).unwrap();
        assert!(
            live.cli().await.is_none(),
            "no headless CLI beside the terminal"
        );
        let started = host.started();
        assert_eq!(started.len(), 1);
        let (terminal, command) = &started[0];
        assert_eq!(command.args, vec!["--resume".to_owned(), native.clone()]);
        let worktree = task.workspace.as_ref().unwrap().worktree.clone().unwrap();
        assert_eq!(command.cwd, std::path::PathBuf::from(&worktree));
        let grant = grant_of(command);
        assert!(call_with(&flow, &grant).await, "the terminal's grant works");
        let check = check_with(&flow, &grant).await;
        assert!(
            !check.is_error && check.text.contains("notes"),
            "its checks run: {}",
            check.text
        );

        // A second open reattaches; a message waits.
        let again = flow
            .manager
            .open_worker_terminal(id.clone(), 80, 24)
            .await
            .unwrap();
        assert_eq!(again.terminal_id, *terminal);
        assert!(!again.fresh);
        assert_eq!(host.started().len(), 1);
        let task = flow.board().await.tasks[&id].clone();
        let (held, sent) = flow
            .manager
            .message_worker(
                &task.conversation_id,
                &task,
                "Also add a title.".into(),
                "the orchestrator",
            )
            .await
            .unwrap();
        assert!(!sent);
        assert!(held.contains("open in the user's terminal"), "{held}");
        assert_eq!(
            worker_specs(&flow).len(),
            before.len(),
            "nothing ran it headless"
        );

        // The terminal ends: the same session reports.
        host.exit(terminal);
        let board = flow
            .until("the report after the terminal", |board| {
                board.tasks[&id].state == TaskState::Reported && heard.lock().unwrap().len() == 2
            })
            .await;
        assert!(board.tasks[&id].takeover.is_none());
        let after = worker_specs(&flow);
        assert_eq!(after.len(), before.len() + 1);
        assert_eq!(
            after.last().unwrap().origin,
            Origin::Resume {
                native_id: native.clone()
            },
            "the same session, not a size hand-off"
        );
        let last = heard.lock().unwrap().last().cloned().unwrap();
        assert!(last.contains("[terminal]"), "{last}");
        assert!(last.contains("Also add a title."), "{last}");
        assert!(
            !call_with(&flow, &grant).await,
            "the terminal's grant ended with it"
        );
        flow.stop().await;
    }
}

/// Two opens at once start one terminal; an open racing the user's Resume, or a landing,
/// leaves one writer: the terminal, and the landing is refused while it is open.
#[tokio::test]
async fn racing_opens_resumes_and_landings_leave_one_writer() {
    let (flow, host, _, thread) = start("takeover-races", ProviderKind::Claude, 1_000).await;
    let id = task_id(&flow).await;
    let (a, b) = tokio::join!(
        flow.manager.open_worker_terminal(id.clone(), 80, 24),
        flow.manager.open_worker_terminal(id.clone(), 80, 24),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.terminal_id, b.terminal_id);
    assert_eq!(host.started().len(), 1);
    assert!(a.fresh != b.fresh);

    let specs = worker_specs(&flow).len();
    let _ = flow.manager.resume_task(id.clone()).await;
    let live = flow.manager.existing_task_live(&id).unwrap();
    assert!(live.cli().await.is_none());
    assert_eq!(worker_specs(&flow).len(), specs, "no headless CLI started");

    // A headless start already past those checks (a retry, a quota timer) doesn't register.
    let task = flow.board().await.tasks[&id].clone();
    let native = task.native_session.clone().unwrap();
    let late = flow
        .manager
        .launch_worker(
            &live,
            &task,
            None,
            Origin::Resume { native_id: native },
            brigadier_providers::TurnInput::text("Carry on."),
        )
        .await;
    assert!(late.is_err(), "a late start is refused");
    assert!(
        live.cli().await.is_none(),
        "the terminal stays the one writer"
    );
    assert_eq!(flow.board().await.tasks[&id].state, TaskState::TakenOver);

    flow.say("LAND NOW").await;
    flow.until("the landing's answer", |_| {
        !thread.lock().unwrap().is_empty()
    })
    .await;
    flow.settled().await;
    let answer = thread.lock().unwrap()[0].clone();
    assert!(!answer.is_empty());
    let board = flow.board().await;
    assert_eq!(board.tasks[&id].state, TaskState::TakenOver, "{answer}");
    assert!(
        crate::manager::flow::git(&flow.repo, &["log", "--format=%s", "main"])
            .lines()
            .all(|subject| subject != "Add notes"),
        "nothing landed: {answer}"
    );

    // The terminal ends: the worker is handed back and reports.
    host.exit(&a.terminal_id);
    flow.until("the hand-back", |board| {
        board.tasks[&id].state == TaskState::Reported
    })
    .await;
    flow.settled().await;
    flow.stop().await;
}

/// Stop, archive and delete end an open terminal and wait for it, with nothing handed back,
/// even as its process ends on its own at the same moment.
#[tokio::test]
async fn stop_archive_and_delete_end_the_terminal_without_handing_back() {
    for how in ["stop", "archive", "delete"] {
        let (flow, host, heard, _) =
            start(&format!("takeover-{how}"), ProviderKind::Codex, 1_000).await;
        let id = task_id(&flow).await;
        let opened = flow
            .manager
            .open_worker_terminal(id.clone(), 80, 24)
            .await
            .unwrap();
        let specs = worker_specs(&flow).len();
        // The process ends on its own as the user's Stop comes.
        let ending = {
            let host = host.clone();
            let terminal = opened.terminal_id.clone();
            tokio::spawn(async move { host.exit(&terminal) })
        };
        match how {
            "stop" => flow.manager.stop_task(id.clone()).await.unwrap(),
            "archive" => {
                flow.manager
                    .archive(flow.conversation.clone())
                    .await
                    .unwrap();
            }
            _ => flow
                .manager
                .delete(flow.conversation.clone())
                .await
                .unwrap(),
        }
        ending.await.unwrap();
        flow.manager.cleanup_finished(&flow.conversation).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert_eq!(
            worker_specs(&flow).len(),
            specs,
            "{how}: nothing handed back"
        );
        assert_eq!(heard.lock().unwrap().len(), 1, "{how}");
        if how != "delete" {
            let task = &flow.board().await.tasks[&id];
            assert!(task.state.is_final(), "{how}: {:?}", task.state);
            assert!(
                task.takeover.as_ref().is_none_or(|t| t.ending.is_some()),
                "{how}"
            );
        }
        flow.stop().await;
    }
}

/// After a restart a taken-over worker keeps its worktree and session: the terminal's process
/// that survived is ended, and the session is handed back; a Stop that was under way finishes
/// instead.
#[tokio::test]
async fn a_restart_ends_the_terminal_and_hands_back_or_finishes_the_stop() {
    for ending in [false, true] {
        let (mut flow, host, heard, _) = start(
            &format!("takeover-restart-{ending}"),
            ProviderKind::Claude,
            1_000,
        )
        .await;
        let id = task_id(&flow).await;
        let survivor = std::process::Command::new("sleep")
            .arg("600")
            .spawn()
            .unwrap();
        let pid = survivor.id();
        *host.process.lock().unwrap() = Some(pid);
        flow.manager
            .open_worker_terminal(id.clone(), 80, 24)
            .await
            .unwrap();
        let native = flow.board().await.tasks[&id]
            .native_session
            .clone()
            .unwrap();
        if ending {
            let task = flow.board().await.tasks[&id].clone();
            flow.manager
                .update_task(&task.conversation_id, &id, |t| {
                    t.takeover.as_mut().unwrap().ending = Some("Stopped".into());
                })
                .await
                .unwrap();
        }
        let worktree = flow.board().await.tasks[&id]
            .workspace
            .as_ref()
            .unwrap()
            .worktree
            .clone()
            .unwrap();
        if !ending {
            let task = flow.board().await.tasks[&id].clone();
            let (held, sent) = flow
                .manager
                .message_worker(
                    &task.conversation_id,
                    &task,
                    "Also add a title.".into(),
                    "the orchestrator",
                )
                .await
                .unwrap();
            assert!(!sent, "{held}");
        }
        let specs = worker_specs(&flow).len();
        flow.restart().await;
        flow.manager.set_terminal_host(host.clone());
        // The launch sweep ended the survivor (it is our child here: reap it).
        let mut survivor = survivor;
        let status = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Some(status) = survivor.try_wait().unwrap() {
                    return status;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the terminal's process ended");
        assert!(!status.success());
        if ending {
            let board = flow
                .until("the stop to finish", |board| {
                    board.tasks[&id].state.is_final()
                })
                .await;
            assert_eq!(board.tasks[&id].state, TaskState::Stopped);
            assert_eq!(worker_specs(&flow).len(), specs);
        } else {
            assert!(
                std::path::Path::new(&worktree).exists(),
                "the worktree stays"
            );
            flow.until("the hand-back's report", |board| {
                board.tasks[&id].state == TaskState::Reported && heard.lock().unwrap().len() == 2
            })
            .await;
            let after = worker_specs(&flow);
            assert_eq!(
                after.last().unwrap().origin,
                Origin::Resume { native_id: native }
            );
            let last = heard.lock().unwrap().last().cloned().unwrap();
            assert!(
                last.contains("Also add a title."),
                "the message held before the restart: {last}"
            );
        }
        flow.stop().await;
    }
}

/// A hand-back whose resume fails ends the task as a failure, and nothing hangs: the failure
/// disposes of the task, which takes the takeover's reservation again.
#[tokio::test]
async fn a_hand_back_that_cannot_resume_fails_the_task() {
    let (flow, host, _, _) = start("takeover-resume-fails", ProviderKind::Claude, 1_000).await;
    let id = task_id(&flow).await;
    let opened = flow
        .manager
        .open_worker_terminal(id.clone(), 80, 24)
        .await
        .unwrap();
    let native = flow.board().await.tasks[&id]
        .native_session
        .clone()
        .unwrap();
    super::REFUSED_RESUMES.lock().unwrap().push(native.clone());
    host.exit(&opened.terminal_id);
    let board = flow
        .until("the task to fail", |board| {
            board.tasks[&id].state.is_final()
        })
        .await;
    assert_eq!(board.tasks[&id].state, TaskState::Failed);
    assert!(board.tasks[&id].takeover.is_none());
    super::REFUSED_RESUMES
        .lock()
        .unwrap()
        .retain(|refused| *refused != native);
    flow.stop().await;
}

/// A hand-off decided for the headless session before the user opened it in a terminal (an
/// error, a usage limit), or a reroute after a wait, leaves the terminal's task alone.
#[tokio::test]
async fn a_hand_off_decided_before_the_open_leaves_the_terminal_alone() {
    let (flow, host, _, _) = start("takeover-stale-handoff", ProviderKind::Claude, 1_000).await;
    let id = task_id(&flow).await;
    let live = flow.manager.existing_task_live(&id).unwrap();
    let decided_for = live.generation().await;
    flow.manager
        .open_worker_terminal(id.clone(), 80, 24)
        .await
        .unwrap();
    let specs = worker_specs(&flow).len();
    flow.manager
        .hand_off(
            &live,
            crate::work::AttemptEnd::Error {
                kind: brigadier_providers::model::ErrorKind::Auth,
                message: "logged out".into(),
            },
            decided_for,
        )
        .await;
    let task = flow.board().await.tasks[&id].clone();
    flow.manager.continue_task(&live, task).await;
    flow.settled().await;
    let board = flow.board().await;
    assert_eq!(board.tasks[&id].state, TaskState::TakenOver);
    assert!(board.tasks[&id].takeover.is_some());
    assert!(
        board.tasks[&id]
            .attempts
            .last()
            .is_some_and(|attempt| attempt.ended_at_ms.is_none()),
        "its attempt goes on"
    );
    assert_eq!(worker_specs(&flow).len(), specs, "nothing ran it headless");
    assert!(host.terminated.lock().unwrap().is_empty());
    flow.stop().await;
}
